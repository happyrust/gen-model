# 会话上下文 — 2026-08-31 · e3d-io 数据接口 + core.dll 方案另立生成算法（ams1112 试点）

> 本会话：BajieAsk-agent-1-e9ff0ef9。00:05 收到任务。
> 前序档案（同目录，已通读）：
> - 《会话-2026-08-30-接力DR6W-9DOL.md》（最近一手：CRFA 修法 A 落地、ISSUE-027、Phase 2 第一项）
> - 《会话-2026-08-30-接力AJXG-DR6W.md》（S1 DbElement 门面落 e3d-io、7333 对拍归因）
> - 《会话-2026-08-30-模型生成core-dll全流程与数据接口.md》（Core3D 管线 + 数据接口清单）
> - 对标矩阵 `docs/plans/2026-08-30-core-dll-api-alignment.md`
> - 既有计划 `.planning/2026-08-30-direct-read-model-generation/task_plan.md`

## 任务（用户 00:05 原话拆解）

「提供 e3d-io 的数据接口，用来直接生成模型，按 ams 1112 测试。
模型生成的算法相当于**另外一套**，按照 core.dll 里提供的模型生成方案。」

两部分：
1. **e3d-io 数据接口**：补齐/固化模型生成要吃的读侧接口（S1 DbElement 门面已在，缺口见对标矩阵 ❌/🔶 格）。
2. **生成算法另立一套**：不复用 gen-model `fast_model`，按 core.dll（Core3D 管线）方案新写：
   ADDDES 树遍历 → MODCMP noun 分类 → ELMODL 逐元素建模 → GTGEOM 取几何 → 负几何 CSG（libgm 语义）→ 变换 → 产物。
3. 试点语料 **ams1112**。

## 关键事实（本会话 00:1x 实测/核实）

### 与既有计划的关系（重要口径变化）

`.planning/2026-08-30-direct-read-model-generation/task_plan.md` 的前提是
「新增一种**数据源**，生成算法本身（fast_model/resolve/cata_model/prim_model/loop_model）**不重写**」。
本任务口径**升级**：算法另立一套（按 core.dll 方案）。既有计划的 Phase 2（查询面直读）成果可直接复用，
Phase 4「试点」被本任务替代/超越。**不改旧计划文档，新开一条线，避免和在飞 agent 撞。**

### ams1112 是什么（SurrealDB 8009 实查，dbnum=1112，共 ~31k 活元素）

船体结构/舾装设计库（DESI），noun 分布主力：

| noun | 数量 | 语义 |
|---|---:|---|
| PAVE | 18521 | 板边界环顶点 |
| PLOO | 4477 | 板边界环（loop） |
| PANE | 4275 | 板（panel，拉伸主体） |
| VERT | 1135 | 型材顶点 |
| FIXING/SBFI/CMPF/FITT/PFIT | 255/140/137/86/200 | 固定件/目录件（跨库 5052 目录） |
| GWALL/CWALL/STWALL/WALL/FLOOR/CFLOOR | 121/121/76/14/81/15 | 墙/楼板 |
| NXTR/NBOX/NCYL/NPYR/NREV/NRTO/NCTO | 210/181/60/15/2/8/4 | **负几何**基本体 |
| LOOP/SPINE/CURVE | 212/14/14 | 环/脊线/曲线（型材路径） |
| JLDATU/PLDATU | 252/252 | 基准 |
| SITE/ZONE/STRU/FRMW | 1/3/6/12 | 树骨架 |

→ PANE(PLOO/PAVE 环拉伸) + N* 负几何扣减这一条链覆盖 1112 的 90%+ 元素量，
正好压 core.dll 管线的全部要素（遍历/分类/几何/负几何/变换）。

- 语料文件 `D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams1112_0001`（20 MB，
  **00:02:28 刚被写过**——E3D/别的进程在动它，开库必须按水位 pin sesno + FileIdentity 守卫）。
