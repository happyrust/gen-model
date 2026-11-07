# GPWL / GPSET 分组节点：跨仓开发方案（gen-model · plant3d-web · plant-ui）

日期：2026-11-08 · 状态：方案评审中（plannotator）；用户表态前 §7 暂按推荐项推进（可随时改选） · 依据：[`2026-11-08-gpwl-gpset-group-nodes.md`](./2026-11-08-gpwl-gpset-group-nodes.md)（下称「源计划」）+ 三仓代码核查

> 代码基线：gen-model `refactor@5117dda11`、plant3d-web `main@d2d4bf17`、plant-ui `master@d01007a29`。文中行号均按此基线核过。

## 0. 结论先行

1. **源计划对 gen-model 的诊断成立**：树、子树、生成根三条路都只沿主成员链 / OWNER 链走，GPSET → GPITEM 到叶子即止，GPREF 指向的设计元素永远不入域。
2. **源计划 Phase B「`model_records` 不动、解析成员的子树自然命中」只对 plant3d-web 成立，对 plant-ui 不成立**：
   - plant3d-web 用 `/model/generate` 回执的 `generation_roots[]` 逐根取记录 → 后端改好即生效；
   - plant-ui 按 ADR-0030 D1「显示按祖先查」，明确不收 `generation_roots[]`（`model_update_api.rs:884`）。记录按主链祖先过滤，GPSET 不在任何记录的祖先链上 → **点 GPSET 依旧空白**。
   - → 新增决策 **D4**：`/model/records` 对分组节点做服务端展开（推荐），或 plant-ui 自己逐成员查。
3. **两个前端的树状态都按 refno 记**（plant3d-web `checkStateById`，plant-ui `HashMap<RefU64, …>`）。解析视图下同一元素会同时出现在主位置与 GPSET 下，要处理行 key / egui Id 冲突与父级勾选聚合。
4. **GPWL 挂在 WORL 下、与 SITE 同级，而两端树根只取 SITE**（`/tree/roots` 缺省）。入口靠 `/tree/roots?nouns=SITE,GPWL` 或单独分区（`nouns=GPWL`）→ 决策 **D5**。两种都要后端小改：`requested_root_nouns` 原先只放行 SITE / APPLDW，传 GPWL 回 400，G1 放行。

## 1. 现状核查（证据）

### 1.1 gen-model

| 位置 | 现状 | 对本方案的含义 |
|---|---|---|
| `src/data_interface/direct_tree.rs:102` `DirectTreeService` | 冻结会话；`children()` 带整层 `children_memo` | 解析视图的 memo 要与物理视图分开 |
| `direct_tree.rs:238` `subtree_refnos` | DFS `store.members` | 分组另走 `resolved_subtree_refnos` |
| `direct_tree.rs:271` `generation_roots_in_subtree` | 先 `generation_root_of`，再 `generation_roots_in_subtree_on` | 分组入口另起 `resolved_generation_roots_in_subtree` |
| `direct_tree.rs:414` `TreeStore` | 只有 `named_attmap` + `members`；实现者 `DirectStore` / `MdbReadStore` | 解析只靠这两个方法即可实现，不必扩 trait |
| `src/data_interface/generation_root.rs:35` `COARSE_HIERARCHY_NOUNS` | `WORL / WORLD / SITE / ZONE`，无 GPWL / GPSET | 分组既不会被当容器拦下，也不会被展开 |
| `generation_root.rs:611` `enumerate_generation_roots_in_subtree` | 直连按需与增量规划器**共用的唯一遍历**（ADR-056 N7）；`:2056-2068` 有对适配函数源码文本的断言 | **不改它的成员语义**，否则牵动增量规划器与对拍测试 |
| `src/web_service/handlers.rs:1464` `TreeReq` | `identity`(flatten) + `refno` + `nouns`，无 `deny_unknown_fields` | 旧后端会静默忽略新参数 `resolved` |
| `handlers.rs:1572` `tree_children` | 回 `{source, parent, nodes}` | 加 `resolved` / `view` / `unresolved` |
| `handlers.rs:1719` `tree_ancestors` | 回 `{source, refnos}`（自身在前） | 加 `secondary_parents` |
| `handlers.rs:3685` `model_generate`、`:3514` `model_delete_subtree` | 都调 `generation_roots_in_subtree` | 分组按 noun 分流 |
| `src/query_service.rs:817`、`src/data_interface/model_scope_regen.rs:555` | 同上 | 一并分流 |

