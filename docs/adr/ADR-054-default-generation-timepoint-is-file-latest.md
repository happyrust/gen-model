# ADR-054：未指定时点的模型生成，一律取文件最新会话

状态：**已接受**（2026-09-02 用户拍板原话：「如果没有指定时间，默认就是要使用最新的数据去生成模型」）；
**已实施**（2026-09-02，实施约束 1–7 全部落地：新模块 `src/data_interface/model_source.rs`，改动清单见 `changelog.md` 同日条目）
日期：2026-09-02
关联：ADR-053（direct 模式生成读，**本 ADR 取代其 Q3**）、ADR-001（`applied_sesno` 是唯一权威水位——**不改**）、
ADR-025（严格初始化相位 · 模型门）、`docs/plans/direct-dbelement-read-api.md` §D6（把本编号预留给「免入库直生的范围与权威边界」）、
`docs/plans/2026-09-01-refno-scope-model-reconcile-plan.md`（凭证判据从等值走向单调的同一方向）、
plant-ui ADR-0009（显示时补齐模型走服务）。

## 背景

ADR-053 Q3 把生成期读取时点钉在各库的 `dbnum_watermark.applied_sesno` 上（选项 A），理由是与 DB 模式
读到同一逻辑时点、便于对拍；「文件最新会话」（选项 B）被留作「免摄入直生」形态。今天代码里这个时点在
当前投影链路上有三处权威，都读同一张表：

| 位置 | 用法 |
| --- | --- |
| `src/data_interface/direct_store.rs` `pins_from_watermark()` | `applied_sesno` → `DbPin.sesno`；`<= 0` 时给 `None`，而 `E3dModelService::generate_roots` 对 `None` 直接报「direct model pin has no fixed session」 |
| `src/data_interface/model_update_pending.rs` `root_model_source()` → `ensure_regen_pending_current()` | 按需生成的 durable 工作项认领 `state.applied_sesno / applied_sesno_time` |
| 同文件 `generation_root_cache_current()` / `gen_root_credential_is_current()` / `model_coverage_current()` | 完成凭证「当前」= `source_end_sesno == applied_sesno ∧ 时刻相等` |

后果有两个，都是用户可见的：

1. **没解析过的项目看得见树、点不出模型。** direct tree（2026-08-31）已经零解析出树，但 ensure 在
   `root_model_source` 处就因「dbnum 没有已应用水位」失败；`pins_from_watermark` 也给不出 pin。
2. **文件比库新的那段时间，模型停在旧数据上。** 用户在 E3D 里 SAVEWORK 之后、摄入追平之前，点看
   得到的是 `applied_sesno` 那一版；而「最新数据」才是用户点看时的预期。

同时，历史投影（`src/fast_model/historical_model.rs`）已经长出了正确的抽象：
`SessionSelector { Latest, Sesno(u32), At(DateTime) }` + `resolve_session(path, selector)`，
`Latest` = `ReadOnlyEngine::open(path).session().sesno` 加会话写入时刻。**当前投影与它用的是两把尺子**，
这就是本 ADR 要收掉的分叉。

## 决策（用户拍板）

| # | 决策点 | 选项 | 结论 |
| --- | --- | --- | --- |
| Q1 | 未指定时点时生成用哪一版数据 | A｜文件此刻自报的最新会话；B｜`applied_sesno`（ADR-053 Q3-A 现状） | **A**。ADR-053 Q3 自此被取代 |
| Q2 | 「指定了时点」是什么 | 请求显式带 `sesno` / 时刻（历史投影 `SessionSelector::Sesno / At`），以及增量管线**按窗口右端**生成（窗口右端就是显式时点） | 给了就用它，一个字不改 |
| Q3 | `applied_sesno` 的角色 | A｜继续兼任生成时点；B｜只做摄入水位（属性面板 / 搜索 / 房间归属 / 暂存窗口的口径，ADR-001 原义） | **B**。它不再是生成时点的默认来源，但 ADR-001 一个字不动 |

一句话：**时点只有两种来源——调用方显式指定，或文件最新；水位不是第三种。**

## 实施约束（由 Q1–Q3 推出；实施时逐条核，不得静默绕开）

1. **一把尺子。** 当前投影解「最新」必须复用 `historical_model::resolve_session(path, Latest)` 那条路
   （`ReadOnlyEngine::open(path).session().sesno` + 会话时刻），不得再写第二份「读文件最新」。
   `DbPin::sesno == None` 在 `DirectStore` 里已是「开库那一刻解一次最新然后整个运行内冻住」；
   `E3dModelService` 的 pin 应在构造时把这个数解出来带上，而不是把 `None` 当错误。
2. **文件从哪来。** 库文件路径的权威是 MDB 成员（`e3d_model_service::current_mdb_sources()` 已经按
   `mdb_membership` 列出全部库与路径）；`dbnum_watermark.file_path` 只是它的一份登记副本，
   **不得成为生成的前置条件**——否则零解析项目照旧点不出模型。
