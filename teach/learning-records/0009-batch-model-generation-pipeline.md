# 0009 — 批量模型生成流水线：core.dll 是"图形/几何供应商"，批量循环在 Core3D.dll

- **日期**：2026-07-27
- **背景**：问"`core.dll` 怎么做批量的模型生成"。结论是**批量循环不在 core.dll**——core.dll 提供图形抽象层(GINO 141 opcode)、图元包围盒(boxlib)、图片存储(cpslib)和 noun 分类器；真正"一条命令生成 N 个元素图形"的循环在 `Core3D.dll` 的 `add / build / css / cachegml / maplib` 五个模块里。
- **会话**：`core31-retrace`(core.dll)、`core3d-retrace`(Core3D.dll)。
- **相关**：修正 `0001` 第 5 条、`0002` 第 4 条（"逐-noun 几何派发待定位"）。

## 一、证据链：为什么说批量不在 core.dll

| 观察 | 证据 |
|---|---|
| core.dll 只建"画布"，不建"每元素段" | `FZ3SGL`(0x5297141) 是 core.dll 里**唯一**调 `GLSVCR/GLVSEG/GLSVBO/GLSVSA/GLSVSE/GLVCRE` 的函数；它只 `GLVCRE` 建 1 个视图 + `GLSVCR(140,140,1,1)` 建固定段。全 core.dll 对 sgl5NET 的调用者只有 35 个函数，全是视图/刷新类。 |
| core.dll 只 import 32 个 sgl5NET 符号 | IAT `0x61518dc..0x6151958`，全是 `GLV*`/`GLS*` 视图段管理 + `GLUPDA`。 |
| GINO 绘图 opcode 全部**只被导出表引用** | `GSTDRG`(0x561e1a6)、`GFIDRG`(0x561ecbd)、`GOPSGI`、`GMASTE`、`CPTSGL`(0x538cb91)、`MBX*` 的唯一 xref 都是 `0x5e14028`（导出目录）⇒ 由外部 DLL 调用。 |
| Core3D.dll 才是消费者 | Core3D.dll 从 `core` 导入 **4859** 个符号；从 `sgl5NET` 导入 **113** 个（远超 core.dll 的 32）；从 `libgm` 导入 **124** 个 `gm_Create*` 实体建模函数。 |

## 二、core.dll 提供的四件"批量原料"（都已导出）

1. **GINO 风格图形抽象层，141 个 opcode**
   - 名字表 `0x60f9f00`（6 字符 × 141：`GOPEN GDFPIC … GSTDRG GFIDRG GCRVEW GSTDRU GQUNWV`），下标→名字 helper `sub_54D4308`。
   - 描述表在 `0x60fd650` 起、步长 `0x190`（40 字符定长）："Start drawing"/"Finish drawing"/"Define segment"/"Define primitive-group"/"Regenerate picture"/"Set segment user-data as Splash element"/"Query Item in View"…
   - **`GSTDRG`/`GFIDRG` = 一次批量绘制的括号**（Start/Finish drawing）。
2. **`boxlib` 逐图元包围盒**（`MBXBOX/MBXCIR/MBXSPH/MBXTOR/MBXSNO/MBXDSH/MBXPYR/MBXSLC/MBXSRE/MBXSAN/MBXARC/MBXELL/MBXSEC/MBXPOI` + `MBXADD` 合并 + `LINBOX/LINVOL/UNVBOX/EXPBOX`）。这是**按图元类型分派的第一张真表**，比几何便宜，用于体积/限界过滤。
3. **`cpslib` = CPS 图片存储**（`CMINIT/CMHEAD/CMGTPF/CMPTGP/CMDLGP/CMGTUD/CMPTUD/CMQSGU`…）。`CMPTUD`(0x5595aec) 用 `GQUSGD`→改→`GSTSGD` 往段上挂 user-data（Splash 元素 id）。
4. **`nounlib` 分类谓词**：`IFCOMP/IPCOMP/IHCOMP/IECOMP/INCOMP/INGCOM/IGMCOM/IG2COM/IASLCO/ICABCO/IPFCOM/LPRMTV/MEMTES`——Core3D 的每一层分派都在调它们。