### 1.2 plant3d-web

| 位置 | 现状 |
|---|---|
| `src/api/genModelV1Api.ts:811-823` | `genModelV1TreeChildren` / `genModelV1TreeAncestors`，只传 `refno` |
| `src/api/genModelE3dTypes.ts:1` `TreeNodeDto` | `refno, name, noun, owner, children_count, dbnum, short_name, stored_name` |
| `genModelV1Api.ts:592` `ModelGenerateResponse.generation_roots` | 「容器节点解出的全部生成根；空 / 缺省时用请求的 refno 自己」→ 逐根取记录 |
| `src/model-source/genModelV1/treeSource.ts:327` | 子树 = BFS `tree/children`，上限 `SUBTREE_MAX_REQUESTS` |
| `src/composables/usePdmsOwnerTree.ts:91,109` | `checkStateById`、`childrenCountById` 按 refno 键 |
| `src/components/model-tree/ModelTreePanel.vue`、`ModelTreeRow.vue` | 树面板（虚拟列表 `vr.key`）与行 |
| `src/model-source/genModelV1/modelRecordSource.ts`、`modelRecords.ts` | generate → records 流程，已处理 422 container / 409 not_generated / 202 pending |
| `src/measurement/dpoint/designPoints.ts:46` | `DESIGN_POINT_HOST_STOP_NOUNS` 含 `'GPWL', 'GPSE'`：`GPSE` 是 `GPSET` 的笔误，但设计元素的 owner 链不会经过 GPSET，实际无行为影响 |

### 1.3 plant-ui

| 位置 | 现状 |
|---|---|
| `crates/plant-ui-app/src/model_update_api.rs:410,429` | `tree_children` / `tree_ancestors`（`/api/v1/tree/children?refno={}`） |
| `model_update_api.rs:884,963` | `generate_model`：回执 `generation_roots[]` **不收**，显示按祖先查（ADR-0030 D1） |
| `model_update_api.rs:1107,1259` | 清理（`DELETE /model/subtree`）、记录（`/model/records`） |
| `crates/plant-ui-app/src/main.rs:891` | `children: HashMap<RefU64, Vec<EleTreeNode>>`（按父存列表） |
| `crates/plant-ui-app/src/sim.rs:386` | `nodes: HashMap<RefU64, EleTreeNode>`（按 refno 存单份） |
| `crates/plant-ui-app/src/read_face/{service,store,parity}.rs` | 读门面与对拍 |
| `crates/plant-ui-data` | `EleTreeNode` 定义 |

## 2. 跨仓接口契约（先冻结，再三仓并行）

原则：**全部增量、缺省不变**——旧前端连新后端行为不变；新前端连旧后端可探测降级。

### 2.1 `GET /api/v1/tree/children?refno=<a/b>&resolved=1`

```jsonc
{
  "source": "direct",
  "parent": "<GPSET refno>",
  "view": "resolved",              // 新增：服务端认了 resolved 才回 "resolved"，否则 "stored"
  "nodes": [
    {
      "refno": "<设计元素 refno>", "noun": "BRAN", "name": "…",
      "owner": "<它自己的主 owner>", "children_count": 12, "dbnum": 250209,
      "via": "<GPITEM refno>",     // 新增：经由哪个 GPITEM 解析；SCOSEL 规则成员为 null
      "secondary": true,           // 新增：owner ≠ parent（二级成员）；主链子节点不带
      "rule": false                // Phase C：SCOSEL 求值成员为 true
    }
  ],
  "unresolved": [                  // 新增：仅 resolved 视图且有坏槽时出现
    { "via": "<GPITEM refno>", "target": "<GPREF 原值或 null>", "reason": "db_not_in_mdb" }
  ]
}
```

