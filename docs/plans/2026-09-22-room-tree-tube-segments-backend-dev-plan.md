# 房间层级树里列出直段（方案 B）· T1 后端执行计划

> 来源：plant3d-web `docs/plans/2026-09-21-room-tree-tube-segments-plan.md`（小计划，`fc67d339`）§3.1 B1–B4 / §4 T1；用户 2026-09-22 11:19
> 拍板「D1–D7 全按推荐，从 T1 后端开工（先写后端执行计划再动代码）」。上位：plant3d-web ADR 0068（房间层级树，2026-09-21「管件带直段」追记）；
> 本仓 ADR-072 D4（隐式管身不进空间树——**保持**）；spec `docs/specs/web-service-api.md` §4.13.5 / §4.13.6。
> 基线：本仓 `main@579b1d8b2`（`:8027` 验证实例 `65dacd576`，mem 档，AMS 7997，`room_membership=true`）；plant3d-web `main@151290c1`。
> 状态：**T1 代码已落地**（计划 `fa744490a` 先落一笔，代码随后一笔：§3 T1-a…f 全部，单测 13 + 37 + 274 绿、clippy 触及文件 0 告警；
> 真机 `:8027` 账见 §4 与 plant3d-web 验证 README §10）。T2 另立一笔。
> 已拍板：D1 (b) 按树里已有的 BRAN 单元补 / D2 独立 `tube_count` / D3 四元组身份 + 落盘补 `route_ordinal`（T2）/ D4 (i) 两端管件任一属该房 /
> D5 (i) 前端只读（T3）/ D6 `tubes=` 缺省关 / D7 直段计入 `leaf_count`、共用 `leaf_cap`。

## 0. 一句话

树路由（`nearby/tree` / `rooms/{refno}/tree`）加一个 **opt-in 参数 `tubes=1`**：折完管件之后，对树里出现的每个 **BRAN 单元**读它根名下的隐式管身
记录（内存投影有回执就取投影，没有就冷读 `tubi_relate`），按与管件**同一把尺**量距离、过范围，按两端管件的房间归属挂到该单元下，
输出 `TreeUnit.tubes[]`（`TreeTube` 行）与逐层 `tube_count`、全树 `total_tube_count`；`count` / `total_count` / 38 条金样 / 缺省响应**一个字节不变**。
不建管身 R*-tree、不碰快照 / 启动判据 / `/health`。

## 1. 基线盘点（本仓，行号对 `main@579b1d8b2`）