- 目录库 ams5052（159MB）在盘；DbOption-ams.toml 历史上就用 manual_db_nums=[1112] 测试。
- SurrealDB 8009 活着（rocksdb:.surreal/ams-rvm-rebuild-20260824），dbnum1112 数据在库，可做双跑对拍参照。

### e3d-io 现状（数据层，对标 core.dll 0x5）

- 路径 `d:\work\plant-code\old\vendor\e3d-io`（gen-model Cargo.toml:117 path 依赖；伴生 `../e3d-attlib`）。
- HEAD `6bea669`（S1 已提交）：`db_element.rs` = DbSet（按 dbno 池化）+ DbElement（惰性句柄）
  + MemberCursor（NXTITM 语义原序）+ DbFileResolver（跨库补开）+ typed getter（按 DescriptorValue 投影）
  + find_named。lib 174 绿 + 门面集成 6 绿。
- 引擎：`ReadOnlyEngine::open_at(path, sesno)` 时点 pin；`scan_elements`；index/diff（t-327）。
- 已知缺口（对标矩阵 ❌ 格，与本任务相关的）：World 定位门面、owner 链 world transform 收口、
  qualifier/UDA 面、G4 目录表达式求值（Phase 3 未做——FITT/SBFI 等目录件要吃）。

### core.dll 生成方案（结论，见 0830 全流程档案 + teach/0009/0011）

```
ADDDES(树DFS, LISTOP/LPRMTV 分流) → MODCMP(I*COM 谓词按 noun 分类)
→ ELMODL(逐元素: 表示旗标→DRAWOP→GTGEOM 取几何→LNEGIT 负几何→MAT16 变换→落图段)
→ libgm(gm_CreateNormalisedItem / gm_CreateCombination=CSG / gm_CreateFacetStructure=网格化)
```

Rust 侧几何内核可用现成依赖：manifold-csg（ADR-029，gen-model 在用）+ glam 0.29 + parry。

### vendor 现状

vendor/ 下无任何生成算法 crate（只有 e3d-io / e3d-attlib / old-* 存量）。新算法 crate 需新建。

## 决策点（已发决策卡 e9ff-genplan，等用户拍板）

1. **新算法落点**：A 新建 crate（`vendor/e3d-model`，依赖 e3d-io + manifold-csg，镜像
   core.dll/Core3D/libgm 模块边界，推荐）｜B gen-model 内新模块（复用 mesh 设施但易与 DB 栈纠缠）｜
   C 放 e3d-io（不推荐，破坏读库 crate 边界）。
2. **首里程碑范围**：A 骨架+结构最小闭环（遍历/分类 + PANE 环拉伸 + N* 负几何 CSG + 变换 + mesh 输出，推荐）｜
   B 只做遍历+分类（不出几何）｜C 全 noun 一次到位（含目录件，压 G4，风险大）。
3. **验收方式**：A 与 fast_model DB 模式双跑对拍（AABB/体积/变换等不比拓扑）+ OBJ 抽查目验（推荐）｜
   B 只 OBJ/glTF 目验。

## 用户拍板（00:14，决策卡 e9ff-genplan）

- q1 = **A 新建 `vendor/e3d-model` crate**（依赖 e3d-io + manifold-csg + glam）。
- q2 = **A 骨架+结构最小闭环**（DFS 遍历 + noun 分类 + PANE 环拉伸 + N* 负几何 CSG + 世界变换 + mesh 输出）。
- q3 = **自由输入：验收走 E3D TTY 导出 RVM 来对比验证**（不是 fast_model 双跑对拍）。
  → 参照 AGENTS.md：先读 `docs/2026-08-26_e3d-tty-ams-agent-usage-guide.md`；
  RVM 几何-only 走 TTY 宏；仓内有 `src/rvm/` + `src/rvm_baseline/` 对拍设施 + `test_data/` RVM 基准。

## 工作日志