- 非分组父节点：`view: "resolved"`，`nodes` 与物理视图逐字相同。
- GPWL：子节点就是 GPSET（存储 = 展开），只是 GPSET 行的 `children_count` 改为解析后成员数。
- `reason` 取值：`gpref_missing` / `db_not_in_mdb` / `target_missing`。

### 2.2 `GET /api/v1/tree/ancestors?refno=<a/b>`

```jsonc
{
  "source": "direct",
  "refnos": ["<自身>", "<owner>", "…"],          // 主链，不变
  "secondary_parents": [                          // 新增：主链上每一级被哪些分组直接收录
    { "member": "<主链上某一级>", "group": "<GPSET refno>", "group_noun": "GPSET", "group_name": "<分组 NAME，无名为 null>", "via": "<GPITEM refno>" }
  ]
}
```

用途：两端「定位到树」仍走主链；属性面板「所属分组」与 GPSET 勾选态推导用 `secondary_parents`。

### 2.3 `POST /api/v1/model/generate`（refno 为 GPWL / GPSET）

- 范围 = Σ(解析成员 ∪ 其主成员子树)；每个成员按**自己的 owner 链**定生成根，全程 `seen` 去重。
- `generation_roots`：解析后的生成根（按成员顺序）。**空组回显式 `[]`**，客户端不得回退为请求的 refno。
- 新增 `scope_kind: "group"`、`unresolved_count`。
- 并发 / 锁 / 202 / 409 语义沿用现有逐根 try-lock。

### 2.4 `POST /api/v1/model/records`（refno 为 GPWL / GPSET，取决于 D4）

- 推荐 D4-a：服务端把分组展开为解析成员，返回 ⋃ records-under-member（按记录身份去重），新增 `expanded_from_group: true`、`scope_refnos`。
- 效果：plant-ui 显示路径零改动；plant3d-web 本来就逐根取，不受影响。

### 2.5 `DELETE /api/v1/model/subtree`（refno 为 GPWL / GPSET，取决于 D6）

- 推荐 D6-a：v1 回 422 `group_scope`，提示到成员主位置清理——清 GPSET 等于清别的 owner 名下的模型，副作用大且不可逆。

### 2.6 兼容性

| 组合 | 行为 |
|---|---|
| 旧前端 × 新后端 | 不带 `resolved` → 物理视图不变；`secondary_parents` 等新字段被忽略；generate(GPSET) 开始有产出（plant3d-web 直接受益；D4-a 下 plant-ui 也直接受益） |
| 新前端 × 旧后端 | `TreeReq` 不拒未知参数，旧后端忽略 `resolved`，响应无 `view` 字段 → 前端按物理视图显示，GPSET 行提示「后端未支持分组解析」 |

## 3. 分阶段实施

依赖关系：`G0 → G1 → (W1 → W2 → W3 ∥ U1 → U2)`；`G1 → G2 → (W4 ∥ U3)`；`G3`、`G4` 与前端解耦；`G5` 收尾。

### 3.1 gen-model

**G0 夹具与红灯用例**
- 夹具：aps250218（GPITEM 显式组）、aps250204（SCOSEL 规则组）、ams `/FWD_BLOCK`（31 个 GPITEM、跨库 GPREF）。
- 先写失败用例：GPSET 解析 children、跨库 GPREF、generate(GPSET) 根非空、records(GPSET) 非空。
- 验收：用例在当前基线全红，且失败原因与源计划诊断一致。

**G1（源计划 Phase A）读侧解析**
- 新增 `src/data_interface/group_membership.rs`：
  - `ResolvedMember::{Owned, Reference { via, target }, Rule}`、`resolve_member_element`、`resolved_members_on(store: &dyn TreeStore, refno)`；GPREF 读取走 `named_attmap()` 的自动跨库路由（禁止 `attrs_in(dbnum)`）。
  - `GroupIndex`：从各库 WORL 一次扫出 `WORL → GPWL → GPSET → GPITEM`，建反向表 `target → [(group, via)]`；挂在 `DirectTreeService` 上，与冻结会话同寿命（换会话即换服务，天然失效）。扫描面只有分组层级，规模远小于全库。