| 积木 | 位置 | 与本需求的关系 |
| --- | --- | --- |
| 树路由流水线：`nearby_tree` = 核房间 → `scan_global_tree`（读锁内解中心 + 取候选）→ `resolve_rooms_for_scan`（async）→ `assemble_off_runtime`（`spawn_blocking`）里 `assemble_tree` | `src/fast_model/spatial_query.rs` L1342–1373、L1296–1306 | 直段读取有一步可能要 **await**（冷读库），所以折树拆成两段（§2.2），中间插一段 async |
| `assemble_tree`：`filter_candidates` → `sort_hits(Distance)` → 按 refno 去重 → 每条命中 `member_rooms` × `unit_root_of` → `per_room[房间][spec].units[单元 noun][单元 refno] = UnitAcc{root, members}` → 折成 `TreeResponse`；`inline_all = only.is_none() && leaf_count <= leaf_cap` | `src/fast_model/spatial_tree_view.rs` L417–612（`UnitAcc` L400–405、`SpecAcc` L407–413） | 直段挂在 `UnitAcc` 旁新加的 `tubes` 槽里；内联规则 / 选择器原样复用 |
| 响应形状 `TreeLeaf` / `TreeUnit{refno,noun,name,count,min_distance,elements?}` / `TreeUnitType` / `TreeOtherNoun` / `TreeOthers` / `TreeSpec` / `TreeRoom` / `TreeResponse{total_count,leaf_count,leaves_inline,inlined?,…}`，全部 `serde(skip_serializing_if = Option::is_none)` 可选键 | `spatial_tree_view.rs` L303–398 | 新键一律 `Option` + skip：`tubes=` 关着时序列化结果与今天逐字节相同 |
| `TreeOptions{leaf_cap, only}`；`LeafSelector::parse`；handler `RawLeafSelector{unit, other_noun}` → `tree_options()` | `spatial_tree_view.rs` L237–250、L204–234；`src/web_service/handlers.rs` L1549–1576 | `RawLeafSelector` 加 `tubes`，`TreeOptions` 加 `tubes` / `tube_root_cap` |
| 候选距离的三把尺：点 + sphere `aabb_point_distance ≤ r`；点 + cube 与 `center±r` 相交；refno 目标与 `target±r` 相交、sphere 再 `aabb_min_distance ≤ r`；中心线走廊逐段探针 + `centerline_aabb_distance` | `spatial_query.rs` `collect_candidates` L694–731、`collect_corridor_candidates` L654–686 | 直段不在树上，要一个**对单个盒**的同口径函数（§2.3 `measure_box_in_range`），钉「与树上取候选同判据」 |
| 过滤链：`include_self`（`center.subtree`）→ 负实体 → `dbnums` → `rooms`（`RoomScope::contains`）→ facet 计数 → `nouns` → `spec_values` → `keyword` | `spatial_query.rs` `filter_candidates` L841–921 | 直段按 §2.3 走能走的那几格 |
| `Center{point, source, target_box, subtree, centerline}` | `spatial_query.rs` L525–537 | 量直段距离 / 自身剔除的输入 |
| `ElementAttributes{dbnum_of, name_of, spec_value_of, unit_root_of}`；`DirectTreeService` 实现；`DirectTreeService::noun_of(refno) -> anyhow::Result<String>` 已有 | `spatial_query.rs` L402–435；`src/data_interface/direct_tree.rs` L256 | 直段行要两端 noun：命中里有就用命中的，没有再 `noun_of`（加一个带缺省的 trait 方法） |
| `RoomScope{rooms, members, member_rooms: HashMap<RefU64, Vec<usize>>, unresolved, …}`——**没有房间盒** | `src/fast_model/spatial_rooms.rs` L173–188 | D4 (i)：直段的房 = `member_rooms[from] ∪ member_rooms[to]` |
| 内存投影：`ProjectedGeometry{source_refno(=容器 BRAN), dbnum, world_transform: DMat4, world_bounds, shape, tube: Option<ProjectedTube{route_ordinal, from, to, invalid}>}`；`ModelMemoryStore::root_snapshot(Current, root)`（有回执才 `Some`，与 `/model/records` 同一谓词）/ `records_of_root` | `src/fast_model/model_memory_store.rs` L76–106、L1396–1434 | 直段的内存来源；**不改这个文件**（别的会话正在改它，d-149 补丁） |
| 冷读：`model_record_reader::records_of_root`（回执在 → 投影，否则 `durable_records_of_root`）；两路作用域读各走 `idx_<表>_anc` / `idx_<表>_in` 再 `array::union`（一根 2–3 ms；合成 OR 会退回 `Iterate Table`）；`tubi_projection` 只投 `is_tubi` / `is_invalid_tubi`，**没有两端** | `src/fast_model/model_record_reader.rs` L12–35、L45–117；钉 `durable_cold_read_scopes_are_planned_through_indexes` L385 | 加一条**只读 `tubi_relate`、只按 `in`** 的窄读（管身行的 `in` 就是容器 BRAN，不需要 `anc` 那一路），同样钉 EXPLAIN |
| 落盘行：`tubi_relate:[pe:⟨BRAN⟩, 分支全局槽位]`，字段 `in`（BRAN）/ `out` / `leave` / `arrive` / `aabb` / `world_trans` / `bore_size` / `invalid` / `anc` / `dbnum` / `model_sesno`——**没有 `route_ordinal`** | `src/fast_model/model_db_adapter.rs` L2652–2668 | 冷读能拿到两端与盒；序号 T1 按 `(from, to)` 内出现序补号，T2 落字段 |
| 索引：`idx_tubi_relate_anc` / `idx_tubi_relate_dbnum` / `idx_tubi_relate_in` | `src/fast_model/pdms_inst.rs` L145–154 | 窄读走 `idx_tubi_relate_in`；`IN [...]` 多成员是否走索引在本 fork 2.1.4 上要 EXPLAIN 复核（L257–263 的提醒），不走就退成逐根等值、一次多语句 |
| 生成根 = 最小交付单元（交付单元优先，进了单元之下不再出根）；BRAN 是自己的生成根 | `src/data_interface/generation_root.rs` L599–611 | `root_snapshot(BRAN)` / 冷读 `in = BRAN` 都以 BRAN 为键成立 |
| 隐式管身不进树：`projected_tree_entry` 见 `tube.is_some()` 回 `None`（ADR-072 D4） | `src/fast_model/aabb_tree.rs` L11–30 | **保持**；本计划只在树路由响应里按单元补 |
| e3d-model：`route_ordinal` = 同 `(from, to)` 站点对内的出现序（`slot_ordinals`）；直管 = 单位圆柱 `z∈[0,1]` 摆到 `T(0,0,-L/2)·S(r,r,L)`，`persisted_world_transform` 把它折进存下的世界变换 | `../vendor/e3d-model/src/pipeline.rs` L976–979、`route.rs` L425、`primitive_instance.rs` L266–286；本仓 `e3d_model_service.rs` L3880–3892 | 冷读补号与内存 `route_ordinal` 同一规则；`length` = 世界变换 z 轴列的模 |
| 金样脚本 38 条检查（HTTP，真机 `:8027`） | plant3d-web `docs/verification/spatial-room-hierarchy-tree-2026-09-20/spatial-tree-check.ps1` | 一条不改；`tubes=1` 另加两条（§4） |

