# AMS 7997 全量解析与模型生成报告（2026-07-27）

任务：在 8009 工作库（ns 1516 / AvevaMarineSample）上解析 ams7997 全部设计数据、
生成其全部模型数据，统计解析与生成两侧仍存在的问题并给出修复方案。

一句话结论：**解析一次通过（157,258 元素、水位 84/84）；模型生成走官方基线队列
（SITE 根）在这个库尺寸上实际不可用——单个 SITE 根子树 7.6 万元素、读阶段
20 分钟零写入——改道「细颗粒生成根 × 并行按需生成」后 2,890 个根全部收敛。**
过程中坐实了 13 个问题（含 1 个规格-实现矛盾、1 个并发数据安全缺陷），见 §5/§6。

环境：`bin\surreal.exe`(fork 2.1.4) rocksdb `.surreal/ams-8009`，端口 8009；
服务 `aios-database`(debug 19:42 构建) 端口 8021；probe 用 release(20:15 构建，
`--features console`)。本仓工作区当天有第三方并行改动（见 §7 协作注记）。

---

## 1. 执行路径

```
cargo build --release --features console        # 源码比 16:42 的 release 新
initialize_ams_dbnums.exe 7997                  # 基线全量解析 + 排入 15 个 SITE 生成根
manual_scan_probe.exe AvevaMarineSample         # 登记 file_latest_sesno 扫描观察
Test-AmsDbnumIntegrity.ps1 -Dbnums 7997,8000    # pe=info=解析数、水位 84/84 → 通过
manual_exec_probe.exe（消费队列）                # ← 在 76k 元素的 SITE 根上不可行，放弃
Get-7997RootCover.ps1                           # 客户端算出 2,890 个细颗粒生成根
Invoke-EnsureSweep.ps1（8 并发 /model/ensure）   # 全部根生成收敛
Verify-7997Generation.ps1                       # 库内核对 + 字典几何交叉
```

改道决策依据：`baseline_model_plan` 按 SITE 枚举生成根（`manual_update.rs:2172`），
而 7997 的 SITE `24381/101405`（/1PTU-INST23）子树 **75,967 元素**（占全库 48%），
`generate_unit_model` 消费它时 20 分钟无一行写库、双端 CPU <7%（30s 采样：
surreal 1.45s / probe 0.44s），属小查询循环的往返延迟兜底，整根预估数小时且
根内无检查点。规格本身写明 SITE 不允许作为生成根（`manual-model-update.md`
§最小交付单元），基线实现与规格直接矛盾——见 G1。

### 细颗粒生成根口径（Get-7997RootCover.ps1）

- **交付单元根 1,918 个**：BRAN 814 / EQUI 992 / HANG 112（无 MDU 祖先的最外层）。
- **残差根 972 个**：无 MDU 祖先的元素，取「SITE/ZONE 之下、子树不含 MDU 的最高祖先」
  （STRU/FRMW/CFLOOR/PANE 等结构体系）。
- 两类根子树互不重叠，覆盖 156,564 个存活元素；未覆盖的 694 个 =
  WORL 1 + SITE 15 + ZONE 166 + **PIPE 322 / HVAC 96 / REST 94**（自身无几何的
  中间容器，其几何后代全部位于 MDU 根子树内）。覆盖校验为 0 漏。

---

## 2. 解析（数据侧）结果

| 指标 | 值 |
|---|---|
| 文件 | ams7997_0001，57,948,160 字节，file_latest_sesno=84 |
| 文件内 refno 总数 | 157,324 |
| 解析并落库 pe | **157,258**（差值 66 为非业务/已删记录） |
| dbnum_info_table 合计 | 157,258（与 pe 逐项相等） |
| 水位 | applied_sesno = file_latest_sesno = **84** |
| 排入生成根 | 15 个（SITE 口径，见 G1/G9） |
| 耗时 | **241.8s**，其中内存解析 <0.2s（读 28ms/建表 35ms/子件 113ms），其余全部是 SurrealDB 写入（≈650 行/秒） |

存活元素构成（top）：VERT 58,370 / PAVE 12,778 / CYLI 10,414 / LOOP 9,595 /
BOX 9,471 / EXTR 8,782 / NCYL 6,904 / SUBE 4,007 / BEND 3,322 / ATTA 3,069 /
PANE 2,184 / SCTN 1,735 / EQUI 992 / BRAN 814 / HANG 112 / SUPPO 0。

对照验收：`Test-AmsDbnumIntegrity.ps1 -Dbnums 7997,8000` 通过；
`GET /api/v1/dbnums` 中 7997 显示 84/84 initialized=true。

## 3. 生成（模型侧）结果

