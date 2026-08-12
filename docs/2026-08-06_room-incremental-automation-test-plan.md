# 房间增量更新自动测试计划

日期：2026-08-06  
状态：实施草案（承接 `2026-08-06_e3d-l3-automation-test-plan.md` 的 F8）

> **2026-08-12 修订**：对齐 ADR-010 2026-08-09 修订与 ADR-017 §5 的现行实现。
> 本文初稿基于 2026-08-05 的暂存房间方案（「窗口提交前内联重算、room_relate 与
> 水位一起收口」），该方案已被取代——现行方案是：尾事务只把房间目标（durable
> pending）与空间意图、水位一起持久化；**提交成功后**在同一提交串行段内按
> 「空间树收敛 → 释放 kv-mem 窗口 → 本任务精确 scope 从 RocksDB 重算房间」的
> 固定顺序执行（`batch_worker::execute_staged_batch` →
> `model_update_pending::drain_rooms_scoped`）。本次修订只改 §0 流程、§1.2 缺口
> 4、RF12 期望、RI-2/RI-5 与 §5.1 观测字段的口径，场景矩阵与门禁结构不动。

## 0. 定位

本计划只补房间增量链路：

```text
数据/结构变化
→ 暂存窗内完成数据/regen 并归并 room_recalc_element / room_recalc_panel 目标
→ 尾事务把房间目标（durable pending）、空间意图与水位一起持久化
→ 提交成功后：空间树收敛 → 释放 kv-mem 窗口 → 本任务精确 scope 从 RocksDB 重算房间
→ 单目标失败保留 durable pending，数据批次记 partial，空闲房间轮继续恢复
```

不重写已有几何、队列和水位测试；底层语义以
`docs/adr/ADR-010-room-membership-incremental-update.md`（含 2026-08-09 修订：
房间后置到提交后 scoped drain）与 ADR-017 §5 为准。测试分两条轨：

1. **合成轨（L1/L2）**：内存 Surreal + `room_fixture`，无 E3D license，负责完整语义矩阵；
2. **实机轨（L3/V）**：E3D 宏 + 服务 HTTP + plant-ui，只保留用户可见的关键闭环。

唯一硬标准仍是：**增量收敛后的规范化边集合等于同数据上的全量重建结果**。RF8
只冻结当前 AABB 门控边界，不把“实体形状变了但 AABB 相等”伪装成已解决；要覆盖它必须先增加实体 hash 产品信号。

## 1. 已有资产与缺口

### 1.1 已有自动覆盖

| 能力 | 现有测试/资产 | 结论 |
|---|---|---|
| 正向/反向共享归属谓词、跨房多边 | `live_room_fixture_parity` | 已覆盖 |
| 元素 A→B 搬家，增量与全量逐边一致 | `live_room_incremental_parity` | 已覆盖 |
| 面板移动走整间分支 | `live_room_panel_move_parity` | 已覆盖 |
| 面板任务吸收同轮元素任务 | `live_room_panel_task_absorbs_element_task_in_the_same_round` | 已覆盖 |
| 跨面板搬家时禁止错误吸收 | `live_room_cross_panel_move_defeats_absorption` | 已覆盖 |
| 构件/面板删除双向清边、空间树清理 | `live_room_delete_clears_membership` | 已覆盖 |
| 被删边在同房移动后恢复 | `live_room_deleted_edges_come_back_after_a_move` | 已覆盖 |
| TUBI 首次进树、未动重生成、搬家 | `live_room_tubi_row_enters_tree_and_tracks_regen` | 已覆盖 |
| FRMW 改名、PANE OWNER 触发面板任务 | `live_room_structural_triggers_enqueue_panel_recalc` | 已覆盖计划层 |
| 改名成为合规房间并同窗提交两张关系表 | `live_room_rename_into_compliance_recomputes_membership` | 已覆盖暂存提交 |
| 真 AMS CAP 移动恢复归属 | `issue7_e2e_room_comes_back_after_e3d_save`、`issue7_cap_pos_*.mac` | 有手工/ignored E2E |
| L3+V 单场景 | `l3_suite` 的 F8 | 仅覆盖 CAP +100mm U |

