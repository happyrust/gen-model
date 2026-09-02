# 开发计划:BRAN/HANG router 几何 —— 收口到 core.dll / Core3D.dll 全等价

> 计划 ID:`2026-08-31-bran-hang-model-generation`（本文件 = 审核后的收口计划）
> 创建:2026-08-31 · 作者会话:fable-5-42（ida-bridge 审核轮）
> 状态:**draft**（待 plannotator 门禁）
> 同目录配套:`algorithm-bran-hang.md`（算法全文）、`task_plan.md`（P0–P4 主计划）、`route-nouns.json`
> 权威实例:`idalib-41236` = `core.dll`;`idalib-35724` = `Core3D.dll`（凡标地址处可按地址复查）
> 审核对象:`D:\work\plant-code\old\vendor\e3d-model`（`route.rs` / `pipeline.rs` / `category.rs`）

---

## 0. 这份计划从哪来（一句话）

2026-08-31 本轮用 ida-bridge 把 `vendor/e3d-model` 的 router 几何算法逐个函数对完了
core.dll / Core3D.dll，**结论是承重点全部一致**（§1 基线表）。剩下的不是「不一致」，
而是三处**未完成**（真外径、几何集特例路、零长阈值）外加一处**未复核**（P 点求解链），
本计划把它们收口，并先处理一处**版本落差**（§2）。

---

## 1. 审核基线：已与 DLL 逐条坐实（全部本轮现场反编译）

| 环节 | DLL 地址 | 反编译结论 | Rust 对应 | 判定 |
|---|---|---|---|---|
| ORIMAT 定向 | core `0x526d674` | Z=归一(dir);副轴按 X̂→Ŷ→Ẑ 试 `X=归一(Z×e)`;`Y=Z×X`;行序 [X;Y;Z] | `route.rs::orimat` `from_cols(x, z×x, z)` | ✅ |
| TUMAT 定位 | Core3D `0x103439c0` | 平移=(P1+P2)×0.5;`v=归一(P1−P2)`，退化回落 D1 | `route.rs::tumat` | ✅ |
| 中点常量 | Core3D `0x10B4DC10` | 实读 = **0.5** | `* 0.5` | ✅ |
| 长度 | GTTUBG 内 `VDIST(P1,P2)` | 两点直线距离，不投影不扣端面 | `tube_length` | ✅ |
| 造圆柱 | GTTUBG `gm_CreateCylinder(od/2, 长度)` | 高 = 真实长度直接建 | `csg_cylinder_solid(diameter, length)` | ✅ |
| CGETOD 外径 | pplib `0x103bf7ac` | od=PARA[2];保温位置则 +IPAR[1] | `tube_outer_diameter`（catalogue_od + insulation） | ✅ 函数正确 |
| 外径回落 | GTTUBG `失败则 v60=v57` | v57 = GTTUBE 第 12 出参 = **到达侧 bore2** | 回落 `arrive_bore` | ✅ 取到达侧 |
| 取两端点 | GTTUBE `0x1034352c` | A=BRAN∨HTCOMP→IHEAD/TAIL(H/TPOS);否则 PLEAVE/PARRIV | `container_head/tail` + `PPointResolver` | ✅ |
| HANG≡BRAN | `0x106725f0` / `0x10672770` | next 到尾返回 TposElement、prev 到头返回 HposElement | 5 容器同一 `walk_route` | ✅ |
| 容器/成员集 | GTTUBE 硬编码 BRAN=808220 / TRUNNI=137403155 / LUG=537123 | 全落在 HPOS+TPOS 判出的 5 容器内 | `category.rs` 5 容器 / 104 成员 | ✅ |
| CLIN 中心线 | GTTUBG noun=813891 走线段 | 非实体 | `build_tube` 只出圆柱 | ✅ |

**基线不再动**：以上任何一条被后续改动破坏，都算回归。P1 的复核单测要把 ORIMAT 副轴
tie-break 顺序、TUMAT 中点/退化回落、外径回落取到达侧这三条钉成断言。

---

