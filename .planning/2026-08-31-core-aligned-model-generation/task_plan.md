# Plannator 开发计划:e3d-model — 按 core.dll CSG 架构另立的模型生成算法

> 计划 ID:`2026-08-31-core-aligned-model-generation`
> 创建:2026-08-31
> 状态:**approved**（plannotator 门禁 2026-08-31 r3 `approved`,见 `.gate-result-r3.json`;
>   r1/r2 批注已吸收,见文末批注处置记录。**门禁通过后又用活桥结掉了 §2.4.1 阻断项,
>   该节已从「存疑」改为「坐实」,结论见下——这条修正若需重过门,请拍板**)
>   **fable-5-21 续:又结掉 §2.4.1 定的 P2 前置 = 旧路 PANE 语义(见 §2.4.2)——PANE 走复合
>   环拉伸旧路、厚度 = 首 PLOO 的 HEIG、对齐 = 首 PLOO 的 SJUS;不改技术方向,P2 首项已可动工。**
> 拍板前提:
> - 2026-08-31 00:05 用户:「提供 e3d-io 的数据接口,用来直接生成模型,按 ams 1112 测试。
>   模型生成的算法相当于**另外一套**,按照 core.dll 里提供的模型生成方案。」
> - 00:14 用户拍板:新建 `vendor/e3d-model` crate(依赖 e3d-io + manifold-csg + glam);
>   首里程碑 = 骨架 + 结构最小闭环;验收走 **E3D TTY 导出 RVM 对比**(不是 fast_model 双跑)。
> - 01:13 用户:「结合 ida-bridge,继续完善对模型生成的开发计划文档,使用 plannator。」
> - 01:22 用户(plannotator 批注):「**CSG 的部分,我们可以使用 manifold 来代替**」
>   → 定下 §1.1 的内核边界:core.dll 只作语义权威,几何内核用 manifold,**不复刻 libgm**。
>
> 权威(按证据强度排序):
> 1. **ida-bridge 活体反编译**(本计划 §2,本会话 01:1x 实测,实例 `idalib-35724` /
>    `D:\ida_scratch\plant3\Core3D.dll.i64`)——凡标「活桥坐实」的都能按地址复查。
> 2. `上下文/会话-2026-08-31-core-dll模型生成活桥分析-7UW4.md`(7UW4 首轮活桥分析)。
> 3. `上下文/会话-2026-08-31-e3d-model实现审核-c5121ac3.md`(本会话 01:10 实现审核)。
> 4. `docs/plans/2026-08-30-core-dll-api-alignment.md`(读侧对标矩阵)、teach/0009、teach/0011。
>
> 上下文:`上下文/会话-2026-08-31-e3d-model实现审核-c5121ac3.md`(实时更新)

## 与既有计划的关系

`.planning/2026-08-30-direct-read-model-generation/task_plan.md`(gate approved,Phase 1 完)
是**数据源线**:把取数从 SurrealDB 换成直读 `.dat`,前提写死「生成算法本身不重写」。
本计划是**算法线**:算法另立一套,按 core.dll 的 CSG 架构复刻。两条线的关系:

| | 数据源线(0830) | 算法线(本计划) |
|---|---|---|
| 载体 | gen-model `src/data_interface/` + e3d-io | `vendor/e3d-model` 新 crate |
| 算法 | 复用 fast_model | 另立,镜像 core.dll `CSG_TreeBuilder` 家族 |
| 关系 | 本计划**消费**它的 Phase 2 读侧成果(S1 `DbElement` 门面已在 e3d-io `6bea669`) | 它的 Phase 4「试点」被本计划替代 |

**不改旧计划文档**,避免与在飞 agent 撞。旧计划 Phase 3(G4 目录表达式)是本计划 P4 的硬依赖。

---

## 一、目标

在 `vendor/e3d-model` 里实现一套**镜像 core.dll 现代 C++ CSG 架构**的模型生成算法:
数据一律经 e3d-io 直读 dabacon 库文件(不连任何数据库),按 noun 分派到各自的
tree builder,产出 CSG 树 → 网格,以 **ams1112**(dbnum 1112,~30940 元素)为试点语料,
以 **E3D TTY 导出 RVM** 为验收基准。

### 1.1 内核边界:core.dll 管语义,manifold 管几何

用户 2026-08-31 01:22 批注拍板。这条划死了「对标到哪一层」:

| | 权威 | 说明 |
|---|---|---|
| **语义层**(照抄 core.dll) | `CSG_TreeBuilder` 家族 | 哪个 noun 归哪个 builder;哪些成员算负几何;谁减谁、什么顺序;容器何时让位;门控(`isWanted`)条件 |
| **几何层**(用 manifold) | `manifold-csg` | 布尔运算、网格化、容差、鲁棒性 |

**推论(直接改实现口径)**:

- libgm 的 `gm_CreateCombination(3)` / `gm_AddMember` / `gm_CreateTransform` /
  `gm_CreateFacetStructure` 只作**语义读法**——`op=3` 读成「差集」,`AddMember` 顺序读成
  「首个成员是被减数」。**不复刻 gm 的内核实现**,布尔一律交 manifold。
  > 注:**transform 节点要不要保留,见 §1.2**——那里按「实例化复用」的需要把这条收窄成
  > 「**只在必须做布尔时才烘平变换**」,平时保留 (几何, 变换) 的分离形态。
- **不追位级一致**。manifold 与 libgm 是两个内核,网格三角化、容差、退化处理必然不同。
  RVM 对拍的门是**体积 / AABB / 连通分量**这类内核无关不变量,不是逐三角对比。
- **`libgm.dll` 的逆向需求随之注销**(原 §6.3 的 GM_Operation 枚举、facet 网格化算法与容差)。
  只保留一条:`op` 值除 `3=差集` 外若在别的 builder 里出现新值,按语义查证(交/并),
  这在 Core3D.dll 内部就能看出来,不必另起 libgm 的 i64。
- **`restol` 让刀、倒圆段数规则仍然要照抄**——那是**输入侧**的离散化口径(决定送进内核的
  多边形长什么样),属语义层,不属内核层。审核已确认 e3d-model 这块实现站得住。

### 1.2 图元实例化:一份基本体 + 变换复用,少生成 mesh

用户 2026-08-31 01:28 批注拍板:「还要设计一套,可以复用基本体的方案,可以方便减少 mesh 的
生成,通过 transform 来缩放已有的 mesh」。

**先说清它和 §1.1 的张力。** §1.1 我写了「不留 gm 风格的 transform 节点、直接烘平」,
这条批注把它按回去了一半——而且**按回去的方向恰好更贴 core.dll**:libgm 的
`gm_AddMember(geom, transform, comb)` 本身就是「几何 + 变换」的场景图,
`CSG_BasicPrimitive::primList_` 也本来就是**每个几何类只有一个单例 builder**。
所以复用不是我们的发明,是把 core.dll 原本的结构照抄回来。收窄后的规矩:

> **默认保留 (共享几何, 实例变换) 的分离形态;只有该元素确实要做布尔时,才把变换烘进
> 操作数、跑 manifold、产出独立结果。**

#### 1.2.1 缓存键怎么设(关键在「形状参数」和「尺寸」要分开)

一个图元能不能靠仿射变换从别人身上变出来,取决于它的参数**哪些是形状、哪些是尺寸**:

| 几何类 | 可仿射复用 | 缓存键里的形状参数 | 由变换承担 |
|---|---|---|---|
| `CSG_BasicBOX` | ✅ 完全 | 无 | xlen/ylen/zlen 三轴缩放 |
| `CSG_BasicCYL` | ✅ | **段数 N** | r、h |
| `CSG_BasicCON` | ✅ | N、**上下半径比** | 整体缩放 |
| `CSG_BasicSNO` | ✅ | N、半径比、偏心 | 整体缩放 |
| `CSG_BasicPYR` | ⚠️ 部分 | 上下底比例、偏移比例 | 整体缩放 |
| `CSG_BasicSLC` | ✅ | N、**切角** | r、h |
| `CSG_BasicCTO`/`RTO` | ✅ | N、M、**细/粗半径比**、包角 | 整体缩放 |
| `CSG_BasicDIS` | ✅ | N、**高径比** | 整体缩放 |
| `CSG_BasicEXT`/`REV` | ❌ 一般不行 | 整条轮廓(含倒圆折线化结果)的哈希 | — |
| `CSG_BasicPOL` | ❌ | 顶点表哈希 | — |

**★ 最容易踩的坑:段数 N 必须进缓存键。** core.dll 的段数由 `restol` 弦高容差 + 真实半径
决定(审核已确认 e3d-model 照抄了这套,含封顶 1000、取 4 的倍数)。把一个单位圆柱缩放到
大半径,拿到的是**缓存那份的段数**,不是这个半径该有的段数——体积会差
(N 边形棱柱体积 = `(N/2)·sin(2π/N)·r²·h`),RVM 对拍直接飘。
反过来,**只要 N 相同,缩放就是数学精确的**:按 r 缩放单位 N 边形棱柱,与直接按半径 r
生成 N 边形棱柱,逐顶点相等。所以规矩是:**先按 restol 算出 N,再拿 N 进键查缓存。**