### 1.2 当前自动化缺口

1. 合成 live 测试各自靠人工起 8071、逐条运行，尚无一键编排与统一报告；
2. F8 只判断 `room_relate` 载荷“有变化”，没有断言**准确 membership、排序载荷、拓扑表和恢复结果**；
3. 实机只测“仍在同一房间”，缺“移出房间后边消失、恢复后精确回基线”；
4. 正常暂存路径没有独立 `room_recalc` task（房间在提交后按本任务 scope 内联于数据批次执行），scoped `DrainReport`（requested/loaded/done/failures）目前只落在「写回后房间计算 …」日志行、warnings 与 partial 状态里，没有随 data task 结构化返回；durable pending 行虽带来源 `(dbnum, source_end_sesno)` 追踪字段，但 data task 侧缺结构化摘要，当前不能只靠 task JSON 可靠关联本轮房间工作；
5. `gen_spatial_tree=false`、房间预载失败、死信复活、房间轮饥饿等门控/故障语义只有散落单测；
6. plant-ui 当前只有房间任务泳道，没有可 inspect 的房间号/房间树 surface；截图只能证明几何和队列，不能证明 membership；
7. 正式 L3 manifest、E3D/Surreal 金基线尚未铸造，RL1–RL4 的水位、完整 target 集和结构 refno 仍待定标。

## 2. 场景矩阵

### 2.1 合成冒烟（每次房间相关改动必跑）

| ID | 操作 | 必须命中的分支 | 核心判据 |
|---|---|---|---|
| RS1 | 构件在 A 房内平移，预先删掉它的边 | `room_recalc_element` | 恢复为原一条边；其它 5 条不动；与全量一致 |
| RS2 | 构件从 A 搬到 B | `room_recalc_element` | A 边消失、B 边出现；载荷完整；与全量一致 |
| RS3 | 移动/扩缩 PANE A | `room_recalc_panel` | A 的全部出边重算；B 不受牵连；与全量一致 |

复用现有 `room_fixture`，不新增第二套夹具。

### 2.2 合成全量（发版/房间大改前）

| ID | 操作 | 主要风险 | 期望 |
|---|---|---|---|
| RF1 | PANE A 与其成员同轮变化 | 重复工作/互踩 | 面板先跑；封闭时元素被吸收；任务总数与去重后目标一致 |
| RF2 | 构件 B→A，同时仅 A 面板变化 | 错误吸收留下 B 陈旧边 | 元素分支仍执行；B 旧边删除；A 新边出现 |
| RF3 | 构件跨 A/B 重叠区 | 多归属与排序不确定 | 保留两条边；`inside_count DESC, center_dist ASC, room_num ASC` 决定首选 |
| RF4 | 删除普通构件 | 无新 AABB，触发器不点火 | 不入房间队列；所有 `room_relate` 入边与树条目立即删除 |
| RF5 | 删除 PANE | 双向边/拓扑悬空 | `room_relate` 出边、`room_panel_relate` 入边、树条目全部删除 |
| RF6 | FRMW 名称：不合规→合规→不合规 | 非几何结构触发遗漏 | 两块 PANE 入队；两张关系表随名称同窗建立/清空 |
| RF7 | PANE OWNER 从房间 A 迁到房间 B | AABB 不变、拓扑陈旧 | 该 PANE 走整间分支；旧/新房间拓扑均收敛 |
| RF8 | 普通重生成但 AABB 相等 | 当前产品边界被误当回归 | 当前应为零房间任务；未来只有引入实体 hash 后才把它升级为触发用例（RF9 已覆盖此负例） |
| RF9 | TUBI 首次进树→未动重生成→搬家 | 回填与幂等混淆 | 首次 1 条、未动 0 条、搬家 1 条；最终与全量一致 |
| RF10 | `gen_spatial_tree=false` | 静默丢工作 | 零房间 pending；health 明示关闭；数据/模型批次照常成功 |
| RF11 | 面板工作集预载失败 | 空集覆盖真边 | fail-closed：关系表不变、水位推进、目标保留 durable pending；去故障后原目标重试成功 |
| RF12 | 数据批次持续到达且 fallback 房间任务已有积压 | 饥饿或抢跑 | 正常暂存房间仍在提交后立即按本任务 scope 收敛（历史积压不搭车）；durable backlog 数据优先，到饥饿阈值后获得一次执行 |
| RF13 | 同 target 跨 dbnum/多次触发 | 重复整间重算 | record id 只按 action+target；revision 增长但仅一行 |
| RF14 | 房间任务失败到死信后人工 retry | 无恢复出口 | retry 复活原行、attempts 清零、revision 递增、worker 被唤醒；去故障后成功删除 |

