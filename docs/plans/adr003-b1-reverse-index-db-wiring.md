# B1 反向引用索引「DB 接线」实施规格（ADR-003 workflow B 收尾）

> 状态：待实施（需本地 Surreal/E3D 真机）。前置的**纯逻辑两端已落地并单测**（见下）。
> 关联：`docs/adr/ADR-003-reverse-cascade-index.md`、`docs/plans/core-dll-aligned-incremental-gen.md`（§4 工作流 B）、`docs/specs/manual-model-update.md`。
>
> 记号约定：下文 SurrealQL 里的 `pe:{X}` 是占位写法，实际请用现有 `RefnoEnum::to_pe_key()`
> 生成的记录链接（SurrealDB 对含 `/` 的 record-id 会自动尖括号转义，风格同 `inst_relate`）。

## 0. 目标

让「改共享目录/规格元件，或移动被分支 HREF/TREF 连接的 NOZZ/邻居 -> 重生成所有引用它的设计实例（含其 TUBI）」在**生产**闭合。当前缺口 A（连接级联）/ 缺口 B（共享目录级联）只差这一段 DB 接线。

## 1. 已就位的纯逻辑 seam（无需改动，只需喂数据）

| 组件 | 位置 | 作用 |
|---|---|---|
| `OwnershipSnapshot.ref_reversal: HashMap<RefnoEnum, Vec<RefnoEnum>>` | `src/data_interface/manual_update.rs:283` | 消费侧输入：`referenced -> [referrers]`。**空 = 行为不变** |
| `build_unit_rollup` 反向级联段 | `manual_update.rs:690` | 命中 `ref_reversal` 的 model-affecting 变更 -> 把每个 referrer 归一到其交付单元一起重生成（`cascaded`） |
| `extract_reverse_ref_edges(op) -> ReverseRefEdges` | `manual_update.rs:481` | 生产侧：从一个变更元素抽 `referrer` + `referenced[]`（post 态），Deleted 置 `purge` |
| `reference_cascade_targets(att, referrer)` | `manual_update.rs:446` | 只反转 `classify_attribute_effect == DependencyCascade` 的引用属性；去重、去自引用、含 ref-list |

本规格只需实现两件事：**（B1-emit）落库时把 `extract_reverse_ref_edges` 的结果写进 Surreal**；**（B1-query）`resolve_unit_rollup` 查回来填 `ref_reversal`**。填上后 B2 立即生效，`gap_a_*`/`gap_b_*` 描述的缺口在生产闭合，`reverse_cascade_*` 三条测试描述的即是目标行为。

## 2. Surreal 表设计（RELATE 图边表）

`ref_rev` 是**真正的图边表**，`in = referrer` / `out = referenced`，与项目里既有的 `pe_owner` 完全同构。边 id 用 `ref_rev:[in, out]` 复合键，天然幂等、天然去重。

```surql
-- 无需 DEFINE：INSERT RELATION 会自动建表。存量库跑一次 rebuild_ref_rev 完成迁移。
INSERT RELATION INTO ref_rev [
  { id: ref_rev:[pe:{referrer}, pe:{target}], in: pe:{referrer}, out: pe:{target} }
];
```

**为什么必须是图边而不是普通表**：`in`/`out` 让 SurrealDB 维护每个顶点的邻接，`X->ref_rev`（出边）和 `X<-ref_rev`（入边）都是邻接局部访问，代价只跟命中的边数走。用普通表 + `WHERE referrer = X` 则依赖二级索引，而那些索引原先只在 `rebuild_reverse_index()` 收尾时才创建——没跑过全量重建的库上，**每个变更元素的清边都是一次全表扫描**。图边化把这个坑连根拔掉，所以不再需要 `ref_rev_by_referenced` / `ref_rev_by_referrer` / `ref_rev_unique`（重建时会 `REMOVE INDEX IF EXISTS` 清掉存量库的遗留索引）。

## 3. B1-emit：落库时维护（`increment_pipeline.rs`）

挂点：`apply_one`，在 `persist_latest_main_data` 成功之后。**语义 = 按 referrer「先删后建」**（幂等、天然处理被移除的引用）：

对本窗口每个 `EleOperationData op`：
```text
let edges = extract_reverse_ref_edges(op);   // 已实现
DELETE pe:{edges.referrer}->ref_rev;         // 沿自身邻接清出边，无扫描、无索引依赖
if !edges.purge && !edges.referenced.is_empty() {
    INSERT RELATION INTO ref_rev [ { id: ref_rev:[in,out], in, out }, ... ];
}
// Deleted：只有上面的 DELETE，不再建边
```

**关键约束（不能拖垮水位）**，二选一：
- **v1（安全先行，推荐先上）**：反向索引维护做成**独立、非致命**步骤——写失败只 `warnings.push(...)` + `eprintln!`，**绝不** `?` 冒泡、**绝不**并入主数据事务、**绝不**阻止 `advance_applied`（`increment_pipeline.rs:214`）。代价：偶发写失败 -> 该批引用边缺失 -> 该次可能漏级联（可接受，靠后续重建/下次触及修正）。
- **target（验证充分后）**：把这些 `DELETE/UPSERT` 语句**并入** `persist_latest_main_data` 的分块事务（与主数据原子）。最可靠、绝不漂移；风险是 SQL bug 会连累整批 -> 卡水位，故必须先在真机把 SQL 跑穿再切。

> 冷启动/全量导入：需一次性对现有 `pe` 全量建索引（遍历 `pe` 读属性 -> `reference_cascade_targets` -> 批量 INSERT）。可作为 `ensure_reverse_index_built(project)` 单独入口，非水位路径。