#### 1.2.2 收益落在哪(别高估)

- **没有负几何的图元**(ams1112 里绝大多数)压根不用跑布尔,直接发一条实例记录即可——
  这是最大的一块,省的是布尔不是三角化。
- **目录件是第二大块**:FITT/SBFI/PFIT 把同一份目录几何实例化成百上千次,按
  「目录元素 ref + 求解后的设计参数」做键去重,天然高命中。
- **要做布尔的元素拿不到复用红利**(结果各不相同),但**操作数**仍可复用——
  同规格的 NBOX/NCYL 挖洞在结构件里高度重复。
- **输出侧**:OBJ 不支持实例化,导出时仍要烘平(省的是生成不是文件体积);
  JSON 不变量与后续可能的 glTF 输出应当**保留实例表**(一份 mesh + N 个变换)。

#### 1.2.3 正确性怎么钉住

- **A/B 自证**:留 `--no-instance-cache` 开关跑同一批元素,两条路的产物必须
  **逐元素体积/AABB/连通分量完全一致**(不是"在容差内",是一致——同 N 的缩放是精确的)。
  这条进 CI,防止哪天改了缓存键悄悄改了几何。
- **属性测试**:随机参数下 `build(params)` 与 `unit_mesh(key).transform(M)` 顶点集相等。
- **镜像/负行列式**:变换含反射时三角绕向要翻,法线走逆转置。这条单独写测试,
  它是最典型的"看着没事、体积对、法线全反"的坑。
- **缓存键漏项 = 静默错几何**,比崩还难查。键的每一项都要有一行注释说明为什么它是形状参数。

---

## 二、ida-bridge 实证底账(本轮新增,计划的事实地基)

活桥环境可复用(7UW4 已起,本会话续用):

```powershell
ida-bridge list                                    # idalib-35724 → Core3D.dll.i64
ida-bridge exec idalib-35724 --sql "SELECT group_concat(line,char(10)) FROM pseudocode WHERE func_ea=0x……"
# pseudocode 表必须带 WHERE func_ea=<地址> 或 ea=<地址>;names 表列名是 address 不是 ea
```

### 2.1 生成主干:两条路 + 一个几何内核(7UW4 坐实)

```
ELMODL → GTGEOM(0x10341d2e)
   ├─(1) 现代 C++ CSG【首选】 GTGM2(0x10714e60) → CSG_TreeBuilder::getCSGTree(0x10715b30)
   │        → 按 noun 查 builder 注册表 → builder->getCSGTree(elem, options, &transform)
   └─(2) 旧 FORTRAN【回退】 noun 大 switch(目录件 0x10714fc0 / 设计图元 0x10343b80)
   ▼ SGDRAW(0x102556de) 逐项套 MAT16/CONCAT → 落图形段
   ▼ libgm: gm_CreateCombination / gm_AddMember / gm_CreateTransform / gm_CreateFacetStructure
```

最后那层 libgm **只读语义不复刻实现**(§1.1),本仓对应位置换成 manifold。

> ⚠️ 上一稿在这里写了「**本计划只对标现代路**,旧 FORTRAN 路仅作对照」。
> **这句已被 §2.4.1 坐实推翻**:ams1112 的主力 noun 在 Core3D.dll 里根本没有现代 plug,
> 走的是旧路。对 ams1112 而言旧路是**主路**,现代 builder 族只对少数 ACC/管系 noun 生效。

### 2.2 完整 builder family(活桥坐实:`names LIKE '%getCSGTree%'`,19 个具体实现)

| builder | 地址 | ams1112 相关度 | 本计划阶段 |
|---|---|---|---|
| `MDR_BPanelVisualisationManager` | 0x109fff10 | ★★★ PANE 4275 | P2 |
| `MDR_WallVisualisationManager` | 0x10a00340 | ★★★ GWALL/CWALL/STWALL/WALL | P2 |
| `MDR_WallJoinerVisualisationManager` | 0x10a00230 | ★★ 墙接头 | P2 |
| `CSG_TreeBuilderPrimitive` | 0x10726890 | ★★★ 正图元 | P1/P3 |
| `CSG_TreeBuilderNegativePrimitive` | 0x10726770 | ★★★ N* 负几何 | P1/P3 |
| `CSG_TreeBuilderMyBox` | 0x10728c40 | ★ | P3 |
| `CSG_TreeBuilderCat` | 0x1072f5d0 | ★★ FITT/SBFI/PFIT 255+ | P4 |
| `CSG_TreeBuilderFLRLAY` | 0x1090d780 | ★★ FLOOR/CFLOOR 96 | P2 |
| `CSG_TreeBuilderBNDLIN` | 0x1090cc20 | ★ | 记账 |
| `CSG_TreeBuilderCLNTIL` | 0x1090d0d0 | ★ | 记账 |
| `CSG_TreeBuilderINSURQ` | 0x1090d9e0 | ★ | 记账 |
| `CSG_TreeBuilderAccommodationFitting` | 0x1090ca10 | ★ | 记账 |
| `ACC_WallProfileVisualisationManager` | 0x109ffde0 | ★ | 记账 |
| `ASL_MDR_Handrail/Kickplate/Platform/Rail` | 0x109f96a0 / 9890 / 99c0 / 9c10 | — | 记账 |
| `MDR_HRPostVisualisationManager` | 0x109f4ed0 | — | 记账 |
| `MDR_Branch/SegmentVisualisationManager` | 0x105e9aa0 / 9c80 | — 1112 无管 | 不做 |
| `xba_drawgenericprimitive` / `xba_drawterraincurve` | 0x1077e190 / e690 | — | 不做 |
| `CSG_BaseCSGTree`(基类) | 0x10715a60 | — | — |

### 2.3 ★ 完整图元注册表 `CSG_BasicPrimitive::primList_`(活桥坐实:`CSG_PrimitiveUtilities::initialise` 0x10727540)

**7UW4 §7.2 留的空白,本轮补齐。** 12 个几何类,正负 noun **成对共用同一个几何 builder**
——负几何不是另一种几何,只是另一个 tree builder 包装:

| 几何类 | 正 noun | 负 noun | ams1112 数量(正/负) |
|---|---|---|---|
| `CSG_BasicBOX` | BOX | NBOX | — / 181 |
| `CSG_BasicCYL` | CYLI | NCYL | — / 60 |
| `CSG_BasicCON` | CONE | NCON | — / — |
| `CSG_BasicPYR` | PYRA | NPYR | — / 15 |
| `CSG_BasicDIS` | DISH | NDIS | — / — |
| `CSG_BasicSNO` | SNOU | NSNO | — / — |
| `CSG_BasicCTO` | CTOR | NCTO | — / 4 |
| `CSG_BasicRTO` | RTOR | NRTO | — / 8 |
| `CSG_BasicPOL` | POLYHE | NPOLYH | — / — |
| `CSG_BasicSLC` | SLCY | **NSLC** | — / — |
| `CSG_BasicEXT` | EXTR | **NXTR** | — / 210 |
| `CSG_BasicREV` | REVO | **NREV** | — / 2 |

**立刻可用的结论**:
- `NXTR` 与 `EXTR` 是**同一个 `CSG_BasicEXT`**——环拉伸在 core.dll 里是一个**图元**,
  不是板专属路径。e3d-model 现在把 NXTR 当「负环拉伸」单独写,语义上对,但应收进图元族统一实现。
- `NREV`/`REVO` 同属 `CSG_BasicREV`——e3d-model 现在把 Revolution 整个判为里程碑外,
  但它在 core.dll 是**基础图元**,不是高级特性,补它的成本与补 BOX 同级。
- **e3d-model 分类表与注册表有出入,必须逐条对齐**(P1):e3d-model 写的 `NSCY` 在
  core.dll 注册表里是 `NSLC`;
  `POHE` 与 `POLYHE` 需按 e3d-io 反哈希出的规范名收敛成一个。

> ★ **修正(2026-08-31 会话 7KG8,字典实证)**:上一稿这里写的「`NSBO`/`NLCY` 在注册表里
> **根本不存在**」**是错的**,连带 P1 的处置动作「删或降为 `Unknown`」也是错的。
> 按 dabacon 字典(`attlib.dat`,core.dll 自己读的那份)实读:
>
> | noun | 能力位 | 家族位 | 归属 |
> |---|---|---|---|
> | `NSLC` | `primitive` | **INCOMP** | 设计负图元,正体 `SLCY` —— **e3d-model 整个漏掉,落 Unknown** |
> | `NSCY` / `NSBO` / `NLCY` | `geomset` | **INGCOM** | 几何**集**负体,是**另一个家族** |
>
> 它们**存在**,只是不在 Core3D 的 `primList_` 里——因为几何集家族走的是
> `CGTCT2 + sub_10714FC0` 这条独立的路,不是设计图元路。**照原动作删掉,等于把
> 几何集这一整族(IGMCOM 21 + INGCOM 16 + IG2COM 5 = 42 个 noun)从账上抹掉。**
> 正确动作见 `.planning/2026-08-31-noun-coverage-closure/task_plan.md` §4 缺陷 2 / 缺陷 4。

