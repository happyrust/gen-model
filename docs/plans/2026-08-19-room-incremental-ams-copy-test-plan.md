# 房间增量更新测试计划（test-increment 隔离 ams 副本）

状态：**暂停待前置修复**（2026-08-19 立项当日开跑；P0 双绿、R2 房间两案全绿，R1 全量与
P1 基线铸造被「发现二」挡住——用户裁决：等完整性线修复后再铸基线。交接见 §11）
日期：2026-08-19
牵涉：gen-model；隔离环境 `D:\work\plant-code\old\test-increment`
关联：`docs/adr/ADR-010-room-membership-incremental-update.md`（语义与 §9 验收口径）；
`docs/adr/ADR-033-increment-stage-controls.md`（三阶段门，本日新落）；
`docs/2026-08-06_room-incremental-automation-test-plan.md`（场景矩阵 RS/RF/RL 与不变量 RI-1…RI-15，本文直接引用）；
`docs/plans/2026-08-12-room-incremental-live-test-plan.md`（前身：C1–C9 因脏 E3D 硬阻塞，本文是其在隔离副本上的重启）；
`docs/plans/2026-08-16-data-increment-correctness-verification-plan.md`（D5 裁决：房间归第二阶段——本文就是那个第二阶段）；
`test-increment/README.md`（环境事实与三个入口）

## 1. 背景：为什么是现在、为什么在副本上

- 数据线第一阶段已收口：db7999 九场景（含 room-member / room-structure 两个合成房间案）
  2026-08-17 在隔离副本全绿（`runs/fixture7999-uifix2-20260817-180059/`，9/9 PASS，
  四平面断言 + 无关夹具冻结）。按 2026-08-16 计划 D5 的裁决，房间归属是第二阶段——现在轮到它。
- 2026-08-12 计划的 C1–C9 手动 E2E 被「全机唯一 E3D 是逆向 shadow 实例（补丁 DLL + 注入器 +
  E: 工程根）」硬阻塞，从未执行。test-increment（08-17 建）正是为此准备的隔离副本：
  无人值守 TTY、正式数据零风险。当年的阻塞在这里在结构上不存在。
- 今天（08-19）落了 ADR-033 三阶段门（房间收口接入阶段门）与 ADR-017 拆窗修订。
  **阶段门 × 房间没有任何 live 证据**；且正式 test-workspace 的 8000 还压着一个数据写回
  停顿未收口（`docs/evidence/2026-08-19-increment-stage-data-only-live.md`）——破坏性
  房间场景更不该去碰 8009/9099。

## 2. 现状盘点

已有覆盖（不重跑、只作门禁）：

| 轨道 | 资产 | 最近结果 |
|---|---|---|
| 合成 live（8071 内存实例） | `room_fixture` 11 条（parity/吸收/跨面板/删除/TUBI/结构触发…） | 08-12 全绿 |
| pytest 房间档 | `DbOption-roomtest.toml`，增量==全量逐边、删除留痕、durable 直写 | 08-12 15 passed |
| 隔离副本合成房间 | 九场景里的 room-member（精确 R101→R102）与 room-structure（合规改名拆房） | 08-17 PASS |
| 8019 testbed 真数据切面 | `test_cal_rooms`（214 房/229 板/41,370 边）、`rebuild_room_membership_on_the_live_project` | 08-19 通过 |

本计划要收的缺口：

1. **真房间实机场景在副本上从未跑过**：f8（CAP 位移）被 `Run-Suite.ps1` 默认排除
   （注释原话：「f8 需要 1112 面板模型播种，属二阶段」）；RL2/RL3/RL5 的宏
   （`room_cap_out_*` / `room_name_out_*` / `room_cap_cross_db_*`）已写好且有 8009 上的
   历史定标，但从未在隔离副本上执行。
2. **RI-12 硬标准在「真数据 + 实机增量」之后从未验证**：合成轨有 parity，正式库只铸过
   C0/C0b 基线；「一串实机增量收敛后 == 全量重建」的 C9 从未发生。
3. **ADR-033 房间阶段门 0 覆盖**：room=false 的留痕语义、重开后的积压消费、model=false
   时房间不许越过缺失模型的顺序依赖，全部只有单测。
4. RL4（PANE OWNER 迁移）无定标样本；RF3 多归属排序载荷只在合成轨验过。
5. 08-19 续测发现 5 条房间相关 live 用例断言漂移（`live_incomplete_room_panels_*`、
   `test_build_room_panels_relate_common`、`live_issue7_real_db_deleted_edges_come_back`、
   `live_issue13_c2_*`、`staged_transform_*` 的 warning=0 旧断言）——修用例的活，
   立项跟踪，不阻塞本计划。

