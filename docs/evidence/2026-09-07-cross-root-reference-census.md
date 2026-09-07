# 2026-09-07 真库普查：跨生成根的引用边有多少（联动对齐计划 C1 的闸 + C5 的 TOPF 对拍）

- 问题：`docs/plans/2026-09-06-core3d-cascade-parity-dev-plan.md` §3 C1——Core3D `change/EVALAT` 里两类**引用类**联动
  （结构连接 `ADSTCH`、孔洞 `HOLC` 反向）在 direct 文件窗口路径上一条都没有；**真库里这些边跨不跨生成根？有没有只改一端的窗口？**
  数字为零就不写代码。
- 手段：`src/bin/cross_root_reference_census.rs`，只读，5 个真项目 **486 个 DESI 库**全扫（同 09-04 普查的语料），
  `ScanTier::Full` + `desvir.dat` noun 模板解隐式区，根集用**生产同一段遍历** `enumerate_generation_roots_in_subtree`
  （direct 口径，`delivery_unit_types` 缺省 = BRAN/HANG/SUPPO/EQUI）。**18.8 s**。
- 机器可读：`2026-09-07-cross-root-reference-census.json`（合计 + 逐库）；控制台原文 `2026-09-07-cross-root-reference-census.log`。
  ★0907 同日补档（§7 电缆端点边）用的是加了那一档之后的复跑：`2026-09-07-cross-root-reference-census-c8.json`（`--examples 400`；
  结构 / 孔洞 / 全边表 / TOPF 各项合计与首跑逐字相同，只有会话链多走了 6 个只有电缆边的库：79 库 3,907 步 → 85 库 3,935 步）。
- 复跑：
  ```text
  cargo run --release --bin cross_root_reference_census -- \
      --dir D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000 --dir D:/AVEVA/Projects/E3D3.1/AvevaPlantSample/aps000 \
      --dir D:/AVEVA/Projects/E3D3.1/ZDJ/ZDJ000 --dir D:/AVEVA/Projects/E3D3.1/YCYK-E3D/acw000 --dir D:/AVEVA/Projects/E3D3.1/TEST/TES000 \
      --examples 100000 --json docs/evidence/2026-09-07-cross-root-reference-census.json
  ```

---

## 0. 一句话

**结构连接（清单 #12）在语料里是真缺口：21,161 条回指成立的跨根边，10 个库里有只改一端的真实窗口 → C2 立项。
孔洞（清单 #15）在语料里不可达：负实体发出的 9,329 条引用一条都不指向被打孔件，跨根的只有 20 条 `NCYL.RREF→SUBS`，
零窗口 → C3 不立项，改判「语料上不可达（留探针）」。**

| 组 | 边 | 回指成立 | **回指成立 ∧ 跨根（nearest）** | 跨根（any，下界） | 只改一端的窗口 |
|---|---:|---:|---:|---:|---:|
| 结构连接（`CREF`/`JOIS`/`JOIE`/`CFRA`/`PFRE`） | 55,503 | 47,556 | **21,161**（其中 src 在 `STRU` 下 21,161，即 100%） | 23,256 | **783 边·步**，12 个窗口 / 10 个库 |
| 孔洞（12 个负实体 noun 发出的一切引用） | 9,329 | — | 反向 20 | 0 | **0** |
| ★0907 补档 电缆端点（`STAREF`/`ENDREF` → `EQUI`/`ELCONN` [F]、→ `CWBRAN` 之下的路由节点 [E]；同库） | 444 | — | 反向 444（100%） | 444 | **0**（另 15 边·步电缆当时还没建，不算；§7） |

另外两件顺带查出来的事：

- **引用属性 70% 在隐式区。** 全语料 2,169,946 条引用边里 1,529,963 条来自 noun 模板定长槽位（`HREF`/`TREF`/`JOIS`/`JOIE`/
  `CREF`…），只看显式尾块的第一版探针把结构连接边数成了 **61** 条。任何「按引用边做联动」的实现都得走模板解码，不能只读显式块。
