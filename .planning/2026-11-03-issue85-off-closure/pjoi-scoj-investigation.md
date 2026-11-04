# PJOI / SCOJ 取证笔记（2026-11-03）

> 任务：7999 only_baseline 里 PJOINT×894 + SCOJOINT×533 真几何缺失的实现。
> 本文件记录已钉死的事实，后续实现直接按此动手，不要重复取证。

## 1. 元素事实（ams7999_0001 全库扫描，`zz_tmp_pjoi_probe`）

- **PJOI 1874 只**，全部 `PNOD → FRMW` 链下。attrs：`GTYP`、`OPDI`（关节方向）、`BANG`、`CUTB`、`CUTP`、`DELP`、`CREF`、`JLIN`、`ISPE`、`SPRE`、`SPAMAP`。
- **SCOJ 922 只**，全部 `SNOD → SCTN` 链下（与 SJOI 同宿主位）。attrs：`POSL`、`DELP`、`BANG`、`GRAD`、`ISPE`、`SPRE`；**member 固定有 1 只 `SUBJ`**（→ RVM 的 SUBJOINT 空子组）。
- RVM 侧：PJOINT 组 1874（1:1 对应 PJOI），SCOJOINT 组 1937（≈ 每 SNOD 一只，含无 SCOJ 的）。SNODE 记录本身带 PRIM = 点标记线（已实现，`exports_mark_as_lines`）。

## 2. 画不画的门（与 facet dump 逐件对上）

- **SCOJ：画 ⇔ 自身 `SPRE` 有效**（=宿主 SCTN 带名 ⇔ 宿主 SCTN 有 SPRE，三者同集，533/922 精确对上）。
- **PJOI：画 ⇔ 自身 `SPRE` 有效 ∧ SPRE→CATR→JOIN 解得出正体**。库内 own_spre=1394（10 无引用 + 14 被无SPRE的SCTN引用 + 1370 被实体SCTN引用），基准只画 894——**差的 ~500 预计是 SPRE 解到空/失败目录**（JOIN spec 在 db 5054），实现后按对拍核。已排除：JOIS/JOIE 引用（1616≠894）、PNOD.FIXT（全 "FREE"）、PNOD 被引用（无）。
- **带名样本**：FRMW `/1RX02-LSUP-1023-FRMW1` 的 PJOI `24383/86117`：`GTYP="JOIN"`、`SPRE→21438/3356(db5054)`、`OPDI=(-0.765,-0.644,0.0007)`、`BANG=90`、`CUTP=OPDI`、`DELP=(-12.86,15.32,0)`；被 SCTN `/1RX02-LSUP-1023-BAR-1`.JOIS 引用（该 SCTN SPRE→21438/3301）。

## 3. 几何形状（facet dump 还原，record 帧）

- **PJOINT**：每只 1 个 `FacetGroup` ≈26 polygons——外八边形 R=100、厚 6mm、**两个八边形孔 R=10 @y=±61** 的节点板（环0=外轮廓，环1/2=两孔）。z∈[0,6]。属 catalogue JOIN 件（同 SJOI 的 SPRE→SPCO→CATR→JOIN→GMRE 链）。
- **SCOJOINT**：螺栓组件 —— `FacetGroup` 板 200×200×6 + `Cylinder` 杆 R10×150 + `Cylinder` 帽 R12.5×3（bbox (-12.5,-12.5,-1.5,12.5,12.5,1.5)），每只 SCOJ ≈8 图元。同为 catalogue JOIN。
- 每只带几何 PJOINT 的 bbox z 恒 ±100（板平面内），XY 因帧旋转变化。

## 4. RVM 记录结构（80211 基准二进制）

```
CNTB PNODE n of FRMWORK m …        （平移=PNOD 世界原点 mm）
  CNTB PJOINT 1 of PNODE n …       （画时内含 FacetGroup PRIM）
  CNTE / CNTE
CNTB SNODE n of SCTN m …           （含 ≤3 条点标记 PRIM）
  CNTB SCOJOINT 1 of SNODE n …
    CNTB SUBJOINT 1 of SCOJOINT … CNTE（恒空子组）
  CNTE / CNTE
```

生成侧现状：**PNODE/SNODE/PJOINT/SCOJOINT/SUBJOINT 记录全不发**（generated.rvm grep=0）。需补整条容器链 + PJOINT/SCOJOINT 内几何。

## 5. 代码挂点（全部已存在，SJOI 同型）

- `category.rs`：`is_catalogue_joint`（现只认 SJOI@SNOD@SCTN）→ 加 PJOI@PNOD@FRMW、SCOJ@SNOD@SCTN；`builds_from_catalogue_component` 名单加 `"PJOI"|"SCOJ"`；`is_model_unit_of` 的 SJOI 特判扩到 PJOI/SCOJ。
- `transform.rs`：`ImplicitFrame::SectionJoint` 给 SJOI 的标架；PJOI 需新变体（PNOD NPOS + OPDI/CUTP/BANG/DELP——参考 `datum::section_joint_world` 同族公式，86117 的 CUTP=OPDI、DELP≠0）；SCOJ 用 SNOD 站位帧（`ImplicitFrame::SectionNode` 同源）。
- `elmodl.rs::model_element_with_cache`：`Category::Catalog && builds_from_catalogue_component` → `model_catalogue_member_with_cache`，正体负体一刀不动。
- 负体链 `member_chain_mode`：`(SNOD,SJOI)=1` `(SCOJ,-)=2` `(SCOJ,SUBJ)=1` 已就位。

## 6. 未完成 / 风险

- PJOI 的 ~500 only_generated 风险：own_spre=1394 vs 基准 894，若 SPRE→JOIN 解不出正体会自然落到 NoShape，与 SJOI 的 `is_catalogue_joint` 同门——先实现再对拍。
- PJOI 帧公式未推：86117 的 record transform（RVM transform_cols `[0.000644,-0.000765,~0, 5.7e-7,4.8e-7,-0.001, 0.000765,0.000644,~0, 12.82,11.87,-1.05]`）vs PNOD NPOS(113,0,-42.3)+DELP(-12.86,15.32,0)+OPDI/CUTP(-0.765,-0.644)+BANG=90——世界原点差 = FRMW 世界变换，需对 `world_matrix` 链核一两只钉帧公式。
- SCOJ 帧：SNOD ZDIS 站位 + host 轴（同 `ImplicitFrame::SectionNode`），BANG/POSL 可能参与旋转。
- 工具：`zz_tmp_pjoi_probe`（--noun PJOI|SCOJ|--name <frmw名> 子树/--limit N；cross 统计）；facet dumps 在 `gen-bisect/{pjoint,scoj}_facets.json`。
