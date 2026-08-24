# 2026-08-25 T053 范围盘点：五类复用曲面原语的段数等价类

`specs/009-retire-occ/tasks.md` **T053** 要的东西：段数进身份键之后，
每一类复用型曲面原语各自炸成几行。plan 的 G3 至今只写了「柱 1 → 37」，
那个数来自 T045，而 T045 只数了柱。

**裁决：按 `geo_relate` 记，全量是 392 行 → 474 行（+82）；柱自己是 1 → 44，不是 37。**
**顺带查实一件更要紧的：T045 的 37 算在一个覆盖不全的来源上（`insts_flat`），
它漏掉的恰好是极端尺寸那一头。**

## 数据源与口径

| | |
|---|---|
| 数据 | `.surreal/ams-7997-e3d-test-20260805` 的一次性副本（库 A，与 T045 / T052 / 负 RINS 盘点同源） |
| 端点 | `http://127.0.0.1:8039/sql`（`bin/surreal.exe` 2.1.4；查完停进程、删副本，母本未写） |
| `inst_geo` 行数 | 8,094 |
| 容差 | `libgm_discretise::FACET_TOL_MM = 0.5` |
| 规则 | `circle_segments` / `part_rev_segments` / `spherical_dish_facets` / `elliptical_dish_facets` 逐行照抄自 `src/fast_model/libgm_discretise.rs`，脚本启动先跑一遍该模块单测里的对照表自检（R=1/25/100/250/3000/23400 → 8/16/32/52/176/484，以及 `elliptical_dish_facets(1000, 250, 0.5) == (100, 8, 9)`）才继续 |

**真实半径怎么从单位行还原**（依据各 `BrepShapeTrait::get_scaled_vec3`，
`../vendor/old-aios-core/src/prim_geo/`）：

| 变体 | 归一化字段 | `scale[0]` 是什么 | 真实半径 |
|---|---|---|---|
| `LCylinder` / `SCylinder` | `pdia = 1` | **直径** | `0.5 · scale[0]` |
| `LSnout` | `pbdm = 1`（底退化时 `ptdm = 1`） | **直径** | `0.5 · max(pbdm, ptdm) · scale[0]` |
| `Dish` | `pdia = 1` | **直径** | `a = 0.5 · scale[0]`，`h = pheig · scale[0]` |
| `CTorus` | `rout = 1` | **半径**（`get_scaled_vec3 = splat(rout)`） | `r_out = scale[0]`、`r_ins = rins · scale[0]` |
| `RTorus` | `rout = 1` | **半径** | `r_out = scale[0]` |

碟的高度那条单独验过：全部 17 行逐 scale 比对 `pheig · scale[0]` 与 `scale[2]/2`，
椭圆碟（`prad > 0`）两者逐个相等；球碟（`prad ≤ 0`）`get_scaled_vec3` 是
`(dia, dia, dia)`，`scale[2]/2` 本来就不表示高度，只有 `pheig · scale[0]` 成立。
两族统一用后者。全部五类的 `scale[0] == scale[1]`，无一例外。

## 一、等价类（按 `geo_relate`，即每个摆放一条边）

| 变体 | 行 | 实例 | 不同 `scale[0]` | **段数等价类** | 段数区间 |
|---|---:|---:|---:|---:|---|
| 单位柱 `inst_geo:⟨2⟩`（`PrimLCylinder`+`PrimSCylinder` 双键同一行） | 1 | 20,661 | 427 | **44** | 8 … 456 |
| `PrimCTorus` | 95 | 664 | 94 | **102** | 环向 2 … 82 |
| `PrimRTorus` | 167 | 1,945 | 147 | **174** | 3 … 256 |
| `PrimLSnout` | 112 | 1,201 | 115 | **133** | 8 … 160 |
| `PrimDish` | 17 | 102 | 21 | **21** | 绕轴 8 … 492 |
| **合计** | **392** | | | **474** | |

**改键后 `.mesh` 行数 392 → 474，净增 82。** 与表里已有的 3,896 行挤出相比仍可忽略，
ADR-044 决策 2 依旧成立——但排期与重建代价要按 82 记，不是按 plan 现写的 36
（37 − 1）。

等价类是**逐行**数的（同一行的不同实例落到不同段数才算炸开），因为身份键是
「原语参数 + 段数」，跨行不合并。

各类里最坏的那几行：