### 2.3 实机 L3/V

| ID | E3D 操作 | 宏 | 判据 | V 级画面 |
|---|---|---|---|---|
| RL1 | 先删 CAP `24383/66460` 的现存边，再 `POS +100mm U`，仍在 R512 | 复用 `issue7_cap_pos_apply/restore.mac` | CAP membership key 恢复；新位置载荷等于全量结果；BRAN regen 带出的完整房间 target 集先定标，不假设只有 1 个；restore 后边/AABB 精确回基线 | 位置/几何变化；房间值先用库侧证据 |
| RL2 | 同 CAP `POS +100000mm U`，移出房间 | 新增 `room_cap_out_apply/restore.mac` | apply 后 CAP `room_relate=[]`；完整 target 集先定标；restore 后精确等于基线 | 位置/几何变化；房间值先用库侧证据 |
| RL3 | 合规 FRMW 政名为仍命中关键字但不合规，再恢复 | 新增宏对；目标在金基线铸造时定标 | 名下 PANE 全部走 panel action；两张关系表清空/恢复；零几何重生成 | 有 room inspect surface 后才启用房间树 V 断言 |
| RL4 | PANE OWNER 在两个合规房间间搬迁，再恢复 | 新增宏对；目标在金基线铸造时定标 | 目标 PANE 仅一行 pending；旧/新 topology 精确切换 | 有 room inspect surface 后才启用成员归属 V 断言 |
| RL5 | **跨库房间迁移**：CAP `24383/66460`（db7999）从 R512（房间/面板在 db7997）搬进 `/6KA-RM01-K101`（房间树在 db1112，SITE `/6KA-ARCH` → ZONE `/6KA-RM`），再恢复 | `room_cap_cross_db_apply/restore.mac`（已落地 2026-08-08，legacy 用例 `cross-db-room`） | apply 后归属边精确等于 `[(17496_230552,K101),(17496_230648,K170)]`——K170 公共区域体完全包含 K101，双归属为该区域常态（定标证据：K101 现有 3 成员中 2 个同时挂 K170 边）；restore 后精确回 `[(24381_35844,R512)]`；动态基线 + `-RM` 关键字（房间图须同时纳入 7997 与 1112 两库） | 位置/几何变化；房间值先用库侧证据 |

实机删除不另造场景：RF4/RF5 已在合成轨精确覆盖，真实删除由现有 M3 验数据/模型清理；只有找到“基线有房间边且可由金基线恢复”的稳定样本后再加删除场景。

RL5 定标记录（2026-08-08，8009 = `.surreal/ams-7997-e3d-test-20260805` 实测）：目标 POS 取 K101 面板
AABB 中心 `E -18685 N -16426.047 U -7375`（面板体 `[-23560,-23100,-9000]…[-13810,-9752,-5750]`）；
恢复 POS 与 `issue7_cap_pos_restore.mac` 同一黄金坐标。该点 AABB 候选恰为 K101 与 K170 两块面板。

