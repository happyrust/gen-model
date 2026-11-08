# GPWL / GPSET 分组节点：创建、解析与加载显示

日期：2026-11-08 · 状态：待实施 · 证据基础：core.dll/Core3D.dll IDA 反编译 + aps/ams 真库 dump + desvir.dat 模板

## 0. 已确证的事实（不再猜测）

### 存储形态（真库 dump，aps250204 / aps250218 / ams）

| 层级 | 形态 | 证据 |
|---|---|---|
| WORL → GPWL | 主成员链 | `GPWL 2013286762/4068` owner=WORL |
| GPWL → GPSET | 主成员链，owner=GPWL | `members[10]: GPSET×10` |
| GPSET → GPITEM | 成员链上是 **GPITEM 元素记录**（owner=GPSET） | `members[10]: GPITEM×10`，ams `/FWD_BLOCK` 31 个 |
| GPITEM → 真身 | **GPREF 隐式属性** → 设计元素，**可跨库** | `GPREF=RefNo(…/40094, dbno=250209)` 而 GPITEM 在 250218 |
| GPSET 规则成员 | `SCOSEL` 序列化规则；纯规则组 `members[0]` | `/AREA01_PIPE` 等 27 组 |

### E3D 处理（IDA 反编译实锤）

- `DB_SecondaryTreeDefinition::firstMember`：`secondaryHierarchy==1` 的元素读 `SEXPND` 展开视图（树里 GPSET 下显示解引用的设计元素，GPITEM 不可见）。
- `DB_Element::secondAbove`：主祖先 ∪ SEXPND 成员的祖先。
- Core3D 注册：GPSET→`DB_PseudoGroupSMEMB/SEXPND`（成员链+GPREF 解引用+递归展开），GPWL→`DB_PseudoOldGroupSMEMB/SEXPND`（存储数组+去重）。
- GPWL/GPSET 的 NWEI/GWEI/RWEI 伪属性全返回 0 —— 容器不占显示槽，自身不出几何。
- `DES_DrawList::addRemoveSecondaryHierarchy`：GPSET 进视图 = 展开成员的 TOPF 子树进显示表。

### 模板（desvir.dat 2.10 describe 实锤）

- `GPITEM` 模板声明 `GPREF` + `GPRFTY` —— `insert`/`set_explicit_refno` 可直接写。
- `GPSET` 模板声明 `SCOSEL`（int 数组）—— `set_int_array_hunk`/`set_explicit_words` 可写。
- `GPWL` 模板声明 `RREF`/`RATM`/`RATS` —— GPWL 是外部引用宿主（RREF=被引库）。

### SCOSEL 字节码（样本聚类）

```
[word_count] 100016,1  100009,N  {100007,<attr_hash> 100015,<len> 1 <expr>* 402} | {100001,…}
expr ≈ 106,4,<attr> 1,120,0  1602,4,2,<attr> 6,1701  118,<len>,<ASCII> 402 … [301 = 子句合并]
```

- `100007` = 规则组（带表达式）；`100001` = 显式成员组（GPITEM 链）——可同存。
- `118,N` + N 个 ASCII 字 = 字符串字面量；`301` = 子句合并算子（GRID 组两条 NAME 前缀规则）。
- 求值器语义待 IDA 子代理回填（opcode 106/120/1602/1701/402 的确切含义）。

### gen-model 现状

- `pe_owner` 边把 GPITEM 当普通子节点落库；`/tree/children` 原样返回 GPITEM 行（物理图）。
- `GPREF`/`SMEMB`/`SEXPND`/`SCOSEL` 在 `src/` 零消费；`secondary_hierarchy` 仅有 `noun_dict.rs:69` 字段无消费。
- `members()`/`attrs()`/`element()` 已按 refno.dbnum 自动跨库路由（GPREF 跨库解引用免费获得）。
- **当前故障**：`enumerate_generation_roots_in_subtree` 走 GPSET → GPITEM（叶子）→ 设计元素永不入域 → GPSET 加载显示产出为零。
- `e3d-io::write::Transaction` 已有 `insert/update/rename/delete`、`MemberPosition`、`set_explicit_refno`、int-array hunk —— 写侧原语齐全。

## 1. 目标形态

```
解析：  WORL ──members──▶ GPWL ──members──▶ GPSET ──members──▶ GPITEM ──GPREF──▶ 设计元素(可跨库)
                                                      └─SCOSEL──▶ 规则求值成员(可选)
树显示： GPSET 的 children 返回「解析后成员」（GPITEM 解引用后的真身），GPITEM 行不展示
加载：  GPSET 作用域 = Σ(每个解析成员 ∪ 其主成员子树) → 各成员的生成根(走其自身 owner 链) → 绘制
祖先：  ancestors 保持主链；新增 secondary 维度（元素 ← GPITEM.GPREF ← GPSET）
```

原则：**存储视图（`members`/`children` 原样）与解析视图（`resolved_members`）分层**，不互相污染——E3D 自己也分 `SecondaryListTreeDefinition`（存储）与 `SecondaryTreeDefinition`（展开）。