## 3. 判定口径与硬标准

- **外部权威**：dabacon 库文件（宏 `Q POS/Q NAME/Q OWNE` 为人读投影）。
- **被验对象**：`room_relate` / `room_panel_relate` / `model_update_pending`（房间行）/
  `dbnum_watermark` / `/health` 的 `spatial_tree` 与三阶段门字段。
- **呈现层**：plant-ui 只验队列泳道截图（RI-14——房间号/房间树 inspect surface 仍未提供，
  V 级房间断言保持关闭）。
- **唯一硬门（RI-12）**：规范化边集（`panel, element, room_num, inside_count, center_dist`，
  数值容差 0.01mm）增量收敛后 == 同数据全量重建，**逐边 diff==0**；禁止只比 count。
  快照 SQL 沿用 2026-08-06 计划 §5.3，房间 pending 按 `(action, target_refno)` 查询
  （房间行 `dbnum=0`，不许按场景 dbnum 过滤）。

## 4. 环境与前置

### P0 通道与构建（约半小时）

```powershell
cd D:\work\plant-code\old\gen-model
cargo build --release --locked --features http_api --bin aios-database `
  --bin l3_suite --bin sync_sys_only --bin initialize_ams_dbnums --bin manual_scan_probe

cd D:\work\plant-code\old\test-increment
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\Test-Channel.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\Run-Fixture.ps1 -CheckOnly
```

- 8048/8028 空闲；正式 8009 不参与、不停它。
- des.exe 互斥门残余代价照 README：`L3_ALLOW_EXISTING_E3D_SESSION=1` 使
  FailedBeforeSave 重试前的清洁检查空转——宏崩过一次的场景须人工确认上一 TTY 会话已退。
- report 里记录工作树基线（当前 `f3c4263c` + 未提交改动清单）——房间链路近期动过
  `batch_worker` / `room_model` / `room_live_issue7`，证据必须能回答「跑的是哪一版」。

### P1 房间金基线铸造（一次性，预算半天，D1 裁决执行窗口）

1. bootstrap 空店：`included_db_files = ams7997/7999/5052/amssys`。
   ~~1112~~ **不再需要**（发现四：当前文件代 1112 无任何房间，f8 的 R512 面板在
   db7997）；等 1112 补回 6KA 数据后再把它加进面并启用 RL5。
2. **scoped 模型播种，不做全库生成**：`/model/ensure` 点名——
   R512 房间（`/1RX-RM05-R512`）名下 PANE 集（已定标 `=24381/35844`，db7997）；
   CAP `24383/66460` 所在 BRAN `/1WCC1135/B1`（db7999，探针已确认在位）；
   （若 D3 采纳）一条带 TUBI 的 BRAN。靶点复核入口：
   `Test-Channel.ps1 -Macro scripts/e3d/room_targets_probe.mac`。
3. 启动全量房间重建并盖章 `room_build:main`；空间树 `/health` 全键留档
   （state=Ready、drift=false）。
4. **输入栅栏**（08-12 C0 的做法）：规范化全量快照连拍两次，逐字节相等才算基线成立。
5. 落盘 `baseline-targets.json`：CAP 现有边（预期 R512）、R512 名下 PANE 集、
   K101/K170 双归属证据（08-08 定标记录作参照，以本副本实测为准）、RL4 候选样本
   （找不到则记「无稳定样本」触发 D2）。

## 5. 场景矩阵

### R1 九场景固定回归（约 10 分钟）

今天动了拆窗、阶段门、法线修复——先证明 08-17 的 9/9 绿在当前二进制上没有回归：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\Run-Fixture.ps1
```

room-member / room-structure 判据不放宽：四平面 + 精确 `room_before/room_after` +
无关夹具冻结 + pending 清零。

### R2 真房间实机场景（核心增量）

统一节奏（照 2026-08-06 §5.2）：before 规范化快照 → apply 宏（TTY）→ execute →
data task 房间摘要（RI-5 临时口径：「写回后房间计算 … room_done=N」日志行 + partial
状态；`DataBatchTaskResult.room` 结构化字段落地前不许用「最新 task id」猜关联）→
after 快照 → 断言 → **第二次 execute 零工作、边集不变（RI-11）** → restore 宏 →
恢复断言逐边回基线（RI-15）。

