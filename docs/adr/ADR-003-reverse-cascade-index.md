# ADR-003：反向目录/规格级联——自建正向引用反转索引（暂不直读 E3D back-ref）

状态：已接受
日期：2026-07-23
关联：ADR-002；`src/data_interface/model_refresh.rs`（cascade TODO / `cascade_refnos`）；`vendor/aios-parse-pdms/src/parse.rs`

## 背景

workstream B 要补「改共享目录/规格元件 → 重生成所有引用它的设计实例」这一缺口。

- core.dll 的权威机制是**存储型 back-ref 逆指针**：`ATT_BREF`/`ATT_SPBREF`/`ATT_SCBREF`/`ATT_TABREF`/`ATT_DBREF`… 由 `DB_ElementChangesPlugger::PostSetRefListAttribute` 维护，另有 `DB_Clone::getRelatedElements`。
- 实测：gen-model 解析器 `parse_raw_ele_data_with_info` 按 **schema 固定偏移**解码属性；back-ref 不在 schema(`all_attr_info.json`)，且 PDMS 里 back-ref 是**独立引用表 / 系统维护结构**，不是元素隐含块里的固定偏移属性 → **当前离线不可得**。
- 正向引用属性（`SPRE`/`CATR`/`DESP`/`PARA`/`PRTREF`/`HREF`/`TREF`… att_type=ELEMENT）可正常解码。

## 决策

自建「正向引用反转」持久索引：`referenced_refno → [referrer_refnos]`，在落库时同步维护。增量时，被改动的目录/规格元素（命中 `DependencyCascade`、已进 `cascade_refnos`）→ 查反向索引得引用实例 → 并入 `changed_seed_refnos` → 经 `resolve_significant_owner` 归一为重生成根。**显式处理**间接引用：spec 表链（`SPRE→SCOM 组件→PRTREF→TABITE`）与克隆副本。

「直读 E3D 存储 back-ref」列为**将来优化 ADR**（需更全字典的 hash/offset + 解析器扩展，且需确认 back-ref 在元素页可解码）。

## 结果 / 约束

- 满足 ADR-002 的行为对齐目标；不依赖 E3D 许可或更全字典。
- 代价：多维护一份反向索引；间接引用（spec 表/克隆）需自行覆盖，是本工作流的主要风险点。