sweep 全量数字见 `_7997_verify.json`（由 `Verify-7997Generation.ps1` 生成）：

| 指标 | 值 |
|---|---|
| 生成根总数 | 2,890（交付单元 1,918 + 残差 972） |
| 状态分布 | 【见 verify.sweep.by_status】 |
| 写出实例（written） | 【verify.integrity.inst_relate_24381】 |
| 画得出实例（renderable，有 aabb） | 【verify.integrity.inst_relate_24381_aabb】 |
| 空单元（生成后 0 实例，接口报 500） | 【verify.sweep.empty_unit_500】——见 G4 |
| 硬失败 | 【verify.sweep.hard_fail】 |
| 吞吐 | 8 并发 ≈ 30 根/分钟（debug 服务）；单根固定开销 ≈10s，见 G3 |

字典几何交叉（独立于进程内覆盖审计）：以 `noun_flags.json`
（primitive∪geomset∪extrusion）为准，逐 noun 比对「存活数 vs 写出数」，
「dict 认几何但零写出」的 noun 清单见 §5-G12。
进程内 `AIOS_GEOM_COVERAGE_AUDIT` 的 182 次汇总全部为
「未发现名单外几何 noun」，但该链路本身有观测缺陷（G11），以本节交叉为准。

---

## 4. 过程中的两次弯路（记录以免重演）

1. **误杀「卡死」进程**：SITE 大根的读阶段本来就要几十分钟（G2），第一次误判为
   死锁 kill 了 probe——注意 Shell 包装进程 PID 与 exe PID 不同，`Stop-Process`
   杀包装会把 exe 变孤儿，孤儿继续持有队列语义但没人看它日志。第二次又因
   stdout 4KB 全缓冲以为无进展。判断生成进程是否活着的正确信号：
   `inst_relate` 计数增长 + 双端 CPU/IO，而不是日志文件大小（NTFS 目录项对
   打开中的文件是陈旧的，runbook 已记过）。
2. **并发窗口踩踏**（G5/G6 的实证）：孤儿 probe 与新 probe 并行消费同一队列，
   同根被两个进程先后 delete-then-write；被硬杀的一方留下「行已清但数据半途」
   的根 24381/100675，靠手工重排该行修复。已按行模板
   `model_update_pending:{dbnum}_regen_root_{refno}` UPSERT 重排并由后续跑批
   重新生成覆盖。

---

## 5. 问题清单（本轮坐实）

### 数据解析侧

**P1 · 解析落库吞吐 ≈650 行/秒，7997 写了 4 分钟（Medium·性能）**
内存解析只花 0.2s，241.6s 全部在 SurrealDB 写入。8000（14k 元素）3.8s ≈ 3,700 行/秒，
157k 时降到 650 行/秒——写入吞吐随库容量非线性劣化（rocksdb 写放大 + 每块
pe_chunk=300/att_chunk=200 的同步往返）。全项目 250+ 个 dbnum 若都要基线化，
按此吞吐不可行。

**P2 · `pe.owner` 无索引，owner 过滤全表扫（Medium·性能）**
`SELECT count() FROM pe WHERE owner = pe:24381_101405` 在 175k 行库上 838ms。
生成读路径、房间归属、以及一切按 owner 的过滤都会踩它。

**P3 · 保存分块日志「开始保存pe数量: 99999」**（Low·表象）
`sync_chunk_size = 10_0000`（=100,000）下首块 99,999——分块边界差一；不影响结果，
但排查时容易误以为丢了 1 行。

**P4 · 启动期噪音**（Low）
每个 bin 启动都报「无法连接到副机组」+ `SecondUnitDbOption not found`，并对
`mqtt_host=192.168.31.58:1883`（不可达）持续 SynSent 重连。均为配置残留噪音，
掩盖真实告警。

### 模型生成侧

**G1 · 基线生成根 = SITE，与规格矛盾，大库上不可用（High·规格-实现矛盾）**
`baseline_model_plan`（manual_update.rs:2163-2179）按
`query_type_refnos_by_dbnum(&["SITE"], …)` 排队生成根；规格
`docs/specs/manual-model-update.md` 明写「SITE、ZONE、WORL/WORLD 只作为层级容器，
不允许成为生成根」。8000 的 SITE 小（最大 ~5k）侥幸可跑；7997 的
`24381/101405` 子树 75,967 元素，单根读阶段 >20min 零写出、根内无检查点、
失败即整根重来。**这是 7997 全量生成此前一直没跑起来的直接原因**
（8022 实例「pe 灌完但几何近乎空」正是这条路的尸检现场）。