- `TreeStore` **建议不扩方法**，用自由函数实现。这是与源计划的差异，理由：`generation_root.rs:2056-2068` 对 `generation_roots_in_subtree_on` 有源码文本断言、两套 store 有对拍；不动 trait 就不碰它们。
- `DirectTreeService` 新增：`resolved_children(parent)`（memo 与 `children_memo` 分开）、`resolved_subtree_refnos(root)`、`resolved_generation_roots_in_subtree(root, unit_types)`、`secondary_parents(refno)`。
- `enumerate_generation_roots_in_subtree` 不动。分组入口：对每个解析成员先 `generation_root_of`（有祖先根就取祖先根），否则 `generation_roots_in_subtree(member)`，`seen` 去重。
- Web：`TreeReq` 加 `resolved`；`tree_children` 分流并回 `view` / `unresolved`；`tree_ancestors` 回 `secondary_parents`。
- `/tree/roots` 的 `nouns=` 放行 `GPWL`：`roots_of_nouns` 本身按 noun 过滤即可，但 handler 的 `requested_root_nouns` 原先只认 SITE / APPLDW（传 GPWL 回 400），G1 放行并补文档与用例。GPWL 只作树入口，不是生成根。
- 验收：G0 前两条转绿；物理视图快照不变；现有树 / 生成根用例全绿。

**G2（源计划 Phase B）加载显示**
- 入口按 noun 分流到 `resolved_generation_roots_in_subtree`：`model_generate`（`handlers.rs:3685`）、`model_delete_subtree`（`:3514`，按 D6）、`query_service.rs:817`、`model_scope_regen.rs:555`。
- `/model/records` 分组展开（D4-a）。
- 空组 / 坏槽回执按 §2.3。
- 分组节点在各种读写形态下都走解析根，不得落入 `ApiError::container`（422，原本只给 WORL / SITE / ZONE「不能做生成根」用）。
- 验收：G0 后两条转绿；非分组节点 generate / records 回执逐字不变。

**G3（源计划 Phase C）SCOSEL 规则组**
- 等 IDA 回填 opcode 语义；先做 NAME 前缀子集，未知 opcode 报错不静默；开关 `group_scosel` 默认关；树里规则成员为 `rule: true, via: null`。

**G4（源计划 Phase D）写侧**
- 落点按 D3；只在副本库演练；写后用 e3d-io 读回对照 `members / GPREF / SCOSEL`；有条件再用真 E3D 开库验收。前端 v1 不做「加入分组」入口。

**G5（源计划 Phase E）增量**
- GPITEM 增删 / GPREF 改动归为「显示域变化」，不触发几何重算。
- 现有 `/api/v1/ws` 只推任务事件，没有树变更事件 → v1 前端在 generate 回执或会话刷新后自行失效分组缓存；是否在 ws 上加 `tree_changed` 事件到 G5 再定。

### 3.2 plant3d-web

**W1 API 与类型（依赖 G1）**
- `genModelE3dTypes.ts`：`TreeNodeDto` 加 `via?`、`secondary?`、`rule?`。
- `genModelV1Api.ts`：`TreeChildrenResponse` 加 `view?`、`unresolved?`；`TreeAncestorsResponse` 加 `secondary_parents?`；`genModelV1TreeChildren(refno, { resolved })`；`ModelGenerateResponse` 加 `scope_kind?`、`unresolved_count?`。
- 能力探测：响应无 `view` 或为 `"stored"` 即按物理视图（§2.6）。
- 单测：`genModelV1Api.test.ts` 查询串含 `resolved=1`；响应无 `view` 时降级。