RL1/RL2 当前 CAP 会规划所属 BRAN 的 `RegenRoot`，所以场景断言的是定标后的**完整 target 集**，不是“一个 CAP 对应一个 room target”。若后续找到稳定的纯 Transform 靶子，再用它替换 CAP 以缩短实机冒烟。

## 3. 统一不变量

| 编号 | 不变量 |
|---|---|
| RI-1 | 数据解析/提交硬失败时水位不动；仅房间计算失败时数据窗口和水位仍提交，同时留下 durable pending |
| RI-2 | 正常 DESI 房间工作在写回成功后、同一提交串行段内按本任务精确 scope 执行（顺序固定：空间树收敛 → 释放 kv-mem 窗口 → scoped room drain）；fallback 才由后续空闲房间轮处理 |
| RI-3 | AABB 变化按 noun 分发：PANE→panel，其它→element；AABB 未变的普通重生成当前不入队，实体 hash 触发另立产品改造 |
| RI-4 | pending record id 为 `room_recalc_{panel|element}_{target}`；同 target 只有一行 |
| RI-5 | 正常路径的 data task 必须带 `room:{requested,loaded,done,failures,duration_ms}`（写回后 scoped `DrainReport` 的结构化摘要；字段落地前以「写回后房间计算」日志行 + warnings + partial 状态为临时证据）；fallback 关联用 durable pending 行自带的 `(dbnum, source_end_sesno)` 与 `(action,target)`，禁止用“最新 task_id”猜关联 |
| RI-6 | 正常路径 room summary 零失败；fallback 逐轮 `done <= total`，允许分页 backlog，只要求最终一轮 succeeded 且 live=0；故障用例允许 failed |
| RI-7 | 本轮涉及的 room pending 收敛为 0；非本轮死信不得被误删或误判成功 |
| RI-8 | `room_relate` 使用规范化边：`panel, element, room_num, inside_count, center_dist`，排序后精确比较 |
| RI-9 | `room_panel_relate` 与房间→PANE 拓扑精确一致；结构变更必须同窗收敛两张表 |
| RI-10 | 删除后不存在指向/来自已删 refno 的两种房间边，也不存在空间树条目 |
| RI-11 | 第二次 drain/execute 为零工作，边集合和截图哈希均不变 |
| RI-12 | 合成轨最终 `incremental_edges == full_rebuild_edges`；不得只比较 count |
| RI-13 | apply 宏的 `Q POS/Q NAME/Q OWNE` 与 Surreal 的位置、名称、owner/room 边逐字段对拍 |
| RI-14 | 当前 V 只断言几何与任务泳道；房间号/房间树需先提供可 inspect UI surface，启用后 after 必须来自自动刷新，重启或手动全量重载不计 |
| RI-15 | apply 后无论哪条断言失败都必须进入 restore；restore 失败立即终止剩余破坏性 L3 场景 |

数值容差：坐标/AABB `0.01mm`；`center_dist` `0.01mm`。集合与 refno、room_num、inside_count 不设容差。

## 4. 合成轨编排

新增薄脚本 `scripts/Run-RoomFixtureSuite.ps1`，只做以下工作：

1. 校验 8071 空闲；
2. 用仓库 `bin/surreal.exe` 起 `127.0.0.1:8071 memory`；
3. 设置 `AIOS_LIVE_WS=ws://127.0.0.1:8071`；
4. 按场景串行运行 ignored live test（全局 `SUL_DB`、空间树与 mesh 文件不并发共享）；
5. 每条测试独立日志，失败不中断后续测试；
6. 终态清理 Surreal 进程与夹具 mesh；生成 `output/room-suite/<时间戳>/report.md`。

冒烟命令集：

