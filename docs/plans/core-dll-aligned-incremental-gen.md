# 开发方案：以 core.dll 为准的增量模型生成对齐

> 本方案由 `/grill-with-docs` 会话产出。决策见 `docs/adr/ADR-002`、`ADR-003`；术语见 `CONTEXT.md`。本文件只写「做什么、按什么顺序、怎么验收」，不含逐行实现。

## 1. 目标与验收
- 目标：让 gen-model 的「增量模型生成 + 关联模型判定」以 AVEVA E3D `core.dll` 实现为准。范围（ADR-002）：**A 影响判定 + B 反向级联**；C 生成根颗粒 / D TransformOnly / E 触发刷新 **仅在行为对齐测试暴露分歧时才对齐**。
- 验收（行为对齐，非逐码字节一致）：给定一组设计变更，gen-model 产出的「重生成根集合」与 core.dll 的「重绘/重建集合」一致。

## 2. core.dll 事实基线（实测会话 core31-retrace）
- **变更集**＝`DB_UserChanges`（`DB_DB::elementsChangedBetween`），五类 created/deleted/attributeModified/reordered/included。
- **判定**＝读 `DB_Noun` schema 标志（`primitive` #659518 / `geomset` #859903 / `graphicsBehaviour` this+0xB4，`internalGetField`），vtable 分派；per-attr DCHC 码在 E3D 字典、不在二进制（仅 forced `REDRAW=4`/`INTUBE=1`）。
- **正向关联**＝`SPRE`/`CATR`→`SCOM`/`SPCO`→`GMSET`→`DESP`/`PARA[]` 展开图元（段重建时现场展开）。
- **反向关联**＝存储型 back-ref（`BREF`/`SPBREF`/`SCBREF`/`TABREF`…）+ `DB_ElementChangesPlugger::PostSetRefListAttribute` + `DB_Clone::getRelatedElements`。
- **刷新**＝`FZXUPD`(0x5294555)→`FUPALL`(0x52f1f82)→`GLUPDA` 全量视图 flush（粗粒度）+ `RIO_OutputListener::SendUpdate`；几何按元素段 `FZ3SGL`(sub_5297141) 重建、视图句柄 `HPUTI1` 存元素上。
- **结论**：core.dll 是在线 viewer（按元素重建段 + 全量刷新显示）；离线 gen-model 必须自算最小重生成集，故刷新粒度不照抄 → C/D/E 暂不动。

### 2.1 模型更新逻辑（事件驱动 + 按元素重生成，实测闭环；详见 `teach/learning-records/0002`）
core.dll 用**两套并行系统**：数据变更传播（观察者）与图形重生成（按元素）。
- **变更传播** = 观察者 `DB_ElementChangesPlugger` + 分类型 handler：`PostSetAttribute`（标量）、`PostSetRefListAttribute`（引用列表 → 维护 back-ref）、`PostCreate/PostDeleteElement`、`PreSetAttribute`（合法性）；订阅者 `ADM_SCPlugsFor*::PostSetAttribute`。
- **按属性精准失效引用表缓存（闭环）**：`PostSetAttribute` → `DB_RefTabDatabasesPostSetAttr::PostSetAttribute`(0x59fbd00) → `DB_RefTableDatabases::invalidate(attr)`(0x59fbfe0) 在按属性建的 RB-tree 里定位该属性引用表项、置脏 → 下次查询重解析。
- **GUI 重绘派发（⚠️ 修正，详见 `teach/learning-records/0005`）** = `VFCRGD`(sub_52DB664, `fmgadget/VFCRGD`) 是 **Forms&Menus 的 GUI 控件重绘派发器**，按 `HQTYPE`(控件类型) 分派各 GUI 控件（按钮/文本/滑块/列表/…），**非** 3D noun 几何派发。其 **case 3/16 = `FZBG3D`(sub_5296A17, `fm3dcanv`) = 3D 画布控件** 是 GUI→3D 桥 → `FZ3SGL`(sub_5297141) 建 GL 段。catalogue 的 `SPRE→SCOM→GMSET→PARA` 现场展开发生在 FZBG3D 之下的场景填充路径（真正的 3D 逐-noun 几何派发处，由 `DB_Noun::graphicsBehaviour` 驱动，**待定位**）。
- **端到端**：写属性 → `invalidate(attr)` 标脏引用表 →（改 ref-list 则 `PostSetRefListAttribute` 更新 back-ref）→ `VFCRGD` 重绘表单控件、其 3D 画布控件(FZBG3D)触发 3D 场景重建 → `FZXUPD→FUPALL→GLUPDA` 全量 flush。
- **映射方案**：关联判定三支柱 = 正向现场展开 + 反向 back-ref + 按属性精准失效缓存 → 对应 A(判定) + B(反向索引) + 精准失效；**B1** 应挂在"落库写引用属性"处（同 `PostSetRefListAttribute` 挂点）；把 `increment_pipeline::clear_all_caches` 精准化为按引用属性失效，是 **C/D/E "分歧才对齐"** 的候选。