真机数（09-21，AMS 7997）：BRAN `24381_105030` `model/records` 58 条 = 36 管件 + **22 段 TUBI**；R432 整房树 1298 管件（BRAN 28 单元）、3 m 球 1055（BRAN 17 单元）。

## 2. 设计

### 2.1 参数与响应形状（D2 / D6 / D7）

- **参数** `tubes=`：`RawLeafSelector` 旁新加一格（`parse_flag`，接受 `1/true/yes/on` 与 `0/false/no/off`，非法 400），**缺省关**。两条树路由都认。
- `TreeOptions` 加 `tubes: bool`（缺省 `false`）与 `tube_root_cap: usize`（缺省 `DEFAULT_TUBE_ROOT_CAP = 500`，§2.6）。
- **响应**（只在 `tubes=1` 时出现，全部 `Option` + `skip_serializing_if`；关着时与今天**逐字节相同**）：

```text
TreeTube  { ordinal: u32, from: "a_b", to: "a_b", from_noun: "ELBO", to_noun: "REDU", distance: f32, length: f32,
            aabb: { min: [f32;3], max: [f32;3] },   // 与 nearby 结果的 aabb 同形（BoxJson）
            invalid: bool, shared_rooms?: usize }
TreeUnit  { …, count, tube_count?: usize, min_distance, elements?, tubes?: Vec<TreeTube> }
TreeUnitType / TreeSpec / TreeRoom { …, count, tube_count?: usize }
TreeResponse { …, total_count, total_tube_count?: usize, tubes_truncated?: bool, leaf_count, … }
```

- `count` / `total_count` **不变**（只算管件，D2）。`tube_count` 是「该节点下、落在范围内、属于该房间的直段放置数」；`total_tube_count` 按身份键
  去重（跨房直段只算一次，同 `total_count` 口径）。
