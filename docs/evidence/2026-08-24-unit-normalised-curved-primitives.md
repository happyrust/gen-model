# 2026-08-24 活库盘点：曲面原语几乎全都是单位几何，段数规则喂到的是单位半径

起因是 T038a 想补一句「椭圆碟活库有没有样本」。样本有——**15 / 17 行是椭圆碟、102 个
实例**，T038a 不是纯防御。但顺着这一列往下看发现了更大的一件事，独立记在这里。

## 结论

`inst_geo.param` 里的 **`PrimDish` / `PrimCTorus` / `PrimRTorus` / `PrimLSnout`（不偏心的）
全部是单位几何**，与早已知道的单位柱、单位球一样。真实尺寸在实例变换的 `scale` 里。

于是 WP-G2 那套「段数由真实半径算出」的权威规则，在生产路径上**拿到的是单位半径**：
`elliptical_dish_facets(0.5, h, 0.5)` 里 `tol/R = 1.0`，角步长直接撞 45° 封顶，
**任何尺寸的碟都得到 8 段**。

也就是说 T038 / T038a 把规则改对了，但改对的规则还没有真正生效——挡在前面的是身份键
（ADR-044 决策 2 / 5，本仓 T041 / plan 的 G3）。**G3 现在写的是「柱与球」，实际是
「所有参与复用的曲面原语」。**

## 证据（库 A：`.surreal/ams-7997-e3d-test-20260805` 只读副本 @8039）

```sql
SELECT param.PrimCTorus.rout  AS r, count() FROM inst_geo WHERE param.PrimCTorus  != NONE GROUP BY r;
SELECT param.PrimRTorus.rout  AS r, count() FROM inst_geo WHERE param.PrimRTorus  != NONE GROUP BY r;
SELECT count() FROM inst_geo WHERE param.PrimLSnout != NONE AND param.PrimLSnout.pbdm = 1.0 GROUP ALL;
SELECT param.PrimDish.prad AS prad, param.PrimDish.pdia AS pdia, param.PrimDish.pheig AS pheig,
       count() FROM inst_geo WHERE param.PrimDish != NONE GROUP BY prad, pdia, pheig;
```

| 变体 | 归一化字段 | 观测 |
|---|---|---|
| `PrimCTorus` | `rout` | 95 行**全等于 1.0**，`rins` 是 0…1 的比值 |
| `PrimRTorus` | `rout` | 167 行**全等于 1.0** |
| `PrimLSnout` | `pbdm` | 112 行**全等于 1.0**（`poff = 0` 的那些；偏心那件在库 B，带真实尺寸） |
| `PrimDish` | `pdia` | 17 行**全等于 1.0**，`pheig` / `prad` 是比值 |

`Dish::gen_unit_shape`（`../vendor/old-aios-core/src/prim_geo/dish.rs`）就是这么写的：
`pdia: 1.0`、`pheig: h/dia`、`prad: prad/dia`；`hash_unit_mesh_params` 只哈希
`theta` / `prad` / `beta`，**不含半径也不含段数**。

## 尺度到底跨多大

```sql
LET $d = (SELECT VALUE id FROM inst_geo WHERE param.PrimDish != NONE);
LET $t = (SELECT VALUE trans FROM geo_relate WHERE out INSIDE $d);
LET $s = (SELECT VALUE d.scale[0] FROM trans WHERE id INSIDE $t);
RETURN { instances: array::len($s), distinct_scales: array::len(array::distinct($s)),
         min: math::min($s), max: math::max($s) };
```

碟：`geo_relate` 边 **102** 条，落在 **22** 个不同的 `trans` 上、**21 个不同的 scale**，
从 **13 mm 到 48,900 mm**（48.9 m 的封头）。

按 `FACET_TOL_MM = 0.5`：

| 直径 | 现在的绕轴段数 | 权威规则应给 | 弦高 |
|---|---:|---:|---:|
| 13 mm | 8 | 8（撞 45° 下限，恰好对） | — |
| 48,900 mm | 8 | **492** | **1,861 mm** |

最大那件的弦高是容差的 **3,700 倍**。这已经不是「跟 E3D 差几段」，是这块几何根本没有
被离散出来。

## 这条改变了什么

1. **G3 / T041 的范围**。plan 的 G3 只写了柱与球，爆炸半径按单位柱的 37 个等价类估。
   实际至少还要加 碟 / 圆环面 / 矩形环面 / 同心 Snout 四类，每一类都要各自数等价类。
   排期与「整库重建」的代价都得重估。
2. **T038 / T038a 的「已完成」要加限定语**。规则是对的、单测是绿的，但**生产路径上还
   没有一个曲面原语真的按真实半径分段**（除了不参与复用的那些）。真值表里的 ✅ 指的是
   「规则正确」，不是「现场生效」。
3. **顺带一个可疑点，没查**：`Dish::hash_unit_mesh_params` 哈希的是**未归一化**的
   `prad`，而 `gen_unit_shape` 落库的是 `prad/dia`。两个几何相似、raw `prad` 相同而
   `dia` 不同的碟会**同键不同内容**——与 snout 那条 T002 已修的双键问题同一形状。
   本次没有构造用例验证，只是读码所见，记在这里。