- **C5：core 单独重画而我们一个根都不归的 primitive 元素只有 268 个 / 1,961,172**，几乎全是船体族
  （`CPANEL` 160 + `CPROF` 82 + `BNDLIN` 5，挂在 `BLOCK` / `CSURPX` / `BLEVEL` 下），只在 AvevaMarineSample 里。
  BOX 直挂 ZONE 那一类在语料里**不存在**（`RPLA`×2、`CNODE`×3 直挂 ZONE 是仅有的两例）。

## 1. 语料与判据

| | |
|---|---|
| 库 | 936 个文件 → 486 个 DESI（448 个不同 dbnum：`zdj7200_0001` / `zdj7210` / `aps7200_0001` 是同一库的三份拷贝，合计里有重复）；跳过 450 个非 DESI |
| 元素 | 1,961,172；**96,102 个不在任何根之下**（含 `DBMETA`/`DBMDAT` 之外的所有层级容器、组织节点、船体族） |
| schema | `E:\reverse\e3d\shadow_e3d31_aps_all\{attlib.dat, desvir.dat}`（direct 模式的默认模板目录）；0 个元素没解出模板 |
| 根 | `enumerate_generation_roots_in_subtree`（`generation_root.rs`）——交付单元优先、交付单元之下不再出根、其外 `core_significant_bit` 为真且非层级容器的 noun 成根 |
| 根链 | 元素 owner 链上所有是根的祖先（含自身）；「最近的根」= 链首 |
| 正向覆盖（nearest） | src 变了 → dst 会不会重算：**dst 最近的根 ∈ src 的根链**。嵌套根各管各的几何时的口径 |
| 正向覆盖（any） | 两条根链有交集。外层根整棵子树重生成时的口径；比 nearest 宽，作缺口**下界** |
| 反向覆盖 | 同上两档，src / dst 互换（孔洞那条看这个方向） |
| 回指核对 | 与 `ADSTCH` 同形：`el.CREF → X` 要 `X.JOIS == el ∨ X.JOIE == el`；`el.JOIS/JOIE → X` 要 `X.CREF == el`；`el.CFRA → X` 要 `X.PFRE == el` |
| `STRU` 门 | `EVALAT` 规则 K 的前置 `CGETEL(STRU)`：src 的 owner 链上有 `STRU` 才走 `ADSTCH`。单独数一格 |
| 窗口 | 每库会话链 `diff_steps(首个保留会话 → 最新)` 逐步，79 个库 3,907 步（其余 407 个库没有跨根边，不看）；每步的变更键集取**索引层**（未逐记录排除「搬家未改字节」） |

## 2. 结构连接（清单 #12）

### 2.1 边表（合计 55,503；只列 ≥ 60 条的）