### 2.4 ★ 现代路 plug 全表(活桥坐实:`CSG_TreeBuilder::addPlug` 0x107158b0 的**全部 12 个调用点**)

上一稿只看了 `CSG_PrimitiveUtilities::initialise` 一处,把「DISH/SNOU/… 没有 plug」列成待查。
本轮按 xref 把 12 个注册点**全部枚举**(11 个反编译成功):

| 注册点 | 挂上的 noun |
|---|---|
| `CSG_PrimitiveUtilities::initialise` 0x10727540 | Primitive:**BOX/CYLI/CONE/PYRA**;Negative:**NBOX/NCYL/NCON/NPYR**;Cat:NOZZ/SUBCOM/HVBRCO/HVFLAN/HVHACC/HVSADD/HVSPLR/HVSTIF/HVTPPO/HVSKIR/HVIDAM |
| `ACC_MDR_CSGUtility::initialise` 0x10a00520 | **CTWALL**→Wall、**BPANEL**→BPanel、WLJOIN→WallJoiner、WLPROF→WallProfile、WLPANE→WallPanelPlugger |
| ACC 建筑(未命名)0x108f54f0 | FLRLAY、CLNTIL、FPFITT、ELFITT、HVACFI、INFITT(+2 个间接 noun,疑 BNDLIN/INSURQ) |
| `ASL_Manager::initialise` 0x108f89a0 | STRFLT、RLADDR、SLADDR、HRPOST |
| `MDR_CSGUtility::initialise` 0x105e9db0 | HATTA、TATTA(+Branch/AttachmentPoint/Segment 走间接 noun) |
| `CAB_MDR_CableWayManager::initialise` 0x109b55e0 | RNODE、CNODE、CTSTRA、CTCOUP、CTBEND、CTRISE、CTTEE、CTCROS、CTREDU、CWBRAN、POINTR |
| 未命名 0x109b61a0 | CABLE |
| 未命名 0x1062cce0 | REFGLN、REFGAR、AIDLIN、AIDARC、AIDCIR、AIDPOI、LINDIM、MLABEL、STWELD |
| 0x107293d0 / 0x10728ec0 | MYBOX/MYOBJ/MYSHIP/MYNOZ —— UDET **示例插件**,非产品 noun |
| `xba_base::Init` 0x1077cc00 | **未知**:Hex-Rays 报 `decompile returned None`,唯一没读到的一处 |

**两条结论:**

1. **原「不对称」查证项已结案**:DISH/SNOU/CTOR/RTOR/POLYHE/SLCY/EXTR/REVO 及其负体
   **确实全无 plug**,只活在 `primList_` 里,只在被属主当**负几何成员**时经 `findPrimitive` 取用。
   之前列的两种可能中,①成立。
2. **★★ 但同时炸出一个前提级问题,见 §2.4.1。**

### 2.4.1 ★★ 前提修正(已坐实):ams1112 的主力 noun 走旧路,不走现代 CSG builder

**PANE / CWALL / GWALL / STWALL / WALL / FLOOR / CFLOOR / SCTN —— §2.4 的 12 个注册点里一个都没有。**
上一稿把这条列成「阻断项 / 待查证」;本轮(2026-08-31 恢复会话 2e34fff3)用活桥把三条反证
方向全部排除,**结论坐实**:这些 noun 在 core.dll 体系里由**旧路**构建,现代 builder 族与它们无关。

**四条证据(都可按地址复查):**

1. **派发口径**(`CSG_TreeBuilder::getCSGTree` 0x10715b30,本轮反编译):按
   `actualType() → type() → hardType()` 三键依次查 plug 树,**三键全 miss 就 `return 0`**。
   (三键别名到已挂 noun 的可能性:结构 noun 各有独立伪属性注册,别名概率极低,未逐一验证,列 §7 残留。)
2. **回退是设计好的正式路,不是残留**(`GTGEOM` 0x10341d2e,本轮反编译):先试现代路
   `GTGM2`(sub_10714E60);`(结果 & 1)==0`(即 getCSGTree 返 0)就落进 `I*COM` 谓词级联
   ——`IPCOMP / IHCOMP / IFCOMP / IPFCOM / IECOMP / ICABCO / INCOMP / IG2COM / IGMCOM /
   INGCOM / ICCOMP`——按族分派到旧几何 builder `sub_10714FC0` 或设计图元 switch `sub_10343B80`。
   现代命中才走 `else` 分支。**core.dll 明确预期「现代没挂的 noun 落旧路」。**
3. **Core3D 内无结构 noun 的 CSG plug**:枚举全部引用 `NOUN_PANE`(0x10ae9ff0)/`CWALL`/
   `GWALL`/… 的函数,唯一的「注册」函数 0x1062cce0 用的是
   `DB_PseudoAttPlugger::instance(ATT_THICKN / ATT_SJUS / ATT_GAREA / ATT_NAREA, NOUN_PANE/GWALL/FLOOR, …)`
   ——**伪属性插头,不是 `CSG_TreeBuilder::addPlug`**。(附带收获:PANE/GWALL/FLOOR 的厚度
   `ATT_THICKN`、对齐 `ATT_SJUS` 是**伪属性**=算出来的,不是直存;SCTN 有 `ATT_JSPOSS/JSPOSE`。)
4. **外部模块也挂不进来**:`addPlug`/`getCSGTree` 虽是导出符号(ord 0x9fa / 0xde2),但唯一
   另一个被分析的模块 `core.dll` **完全不导入 Core3D**(其导入模块表无 Core3D 项;方向是
   Core3D 依赖 core.dll,不是反过来。core.dll 自带 `libgeom` + `libifcoremd`=FORTRAN 几何引擎)。
   E3D 装目录里也没有第三个几何 DLL 的 i64——只有 core.dll / Core3D.dll 两个。

**结论:** PANE 4275 / 墙(CWALL/GWALL/STWALL/WALL)/ 楼板(FLOOR/CFLOOR)/ SCTN 的几何,
经 `I*COM` 分派 → `sub_10714FC0` / `sub_10343B80` → `libgeom`(7UW4 记为「旧 FORTRAN 路」,
但 C++ 入口在 Core3D 内,是否真落到 core.dll 的 FORTRAN 例程待 P2 反编译确认)。

**对计划的影响(P2 权威改写):**

- §2.5 的 `MDR_BPanelVisualisationManager`(挂 `BPANEL`)、`MDR_WallVisualisationManager`(挂
  `CTWALL`)是 **ACC 建筑模块**的 builder,**不是**结构 PANE/CWALL 的权威。其「环拉伸 + 直属
  负几何」语义只能当**参考**。§2.2 里 BPanel/Wall/WallJoiner 的相关度从 ★★★ 降为「参考级」。
- **但环拉伸这一几何模型本身大概率仍对**:e3d-model 审核已确认它「读 PLOO/LOOP 环 + 高度 +
  挤出」与 gen-model 生产行为一致,而生产行为对标的正是 E3D 实际输出。**修正的是「权威引用」**
  (从现代 builder 改为旧路 + 伪属性 `ATT_THICKN`/`ATT_SJUS`),**不是推翻环拉伸方案**。
- P2 前置反编译清单(见 §6.2)因此新增:`sub_10714FC0` / `sub_10343B80` / 相关 `I*COM` 谓词
  / `ATT_THICKN`·`ATT_SJUS` 伪属性求值——确认结构 PANE 的厚度、对齐取法,再落 P2。
  **在这几个反编译出来之前,P2 不许照 ACC builder 的成员规则去做结构梁板。**
- **与 §1.1 不冲突,别读混了**:§2.4.1 改的是**语义权威**(该读哪些环、厚度/对齐从哪取、
  谁减谁什么顺序),**不是几何内核**。CSG **一律走 `manifold-csg`**(§1.1 已定,用户 2026-08-31
  再次确认「CSG 现在统一通过 manifold-csg 实现」),旧路 `sub_*` / `libgeom` **只当语义蓝本读,
  绝不复刻其内核实现**。反编译旧路 = 抄它的「做什么」,几何的「怎么算」永远是 manifold。

### 2.4.2 ★★ 结构 PANE 语义(旧路反编译坐实,§6.2 第 6 条 / P2 前置已结)

会话 fable-5-21(2026-08-31)续用活桥 `idalib-35724`(Core3D.dll.i64)把 §2.4.1 定的 P2 前置反编译做完,三层结论都可按地址复查:

