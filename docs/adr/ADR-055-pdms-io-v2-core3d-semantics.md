# ADR-055：新版 pdms-io（db1~db5）的元素语义以 Core3D.dll 为准

状态：**已接受**（2026-08-29 grill 确认：Q1–Q8 全采推荐项）
日期：2026-08-29
关联：ADR-002（core.dll 权威范围）、ADR-004（按需解析 CATA）、ADR-053（direct 模式生成读）、
`docs/specs/core3d-partial-update-conformance.md`（R0–R29 规则核对表）、
`docs/specs/core3d-partial-update-test-cases.md`（C 编号用例）、
`tests/fixtures/core-noun-granularity-e3d31.json`（1931 noun 位表快照）、
`src/data_interface/core3d_reference.rs`（可执行参考模型）、
`teach/learning-records/0009`（Core3D 批量生成流水线 RE）；
产出计划 `docs/plans/pdms-io-v2-core3d-alignment.md`，规格 `specs/034-core3d-semantics/`。

## 背景

新版 pdms-io（`d:\work\plant-code\pdms-io-fork`，`pdmsdb_engine_v2`）按 core.dll 的 DB1–DB5
模块分层从零重建了 dabacon 引擎（提交 `d53ffc99`）：`fortran_io`（直接存取 I/O）、
`db1`（页缓存/页锁）、`db2`(头/会话/库查找/extract)、`db3`（B 树）、`db4`（元素/属性/CE/引用）、
`db5`（库级操作），对拍口径 `compare/` 目前只有 `legacy_oracle` 与 `core_dll_oracle`。

但 Core3D.dll（core.dll 的消费者，导入其 4859 个符号）定义的**元素使用语义**在 db4 层缺失：

- `DB_Noun::getField(id, &out)` 位表分类（significant / primitive / negative）——缺；
- `Members(e, mode)` 三模遍历，收集判据与下潜判据是两个独立闭包（R11）——缺；
- `SignificantOwner(e)` 含自身、无深度上限、按位终止（R14）——缺；
- `climb(e, NOUN)` 按 noun 找祖先（R2 XGEOM 门）——缺；
- `DB_DB::type(e.getDB()) == 1` DESI 门（R1）——db2 有 `db_lookup` 但类型语义未对齐；
- `DSAVE`/`DRESTO` 指针栈 + `NXTITM` 游标——`db4/ce.rs` 只有裸栈，`NavDirection` 枚举定义了没人用。

gen-model 侧已把 Core3D 的 `PartialUpdateDesiMgr` 逆向到规则级（R0–R29 核对表 + C 用例 +
可执行参考模型 + 1931 noun 位表快照），这些资产不重造，直接作为对齐基准。

## 决策（grill Q1–Q8）

| # | 决策点 | 选项 | 结论 |
|---|---|---|---|
| Q1 | 语义权威怎么分 | A｜分层定权威：db1–db3（页/会话/B 树）以 core.dll 为准，db4–db5 及以上（分类/遍历/攀爬/库类型门）以 Core3D 为准；B｜全栈以 Core3D 为准；C｜维持现状只对 core.dll | **A** |
| Q2 | noun 位表放哪、从哪来 | A｜快照硬编进 pdms-io；B+C｜接口在 pdms-io（`trait NounBitSource`），生产读 gen-model 已导快照（`core_sha256` 校验），对拍走 core.dll FFI 现取；C｜只走 FFI | **B+C** |
| Q3 | `Members(mode)` 抄到什么程度 | A｜db4 严格照 R11：显式栈 LIFO、收集与下潜判据分离、不物化列表，mode 2 实现但 `#[doc(hidden)]`；B｜只给 children 迭代器，三模留给调用方 | **A** |
| Q4 | CE 导航栈补不补齐 | A｜补齐：`NavDirection` 真正驱动导航，加 `climb(noun)`/`owner_chain()`/`significant_owner()`，对齐 `DSAVE`/`DRESTO`；B｜保持纯数据结构 | **A** |
| Q5 | 「以 Core3D 为准」怎么证伪 | A｜三层 oracle：`legacy_oracle` + `core_dll_oracle` + 新增 `core3d_oracle`（可执行参考模型当期望值，C 用例驱动）；B｜只做真库端到端对拍 | **A** |
| Q6 | 调度语义归属边界 | A｜pdms-io 只做读语义（R0/R1/R2/R3/R9/R11/R12/R14/R26），队列/去重/三遍消费留在 gen-model；B｜把 `PartialUpdateDesiMgr` 整个搬进 pdms-io | **A** |
| Q7 | 多 extent | A｜db2 补 extract/extent 寻址、db1 页层跨 extent，排后期阶段；补齐前显式拒绝并点名文件；B｜本轮只显式拒绝 | **A**（排 P3 阶段） |
| Q8 | 写侧 | A｜本轮只读，`record_writer`/`db5` 的 mark-refresh-compact/`e3d31-writeback` 冻结；B｜读写一起推 | **A** |

## 关键取舍（Considered Options）

- **Q1 A vs B/C**：Core3D 不碰页与 B 树——它是 core.dll 的调用方，拿它当页层权威没有证据来源；
  而 db1–db3 的 `core_dll_oracle` 对拍基准已建立，换基准等于重置已验证的东西。C（只对 core.dll）
  则让 db4 的元素语义永远停在「我觉得对」。分层定权威与两边的证据来源一一对应。