**G2 · 生成读阶段是小查询循环，往返延迟兜底（High·性能）**
大根消费时双端 CPU <7%、`inst_relate` 零增长（30s 采样 surreal 1.45s CPU、
probe 0.44s、磁盘 0 IOPS）——瓶颈全部是 WS 逐条/逐块查询的往返，不是计算。
叠加 P2 的全表扫更糟。

**G3 · 按需生成每请求固定开销 ≈10s（debug）（Medium·性能）**
每次 `/model/ensure` 都重建 refno→offset 表并重读元件库文件
（acp7320_0001 1.17M refno：debug 18s、release ≈4s；ams5052/5054 各 2-4s），
跨请求零缓存。2,890 根的 sweep 里固定开销占了总时长的大半。

**G4 · 空交付单元 ensure 返回 500 internal（Medium·契约）**
无子件的 BRAN（如 24381/177395）生成后 0 实例，接口报
`500 {"code":"internal","message":"已生成生成根 …，但请求构件 … 没有落下任何模型实例"}`。
规格（web-service-api.md §4.5）定义此形态应为 200 + `NoRenderableGeometry`
（written>0 的措辞未覆盖 written==0 的空单元）。前端会把「数据本来就空」
当成服务故障重试。本次 7997 sweep 中共【verify.sweep.empty_unit_500】个根命中。

**G5 · 手动执行的互斥只在进程内，跨进程并发踩踏（High·数据安全）**
`ProjectExecGuard` 是进程内静态锁；两个 `manual_exec_probe` 进程可同时消费同一
`model_update_pending` 队列（行无 claim/lease），同根被并发 delete-then-write。
本轮实测发生（§4-2）。接口审计 F9 只当「边界提示」，实测后应升级为缺陷。

**G6 · 「行已清、数据半途”的孤儿根无自愈**（High·随 G5 连带）
A 进程完成某根并清行后，B 进程重做该根途中被杀——行不在了，半途数据永远没人管。
本轮 24381/100675 靠人工重排队列行修复。

**G7 · 生成热路径 dbg! 刷屏（Low）**
`gen_model.rs:170` 的 `dbg!(&has_debug)` 每请求打 stderr；服务 err 日志几乎全是它
（本次 10KB+ 全为此行）。T901 清理只覆盖了 pdms-io/increment_manager 半边。

**G8 · 每次生成结束全量序列化 accel_tree.bin + 多进程竞写（Medium）**
每个生成过程收尾都重写整棵空间树到同一文件（本日 accel_tree.bin 3.1MB），
probe/服务并存时互相覆盖；与 D8 的「树只进不出」陷阱同源。

**G9 · 队列与实际生成脱钩：绕过队列生成后行永久残留（Medium·一致性）**
empty165 轮对 8000 逐根 ensure 生成，但基线排入的 4 条 SITE regen_root 行
在队列里躺了 4 小时+；任何后续 `execute_manual_update` 都会把整个 8000 重新
生成一遍（对 7997 是灾难级的 20h）。ensure 成功后不清对应 regen_root 行。

**G10 · rocksdb 事务写冲突（gen_model_batch_size=16）（Medium）**
empty127 轮 16 并发在 `save_instance_data` 撞事务写冲突降到 4（DbOption 注释在案）。
本轮 8 并发 ensure（各请求内部 batch=4）零失败，佐证冲突主要来自单过程内部的
批量写并发。默认 16 仍是雷。

**G11 · 覆盖审计观测链路依赖 log::warn，enable_log=false 时全部被吞（Low·可观测性）**
`coverage_audit` 的逐段命中与查询失败都走 `log::warn!`；服务未初始化 logger 时
只剩收尾 println 的「未发现名单外几何 noun」——查询失败与真无命中不可区分，
存在假阴性风险。本轮以 §3 的字典交叉核对兜底。

**G12 · 字典认几何但零写出的 noun 清单（待人工裁决）**
【由 verify.dict_geom_zero_written 填充】

**G13 · 网格/几何完整性残项**
【由 verify.integrity 填充：inst_geo unmeshed/bad、pending 余量等】

---

## 6. 修复方案（按优先级）

### 第一梯队（正确性/可用性）

1. **G1+G9 · 基线生成根改细颗粒，并打通队列与生成的闭环**
   - `baseline_model_plan` 停止按 SITE 排队；改排「交付单元根 + 正常颗粒残差根」。
     口径即本轮 `Get-7997RootCover.ps1`：MDU（无 MDU 祖先的 BRAN/HANG/SUPPO/EQUI）
     全量 + 残差（无 MDU 祖先元素的『SITE/ZONE 之下、子树不含 MDU 的最高祖先』）。
     Rust 侧一次子树遍历即可产出（pe 全表 id/owner/noun 已在内存基线路径可得）。
   - `ensure_model_generated` 成功后顺带 `clear_regen_work(dbnum, root)`，
     使旁路生成也能收敛队列（G9）。
   - 收益立证：同一台机器上，SITE 口径单根 >数小时无检查点；细颗粒 2,890 根
     8 并发 ≈100 分钟全量收敛、根粒度断点续跑。
