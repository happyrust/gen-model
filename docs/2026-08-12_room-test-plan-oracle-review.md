# 房间增量测试计划——Oracle 预审（O1）

- 日期：2026-08-12
- 评审人：GPT-5.5 Pro（oracle MCP，browser 引擎，session `room-testplan-o1-review-2`，20m41s，附件 4 份打包上传）
- 评审对象：`docs/plans/2026-08-12-room-incremental-live-test-plan.md`
- 语义基准：ADR-010、`docs/2026-08-06_room-incremental-automation-test-plan.md`（RS/RF/RL 矩阵、RI-1..RI-15）、`docs/2026-08-12_live-test-ledger.md`
- 送审时上下文：room_incremental 编译缺省当日翻 true 并部署 test-workspace；pytest 房间档 15 绿；
  Rust 房间夹具 live 11 条初跑发现两处测试基建问题（8019 覆盖率闸门、`mark_spatial_tree_fixture_preloaded` 缺失），修复中。
- **执行侧落地状态见文末附注。**

---

## 总评

Phase A 的结果说明核心算法路径已有较强基础，但**门禁尚未闭合**：15 条 pytest 与 9 条 parity 全绿不能替代"修复后 11 条 live_room 在全新 8071 上整批 11/11 通过并回填台账"。现计划仍写着 8019，且明确规定"任何一条红即停"；当前执行环境与文档已经分叉，必须先修正。

更严重的是，C0 本身已经是一次全库写操作：当前 `room_build:main` 从未盖章，首次启动全量会改写大量关系；而现有回滚方案只有环境变量和旧二进制，没有数据库或 E3D 源数据恢复方案。

---

# 1. C1–C8 场景矩阵缺口

## P0：必须在任何破坏性场景前补齐

| 编号 | 缺口 | 必须落地的改法与断言 |
| ----- | ----------------------------------------- | ------------------------------------------------------------ |
| P0-1  | **Phase A 尚未正式闭门** | 更新计划：room fixture 批次统一使用一次性空库 8071，不再写 8019；11 条用例必须从全新库整批 11/11 通过。每条 ignored live test 应独立进程执行，不能在同一 Rust 进程复用全局 `SUL_DB`。`mark_spatial_tree_fixture_preloaded()` 必须只出现在 fixture 初始化中；8009 的 `/health.spatial_tree.startup_verdict` 不得是 `preloaded`。 |
| P0-2  | **C0 缺少数据级回滚点** | 在 C0 前停止 9099 写入和 E3D 保存，做一次 8009 RocksDB/namespace 离线副本；最低限度也要导出全量 `room_relate`、`room_panel_relate`、`room_build:main`、`spatial_epoch:current`。旧的 78,438/497 只能作为取证值，不能作为 C0 后预期 count，因为 C0 是首次按新语义重建。 |
| P0-3  | **没有"同一输入"栅栏** | C0/C9 之前必须连续两个空闲轮满足：全部 `model_update_pending` 为空、`room_units.live=0`、`file_latest_sesno` 与应用水位不变、空间状态为 `Ready`、`drift=false`、没有 E3D 新存盘。两次规范化快照哈希必须相同，才允许重启。真实 8009 非空项目出现 `ReadyEmpty` 也应直接判红。 |
| P0-4  | **"2～3 个目标"不足以隔离八个场景** | 至少需要五类互不污染的金样本：纯元素移动目标、带 TUBI 的 BRAN、可移动且有几何的 PANE、合规 FRMW、一次性创建后可删除的牺牲构件。每个场景 manifest 固化 `refno/dbnum/noun/owner/POS/ORI/AABB/原始边/拓扑/apply 宏/restore 宏`。C1–C7 每次 apply 后都必须进入 `finally restore`，恢复失败立即终止后续场景（RI-15）。 |
| P0-5  | **C1 与 RS1 不等价** | 必须在 C1 前执行 `DELETE room_relate WHERE out = pe:<TARGET>;`，确认结果为空，再移动。否则没有覆盖 issue #7 的"缺边恢复"。 |
| P0-6  | **C5 不能按现状在真实工程执行** | 2026-08-06 计划明确决定：实机删除不单独造场景。必须二选一：① 从 Phase C 删除 C5；② 在 E3D 中先创建一次性牺牲构件，确认生成并获得房间边，再删除它。**不得删除既有 AMS 元素**。数据库回滚不能恢复 E3D 文件中的删除。 |
| P0-7  | **把 `/update/pending-units` 泳道当主要证据是错误的** | 正常 scoped drain 的 pending 可能在 UI 第一次轮询前就被消费。最低可接受证据：记录操作前日志 offset 和 `(dbnum,end_sesno)`，操作后解析同一窗口内的 `requested/loaded/done/failures` 与逐 target 日志。P1 再补 `DataBatchTaskResult.room`。 |
| P0-8  | **C4 把两个不同路径混成一个场景** | 拆成 C4a（挪管件）/C4b（挪整条 BRAN）。两者都要直接查询 BRAN 下隐含 TUBI 行的 `world_trans`、`aabb` 和房间边，不能只观察显式管件。 |
| P0-9  | **C2/C3/C6 的"变化范围"没有量化** | 每场景必须对**全局边集**做 diff，并限制允许变化闭包（见下）；`unexpected_changed_edge_count > 0` 直接失败。 |
| P0-10 | **C8 的零工作可能只是"保存没有被服务观察到"** | C8 必须证明产生了新的 sesno 或服务明确观察到一次新批次，然后作出 `room requested=0` 的裁决；并断言空间 epoch、目标 AABB、两张关系表规范化哈希、room pending revision 全部不变。 |