- `tubes[]` 的内联规则与 `elements[]` **同一套**（`inline_for(selected)`：`leaves_inline=true` 全带；超上限省略；`unit=` 点到的单元带）。
  直段放置数**计入 `leaf_count`、共用 `leaf_cap`**（D7）：`inline_all` 的判据在直段并进来**之后**算。
- 排序：单元内 `tubes[]` 按 `distance`、再 `(from, to, ordinal)`。`min_distance` 仍只看管件。

### 2.2 直段从哪来（D1 (b)）：折树拆成两段

`assemble_tree` 今天整段跑在 `spawn_blocking` 里，而冷读要 `await`。拆成：

```text
draft_tree(scan, params, attrs, scope, unit_types, options) -> TreeDraft   // 第一段（阻塞线程）：过滤 / 排 / 去重 / 单元 / 折入 per_room
TreeDraft::tube_roots(&self) -> Vec<RefU64>                                 // tubes 开着才非空：树里出现的 BRAN 单元 refno，去重、按最近成员距离排，截到 tube_root_cap
load_tube_supplement(roots) -> anyhow::Result<TubeSupplement>               // async：投影有回执的根取 root_snapshot 里 tube.is_some() 的记录；其余一次冷读
TreeDraft::finish(self, supplement) -> TreeResponse                         // 第二段（阻塞线程）：量距离 / 过滤 / 归房 / 挂单元 / 计数 / 折响应
assemble_tree(...) = draft_tree(...).finish(TubeSupplement::default())     // 旧签名保留，tubes 关着时走它，行为不变
```

`nearby_tree`：`draft` 在 `assemble_off_runtime` 里出；`options.tubes` 为真才 `load_tube_supplement`；`finish` 再进一次 `assemble_off_runtime`。
`TreeDraft` 要 `Send + 'static`（`Vec<Hit>` / `BTreeMap` / `RoomScope` 克隆 / `Arc<dyn ElementAttributes>`）。

**只对 BRAN 单元读**（`UnitRoot.noun == "BRAN"`，管道与风管分支都是 BRAN；HANG / SUPPO / EQUI 没有隐式管身，不读——落盘形态下每根一次冷读，
R432 整房 226 单元里读 28 根与读 226 根差一个量级）。

`TubeRecord{ bran: RefU64, ordinal: u32, from: RefU64, to: RefU64, aabb: Aabb, length: f32, invalid: bool }`：

- 内存路：`ProjectedGeometry` → `bran = source_refno`、`ordinal / from / to / invalid` 取 `ProjectedTube`、`aabb = world_bounds`（不可用的盒丢弃，
  同 `usable_bounds`）、`length = |world_transform 的 z 轴列|`；
- 冷读路（§2.7）：`bran = record::id(in)`、`from / to = record::id(leave / arrive)`（缺任一端的历史行丢弃）、`ordinal = route_ordinal ?? 按同 (from, to) 内
  出现序（行按 `record_order` 排）补 0、1、2…`、`aabb = aabb.d`、`length = |world_trans.d 的 z 轴列|`（`scale.z`）、`invalid = invalid ?? false`。

### 2.3 直段的过滤链（与管件同一把尺）

对每条 `TubeRecord`，按 `filter_candidates` 的顺序走**它能走的**几格；走不了的格由「它挂在哪个单元下」隐含：

1. **范围**：`spatial_query::measure_box_in_range(center, params, &aabb) -> Option<f32>`，与 `collect_candidates` 逐分支同判据（点 + sphere：
   `aabb_point_distance ≤ r`；点 + cube：与 `center±r` 相交；refno 目标：与 `target±r` 相交、sphere 再 `aabb_min_distance ≤ r`；中心线：任一段探针相交、
   `centerline_aabb_distance`、sphere 再 `≤ r`）。`None` 丢弃。单测用同一夹具对着 `collect_candidates` 的结果钉「树上一个盒两种取法答案相同」。