**W2 树数据源与状态（依赖 W1）**
- `treeSource.ts`：children 统一带 `resolved=1`（非分组节点后端直通），缓存 key 带视图；分组节点的「子树」不再走 BFS（成员可能是整片 ZONE，会撞 `SUBTREE_MAX_REQUESTS`），可见性 / 勾选改用 generate 回执的 `generation_roots`。
- `usePdmsOwnerTree.ts`：`checkStateById` 继续按 refno 共享（同一元素两处同一勾选态，符合「同一模型对象」）；父级聚合改为按「当前视图中父 → 子列表」计算，不再沿 `node.owner` 上推，GPSET 才拿得到成员的勾选变化；勾选 GPSET = 勾选全部解析成员。
- `ModelTreePanel.vue`：虚拟列表行 key 改为路径 key（`父路径>refno`），避免主位置与 GPSET 同时展开时 key 冲突。
- 单测：`treeSource.test.ts`（解析 children、降级）、`usePdmsOwnerTree` 双位置勾选聚合。

**W3 树显示（依赖 W2）**
- 根：按 D5 请求 `/tree/roots?nouns=SITE,GPWL`，或单独「分组」分区。
- `ModelTreeRow.vue`：GPWL / GPSET 图标；`secondary` 行加「引用」角标，tooltip 显示主位置路径；右键「定位到原位置」（沿 ancestors 主链展开）；`unresolved` 非空时 GPSET 行显示「N 个成员不可解析」。
- 属性面板加「所属分组」（`secondary_parents`）。
- 顺手把 `designPoints.ts:46` 的 `'GPSE'` 改为 `'GPSET'`（无行为影响，低优先级）。

**W4 加载显示（依赖 G2）**
- `modelRecordSource.ts` / `modelRecords.ts`：分组节点回执 `generation_roots: []` → 提示「分组为空或成员不可解析」，**不回退**到请求 refno；非空 → 走现有逐根取记录路径。
- 「清理模型 / 重新生成」菜单对分组节点按 D6 隐藏或改文案。
- 单测 + Playwright e2e：展开 GPWL → GPSET → 成员；勾选 GPSET 后成员模型出现；跨库成员可见；空组提示。

### 3.3 plant-ui

**U1 数据层（依赖 G1）**
- `crates/plant-ui-data`：`EleTreeNode` 加 `via: Option<RefU64>`、`secondary: bool`、`rule: bool`（全部 `#[serde(default)]`，兼容旧后端）。
- `model_update_api.rs`：`tree_children` 加 `resolved` 参数（`?refno={}&resolved=1`），`TreeNodesReply` 解析 `view` / `unresolved`；`tree_ancestors` 解析 `secondary_parents`。
- `read_face/service.rs`、`store.rs` 透传；`parity.rs` 加分组对拍用例。

**U2 树 UI（依赖 U1）**
- `main.rs:891` 按父存列表，同一元素挂两个父节点天然可容纳；`sim.rs:386` 按 refno 存单份 → 只存主上下文，GPSET 上下文不得覆盖（`owner` 不同）。
- egui 行 Id 带父上下文（`push_id((parent, refno))`）；展开态若按 refno 记会两处联动，v1 接受并写明，或改路径 key。
- GPWL / GPSET 图标、引用角标、「定位到原位置」；根入口按 D5。

**U3 加载显示（依赖 G2 + D4）**
- D4-a（服务端展开）：`data.rs` 显示路径不改，records 直接传 GPSET；只补分组用例，并保持 `main.rs` / `data.rs` 现有「显示路径不打 generate」断言成立。
- D4-b（客户端展开）：`data.rs` 对分组先取解析成员再逐个按祖先查，并修订 ADR-0030 D1。
- 右键清理 / 重新生成按 D6。

## 4. 里程碑与联调顺序

| 里程碑 | 内容 | 可见效果 | 前置 |
|---|---|---|---|
| M1 | G0 + G1 | `/tree/children?resolved=1` 返回 GPSET 的真身成员；旧客户端无感 | — |
| M2 | W1–W3、U1–U2 | 两端树里能展开 GPWL → GPSET → 成员，有引用角标和「定位到原位置」 | M1 + D1 / D5 |
| M3 | G2 + W4 + U3 | 两端勾选 / 加载 GPSET 时显示成员模型 | M1 + D4 / D6 |
| M4 | G3 | SCOSEL 规则组成员出现在树和显示里 | IDA 证据 |
| M5 | G4 | 真库创建 GPWL / GPSET / GPITEM（副本库验收） | D3 |
| M6 | G5 + 三仓 e2e | 增量下分组变化即时反映；aps250218 / aps250204 / ams 验收 | M3 |