## 2. P0（前置）：先对齐 vendor 与 gen-model 的版本落差

**问题**：本轮审核的 `vendor/e3d-model` 快照仍停在 **P2-partial**（`pipeline.rs`
第 445 行恒传 `tube_outer_diameter(None, None, to.bore)`、`tubes_measured=0`），
而 `task_plan.md` 文末「2026-08-31 ida-bridge 实施进度」段已记录 **562 根管建成、
TLEN/PTCD 已实现、动态分类 `unknown_nouns=0`**——两处描述的不是同一份代码。

**动作**（本计划任何几何改动之前必须先做）：

- [ ] diff `vendor/e3d-model/src` 与 gen-model 侧实际在改的那份 e3d-model，判定哪一份是主线。
- [ ] 若 vendor 落后：先把 gen-model 侧的 P1（catalogue eval / TLEN / PTCD）与 P2 已建成
      成果同步进 vendor，再在**同一份代码**上做 §3 的收口；否则会在旧快照上重复造轮子。
- [ ] 两份都无 git 隔离时，动手前确认无并发写（`task_plan.md` §7 记过两次同树互踩）。

**验收**：确定唯一主线代码路径；`gen_ams` 全库 ams8000 能复现进度段那份账
（`route_containers=560`、`implied_tubes=3172`、管身建成数与进度段一致）。

---

## 3. 收口缺口（按优先级；每条都给 DLL 依据与验收）

### G1 —— 真外径 CGETOD ★★ RG 实测后翻案：大概率是伪命题，**先别做**

> **2026-09-01 已了断，冻结解除**——见文末「批注处置记录」G1 行。要点：GATREA/GATCAT/GATCRF
> 反编译钉死了 TUBI 伪元素经离开侧 stub 的物化链，推翻下面「解析停在 barren SPCO」的推理；
> GTTUBG 层复核再证「OD 优先、到达侧 bore 回落」即 core 原语义。vendor 已按此接线且带
> `catalogue_od`/`bore_fallback` 双账，RVM 半径对拍仍留在 §5 验收总门。下面的 RG 记录**作废存档**。

> **2026-08-31 RG 探针（`examples/tube_od_probe.rs`，ams8000 全库）结论：G1 很可能不成立，
> 当前的 bore 半径已经和 E3D 一致；「接真外径」反而会把管子画成错的粗细。**
>
> 实测链（3102/3110 解通，余 8+13 条跨库 db 6890/7000 未加载）：
> `离开侧元素 --stub(HSTU容器头 / LSTU构件)--> SPCO --CATR--> SCOM`
> - **SPCO（= `v65.TUBI` 的自然目标）自身 PARA=0、GMRE=0**（barren，0/3110）。
> - 只有再跳一次 `CATR` 的 **SCOM** 上才有 `PARA=[bore, OD, code]`（OD=21.3 / 25.0…）与
>   `GMRE`（成员**清一色 LINE** 中心线，3089/3089）。
>
> **决定性推理（为什么是 bore 而非 OD）**：真管是实体。若 `v65.TUBI.GMRE` 真到达那个
> SCOM 几何集（只含 LINE），`GTTUBG` 会认出 LINE→`v16=-1`→**只画中心线、跳过默认圆柱→
> 不出实体**，与实体管矛盾。所以 E3D 的 `TUBI` 解析停在 barren 的 SPCO：`GATRF1(v65,TUBI,GMRE)`
> 落空→走默认圆柱路;同理 `CGETOD` 的 `GATREA(v65,TUBI,PARA,2)` 也落空→`v60=v57`→
> **半径取到达侧 bore**。即 `tube_outer_diameter(None,None,to.bore)` 已经复刻了 E3D。
>
> **残余不确定**：`GATREA/GATREF` 的 `TUBI` 解析是否真停在 SPCO（`GATCRF` 对 GMRE 会多跳
> 一次 CATR、对 PARA 不跳，但那是另一个 resolver）——静态没 100% 钉死。**唯一干净的了断
> 是 RVM 半径对拍**：E3D 的管半径 == bore/2 还是 OD/2。在拿到 RVM 前，**不动半径**。
>
> 下面这段原始设想（接 PARA[2]）**作废存档**，除非 RVM 判出 E3D 用 OD 才复活。