### P0 后各场景应固定的允许变化闭包

设 `E` 为根据 before/after AABB 精确差分得到的元素 target 集，`P` 为 panel target 集：

```text
C1/C2/C3/C4:
  allowed(room_relate) = rows whose out ∈ E
  allowed(room_panel_relate) = ∅

C6:
  allowed(room_relate) = rows whose in ∈ P
  allowed(room_panel_relate) = ∅

C7:
  allowed(room_relate) = rows whose in ∈ panels_under(FRMW)
  allowed(room_panel_relate) =
      rows whose in = FRMW or out ∈ panels_under(FRMW)

C8:
  allowed(room_relate) = ∅
  allowed(room_panel_relate) = ∅
```

对 RegenRoot 场景，`E` 不能只写用户操作的 CAP/FTUB；应当由该生成根下所有实例的 before/after AABB 差分推导。

## P1：本轮内必须补

| 缺口 | 建议落地方式 |
| ---------------------------- | ------------------------------------------------- |
| **未覆盖 RF1/RF2 的吸收封闭性** | 加一个组合场景：同一 E3D 会话内移动元素 B→A，同时移动或修改 A 面板，使 A panel task 与 element task 同轮出现；确认旧 B 边被删。 |
| **未覆盖真实多归属与排序载荷** | 把 C2 具体化为已有 RL5 跨库 K101/K170 场景，预期恰为两条边，并逐字段核对 `inside_count/center_dist`，再按 `inside_count DESC, center_dist ASC, room_num ASC` 独立计算首选房间。 |
| **C7 只测 FRMW 改名，漏了 RF7/RL4** | 增加 PANE OWNER 从房间 A 迁到房间 B，再恢复。 |
| **C4 漏容器侧 issue #5** | 增加移动 PIPE 或 ZONE：预期容器保留 Transform，同时其下 BRAN/HANG 进入 RegenRoot；直接检查至少一条 TUBI 的 AABB/world transform。 |
| **C8 不等价于 RF8** | 另加"普通重生成、AABB 逐位相等"的负例，预期零 room target。 |
| **缺少实机故障恢复冒烟** | 不在 8009 注入故障；在持久化的一次性库上补一次 `SPATIAL_TREE_NOT_READY`/panel preload 失败：关系表不得改变、pending 必须保留，恢复 Ready 后同一 target 成功。 |
| **C9 仍是组合测试** | 增加"只执行 `build_room_relations`、不做 startup autorun、不重启 watcher"的 rebuild-only 测试入口。C9 定位为"重启恢复 + 全量对拍"，不是唯一的纯 RI-12 证明。 |

## P2：记账后补