## 三、Core3D.dll 的批量流水线（本次主结论）

```
add/ADDELE      0x1021b4ec   命令层：ADD <CE|SITE|ZONE|…>
  └ add/ADDDES  0x1021d005   走设计库子树，逐元素登记
      └ css/SEGCRE   0x10232c9d   为该元素建 SGL 段（GLDFSG）
          └ build/MODCMP  0x10251012   按 noun 类挑"建模方式"
              └ build/ELMODL 0x1025277e (7675B)  ★元素模型构建主循环
                  ├ build/DRAWOP  0x10254579   表示法/层级闸门
                  ├ build/SGDRAW  0x102556de (4120B) 逐类分派
                  │   └ build/SGHDRW 0x10255013  层级递归
                  │       └ build/NXTITM 0x102547f5  成员迭代器
                  │           └ build/GMDRAW 0x10254b01  ★逐元素几何
                  ├ cachegml/GTGEOM 0x10341d2e  几何缓存命中/装配
                  └ css/SEGNEW 0x10233094
```

- **`DRAWOP`**：用 `IFCOMP/IPFCOM/IASLCO/ICABCO` 判类，选一个表示法标志字（`dword_10EB4000/4004/4008/400C/4010`），再按 bit 1/2/4/8 开关"障碍/绝热/…"，并调 `LEVGET`(0x1025bc2e) 取绘图 level。**几何还没生成，先过闸门。**
- **`GMDRAW`**：先对 5 个 noun 码直接 return（`621502 / 621505 / 312510241 / 312510247 / 312510290`）；再 `sub_10402560(...)==1` 判"是不是 3D"，否则打点 `"GMDRAW: Skipping non-3d:"`；通过后 `GMCFST` 取缓存首项再绘制。**两级早退。**
- **`NXTITM`**：`*a2==0` 时取首成员、`-1` 表示结束——典型游标式迭代，不物化列表。
- **`ELMODL`** 用 `DSAVE`/`DRESTO` 存取 DB 指针栈做子树遍历，用 `GLSATT/GLSMAT/GLGOTO/GLXIST` 直接操段。

### 另一条批量入口（DRAFT 出图）
`dra_model/GENMOD`(0x100df724) → `updview/DESUPV`(0x101b7b7a) → `UVIEW`(0x101b2254, 12.7KB)。同样最终落到 `MODCMP/ELMODL`。

### 增量与登记
- `maplib` 41 个 `MAP*`/`MU*`：`MAPINS`(0x1038f5e4) 元素↔段登记，`MAPGET/MAPELE/MAPREM/MAPBOX`；`QVOLDS/SVOLPR` 体积过滤。
- `css/SEGREP`(0x102334d0) 只替换一个元素的段（同样调 `MODCMP`）——这就是"改一个元素只重建一个段"。
- `change` 模块：`EVALST/EVALAT/EVALCD/EVALNW/EVALRF`(变化求值) → `UPCACE/UPCACP`(缓存失效) → `UPGRPH`(0x1022eaee, 图形更新) → `UPDATD/UPDATN/UPDATP`。
- `cachegml`：`GTGEOM/GTCLEL/GTCLRF/GTCLSH/GTTUBG` + `BXWANT/SLWANT`（要不要盒/要不要实体的意向位）。

### 真正的实体建模在 libgm（不在 core.dll，也不在 sgl5NET）
Core3D 导入 124 个 `libgm` 符号，**PDMS 目录图元一一对应**：
`gm_CreateBox / gm_CreateCylinder / gm_CreateSnout / gm_CreatePyramid / gm_CreateCircularTorus / gm_CreateRectangularTorus / gm_CreateEllipticalDish / gm_CreateSlopeEndedCylinder / gm_CreateExtrusion / gm_CreateRevolution / gm_CreateRuledSolid / gm_CreatePolyhedron(+AddVertex/AddFacet) / gm_CreateFacetStructure`
以及 CSG 组合与遍历：`gm_CreateCombination / gm_CreateClippedTree / gm_CreateExpandedTree / gm_CompressTree / gm_CreateIterator / gm_CreatePicture / gm_AddMember`。

