# 0002 — core.dll 模型更新逻辑：事件驱动传播 + 按元素图形重生成

- **日期**：2026-07-23
- **背景**：深挖 AVEVA E3D `core.dll` 的"改数据 → 更新模型/图形"链路（IDA 会话 `core31-retrace`，`D:\AVEVA\Everything3D3.1\core.dll`）。
- **相关**：`0001`（编排层/分类/渲染）；`docs/plans/core-dll-aligned-incremental-gen.md`；`docs/adr/ADR-002`、`ADR-003`。

## 关键洞见（均有地址 / 反编译证据）

1. **两套并行系统**：① 数据一致性 / 引用缓存传播（观察者 plugger）；② 图形重生成（按元素、类型分派）。二者解耦：先失效缓存 → 再按元素重建 → 最后全量刷新显示。

2. **变更中枢 = `DB_ElementChangesPlugger`（观察者 / plugger）**，注册分类型 handler：
   - `DB_PostSetAttributeHandler`（标量属性写）
   - `DB_PostSetRefListAttributeHandler`（引用列表写 → 维护 back-ref 逆指针 `SPBREF`/…）
     ⚠️ 2026-07-26 精化（详见 `0007`）：`DB_ElementChangesPlugger::PostSetRefListAttribute`(0x591E780) 本身**只广播**——
     过一道 `DB_Attribute::wnoevt`(0x58d5290) 闸门后，遍历订阅者数组虚调用转发「(元素, 引用属性, 新引用目标列表)」三件套；
     真正写 back-ref 的是订阅它的 handler（订阅入口 `SubscribePostSetRefListAttribute` 0x581f7e0，尚未逐个定位）。
   - `DB_PostCreateElementHandler` / `DB_PostCreateCopyElementHandler` / Post*DeleteElement
   - `DB_PreSetAttributePlugger`（写前合法性）
   - 具体订阅者：`ADM_SCPlugsForDb / TYPEDB / OthersACLASS / ECLASS / UTCSWT / PRJLCK::PostSetAttribute`（Schema Control 插件）。

3. **按属性精准失效引用表缓存（实测闭环）**：属性写 → `DB_RefTabDatabasesPostSetAttr::PostSetAttribute`(0x59fbd00) → `DB_RefTableDatabases::invalidate(const DB_Attribute*)`(0x59fbfe0)。invalidate 在一棵按属性指针排序的 RB-tree 里定位该属性的引用表项，置脏位 `*(node[5]+4)=1` → 后续对该 spec / 目录引用的查询重解析；`invalidateAll`(0x59fc020) 全清。

4. **图形重绘按控件(gadget)类型分派（⚠️ 2026-07-23 修正，详见 `0005`）**：`VFCRGD`(sub_52DB664, `fmgadget/VFCRGD`) 实为 **Forms&Menus 的 GUI 控件(gadget)重绘派发器**——遍历一个表单的 gadget、按 `HQTYPE`(=**控件类型**，非 noun graphicsBehaviour) 的 switch 分派到各 GUI 控件构建器（`FZBBTN`按钮 / `FZBTEX`文本 / `FZBSLD`滑块 / `FZBLIS`列表 / `FZBFRA`框 / …，14 个真名全是 `fm*` GUI 控件）。其中 **case 3/16 = `FZBG3D`(sub_5296A17, `fm3dcanv/FZBG3D`) = "3D 画布控件"**，是 GUI→3D 的桥：它调 `FZ3SGL`(sub_5297141, `fm3dcanv/FZ3SGL`) 建 GL 段（`GLVCRE/GLSVCR/GLVSEG`）。⚠️ **真正的 3D 逐-noun 几何派发不在 VFCRGD**（那是 GUI 控件表），而在 FZBG3D 之下的场景填充路径、由 `DB_Noun::graphicsBehaviour`(字段 5099119) 驱动——**待定位**。（原记录误将这 31 个 case 当作"按 noun 分派几何"，现更正。）

5. **正向关联在重建时现场展开**：catalogue 元素重建时按 `ATT_SPRE`/`ATT_CATR` → `NOUN_SCOM`/SPCO → `GMSET`(geomset 字段 #859903) → `ATT_DESP`/`ATT_PARA` 展开图元（形状不存在设计元素里，绘制时现场算）。

6. **显示刷新是粗粒度全量**：`FZXUPD`(0x5294555)→图形模式闸门→`FUPALL`(0x52f1f82)→`GLUPDA`(0x5aa90d0) flush 所有视图段 + `RIO_OutputListener::SendUpdate`(0x588b110)。GINO metafile opcode 表含 "Regenerate picture / Regenerate view / Make outline from primitive-group"。**core.dll 不计算"最小重生成集"**。

7. **端到端时序（改 PARA/SPRE）**：写属性 → `PostSetAttribute` 链 → `invalidate(attr)` 标脏引用表 →（改 ref-list 则 `PostSetRefListAttribute` 更新 back-ref）→ CE / 视图变更触发 `VFCRGD` **重绘表单控件**；其 **3D 画布控件 `FZBG3D`** 触发 3D 场景重建 → `FZ3SGL` 建 GL 段 + 场景填充路径（读到失效后重解析的新引用、按 GMSET+PARA 重展开）→ `FZXUPD→FUPALL→GLUPDA` 全量 flush + 通知视图。（⚠️ 原写"VFCRGD 重建元素(+成员)"不准确：VFCRGD 重绘的是 GUI 控件、经 FZBG3D 桥入 3D 场景，见 `0005`。）

## 对 gen-model 的启示

- **关联判定三支柱**：正向现场展开 + 反向 back-ref 逆指针 + 按属性精准失效引用表缓存，三者合力保证"关联模型也更新"。方案对应 A(判定) + B(反向索引) + 精准失效。
- **B1 反向索引挂点**：core.dll 在"写 ref-list 属性"时(`PostSetRefListAttribute`)维护 back-ref → gen-model 自建正向反转索引应同样挂在"落库写引用属性"处（ADR-003）。
- **精准失效候选**：`increment_pipeline` 的 `clear_all_caches` 是按 refno 粗失效；core.dll 是按属性精准失效引用表 → 若行为对齐测试暴露过 / 欠失效，可对齐为按引用属性精准化（C/D/E "分歧才对齐"）。
- **颗粒不照抄**：core.dll 按元素重建段 + 全量 flush（在线 viewer）；离线 gen-model 必须自算最小集（owner / 交付单元归一）——印证 C 不动。
