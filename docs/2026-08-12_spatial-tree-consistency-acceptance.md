# 验收记录：空间树一致性闭环（V2 快照 / 状态机 / 串行锁）

日期：2026-08-12
方案：`docs/plans/2026-08-12-spatial-tree-consistency-closure-plan.md`（D1–D8）
状态：**沙箱子集已实测通过；E3D 侧场景待生产空窗执行（runbook 见 §4）**

## 1. 自动化测试（本机全绿）

- `cargo test --lib --features http_api`：**678 通过 / 0 失败**（81 ignored 为
  live/manual 用例），含本方案新增：
  - 启动真值表（D2 交叉项：快照损坏 + pending → Rebuild；指纹一致 + pending →
    Replay）`startup_action_truth_table`；
  - V2 编解码矩阵（round-trip / 截断 / 错项目 / 错 namespace / 哈希与载荷篡改 /
    条目数篡改 / 版本不符）`snapshot_v2_validation_matrix`；
  - V2 损坏不回落旧格式 `corrupt_v2_snapshot_never_falls_back_to_legacy`；
  - 重建协议钉（stamp 先读、分页锁外、复核换树发布锁内、重试有界、耗尽落
    DegradedBlocked）`rebuild_protocol_reads_outside_and_swaps_inside_the_serial_lock`；
  - 扫描口径钉（current-only / record-range / 无 LIMIT-START）
    `pointer_scan_page_sql_pins_the_current_only_scope`、
    `aabb_usability_rejects_nan_inf_and_reversed_ranges`；
  - 锁序钉（direct 两取锁点 serial→tree、删除分支 serial→probe→bump 事务→摘树）
    `direct_paths_take_the_spatial_serial_lock_before_the_tree_lock`、
    helper 侧删除钉扩展；
  - 消费者门禁钉（房间轮状态门先于 pending 检查、drain 入口门、覆盖率闸门
    状态门 + ReadyEmpty 放行）；
  - 状态机/发布门/错误码/迁移分支/revalidator 边界钉；
  - /health 十五键契约（G-02 迁移）
    `spatial_tree_status_keeps_its_fifteen_key_shape_in_both_branches`。
- **fork 双跑**：`dual_pointer_scan_pagination_agrees` 在嵌入 mem 引擎与
  `bin/surreal.exe`（fork 2.1.4，rocksdb 后端一次性实例）上双跑通过——
  record-range 分页页间无漏无重（页长 7 不整除 40，每页区间起点剔重）、
  版本化数组 id 行 / `in.deleted` 软删行 / 无指针行被谓词排除、
  单页整表扫描与多页扫描同集合。

## 2. 沙箱实测（testbed @8019，`python/testbed/spatial_acceptance_probe.py`）

环境：`Start-TestSurreal.ps1`（fork 2.1.4 rocksdb）+ 7997 基线在位；
`maturin develop` 装最新绑定；树文件落隔离 cwd
`python/testbed/out/spatial-acceptance/`（不触碰仓库根生产工件）。
每个场景一个子进程 = 一次「服务重启」。六场景全过：

```text
A 无快照首启      verdict=rebuilt，state=ready，entries==usable==17，invalid=0，
                  format_version=2 + sha256 在场，快照文件落盘        （2.9s/次会话）
B 正常重启        verdict=reused（快路径），条目一致、快照哈希未变（无重建重写）、
                  drift=false、pending=0                              （2.8s）
C 截断快照        校验失败 → verdict=rebuilt，条目收敛                 （2.9s）
D 删除快照        verdict=rebuilt，快照重新在场                        （2.9s）
E 崩溃窗口③注入   AIOS_FAILPOINT=spatial_snapshot_tmp_written：
                  子进程 abort（exit 3221226505），正式快照未出现（rename 未发生）
F 崩溃后重启      verdict=rebuilt，收敛到同一规范化集合（17 条、无漂移），
                  快照重新发布                                        （2.9s）
```

会话耗时 ≈2.9s 为 full_init 全量（连接 + 自检 + 索引 + 装载）；沙箱树仅 17 条，
装载/重建本身毫秒级——AMS 6 万条级的耗时与峰值内存记录归 §4 生产侧验收。