- 00:05 收任务；通读 4 份前序档案 + 对标矩阵 + task_plan。
- 00:1x 实测：dbnum1112 noun 分布（31k 元素，PANE/PLOO/PAVE 主力）；ams1112 文件 00:02 刚被写过；
  vendor 无生成 crate；gen-model 几何依赖 manifold-csg(ADR-029)/glam029/parry；teach 0009/0011 在。
- 00:1x 建本档案；发决策卡（落点/范围/验收三问）。
- 00:14 用户拍板 A/A/RVM 对比。开工：先读 teach/0009、0011、TTY 指南、现有 PANE 生成逻辑与 RVM 对拍设施。
- 00:27~00:45 建 `vendor/e3d-model`：8 个模块 + `bin/gen_ams`，`cargo check` 过。**未跑单测、未跑真数据**。
- 00:57 会话交接（本档案自 00:14 起未更新，下段由接力会话补记）。

---

# 接力段（会话 BajieAsk-agent-1-cde5ad2b，00:57 接手）

## 接手时的实际状态

`vendor/e3d-model` 已成型（category / discretise / elmodl / mesh / pipeline / profile / solid /
transform + `bin/gen_ams`），`cargo check --all-targets` 绿。但**单测没跑过、真数据没跑过**，
交接前的「完成」只到编译通过这一层。

## 本段做完的事

### 1. 单测从 17/18 修到全绿，段数规则改挂权威复刻

`solid::cylinder_is_centered_on_origin` 首跑即红：断言 `|V − πr²h|/πr²h < 5e-3`，
而 R=100 按 0.5mm 弦高容差分 32 段，内接 32 边形比真圆少 **0.64%**——这条断言算术上就不成立。

顺带发现更要紧的一处：e3d-model 自己另写了一套分段规则，带 `clamp(8, 256)` 与
`MAX_ARC_SEGMENTS = 64` 两个上限，而 gen-model `fast_model/libgm_discretise.rs` 有
`d2_numberOfSegmentsForCircle` 的**逐位复刻**，上限是 libgm 的 1000。R=23400（RM13 穹顶那量级的圆）
权威值 484 段，旧规则会静默截到 256。

已改：
- `discretise::circle_segments_uncapped` = libgm 公式（度制、步长封顶 45°、向上取 4 的倍数，不封顶）；
- `solid::cylinder_segments` = 上式 + `MAX_SEGMENTS = 1000`（曲面原语各自封顶那一支）；
- 倒角弧段数改成**按整圈段数等比例缩**（`d2_numberOfSegmentsForPartRev` 口径），不再拿扫角除步长；
- 新增测试钉 libgm 对照表 `(1,8) (25,16) (100,32) (250,52) (3000,176) (23400,484)`，与 gen-model 同一组数；
- 圆柱体积改成断言**内接多边形棱柱**的精确值 + 0.64% 缺口区间，不再拿 πr²h 配模糊容差。

**仍与 E3D 逐位不同的一处（已记进模块文档）**：libgm 把弧点铺在全局角度格子上（相位锁死），
本 crate 是把扫角均分。段数一致、相位差半格；比不变量看不出来，要逐面全等才需要移植
gen-model 的 `span_polyline_in_steps`。

### 2. 真数据首跑暴露的主缺陷：DFS 在 CWALL/CFLOOR 整片截断

首跑 ams1112：`visited=160 generated=0`。全库 30940 个元素只走到 160 个，
而报告一片绿——只有一行「catalog: CWALL 121」。

写 `examples/tree_census.rs`（索引侧普查：终极祖先 + 可达性 + 父子 noun 边）定位到真因：

```
WORL → SITE → ZONE → STRU → FRMW → CWALL(121) / CFLOOR(15)
CWALL → PANE 1995 / GWALL 122 / STWALL 76 / WALL 14 / PNOD 1
CFLOOR → PANE 2280 / FLOOR 81
PANE  → PLOO 4275 → PAVE 18521
```

