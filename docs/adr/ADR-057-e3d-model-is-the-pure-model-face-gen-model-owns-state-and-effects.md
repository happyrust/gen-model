# ADR-057：e3d-model 是模型面的纯函数层；模型面的状态与副作用归 gen-model

状态：**提议**（2026-09-02 起草；起因是用户 2026-09-02 问「现在模型生成和模型增量，不是都应该发生在 e3d-model 吗」，
分析后选定「把边界写成 ADR」——见 `上下文` 同日会话记录；**待用户拍板后**改「已接受」并 `record_decision`，与 d-52 / d-58 并列）；
**未实施**（实施约束 1 随本 ADR 同批落地；2–4 按 spec 035 P2 的节奏推进）
日期：2026-09-02
关联：
- **承接** ADR-056（kv-mem 退役；D3「模型增量执行粒度为根级」、N7「模型面不以 `pe` 行为前置」是本 ADR 的前提，本 ADR 把它们
  背后那条隐含的 crate 边界写成显式规则）、ADR-054（生成时点 = 显式指定或文件最新，共识 d-38）、ADR-053（direct 模式生成读）、
  ADR-014（分支原子替换，以根级 CAS 发布的形态成立）、ADR-050（`model_update_pending` 进程本地）；
- **修订** ADR-056 P2-1 的一处措辞：`touches_roots` 抽成 e3d-model pub 函数这一条从「计划项」升为「边界要求」（本 ADR D5）；
- **不动** ADR-001（`applied_sesno` 是数据水位）、ADR-025 §7（数据批次只提交数据 + 水位 + 模型意图）、ADR-029（CSG 内核）；
- 证据：`docs/plans/2026-08-31-core-aligned-increment-architecture.md` §3.1（`toplevel` / `is_model_unit` 两层粒度实测）、
  `docs/plans/2026-09-02-increment-update-audit-and-next-plan.md`（S2 两套规划器、S8 direct 无变更检测）、
  `docs/plans/2026-09-02-e3d-model-increment-direct-rocksdb-plan.md` §1 F3–F5、§4 P2-1/P2-2。

## 背景

`vendor/e3d-model` 的自我定位写在 `src/lib.rs` 首行：**「数据一律经 `e3d-io` 直读 dabacon 库文件，不连任何数据库」**。
到 2026-09-02 它已经同时持有模型面的两件事：

- **全量生成**：`pipeline::generate_subtree(_with_cache)` → `elmodl::model_element` → `solid` / `route` / `catalogue` / `transform`
  （对标 core.dll `ADDDES → MODCMP → ELMODL → libgm`）；
- **增量判定 L0–L4**：`increment::collect_window → plan_update → UpdatePlan / AffectedClosure`，加 `element_diff` 与 `ledger`
  （对标 `DB_IndexTableCompare → DB_Compare::checkEle → DB_UserChanges → findTopLevelElement`），以及单元级执行 `execute_plan`。

ADR-056 能把 kv-mem 暂存层拆掉，唯一的前提就是这一条：生成器不再从 `pe` / `ATT_*` 取数，所以「先把数据暂存起来让生成器读到
自己刚写的行」这个需求不存在了。**这条前提今天只是 e3d-model 的一句模块注释，不是任何地方的规则。**

同一天的审核把三处漂移摆到了一起，都指向「边界没写死」：

