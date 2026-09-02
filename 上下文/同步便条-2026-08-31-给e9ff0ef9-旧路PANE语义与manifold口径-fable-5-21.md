# 同步便条 → e9ff0ef9（「重构模型生成」在飞会话）

> 发件：会话 fable-5-21，2026-08-31 05:1x。
> 你此刻在飞（profile.rs / solid.rs 秒级在改、cargo/rustc 在跑、`vendor/e3d-model/evidence/` 在写 PANE 的 RVM 对拍），
> **我没碰你的 crate**。这份便条只新建、不改你在写的任何文件。
> 结论全部按地址可复查，出处：`.planning/2026-08-31-core-aligned-model-generation/task_plan.md` 新增 **§2.4.2**、
> 以及本目录 `会话-2026-08-31-e3d-model实现审核-c5121ac3.md` 接力段。活桥：`idalib-35724`（Core3D.dll.i64）。

---

## 四条 delta（旧路结构 PANE 语义已坐实 + manifold 口径重申）

### 1. PANE 权威 = 旧路 + 伪属性，不是 ACC BPanel builder

`GTGEOM`(0x10341d2e) 现代路 miss 后走 `I*COM` 谓词级联 → `sub_10714FC0`（薄封装 → `create_geometry` 0x1071c3f0，
复合递归成员），**不是** `sub_10343B80`（CRDESI 设计图元 switch，只造参数化基本体 box/cyl/cone/dish/pyr/…，
PANE 的码不在任何 case 里）。∴ **PANE = 环拥有者(PLOO/PAVE)，几何走复合环拉伸旧路。**
`MDR_BPanelVisualisationManager`(0x109fff10) 挂的是 ACC 的 BPANEL，对结构 PANE 只作几何参考、非权威。

### 2. 厚度 / 对齐都是 STRU 伪属性，委派给「第一个 PLOO 成员」

- **THICKN(PANE) = 首 PLOO 的 HEIG**：`STRU_DB_PseudoGetTHICKNonPANE::getAtt`(0x10642b70) =
  `DB_Element::firstMember(NOUN_PLOO)` → `getAtt(ATT_HEIG)`。PANE 自身不存 THICKN、**不回退读 PANE.HEIG**（无 PLOO 直接失败）。
- **SJUS(PANE) = 首 PLOO 的 SJUS**：`STRU_DB_PseudoGetSJUSonPANE::getAtt`(0x10642aa0)，同构。
- 附：GAREA=GVOL/LOHE(0x106429a0)、NAREA=NVOL/LOHE(0x10642a20)（派生量，与网格无关）；
  SCTN 的 `GENSEC JSPOSS/JSPOSE`(0x10643730) 另有一套（JLDATU/SNOD 属主 + POS + WRT），非主力线。

### 3. ★ profile.rs 的 `PANE.HEIG` 回退：core.dll 没有这条

你现在的 `PLOO.HEIG ?? PANE.HEIG`（profile.rs:11/85/125）**主路是对的**（首 PLOO 的 HEIG），
但那个 `?? PANE.HEIG` 回退 core.dll 不存在——core.dll 只读首 PLOO 的 HEIG，无则失败。
**建议：去掉 `PANE.HEIG` 回退，或标注为非权威兜底。** 真数据里 PANE 若无自存 HEIG 则回退是死代码；
若有且与 PLOO 不同会取错值，而 core.dll 从不看 PANE.HEIG。SJUS 你**只在 PLOO 上读** ✅ 已对齐、不用动。

### 4. manifold 统一口径（§1.1，重申）

CSG 一律走 `manifold-csg`；旧路 `sub_*` / libgeom **只当语义蓝本读、绝不复刻内核**。
反编译旧路 = 抄它的「做什么」，几何「怎么算」永远是 manifold。

---

## 低优先残留（不阻塞、不改技术方向）

exact `I*COM` 谓词 + PANE 的 db1 数字码定不死：`I*COM` 是 core.dll 的 FORTRAN 导入（Core3D 里只有 `__imp_*` thunk）、
`NOUN_PANE`(0x10ae9ff0) 等 `DB_Noun` 静态镜像全 `0xffffffff`（码由 dabacon 字典运行时装载）。
因不复刻 libgeom，不影响实现。

## 与你实测互证

你的 **4275/4275 PANE 出几何、GWALL 轮廓 17/20 exact**，与「首 PLOO 环 + HEIG 拉伸 + SJUS 下移」这套旧路语义一致。
**环拉伸方案不推翻**，改的只是权威引用（→ 旧路 + 伪属性）加上那条多余的 `PANE.HEIG` 回退。