**CWALL / CFLOOR 是容器**：aios-core 的两张权威名单里都没有它们
（`GNERAL_LOOP_OWNER_NOUN_NAMES` 不含 ⇒ 不带 PLOO；`USE_CATE_NOUN_NAMES` 不含 ⇒ 不吃目录），
而 `generation_root` 把它们当**生成根**。分类表却把 CWALL 判成目录件、CFLOOR 判成未实现项，
两者都不下钻，4275 个 PANE 一个都没进管线。1+1+3+6+12+121+15+1 = 160，分毫不差。

改法不止于改这两个 noun 的分类——根因是**「本元素不建模」被当成了「不下钻」**：

- `Category::consumes_members()`：只有产正实体的元素消费成员（自己的剖面环 + 负体）；
- 遍历改成**无条件下钻**，只有被本元素消费的成员不入栈；
- `CWALL`/`CFLOOR` → `List`；
- `NPYR/NRTO/NCTO/NSBO/NCON/NSNO/NDIS/NSCY/NLCY` 从 `Unsupported` 改成新的
  `NegUnimplemented`——它们是负体，账要记在**属主**头上（否则只得到一行「NPYR: 15」，
  看不出是哪 15 个 FLOOR 少挖了刀）；
- `Report` 加 `consumed`（被消费子树的元素数）与 `orphans`（没人消费的剖面/负体）；
- `Report::accounts_for(tree_total)`：`visited + consumed` 必须等于**索引侧独立数出来**的总数。

`accounts_for` 立刻抓到第二个漏子：`gen_ams` 的合并函数没并 `consumed`/`orphans` 两本新账。
合并逻辑因此从调用方挪进 `Report::merge`，用解构赋值让「加了字段忘了并」编译期就露头。

### 3. ams1112 全库跑通，账已平

```
索引 30940 个元素，扫到 2 个根: [17496/0(WORL /*), 17496/1(MNUM /**)]
visited=6115 consumed=24825 generated=4476 skipped=0 failed=2
catalog=774 unsupported=0 unknown=1 orphans=28 neg_skipped=31
按 noun 生成: {FLOOR: 81, GWALL: 120, PANE: 4275}
账已平：visited + consumed = 索引 30940            用时 905 ms
```

产物 `vendor/e3d-model/out/ams1112/`：`model.obj`(4.9MB) / `elements.json`(1.8MB) / `report.json`。

逐项归因（每一条都有名有姓）：

| 账 | 数 | 内容 |
|---|---:|---|
| generated | 4476 | PANE 4275/4275、FLOOR 81/81、GWALL 120/122 |
| failed | 2 | GWALL `17496/106456` 无任何 PLOO；`17496/117236` 的环 0 有 0 个顶点 |
| catalog_pending | 774 | FIXING 255 / PFIT 200 / SBFI 140 / FITT 86 / STWALL 76 / WALL 14 / CMFI 3（二期 G4） |
| orphans | 28 | SPINE 14 + CURVE 14 —— 属主 WALL 是目录扫掠件，本期不建，故其脊线无人消费 |
| unknown | 1 | MNUM（`/**` 那个库务根，无几何；分类表待补一条 NonGraphic） |
| neg_skipped | 31 | NPYR 15 / NRTO 8 / NCTO 4 / NREV 2 / NXTR 2，全部点名到属主 refno |
| consumed | 24825 | PLOO 22998（4477 PLOO + 18521 PAVE）+ NXTR 1547 + NBOX 181 + NCYL 60 + NREV 12 + NPYR 15 + NRTO 8 + NCTO 4 |

几何不变量：体积无非正值，min 310,000 mm³ / max 819 m³ / 合计 4569 m³；三角总数 97,028；
整体 AABB X[−23300, 23300] Y[−23300, 23300] Z[−2595, 13780]。
**待查**：21 件亏格为负（−1/−2/−3/−9），即布尔把实体切成了 2~10 个壳，
是共壁负体让开量不够的老病（gen-model RM13 穹顶同款），占 4476 件的 0.5%。