1. HANG/BOXI 的真实派生几何场景；当前 C4 只覆盖 BRAN/TUBI。
2. RF12 持续数据流下的房间 backlog 饥饿、RF13 同 target 跨 dbnum revision、RF14 deadletter retry 的长时压力套件。
3. plant-ui 可 inspect 房间号/房间树 surface。
4. 引入实体 geometry hash，最终关闭"形状变化但 AABB 相等不触发"的产品边界。
5. 有可恢复 E3D fixture 后再加真实普通构件/PANE 删除场景。

---

# 2. C0/C9 重启全量重建对拍的漏洞

## 2.1 它不是纯粹的 RI-12 测试

RI-12 的语义是：**同一份数据上，增量收敛后直接执行全量重建，然后逐边比较**。C9 额外引入：新进程、空间树快照校验/可能的指针重建、startup pending 立即重放、`AIOS_STARTUP_AUTORUN` 可能消费未处理会话、配置重载、`room_build:main` 裁决。因此 C9 是有价值的**组合验收**，但不能单独定位"room_incremental 算法是否正确"。纯语义门禁仍应由 Phase A 的同数据 parity 或 rebuild-only 入口承担。

## 2.2 会造成假红或归因错误的情况

| 情况 | 表现 | 处理 |
| -------------------------------- | ------------------------------ | -------------------------------------------- |
| startup autorun 处理了新数据或旧 pending | C9 后 PE/AABB/水位已经变化，边 diff≠0 | 判定为"本轮 C9 无效"，不是 room parity 失败。C9 前后必须比较 `spatial_epoch`、水位、`file_latest_sesno`、目标 PE/AABB 指纹。 |
| AABB 逐位相等但实体几何变化 | 增量按现行边界不入队，全量使用新几何后可能得出不同边 | 已知产品边界，不得靠删 diff 掩盖。该目标从本轮 RI-12 输入中排除，单独记 RF8。 |
| C0 尚未把旧数据规范化 | 首次全量清掉旧重复边、补 payload，C0 前后差异很大 | 这不是 C1–C8 回归。先单独审查 C0，C0 完成后再铸造金基线。 |
| `NoGeometry` 面板集合在 C0/C9 之间变化 | 一轮跳过、一轮能计算，产生边差异 | 判定 C9 输入不一致；必须记录精确 missing panel refno 集合。 |
| 原始 JSON 行顺序或 float 文本格式不同 | 逻辑相等但 raw bytes 不同 | 对结构化对象排序比较；`center_dist` 用 0.01mm 容差。 |
| snapshot 在 room round 运行中读取 | 两张表来自不同时间点 | 使用同一事务快照，读前确认两轮稳定；导出行数与独立 `count()` 相等。 |

需要特别区分：**启动时空间树从陈旧状态恢复到数据库指针真值，随后 full rebuild 改变了边，这不是应被归一化掉的假红**——它说明增量运行时使用过错误空间状态，属于系统一致性失败。

## 2.3 会造成假绿的情况

| 情况 | 为什么 `diff==0` 仍可能错 |
| ------------------------------------ | ---------------------------------------- |
| C9 实际没有执行 full rebuild | `room_build` 判据跳过、空间状态未 Ready、启动调用点只告警继续。必须证明重建开始、结束、零失败，且 `room_build:main` 盖章字段变化。 |
| full rebuild 部分失败 | 面板事务部分写入 + 启动层只告警；必须要求 summary `failures=0`。 |
| pending 在 restart 后先被重放并自愈 | ReplayRequired 进入后立即重放；杀进程前快照必须完整落盘。 |
| 增量与全量共享同一个错误输入 | TUBI 的 `world_trans/aabb` 若未随移动更新，两边都按旧位置算出相同错误边。 |
| 两边共享同一个错误谓词 | 正反向同用 `element_in_panel`；parity 必须额外断言"搬家确实发生"。 |
| full rebuild 对 `NoGeometry` 只跳过、不清旧边 | 同一陈旧边两侧都保留，diff 为 0。 |
| 元素旧边指向 missing panel | 必须证明每个元素目标的旧入边与 missing panel 集合无交集。 |
| normalizer 去重或忽略 payload | 重复 relation 压成一条、payload 丢弃、NONE→0 都可制造假绿。 |
| 只比较目标局部边 | 吸收错误把陈旧边留在另一个旧 panel；局部 snapshot 看不到。 |
| 只比较 `room_relate` | C7 的 `room_panel_relate` 可能仍陈旧（RI-9 失败）。 |
| 操作根本没有被服务观察到 | before=after，full 也相等。必须用 E3D Q、Surreal PE/inst、水位三方证明操作已发生。 |
| C9 只看最终恢复态 | apply 阶段算错、restore 恰好恢复基线；每个 apply 后仍需独立断言。 |
| 吸收分支根本未被命中 | C1–C8 没有 panel+element 同轮冲突场景时，C9 为 0 不能证明吸收封闭性正确。 |