| id | 对应 | 操作与宏 | 房间专项判据（通用五平面之外） |
|---|---|---|---|
| R2.1 | RL1/f8 | 先删目标现存边（复刻 issue7），CAP `24383/66460` +100mm U，`issue7_cap_pos_apply/restore.mac` | membership 恢复 R512 且载荷更新；BRAN `RegenRoot` 的完整 target 集按 P1 定标断言（不假设只有 1 个 room target） |
| R2.2 | RL2/C3 | CAP 移出所有房间，`room_cap_out_apply/restore.mac` | apply 后目标 `room_relate == []`，且必须旁证「是删干净不是没算」（room_done>0）；restore 逐边回基线 |
| R2.3 | RL3/C7 | FRMW `/1RX-RM05-R512` 改名出格再改回，`room_name_out_apply/restore.mac` | 名下全部 PANE 走 panel 分支；`room_relate` 与 `room_panel_relate` 两表同窗清空/恢复；零几何 regen |
| R2.4 | RL5 | ~~CAP 跨库搬进 K101~~ **本文件代不可执行（发现四）**：db1112@722 无 6KA 房间树，08-08 定标作废。跨库归属语义由 R2.1/R2.2 覆盖（元素 db7999 × 面板 db7997）；「房间图跨两个结构库」待 1112 数据补回后另行定标 | 定性记录进 report，不算红 |
| R2.5 | RL4 | PANE OWNER 在两个合规房间间迁移（新宏对） | 目标 PANE 仅一行 pending；新旧两间拓扑精确切换。**P1 找到稳定样本才启用**（D2） |
| R2.6 | C4/RF9 | 挪管件 → BRAN RegenRoot → TUBI 行归属跟随（实机版） | 合成轨已覆盖（`live_room_tubi_*`、issue5 live 今日 8019 复验通过），实机作可选加严（D3） |

### R3 ADR-033 阶段门 × 房间（全新覆盖）

| id | 配置 | 判据 |
|---|---|---|
| R3.1 | `room_incremental=false`，做一次 R2.2 类变更 | 数据+模型照常、水位推进；房间目标 durable pending 留存；health `room_incremental=false`、有声日志。**反证 2026-08-10 翻车形态：不许出现「按空集收敛」清边** |
| R3.2 | 重开 `room=true` 并重启 | 积压收干净（完成 N / 失败 0）；终态边集 == 从未关门跑法的终态（拿 R2.2 的 after 快照对拍） |
| R3.3 | `model_incremental=false` 且 `room=true` | 房间不许拿旧几何跑（ADR-033 §3）：房间 pending 保留不执行；重开模型后先模型后房间收敛 |

### R4 终局对拍（RI-12 唯一硬门）

全部 restore 后：规范化全量快照 → `AIOS_STARTUP_AUTORUN=1` 重启触发全量房间重建 →
重建后快照与增量收敛快照 **逐边 diff==0**。允许的例外清单按 D4 裁决（建议：只允许
C0 台账已记录的「关窗期回补」类，逐条说明，其余一律红）。顺带断言：
`/health` `spatial_tree.drift=false`、epoch 一致、房间 pending=0。

## 6. 证据与目录约定

`test-increment/runs/room-<phase>-<yyyyMMdd-HHmmss>/`：

- `report.md`（含工作树基线、配置、门禁值）；
- `room-edges-{baseline,after-<case>,restored,final,rebuild}.json`（规范化快照）;
- 每案 `mutation-evidence.json`、apply/restore 宏日志、`execute-receipt.json`、
  `pending-<case>.json`、`health-<case>.json`；
- UI 只存队列泳道截图（启用 `-Ui` 时）。

台账回填：`docs/2026-08-12_live-test-ledger.md` 与 2026-08-06 计划 §10；本文 §10。

## 7. 失败与清理纪律

1. apply 后任何断言失败**仍必须 restore**（RI-15）；restore 失败立即终止剩余破坏性场景，
   `scripts/Reseed-Project.ps1` 归零副本后再议。
2. R3 门控场景遗留的 durable pending 在场景收尾必须显式消费或清账——否则下一场景的
   「pending 清零」断言背锅。
3. 阻断即定性记录（08-12 计划的纪律）；环境冲突、限流不算用例失败。
4. 全程不碰 8009/9099 正式库；8000 写回停顿是另一条诊断线，不混入本计划。
5. 破坏性场景串行，一次只跑一案；`delete` 类永远排最后（夹具既有纪律）。

## 8. 待决问题

- **D1 执行窗口**：P1 的 1112 首建 + scoped 播种预算半天，何时跑？
- **D2 RL4 样本**：真库找不到稳定 PANE OWNER 迁移样本时，是记缺口跳过，还是在副本上
  手工造一个（副本无副作用，倾向造）？
