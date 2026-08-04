# ADR-002：以 core.dll 为准的增量模型生成——范围与 DCHC 权威口径

状态：已接受
日期：2026-07-23
关联：ADR-001（DBNUM 水位）；`src/data_interface/model_impact.rs`、`model_refresh.rs`

## 背景

目标是让 gen-model 的「增量模型生成 / 关联模型判定」**以 AVEVA E3D `core.dll` 的实现为准**（IDA 会话 `core31-retrace`，`D:\AVEVA\Everything3D3.1\core.dll`，32 位）。

实测 core.dll 得到三条地基事实：

1. **变更模型 = `DB_UserChanges`**，由 `DB_DB::elementsChangedBetween(from, to, …, DB_UserChanges&)` 按会话区间算出。分类为 `elementCreated / elementDeleted / attributeModified / elementIncluded / elementReordered`（比 gen-model 现有的 `Add / Modified / Deleted / None` 更细）。
2. **关联链真实存在**：`PSPREF`/`FSPREF`(`ATT_PSPREF`/`ATT_FSPREF`，`DB_Attribute`) → `SPCO`(`NOUN_SPCO`) → `GMSET` → `PARA[]`；刷新链 `FZXUPD`(0x5294555) → `FUPALL`(0x52f1f82) → `GLUPDA`(0x5aa90d0)。
3. **per-(noun,attr) 的 DCHC 设计变化码不在 core.dll 二进制内**（只有 `UI_REDRAW` 串；`INTUBE/DESPARAM` 连静态串都没有）。码编在 **E3D 字典**里；core.dll 只做「读 `DB_Noun`/`DB_Attribute` schema 标志位来判」的**逻辑**。仅 forced 码可静态得（`REDRAW=4`、`INTUBE=1`）。

## 决策

1. **范围**：本轮聚焦 **A（影响判定以 core.dll 为准）+ B（补目录/规格反向级联）**；C（生成根颗粒）、D（TransformOnly 变换）、E（触发/刷新）**仅在发现与 core.dll 分歧时才对齐**，不预先大改。
2. **DCHC 权威口径 = 逻辑复刻 + 行为对齐**（而非逐码字节一致）：
   - 复刻 core.dll 中**确实可得**的判定逻辑：`DB_UserChanges` 变更分类 + 读 `DB_Noun`/`DB_Attribute` schema 标志的判定结构；
   - per-attr 影响**数据**用运行库 `att_meta`(702) / E3D 字典交叉校验来补；
   - 字典未覆盖的属性**保留「宁多勿漏」保守兜底**（`model_impact.rs` 现有清单从「唯一真相」降级为「兜底」）；
   - **验收标准**：在一批测试语料上，与 core.dll 的「重生成 / 重绘集合」**行为一致**。

## 结果 / 约束

- 不依赖活 E3D 许可即可推进主线；需要一套「设计变更 → 期望重生成集合」的测试语料作行为对齐验收。
- 若日后要逼近逐码一致，可另起「活字典 DCHC 权威表导出」为增量 ADR，不影响本决策成立。