**① 几何路由:PANE 走旧路的「复合/组件」分支,不进设计图元 switch。**

`GTGEOM`(0x10341d2e,本轮全文反编译)派发顺序:先试现代 `GTGM2`(sub_10714E60),`(结果 & 1)==0` 即 miss(§2.4.1 已证 PANE miss)→ 落 `I*COM` 谓词级联:

| 命中 | 去向 |
|---|---|
| `IPCOMP\|IHCOMP` 或码 1062572/209154074/195113669 | `sub_10714FC0` |
| `IFCOMP\|IPFCOM\|IECOMP\|ICABCO\|INCOMP` | `sub_10714FC0`(码 239044746 附加 `sub_10018CE7`) |
| `IG2COM\|IGMCOM\|INGCOM\|ICCOMP` | `CGTCT2` + `sub_10714FC0` |
| `sub_10342568` 真 | `sub_10343B80`(设计图元 switch) |
| 码 207879217 / {779182,560322,959395,790729,832210,813985,719964,856622,822719,985369} | `sub_10343B80` |
| `IGMCOM\|INGCOM\|IG2COM` 或码 644263/919825 | `sub_10714FC0`(v11=6) |
| 都不中 | 无几何(v11=-1) |

- `sub_10714FC0`(0x10714FC0)是**薄封装** = `create_geometry`(0x1071c3f0) + 应用 12 元变换(`asArray12` + 2×48B 矩阵行拷贝)。真正的复合几何在 `create_geometry`:`sub_1072C2E0` 从 `DB_Ref` 起一个几何 → `operator new(0x38)` 包一层 builder → `(*(*builder+16))(builder, elem, transform)` **递归成员**落几何段。
- `sub_10343B80`(符号 `gml_bkend/CRDESI` = Create Design)是设计图元大 switch,**只造参数化基本体**(CYLI/CONE/BOX/DISH/PYR/CTOR/RTOR/EXTR;每 case `DGETR` 读设计属性 → `GMCBOX`/`sub_10402E10`/… 造原语;`*a2` 输出 1=正 / 2=负)。**PANE 的码不在任何 case 里。**
- ∴ **PANE = 环拥有者(PLOO/PAVE),几何走 `create_geometry` 复合路的环拉伸,不是 `sub_10343B80` 的参数化图元。** 这就是「§2.4.1 说的旧路」在 PANE 上的确切落点。

> **exact I*COM 谓词 / PANE 的 db1 数字码拿不到,列为低优先残留(不阻塞 P2):** I*COM 全是 core.dll 的 FORTRAN 导入(Core3D 里只有 `__imp_IPCOMP` 这类 thunk,真身在 core.dll);且 `NOUN_PANE`(0x10ae9ff0)等 `DB_Noun` 静态镜像全 `0xffffffff`——noun→数字码由 dabacon 字典**运行时**装载,两个 DLL 静态都读不到 PANE 码。**但这不改实现**:我们走 manifold 环拉伸、不复刻 libgeom(§1.1),只需坐实「PANE 是复合环拉伸、非参数化图元」这一层——已坐实。

**② 厚度 / 对齐:都是 STRU 模块的伪属性,委派给「第一个 PLOO 成员」。**

四个 STRU pseudo-att 求值器(vtbl `0x10b3f1e4`… 已解出 method0,均按地址可复查):

- **THICKN(PANE)** = `STRU_DB_PseudoGetTHICKNonPANE::getAtt`(0x10642b70):`DB_Element::firstMember(NOUN_PLOO)` → PLOO `isOK` 则 `getAtt(ATT_HEIG)`。**厚度 = 第一个 PLOO 的 HEIG**;PANE 自身无 THICKN 存储,**且不回退**读 PANE 的 HEIG(无 PLOO 直接失败返 0)。
- **SJUS(PANE)** = `STRU_DB_PseudoGetSJUSonPANE::getAtt`(0x10642aa0):同构,`firstMember(NOUN_PLOO)` → `getAtt(ATT_SJUS)`。**对齐 = 第一个 PLOO 的 SJUS**。
- **GAREA(PANE)** 0x106429a0 = `ATT_GVOL / ATT_LOHE`;**NAREA(PANE)** 0x10642a20 = `ATT_NVOL / ATT_LOHE`。派生面积量,与网格无关(仅记档)。
- 附 SCTN:**GENSEC JSPOSS/JSPOSE** 0x10643730 比 PANE 复杂——从 `JLDATU`/`SNOD` 属主 + `ATT_POS` + WRT 限定符算截面端点位;SCTN 走这条,非主力线,P2 之外。

**③ e3d-model 口径核对(只读 `vendor/e3d-model/src/profile.rs`,未改任何源码):**

| 语义 | core.dll 权威 | e3d-model 现状 | 判定 |
|---|---|---|---|
| SJUS | 只读第一个 PLOO 的 SJUS | 只在 PLOO 上读 SJUS(profile.rs:12/86-87) | ✅ 一致 |
| 厚度 | 只读第一个 PLOO 的 HEIG,**无回退** | 第一个环 HEIG 优先,**回退 `PANE.HEIG`**(profile.rs:11/85/125) | ⚠️ 主路一致;`PANE.HEIG` 回退是 core.dll 没有的分支 |
| 首环 | `firstMember(PLOO)` 文件原序 | 第一个 PLOO,文件原序(profile.rs:7) | ✅ 一致 |

**总判:e3d-model 读 HEIG/SJUS 的口径站得住**(都从第一个 PLOO 取,与 core.dll 伪属性定义逐点一致),唯一偏差是 `PLOO.HEIG ?? PANE.HEIG` 那个 core.dll 不存在的回退(真数据里 PANE 若无自存 HEIG 则为死代码;若有且与 PLOO 不同会取错值,而 core.dll 从不看 PANE.HEIG)。**P2 落地时:去掉该回退,或明确标注为非权威兜底。** 此结论与实现线实测(4275/4275 PANE 出几何、GWALL 轮廓 17/20 exact)互证。

### 2.5 关键 CSG 语义(活桥坐实)

> ⚠️ **读下面两段前先看 §2.4.1**:这两个 builder 挂的是 `BPANEL` / `CTWALL`(ACC 建筑模块),
> **不是** ams1112 的结构 `PANE` / `CWALL`。语义本身坐实,但**适用范围已定:仅 ACC 模块**——
> 结构 noun 走旧路(§2.4.1 已坐实),下面两段对结构 PANE/CWALL 只作**几何参考**,不是权威。

**ACC 隔断板 BPANEL**(`MDR_BPanelVisualisationManager::getCSGTree` 0x109fff10,7UW4 反编译):
```
isWanted 门 → gm_SetDefaultLabel → 建板上下文 → panel = 环拉伸(sub_10A04590)
for 每个成员 m in {NBOX,NCYL,NPOLYH,NSLC,NSNO,NDIS,NCON,NPYR,NCTO,NRTO,NREV}:
    首个洞: comb = gm_CreateCombination(3)  // op=3 = 差集
            gm_AddMember(panel, gm_CreateTransform(), comb)   // 板 = 被减数
    neg = findPrimitive(m.hardType())->build(m)
    tr  = gm_CreateTransform(); m.getAtt(ATT_TRANS,&xf); gm_SetTransform(tr,xf)
    gm_AddMember(neg, tr, comb)                                // 负体 = 减数
return comb ? comb : panel
```
→ **负几何只认「直属成员」这一层,且只认上面 11 个 noun**。e3d-model 现在还额外
下钻 CMPF 深层收负体(`collect_negatives_deep`),与 core.dll 不一致,P1 要对齐或给出依据。

**manifold 落法**(按 §1.1):
```rust
let mut solid = ring_extrusion(panel)?;          // 板本体
let negs: Vec<Manifold> = direct_members(elem)
    .filter(|m| NEG_MEMBER_NOUNS.contains(m.noun()))   // 就上面 11 个
    .map(|m| build_primitive(m)?.transform(world_of(m)))
    .collect();
if !negs.is_empty() { solid = solid.difference(&Manifold::batch_union(&negs)); }
```
`gm_CreateTransform` + `gm_SetTransform` 这一对退化成「烘进 `Manifold` 的世界变换」,
不留 gm 风格的 transform 节点。

**ACC 墙 CTWALL**(`MDR_WallVisualisationManager::getCSGTree` 0x10a00340,★本轮新反编译):
```
isWanted 门 → gm_SetDefaultLabel
for 每个成员: if (type == NOUN_WLCOMP) return 0;    // ★ 有组件就整个让位,自己不出几何
建墙上下文(sub_10A03F80) → return sub_10A04590(transform)   // 与板共用同一个环拉伸本体
```
→ **容器让位规则是「有 WLCOMP 成员则自己不出几何,交给组件」,不是「墙一律是容器」**。
这条**在 ACC 墙上**成立。e3d-model 把结构 `CWALL` 一刀切成 `List` 能让 DFS 跑通,
但拿这条当依据是**跨模块套用**——`CWALL` 的正确处理要等 §2.4.1 结论。