## 5. 测试与验收矩阵

| 场景 | gen-model | plant3d-web | plant-ui |
|---|---|---|---|
| GPSET 解析 children（显式组） | 夹具 aps250218 | treeSource 单测 + e2e | read_face 对拍 |
| 跨库 GPREF | ams `/FWD_BLOCK` | e2e 跨库成员可见 | 对拍 |
| 坏槽（库不在 MDB / 目标缺失） | `unresolved` 原因码 | 行提示 | 行提示 |
| 同一元素双位置 | — | 行 key、勾选聚合单测 | egui Id、`sim.rs` 不覆盖 |
| generate(GPSET) 根 / 空组 `[]` | 用例 | modelRecords 单测 | — |
| records(GPSET) 展开（D4-a） | 用例 | 不受影响 | 显示用例 |
| 旧前端 × 新后端 / 新前端 × 旧后端 | 物理视图快照不变 | 降级用例 | serde default 用例 |
| SCOSEL（M4） | aps250204 规则组 | `rule` 角标 | `rule` 角标 |

## 6. 风险与对策（在源计划 §3 之上补充跨仓项）

| 风险 | 对策 |
|---|---|
| 契约三仓各写各的，字段漂移 | §2 先冻结；gen-model 加响应快照测试，前端类型以快照为准 |
| 分组成员是整片 ZONE，前端 BFS 撞上限或卡顿 | 分组可见性走 generate 回执的根，不 BFS |
| 同一 refno 双位置导致 UI key / Id 冲突、勾选错乱 | 路径 key、按视图聚合、`sim.rs` 只存主上下文 |
| plant-ui 按祖先查显示，GPSET 空白 | D4-a 服务端展开 |
| 清理 GPSET 误删别处模型 | D6-a v1 拒绝 |
| `GroupIndex` 首次构建延迟 | 只扫分组层级；随冻结会话 memo；性能不够再上 D2 的持久化边表 |

## 7. 待拍板（源计划 §4 三项 + 本方案新增三项）

> 当前：用户表态前按推荐项推进（D1 a、D2 a、D3 a、D4 a、D5 b、D6 a）。改选 D1 / D2 影响 G1，需在 M1 开工前定；D4 / D6 只影响 G2、W4、U3；D5 只影响 W3、U2；D3 只影响 G4。

| # | 问题 | 选项 | 推荐与理由 |
|---|---|---|---|
| D1 | `/tree/children` 解析视图怎么开 | a. `?resolved=1`，缺省物理视图；b. 缺省切解析视图 | **a**：旧客户端零影响；新前端统一带参即可 |
| D2 | 二级成员是否落库（`pe_gpmember` 边表） | a. v1 不落库，运行时 `GroupIndex` + memo；b. 落库 | **a**：扫描面只有分组层级，代价小；落库多一个 schema 面和增量同步点，等有性能数据再定 |
| D3 | 写侧落点 | a. e3d-io 封装 `insert_gpset_member` 等便捷 API，gen-model 新 bin 调它；b. gen-model 直接拼 `Transaction` | **a**：成员链、TYPEX、claim 这些写库细节留在 e3d-io 一处，gen-model 只做编排 |
| D4（新） | GPSET 的显示记录由谁展开 | a. `/model/records` 服务端展开；b. plant-ui 逐成员查 | **a**：plant-ui 显示路径与 ADR-0030 D1 都不动，plant3d-web 不受影响，一处实现两端受益 |
| D5（新） | GPWL 在树里的入口 | a. 与 SITE 混排（`nouns=SITE,GPWL`）；b. 单独「分组」分区 / 页签（`nouns=GPWL`）。两项都依赖 G1 放行 `nouns=GPWL` | **b**：不扰动按 SITE 工作的既有流程（rvm-check 的 `runAllSites()`、根枚举等） |
| D6（新） | 分组节点上的「清理模型」 | a. v1 拒绝（422 `group_scope`），菜单隐藏；b. 允许，清全部成员模型 | **a**：b 会清掉别的 owner 名下的模型，不可逆 |