2. **`include_self`**：`exclude_subtree && center.subtree.contains(bran)` 丢弃（容器在目标子树里，它的直段是目标自己的）。
3. **负实体**：不涉及（TUBI 不是负实体）。
4. **`dbnums`**：隐含——直段与它的 BRAN 同库，BRAN 单元能出现说明成员过了 `dbnums`。
5. **`rooms`**：§2.4。
6. **`nouns`**：`params.nouns` 非空且不含 `TUBI` → 全部直段丢弃（点名了类型却没点 TUBI，直段不该冒出来）。
7. **`spec_values`**：隐含——直段的专业就是 BRAN 单元的专业，单元能出现说明成员过了 `spec_values`。
8. **`keyword`**：`keyword_matches(keyword, bran, "TUBI")`——直段没有 refno，用容器 refno + `TUBI` 参与同一个谓词。

### 2.4 房间归属（D4 (i)）

`ends = member_rooms[from] ∪ member_rooms[to]`（房间下标集合；容器头 / 尾那一端是 BRAN 自己，解不出，正常）。

- `ends` 非空：直段放到 `ends ∩ {该 BRAN 单元出现的房间}` 的每一间——只往**已有的单元节点**下挂，成员都被过滤掉的房间没有节点可挂；交集为空的直段
  不放置（不算 `unresolved`，它属于别的房）。
- `ends` 为空（两端都不是任一所选房间的成员）分两档（落地时收窄，真机 R432 整房暴露：8 段穿过房间盒、两端却是邻房的 BEND / UNIO / VALV，
  它们旁边的管件正是这样被 `rooms=` 滤掉的）：任一端**在范围里**（是候选，`TreeDraft::candidate_refnos`）却不是成员 = 已经判过、属于别处，
  直段跟着不列；两端都**不在范围里** = 解不出，按推荐兜底，放到该 BRAN 单元出现的**每一间**房。
- 放到 ≥ 2 间房的直段带 `shared_rooms`；`total_tube_count` 只算一次。

### 2.5 身份键与序号（D3）

`(BRAN, from, to, ordinal)` 四元组，内存路直接来自 `ProjectedTube`；冷读路 T1 按出现序补号（§2.2），**T2** 再把 `route_ordinal` 写进 `tubi_relate` 行
（`render_records` / 参数形态各多一格，`INSERT IGNORE` 追加字段，老行读成 `NONE` 仍走补号）。同 `(from, to)` 多段是罕见形态（`route_ordinal` 常为 0），
补号与真值不一致的窗口只在这类 BRAN 的冷读上、且 T2 之后消失——写进 spec。

### 2.6 预算（`tube_root_cap`）

`TreeDraft::tube_roots()` 把 BRAN 单元按「最近成员距离、再 refno」排，只取前 `tube_root_cap`（缺省 500）根；超出 → `tubes_truncated = true`、
`warnings` 加一句「直段：BRAN 单元 N 个超过预算 500，只为最近的 500 个补了直段」。R432 3 m 球 17 根 / 整房 28 根远在预算内；100 m 多房那档
（`05b` 金样，BRAN 单元以百计）被它兜住，mem 档每根一次 HashMap 取、落盘档一次多语句冷读（§2.7），都不许把树路由拖成秒级。

### 2.7 冷读 SQL（`model_record_reader.rs`）

管身行的 `in` 就是容器 BRAN，所以只要 `in` 一路、不要 `anc` 一路：

```sql
SELECT record::id(in) AS container, record::id(leave) AS tube_from, record::id(arrive) AS tube_to,
       route_ordinal AS tube_ordinal, aabb.d AS world_aabb, world_trans.d AS world_trans, invalid ?? false AS invalid,
       id AS record_order
  FROM tubi_relate
 WHERE in = pe:⟨a_b⟩ AND aabb.d != NONE AND world_trans.d != NONE
 ORDER BY record_order;
```