**独立负几何**(`CSG_PrimitiveUtilities::addStandAloneNegative` 0x10726620,★本轮新反编译):
```
comb = gm_CreateCombination(3)                       // 差集
gm_AddMember(gm_CreateNull(), gm_CreateTransform(), comb)   // ★ 被减数是 Null
neg 自身变换 → gm_AddMember(a2, tr, comb)
return comb
```
→ **没有属主消费的负几何 = `Null ⊖ neg` = 空几何**,不是错误、也不是静默丢弃。
e3d-model 的 `orphans` 账本设计方向正确,产物应当是「显式的空」。

**manifold 落法**:不需要真去 `Manifold::empty().difference(neg)`,直接记 `orphans` 账
并产出空几何即可——两者结果等价,后者省一次布尔。**但账必须记**,不许当没看见。

**正图元的洞**(`CSG_TreeBuilderPrimitive::getCSGTree` 0x10726890,7UW4):
`geom = prim->build(elem)` → `addHolesBelowPrimitive(elem, geom, options)`(0x10726150)
→ owner 是 TMPL 且 TMPL 的 owner 是 FIXING 时,迭代 `addHolesBelowTemplate`(0x107263a0)。
> `addHolesBelowPrimitive` 本轮反编译出来是**两行桩**(取 att 后原样返回),与符号声明的三参
> 原型对不上,判断是 Hex-Rays 没套上 demangle 原型所致。**真身待复查**,列入 §6.1。

---

## 三、现状底账:`vendor/e3d-model` 实现审核(2026-08-31 01:10 快照)

> ⚠️ **本节是 01:10 的快照,已经严重过期**(2026-08-31 21:3x 由会话 `f40baab7` 实测标注)。
> 下表七行里至少五行不再成立,**读它之前先看这一段**:
>
> | 01:10 写的 | 21:3x 实测 |
> |---|---|
> | 「不是 git 仓库」 | 已建仓,HEAD `491d0e3`,三笔提交 |
> | 「visited 160 / generated 0,`model.obj` 0 字节」 | ams1112 实跑 visited 6059 / generated 4613(`out/ams1112-n3`,18:10) |
> | 「NPYR/NRTO/NCTO 落进 `unreachable!()` panic」 | 已修,`elmodl.rs:671` 有 `NegUnimplemented` 臂 |
> | 「23 单测没有一个碰 e3d-io;`tests/` 不存在」 | 118 项测试,含真库门 `tests/increment_real.rs`(五窗,两端全量当裁判)与 `tests/noun_coverage.rs` |
> | 「RVM 对拍脚手架空白」 | `src/bin/rvm_compare.rs` 34 KB,6 个单测 |
>
> 仍成立的两行:一号缺陷的**成因描述**(判据写两份 → DFS 截断)作为教训仍有效,
> 且这个形状后来又复发了三次(见 `491d0e3` 提交正文的四例);「站得住」那行不变。

详见 `上下文/会话-2026-08-31-e3d-model实现审核-c5121ac3.md`。摘要:

| 面 | 状态(01:10 原文,勿当现状) |
|---|---|
| 规模 | 2007 行 / 11 文件 / 23 单测;**不是 git 仓库**,未进任何 workspace |
| 实跑结果 | `out/ams1112/` = **visited 160 / generated 0**,`elements.json` 空,`model.obj` 0 字节 |
| 一号缺陷 | CWALL 判 `Catalog`、CFLOOR 判 `Unsupported` 且两者不下钻 → DFS 在 FRMW 下一层截断,30940 元素只走到 160(在飞会话已在改) |
| 会崩 | `category.rs` 新增 `NegUnimplemented` 但 `elmodl.rs` 的 `negative_world_solid` 未同步 → NPYR/NRTO/NCTO 落进 `unreachable!()` panic |
| 站得住 | 倒圆折线化 / libgm 段数规则(封顶 1000、4 的倍数)/ `restol=0.051` 让刀 / ORI 与 e3d-io 权威实现逐例对齐 |
| 测试 | 23 单测**没有一个碰 e3d-io**;`tests/` 不存在;读库四模块(pipeline/elmodl/loop_profile/world_matrix)覆盖为 0 |
| 验收 | RVM 对拍脚手架**空白** |

---

## 四、完成判据

- [ ] `e3d-model` 在 ams1112 上跑完全库:`visited + consumed == 索引全集元素数`,
      五本账把每个没出几何的元素解释干净,**零静默缺件**。
- [ ] 分类表逐条对齐 §2.3 注册表 + §2.4 plug 名单,出入处要么改齐、要么在计划里写明依据。
- [ ] 板 / 墙 / 楼板三条容器路径按 §2.5 的 core.dll 语义落地(含 WLCOMP 让位)。
- [ ] 图元族 12 个几何类按正负成对实现,负几何走 `Manifold::difference`(对应 `op=3`)。
- [ ] **RVM 对拍门**:同批元素,e3d-model 产物 vs E3D TTY 导出 RVM,逐元素
      **体积 / AABB / 连通分量数**在给定容差内一致(内核无关不变量,不逐三角对比,
      理由见 §1.1);超差元素逐条归因,不留未解释分歧。
- [ ] **实例化门**(§1.2):`--no-instance-cache` A/B 两条路产物**逐元素完全一致**;
      缓存命中率与省下的 mesh 生成次数进报告;镜像变换绕向测试绿。
- [ ] 覆盖矩阵:§2.2 的 19 个 builder 逐个写终态(已复刻 / 记账不做 / 有意省略),不留空白。
- [ ] 全部改动进 git(用户已决定「等里程碑跑通再入库」,本条随 P5 收口)。

---

## 五、阶段

### P0 — 抢修与账本闭合(最便宜,先做)

状态:**done**(2026-08-31 21:3x 由会话 `f40baab7` 回源码复核后改判;原写
`proposed`,但三条抢修其实在别的会话里已被顺手做掉,文档没跟上)。
用户 2026-08-31 01:12 已定优先级:先堵 panic + 账本闭合。
**并发约束**(已解除):`vendor/e3d-model` 当时正被会话 e9ff0ef9 实时改写;
现已建仓并落 `491d0e3`,工作树静止。

- [x] `elmodl.rs::negative_world_solid` 补 `Category::NegUnimplemented(_) => Ok(None)` 臂,
      堵掉必然触发的 `unreachable!()`。兜底一律改成 `Err` 记 `failed`,不许 panic
      (AGENTS.md 第 108 条:一个坏元素不该打崩整库)。
      **已关**:`elmodl.rs:671` 有该臂,`unreachable!` 退到 `:684` 只兜「负体收集给了
      非负类别」这种编程错。
- [x] `pipeline.rs` 收尾半成品:下钻无条件化、写入 `consumed`/`orphans`、补
      `Report::accounts_for`(doc 已引用但函数不存在)、清掉重复的 `NonGraphic` arm、
      `totals_line()` 补新字段。
      **已关**:`Report::accounts_for` 在 `pipeline.rs:577`(判据 `visited + consumed`
      == 子树元素总数),`consumed` 写在 `:454`、`orphans` 写在 `:326`。
- [x] 补账:`negative_world_solid` 的 `notes` 挂回属主;正体建模失败/跳过时其负体成员
      要留名;负体全吃掉正体按「合法全扣」而非 `failed`。
      **已关**(`elmodl.rs:675` 起的注释写明「记在属主头上」);
      「合法全扣 vs failed」这一条本次未逐例复验,若要当已证需补一条测试。
- [x] ~~查证 §2.4 的不对称 + §2.4.1 三条反证~~ **已在 2026-08-31 恢复会话里用活桥结掉**
      (§2.4 / §2.4.1)。剩 `addHolesBelowPrimitive` 真身移至 P3。

验收:ams1112 全库跑完不 panic;`accounts_for` 判据通过;报告能自证没丢子树。

### P1 — 分类表对齐 core.dll 注册表

状态:proposed。依赖 P0。

- [ ] 用 §2.3 + §2.4 重写 `category.rs` 的 `classify`:每个 noun 的归类都要能指回
      注册表里的一行(或明确标注「注册表无此项,依据 X」)。
- [ ] 消掉已知出入:**补上漏掉的 `NSLC`**(设计负图元,正体 `SLCY`);
      ~~`NSBO`/`NLCY` 若注册表确无则删或降为 `Unknown`~~ **←(7KG8 修正:此动作错误,勿执行)**
      改为:`NSCY`/`NSBO`/`NLCY` 从设计负图元**移出**,归入新增的几何集负体类别;
      `POHE`/`POLYHE` 按 e3d-io 反哈希规范名收敛。