## 2.4 对拍前必须采用的规范化与输入栅栏

### A. 单事务快照

C0、每个场景、C9 均执行同一份查询：

```sql
BEGIN TRANSACTION;
LET $rr = (SELECT id, record::id(in) AS panel, record::id(out) AS element,
                  room_num, inside_count, center_dist
           FROM room_relate ORDER BY panel, element, id);
LET $rp = (SELECT id, record::id(in) AS room, record::id(out) AS panel, room_num
           FROM room_panel_relate ORDER BY room, panel, id);
LET $rq = (SELECT action, target_refno, attempts, revision, last_error
           FROM model_update_pending
           WHERE action IN ['room_recalc_element', 'room_recalc_panel']
           ORDER BY action, target_refno);
RETURN { room_relate: $rr, room_panel_relate: $rp, room_pending: $rq,
         room_build: (SELECT * FROM room_build:main),
         spatial_epoch: (SELECT * FROM spatial_epoch:current) };
COMMIT TRANSACTION;
```

### B. 比较规则

```text
relation_id / panel / element / room_num / inside_count：精确相等
center_dist：两边均须 finite，abs(a - b) <= 0.01mm
NONE 与 0：不得等同
-0.0：仅可规范化为 0.0
集合：按 multiset 比较，禁止先 distinct
排序：panel, element, relation_id
```

完整性断言（每次快照后）：

```sql
-- 同一端点不得有重复 relation
SELECT * FROM (SELECT record::id(in) AS panel, record::id(out) AS element, count() AS n
               FROM room_relate GROUP BY panel, element) WHERE n != 1;
-- 不得指向不存在的端点
SELECT id, in, out FROM room_relate WHERE !record::exists(in) OR !record::exists(out);
-- 新写入的可计算边 payload 必须完整
SELECT id, in, out, inside_count, center_dist FROM room_relate
WHERE inside_count = NONE OR inside_count < 0 OR inside_count > 8
   OR center_dist = NONE OR center_dist < 0;
```

### C. `NoGeometry` 分区

1. **两轮均可计算**：纳入 RI-12 全字段强比较。
2. **两轮均 NoGeometry**：只断言其边在 C9 前后原样保留，且任何 C1–C7 target 都不属于这一类。
3. **可用性发生变化**：本轮 C9 无效，重建稳定输入后重跑。

额外断言：`incoming_panels(element_target) ∩ missing_panels == ∅`；`panel_target ∉ missing_panels`；`missing_panels_before == missing_panels_after`。

### D. 多归属的派生结果也要对拍

```text
expected_primary = sort(edges, inside_count DESC, center_dist ASC, room_num ASC)[0].room_num
assert fn::room_num_of(target) == expected_primary
```

### E. C9 必须证明输入没有被 startup 改写

```text
spatial_epoch:current 完整行相同；file_latest_sesno 相同；应用水位相同；
目标 PE.name/owner/position 相同；目标及全部 room panel 的 inst_relate.aabb/world_trans 指针相同；
missing_panels 集合相同；无新增数据/模型 pending
```

任一项变化，报告写"C9 invalid: input changed"，不能写 parity pass/fail。

---

# 3. 各场景可证伪判据审查

统一不变量要求同时证明水位、执行顺序、pending 收敛、完整载荷、拓扑、Q 与数据库一致、恢复成功；现有 C1–C8 只有终态方向性描述，均不足以单独防止"两边同时算错"。