```powershell
cargo test --lib fast_model::room_fixture::tests::live_room_deleted_edges_come_back_after_a_move -- --ignored --exact --nocapture
cargo test --lib fast_model::room_fixture::tests::live_room_incremental_parity -- --ignored --exact --nocapture
cargo test --lib fast_model::room_fixture::tests::live_room_panel_move_parity -- --ignored --exact --nocapture
```

全量直接复用 §2.2 对应的现有 test；RF8 由 RF9 的“未动重生成”负例覆盖，只为 RF10、RF11、RF14 的缺口各补一条最小测试，不拆新夹具、不引入测试框架。

## 5. L3 runner 扩展

### 5.1 场景数据

把当前粗粒度 `Expect::Room` 收紧为房间专用声明：

```rust
RoomExpect {
    setup: RoomSetup,                   // None | DeleteTargetEdges
    targets: &'static [RoomTarget],     // action + refno + before/after/restored rooms
    room_refs: &'static [&'static str],
    panel_refs: &'static [&'static str],
    topology_changes: bool,
    ui_anchor: Option<&'static str>,     // 有可 inspect surface 后使用
}
```

不做通用 SQL DSL；四个实机场景用同一个规范化 snapshot 函数即可。RL1 使用
`DeleteTargetEdges` 复刻 issue #7，且 `targets` 必须来自金基线定标，不能硬编码成单个 CAP。

执行 L3 前先补最小观测字段：

- `DataBatchTaskResult.room = { requested, loaded, done, failures, duration_ms }`
  （即写回后 scoped drain 的 `DrainReport`），承载提交后本任务房间收敛结果；
- fallback `room_recalc` task detail 增加 `sources:[{dbnum,end_sesno}]`（映射
  durable pending 行已有的 `dbnum` / `source_end_sesno` 追踪字段），承载
  durable pending 来源。

没有这两个字段时测试报告“证据不足”，不再用“本轮后新出现的 task id”猜关联。

### 5.2 每场景流程

```text
before：PE/inst/AABB/room_relate/room_panel_relate/pending
→ before.png
→ 执行 setup（RL1 删除目标现存 room 边）
→ E3D apply 宏及 Q 日志
→ preview（只断言数据/regen/transform 计划；room action 尚未在此阶段产生）
→ execute 数据批次
→ 等 data task terminal，保存其 room summary + queue.png
→ 若 summary 有失败：按来源窗口及 `(action,target)` 找 durable pending，再等 fallback room task 最终收敛
→ after 规范化快照 + after.png
→ 精确断言 RI-1…RI-15 的适用子集
→ 第二次 execute/drain + repeat.png/hash
→ finally 中执行 restore 宏，同链路反向执行并精确回基线；restore 失败则终止剩余 L3
```

删除当前 F8 无条件调用 `wait_room` 的路径：正常暂存执行成功后不会产生独立 room task，等待它会超时。只有 data task 的 room summary 报告失败且出现 durable pending 时，才进入 fallback task 等待；逐轮允许分页 backlog，保存每轮 running/terminal JSON，直到 live=0。

### 5.3 规范化快照

每次至少保存：

```sql
SELECT record::id(in) AS panel,
       record::id(out) AS element,
       room_num, inside_count, center_dist
FROM room_relate
WHERE out = pe:TARGET OR in IN [pe:PANEL_TARGETS]
ORDER BY panel, element;

SELECT record::id(in) AS room,
       record::id(out) AS panel,
       room_num
FROM room_panel_relate
WHERE in IN [pe:ROOM_TARGETS] OR out IN [pe:PANEL_TARGETS]
ORDER BY room, panel;

SELECT action, target_refno, attempts, revision, last_error
FROM model_update_pending
WHERE action IN ['room_recalc_element', 'room_recalc_panel']
  AND target_refno IN ['TARGET_REFNOS'];
```