- [ ] **`POGON` 这个 noun 不存在**(7KG8 字典实证):`category.rs` 的 `ProfileData` 名单里
      写的 `POGON` 两份字典都查无。现代 POLYHE 的面是 **`POLFAC`**(码 44236870,与 §3.7.4
      引的码逐位相符),旧式 POHE 的面是 **`POGO`**(832210)。后果不是建不出几何,是**账错了**:
      面落 `unknown`、其下 `POLOOP`/`LOOPTS` 落 `orphans`,而 `accounts_for` 是等式判据、逮不住。
- [ ] **覆盖率的分母改用旧路口径**:本节(§2.3/§2.4)是 Core3D **现代路**的名单,
      而 §2.4.1 已坐实主力 noun 全走旧路。旧路的全集见
      `.planning/2026-08-31-noun-coverage-closure/task_plan.md`:GTGEOM 的 `I*COM` 级联
      判定「该出几何」的 noun **277 个**,e3d-model 现覆盖 22 个、欠账 129 个。
- [ ] 负几何成员白名单按板 builder 的 11 个 noun 收口;CMPF 深层收负体这条与 core.dll
      不符,要么删、要么写明为什么本仓需要。
- [ ] 回归测试:注册表表驱动,新增/改动一个 noun 就当场红。

验收:分类表逐条有出处;`unknown_nouns` 在 ams1112 上为空或每条有归因。

### P2 — 容器与环拉伸(ams1112 主力,90%+ 元素量)

状态:proposed。依赖 P1 + §6.2 第 6 条(旧路反编译)。
**权威已由 §2.4.1 改写**:结构 PANE/墙/楼板走**旧路**,ACC 的 `MDR_BPanel/WallVisualisationManager`
只作参考,不是权威。开工前必须先做完 §6.2 第 6 条,否则不许照 ACC 成员规则做结构梁板。

- [x] ~~**先反编译旧路**(§6.2 第 6)~~ **已结(§2.4.2,fable-5-21)**:PANE 落**复合环拉伸旧路**
      (`create_geometry`),非 `sub_10343B80` 参数化图元;**厚度 = 第一个 PLOO 的 HEIG**(伪属性
      THICKN,非直存、非 PANE 自身)、**对齐 = 第一个 PLOO 的 SJUS**;e3d-model 口径核对通过
      (唯一偏差:`PANE.HEIG` 回退)。exact I*COM 谓词列低优先残留,不阻塞。
- [ ] 板(PANE):按旧路语义落环拉伸本体 + 直属负几何差集。**与 ACC BPanel 语义逐点对比**,
      相同就复用那份实现,不同就以旧路为准并记录差异。
- [ ] 墙(CWALL/GWALL/STWALL/WALL):确认旧路是否也有「有组件成员则让位」的规则
      (ACC 墙的 `WLCOMP` 让位是坐实的,但那是 ACC 模块;结构墙待旧路证实)。
- [ ] 楼板(FLOOR 81 / CFLOOR 15):旧路语义定完再落。`CSG_TreeBuilderFLRLAY` 仅参考。
- [ ] 环拉伸本体统一成一份实现(结构板/墙/EXTR/NXTR 共用),对齐 `D2_Profile` → 按厚度拉伸。

验收:ams1112 的 PANE/GWALL/FLOOR 全部出几何;`skipped`/`failed` 逐条有依据;
每条几何语义都能指回旧路的一个函数地址(不是指回 ACC builder)。

### P3 — 图元族补齐(12 个 `CSG_Basic*`)

状态:proposed。依赖 P1。

- [ ] 按 §2.3 逐个实现,正负共用同一份几何构建,负性只体现在 tree builder 包装
      (正体 = 直接产出 `Manifold`;负体 = 同一份 `Manifold` 交给属主做 `difference`)。
- [ ] 每个图元的尺寸属性读法以活桥反编译各自的 `build` 为准,不从旧 FORTRAN 表反推
      (旧路 §4.3 的数字码表只作交叉验证)。
- [ ] `REVO`/`NREV` 不再当里程碑外——它在 core.dll 是基础图元。
- [ ] **按 §1.2 建实例化缓存**:先按 restol 定段数 N,再拿「几何类 + 形状参数 + N」进键;
      单位体只生成一次,尺寸与摆放交给变换。`--no-instance-cache` 开关 + A/B 一致性测试
      同批落地(不要事后补,补的时候就没有干净基线了)。
- [ ] 镜像/负行列式的绕向与法线处理单独写测试。

验收:12 个几何类各有单测(体积/AABB/亏格不变量);ams1112 的 N* 负几何全部可扣;
A/B 两条路产物完全一致。

### P4 — 目录件(`CSG_TreeBuilderCat`)

状态:proposed。**硬依赖**旧计划 Phase 3(G4 目录表达式求值)。

- [ ] 反编译 `CSG_TreeBuilderCat::getCSGTree`(0x1072f5d0)定语义。
- [ ] ams1112 的 FITT/PFIT/SBFI/FIXING/CMFI(255+,跨库目录 5052)接进来。
- [ ] 表达式求值复用旧计划 Phase 3 的成果,不在本 crate 里造第二套。
- [ ] **目录件实例化**(§1.2 收益第二大块):按「目录元素 ref + 求解后的设计参数」做键去重,
      同一份目录几何在成百上千个 FITT/SBFI 实例间共享。命中率进报告。

验收:目录件出几何;`catalog_pending` 清零或逐条归因;目录实例缓存命中率可观测。

### P5 — RVM 对拍验收与收口

状态:proposed。依赖 P2(可先跑结构件子集)。

- [ ] 按 `docs/2026-08-26_e3d-tty-ams-agent-usage-guide.md` 走 TTY 宏导出 ams1112 的 RVM。
- [ ] 接 gen-model 现成的 `src/rvm/` + `src/rvm_baseline/` 对拍设施与 `test_data/` 基准,
      **不新造对拍框架**。
- [ ] 逐元素体积/AABB/连通分量差异表 + 超差归因;容差口径**先定后跑**(不许跑完再调容差),
      且容差要按 manifold 与 libgm 是两个内核这一前提定(§1.1),不设位级一致的门。
- [ ] `tests/` 用 ams1112 真语料钉住读库四模块;git 入库;覆盖矩阵写终态;CHANGELOG。

验收:RVM 对拍门全绿或超差逐条归因;矩阵无空格;代码进 git。

---

## 六、待补逆向(都用现有活桥,不新起环境)

### 6.1 P0 就要查的(会改变实现口径)

1. ~~DISH/SNOU/CTOR/RTOR/POLYHE/SLCY/EXTR/REVO 有没有 tree builder plug~~ **已结案**(§2.4:
   确无 plug,只活在 `primList_`,仅作负几何成员时经 `findPrimitive` 取用)。
2. ~~§2.4.1 三条反证(外部 DLL 注册 / `xba_base::Init` / hard-vs-actual type)~~ **已结案**(§2.4.1:
   现代路三键 miss 即回退;core.dll 不导入 Core3D;结构 noun 只被伪属性插头引用)。`xba_base::Init`
   反编译失败一项转为「无关项」——它属 xba 绘图模块(挂 IMPREF/generic-primitive/terrain-curve),
   与结构 noun 无关,不再阻塞。
3. `addHolesBelowPrimitive`(0x10726150)真身——本轮反编译仍是两行桩,疑原型未套上。留待 P3 用。

### 6.2 排在 P2/P3 前的

3. 各 `CSG_Basic*::build` 的属性读法(12 个几何类,决定尺寸语义)。
4. `sub_10A04590` 环拉伸本体的 `vtbl[52]/[64]/[40]`——2D 轮廓生成、厚度/方向取法、拉伸实现。
   (注:这是 **ACC** 板/墙的环拉伸本体,只作参考;结构 PANE 的权威在下面第 6 条的旧路。)