## 4. B1-query：消费时查回（`manual_update.rs:resolve_unit_rollup`）

现状 `resolve_unit_rollup`（`manual_update.rs:750`）在 `let snap = OwnershipSnapshot { ... ref_reversal: HashMap::new() }`（`:772`）处传空。改为**只按本批变更 seeds 反查**（不拉全表）：

```text
let seeds = details.iter().map(|d| d.refno).collect();   // 本批变更 refnos
// 沿 seeds 自身的入边邻接反查（非 WHERE 过滤）：
SELECT in, out FROM array::flatten([pe:{seed}, ...]<-ref_rev);
// 组装：for row in rows { ref_reversal.entry(row.out).or_default().push(row.in); }
```

> `array::flatten` 不能省：多记录遍历默认按**源记录**分组返回 `{ in: [...], out: [...] }` 嵌套数组，flatten 之后才是扁平的 `{in, out}` 配对行。配对结构必须保留——`build_unit_rollup` 靠它决定「命中交付单元就止步」。
把结果填入 `OwnershipSnapshot.ref_reversal` 即可；`build_unit_rollup` 无需再改。

> **关键**：referrer 需能被 `resolve_change_unit` 在 owner 图里归一 -> `resolve_unit_rollup` 现有喂给 `load_base_graph` 的 `seeds` 必须**并入这些 referrer**，否则 referrer 的 owner 链不在 `base` 里、级联会落空。顺序：先查 `ref_rev` 得 referrers -> 把 referrers 加进 `load_base_graph` 的 seeds -> 再 `build_unit_rollup`。

## 5. 间接引用（ADR-003 B3）

emit 侧仍只写**直接**正向引用边（`reference_cascade_targets` 抽到的）；间接引用在 **query 侧**用传递闭包解决：

- `load_ref_reversal_closure(seeds)` 逐跳反查——每轮拿上一轮新发现的 referrer 当下一轮的 `referenced` 再查，`visited` 去重防环，上限 `MAX_REVERSE_CASCADE_HOPS=8` / `MAX_REVERSE_CASCADE_REFERRERS=50000`。`resolve_unit_rollup` 用它填 `ref_reversal`。
- 这一步是必需的：`build_unit_rollup` 的级联段是逐跳走 `ref_reversal` 的，而目录中间体（SPCO/SCOM）本身不是本窗口的变更元素。**只查一跳的话它在 map 里没有 key，BFS 第二跳必然 miss**，规格表链（`TABITE->SPCO->BRAN`）会静默落进「无法解析最小交付单元」。
- 回归测试：`reverse_cascade_closure_loads_every_hop`（断言第二跳被加载）、`reverse_cascade_closure_terminates_on_cycles`（互引不自旋、每个元素最多查一次）。注意 `reverse_cascade_is_transitive_through_catalog_intermediates` 只覆盖纯函数、手工注入完整 map，**不能**替代上面两条。

克隆副本（`DB_Clone::getRelatedElements`）仍未覆盖，列为后续。

## 6. 真机验证步骤

1. **存量库先跑一次 `cargo run --bin rebuild_ref_rev`**：这是从旧的 `{referrer, referenced}` 普通表迁到 `{in, out}` 图边的迁移入口，它会重建全部边并清掉遗留的三个二级索引。不跑的话旧行还在，但新查询（`<-ref_rev`）看不到它们。
2. 降低某 `dbnum_watermark`，`init_watcher` 增量一次；断言 `ref_rev` 有边（挑一条已知 `HSTU->CATA` / `HREF->NOZZ` 的分支核对）。
3. 手动更新预览 + 执行一个「改共享管件规格 SPCO」的变更；断言引用它的多条 BRAN 都进了重生成单元（`DeliveryUnitSummary.cascaded > 0`）。
4. 移动一个被分支 HREF 连接的 NOZZ；断言被连接 BRAN 也重生成（其头段 TUBI 更新）。
5. 纯逻辑已由 `reverse_cascade_shared_spec_regenerates_referring_branches` / `reverse_cascade_nozzle_move_regenerates_connected_branch` / `gap_a_*` / `gap_b_*` 钉死；真机只验证「索引确实被填 + 端到端几何刷新」。

## 7. 风险与权衡

- **水位安全**：v1 非致命隔离是硬约束（见 §3）。切 target 事务内前必须真机验证 SQL。
- **索引一致性**：先删后建保证「移除的引用」被清；异常中断可能残留 -> 提供 `rebuild_reverse_index` 全量重建兜底。
- **存量迁移**：图边化改了行结构（`referrer/referenced` -> `in/out`）。旧行不会被新查询读到，也不会被新的 `DELETE X->ref_rev` 清掉，只能靠 `rebuild_ref_rev` 的 `DELETE ref_rev` + 全量重灌完成切换（见 §6.1）。
- **版本约束**：客户端与服务端均为 SurrealDB 2.1.4，递归路径语法（`.{..}`，2.2+）不可用；多跳只能逐跳查（Rust 侧 BFS，遇空 frontier 提前收敛，典型 2–3 跳）或手工展开层级（同 `helper.rs` 的 `pe_owner` 写法）。
- **成本**：多一份边表 + 落库多几条幂等语句；查询沿邻接反查、量小。
- **att_meta 交叉**：`model_impact.rs` 近期新增 `classify_attribute_effect_with_meta`（att_type 兜底）；若将来 emit 侧想用「att_type=ELEMENT」更精确地筛引用属性，可切到该函数，但 `reference_cascade_targets` 现按 `DependencyCascade` 分类已单一事实源、够用。