## 2. 实施阶段

### Phase A — 读侧解析（GPITEM→GPREF 解引用层）

1. `src/data_interface/` 新增 `group_membership.rs`：
   - `resolve_member_element(store, member_refno) -> ResolvedMember`：成员 noun==GPITEM → 读 `GPREF` → `ResolvedMember::Reference(target)`；否则 `::Owned(member)`。
   - `resolved_members(store, refno) -> Vec<RefU64>`：GPITEM 解引用 + 无效槽滤除；GPREF 跨库走 `attrs()` 自动路由（**禁止用 `attrs_in(dbnum)`**，那会拿错库）。
   - `secondary_parents(store, refno) -> Vec<RefU64>`：元素 ← 持有 GPREF==refno 的 GPITEM ← 其 owner（GPSET/GROU…）。v1 运行时扫描+memo；持久化边表见 §4 决策。
2. `TreeStore` trait（direct_tree.rs:414）加 `resolved_members` 默认实现 = `members`；`MdbReadStore` 覆写。
3. `DirectTreeService`：
   - `resolved_children(parent)` —— GPSET 类（成员含 GPITEM）解引用；其余直通 `children`。
   - `resolved_subtree_refnos(root)` —— BFS 中对二级成员节点走 `resolved_members`。
   - `resolved_generation_roots_in_subtree(root, unit_types)` —— 对解析成员各自 `generation_root_of`（成员走自己 owner 链），去重。
4. Web：`/tree/children` 加 `?resolved=1`（缺省保持物理视图，不破坏既有契约）；`/tree/ancestors` 响应加 `secondary_parents` 数组（主链不动）。

### Phase B — 加载显示

1. `on_demand_model` / `generation_root`：GPSET/GPWL 作用域入口走 `resolved_generation_roots_in_subtree`；非分组元素行为不变。
2. 显示域 = TOPF 子树语义：解析成员的元素自身 + 其主成员子树（`subtree_refnos(member)`），与 `getElementsAtOrBelowTOPF` 对齐。
3. `model_records`/`overlay_records_under_node` 不动——记录按主链 ancestors 过滤，解析成员的子树自然命中。

### Phase C — SCOSEL 规则组求值

1. `scosel.rs`：解析字节码 → `ScopeExpr` AST（字段表 100016/100009 头 + 100007 规则体 + opcode 106/120/1602/1701/118/402/301）。
2. `resolved_members(GPSET)` = stored(GPITEM→GPREF) ∪ evaluate(SCOSEL)。求值域按 IDA 证据（`DB_Collection::preProcessClassIds` 语义）定：MDB 全量 or 同库。
3. `100001`（显式成员）flavor：仅声明，不求值。

### Phase D — 写侧（真库创建）

1. `Transaction::insert`：GPWL under WORL；GPSET under GPWL；GPITEM under GPSET（`insert` 自动挂成员链+TYPEX+claim）。
2. `build` 内 `set_explicit_refno(GPREF, target)`；`set(SCOSEL)` 写规则序列。
3. RREF/RATM/RATS 的 GPWL 引用语义另立小项（先只建空 GPWL，RREF 留后续）。
4. 验证：写完用 e3d-io 读回对照 `members`/`GPREF`/`SCOSEL`；有条件再用真 E3D 开库验收。

### Phase E — 增量与验证

- GPITEM create/delete/update(GPREF) → GPSET 成员变化 → 影响显示域而非几何生成——变更分类走 dchc 类表评估。
- 测试：aps250218（GPITEM 显式组）+ aps250204（SCOSEL 规则组）真库 fixture：children 解析、跨库 GPREF、subtree/generation_roots、SCOSEL 求值。
- sigmap：本版只有 `plan/query/--impact/--diff`（无 scaffold/verify-plan/create），用等价命令做计划核对与 diff 审计。

## 3. 关键风险与对策

| 风险 | 对策 |
|---|---|
| `resolved` 语义污染存储视图 | 分层 API，物理视图默认不变 |
| GPREF 不落 pe attmap → 反向查难 | v1 运行时扫描+memo；持久化 `pe_gpmember` 边表为可选项（§4-2） |
| SCOSEL opcode 语义误判 | 等 IDA 取证；先只实现已确认子集（NAME 前缀），未知 opcode 报错不静默 |
| GPSET 成员与成员子树重复计入 | `subtree`/`generation_roots` 全程 `seen` 去重 |
| 写错真库 | `Transaction` 锁 + 读回对照 + 先在副本库上跑 |

## 4. 待用户拍板

1. `/tree/children`：`?resolved=1` 参数（默认物理视图）还是默认切解析视图？——倾向参数化。
2. 二级成员是否落库为新边表（如 `pe_gpmember: GPSET → resolved`）？——影响增量与 /tree/ancestors 反查性能，但多一个 schema 面。
3. 写侧落点：gen-model 侧新增 service/bin 调 `e3d-io::Transaction`，还是 e3d-io 内部再封装 `insert_gpset_member` 便捷 API？