5. `CSG_TreeBuilderFLRLAY`(0x1090d780)、`MDR_WallJoinerVisualisationManager`(0x10a00230)。
6. ~~**★ 旧路结构几何(§2.4.1 定的 P2 前置,ams1112 主力全在这)**~~ **已结案(§2.4.2,会话 fable-5-21)**:
   - `sub_10714FC0` = 薄封装 `create_geometry`(复合递归);`sub_10343B80` = CRDESI 设计图元 switch(只参数化基本体,**PANE 不在 case 里**)→ **PANE 走复合环拉伸旧路,非参数化图元**。
   - 伪属性坐实:**THICKN(PANE) = 第一个 PLOO 的 HEIG**(0x10642b70)、**SJUS(PANE) = 第一个 PLOO 的 SJUS**(0x10642aa0);e3d-model `profile.rs` 口径核对通过(唯一偏差:`PANE.HEIG` 回退,core.dll 无)。
   - ~~**残留(低优先,不阻塞 P2)**:exact I*COM 谓词 + PANE 的 db1 数字码——I*COM 是 core.dll 的 FORTRAN 导入、`DB_Noun` 静态全 `0xffffffff`(码运行时装),两个 DLL 静态定不死;§6.4 的 NOUN→码表随之只能靠字典/运行时取,暂挂;因不复刻 libgeom,不影响实现。~~
     **★ 已结案(2026-08-31 会话 7KG8)**:`core.dll.i64` 本轮已挂上活桥
     (`idalib-41236` → `D:\ida_scratch\plant3\core.dll.i64`),I*COM 的真身读得到了。
     **22 个谓词的地址 + 各自读的 noun 字典字段号全部解出**
     (`.ida_scratch/probes/icom_field_ids.py`;谓词形状统一是
     `ATNINT(noun, &field_id, &value, &err)`,字段号是静态 dword):

     | 在 GTGEOM 级联内 | 地址 | 字段号 | 命中 noun 数 |
     |---|---|---|---|
     | `IPCOMP` | 0x53a9a10 | 602413 | 58 |
     | `IHCOMP` | 0x53a9aac | 595979 | 30 |
     | `IFCOMP` | 0x53a9974 | 600459 | 32 |
     | `IPFCOM` | 0x561c2fc | 603790 | 52 |
     | `IECOMP` | 0x561bc58 | 606263 | 32 |
     | `ICABCO` | 0x561ba84 | 591978 | 15 |
     | `INCOMP` | 0x561c1b0 | 599651 | 12 |
     | `IG2COM` | 0x561bd90 | 605428 | 5 |
     | `IGMCOM` | 0x561be2c | 604699 | 21 |
     | `INGCOM` | 0x561c260 | 600170 | 16 |
     | `ICCOMP` | 0x561bb20 | 605100 | 4 |

     不在级联内的 8 个(**这是范围边界**):`IHLCOM`(599813,**船体 85 个 noun**)、
     `IPTCOM`(604897,9)、`IPLCOM`(601036,2)、`ICOCOM`/`IFICOM`/`IJOCOM`/`IPRCOM`(各 1)、
     `IHVCOM`(591821,2.10 字典里查无此字段);`IASLCO`/`IUPCOM` 不读字段;
     `ISUBCO` 走 `DGETI` 读元素属性而非 noun 字段。
     → **船体那一大片 `H*` 不走 GTGEOM**,出现在 `unknown` 里不算本线欠账。
     PANE 的 db1 码同样已可反查(见 §6.4 第 8 条)。

### 6.3 ~~精度对齐(需另起 libgm.dll.i64)~~ —— **已注销**

用户 01:22 拍板 CSG 用 manifold(§1.1),不复刻 libgm 内核,以下两项随之取消:

6. ~~`gm_CreateCombination` 的 `GM_Operation` 枚举确切值~~ → 只需语义:已坐实 `3=差集`;
   若别的 builder 里出现新 `op` 值,在 Core3D.dll 内部按上下文读出交/并即可,不起 libgm。
7. ~~`gm_CreateFacetStructure` 的网格化算法与容差~~ → 内核换成 manifold,不追位级一致;
   精度门改为体积/AABB/连通分量这类内核无关不变量(见 §4)。

### 6.4 其它

8. ~~noun 具名枚举 `NOUN_*` → db1 hash 数字码的映射表(旧路用数字码,现代路用具名)。~~
   **已结案(2026-08-31 会话 7KG8)**:码不在 DLL 里——`DB_Noun` 静态镜像全 `0xffffffff`
   这个观察没错,但结错了对象。**映射表就是 `noun_flags.json` 的 `noun_hash` 字段**,双向可查。
   本计划里所有硬编码码已反查完:

   | 出处 | 码 → noun |
   |---|---|
   | GTGEOM `IPCOMP\|IHCOMP` 分支 | 1062572=**NOZZ**、209154074=**ELCONN**、195113669=**EQUCOM** |
   | IFCOMP 分支附加 `sub_10018CE7` | 239044746=**CTSUPP** |
   | `sub_10343B80` 设计图元 switch | 207879217=**GRIDLN**;{779182=PVOL, 560322=RPLA, 959395=DATU, 790729=GRDM, 832210=**POGO**, 813985=**POIN**, 719964=IPOI, 856622=TANP, 822719=BOUN, 985369=DRAW} |
   | `sub_10714FC0` v11=6 分支 | 644263=**PTSE**、919825=**PTSS** |
   | `cachelib/GTTUBE` | 808220=BRAN、137403155=TRUNNI、537123=LUG(与既有结论一致 ✅) |

   **两条即用推论**:① `NOZZ`/`ELCONN` 被 GTGEOM **硬编码**送进复合几何路,而 e3d-model
   判它们 `NonGraphic`(不建、且没有按 noun 的分账)——直接冲突,管嘴很可能整族漏建;
   ② `sub_10343B80` 的 case 表里还有 `POGO`/`POIN`/`DRAW`/`TANP`/`PVOL` 等 11 个 noun,
   e3d-model 一个都没认领。
9. **正负配对不必手写**(7KG8):字典字段 `positiveEquivalent`(778791)给出权威 12 对——
   NBOX→BOX、NCON→CONE、NCTO→CTOR、NCYL→CYLI、NDIS→DISH、NPOLYH→POLYHE、NPYR→PYRA、
   **NREV→EXTR**、NRTO→RTOR、NSLC→SLCY、NSNO→SNOU、NXTR→EXTR。
   ⚠️ `NREV→EXTR` 与 §2.3 注册表的 `CSG_BasicREV(REVO/NREV)` 配对冲突,两者可能不矛盾
   (「等价 noun」≠「共用 builder」),**查清前不许拿字典配对驱动回转体建法**。
9. `core.dll`(0x5)的 typed getter / exprlib(目录件尺寸若走表达式则 P4 需要)。

---

## 七、风险与依赖

- **并发写同一个 crate。** `vendor/e3d-model` 此刻正被 e9ff0ef9 实时改写,且**不在 git 里**,
  没有回滚点。任何人动手前先确认对方停手;用户已决定「等里程碑跑通再入库」,
  这段窗口的丢失风险是明知接受的。
- **活桥结论必须可复查。** 凡写进本计划的地址与语义都标了函数地址,复查一条命令的事;
  **不许把「推测」写成「坐实」**——§2.4 的不对称就是典型,已标成查证项。
- **旧 FORTRAN 表只作交叉验证。** 7UW4 §4.3 的数字码建法表是回退路,与现代路可能不同口径,
  不能拿它当现代路的实现依据。
- **RVM 对拍容差要先定后跑。** 跑完再调容差等于没有验收门。且既然内核换 manifold(§1.1),
  就别拿位级一致当门——门设在体积/AABB/连通分量。反过来说,**一旦这些不变量也对不上,
  那就是语义错了(减错东西/漏了成员),不许赖到"内核不同"头上**。
- **实例化缓存键漏一项 = 静默错几何。** 这类错不崩不报,只是某批元素形状悄悄不对,
  比 panic 难查一个量级。防线是 §1.2.3 的 A/B 一致性测试**进 CI**,不是靠 review 眼力。
  段数 N 是最容易漏的那一项(它不在元素属性里,是算出来的)。
- **manifold 的鲁棒性是新引入的风险面。** libgm 能跑通的退化输入(自交、零厚度、共面重合),
  manifold 未必;这类失败要落 `failed` 账并留元素号,不许静默出空。
- **目录件卡在 G4。** P4 不可能早于旧计划 Phase 3 完成,排期时别把它当并行项。
- **AVEVA 语料会被升级重排。** 写死偏移/refno 锚点必须允许缺席跳过。
- **会话中断风险。** 阶段结论实时写 `上下文/会话-2026-08-31-e3d-model实现审核-c5121ac3.md`。
- **§2.4.1 的两个低概率残留(不阻塞,但别当零):** ① `getCSGTree` 三键(actualType/type/hardType)
  是否有某个结构 noun 别名到已挂 noun,未逐一验证——结构 noun 各有独立伪属性注册,概率极低;
  P2 反编译旧路时若发现某结构 noun 其实进了现代 builder,以实测为准回填。② 「旧 FORTRAN 路」
  的 FORTRAN 归属沿用 7UW4 说法,C++ 入口(`sub_10343B80`)是否真落到 core.dll 的 FORTRAN
  例程,待 P2 反编译坐实,别在文档里把它当已证。

## 批注处置记录