一次调用把冷根拼成**一条多语句**（每根一句、各走 `idx_tubi_relate_in`，`take(i)`），不写 `in IN [...]`——`pdms_inst.rs` L257–263 记着本 fork 对 `IN`
的索引联合支持没复核过，等值是最朴素的形态。钉：新单测对 `mem://` 建 `INST_RELATE_INDEX_SQL` 后 `EXPLAIN FULL`，计划不含 `Iterate Table` 且落在
`idx_tubi_relate_in`。行按 `record_order`（槽位）排，补号按此序。

## 3. 分步

| 步 | 文件 | 内容 | 钉 |
| --- | --- | --- | --- |
| T1-a | `spatial_query.rs` | `measure_box_in_range(center, params, aabb) -> Option<f32>`（pub）；`ElementAttributes::noun_of(refno) -> Option<String>` 带缺省 `None`，`DirectTreeService` 实现走 `noun_of` | 单测：夹具树上每个盒 `collect_candidates` 命中 ⇔ `measure_box_in_range` 为 `Some`，且距离相等（点 / refno / cube / sphere / 走廊五档） |
| T1-b | `model_record_reader.rs` | `DurableTubeRow`、`durable_tube_rows_sql(branches)`、`durable_tube_rows_of_branches(db, &[RefU64])` | 单测：`mem://` 造 3 行（两端齐 / 缺 `arrive` / 别的 BRAN）读回 2 行、顺序按槽位、`tube_ordinal` 为 `None`；EXPLAIN 钉索引 |
| T1-c | 新 `spatial_tree_tubes.rs` | `TubeRecord` / `TubeSupplement{by_root, truncated}`；`from_projected(&ProjectedGeometry)`、`from_durable(rows)`（补号）；`load_tube_supplement(roots, cap)`（回执 → 投影 / 否则冷读） | 单测：投影记录 → `TubeRecord`（`length` 取 z 列模、坏盒丢）；冷读行补号「同 (from,to) 0,1,2、无关段 0」 |
| T1-d | `spatial_tree_view.rs` | `TreeTube` / 各层 `tube_count` / `total_tube_count` / `tubes_truncated`；`TreeOptions.tubes / tube_root_cap`；`draft_tree` / `TreeDraft::tube_roots` / `finish`；`assemble_tree` 变薄壳 | 单测（夹具加两条 BRAN、五段直管）：范围内 / 外、两端同房 / 一端别房 / 两端都解不出（兜底）、`tube_count` 逐层、`total_tube_count` 去重、`leaf_count` 含直段且 `leaf_cap` 共用、`unit=` 只内联一组、`nouns=ELBO` 不出直段、`nouns=ELBO,TUBI` 出、预算截断 `tubes_truncated` + warning；**`tubes` 关着时 `serde_json::to_value` 与旧响应相等** |
| T1-e | `spatial_query.rs` / `handlers.rs` | `nearby_tree` 两段 + 中间 `load_tube_supplement`；`RawLeafSelector.tubes` → `tree_options()`（`parse_flag`） | 单测：`tubes=abc` 400；既有路由只读钉不动 |
| T1-f | 文档 | spec §4.13.5 加「`tubes=1`」一段（形状、过滤链、归属、预算、孤直段与补号的两条已知漏洞）、§4.13.6 一句透传；ADR-072 D4 追一句「直段不进主树，树路由按单元补（本计划）」；`changelog.md` 2026-09-22 一条；`CONTEXT.md`「空间邻近查询」加词条「直段行 (Tube Row)」 | — |

顺序 a → b → c → d → e → f，每步 `cargo test --lib <模块>` 绿再进下一步；一笔提交（含本计划的更新），`git show --stat` 只含上表文件 + 本计划。

## 4. 验证口径

- `cargo test --lib spatial_query spatial_tree_view spatial_tree_tubes model_record_reader` 全绿；`cargo test --lib web_service` 的路由只读钉 / 源码钉不新增红；
  `cargo clippy --lib` 触及文件零新告警。