### 4. RVM 验收资产已盘清（尚未对拍）

仓内已有 **ams1112 的 TTY 导出基准**：`test_data/rvm/1RS-WF03-W-C-RR001.rvm` + `.rvm.json`
（CWALL `/1RS-WF03-W-C-RR001`，299 成员 / 449 primitive，`export_scope=narrow`，
无 ATT 故 `unresolved=298`，按序号配对）。其成员构成：**GWALL 20** / WALL 4 / STWALL 4 /
FIXING 56 / PLDATUM 56 / SBFITTING 47 / SPINE 4，**没有 PANE**。

含义：这份现成基准能立刻对拍 20 件 GWALL，但盖不到里程碑主力的 4275 件 PANE；
要盖 PANE 得按 `docs/2026-08-26_e3d-tty-ams-agent-usage-guide.md` §11 重新起 E3D 导一份
CFLOOR/CWALL 的 RVM。**此处需用户拍板，已发决策卡。**

## 本段改动的文件

- `vendor/e3d-model/src/category.rs`：容器分类、`NegUnimplemented`、`consumes_members`、4 条新测试
- `vendor/e3d-model/src/pipeline.rs`：无条件下钻、`consumed`/`orphans`、`accounts_for`、`Report::merge`
- `vendor/e3d-model/src/elmodl.rs`：`NegativeSolid` 枚举，负体跳过理由具体化
- `vendor/e3d-model/src/discretise.rs`：libgm 分段公式、等比例弧段数
- `vendor/e3d-model/src/solid.rs`：`MAX_SEGMENTS=1000`、libgm 对照表测试、内接棱柱体积测试
- `vendor/e3d-model/src/bin/gen_ams.rs`：`scan_index` 普查、账目自检、去掉本地合并函数
- `vendor/e3d-model/examples/tree_census.rs`：**新增**索引侧普查探针

单测 23 条全绿（`cargo test`）。

---

# 补记段（cde5ad2b 断线未落盘的最后一程；由恢复会话 fable-5-7 于 02:4x 依据
# `.session-restore/conversation-73c37445.md`（370 条全量提取）补记）

## 用户拍板（01:18，决策卡 cde5-rvm-verify）

- Q1 RVM 对拍路线 = **两步走**：先拿现成 GWALL 基准跑通对拍，再起 E3D 导含 PANE 的 RVM。
- Q2 = **先补齐负体再对拍**（NPYR 15 / NRTO 8 / NCTO 4 + NREV 2）。
- Q3 = 21 件亏格为负**先挂账**，等对拍指认再查。

## 5. 负基本体补齐（29 件全落地）

- `solid.rs` 新增：`pyramid_solid`（八角点**凸包**，XOFF/YOFF 上下各半，语义对齐 aios-core
  `prim_geo/pyramid.rs`）、`rect_torus_solid` / `circular_torus_solid` / `revolve_rings`
  （manifold `Revolve`：截面在 XZ 半平面、绕局部 Z、起 +X 转向 +Y；负扫角=按 |角| 建再绕 Z 反转）。
- 段数规则：环向 `part_rev_segments` 按整圈段数**等比例缩**（`d2_numberOfSegmentsForPartRev` 口径），
  管向 `circle_segments`，均封顶 1000。
- 单测新增解析体积钉法（内接楔形和公式、全转=矩形环互证等），**32 条全绿**。
- `category.rs`：NPYR/NRTO/NCTO 归负基本体、NREV 归 `NegRevolution`；`elmodl.rs` 接线。
- 重跑 ams1112：`neg_skipped` **31 → 2**（剩 2 件 NXTR 是真坏数据：`17496/118912` 截面填充为空、
  `17496/116867` 环 0 有 0 顶点——与 failed 的 2 件 GWALL 同源）；负体应用 478 处；
  总体积 4569 → **4561.035 m³**（挖掉 ~8 m³）；三角 100280；亏格<0 仍 21 件；**账仍平**。