| 轮次 | 批注 | 处置 |
|---|---|---|
| 2026-08-31 01:22 · plannotator `annotated` | 「CSG 的部分,我们可以使用 manifold 来代替」 | 新增 §1.1 内核边界(core.dll 管语义 / manifold 管几何);§2.1 加内核不复刻说明;§2.5 板挖洞、独立负几何两处补 manifold 落法;§4 验收门从「体积/AABB」改为「体积/AABB/连通分量」并写明内核无关理由;§6.3 两条 libgm 逆向需求**注销**;P3 图元正负实现改述、P5 容差口径补前提;§7 新增 manifold 鲁棒性风险条 |
| 2026-08-31 01:28 · plannotator `annotated` | 「还要设计一套,可以复用基本体的方案,可以方便减少 mesh 的生成,通过 transform 来缩放已有的 mesh」 | 新增 §1.2 图元实例化(缓存键分「形状参数 / 尺寸」表、段数 N 必须进键的坑、收益分布、正确性钉法);**收窄 §1.1** 的「不留 transform 节点」为「只在要做布尔时才烘平」并说明这更贴 core.dll 的 `gm_AddMember(geom, transform, comb)` 原结构;§4 加实例化门;P3 加缓存实现 + `--no-instance-cache` A/B + 镜像绕向测试;P4 加目录件实例去重;§7 加缓存键漏项风险条 |
| 2026-08-31 · plannotator `approved`(r3,`.gate-result-r3.json`) | 无批注,门禁通过 | 计划状态 draft → approved |
| 2026-08-31 恢复会话 2e34fff3(门禁后) | 无批注;续用活桥把 §2.4.1「阻断项」结掉 | 用活桥反编译 `getCSGTree`/`GTGM2`/`GTGEOM` + 枚举结构 noun 的引用 + 扫 core.dll 导入表,**坐实**结构 noun 走旧路(§2.4.1 从「存疑」改「坐实」);连带改:§2.1 旁注、§2.5 适用范围、§6.1 结案两项、§6.2 新增旧路反编译清单、P0 勾掉查证、P2 权威改写为旧路。**此修正在门禁通过之后落的,若需重过门请拍板。** |
| 2026-08-31 会话 fable-5-21(门禁后) | 无批注;续用活桥做完 §2.4.1 定的 P2 前置(旧路 PANE 语义) | 反编译 `GTGEOM`/`sub_10714FC0`/`create_geometry`/`sub_10343B80` + `STRU_DB_PseudoGet{THICKN,SJUS,GAREA,NAREA}onPANE` + `GENSEC JSPOS`,**坐实**:PANE 走复合环拉伸旧路、**厚度 = 首 PLOO 的 HEIG**、**对齐 = 首 PLOO 的 SJUS**;核对 e3d-model `profile.rs` 口径通过(仅 `PANE.HEIG` 回退偏差)。新增 §2.4.2、§6.2 第 6 结案、P2 首项勾掉。exact I*COM 谓词 / PANE db1 码列低优先残留,不改技术方向、无需重过门。 |
| 2026-08-31 会话 7KG8(门禁后) | 无批注;用户令「结合 ida-bridge 分析 NOUN 覆盖是否完毕」,答复后拍板「现在就回填」 | `core.dll.i64` 首次挂上活桥,解出 22 个 `I*COM` 的地址与字段号 → 从 dabacon 字典导出全部 1384 个 noun 的家族位矩阵。**回填三处**:① §2.3 结论修正——「`NSBO`/`NLCY` 根本不存在」是错的(它们属几何集家族 INGCOM),连带 **P1 的动作「删或降为 Unknown」作废**(照做会抹掉 42 个 noun 的一整族),并补 `NSLC` 漏项与 `POGON`→`POLFAC` 笔误;② §6.4 第 8 条结案——NOUN→db1 码表就是 `noun_flags.json` 的 `noun_hash`,本计划所有硬编码码已反查完,新增第 9 条(字典的 `positiveEquivalent` 权威 12 对);③ §6.2 第 6 条的「exact I*COM 残留」结案,并划出范围边界(船体 85 个 noun 不走 GTGEOM)。**新增覆盖率分母**:旧路口径 277 个该建 / 现覆盖 22 / 欠账 129,明细见 `.planning/2026-08-31-noun-coverage-closure/task_plan.md`。**本次修正在门禁通过之后落,①改变了 P1 的一个既定动作,若需重过门请拍板。** |

## Errors Encountered

| Error | Attempt | Resolution |
|-------|---------|------------|
| `pseudocode` 表查询报 `no such column: code/address/ea` | 按常见列名试 | 表列名是 `n/type/func_ea/line/ea/placement/…`,且**必须**带 `WHERE func_ea=` 或 `ea=`;`names` 表列名是 `address` |
| `addHolesBelowPrimitive` 反编译出两行桩 | 直接按地址反编译 | 疑似 Hex-Rays 未套 demangle 原型;列入 §6 待复查,不据此下结论 |
| 全函数反编译取 `src` | — | `SELECT group_concat(line,char(10)) AS src FROM pseudocode WHERE func_ea=0x…` |
| xref 某全局的所有引用函数 | — | `xrefs(from_ea,to_ea,from_func,type,type_name,is_code)`;按 `to_ea=<全局地址>` 查,`from_func` LEFT JOIN `names.address` 取名 |
| 判断某符号是否导出 / 被谁导入 | — | 导出查 `entries(ordinal,address,name)`;导入查 `imports(address,module,name,ordinal)`;`funcs` 主键列是 `start_ea`(不是 `func_ea`) |
| 扫另一个 DLL(未连) | — | `ida-bridge exec-idb --idb <other.i64> --sql "…"` 一次性起停;core.dll.i64(421MB)载入约数秒 |
| 读 `DB_Noun`(NOUN_PANE 等)取数字码 | IDAPython `idc.get_wide_dword` | 静态镜像全 `0xffffffff`——noun→码由 dabacon 字典运行时装,两个 DLL 静态都读不到 |
| 反编译 `I*COM` 谓词 | 按名反编译 | Core3D 里只是 `__imp_*` 导入 thunk,真身在 core.dll(FORTRAN);要 `core.dll.i64` 才有本体 |
| xref 扫某 vtbl 数据区取虚函数指针 | `xrefs WHERE from_ea BETWEEN …` | 全表扫描慢(单次 40~60s,会超默认 60s 超时);改用 IDAPython `idc.get_wide_dword(vtbl+4*i)` 直读槽,秒回 |


## 2026-08-31 ida-bridge 实施进度（实测账本）

> 本节只记录已经运行并有产物的结果；“已分类”不计入“已建”。

- **字典基线**：已用 E3D 3.1 `attlib.dat`（5,840,896 bytes）重算 327 个级联 NOUN。19 家族口径下，动态分类已做到 `unknown_nouns=0`；其中 27 个进入已有 builder，300 个仍为 `EvidencePending`，不能算几何覆盖。
- **route member 分区**：2.10 为 55 IPCOMP + 30 IHCOMP + 7 个 2.10 缺失 + 12 无家族位；3.1 中这 7 个已进入 IPCOMP，因此为 62 IPCOMP + 30 IHCOMP + 12 无家族位。5 个 route container 仍独立记账。
- **IDA 类 28**：已确认 `GTGEOM -> sub_10714FC0 -> create_geometry`，以及 `sub_107189A0` 将 `NOZZ/ELCONN/EQUCOM` 映射为 28、IPCOMP/IHCOMP 映射为 29/30。`sub_107210A0` 会按成员顺序创建并串接 member builder。AMS8000 的 34 个 ELCONN 均无成员，故目前保持 `EvidencePending`，没有冒充已建。
- **AMS1112（sesno 722）**：索引 30,940；`visited=6059`、`consumed=24881`、`unknown=0`、`orphans=0`、`failed=0`。实际生成 4,613（PANE 4,275、GWALL 120、FLOOR 81、SBFI 137）。14 组 `WALL -> SPINE -> CURVE` 已按属主语义消费；目录待实现 637（其中短 DESP 的 SBFI 3 个已从错误账移入目录待实现账）。
- **AMS8000（sesno 264）**：索引 6,605；`visited=6088`、`consumed=517`、`unknown=0`、`orphans=0`、`failed=0`。3,172 个管身槽位中实际建成 562，零长度终态 2,590，缺 P 点 19，另有 target-not-found 8、unbuildable 1。逐元素缓存开/关结果完全一致（715 元素，规范 JSON SHA-256 `60f6ae4629259950f8c8ead91c47bcf1a05f1ed5c0726a4ef5cea0ffd2089094`）。
- **TLEN/PTCD**：PTCD 已按 Core `DORTXT` 的轴链 + PML 角表达式实现；`FTUB + RPRO TLEN` 仅在该 noun/key 组合下映射到实例 HEIG，依据旧 E3D/RVM 对拍中 `PXLE=TLEN=1500` 且实例 `HEIG=1500`。启用 TLEN 后缺 P 点由 1,545 降到 19，实建管身由 329 增到 562。
- **仍未过门**：AMS1112 的 637 个目录 builder、AMS8000 的 550 个目录/路由构件实体、8 个跨会话目录引用、RVM facet 的 only-baseline/only-generated 与体积/连通分量门，以及真实增量回归 721→722 少删除 42 个几何。上述项目仍是待实现/待对拍，不计入完成。
