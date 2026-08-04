# 0001 — core.dll 是"编排层"，分类靠 schema，渲染靠 sgl5NET

- **日期**：2026-07-23
- **背景**：分析 AVEVA E3D `core.dll` 的"几何体 update 图形"。

## 关键洞见
1. `core.dll` 本身不做像素/网格渲染，是**编排层**：判类型、跑更新队列、发刷新信号。
2. **类型分类是数据驱动**：元素类型=整数 noun 码，由字典库(dabacon)描述为 `DB_Noun` 对象；
   `primitive()`(字段#659518)、`graphicsBehaviour()`(this+0xB4 枚举)、`geomset()`、`extrusion()` 决定是否几何体及画法。
3. **关联模型生成**：设计元素 →(SPREF: PSPREF/FSPREF)→ 目录 SPCO → GMSET 几何集 → 用 DESPARAM/PARA[] 参数化展开图元(SCYL/SBOX/SSPH…)。
4. **图形容器**：`FZ3SGL`(sub_5297141) 用 sgl5NET 建"视图+段"（GLVCRE/GLSVCR/GLVSEG）；几何入段。
5. **三角化不在 core**：sgl5NET 只暴露"视图/段管理+GLUPDA"，无逐三角 API；facet 化在 FORTRAN 几何引擎/sgl5NET，经 metafile(FACET 令牌)交换；几何数学在 libgeom。

## 对 gen-model 的启示
- 复刻几何 = 复刻"SPREF→SPCO→GMSET→PARA[] 展开图元"这条解析链；draw 与否看 LEVEL/CLFLA/TUFLA。
- 增量更新 = 元素/属性变更 → 标记 → 重建该元素的段 → flush；与 core 的更新队列模型一致。