- **D3 R2.6 取舍**：实机 TUBI 跟随本轮做不做（合成轨已覆盖，建议可选、时间富余才做）？
- **D4 R4 例外口径**：终局对拍允许的 diff 例外清单（建议只允许「关窗期回补」类）。

## 9. 完成定义（DoD）

- R1 九场景全绿（当前二进制）；
- R2.1–R2.4 全绿（含每案 RI-11 幂等与 RI-15 恢复）；
- R3.1–R3.2 全绿（R3.3 允许定性记录）；
- R4 逐边 diff==0（例外逐条说明并经 D4 口径核准）；
- 台账回填 + §2.5 的用例漂移修复立项；
- R2.5/R2.6 未跑的写明原因进 report。

## 10. 轮次台账（随执行回填）

| 时间 | 阶段 | 结果 | 证据 | 首个失败判据 |
|---|---|---|---|---|
| 08-19 19:15 | P0 通道 + 夹具预检 | **双绿**：TTY 5s 登进副本无残留；db7999 头可读（44,955 refno）。8028/8048 空闲。在跑的 des.exe(53064) 绑正式 `D:\AVEVA`（shadow 运行时 + 补丁 DLL），与副本不同根，按既有 `L3_ALLOW_EXISTING_E3D_SESSION=1` 姿势放行 | `runs/channel-20260819-191459`、`runs/fixture7999-20260819-191517` | — |
| 08-19 19:16 | R1 首跑（默认 paged 读取） | **2/9**：data/transform 过；geometry/boolean/owner/add 全倒在同一处，房间两案未及执行 | `runs/room-r1-20260819-191600` | CATA 必需依赖准备失败：`paged ref0 scan snapshot mismatch ams7329_0001 paged=0 authoritative=221`（**发现一**） |
| 08-19 19:24 | R1b 重跑（`AIOS_PDMS_ON_DEMAND_READ_MODE=legacy`） | **5/6**：data/transform/geometry/boolean/owner 过；add 红且中断后续 | `runs/room-r1b-20260819-192423` | add restore 未复现基线：336 删除会话被收集（删除=1）、批次 succeeded、水位推进至 336，但 `pe:24383_102098` 存活（deleted=false、sesno=0）（**发现二**） |
| 08-19 19:34 | R2 房间专项（房间专用清单，legacy 模式） | **room-structure PASS、room-member PASS**（四平面 + 精确归属）。room-member 逐边证据：before `R101 inside=8` → after `R102 inside=8` → restored `R101 inside=8`，恢复精确回基线 | `runs/room-r2-20260819-193411`、清单 `test-increment/room-only-manifest.json` | — |
| 08-19 20:06–20:16 | 环境准备：真房间靶点只读定标 | **RL1/RL2/RL3 靶点全部在位**：CAP（/1WCC1135/B1 首 CAP，POS W5001 N10706 U5822，与黄金坐标一致）、R512（FRMW→VOLU→PANE `=24381/35844`，refno 与历史定标逐位一致）。**RL5 定标作废**（发现四）；顺带实测出 TTY 崩溃纪律（发现三） | `runs/channel-20260819-201536`（定标全文）；探针 `scripts/e3d/room_targets_probe.mac`；入口 `test-increment/scripts/Run-RoomFixture.ps1`（-CheckOnly 已验） | — |

### 执行发现（移交完整性线，2026-08-19）

- **发现一（P1，ADR-037 在途）**：新分页读取器对单区段 DESI 库 `ams7329_0001` 读出
  `snapshot().sesno=0`，权威读取器读出 221（与 8009 水位表一致）。守卫按设计 fail-loud，
  但被守的读取器本身错了；该库在 CATA 必需依赖准备的路径上，任何带 regen 的窗口全部被
  阻断。临时绕过：`AIOS_PDMS_ON_DEMAND_READ_MODE=legacy`（本计划 R1b/R2 即此姿势）。
- **发现二（P0 候选，删除写回丢失）**：add 案 restore 的删除会话（336..=336）被净窗口
  收集（删除=1）、暂存写回三块 journal + 尾事务全部成功、水位推进，但被删 BOX 的 pe 行
  在库里存活，`deleted=false`、**`sesno=0`**——疑似模型/祖先预载路径写入的 sesno=0 行
  逃过了带会话守卫的删除语句。违背 ADR-021「水位是承诺」。并行会话已在同一运行目录留下
  取证探针（`probe-336.py`、`probe-add-delete.py`），归属其继续；本线不动源码。