| # | 漂移 | 现码位置 |
|---|---|---|
| G1 | **生产路上的增量判定还没接 e3d-model**：legacy worker 用 `model_impact::classify_operation_impact`（noun/属性名单三态）+ old-pdms-io 净窗口选根；生产实际跑的 direct 模式没有任何窗口差分，每次 SAVEWORK 让该库全部凭证过期（审核 S8）。今天新建的桥 `window_root_plan.rs` 已调 `plan_update`/`AffectedClosure`，但尚未替换 `apply_one` 调用点 | `model_update_plan.rs:260`、`model_impact.rs:403`、`window_root_plan.rs:37/168/304` |
| G2 | **e3d-model 的增量「执行」半边将没有生产消费者**：`increment_update` / `execute_plan` 在 gen-model 侧唯一的入口是 `E3dModelService::apply_window`，而它只从暂存路径 `run_unit_worklist(Some(window))` 可达，spec 035 P1 要删这条臂（ADR-056 D3：执行是根级 `generate_roots`）。**2026-09-02 14:3x 追记：T121 已落地，`model_refresh.rs::apply_window` 已标 `dead_code`**——G2 从「将要发生」变成「已经发生」，读代码的人现在就会问「为什么增量不在这里执行」 | `e3d_model_service.rs:16/293`、`model_refresh.rs::apply_window`（dead_code） |
| G3 | **根枚举是纯文件函数却在 gen-model**：`generation_root.rs::enumerate_generation_roots_in_subtree(root, unit_types, lookup)` 与 e3d-io 适配 `enumerate_generation_roots(set, roots, unit_types)` 只读 `DbSet`、不碰库（N7 要求的），与 e3d-model 的 `nearest_unit` / `is_model_unit` 是同一棵树上的两种粒度，却分居两个 crate，只靠 `include_str!` 护栏和记性对齐 | `generation_root.rs:562`、`direct_tree.rs:173/253` |

## 决策（提议；用户拍板前按推荐项落笔）

一句话：**对 `(库文件, 会话)` 是纯函数的，进 e3d-model；要碰 SurrealDB、凭证、队列、锁、房间、空间树、MQTT 的，留 gen-model。**

| # | 决策点 | 结论 |
|---|---|---|
| D1 | e3d-model 的职责边界 | **模型面的纯函数层**：全量生成、窗口差分 L0–L4、单元与根两层上卷、**根枚举**、目录 / P 点 / 表达式求值、世界变换、网格离散。输入身份只有 `(文件路径, sesno)` 与显式传入的配置 / 缓存（`unit_types`、`CatalogueMeshCache`）；输出是 `GeneratedElement` / `UpdatePlan` / `AffectedClosure` 这类不带句柄的值。**不持 DB 句柄、不产生副作用、不认识 SurrealDB / 凭证 / 队列 / 水位** |
| D2 | gen-model 模型面的职责 | **模型面的状态与副作用**：`gen_root` 凭证（含前移）、`RootPublishClaim` CAS 发布、`ModelTarget` 指纹与 `published_manifest_hash` 去重、mesh 文件（`e3d_mesh_store`）、durable 队列 `model_update_pending` 与重试账、`db_generation_lock`、房间归属 / 空间树 / MQTT 通告、HTTP 面。加上**数据面全部**（ADR-056） |
| D3 | `execute_plan` / `increment_update` 的定位 | **参考实现 + 离线工具**：给 `update_ams` bin、`increment_real.rs` 真库门、`increment_planner_parity` 对拍用，证明 L4 上卷「先擦后画」的语义闭合。**不是生产执行器**——生产执行 = 根级 `generate_roots`（ADR-056 D3）。**不删除**（它是 L0–L4 语义的可执行规格），**不在生产接线**；单元级落库若要做，另立 ADR 解决 `gen_root` 凭证 / cohort CAS / scoped delete 的单元级对应物。e3d-model `increment.rs` 模块文档须写明这一定位（实施约束 1） |
| D4 | 根枚举归属 | **迁入 e3d-model** 作 pub 纯函数（拟 `e3d_model::roots::{enumerate_generation_roots_in_subtree, enumerate_generation_roots}`），`unit_types` 仍作参数由 gen-model 从项目配置传入——MDU 名单是**交付**概念、归 gen-model 配置，但「按名单在树上找根」是纯遍历、归 e3d-model。gen-model `generation_root.rs` 只留 `gen_root` 读侧（`roots_S`）与 `plan_window_roots` 分桶（regen / delete / advance / lazy），`direct_tree.rs::generation_roots_in_subtree` 改为委托 e3d-model |
| D5 | `touches_roots` 归属 | 判「根被本窗口触到」= `AffectedClosure::contains` 已在 e3d-model；把 ADR-056 P2-1 计划的 `UpdatePlan::touches_roots(&[RefNo], base, target) -> BTreeSet<RefNo>` 从计划项升为边界要求：**上卷到根这一步在 e3d-model 完成**，gen-model 只拿结果分桶、写凭证 |
| D6 | 依赖方向 | `gen-model → e3d-model → e3d-io / e3d-attlib` 单向。e3d-model 的 `[dependencies]` **不得出现** `surrealdb` / `aios_core` / `pdms_io` / `tokio`；不得 `async`；无进程级可变全局（缓存显式传参）。反向任何一条成立即视为边界破坏 |
| D7 | 两层粒度并存 | e3d-model 内**同时成立**两层：`is_model_unit`（网格产出单元，`BOX` 一件一网格）与生成根（MDU / significant owner，交付与凭证单元，`EQUI` / `BRAN` 一根一凭证）。**互不替换**（§3.1 实测两个集合互不包含：`BOX` 是单元不是根，`EQUI` 是根不是单元）；D4 迁入的是第二层的**枚举**，不是把第一层换成第二层 |