| 边 | 条数 | 跨 DESI 库 | 正向缺口 nearest | 缺口 any | 回指成立 | **回指 ∧ 缺口** | 在 `STRU` 下 |
|---|---:|---:|---:|---:|---:|---:|---:|
| `PJOI.CREF → SCTN` | 10,314 | 0 | 7,089 | 6,992 | 10,302 | **7,089** | 10,314 |
| `SJOI.CREF → SCTN` | 7,046 | 0 | 2,089 | 584 | 6,909 | **2,089** | 6,935 |
| `TEE.CREF → BRAN` | 5,615 | 0 | 5,615 | 5,615 | 0 | 0 | 0 |
| `SCTN.JOIS → PJOI` | 5,325 | 0 | 3,699 | 3,495 | 5,325 | **3,699** | 5,325 |
| `SCTN.JOIE → PJOI` | 4,977 | 0 | 3,715 | 3,497 | 4,977 | **3,715** | 4,977 |
| `FIXING.CREF → GENSEC` | 4,715 | 0 | 1,040 | 264 | 4,601 | **1,040** | 4,687 |
| `SCTN.JOIS → SJOI` | 3,716 | 0 | 1,051 | 292 | 3,716 | **1,051** | 3,672 |
| `SCTN.JOIE → SJOI` | 3,193 | 0 | 1,038 | 292 | 3,193 | **1,038** | 3,126 |
| `GENSEC.JOIE → FIXING` | 2,410 | 0 | 478 | 123 | 2,410 | **478** | 2,402 |
| `GENSEC.JOIS → FIXING` | 2,191 | 0 | 532 | 141 | 2,191 | **532** | 2,171 |
| `SCTN.JOIS → SUBJ` / `SUBJ.CREF → SCTN` | 1,307 / 1,307 | 0 | 0 | 0 | 1,307 | 0 | 1,307 |
| `NOZZ.CREF → BRAN` | 971 | 0 | 971 | 971 | 0 | 0 | 0 |
| `ATTA.CREF → HANG` | 430 | 0 | 430 | 430 | 0 | 0 | 0 |
| `PJOI.CREF → STWALL` | 397 | 0 | 37 | 37 | 397 | 37 | 397 |
| `STWALL.JOIE/JOIS → PJOI` | 201 / 196 | 0 | 18 / 19 | 18 / 19 | 201 / 196 | 18 / 19 | 201 / 196 |
| `FITT.CREF → HANG` | 135 | 0 | 135 | 135 | 0 | 0 | 135 |
| `SJOI.CREF → GENSEC` | 120 | 0 | 120 | 0 | 120 | **120** | 120 |
| `OLET.CREF → BRAN` / `BRCO.CREF → BRAN` | 108 / 61 | 0 | 108 / 61 | 108 / 61 | 0 | 0 | 0 |
| `GENSEC.JOIE/JOIS → SJOI` | 60 / 60 | 0 | 60 / 60 | 0 | 60 / 60 | 60 / 60 | 60 |
| `SJOI.CREF → STWALL` | 60 | 0 | 33 | 33 | 60 | 33 | 60 |
| 跨 DESI 库的（`NOZZ.CREF` / `FITT.CREF` / `FIXING.CREF` / `GENSEC.JOIE` → 别的设计库） | 287 | 287 | — | — | 未核（对端在别的库） | — | — |
| `CMPF.CFRA → COFI` / `COFI.PFRE → CMPF` | 10 / 10 | 0 | 0 | 0 | 10 / 10 | 0 | 10 |
| **合计** | **55,503** | 287 | 28,554 | 23,256 | 47,556 | **21,161** | 47,757 |

读法：

1. **管道那一半不算。** `TEE`/`NOZZ`/`ATTA`/`FITT`/`OLET`/`BRCO`/`PIPCA`/`THRE`/`FLAN`/`VALV`/`ELBO.CREF → BRAN/HANG`
   共约 7,400 条全是跨根（管件的连接引用指向别的 BRAN），但**回指成立 0 条**（BRAN 没有 `JOIS`/`JOIE`），src 也**不在
   `STRU` 下**——`ADSTCH` 一条都不会触发。与 09-04 清单 #5「管道连接是存储属性，core 不做依赖递归」互证。
2. **结构那一半全部命中 core 的触发条件。** 21,161 条回指成立的跨根边，src 100% 在 `STRU` 下（`FIXING.CREF → GENSEC`
   的 4,687 / 4,715 在 `STRU` 下，其余 28 条挂在别处）。三族 noun 组成全部（两个方向都算）：`PJOI ↔ SCTN/STWALL`（14,577）、
   `SJOI ↔ SCTN/GENSEC/STWALL`（4,484）、`FIXING ↔ GENSEC/STWALL/WALL`（2,100）。
3. **`nearest` 与 `any` 差得不多**（21,161 vs 23,256 的口径下，结构组 `PJOI.CREF → SCTN` 7,089 vs 6,992）：
   这些边**大多连外层根都不共享**——对端在另一个 `FRMW` / `SBFR` 之下。`FIXING.CREF → GENSEC` 是例外（1,040 vs 264），
   四分之三的 FIXING 与它的 GENSEC 共一个外层根、只是 GENSEC 自己另有嵌套根。
4. `SUBJ ↔ SCTN`（1,307 对）**零缺口**：子关节永远与它的 SCTN 同根。

### 2.2 分项目