- **发现三（环境级，TTY 崩溃纪律）**：无人值守 TTY（shadow_e3d31_gen_model_test 运行时）
  对「导航一个不存在的名字」不是报 PML 错误，而是**直接访问违例退出**（0xC0000005），
  且 ALPHA LOG 缓冲随崩溃丢失、日志为空。对照实验：只含
  `/AIOS-DEFINITELY-NOT-EXISTING` 一行的宏即复现（`runs/channel-20260819-201132`）。
  纪律：实机宏只许导航已证实存在的名字；新增靶点先用 aios_db 在文件侧核实。
- **发现四（RL5 定标作废）**：db1112 当前文件代（sesno 722，2022-11 尾）**不含任何
  6KA 房间树**——`17496/230552`、`17496/230648` 两块面板文件侧点查 MISSING，全文件字节
  搜 `6KA` 零命中。2026-08-08 的 RL5 定标是对着 Surreal 8009（当时 applied=897）做的，
  文件回退（722 < 897）把那段数据整个带走了。连带修正两条旧口径：① `Run-Suite.ps1`
  注释「f8 需要 1112 面板模型播种」失效——f8 的 R512 面板在 **db7997**（24381_*），
  P1 播种只需 7997 结构；② 跨库归属（元素 db7999 × 面板 db7997）RL1/RL2 本来就覆盖，
  RL5 独有的「房间图跨两个结构库」在 1112 补回 6KA 数据之前无法验证。
- 附带（移交完整性线）：对 1112 跑全跨度 `net_window(1, 722)` 被完整性纪律硬阻断
  （`读取索引已选中子页 4667 失败…IndexPageData.noun 断言`）——2016 代旧页与新读取器的
  又一处不合，legacy 重放收集器同跨度可走通。不阻塞本线（P1 首建走全量解析不走差分）。
- 环境注记：`test-worklspace`（正式 8009 一侧）与 gen-model 工作树今天由并行会话活跃
  编辑（aios-database.exe 18:59 在途构建）；本计划所有轮次记录二进制 mtime 作版本证据。

## 11. 交接（2026-08-19 收工）

**已完成**：P0 通道/预检双绿；R2 房间两合成案在含当日全部在途改动的二进制上通过
（room-structure、room-member，精确归属闭环）；两个引擎缺陷定性并移交（§10 执行发现）。
**未开始**：P1 金基线、R2 真房间四案（f8/RL2/RL3/RL5）、R3 阶段门、R4 终局对拍。

**恢复开工的顺序（前置门先行）**：

1. 等完整性线修复发现一/发现二（他们的取证探针在 `runs/room-r1b-20260819-192423/`）。
2. 验证修复：重跑全九场景，**默认 paged 模式、不带 legacy 逃生舱**——
   `powershell -File scripts\Run-Fixture.ps1 -Label room-r1c`。一条命令同时验两个缺陷：
   发现一好了 geometry/boolean/owner 不再倒在 ams7329，发现二好了 add/delete 的
   restore 逐字节回基线。9/9 绿才算 R1 关门。
3. R1 绿后进 §4 P1（金基线铸造），再按 §5 顺序走 R2→R3→R4。

**资产与环境状态**：

- 房间专用清单（R2 用过、可复用）：`test-increment/room-only-manifest.json`；
  配套入口 `test-increment/scripts/Run-RoomFixture.ps1`（`-ReadMode legacy|paged`，
  默认 legacy，完整性线修复后跑一次 `-ReadMode paged` 验证再翻默认；`-CheckOnly` 已验）。
- 真房间靶点探针：`gen-model/scripts/e3d/room_targets_probe.mac`（只读，经
  `Test-Channel.ps1 -Macro` 调用）。**TTY 纪律见发现三：宏里绝不导航未证实的名字。**
- RL5 相关宏（`room_cap_cross_db_*`）暂不可用（发现四），保留待 1112 数据补回。
- legacy 逃生舱：`AIOS_PDMS_ON_DEMAND_READ_MODE=legacy`（进程级环境变量）。修复
  验证时**禁用**它；只有在完整性线未收口又急需跑房间线时才允许挂着它跑非删除类场景。
- 副本 db7999 已累积本日三轮夹具痕迹（apply/restore/teardown 会话若干）；夹具按名字
  解析、幂等自愈，下一轮无需处理。要洁净起点就 `scripts\Reseed-Project.ps1` 归零。
- 本线零进程残留：8028/8048 空闲；机器上现存的 des.exe（E: shadow 运行时）与
  9099 的 aios-database 属并行会话/常驻部署，勿动。
- 二进制版本证据：本日三轮用 `D:\Rust\target\release\`（l3_suite 08-19 00:45、
  aios-database 08-19 18:59 在途构建）；下一轮跑之前重记 mtime。