- 真机 `:8027`（mem 档；换上本笔二进制，重启后 7997 整库 ensure）：
  - `spatial-tree-check.ps1 -SkipEnsure`（ensure 单独跑）**38 条全过**（它们不带 `tubes=`，响应形状不变）；
  - `rooms/24381_35580/tree?tubes=1`：BRAN `24381_105030` 下 `tube_count` ≤ 22（= 该根 `model/records` 里 TUBI 数；范围 = 整房，直段都在房里应相等或更少，
    多了就是 bug）；`Σ 内联 elements + Σ 内联 tubes = leaf_count`；各层 `Σ tube_count` 自洽；`total_tube_count` ≤ `Σ rooms[].tube_count`；
  - `nearby/tree?refno=24381/35580&radius=3000&shape=sphere&rooms=24381_35580&tubes=1`：每段 `distance ≤ 3000`，`from`/`to` 之一在该单元 `elements[]` 里或
    是 BRAN 自己；
  - `tubes=1` 与不带 `tubes` 的同参响应：去掉 `tube_count` / `tubes` / `total_tube_count` / `tubes_truncated` 键后 JSON 相等；
  - 100 m 多房（`05b` 那档）`tubes=1` 耗时 ≤ 不带 `tubes` 的 +20%，或 `tubes_truncated=true` 有 warning；
  - 这两条（`03c-nearby-tree-single-room-tubes.json` / `07c-room-tree-tubes.json`）作为 **+2** 加进 `spatial-tree-check.ps1`（plant3d-web 侧提交，随 T3 或单独一笔）。
- 每条声称完成附「跑了什么、结果如何」；跑不了的标未验证 + 原因。

## 5. 不做 / 已知漏洞

- 不建管身 R*-tree、不改快照格式 / 启动判据 / `/health` 十六键（ADR-072 D4 保持）；(a) 留作「(b) 的漏洞真被踩到」之后的升级路。
- **孤直段**：一条 BRAN 的管件全在范围外、只有直段穿过范围——这根不在树上，直段补不出来。写进 spec。
- **补号窗口**：同 `(from, to)` 多段的 BRAN 在冷读路上按出现序补号，与内存 `route_ordinal` 可能不一致，T2 落字段后消失。写进 spec。
- `length` 取世界变换 z 轴列的模，依赖直管是标准圆柱实例（`instance_cache_enabled()`，生产恒真；调试环境变量关掉实例化时直管烘焙、这一格无意义）。
- 平铺态（`nearby`）不列直段；HANG / EQUI 不涉及；`count` 语义不改；不给直段行显隐（T4 另议）。
- 不改 `model_memory_store.rs`（别的会话在改；本计划用它已有的 `root_snapshot`）。

## 6. T2（已落地，另一笔）

`model/records` 的每条记录多一格 `tube?: { ordinal, from, to }`——放在**记录一级**而不是小计划写的 `GeomInst.tube?`（`insts[]` 的元素
`ModelHashInst` 是 `aios_core` 的类型，不为这一格跨仓改它；一条管身记录恰好一个实例）：`model_record_reader::model_records_of_root` /
`ModelRecord { flatten GeomInstQuery, tube? }`，内存路填 `ProjectedTube`；冷读路 `durable_records_sql` 的管身投影加 `tube_from` / `tube_to` /
`tube_ordinal`（`records_of_root` 的 `GeomInstQuery` 不认这三格、原样忽略），`route_ordinal` 缺的按同 `(容器, from, to)` 内出现序补号；
`render_records` / `bind_records` 给 `tubi_relate` 行补 `route_ordinal`。spec §4.5.2「`tube`」。是前端 T4「逐段眼睛」的前置，T1 / T3 不依赖它。
单测在钉住 vendor 旧版的隔离 worktree 里跑（主仓工作树当时因 vendor `e3d-io` 新变体编不过，见 changelog）；真机 `:8027` 换上同一 worktree 的
二进制后 R432 整房 28 根 BRAN 的 340 条 TUBI 记录条条带 `tube`、树里 159 段直段逐条对得上、去掉 `tube` 后记录与旧版逐字相同、金样 41 / 41（changelog）。