- **原现状描述（存档）**：`pipeline.rs` 恒传 `(None, None, to.bore)`，即每根管半径都来自**到达侧
  通径**回落，尚未接 CGETOD 的 `PARA[2]`（+ 保温 `IPAR[1]`）。函数 `tube_outer_diameter`
  本身对，只是没喂真值。
- **DLL 机制（本轮反编译到底，改写了原设想）**：
  - CGETOD `sub_103BF7AC`（`0x103bf7ac`）：`od = GATREA(v65, TUBI, PARA, 2)`;
    `if (insFlag&1) od += GATREA(v65, TUBI, IPAR, 1)`。回落 `v60 = v57`（到达侧 bore2）在
    GTTUBG 调用点，不在 CGETOD 内——Rust 合进 `tube_outer_diameter` 等价，保持不变。
  - `GATREA`（core.dll `0x5a36580`）实测：它构造 `DB_Ref(v65, TUBI)` 取目标元素，再读该目标
    的 `PARA[2]`。也就是 **OD = `设计元素.TUBI → 目录件.PARA[2]`**。
- **★ 本轮踩到的真阻塞（G1 不是接线，是新能力）**：`TUBI`（TUBING）在 `noun_layout.json`
  里是 **`isPseudo:true` 且 `attrs:[]`（零属性）** 的**运行时伪元素**——它不是文件里的记录。
  E3D 在运行时**从管道 spec 按当前通径选出「管子件」**，再由那个 SCOM 的 `PARA[2]` 给 OD。
  所以：
  - **`member.get_element("TUBI")` 读不到东西**（设计元素没有这个引用属性、伪元素也没有记录），
    盲接只会静默回落到 bore——等于没做，还留一个「看着接了」的假象。
  - `catalogue_point.rs` 现在解的是构件**自己的** SCOM（`SPRE→CATR→SCOM`），那给的是**构件
    自身**尺寸，不是管子件的 OD。管子件是 spec 里**另一个**组件。
- **因此 G1 的真落地 = 新增「按 spec + 通径解出管子目录件」这条能力**：
  1. 先反编译 `catdblib` 的选件链（`GATCAT` 0x1035c340 / `G1TSPE` 0x1035c96c）与 TUBING 伪元素
     的物化点（`PDMS_TubiElement` 0x10665430 一带），钉死「给定 BRAN 的 spec 与当前 bore，
     E3D 选中哪个 SCOM 当管子件」。
  2. 在 e3d-io 加一个「按 bore 选 spec 组件」的解析器，返回管子 SCOM;`PARA[2]` = OD。
  3. `pipeline` 把它作为 `catalogue_od` 传进 `tube_outer_diameter`;取不到仍回落 `to.bore`。
- **过渡option（若先要几何、暂不做 spec 选件）**：维持 bore 近似，但在
  `ImpliedTubes` 记账里**单列 `radius_from_bore` 计数**并在 `notes` 标「近似半径」——
  让「半径是真 OD 还是通径凑的」在报告里一眼可见，绝不无声。
- **验收**：ams8000 抽 ≥10 条管，半径 = 管子件 OD/2 与 E3D `Q` 值一致;
  仍回落到 bore 的条数单列（目录缺 OD，不是错）。

### G2 —— 几何集特例路（GTTUBG 的 TUBE/BOXI/LINE 分支）★ RG 实测：ams8000 不触发，非缺口

> **RG 探针结论**：ams8000 里管子件 SCOM 的几何集成员**清一色 LINE**（3089/3089），
> **没有一个 TUBE/BOXI 实体成员**——几何集里没有可出实体的东西。而且（见 G1）E3D 的
> `TUBI.GMRE` 落在 barren 的 SPCO 上，根本不进这条分支。所以在本语料里 G2 既不出实体、
> 也不被触发，`build_tube` 只走默认圆柱是对的。**保持记账不做**；换到别的语料若探到
> 几何集带 TUBE/BOXI 实体成员，再按下面的 DLL 依据实现。

