# 0006 — ref_rev：gen-model 自建的反向引用索引（ADR-003）

- **日期**：2026-07-26
- **背景**：讲解「改共享目录/规格元件后，怎么找到并重生成所有引用它的设计实例」。
- **课件**：`teach/lessons/0002-ref-rev-reverse-reference-index.html`（流程图为主线）

## 关键洞见

1. **问题不对称**：正向（实例 → 目录）写在元素属性里，读一次就有；反向（目录 → 谁在用我）
   PDMS 数据里没有出口，必须自建索引。缺了它，改共享目录件只会重建目录侧容器，
   设计侧几何全部陈旧 —— 即 `model_refresh.rs` 里那段 TODO 描述的缺口。
2. **存正向边、反向查**：`ref_rev` 是 SurrealDB 图边表，`in` = 引用方、`out` = 被引用方，
   边 id `ref_rev:[in, out]` 是复合主键（重复写幂等）。这么存是为了维护便宜：
   `DELETE pe:X->ref_rev` 沿元素自身邻接清出边，不扫表、不需要二级索引。
3. **入索引的门槛只有一个**：属性被 `classify_attribute_effect_with_meta` 判为 `DependencyCascade`
   （`CATR`/`SPRE`/`PRTREF`/`SPCO`/`DESP`/…，加 A2 兜底：schema `att_type == ELEMENT` 的引用属性）。
   `OWNER` 故意排除（层级由 owner 图负责），否则这张表会退化成大杂烩、级联范围失控。
4. **维护 = 先整体清、再整体重写**：`maintain_reverse_index` 对每个变更元素先 DELETE 全部出边，
   再按后态属性图 `INSERT RELATION`。改过引用不会留幽灵边；Deleted 只清不插。
5. **查询必须是传递闭包**：`load_ref_reversal_closure` 逐跳 BFS
   （`MAX_REVERSE_CASCADE_HOPS = 8`、`MAX_REVERSE_CASCADE_REFERRERS = 50_000`）。
   一跳会断在 SPCO/SCOM 这类**没有交付单元的目录中间体**上 —— 这就是 ADR-003 的 B3。
6. **消费端的止步规则**：`build_unit_rollup` 里，referrer 命中交付单元 → 记 `cascaded` 并**止步**；
   命中普通根或没有根 → 继续沿它的引用者上溯。`visited` 去重防环。
   `ref_reversal` 为空时整段是 no-op，所以「先建索引、后接消费」可以安全分期上线。
7. **写/读失败策略不对称**：写失败只记 warning（下次触及整体重写自愈，绝不阻塞水位）；
   读失败会造成静默漏刷，所以降级到 `resolve_unit_rollup_without_reverse_index`
   并为每个受影响元素登记 `CascadeExpand` 持久待办，级联不丢只推迟。

## 对照 core.dll

| 能力 | core.dll | gen-model |
|---|---|---|
| 反向关联 | 数据库自带存储型 back-ref：`BREF`/`SPBREF`/`SCBREF`/`TABREF` | 自建 `ref_rev` 边表 |
| 维护挂点 | `DB_ElementChangesPlugger::PostSetRefListAttribute` | `maintain_reverse_index`（落库写属性同一处） |
| 关联展开 | `DB_Clone::getRelatedElements` | `load_ref_reversal_closure` + `build_unit_rollup` |
| 刷新粒度 | 全量视图 flush（`FZXUPD→FUPALL→GLUPDA`） | 必须自算最小重生成集，粒度不照抄 |

> 基线来源：`docs/plans/core-dll-aligned-incremental-gen.md` §2（会话 core31-retrace 实测）。

## 未决

- 直读 E3D 自带 back-ref 属性、省掉自建索引：ADR 里列为将来优化，需要更全的字典与解析器扩展。
- 目录入口属性按 noun 选择（`NOZZ`/`ELCONN`/`EQUCOM` 用 `CATR`，`TUBI` 按 `TYPE` 选 `HSTU`/`LSTU`）
  是否已被 `DEPENDENCY_CASCADE_ATTR_NAMES` 完整覆盖，待逐项核对（ADR-003 B3 风险点）。