| 项目 | 库 | 元素 | 结构边 | 回指成立 | **回指 ∧ 缺口** | 缺口 any | 看了步 | 窗口：持属性端单独改 / 被引用端单独改 / 两端都改 |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| AvevaPlantSample `aps` | 98 | 499,839 | 11,782 | 10,938 | **2,770** | 1,196 | 124 | **591** / 591 / 2,838 |
| YCYK-E3D `acw` | 34 | 230,527 | 7,375 | 1,874 | 838 | 5,587 | 172 | 0 / 0 / 838 |
| AvevaMarineSample `ams` | 292 | 1,129,863 | 14,143 | 13,152 | 358 | 782 | 468 | 0 / 0 / 700 |
| TEST `TES` | 42 | 52,858 | 19,470 | 19,306 | **16,476**（`TES1038_0001` 一个库 13,988，2,772 个会话） | 15,146 | 3,086 | 2 / 2 / 18,964 |
| ZDJ `zdj` | 20 | 48,085 | 2,733 | 2,286 | 719 | 545 | 57 | **190** / 190 / 736（含 `7200` 两份拷贝） |

28 个库有缺口边；**10 个库有只改一端的窗口**（`aps250108/250110/250112/250209/250560`、`aps7200`、`zdj7200`×2、`TES1038`、`acw` 无）。

### 2.3 只改一端的窗口（C2 的验收窗口）

737 条「回指成立 ∧ 正向 nearest 不覆盖 ∧ 本步只有持属性一端改了」的边·步，落在 **12 个 (库, 步)**：

| 库 · 步 | 边·步 | 组成 |
|---|---:|---|
| `aps250110_0001` 29→30 | 195 | `FIXING.CREF → GENSEC` ×193，`GENSEC.JOIS/JOIE → FIXING` ×2 |
| `aps7200_0001` 3→4（= `zdj7200_0001` / `zdj7210` 3→4） | 94 ×3 | `SJOI.CREF → SCTN` ×94 |
| `aps250209_0001` 15→16、16→17 | 86 + 86 | `GENSEC.JOIS → FIXING` ×54 + `GENSEC.JOIE → FIXING` ×32，两步一样 |
| `aps250112_0001` 18→19 | 44 | `FIXING.CREF → GENSEC` ×44 |
| `aps250108_0001` 15→16 | 32 | `FIXING.CREF → GENSEC` ×32 |
| `aps250110_0001` 2→27 | 7 | `GENSEC.JOIE/JOIS → FIXING` |
| `aps250560_0001` 3→4、`TES1038_0001` 2771→2772、`aps250108_0001` 2→14 | 2 / 2 / 1 | 零星 |

**推荐 C2 的真库窗口**：`aps7200_0001` 3→4（10,450 个元素、10 个会话的小库，一步里 94 个 `SJOI` 单独改了、它们 `CREF` 指的
`SCTN` 没改且不在同一根下；core 会经 `ADSTCH` 组 2 把那 94 根 SCTN 的顶层件重画）与 `aps250110_0001` 29→30
（`FIXING` 单独改、`GENSEC` 对端不动，193 条）。两个窗口的变更键集都只看了索引层，做 C2 时要用 `changed_elements` 逐记录确认
不是「搬家未改字节」。

**「两端都改」占绝对多数**（24,076 边·步）：整个框架一起动时两端同在闭包里，没有缺口。缺口只在「只动一端」的那 783 边·步上。

### 2.4 这条缺口今天在几何上显不显

**今天不显，将来必显。** `vendor/e3d-model/src/section.rs:34` / `elmodl.rs:761`：SCTN 的负体——「JOIS/JOIE 关节负体与成员链上的
FITT / CMFI(+SBFI) / SNOD→SJOI / SCOJ→SUBJ（`DB_Sctn::DB_makeCSG` 的第 4 阶段）」——**还没接**，现在的梁是「没扣关节与配件的毛坯」。
所以今天对端 SCTN 不重算，几何也不会错（它本来就不读关节）；等第 4 阶段接上，`ADSTCH` 这条边就是正确性项：
对端 SCTN 的端部切面由关节决定，关节 / 对端一动，这根 SCTN 不重算就是旧几何。**C2 的验收在规划器层（`regen` 集合），
不依赖几何显不显；先把边接上、开关默认关，等第 4 阶段落地时不用再回头补。**