## 6. RVM 对拍开打（1RS-WF03-W-C-RR001 的 20 件 GWALL）——断线时的战场

工具落地：
- `gen_ams` 的 `elements.json` 增**配对键**：owner 显示名 + noun 前 4 字 + 同 noun 成员文件原序 ordinal
  （即 RVM 侧「GWALL n of CWALL /名字」的 n）。
- 新 `src/bin/rvm_compare.rs`（读 `.rvm.json` 快照按键配对、比 AABB）；
  新 `examples/element_probe.rs`（单元素全属性/owner 链探针）、`examples/loop_dump.rs`（整批环导出）。

已坐实的（这些**不是**问题）：
- **配对正确**：最优指派 ≈ 恒等映射（仅 base 6↔7 一对互换）；20 件 refno 集合与 gen-model
  `mesh_compare.rs` 基准清单完全一致 ⇒ **排除数据版本差**（sesno 720 与 722 的 PAVE 也相同）。
- **摆位正确**：20/20 旋转矩阵逐位相等；平移 16/20 相等，4 件 SJUS=TOP 的差恰为 HEIG
  （RVM 的 geom translation 已含下移，两侧语义一致）；**Z 跨度 20/20 精确相等**。

未解主问题（断线点）：
- **XY 轮廓 0/20 落进 0.5mm**：最大轴差 23.3mm（ord10）～ 5372.8mm（ord19）。
- 基准侧面片远多于我方：如 ord3 = 195 polygons / 235 contours / 1218 verts，我方单环 29 顶点；
  contours>polygons ⇒ 基准剖面**带洞**。ord5（三角墙）基准比我方**局部 Y 镜像**且每边外扩 ~2mm
  （mirror-y 残差 1.737mm）；但整批 as-is / mirror-x / mirror-y / rot180 假设**全部否定**（0/20 命中）。
- 断线时结论：**差异在轮廓形状本身**——E3D 导出的 GWALL 剖面不是库里 PLOO/PAVE 环的原样拉伸。
- **头号嫌疑（待验证）**：墙拼接。新计划 §6.2 恰列
  `MDR_WallJoinerVisualisationManager`（0x10a00230）待——GWALL 端头被邻墙裁剪/延伸 +
  开洞进轮廓，能同时解释「镜像样残差 + 每边毫米级外扩 + 百~千 mm 级外形差 + 面片暴涨」。
- 下一步候选：① 活桥反编译 wall joiner + 直读 `.rvm` 二进制把 facet 顶点逐件对到 PAVE 环上坐实根因；
  ② 按两步走第二步，先导含 PANE 的 RVM（4275 件主力不吃墙拼接，验收门可先打绿）。

## 并行线（本段时间窗内他会话的产出，恢复时已核对）

- 计划线：`.planning/2026-08-31-core-aligned-model-generation/task_plan.md` 已 **plannotator
  gate approved（r3）**——§1.1 内核边界（core.dll 管语义 / manifold 管几何，不复刻 libgm）、
  §1.2 图元实例化（段数 N 必须进缓存键）、P0~P5 阶段表。
- 审核线：`上下文/会话-2026-08-31-e3d-model实现审核-c5121ac3.md`（01:10 实现审核）。
- 风险原样在：**`vendor/e3d-model` 仍不在 git**（用户定的是里程碑跑通再入库）。

---

# 补记段 2（fable-5-9 接 fable-5-7 的 GWALL 取证；推翻断线结论）

## GWALL 轮廓差异根因已坐实——环拉伸方案是对的，断线的「0/20」是判据坐标系错

用 `tools/analyze_gwall_profiles.py`（新增）把 `rvm-facets-RR001.json`（E3D 20 件 GWALL 的
facet 世界顶点）用**我方 world 矩阵的逆**转回墙局部系，逐件与 `loops-RR001-gwall.json`
（我方环）比局部 XY：