### 实测中确认的边界（已写进 ADR-010 增补（二））

**跨进程「同一集合」不能比快照载荷字节。** F 场景最初断言重建后
`snapshot_sha256` 与基线相等，实测不等：`AccelerationTree` 序列化含 HashMap 段，
迭代顺序随每进程 SipHash 种子变化，同一集合两次重建的字节必然不同。
`tree_sha256` 的职责是**单文件完整性自校验**（写入什么、读出什么，C 场景验证的
正是它）；跨进程集合对拍走 entries/usable 口径与 Rust 侧 e2e 的逐边比对。

## 2b. 真实 ams8000 数据实测：启动矩阵 + 增量回放（testbed @8072，15/15）

`python/testbed/spatial_tree_8000.py`（配置 `DbOption-spatial8000.toml`，一次性内存
SurrealDB @8072，项目库文件临时换成 issue-019 的 sesno-24 基线快照、结束逐字节
还原）。与 §2 的探针互补：§2 验 V2 快照文件生命周期，本轮在**真实 db8000 会话
链**上验启动裁决矩阵与「真实删除 → 摘树 → epoch 留痕」，每窗之后再重启断言
reused。2026-08-12 14:25 实跑 `--max-windows 4`，**15/15 全过**
（`.spatial8000/report.json`，逐阶段日志同目录）：

| 阶段 | 断言要点 | 实测 |
|---|---|---|
| probe | 真实文件血统含 sesno 24..26 及两次已知删除 | latest 209，链自 sesno 2 起连续 |
| P0 prepare | 基线水位=24、样本生成、树=指针、落 V2 | entries=3，epoch=3 |
| S1 快照新鲜 | verdict=reused，条目不变 | ✅ |
| S2 快照缺失 | verdict=rebuilt | ✅ |
| S3 库侧 epoch 漂移 | verdict=rebuilt | ✅（epoch 3→4 后重启认出失配） |
| S4 携带待重放意图 | verdict=replayed，重放后 pending=0 | ✅（伪造 pending 行与 Rust `record_id`/字段面逐项同构） |
| S5 快照字节损坏 | verdict=rebuilt | ✅ |
| W25 删 BOX（真实会话） | 水位=25、pe 软删、几何清零、**epoch 严格递增** | entries 3→2，epoch 5→6 |
| W26 删 EQUI | 水位=26；EQUI 树上本无条目，「树应有内容」未变**不 bump** | entries=2，epoch=6 ✅ |
| W27/W28 | 无空间变更窗口：水位推进、epoch 单调不减 | ✅ |
| 每窗之后重启 | verdict=reused（快路径，无重建） | 4/4 ✅ |

补充：`_pointer_count` 谓词已对齐重建/覆盖率的 current-only 口径（排除版本化
数组 id 行与软删行），对齐后 `--skip-windows` 快档复跑 7/7 通过
（`.spatial8000/report-scope-recheck.json`）。

## 3. 全链路冒烟的现状

`python/testbed/run_full_loop.py` 解析层通过（7997 副本 103..104 窗口
collect_changes 正常）；`full_init` 被**活服务探测**按设计拒绝——本机 9099 正在
伺候同一工程（`AvevaMarineSample`），执行层与它并发会互踩暂存窗口/队列/pending
表。不停生产服务，全链路冒烟与下列 E3D 场景一并排入生产空窗执行。

## 4. 待执行 runbook（E3D / AMS 8000 侧，需生产空窗 + E3D 客户端）

前置：停 9099/8022 在跑服务；用**新二进制**起服务（或 testbed 沙箱 +
`run_full_loop.py`）；E3D 打开 AMS 样例工程。

### 执行顺序：与 db8000 会话链录制共用一个空窗

db8000 夹具录制（`docs/plans/2026-08-12-db8000-session-snapshot-fixture-test-plan.md`
阶段二）与本 runbook 用同一批前置（空窗 + E3D + 停服务），一次开窗按序做完：