## 3. 孔洞（清单 #15）

HOLC 位（`0x92663`）没导出（核对表 R0：「P0 不必导 negative 表」），这里用字典 `positive_equivalent != 0` 的 **12 个负实体 noun**
（`NBOX NCYL NCON NSNO NCTO NRTO NXTR NPYR NDIS NREV NSLC NPOLYH`，排除 `DTHOLE`）作代理，收它们发出的**一切**引用边：

| 边 | 条数 | 去向 | 跨根 |
|---|---:|---|---:|
| `NCYL.TMRREF → TMRREL` | 3,632 | 模板关系（同根） | 0 |
| `N*.ORRF → 别的库` | 5,317 | 2,941 指别的 DESI 库、2,376 指非 DESI 库（原件引用，复制来源） | — |
| `NBOX.RREF → NBOX` | 360 | 同根 | 0 |
| `NCYL.RREF → SUBS` | 20 | 同库、反向 nearest 不覆盖 | 20（any = 0：共外层根） |
| **合计** | **9,329** | | **反向缺口 20，正向 0；窗口 0**（20 边·步全是两端都改） |

**没有一条负实体引用指向 `PANE` 或设备子件。** core §2.3-3 那条「被打孔的板 / 设备子件动了 → 引用它的孔洞构件重算」在这批语料里
没有对象。判词：#15 改「语料上不可达（留探针）」，C3 不立项。**不许当已证**：(a) HOLC 代理只覆盖负实体，E3D 孔洞管理的
`PFIT`/`FITT` 一族（`FITT.CREF → PFIT` 3 条、`PFIT.CREF → FITT` 3 条）不在代理里，但它们的量级也只有个位数；(b) 语料里没有
Hole Management 数据本身就是一个事实，出现了再复跑。

## 4. 全部 DESI 内引用边：正向 nearest 不覆盖最多的（背景，不是缺口清单）

| 边 | 条数 | 正向缺口 nearest | any | core 侧对应 | 判词 |
|---|---:|---:|---:|---|---|
| `REFGLN.GRDREF → GRIDPL` | 35,542 | 35,542 | 0 | 规则 B（网格，清单 #14） | 不排期；any = 0 说明外层根罩得住 |
| `HANG.HREF → HANG` / `HANG.TREF → HANG` | 13,115 / 12,457 | 全部 | ~半 | **无**（管道 / 支吊架连接是存储属性，09-04 #5） | 不是联动边 |
| `HTPELE.HTPREF → OBJHD` / `GOBELE.GOBREF → OBJHD` | 11,726 / 7,025 | 全部 | 全部 | 未追（船体 / 对象头） | 船体族，本计划外 |
| `PJOI.CREF → SCTN` 等结构边 | 见 §2 | | | `ADSTCH` | **C2** |
| `PCLA/VSPR/HELE.CRFA → HANG`、`HANG.HREF → VSPR/PCLA/HELE` | 5,758 / 5,624 / 2,458 … | 全部 | ~半 | 规则 P：`IHCOMP` → owner（不沿 `CRFA`） | 不是联动边 |
| `TEE.CREF → BRAN` / `BRAN.HREF → TEE` | 5,615 / 3,700 | 全部 | 全部 | **无** | 同 #5 |
| `VERT/DDAT/CYLI/LOOP/EXTR/… .ORRF → 别的 DESI 库` | 56,102 / 4,526 / … | — | — | 无 | 复制来源，不是几何依赖 |

## 5. C5：TOPF 顶层件 vs direct 根集（真库口径）

按 core `nounlib/LISTOP`（`toplevel` 位，`WORL` / `TMPL` 下除外；`primitive_a` 特例 `RPLA`/`PVOL`/`DRAW` 挂 `SITE`/`ZONE` 下、
`FIXING` 上行到 `STRU`）逐元素判「core 会单独重画」，再看它在 direct 口径下有没有根祖先：