- **18/20 件端头 Δx ≈ 0，17/20 件边 Δy ≈ 0** —— 局部轮廓精确吻合。
- 断线时 `rvm_compare` 报「XY 0/20 命中」是因为它比的是**世界系 AABB**，而墙绕 Z 转了角度
  （ori 如 −174.99°），局部 x/y 转到世界后 AABB 的 X/Y 已不对应局部轴。**是判据用错坐标系，
  不是轮廓错**。头号嫌疑「墙拼接改所有轮廓」被否定。

真实差异三分（每类都有名有姓）：

1. **Z-SJUS：已正确实现，无需修（纠正本段上一版的误报）**。`elmodl.rs:170-176` 里 GWALL
   在 `SJUS_NOUNS`（PANE/FLOOR/GWALL）名单内，挤出后已 `translate(0,0,-sjus_drop)`。
   先前「5 件 Δz=-sjus_drop」是**取证脚本的 bug**：`loop_dump` 的 world 矩阵不含这次下移
   （下移是在挤出实体上单独做的），拿它转 E3D facet 才看到假偏移。把脚本的我方 z 基准改成
   `[-sjus_drop, h-sjus_drop]` 后，5 件 Δz 全部归零。**切勿再去 elmodl 减 sjus_drop——会双重下移。**
2. **开洞（6 件带内环）**：ord3(40 洞)/ord4(20)/ord15(13)/ord18(16)/ord19(7)/ord2(2)，
   E3D 剖面 `contour>polygon`（外环+内环），我方单外环无洞。外轮廓 Δ≈0，多出的面片全是**墙上开口**
   （门窗/PANE 洞），非轮廓外扩——「面片暴涨」的真因。开口来源待查（负几何 opening / PANE 挖洞）。
3. **端头拼接（仅 3 件）**：ord13(`17496/116569`：端头 +20.6/−147.8、边 −79.2)、
   ord14(`17496/116549`：边 −157.9)、ord19(`17496/105880`：端头 +11.8)。这才是真正的
   wall-joiner 效应（端头被邻墙裁剪/延伸），范围远小于断线时以为的 20 件。

## 连带发现：`rvm_compare` 判据本身要改

世界系 AABB 判据对**旋转墙**无意义（旋转把局部 x/y 摊进世界 X/Y/对角线）。应改为：
先按元素 world 矩阵把两侧顶点归到同一局部系再比，或比顶点集的 Hausdorff/逐点距离。
断线的整场「0/20 焦虑」根子在这里。

## 局部系判据重跑结果（`analyze_gwall_profiles.py`，含 SJUS 下移修正）

**17/20 件 exact**（外轮廓 AABB 逐轴 <0.05mm），**Z 仍偏 0 件**，off 仅 3 件：
ord13 `17496/116569`(147.8mm) / ord14 `17496/116549`(157.9mm) / ord19 `17496/105880`(11.8mm)。
开洞件（ord3=40 洞等）外轮廓 exact——洞在内环，不进外包围盒。**环拉伸 + SJUS 全对，
真实差异只剩这 3 件端头拼接。** 断线的「20 件全 off / 疑墙拼接改所有轮廓」彻底推翻。

## 下一步候选

1. IDA 坐实 3 件端头拼接：`MDR_WallJoinerVisualisationManager`(0x10a00230, Core3D.dll)——
   ord13/14/19 端头被邻墙裁剪/延伸的确切规则。
2. 查开洞来源：6 件带内环的洞是墙体 opening 还是 PANE 负几何，决定在管线哪一层挖
   （不影响轮廓 AABB，但影响面片/体积）。
3. 改 `rvm_compare` 判据（世界 AABB → 局部系 / Hausdorff），把这套局部系口径固化进对拍工具。

## 本段新增文件

- `vendor/e3d-model/tools/analyze_gwall_profiles.py`：E3D facet → 局部系 → 逐件对我方环轮廓（取证）。