## 新不变量

- **B1 纯函数**：e3d-model 任一 pub 函数对同一 `(文件, sesno, 显式入参)` 重复调用产出相同值；没有 I/O 以外的副作用（读文件是它唯一的 I/O）。
- **B2 白名单消费面**：gen-model 对 e3d-model 的消费面是有限清单（见「后果」），新增消费项须在本 ADR 追记；**`increment_update` / `execute_plan` / `IncrementOutcome` 不在生产白名单里**。
- **B3 词汇隔离**：`凭证 / credential / CAS / manifest / revision / applied_sesno / gen_root` 只出现在 gen-model；`DbSet / IndexDiff / UpdatePlan / AffectedClosure / GeometryId` 是两侧共用的值类型，e3d-model 定义、gen-model 消费。
- **B4 时点由外部解**：任何「当前」「最新」「水位」都由 gen-model 解析成显式 `sesno` 后传入（ADR-054 `resolve_session`）；e3d-model 不解释「最新」。
- **B5 两层粒度各有判据、各有名字**：单元判据 `is_model_unit` / `is_derived_unit`，根判据 `is_delivery_unit_noun` 优先、`noun_is_significant` 兜底；任一层改判据都必须过 `increment_real.rs` 五窗与 `live_dbset_enumeration_matches_direct_store_enumeration`（ams8000_0001@266：949 根 / 2 WORL）两道门。

## 实施约束（实施时逐条核，不得静默绕开）

1. **`increment.rs` 模块文档加「生产定位」一节**（与本 ADR 同批落地）：说清三段式里 `execute_plan` 是参考实现与离线工具，
   gen-model 生产只消费 `collect_window / plan_update / AffectedClosure`（选根）与 `pipeline::generate_subtree_with_cache`（整根重算），
   以及为什么（根级凭证 / CAS / scoped delete 没有单元级对应物）。不改代码、不改签名。
2. **D4 迁移不改行为**：`enumerate_generation_roots_in_subtree` 与 `subtree_element_from_set` / `enumerate_generation_roots` 原样搬进
   e3d-model（`SubtreeElement{noun, name, members}` 三格取数、每元素恰一次 lookup、容器守卫全部保留）；gen-model `generation_root.rs`
   与 `direct_tree.rs` 改为委托 / re-export；`include_str!` 护栏 `direct_tree_root_enumeration_is_the_shared_traversal` 改钉 e3d-model 路径；
   6 条单测随函数搬家，对拍 `live_dbset_enumeration_matches_direct_store_enumeration` 数字一个不变。与 spec 035 P2-1 同批。
3. **D5 与 ADR-056 P2-1 同批**：`touches_roots` 落在 e3d-model 后，`window_root_plan.rs::WindowRootSources::impact` 改为调它；
   `increment_planner_parity` 的 `ancestors_inclusive` 那段删掉、改调同一函数（对拍工具与生产不得各留一份判据）。
4. **边界护栏进测试**：e3d-model 加一条 `include_str!("../Cargo.toml")` 断言不含 `surrealdb` / `aios_core` / `pdms_io` / `tokio`；
   gen-model 加一条钉「`src/` 下 `e3d_model::increment::{increment_update, execute_plan}` 只允许出现在 `src/bin/` 与 `tests/`」。
5. **不在本 ADR 范围**：单元级落库（另立 ADR）；数据面收集器换底座（ADR-056 P4）；`category` 的 noun 表与 MDU 默认名单是字典还是配置（开放问题 Q1）。

## 取舍

