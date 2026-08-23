# 2026-08-24 活库盘点：偏心 Snout 出现次数

对应 `specs/009-retire-occ/tasks.md` 的 **T052**，为 **T050**（偏心偏移上下各摊一半）与
**T051**（YOFF 未接通）定爆炸半径。两条缺陷的来历见 T011 的 IDA 记录。

**裁决：T050 不是纯防御，本期必须修。** T051 的问题本盘点**答不了**，见第四节。

## 数据源

两个库，与 2026-08-23 的 T045 盘点同源，互为交叉验证。

| | 库 A（大） | 库 B（小） |
|---|---|---|
| 数据 | `.surreal/ams-7997-e3d-test-20260805` 的副本 | `@8009`（已在跑的实例） |
| 端点 | `http://127.0.0.1:8039/sql` | `http://127.0.0.1:8009/sql` |
| `inst_geo` 行数 | 8,094 | 3,637 |

库 A 的副本落在 `.scratch/snout-census-20260824`，走
`.\scripts\Start-Surreal8009.ps1 -Bind 127.0.0.1:8039 -Datastore rocksdb:.scratch/snout-census-20260824`
起 `bin/surreal.exe` 2.1.4+20250317.45013fc9；命名空间 `1516` / 数据库 `AvevaMarineSample`。
全程只有 `SELECT`，原库未动；盘点结束后进程已停、副本已删。正式库 `.surreal/ams-8009`
两次都没碰（AGENTS.md：已被 3.x 写坏且决定不修）。

## 一、偏心行数

```sql
SELECT count() AS n FROM inst_geo WHERE param.PrimLSnout != NONE GROUP ALL;
SELECT param.PrimLSnout.poff AS poff, count() AS n
FROM inst_geo WHERE param.PrimLSnout != NONE GROUP BY poff;
```

| | 库 A | 库 B |
|---|---:|---:|
| `PrimLSnout` 行数 | 112 | 3 |
| 其中 `\|poff\| > 1e-6` | **0** | **1** |

库 A 的 112 行 `poff` **全等于 `0.0`**——它们是靠 `ptdm/pbdm` 比值区分的单位锥台
（抽样一行：`pbdi -0.5 / ptdi 0.5 / pbdm 1.0 / ptdm 1.875`），正是
`LSnout::gen_unit_shape()` 在 `poff == 0` 时走的那条复用路径。

库 B 那一行是真实尺寸而非单位几何，与 `gen_unit_shape()` 对 `poff != 0` 直接
`self.clone()`、不参与复用的写法一致：

| 字段 | 值 |
|---|---|
| `id` | `inst_geo:⟨8962104800037540133⟩` |
| `poff` | **12.06** |
| `pbdm` / `ptdm` | 66.33 / 84.42 |
| `pbdi` / `ptdi` | −57.600002 / 57.600002（高 115.2） |

## 二、实例数

```sql
LET $g = (SELECT VALUE id FROM inst_geo
          WHERE param.PrimLSnout != NONE AND math::abs(param.PrimLSnout.poff) > 0.000001);
SELECT count() AS n FROM geo_relate WHERE out INSIDE $g GROUP ALL;
```

库 B：**2 个实例**。库 A：0 行 ⇒ 0 实例。

## 三、结论：T050 本期做

一件构件、两个实例，数量上微不足道——但 T050 修的不是精度而是**位置**：
libgm 把 `xShift` 上下各摊一半，本仓（`gen_snout` 与 aios-core 的 `gen_occ_shape`
两条后端一致地）全加在顶圈。对这一件而言就是整体错位 `poff/2 = 6.03 mm`，
远超 `FACET_TOL_MM = 0.5`。这不是「够不够细」的问题，按 FR-010 不能靠放宽阈值过门。

它同时是个**好用的验收样本**：单件、真实尺寸、偏移量足够大，正好拿来当 T050 的
RVM 门（WP-J / T049 的曲面原语抽检里加一条）。库 A 反而给不出这个样本。

> 两个库结论相反这件事本身要记住：T045 那次三个专项在两库都是 0，于是「两库一致」
> 成了默认预期。这次不是。**再有「预期为 0」的专项，一个库查出来 0 不能当证明。**

## 四、T051（YOFF）本盘点答不了

`inst_geo` 里 `PrimLSnout` 的 `poff` 是**单值**，`LSnout` 结构里根本没有第二个方向的
字段。所以「活库里有没有 YOFF ≠ 0 的构件」在 `inst_geo` 上是**问不出来的**——
不管源数据里 YOFF 是多少，落到这张表都只会剩一个 `poff`。查出 0 不构成证据。

能确定的只有偏移方向：两库全部 115 行（112 + 3）的

```sql
SELECT param.PrimLSnout.pbax_dir AS b, param.PrimLSnout.paax_dir AS a, count() AS n
FROM inst_geo WHERE param.PrimLSnout != NONE GROUP BY b, a;
```

都是 `a = [0,0,1]`、`b = [1,0,0]`，没有例外。所以 `poff` 沿局部 X，就是 **XOFF**；
T051 里「`poff` 到底是 XOFF 还是沿 `pbax_dir` 的合成偏移」这个疑问可以关掉——
是 XOFF，而 **YOFF 在数据模型里没有落点**。

要回答「YOFF 值不值得接」，得回到 **dabacon 侧**去数 SNOU 元素的 `YOFF` 属性，
不是查 `inst_geo`。这一步没做。