| noun | TOPF 实例 | 有根 | **无根** | 无根实例的 owner | core primitive？ |
|---|---:|---:|---:|---|:--:|
| `APPLDA` | 205 | 0 | 205 | `APPDAR` | ✘ |
| `CWAY` | 187 | 0 | 187 | `ZONE` | ✘（名下已被更细的根罩住，09-04 §2.3c） |
| `CPANEL` | 160 | 0 | **160** | `BLOCK` | ✔ |
| `GPSET` | 107 | 0 | 107 | `GPWL` | ✘ |
| `CPROTR` | 98 | 0 | 98 | `CPROF` | ✘ |
| `BLOCK` | 93 | 0 | 93 | `WORL` | ✘ |
| `CPROF` | 82 | 0 | **82** | `CSURPX` | ✔ |
| `ASMBLY` / `DATU` / `CONVEY` / `CSURPX` / `HMARK` / `CNODE` / `CPLANE` / `RPLA` / `GRIDSY` / `TRN*` | 17 / 9 / 6 / 4 / 4 / 3 / 2 / 2 / 1 / 4 | 0 | 各自全部 | `ASMBLY` `ZONE` `HCMWLD` `MOGWLD` `TRNWLD` … | 混 |
| `BNDLIN` / `GENPRI` | 8 / 158 | 3 / 156 | 5 / 2 | `BLEVEL` `BBLOCK` / `TRNSLD` `TRNSUR` | ✔ |

**core 判 primitive（会出几何）却没有任何根祖先的元素：268 个 / 1,961,172**：`CPANEL` 160、`CPROF` 82、`BNDLIN` 5、`NCYL` 4、
`CNODE` 3、`BOX` 2（挂 `TRNPRI`）、`CPLANE` 2、`GENPRI` 2、`NBOX` 2、`PCLA` 2（孤儿，owner 不在库里）、`RPLA` 2（挂 `ZONE`）、
`CYLI` 1、`TRNPRI` 1。**只有一个成簇的：船体族 `BLOCK → CPANEL / CSURPX → CPROF / BLEVEL → BNDLIN`（247 个，全在 AvevaMarineSample）。**
W1.2 顺带看到的「BOX 直挂 ZONE 不出根」在语料里没有实例（primitive 直挂 ZONE 只有 `RPLA`×2、`CNODE`×3）。

判词进核对表 R0-3：**不改判据**；船体族登记为「几何覆盖度」议题（它们今天根本不生成，不是增量问题），出现船体语料再立项。

## 6. 不许当已证

1. 根链 / 覆盖判定在**元素层**复刻了 direct 规划器的口径（根集用同一段遍历，`touched` = 闭包 ∩ 根集 ⇒ `regen` = 变更元素的根链），
   但**没有真的跑规划器**——「nearest / any 两档哪一档是今天的行为」取决于嵌套根的几何归属（`generate_subtree` 整棵 vs
   `a_projection_that_stops_covering_the_nested_root_deletes_its_rows` 那条护栏），本文两档都给，C2 验收时以规划器实跑为准。
2. 窗口的变更键集取**索引层**（`SessionDiff::changes()`），含「搬家未改字节」的 Modified；做 C2 时对推荐窗口逐记录复核。
3. HOLC 用负实体 noun 代理（§3）。
4. `LISTOP` 的 `FIXING` 特例（「上行到第一个 toplevel 或 FIXING，到达 STRU 才算」）按字面实现，没有逐指令核。
5. 三份 `7200` 拷贝、`TES1038_0001` 的 2,772 个会话都在合计里；分项目表已点名，读合计时要知道。
6. 没有跑任何一条联动的实测；C2 的几何显不显见 §2.4。
7. ★0907 §7 电缆端点边只算**同库**边（引用表一张只答一个库）；跨库那 196 + 152 条没有会话链可对拍，窗口数不含它们。
   「电缆当时还没建」按 `Inserted` 的步序判，链头会话里就有的键当作一直存在。

## 7. ★0907 同日补档：电缆端点边（清单 #13 / #14 的 E 条，对齐计划 C8）