| 行 | 实例 | 1 → N | 段数元组 |
|---|---:|---:|---|
| `inst_geo:⟨2⟩`（柱） | 20,661 | **1 → 44** | 8, 12, 16, …, 456 |
| `inst_geo:⟨7219569483150680546⟩`（碟） | 5 | 1 → 5 | (112,28) (116,29) (128,32) (144,36) (152,38) |
| `inst_geo:⟨4338488021698513840⟩`（圆环面） | 9 | 1 → 3 | (10,12) (16,16) (46,40) |
| `inst_geo:⟨7886993277328827392⟩`（Snout） | 223 | 1 → 3 | 8, 12, 160 |
| `inst_geo:⟨2145556325219415407⟩`（矩形环面） | 25 | 1 → 3 | 14, 26, 30 |

炸开的行是少数：圆环面 6/95、矩形环面 5/167、Snout 18/112、碟 1/17。
**净增的 82 里有 43 来自柱那一行。**

球（`PrimSphere`）与切角柱（SSCL 自成一行的那种）本库仍是 0 行——
`inst_geo:⟨2⟩` 那条双键 param 是 `is_sscl == false` 的直柱，
T045a 的「找库或造样本」决策不受本次影响。

## 二、为什么不按 `insts_flat` 数：它只装正体

T045 数柱用的是 `inst_relate.insts_flat[]`，得 37；本次按 `geo_relate` 得 44。
差的那 7 类不是谁错了，**是两张表在回答两个问题**。

`insts_flat` 的回填式就写在 `pdms_inst::sweep_inst_relate_flat` 里：

```sql
SELECT trans.d AS transform, record::id(out) AS geo_hash
FROM out->geo_relate
WHERE visible && out.meshed && trans.d != none && geo_type = 'Pos'
```

四道过滤，其中 **`geo_type = 'Pos'`** 是关键：它是**读侧投影**，只装看得见的正体。
而 `geo_relate` 装的是全部摆放——本库 69,531 条边里
`Pos` 48,333 / `Neg` 12,848 / `CataNeg` 6,204 / `Compound` 1,628 /
`CataCrossNeg` 516 / `Tubi` 2。

**`.mesh` 是按 `geo_hash` 存的，而负体也吃 `.mesh`**：
`manifold_bool::apply_cata_neg_boolean_manifold` 取负体操作数走的是
`from out->geo_relate where geo_type == "Neg" or geo_type == "CataCrossNeg"`
再 `record::id(out)`——**与正体同一张 `geo_hash → .mesh` 表**。所以一根按 41,800 mm
缩放的负体柱，改键之后同样要一份 456 段的网格；而且负体分错段数正是 ADR-044
要治的那种「共面处留一层壁」（`cancelFacets` 只消全等重叠）。

按 `geo_type` 拆开：

| 变体 | 只算 Pos/Compound（= `insts_flat` 的口径） | 只算负体族 | **全部吃 `.mesh` 的** |
|---|---:|---:|---:|
| 单位柱 | 1 → **37** | 1 → 39 | 1 → **44** |
| `PrimCTorus` | 95 → 101 | 95 → 95 | 95 → **102** |
| `PrimRTorus` | 167 → 170 | 167 → 168 | 167 → **174** |
| `PrimLSnout` | 112 → 133 | 112 → 112 | 112 → **133** |
| `PrimDish` | 17 → 19 | 17 → 18 | 17 → **21** |
| 合计 | 392 → 460（+68） | 392 → 432（+40） | **392 → 474（+82）** |

**Pos 一侧恰好复现 T045 的 37**——那个数对它自己回答的问题是准确的，
本次只是把负体也算了进来。点名一件差异来源：

```sql
SELECT * FROM geo_relate WHERE in = type::thing('inst_info','24381_40090_63');
-- geo_type: "Neg",  scale: [41800, 41800, 200]   ← 一个 41.8 m 直径的圆柱形挖空
```

同类还有 `24381_40056`（41,800）、`24381_34395` / `24381_39915` / `24381_39081`
（34,800）、`24381_38148` / `24381_38285`（33,000）、`24381_39598`（10,200）、
`24381_46420` / `24381_180893`（8,000）、`24381_39597`（5,540），全是负体，
所以 `insts_flat` 上柱的直径只到 5,316。碟同理：492 段那件（48.9 m 封头）
在 Pos 口径下也在（`insts_flat` 的碟上限 156 是它自己那份不完整快照的事，见 §三），
但差别整体来自负体。

**排期口径：按「全部吃 `.mesh` 的」记，即 392 → 474。**

## 三、`insts_flat` 的 11,992 个空数组不是缺陷

顺着上面查下去，把 `inst_relate` 62,824 行里 `insts_flat = []` 的 **11,992 行**
逐行对着回填式的四道过滤分类（脚本 `.scratch/t053/why_empty.py`）：