| 场景 | 现状评价 | 最低可证伪判据 |
| -------------- | ------------------ | ------------------------------------------ |
| **C1 同房移动** | **不足**。边保持可能是更新根本没发生；未覆盖 RS1 缺边恢复。 | ① 预删目标所有入边并确认空；② E3D `Q POS` 与 Surreal PE/transform 一致；③ AABB 按预定平移向量变化、extent 不变；④ 边恢复到原 panel；⑤ `inside_count` 为预设值；⑥ `center_dist` 等于独立计算值且变化超过 0.01mm；⑦ 非 `out∈E` 的全局边不变；⑧ restore 后精确回 C0。 |
| **C2 跨房搬家** | **部分**。"新房"可能合法地是两间或更多。 | ① 预先固定完整 expected room set；② 旧/新集合精确相等；③ payload 完整；④ 独立排序首选房间与 `fn::room_num_of` 一致；⑤ AABB 与 E3D POS 对拍；⑥ 完整 RegenRoot target 集与 AABB diff 集一致；⑦ restore 回基线。推荐 K101/K170 双归属金样本。 |
| **C3 移出所有房间** | **不足**。`room_relate=[]` 同时也是 PanelIndex 丢失/空间状态错误/mesh 不可用的典型错误结果。 | 判空前证明：`after_aabb(target)` 与所有**可计算** panel AABB 均不相交、空间状态 Ready、missing panel 集不含目标旧 panel；AABB 确实到预定远离点；restore 后原边与 payload 精确恢复。 |
| **C4 管件/BRAN** | **不足**。只看"归属跟随"无法证明隐含 TUBI 被更新。 | 拆 C4a/C4b；先枚举 BRAN 下所有 TUBI/共享单位实例行；after 逐条断言 `world_trans`、AABB、房间边到预期；未受影响行不变；无旧行重复保留；observed room targets == 各实例 AABB 差分推导集合。 |
| **C5 删除构件** | **判据方向正确，但场景本身不可安全执行**。 | 只允许一次性牺牲构件：创建→生成→确认房间边→删除。删除后 E3D Q 不可定位；实例、模型节点、两方向边、空间树条目均不存在；不生成 room pending；未误删兄弟元素。 |
| **C6 移动 PANE** | **部分**。目标 panel 移入别的空间可能给别的房间构件新增第二归属，但其它 panel 出边不应被改。 | ① observed action 为 panel；② `Δroom_relate` 每条边 `in=target_panel`；③ `in!=target_panel` 的边逐字段不变；④ `room_panel_relate` 完全不变；⑤ 一个确定掉出 + 一个确定进入的 sentinel element；⑥ PANE AABB 按位移精确变化；⑦ restore。 |
| **C7 FRMW 改名** | **部分**。两张表同时清空也可能是 NAME 未传播/PANE 枚举为空/空间状态异常。 | 固定两段：合规→仍命中关键字但 regex 不合规→合规恢复。每段断言 E3D `Q NAME`、`pe.name`、完整 `panels_under(FRMW)` 一致；无 RegenRoot/Transform/AABB/epoch 变化；invalid 段两表精确清空、其它房间不变；恢复后精确回基线。PANE OWNER 迁移另立场景。 |
| **C8 幂等重存** | **不足**。零工作可能表示 watcher 没看到保存。 | 证明新 sesno 被发现并达到 terminal；`requested=loaded=done=failures=0`；无 room target 日志、无 pending revision、无 epoch bump、无 AABB/transform 变化、两张关系表规范化哈希相同。没有新 sesno 则记"未形成测试输入"。 |

此外，每个 C1–C7 在 apply 断言后都应再做一次无操作 execute/drain：`second requested/loaded/done/failures == 0`，`canonical_edge_hash`/`canonical_topology_hash` 不变。

---

# 4. `room_incremental` 默认翻正的上线风险与回滚触发

## 立即设置 `AIOS_ROOM_INCREMENTAL=0` 的触发条件