## 四、对既有记录的修正

1. **`0001` 第 5 条**"sgl5NET 只暴露视图/段管理 + GLUPDA，无逐三角 API"——**只对 core.dll 的 IAT 成立**。Core3D.dll 导入 113 个 sgl5NET 符号，含 `SGL_define_primitive_d`、`SGL_import_external_geometry`、`SGL_define_segment`、`SGL_set_external_geometry_map`。实体建模则在 `libgm`。
2. **`0002` 第 4 条**"真正的 3D 逐-noun 几何派发…待定位"——**已定位**：在 Core3D.dll `build/SGDRAW` + `build/GMDRAW` + `build/MODCMP`，判据是 core.dll `nounlib` 的 `I*COMP` 谓词族，不是 `graphicsBehaviour` 上的一个 switch。
3. **glossary**：`sub_5621F20` 标注为"图形 metafile 令牌解析器（引用 FACET 字典）"**不准确**。它是 metafile/plotfile 的**设备选项命令解析器**（`CHKEYW` 关键字表 `FRAME/TOKEN/ZCOORD/EUC/SHIFTJIS/APPEND/SCALE/SPLASH/FACET/NTLATIN2/RAWENCODING/ESCUNICODE/UTF8`），`FACET` 在这里是"输出要不要含 facet"的开关，不是几何令牌流。
4. `fm3dcanv/F3BDAD`(sub_5297CCF) 不是 drawlist add，是 **3D 画布边框的方位/仰角标尺**（`UIALBL`/`UIASLD` 造 label 与 slider）。同族 `F3BDDL/F3BWDL/F3BDUP` 同理。

## 五、对 gen-model 的启示

- **批量边界 = 子树，不是元素**：AVEVA 一条 `ADD SITE/ZONE` 就是一个批。遍历用 DB 指针栈(`DSAVE/DRESTO`)+游标(`NXTITM`)，**不物化元素列表**——内存与我们现在的做法可对照。
- **闸门在几何之前**：`DRAWOP`(类+level) → `GMDRAW`(noun 早退 + 非 3D 早退) → 才进几何。我们应把 LEVEL/CLFLA/TUFLA 之类过滤前移，别先建网格再丢。
- **缓存键在目录几何层**：`cachegml/GTGEOM` 按目录/图元缓存，不是按设计元素——同型号构件复用一份几何，这是批量的主要加速点。
- **必须有 元素↔产出 登记表**：`maplib` 的存在使得 `SEGREP` 能只换一个段。我们的增量更新要有等价映射（对应 ADR-003 的反向索引，但这里是"产出侧"索引）。
- **包围盒是独立的便宜通道**：`boxlib MBX*` / `dbboxlib` 先算盒做体积过滤，再决定要不要实体（`BXWANT/SLWANT`）。适合我们做"交付单元"级裁剪。

## 六、未验证 / 待办

- `ELMODL`(7675B) 只读了调用面，**未逐块反编译**其子树遍历与 level 分支。
- `GMDRAW` 里 5 个跳过的 noun 码（621502/621505/312510241/312510247/312510290）**未解码成类型名**。
- `MODCMP` 如何在多种"建模方式"间选择（`sub_102586DB / sub_1025B8B3 / sub_1038158C / sub_10651840`）未展开。
- `libgm` 的 `gm_Create*` 与 PDMS 目录图元(SCYL/SBOX/SSNO…)的**精确对应表未逐一核对**，目前是按名字对应的强推断。
- CPS(`cpslib`) 与 3D 设计视图的关系未验证：CPS 明显服务 DRAFT/2D 出图，是否也缓存 3D 段未查。