- **现状**：`build_tube` 只走默认路（单圆柱 + 跳过 CLIN），未实现几何集分支。
- **DLL 依据**：GTTUBG（`0x10340f8e`）中 `GATRF1(A, TUBI, GMRE)` 命中后的 `switch(TYPE)`：
  - `TUBE 631901`：`QTUBE`（`0x103bf5c0`）读 `PDIA`/`PAXI` → `cylinder(PDIA/2, length)`，
    平移在 TUMAT 之上再叠加「PAXI 偏移经 A 世界矩阵旋转」的量（`MVMULT`）。
  - `BOXI 726491`：`QBOXI`（`0x103bf6d8`）读 x、z → `box(x, length, z)`，**Y 向吃长度**。
  - `LINE 640317`：线段 P1→P2，**非实体**，出网格跳过。
  - 每个成员先过显示过滤（`LEVE[2]`/`OBST`/`TVIS`/`BVIS`/`CLFL`/`TUFL`，经 `sub_107149B0`）。
- **★ 与 G1 同一个前置**：`GATRF1(A, TUBI, GMRE)` 里的 `A.TUBI` 也是那个运行时伪元素，
  几何集挂在 spec 选出的管子 SCOM 上，不在设计元素上。所以 G2 也要先有 G1 那条
  「按 spec + 通径解出管子目录件」的能力，才谈得上取 `GMRE`。
- **落地**：先写探针数 ams8000 里「管子件带几何集」的占比。占比可忽略就**先记账不做**
  （报告显式记该路命中 = N，built = 0），不要为长尾提前铺实现。
- **验收**：命中件逐个比对（体积/AABB）;若记账不做，报告里该路计数可核对、不为静默 0。

### G3 —— 零长阈值 ε 钉死

- **现状**：`ZERO_LENGTH_TOL_MM = RES_TOL_MM`（0.051mm）是占位，`route.rs` 注释已注明
  「GTTUBG 里那个比较的立即数本轮没反编译到」——这是本模块**唯一一个不是抄来的数**。
- **动作**：在 GTTUBG / `gm_CreateCylinder` 路径上定位零长（或近零长）判据的立即数，
  或用真库对零长 BRAN 的取舍对拍钉死。
- **验收**：与 E3D 对同一批零长/近零长 BRAN 的「建 or 不建」判定一致;常量来源标注地址。

### G4 —— P 点求解链复核（endpoint resolution，本轮未对 DLL）

- **现状**：几何**给定两端点后**已对齐 DLL;但两端点从目录侧怎么解（`catalogue_point.rs`
  的 `SPRE→SPCO.CATR→SCOM.PTRE→PTAX/PTCA` 链 + `PDIS`/`PBOR`/`PAXI` 求值）本轮**没有**
  拿 DLL 复核。进度段记有两个硬缺口：`( ATTRIB RPRO TLEN )`（FTUB，1529 处）与 `PTCD`
  （PTCA 方向编码，516 端点）。
- **DLL 依据（复核锚点）**：`PLEAVE`/`PARRIV`（`0x103bcef8`/`0x103bd1c4`）、
  `QPPOS`/`QPDIR`/`QPBOR`（`0x103badac`/`0x103bac30`/`0x103bbb2c`）、
  `GATREA`/`GATINS`；表达式求值对标 `exprlib/EXEV*`、`exppdms/GATPAR`。
- **落地**：① 对一条 `PTAX` 正样本（进度段的 `/LV-CO-1R312-D` ELBO，与容器 TPOS 差 0.005mm）
  回读 DLL 侧 `QPPOS`/`PDIS` 求值路径，确认 e3d-io 求值与 core 口径一致;
  ② `RPRO TLEN` 与 `PTCD` 两条按进度段方案（`DORTXT` 轴链 + PML 角表达式 / HEIG 映射）
  在 DLL 里核对判据，别在 Rust 侧凭邻居值猜。