## 3. 工作流 A：影响判定以 core.dll 为准
目标：`src/data_interface/model_impact.rs`（+ `vendor/aios-parse-pdms/all_attr_info.json` 作 att_meta）。
- **A1 判定口径**（ADR-002）：手写 `classify_attribute_effect` 清单**降级为 curated 覆盖层**，保留逆向 nuance（`NEG`/`OBST`/`LEVE` 等标志位不是尺寸却影响模型）；追求行为一致而非逐码一致。
- **A2 att_meta 兜底 + ELEMENT 自动级联**（Q3）：用 `all_attr_info.json` 的 `att_type`——未被清单覆盖的 `ELEMENT`(引用) → 自动归 `DependencyCascade`；数值类型 → `DirectGeometry` 候选；其余保守。加测试：断言 702 属性 100% 有判定且与清单不冲突；未知仍「宁多勿漏」。
  - ✅ **A2 完成**：`classify_attribute_effect_with_meta(name, is_reference)`（名字落 `Unknown` 且引用类型 → 升 `DependencyCascade`）+ `attribute_is_reference(name)`（读 `aios_core::get_default_pdms_db_info().named_attr_info_map`，`att_type==DbAttributeType::ELEMENT` 聚合、懒加载）+ 接入 `classify_operation_effects` + **覆盖测试** `att_meta_all_attributes_classify_and_references_affect_model`（实测 **6556 属性 / 1421 引用类**，全部有判定且引用类均影响模型）。`cargo test -p aios-database --lib model_impact::` **8/8 绿**。
- **A3 变更集对齐 core.dll**：created/deleted/attributeModified 已覆盖；`elementReordered` 已并入 `Modified.children_changed`→`StructuralMembership`→regen（保守正确，保留）；`elementIncluded`（extract 纳入）在离线合并单库视图下 N/A，记录为不适用。

## 4. 工作流 B：反向目录/规格级联（ADR-003）
目标：落库路径（`increment_pipeline.rs`）、`model_refresh.rs`（消费 `cascade_refnos`）。
- **B1 建反向索引**：落库时对每个元素读其正向引用属性（`DEPENDENCY_CASCADE_ATTR_NAMES` ∩ att_type=ELEMENT：`SPRE`/`CATR`/`PRTREF`/`HREF`/`TREF`/`DESP`…），写 `referenced_refno → [referrer_refnos]`（Surreal 边/表）。可先建不消费。
- **B2 增量消费**：变更元素命中 `DependencyCascade` 且属 CATA/规格库时 → 查反向索引得引用实例 → 并入 `changed_seed_refnos` → `resolve_significant_owner` 归一 → 重生成。补上 `model_refresh.rs` 现有 TODO。
- **B3 间接引用**：显式处理 spec 表链（`SPRE→SCOM 组件→PRTREF→TABITE`）与克隆副本；按 noun 选目录入口属性（一般 `SPRE`，`NOZZ/ELCONN/EQUCOM` 用 `CATR`，`TUBI` 按 `TYPE` 选 `HSTU/LSTU`）。

## 5. 验收与测试（行为对齐语料）
- 建「设计变更 → 期望重生成根集合」语料：几何属性改 / 位姿改 / `OWNER` 搬迁 / 目录参数改 / **共享目录改→多实例**。
- 断言 gen-model 产出的重生成根集合 = 期望集合。
- 可选（将来）：用活 E3D / core.dll 对同一变更取「重绘集合」作黄金基准。

## 6. 分期
1. **A2/A3**（att_meta 兜底 + ELEMENT 级联 + 覆盖测试）——低风险、独立。
2. **B1**（反向索引落库）——独立，可先建不消费。
3. **B2/B3**（消费反向索引 + 间接引用）——依赖 B1。
4. **行为对齐语料与测试**——贯穿全程。

## 7. 风险与未决
- att_meta 覆盖：`all_attr_info.json` 按项目 dump，跨项目属性差异需确认合并口径。
- 间接引用完备性：spec 表 / 克隆的反向覆盖是 B 的主要风险点。
- 直读 E3D back-ref：将来优化 ADR（需更全字典 + 解析器扩展）。
- C/D/E：暂不动，除非行为对齐测试暴露分歧。