- **Q2 B+C vs A/C**：核对表 R0-2 已证明 `primitive` 的第二位跨版本会换（2.10 是 `0xA18B8`，
  3.1 搜不到）。硬编（A）把版本漂移变成静默错误；只 FFI（C）让生产依赖装着 E3D。
  快照 + `core_sha256` 校验（对不上**报错不回落**）给生产，FFI 现取给对拍自证，两路各归其位。
- **Q3 A vs B**：R11 反直觉——mode 0 下非 significant 子节点会**挡住**其下的 significant 孙节点，
  这是判据本身不是遍历副作用。留给调用方（B）等于每个调用方各错一遍。mode 2（Negative）挂在
  死代码上（R16：`m_granularityMode` 恒 0），实现但 `#[doc(hidden)]`、不给生产调用方——
  照着死代码建模只会引诱下一个人去实现它（`core3d_reference.rs` 的既定口径）。
- **Q4 A vs B**：`NavDirection` 枚举定义了没人用是半成品；不补齐，上层一定绕过 CE 自己走 owner，
  `DSAVE`/`DRESTO`「子树遍历不物化」的收益拿不到。
- **Q5 A vs B**：`PartialUpdateDesiMgr` 不能被我们直接调用（要 view、要 `PDMS_Idlist2`、
  `DrawModel` 发 PML）。端到端对拍（B）拿不到中间态，测不出「下潜判据错了」这种问题。
  参考模型已把规则写成能跑的代码——下一次读错就会红，不会一路带到生产。
- **Q6 A vs B**：那套队列绑在 E3D 视图上（`AddIDList` 会 `writeDB()`、`Refresh` 认 `NOUN_VIEW`），
  我们没有视图概念（R7/R8/R27 判 ⚪）；搬进来会把 IO 库变成半个渲染管线，且规则消费者在
  gen-model（`model_impact`/`model_update_pending`/`generation_root`）。
- **Q7 A vs B**：`Db.ExtractNumber` 是 AVEVA API 一等公民，且 gen-model 现在遇到 `_0002+` 会
  静默回落 legacy 全文件读（悬崖）。不排最前的理由：本机 E3D3.1 的 1002 个 dabacon 里 0 个
  多 extent，当前不触发。补齐前的处置是**显式拒绝并点名文件**，不静默退化。
- **Q8 A vs B**：「不预先入库、直接读文件生成」的目标全在读侧；写侧同时动会让对拍基线不稳。

## 后果（Consequences）

- pdms-io-fork 侧：`compare/` 新增 `core3d_oracle`；db4 新增 `Core3dSemantics` trait
  （`is_valid`/`db_type`/`climb`/`is_significant`/`is_primitive`/`significant_owner`/`members`/`exists`）；
  `db4/ce.rs` 补齐导航；db2 暴露 `DbKind` 并补 extent 寻址；页大小 `0x34` 按「字」正确解释、
  探测器双判据；`open_at(path, sesno)` 让「pin applied_sesno」与「读最新」共用一条实现。
- 共享 crate：`core3d_reference` 参考模型 + `NounBitSource` 快照/FFI 双实现提升为单一来源，
  gen-model 与 pdms-io 都依赖它，消除两处漂移。
- gen-model 侧：`DbElement` 门面（`docs/plans/direct-dbelement-read-api.md`）改为薄封装，
  分类/遍历/攀爬转调 db4，不再本地实现第二遍；`generation_root` 名单判定接位表按 R9 口径
  **加层不换判据**。
- 实施纪律：每个实现 Core3D 语义的函数，文档注释回引核对表 R 编号；判据一律读指令流不读伪码
  （Hex-Rays 在 `PartialUpdateDesiMgr` 上系统性丢分支，§1.4 已证）；死代码标注不实现；
  位表校验失败、多 extent、页大小断言不过一律报错不回落（fail loud）。
- pdms-io 改动走升 rev 流程消费，不得带本地 patch 推 main（pre-push 守卫已有）。

## 风险

- **K1 位表跨版本漂移**（`primitive` 第二位 2.10↔3.1 不同）→ 两位分开存 + `core_sha256` 钉版本 +
  加载校验报错；FFI 通道作为自证口径。
- **K2 伪码丢分支**导致再抄错（`AncestorDeletes` 终止条件已错过一次）→ 规则先落进可执行参考模型 +
  C 用例，实现照用例写。
- **K3 参考模型两处漂移**（gen-model 与 pdms-io 各一份）→ P0 提升为共享 crate，单一来源。
- **K4 页大小探测在真库仍会误判**（490 个文件 17 个中招）→ 双判据 + 真库回归用例。
- **K5 db4 语义层成为第二套解析实现** → 语义层只做分类与遍历，记录解析仍走 `record_reader`。
- **K6 多 extent 牵动 db1 页定址回归面大** → 排 P3，先显式拒绝；改动带双 extent 夹具。
- **K7 ida-bridge 当前不可用**（`.cursor/mcp.json` 为空）→ 先吃 `.ida_scratch` 与
  `ida_exports/3.1/` 存量；需要新地址再配 MCP，核对表 §1.4 已给重定位 SQL。