| 风险 | 明确回滚触发 |
| ------------------------- | ---------------------------------------------- |
| **范围外数据被改** | 任一 C1–C7 的全局边 diff 中出现一条不在场景允许闭包内的边。 |
| **可计算 panel 的 RI-12 不一致** | 输入栅栏 + NoGeometry 分区后，C9 在"前后均可计算"panel 集上出现任何 endpoint、room_num、inside_count、multiplicity 差异，或 `center_dist > 0.01mm`。 |
| **按空集 fail-open** | 空间状态非 Ready、panel geometry 不可用或 preload 失败时，任务仍被标记成功并删除现存边。 |
| **TUBI/派生几何停旧值** | 移动 fitting/BRAN/PIPE/ZONE 后，任何应受影响 TUBI 的 `world_trans` 或 AABB 未变化，或房间边按旧位置保留。即使 C9 diff==0 也应回滚。 |
| **关系完整性破坏** | duplicate `(panel,element)`、dangling endpoint、非法 `inside_count`、非有限/负 `center_dist`、relation id 不符端点规则、两表对不上。 |
| **删除清理失败** | 删除后仍有任一方向房间边或空间树条目，或启动全量把已删元素重新收编。 |
| **队列无法收敛** | 场景 target 20 分钟后仍 live pending；出现 room deadletter；同 target revision 增长但边不收敛；backlog 连续三个空闲轮单调增长。 |
| **空间状态持续不可用** | 正常运行后再次 `SPATIAL_TREE_NOT_READY`，连续两个 revalidator 周期未回 Ready 且 room pending 增长；或 `DegradedBlocked` 后仍有房间写入。 |
| **startup 重建不可信** | C0/C9 无零失败完成日志、`room_build:main` 未盖章却继续运行；或无新空间变化的连续两次干净重启都重复全量重建。 |
| **性能反向拖垮主链** | 单次 scoped drain 或 fallback 超 20 分钟；数据批次因房间轮持续饥饿；idle cadence 相对基线连续三轮恶化超 3 倍。 |
| **恢复纪律失败** | 任一 restore 失败、恢复后不能精确回基线：停止全部后续场景并关闭增量。 |
| **配置回滚不生效** | 设环境变量重启后 `/health.room_incremental` 仍 true：立即用备份二进制。 |

## 不应单独触发回滚的现象

- 正常最终一致窗口内短暂读到旧房间号；
- 单次 room target 失败留下 durable pending，随后 fallback 成功且边精确收敛；
- 启动期间短暂 Loading/ReplayRequired，最终 Ready 且期间无房间边写入；
- 有意执行 RF8 的"AABB 相等、零 room task"已知边界。

## 回滚动作不能只停在环境变量

1. 立即停止 E3D 保存和 watcher 新输入，保存日志 offset、pending、全局边集与空间 health。
2. `AIOS_ROOM_INCREMENTAL=0` 写进实际服务启动环境（非当前 shell），同时移除临时 `AIOS_STARTUP_AUTORUN=1`。
3. 重启并断言 `/health.room_incremental == false`。
4. 不先删 pending 或覆盖错误边，先保留证据。
5. 空间状态 Ready 后，用 C0 前数据库副本恢复，或执行验证过的 rebuild-only 全量修复。
6. 环境变量回滚不生效时用 `.bak-roomincr-20260812-165632` 二进制；二进制回滚不能代替数据恢复。

---

# 5. 一句话裁决

**不可按现状开跑 Phase C：最多只能在完成数据库备份、输入栅栏、C0 重建真实性检查、11/11 Phase A 全绿和场景 restore 合同后执行补强版 C0；C1–C8，尤其不可恢复的 C5，当前必须暂停。**

---

## 附注：执行侧落地状态（评审返回当时即已成立/随即落地的项）

- P0-1：**已闭合**——`room-fixture-8071.json` 批次（一次性空库 8071 + `DbOption-roomlive`）
  于评审进行期间复跑，**11/11 全绿**（`output/live-batch/20260812-180949/`）；每条用例本就
  由 Run-LiveBatch 独立进程执行；8009 现网 `/health.spatial_tree.startup_verdict` 为
  `rebuilt`/`reused`，非 `preloaded`。
- P0-2/P0-3/其余 P0：由 Phase C 执行时逐条落实，证据存 `output/room-manual-e2e/<时间戳>/`。
- P1/P2：记入本文件作为后续轮次的输入；C5 采纳"一次性牺牲构件"方案。