3. **dbnum 不再从 `pe` 查。** `root_model_source()` 与 `E3dModelService::dbnum_for_roots()` 现在
   `SELECT dbnum FROM pe:…`，零解析下无行。改走 ref0 → dbnum 定位器（`CataDbLocator::resolve_ref0` /
   `DirectTreeService::dbnum_of`），这与 CONTEXT.md「Ref0 库归属」条目一致：反查不到就报错，不猜。
4. **凭证判据从等值改单调。** `gen_root.source_end_sesno` 记的是「这个模型是按哪一版数据生成的」；
   「当前」应判 `source_end_sesno >= 要求的时点`，不再要求相等。否则 ensure 在最新会话 N+1 生成之后，
   增量管线按窗口右端 N 复核会把它判成过期、重排、再撞 `ensure_not_older_than_persisted` 的「不得
   回退」守卫，把一条正确的新模型报成批次失败。时刻列（plant-ui ADR-0019 口径）随会话号一起记，
   比较只比会话号。这一条与 reconcile 计划里 `cred.source_end_sesno ≥ data(r)` 是同一方向，
   **必须与 ADR-025 模型门（`model_coverage_current`）同一批改并补测试**。
5. **`apply_window` 的断言要拆。** `E3dModelService::apply_window` 现在断言 `pin.sesno == Some(target_sesno)`；
   窗口右端就是显式时点（Q2），它该自己按 `target_sesno` 开库（`build_set(dbnum, Some(target))` /
   `scan_index(.., Some(target))` 已经是这样），而不是要求当前 pin 恰好等于它。
   `ensure_not_older_than_persisted` 保留：已发布的更新版本不得被更旧的窗口覆盖——按第 4 条，
   这种情况是「已被更新版本覆盖」，收口为成功而不是失败。
6. **旧规则的测试要翻过来，不是删掉。** `direct_store.rs` 的
   `a_dbnum_with_no_pin_is_an_error_not_the_newest_session` 注释原话「改成『没钉水位就读文件最新会话』
   会让这条红」——它守的正是被本 ADR 取代的规则。改成：定位器**认识**的库没 pin → 解最新；定位器
   **不认识**的库 → 仍报 `NoFileForDbnum`。同文件模块头「读哪个时点——按 dbnum 钉在该库的
   `applied_sesno` 上」一段同步改写。
7. **一次性代价要说在前面。** `ModelTarget` 的 design 段带 `(file, session)` 摘要；时点换源后现有凭证
   全部不再命中，首次显示等于整片重生成一遍（`prepare_cached_root_publication` 的廉价路径救不了
   几何计算本身）。这是切换成本，不是 bug；发布说明里写明。

## 取舍

- **对拍失去「同一时点」前提。** ADR-053 Q3-A 的价值是 db/direct 双跑读同一版数据。取 A 之后，
  对拍要显式给 `Sesno(applied_sesno)`（Q2 路径）才能复现旧前提——探针 `direct_gen_smoke` /
  `direct_attmap_probe` 改成带时点跑即可，代价可控。
- **模型与属性面板可能短暂分叉。** DB 模式下属性面板 / 搜索仍读 SurrealDB 里 `applied_sesno` 那一版，
  模型却是文件最新版；摄入追平后自然合流。用户认可这一点是「点看即最新」的必要代价；plant-ui 的
  回退阻断卡本来就同时展示两端时刻（ADR-0019），足以让人看出差异来自哪里。
- **不动数据管线。** 暂存窗口、水位推进、房间管线、durable pending 的形状全部不变（ADR-053 Q1-A 仍成立）；
  本 ADR 只改「生成读哪一版」与「凭证怎么判当前」。

## 后果

- 零解析部署（`AIOS_DATA_READ_MODE=direct`，或从未运行过摄入）可以直接 ensure 出模型；
  `dbnum_watermark` 空表不再是阻断条件。
- 已解析部署的行为变化只有一条：文件比库新的时候，点看拿到的是文件那一版。
- 涉及文件（实施清单）：`src/data_interface/direct_store.rs`、`src/data_interface/model_update_pending.rs`
  （`root_model_source` / `ensure_regen_pending_current` / `generation_root_cache_current` /
  `gen_root_credential_is_current` / `sync_and_seed_model_coverage` / `model_coverage_current`）、
  `src/fast_model/e3d_model_service.rs`（`from_current` / `pin` / `generate_roots` / `generate_dbnum` /
  `apply_window` / `dbnum_for_roots`）、`src/data_interface/model_refresh.rs`（`generate_roots_report`
  的 dbnum 来路）。`web_service/handlers.rs` 的 `/model/ensure` direct 分支已从 e3d-io 取根，不动。

## 开放问题

- **Q4** 单调判据下，一条按更旧窗口右端排进来的 regen 工作项撞上已发布的更新版本，收口成
  `AlreadyAvailable` 还是新造一个「已被更新版本覆盖」状态？倾向前者（对客户端是同一件事）。
- **Q5** `ModelTarget.catalogue` 若随 MDB 成员把 CATA 库也列进 pin，凭证身份多出目录库的
  `(file, session)` 段——是要的（目录变了模型该重生成）还是多余的（`hydrate_published_dependencies`
  已按实际依赖登记）？实施时按 `model_target()` 的现状拍。