- **验收**：4714 个端点的定位属性求出率与进度段（67.6% → 目标更高）对齐;`RPRO TLEN`
  的映射有 DLL 或 E3D 实测背书，不是替目录作者编答案。

---

## 4. 收口顺序与依赖（RG 实测后重排）

```
P0 版本对齐(§2) ✅
   │
   ├─► RG 反编译+探针 ✅ ── 结论：G1/G2 大概率伪命题（E3D 按 bore、几何集只含 LINE）
   │        └─► 唯一了断 = RVM 半径对拍：E3D 管半径 == bore/2 ?  ── 判 bore 则 G1/G2 关闭
   │                                                           └ 判 OD 才复活 G1
   ├─► G3 ε 钉死（可能在未加载 libgm 里）
   └─► G4 P点链 DLL 复核（纯反编译，可先行）
                                        └──────► RVM 对拍门(§5)
```

- **RG（本轮已做）**：`examples/tube_od_probe.rs` 在 ams8000 上跑通，链是
  `元素--stub-->SPCO--CATR-->SCOM`。SPCO barren（PARA/GMRE 皆 0），OD/几何集在 SCOM 上，
  但几何集只含 LINE；据「真管是实体」反推 E3D 的 `TUBI` 解析停在 SPCO→bore 回落 + 默认圆柱。
  **⇒ G1、G2 在 ams8000 都翻案为「非缺口」，当前 bore 半径已与 E3D 一致。**
- **现在真正剩下的活**：G3（ε）、G4（P 点链复核）、以及**用 RVM 半径对拍把 G1 的翻案钉死**。
- G3、G4 互不依赖，可并行;G4 纯反编译能立刻推进。
- **不再有「bore 近似过渡」一说**——bore 现在是**正解**而不是近似（除非 RVM 推翻）。

---

## 5. 验收总门（对齐主计划 §四 / §P4）

- [ ] RVM 对拍：体积 / AABB / 连通分量三口径，管身子集逐元素比对，超差逐条归因。
- [ ] `accounts_for` 恒平：`visited + consumed == 索引全集`;隐式管身走独立账不进等式。
- [ ] `--no-instance-cache` A/B 两条路产物逐元素一致（管身天然高命中，单列其命中率）。
- [ ] §1 基线 11 条一条不破（回归断言）。

---

## 2026-09-01 实施进度（fable-5-18 会话，RVM 对拍轮）

> 本节只记已运行有产物的结果。产物目录 `vendor/e3d-model/out/`。

- **显示过滤落地**（`catalogue.rs::display_filtered`，本轮新增）：几何集正体成员按
  ①`LEVE=[lo,hi]` 区间（`lo<=level`，`hi==0` 读作不设上限）②`OBST==2 且 TUFL==false`
  （硬障碍体）③`CLFL==true`（中心线族）三条筛；正体求值器里旧的 TUFL 单旗门全部移除
  （`C-IY` 基准里 `TUFL=false` 的 RESERVED 盒被 E3D 导出，单旗门是误杀）。
  表现级默认 6（GUI/addin 口径），`E3D_REPRE_LEVEL` 环境变量可覆盖（TTY 裸会话实测 0 级）。
  **两旗联判的出处**：管道目录实体成员（AAEA 弯头 SCTO、CNPE 阀 LSNO）顶着 `OBST=2/TUFL=true`
  照画，单看 OBST 会误杀整个管件族——这是四份语料唯一都能过的读法，DLL 侧 want 回调
  （`SETWNT` 注入 `dword_10E50668`）本轮未追到注册端，规则先以基准对拍立账。
- **对拍战果**：
  - `1RX07-LCT`（dbnum 7999 桥架 zone，级 0 导出）：**137/137 exact**（BEND 20 / ELBO 8 / FTUB 109）。
  - `C-IY-1R330-B`（dbnum 8000 槽盒，addin 级 6 导出）：FTUB **17/17 exact**；BEND 6/18 exact，
    其余 12 条差 ~100mm = 我们多画了 L/R_SPLICE_PLATE（小角度弯 E3D 不画拼接板，
    旗子与 BOARD 完全同值，判据未找到——**待 DLL want 回调或更细基准裁**）。
  - `=24383/73948`（用户样例 BRAN，管道）：9 构件 + 5 管身全建成、零失败，OD 全走目录真值；
    RVM 基准**尚无**——TTY 导出被运行中的 E3D GUI 会话（des.exe ams /ALL，8/31 起）挡住。
