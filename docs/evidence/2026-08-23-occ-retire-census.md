# 2026-08-23 活库盘点：退役 OCC 的 `PdmsGeoParam` 分布与段数口径爆炸半径

对应 `specs/009-retire-occ/tasks.md` 的 **T045**（原 T029 / FR-008），
决策依据 ADR-030（IDA 修订二）与 ADR-044。

## 数据源

正式库 `.surreal/ams-8009` 已被 3.x 写坏且决定不修（AGENTS.md），因此盘点走**副本**：

```powershell
Copy-Item .surreal\ams-7997-e3d-test-20260805\* .scratch\occ-census-20260823\ -Recurse
.\scripts\Start-Surreal8009.ps1 -Bind 127.0.0.1:8039 -Datastore rocksdb:.scratch/occ-census-20260823
```

服务端 `bin/surreal.exe` = `2.1.4+20250317.45013fc9`（与 `Cargo.lock` 同 rev）。
命名空间 `1516` / 数据库 `AvevaMarineSample`。查询一律经
`scripts\Invoke-Surreal8009.ps1 -Endpoint http://127.0.0.1:8039/sql`。

原库为只读来源，全程未写；盘点结束后副本与服务进程已清理。

原始导出留在 `docs/evidence/2026-08-23-occ-retire-census/`：
`cyl-diameters.json`（295 个去重直径）、`cyl-diameter-histogram.json`（直径 → 实例数）。

> 同日另有一次独立盘点（数据源 `@8009`，`inst_geo` 3,637 行、单位柱 99 实例），
> 结论记在 `specs/009-retire-occ/plan.md` 的「盘点结论（@8009 只读）」小节。
> 两份的**定性结论一致**，定量爆炸半径因库规模不同而不同（7 vs 37 个段数等价类）。
> 排期以本文件这份（较大的库）为准。

## 一、`inst_geo` 变体分布（8,094 行 = 去重后的单位网格身份数）

```sql
SELECT (IF param = NONE THEN '<absent>'
        ELSE (IF type::is::object(param) THEN array::first(object::keys(param))
              ELSE type::string(param) END) END) AS variant,
       count()
FROM inst_geo GROUP BY variant;
```

| 变体 | `inst_geo` 行数 |
|---|---:|
| `PrimExtrusion` | 3,896 |
| `<absent>`（无 `param`，布尔产物 / 复合） | 2,942 |
| `PrimLoft` | 567 |
| `PrimRTorus` | 167 |
| `PrimPyramid` | 158 |
| `PrimLSnout` | 112 |
| `PrimCTorus` | 95 |
| `PrimLPyramid` | 77 |
| `PrimRevolution` | 61 |
| `PrimDish` | 17 |
| `PrimBox` | **1** |
| `PrimLCylinder` | **1** |

`PrimSphere` / `PrimSCylinder` / `PrimPolyhedron` / `Unknown` / `CompoundShape`
**在本库中一行都没有**。

箱与柱各只有一行，正是单位网格身份（ADR-026）的直接体现：全库所有箱共用
`inst_geo:⟨1⟩`，所有圆柱共用 `inst_geo:⟨2⟩`。

## 二、两个「假回退」的实测确认

```sql
SELECT param.PrimExtrusion.cur_type AS ct, count()
FROM inst_geo WHERE param.PrimExtrusion != NONE GROUP BY ct;
-- → [{ ct: "Fill", count: 3896 }]

SELECT param.PrimRevolution.rot_dir AS axis, count()
FROM inst_geo WHERE param.PrimRevolution != NONE GROUP BY axis;
-- → [{ axis: [1.0, 0.0, 0.0], count: 61 }]
```

- **样条轮廓：0 行。** 3,896 条挤出全是 `CurveType::Fill`。与「整个工作区没有一处
  构造 `CurveType::Spline`」的静态结论一致。T036 因此是纯防御性实现，不是补生产缺口。
- **出平面回转轴：0 行。** 61 条回转的 `rot_dir` 全等于 `[1, 0, 0]`（即 `Vec3::X`）。
  与「唯一构造点走 `Default`」以及「libgm 的 `GM_Revolution` 只接受平面内轴角」两条
  一致。T033 改 `bail!` 不会打掉任何现存构件。

## 三、段数口径的爆炸半径（ADR-044 决策 2 的判据）

实例级数据来自 `inst_relate.insts_flat[]`（`{geo_hash, transform.scale}`）。
圆柱的 `get_scaled_vec3` 是 `(pdia, pdia, height)`，单位柱半径 0.5，
故实际半径 = `scale[0] / 2`。

| 量 | 值 |
|---|---:|
| 展平后的实例条目总数 | 214,847 |
| 箱实例（`geo_hash = '1'`） | 16,725 |
| **圆柱实例**（`geo_hash = '2'`） | **21,354** |
| 圆柱的不同直径数 | 295（6 mm … 5,316 mm） |
| 这些直径按 `circle_segments(r, 0.5)` 落到的**不同段数** | **37**（8 … 164） |

段数取值：8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64, 68, 72, 76, 80,
84, 92, 96, 100, 104, 108, 112, 116, 124, 128, 132, 136, 140, 144, 152, 156, 160, 164。

**结论：改身份键后圆柱从 1 行涨到 37 行**，不是理论上限 249，更远低于「按真实半径逐个
建」。相比表里已有的 3,896 行挤出，37 行可以忽略。ADR-044 决策 2 成立。

## 四、当前写死 32 段到底错得有多厉害

按实例数加权（同一份 `.mesh` 被 21,354 个实例复用，但每个实例的正确段数由自身直径定）：

| | 实例数 | 占比 |
|---|---:|---:|
| 段数恰好该是 32（当前正确） | 429 | **2.0%** |
| 该比 32 多（当前过粗） | 1,525 | 7.1% |
| 该比 32 少（当前过细） | 19,400 | 90.8% |

**98% 的圆柱实例拿到的段数与 E3D 不同。** 而 `cancelFacets` 只消全等重叠
（`plant-4/libgm-boolean-algorithm.md` §6.11），段数不等就不抵消。这条不是画质项，
是布尔正确性项——足以把 ADR-044 从「值得做」推到「不做就没法说 manifold 路径对齐了
E3D」。

九成是**过细**而非过粗，说明现状也在白白多算三角：小口径管道（DN15 一类）E3D 只给
8–12 段，我们给 32。

## 五、对计划的影响

1. T033（出平面轴 `bail!`）、T036（样条→弧形墙截面）确认为纯防御，可以放心做，
   不会打掉现存构件，也不需要等 RVM 门。
2. T041（单位网格身份键带段数）的代价已量化为「圆柱 1 → 37 行」，可以本期做。
3. 本库没有 `PrimSphere` / `PrimSCylinder` / `PrimPolyhedron`，它们的段数改动
   （T038 / T041 的球那一半）在本库上**无法验收**，需要另找含这些原语的库，
   或只以纯函数单测收口并在 plan 里注明未经现场验证。
4. `<absent>` 的 2,942 行（布尔产物）不经 `tessellate_libgm_param`，与本期无关。