1. **录制排最前，期间 db8000 独占**：`scripts/e3d/Record-Db8000SessionChain.ps1`
   逐腿断言 sesno **恰好 +1**，任何并发写者当场判死整轮——所以先录制，本文
   第 6 条（TTY 复制恢复对拍，同样写 db8000）必须排在它之后。7 案例 12 条腿，
   预计窗口 211..约 222（宏纪律与 `-CheckOnly` 已过，baseline 复测 210）。
2. **当场打包**（离开 E3D 即可做）：`db_session_fixture pack --recording
   <输出目录>/recording.json --dbnum 8000 --out
   tests/fixtures/issues/issue-021-db8000-session-pair-suite`。zip 预算预估：
   final ≈17 MB 原始 × 同族文件实测压缩比 0.14 ≈ **2.4 MiB**，在 6 MiB 预算内
   （pack 有硬闸，超了会当场拒绝而不是入库超大文件）。
3. **断言零改动换数据**：`AIOS_SESSION_FIXTURE` 指向新夹具重跑
   `cargo test --test db8000_session_pairs`——七类断言一行不动、只换数据源，
   全绿即阶段二→三交棒完成（这是该设计成立的最终判据）。
4. 随后按下列 1–8 条跑本 runbook 其余场景（第 5 条已降为复核）。录制使 db8000
   前进约 12 个会话，对第 6 条的对拍无影响（对拍是重建前后相对断言）。

1. **全链路冒烟**：`python/testbed/run_full_loop.py`（解析 → 基线 → 生成 →
   房间/收尾），确认房间门禁不阻断 Ready 态的正常管线；`/api/v1/health` 的
   `spatial_tree.state == "ready"`、`pending == 0`。
2. **三方相等**：`usable_pointer_rows == entries == 快照 entries`（health 三字段
   互等；库侧对拍 `SELECT count() FROM inst_relate WHERE
   !type::is::array(record::id(id)) AND in.deleted != true AND world_trans.d !=
   none AND aabb.d != none GROUP ALL`）。
3. **快路径**：正常重启日志必须出现「空间树复用 V2 快照」，且无分页扫描日志；
   记录装载耗时与进程峰值内存（62k 条级基准）。
4. **旧格式迁移**：把旧版本产出的 `accel_tree_{project}.bin/.meta.json` 放回
   cwd、删掉 `.snapshot` → 启动 → `startup_verdict == "migrated"`，旧文件被删除，
   V2 在场。
5. **伪造旧 epoch**：改库 `spatial_epoch:current`（`UPSERT spatial_epoch:current
   SET value += 1, updated_at = time::now()`）后不动快照重启 →
   `verdict == "rebuilt"`（失配无 pending）。
   **沙箱档已覆盖**（§2b S3，真实 ams8000 数据实测 rebuilt）；生产侧本条降为
   对 62k 条级库的复核，顺带记录重建耗时。
6. **E3D TTY 复制恢复对拍**（`scripts/e3d/db8000_equi_copy_apply.mac` /
   `db8000_equi_copy_restore.mac`，`=24384/24776`）：增量收敛后
   `aios_db.spatial.rebuild()` 强制重建，对比重建前后 health `entries` 精确相等；
   房间增量与全量边集合对拍（`live_room_incremental_parity` 手法）；
   `pending / dead-letter / spatial_reconcile` 全部归零。
7. **崩溃窗口 ①②④⑤**（③ 已在沙箱过）：对增量流量分别注入
   `AIOS_FAILPOINT=spatial_direct_after_db_commit / spatial_after_tree_sync /
   spatial_after_publish_before_ack / spatial_rebuild_mid_scan`，杀后重启验证：
   ① 指纹失配 → rebuilt；② pending 在场 → replayed 且重放收敛；
   ④ 指纹相等 + pending → replayed（幂等追认后销账）；⑤ 旧快照在场走正常判据，
   配合注入期间的并发提交验证 stamp 漂移重试日志。
8. **降级恢复**：临时断库起服务 → `state == "degraded_reuse"`、房间轮被门禁且
   日志只播报一次；恢复库后 ≤5min revalidator 收敛 Ready 并唤醒调度器。

证据（日志、health 快照、耗时/内存数字）回填本文件 §2 之后。