- **跨库目录清单坐实**（gen_ams 需 `--catalogue` 全开才零失败）：ams5052/5053/5054/5100/5101/5200
  + ams7000 + ams6890 + **zdj7600**（ZDJ 项目）+ **acp7320/acp7002**（AvevaCatalogue 项目）。
- **全库 ams8000 route-only**：`route_containers=560`、`generated=2665`、**`failed=0`**；
  管身 3172 槽位建成 570（零长 2593 / 缺 P 点 8 / unbuildable 1），`catalogue_od=3099`、
  `bore_fallback=65`。回归 139 lib 测试全绿，fmt/clippy 干净。
- **本轮翻出的新账**（按命中量排序，均记账未做）：
  1. `NSEX` 负拉伸 ×1593（目录负体，螺栓孔/减重孔一族）——复用 `evaluate_sext` 出 cutter 即可；
  2. `SREV` 旋转体 ×340（§2.5 有 DLL 配方：`QSSREV` 0x103b81cc，超 180° 拆两段）；
  3. 桥架 FTUB 的管身路（`LSTU→SPCO→CATR→SCOM(GTYP=TUBE, PARA 含 BOXI 哈希)`）：E3D 给
     tray FTUB 画的 100×50 盒走的是这条（G2 在桥架语料上是真缺口）；目前靠 CABLEWAY 成员
     （尺寸恰好同构）对上了 AABB，出处要在 GTTUBG 的 BOXI 分支钉死；
  4. 小角度桥架弯的拼接板判据（见上）。
- **E3D 侧观察**：TTY 裸会话（未设 repre level）导出的桥架弯/弯头是**单段**面片
  （`1RX07` 的 90° 弯只有 6 面 24 顶点，字节级探针证实文件如此）——用这类基准裁弧面细分
  没有意义，只能裁成员选择与 AABB。

## 批注处置记录

| 轮次 | 批注 | 处置 |
|---|---|---|
| — | （待 plannotator 门禁） | — |
| 2026-09-01（fable-5-16 审核轮） | G1「拿到 RVM 前不动半径」的冻结令，与工作树已接真外径相抵触，缺一条了断 | **已了断，冻结解除。** ① ida-bridge 补课钉死选件链：GATREA（core `0x5a36580`）构造 `DB_Ref(v65, TUBI)` 后读**目标元素**的 `PARA[2]`；GATCAT（`0x1035c340`）/ GATCRF（`0x1035d7d8`）解出 TUBI 伪元素经**离开侧 stub**（HSTU/HSRO/LSTU/LSRO，TYPE==TABITE 再走 PRTREF）物化——RG 轮「解析停在 barren SPCO → 恒回落 bore」的推理被推翻（取证记录：`vendor/e3d-model/examples/tube_od_probe.rs` 头注）。② 2026-09-01 GTTUBG 层复核再证「CGETOD 优先、取不到回落**到达侧** bore」即 core 原语义（`vendor/e3d-model/docs/2026-09-01-next-step-development-plan.md` §2.2），与主计划 §2.7 五条口径第 4 条一致。③ vendor 实现不无声：`ImpliedTubes.catalogue_od`/`bore_fallback` 双账 + 每根管身 notes 标 `source=catalogue PARA[2] / arrive bore fallback`，满足本节「过渡option」的可见性要求。④ RVM 半径对拍从「动手前置」改判为 §5 验收总门过门项（对应新方案 G0-2）；判 bore 则回退接线、判 OD 则关账。 |

## Errors Encountered

| Error | Attempt | Resolution |
|-------|---------|------------|
| （本计划暂无） | — | — |