禁止以“非空”“count 变了”代替逐边比较。房间 pending 使用 `dbnum=0`，必须按
record id 或 `(action,target_refno)` 查询，不能沿用场景 dbnum 过滤。

## 6. 视觉证据

每个 RL 场景沿用四件套：

```text
<id>-before.png
<id>-queue.png
<id>-after.png
<id>-repeat.png
```

另存：

- `data-task-room-summary.json`；fallback 实际发生时再存 `room-task-<round>-running.json` / `room-task-<round>-terminal.json`；
- `room-edges-before/after/restored.json`；
- `room-topology-before/after/restored.json`；
- `apply-macro.log` / `restore-macro.log`；
- `inspect-tree-before/after.txt`。

RL1 判库侧 membership 保持、载荷随位置更新；RL2 判库侧“清空→恢复”。当前截图只验几何与房间任务泳道。RL3/RL4 的房间树/房间号 V 门禁先保持关闭，等 plant-ui 提供可 inspect surface 后启用；after 截图前不得重启 plant-ui。

## 7. 故障与清理纪律

1. 合成轨仅用 8071 memory，不碰 8048/8009 工作数据；
2. 测试串行；失败也执行 `drop_room_fixture` 与进程树清理；
3. L3 只用已校验的金基线对；每个场景用 guard/finally 恢复，恢复失败立即中止剩余场景，不能靠“排轮尾”容错；
4. data task 与 fallback room task 超时上限 20 分钟；E3D 超时沿用 `l3_suite` 的 des/pdmsconsole 全家清理；
5. 预载失败必须保留原边，报告第一个失败判据为 RI-7/RI-9，禁止用空结果覆盖后继续；
6. report 记录本轮开始时的 `gen_spatial_tree`、房间关键字、panel 可用/缺失数和旧死信数。

## 8. 门禁

| 门禁 | 场景 | 时机 | 预算 |
|---|---|---|---|
| RG0 | 普通 unit tests | 每次提交 | ≤2 分钟 |
| RG1 | RS1–RS3 合成冒烟 | 房间相关 PR 合入前 | ≤10 分钟 |
| RG2 | RF1–RF14 合成全量 | 发版/房间大改前 | ≤30 分钟 |
| RG3 | RL1–RL2 实机 | 房间相关 PR 合入前手动跑 | ≤45 分钟 |
| RG4 | RL1–RL4 + V | 发版前 | ≤90 分钟 |

不把 E3D license 场景放入无人 CI；RG1/RG2 可进本机或专用 Windows runner。

## 9. 实施顺序

1. **P0：铸造并验证 L3 金基线对**——固化 7997/7999/8000 水位、RL1/RL2 baseline 边/拓扑/AABB、完整 target 集，以及 RL3/RL4 refno 和 inspect 锚点；
2. **P1：先编排已有合成测试**——新增薄 PowerShell 入口和 report，零 Rust 业务改动；
3. **P2：加 RF10/RF11/RF14 三个缺口测试**，仍复用 `room_fixture`；
4. **P3：给 data task/fallback task 补最小房间观测字段，修复 L3 finally restore，收紧 snapshot**；
5. **P4：把现有 F8 映射为带 DeleteTargetEdges setup 的 RL1；新增 RL2 宏对并跑通“清空→恢复”**；
6. **P5：定标两个结构样本后补 RL3/RL4**；plant-ui 有 room inspect surface 后再启用房间值 V 门禁。

第一实现切片仍只做 P1：它不依赖尚未铸造的实机金基线，把已经存在但依赖人工逐条运行的高价值测试变成一键套件，改动最小，也最快暴露当前合成轨是否真能稳定重复运行。P0 可并行准备，但在 P3/P4 前必须通过。

## 10. 轮次台账

| 日期 | 门禁 | 场景结果 | Surreal 版本/端口 | `gen_spatial_tree` | 证据目录 | 首个失败 RI |
|---|---|---|---|---|---|---|
|  |  |  |  |  |  |  |