| 为什么空 | 行数 | 占比 |
|---|---:|---:|
| B. 这个构件的边**全是负体**（Neg / CataNeg / CataCrossNeg / Compound） | 11,979 | 99.9% |
| C. 有 Pos 边但全部 `visible = false` | 13 | 0.1% |
| **F. 有合格边却仍然是空（真·陈旧）** | **0** | — |
| **G. `booled_id` 有效却仍然是空** | **0** | — |

**空数组是这些行的正确终态**：它们本来就没有可渲染的正体。
`insts_flat` 没有坏，它只是不装负体——与 §二 同一件事的另一面。

唯一的真残留在另一头：`insts_flat = NONE` 的 1,479 行里，
**有 40 行是对读者可见的（`aabb.d != none`）**。清扫段的 `WHERE` 恰好是
`insts_flat = NONE AND aabb.d != none`，而 `pdms_inst` 那条 live 断言写的是
「不应残留 `insts_flat = NONE` 且对读者可见的行」——**按它自己的口径，这 40 行是欠的**。
另外 1,439 行不可见，清扫本来就不碰（读侧走 slim 兜底），不算残留。

那 40 行是 dbnum 7997 / 8000 上的实在几何（`CONE` / `BOX` / `NCYL` / `PANE` /
`GENSEC` / `FIXING`），其中三行还带着 `booled_id`（如 `inst_relate:24381_100679`
→ `24381_100679_65`），按修复段本该是 `[{ geo_hash: booled_id }]`。

**这条只报不判**：静态副本看不出「清扫在这份快照之后有没有跑过」，
所以分不清是缺陷还是没跑。已开
**`issues/ISSUE-021-insts-flat-visible-none-residue.md`**（含可直接粘的复现步骤与
三条定性追问），归 ADR-041 / **ADR-043（`insts_flat` 失效协议）** 与
`specs/025-insts-flat-invalidation/` 那条线（另一会话在推），不在 009 范围内。
本文件只用它说明一件事：**009 的爆炸半径不依赖 `insts_flat`，按 `geo_relate` 记。**

## 三、对 T041 / D1 的影响

1. **plan 的 G3 与 D1 的范围**从「柱 1 → 37」改成「五类合计 392 → 474（+82）」。
   D1 拍的「不设独立窗口、V1–V4 合入同批立即重建」不受影响——82 行的量级
   与 37 行同档。
2. **T041 的门要连带扩到五类**：现在的门只写了「不同半径两柱 `geo_hash` 不同」。
   碟要额外钉一条——它的段数是**三元组**（绕轴 / 球冠 / 拐角），只混绕轴那一个
   会让 `(112,28)` 与 `(116,29)` 之外形状相同、经向不同的两件共用一行。
   圆环面同理是二元组（环向 / 管截面）。
3. **`Dish::hash_unit_mesh_params` 那个可疑点仍未验**（T053 第 3 条：哈希未归一化的
   `prad`、落库归一化后的 `prad/dia`）。本次没构造用例，原样留着。

## 附：复现

脚本 `.scratch/t053/{census,scope}.py`（一次性，未入库）。步骤：

```powershell
Copy-Item -Recurse .surreal/ams-7997-e3d-test-20260805 .surreal/scratch-t053-7997
./bin/surreal.exe start --user root --pass root --bind 127.0.0.1:8039 rocksdb:.surreal/scratch-t053-7997
./scripts/Invoke-Surreal8009.ps1 -Endpoint http://127.0.0.1:8039/sql -Sql `
  "SELECT id, param FROM inst_geo WHERE param.PrimLCylinder != NONE OR param.PrimSCylinder != NONE
   OR param.PrimSphere != NONE OR param.PrimCTorus != NONE OR param.PrimRTorus != NONE
   OR param.PrimLSnout != NONE OR param.PrimDish != NONE;"
./scripts/Invoke-Surreal8009.ps1 -Endpoint http://127.0.0.1:8039/sql -Sql `
  "SELECT out, trans.d.scale AS s FROM geo_relate;"
./scripts/Invoke-Surreal8009.ps1 -Endpoint http://127.0.0.1:8039/sql -Sql `
  "SELECT VALUE insts_flat.map(|`$i| [`$i.geo_hash, `$i.transform.scale]) FROM inst_relate WHERE insts_flat != NONE;"
```

`inst_geo` 的 id 是**字符串**（显示成 `inst_geo:⟨2⟩`），`WHERE out = inst_geo:2`
匹配不到任何行且**不报错**——要写 `type::thing('inst_geo','2')`。踩过一次，记在这里。