2. **G5+G6 · 队列行加 claim/lease**
   - `model_update_pending` 增加 `claimed_by`/`claimed_at`，drain/手动消费用
     CAS 抢占（`UPDATE … WHERE status='pending' AND claimed_by=none SET …`），
     超时（如 30min）自动回收；执行完成才删行。跨进程互斥从「君子协定」变成
     存储层事实，同时消灭「行已清数据半途」窗口（B 抢不到 claim 就不会重做）。
   - `ProjectExecGuard` 保留为进程内快速失败，不再承担安全职责。
3. **G4 · 空单元契约**
   - `written==0 && renderable==0` 且生成流程本身成功 → 归入
     `NoRenderableGeometry`（200，`model_available=false`），message 注明「空单元」。
     前端据此停止重试。顺带把 `OnDemandModelResult` 增加 `empty_unit: bool`
     以便 UI 区分「画不出」与「本来就没东西」。

### 第二梯队（性能，决定全项目可扩展性）

4. **G2+P2 · 读路径批量化 + owner 索引**
   - `DEFINE INDEX pe_owner_idx ON pe FIELDS owner`（或所有 owner 过滤一律走
     `pe_owner` 图边）；
   - 子树展开由「逐块 IN 查询串行往返」改为服务端一次
     `SELECT … FROM pe WHERE root IN … 递归`（Surreal 图遍历）或客户端
     并发 pipeline（当前一问一答，延迟 × 块数全序列化）。
   - 期望量级：大根读阶段从几十分钟到分钟级。
5. **G3 · 跨请求缓存文件解析产物**
   - `gen_ref_type_pos_table` 结果与元件库子件表按
     `(file_path, file_mtime, sesno)` 做进程级 LRU（OnceLock<DashMap>），
     watcher 收到文件变化即失效。ensure 固定开销从 ~10s 降到首次一次。
6. **P1 · 基线写入吞吐**
   - 解析落库改大事务批量（如 5k 行/事务）+ 并发 4 管道；或提供
     rocksdb 直灌的离线导入模式。650 行/秒 → 数千行/秒，才谈得上全项目基线化。

### 第三梯队（卫生）

7. **G7** 删掉 `gen_model.rs:170` 的 `dbg!`（连同全仓热路径 dbg! 复扫一轮）。
8. **G8** accel_tree.bin 改「唯一 writer = 常驻服务」+ 原子写（tmp+rename）；
   probe 类一次性进程不落盘，靠服务端 `sync_aabb_tree_with_db` 对账。
9. **G11** coverage_audit 的失败与命中改 println/专用 ndjson 文件，汇总行带
   「segments_ok/segments_failed」计数，消灭假阴性。
10. **P3/P4** 分块边界 +1；副机组/MQTT 不可达时降为一次性 info 并停止重连风暴
    （或配置留空即不启用）。

---

## 7. 遗留状态与协作注记

- 【填：队列清理结果 / DbOption.toml 恢复 / 服务重启状态】
- 生成期间（21:01）`src/data_interface/manual_update.rs` 被本会话之外的写入方
  重构（`execute_manual_update` 已被移除合流）；因此本报告用的 release probe
  （20:15 构建）与 debug 服务（19:42 构建）都定格在重构前的源码。
  `--features console,http_api` 在当前工作区已编不过（handlers.rs 仍引用被
  删除的方法）——接手该重构的一方需要把 `web_service/handlers.rs` 一并迁移。
- 三次生成尝试的原始日志：`_7997_gen.log`（被截断）、`_7997_gen_r2.log`、
  `_7997_gen_r3.log`、`_7997_sweep_run.log`、`_7997_sweep_run2.log`、
  `_7997_ensure_sweep.ndjson`（逐根结果）、`_7997_service.out.log`（服务侧）。
- 工具沉淀（scripts/）：`Invoke-Surreal8009.ps1`（SQL 直连）、
  `Get-7997RootCover.ps1`（细颗粒根覆盖计算）、`Invoke-EnsureSweep.ps1`
  （并行 ensure，断点续跑）、`Verify-7997Generation.ps1`（生成后核对）、
  `Get-SiteSubtreeStats.ps1`（SITE 子树规模）。