- **否掉的 A｜把发布 / 凭证也搬进 e3d-model**（「模型的一切都在一个 crate」）：它会让 e3d-model 重新变成连库的 crate——
  ADR-056 的前提当场失效，429 个真库上的离线对拍（`update_ams`、`increment_real.rs`、`rvm_compare`）全部要起 SurrealDB 才能跑，
  而这些对拍是 e3d-model 能追 core.dll 口径的唯一手段。代价远大于「两个 crate 各改一处」。
- **否掉的 C｜维持隐含边界**：G1–G3 说明隐含边界已经在漂；`execute_plan` 一旦失去生产消费者，「为什么增量不在 e3d-model 执行」
  这个问题会被每一个新读者重新问一遍，答案却只在会话记录里。
- **接受的代价**：改一次选根判据可能要动两个 crate（e3d-model 判据 + gen-model 分桶）；`execute_plan` 作为没有生产流量的
  参考实现要靠真库门维持不腐——这两条都比重新引入 DB 依赖便宜。
- **顺带收掉的**：G3 之后「谁是根」与「哪些根被触到」来自同一个 crate、同一棵 `DbSet`，两层粒度的对齐从「记性 + 护栏」变成
  「同一编译单元」。

## 后果

- **gen-model 对 e3d-model 的生产消费白名单**（2026-09-02 `rg e3d_model::` 实测，B2 的基线）：
  `increment::{collect_window, plan_update, UpdatePlan, AffectedClosure}`（`window_root_plan.rs`）、
  `pipeline::{generate_subtree_with_cache, Report, Incident}`、`elmodl::{GeneratedElement, GeometryId}`、`catalogue::CatalogueMeshCache`、
  `primitive_instance::{canonical_primitive_mesh, PrimitiveInstance, PrimitiveMeshKey}`、`transform::dmat4_to_affine4x3`
  （`e3d_model_service.rs` / `e3d_mesh_store.rs`）、`category::{is_derived_unit, DERIVED_ROUTE_CONTAINER_NOUNS}`（`model_update_plan.rs`）。
  **过渡期例外**：`increment::{increment_update, IncrementReport}` 经 `apply_window` 的消费随 spec 035 P1 删除臂一起退出白名单。
  `src/bin/` 与 `tests/` 的探针 / 对拍不受白名单限制（`db_discovery::DirectoryDbResolver`、`element_diff::diff_element`、`ledger::ChangeKind`）。
- D4 落地后 gen-model 少一份纯遍历（约 `generation_root.rs` 的 `enumerate_*` 段 + 6 条单测），多一条委托；e3d-model 多一个 `roots` 模块。
- `CONTEXT.md` 新增「模型面 / 数据面 / 模型面状态」三个词条（本 ADR 同批）。
- ADR-056 P2-1 文字中「`touches_roots` 抽成 e3d-model 的 pub 函数」的性质由计划项改为边界要求，其余不动。

## 开放问题

- **Q1** `category` 的 noun 分类表（`data/noun-family-matrix.json`、`route-nouns.json`）是**字典事实**（随 E3D 版本走，归 e3d-model）；
  MDU 默认名单 `BRAN / HANG / SUPPO / EQUI` 是**项目配置**（归 gen-model `DbOption.toml`）。两者在 D4 迁移时以参数分开是清楚的；
  但 `noun_is_significant` 这一条兜底判据今天读的是 core `significant` 位快照，它算字典事实——迁移时确认它的数据源随函数一起进 e3d-model。
- **Q2** D3 的「参考实现」若长期没有生产流量，是否要把 `execute_plan` 缩成 `#[cfg(any(test, feature = "reference-executor"))]`？
  倾向不缩：`update_ams` 与真库门都在用，它有真实流量，只是不在 gen-model。
- **Q3** 房间归属 / 空间树今天从 gen-model 侧的 `inst_relate.aabb` 取盒；e3d-model 产出的 `TriMesh` 已含世界系顶点，AABB 是它的纯函数。
  要不要把「网格 → 世界 AABB」也划进 e3d-model 以避免两份 AABB 口径（ISSUE-028 RVM 快照 AABB 与自身面片不一致的那类问题）？
  等 D4 / D5 落地后按 ISSUE-028 的走向再议。