- 问题：C6 定性后 C8 同库版已接线（`cable_endpoint.rs`，开关默认关），但「只改 `EQUI`/`ELCONN`/`CWBRAN` 一端、电缆不动」的
  窗口一直**没量**——普查的窗口统计原先只跟结构边与孔洞边。用户要求补上，量全语料。
- 手段：`cross_root_reference_census` 加一档 `cable_endpoint`：`STAREF`/`ENDREF` 指向同库 `EQUI`/`ELCONN` 记规则 F，指向
  `HATTA`/`TATTA`/`RNODE`/`RATTA` 且 owner 链上有 `CWBRAN` 记规则 E（主语 = 那条 `CWBRAN`，core 规则 E 在 `CWBRAN` 变了时触发）。
  窗口口径跟着 core 的反向语义走：**被引用端变了 → 电缆要重画**，所以看的是「被引用端改了而电缆那端最近的根没被碰」；
  又因 GETWORK 先 `FNDTOP` 再 `EVALAT(top)`，多看一档「被引用端自己没改、但它最近的根下面有东西改了」。
  边是从链尾会话扫出来的，电缆那端在该步 `to` 会话里还没 `Inserted` 的边当时不存在，单独记 `cable_dst_only_unborn`、不算窗口。

| 边 | 条数 | 同库 | 跨根 | 反向 nearest 不覆盖 |
|---|---:|---:|---:|---:|
| `CABLE.STAREF → EQUI` / `CABLE.ENDREF → EQUI` [F] | 18 / 18 | 全部 | 全部 | 全部 |
| `EXTGEO.STAREF → HATTA/TATTA/RNODE/RATTA` [E] | 101 / 65 / 28 / 10 | 全部 | 全部 | 全部 |
| `EXTGEO.ENDREF → HATTA/TATTA/RNODE/RATTA` [E] | 16 / 85 / 30 / 73 | 全部 | 全部 | 全部 |
| **合计** | **444**（F 36 + E 408） | 444 | 444 | **444** |

只在 9 个库里：`aps250176/250179/250182` 及副本 `250514/250517/250520`（66 / 58 / 32 ×2，全是规则 E——那里的 `CABLE` 端点跨库指电气库），
`aps7200_0001` / `zdj7200_0001` / `zdj7210`（44 ×3 = 12 F + 32 E）。语料里 `CABLE` 与 `ELCONN` 从不同库，所以同库的 F 边对端全是 `EQUI`。

**窗口（85 个库 3,935 步）：**

| 档 | 边·步 | 说明 |
|---|---:|---|
| 被引用端记录改了 · 电缆根没被碰（`cable_dst_only`） | **0** | C8 该拉进来的窗口——**没有** |
| 被引用端的根被碰 · 记录没改 · 电缆根没被碰（`cable_dst_root_only`） | **0** | `FNDTOP` 口径也没有 |
| 只电缆端改（`cable_src_only`） | 15 | 整根重生成本来就覆盖 |
| 两端都碰（`cable_both`） | 457 | 闭包本来就有 |
| 被引用端改了但电缆当时还没建（`cable_dst_only_unborn`） | 15 | dbnum 7200 三份副本 × 5 条 `CABLE.ENDREF → EQUI`，2→3 改的 EQUI（`23584/5364 5382 5400 5417 5437`），电缆 4→5 才建；不加这条判据会把它们数成 15 个窗口 |

- 判词：**实例、边、反向表都在，语料里就是没人单独动过电缆的端点**。C8 同库版今天没有真库窗口可验收，`cable_endpoint::tests::live`
  用的假定源（假定 `EQUI 23584/10206` / 8 条 `CWBRAN` 变了）仍是唯一的真库证据；要一个真窗口得自己造（改一台接了电缆的 `EQUI` 存一次会话）。
  这不改「开关默认关」的建议，也不改 C3 不立项。
- 顺带：这档普查的规则 E 边逐库对拍出 `cable_endpoint.rs` 第一版只查 `CWBRAN` 直接成员会漏 `RNODE` 之下的 `RATTA`
  （`zdj7200_0001` 32 条 E 边只到 27 条），同日改成子树遍历，32 = 32。
