# aps250209 / aps250212 对 E3D TTY 导出的对拍截图（2026-10-24）

两库不在 APS 的 MDB `/ALL` 里，项目副本的 SYS 库用 e3d-io 的写路径登记后才导得出（过程见
`docs/plans/2026-10-21-model-remaining-issues-fix-plan.md` §9「M8 · W7t」）。图里蓝色是 E3D 的只读 TTY 导出
（`verification/civil-tty-20261021/aps250209-w7t/`、`aps250212-w7t/`，表现级 LEVEL 6 / HVAC 6），橙色是当前 gen_ams
（e3d-model `fb72d65`）的 `generated.rvm`，都是 RVM 里的 FacetGroup 记录按世界 mm 渲染，不经任何中间模型。
对拍总账：aps250209 4583 对、4564 exact、off 19、只在 E3D 40、种类不同 2；aps250212 0 对、只在 E3D 6
（`verification/regress-20261021/aps_survey_aps250209-w7t.txt`、`aps_survey_aps250212-w7t.txt`）。

| 图 | 内容 |
|---|---|
| `01-stair-flight-strflt-facet-e3d-vs-ours.png` | `STRFLT 1 / 2 of SBFRAMEWORK /A01-AREA05-BASE-FL-STAIRS`：E3D 给梯段自身写一条面片（踏步 + 斜梁），我们只画它名下的 HANDRA（只在 E3D 18） |
| `02-stair-handrail-top-end-9mm.png` | `HANDRA 1 of STRFLT 1`：我们的扶手沿路径到 36454.07、E3D 到 36445.00（梯段顶边 36455.00），顶端多伸 9.07 mm（off 16） |
| `03-arc-kickplate-handrails-only-in-e3d.png` | `HANDRA 1–5 of SBFRAMEWORK /A01-AREA05-2ND-FL-HANDRAILS`：带圆弧踢脚板的 1 / 2 我们生成失败（`layout kickplate does not support arc segments`，只在 E3D 22） |
| `04-ctorus-equipment-template-radius.png` | `CTORUS 1 / 2 of TMPLATE 1 of EQUIPMENT /A02-AREA05-PRIMARY-CRUSHER`：同一根半圈管带 NCTO + 2 只 NRTO 负体，E3D 的布尔结果是内侧半管（盒 ±1968）、我们的是上段 + 碎块（盒 ±2377.56，off 409.56） |
| `05-terrain-genpri-only-in-e3d.png` | aps250212 `TRNMOD /TerrainA/DEF01/MOD01 → TRNCC 1 → TRNCEL 1..6`：E3D 每格一片 GENPRI 地形网格，我们对 TRNMOD 子树一件都没画（只在 E3D 6） |

重画：`python output/w7d/render_w7t_pngs.py`（读上面两处 RVM，需要 matplotlib）。

## 修后（2026-10-24，`*-fixed.png`）

五堆都修了（e3d-model `df70971` 扶手端向 + 圆角踢脚板、`eb7e633` STRFLT、`87c0acb` 进了布尔的 CTOR / RTOR 粗格、`0b7b1b5` 地形 GENPRI
的缓存面片）。同一套图换成 gen_ams `w7t8` 的 `generated.rvm` 再画一遍：`python output/w7d/render_w7t_pngs.py docs/assets/2026-10-24-aps2502xx-tty-compare w7t8 -fixed`。
对拍总账：aps250209 4623 对、4622 exact、off 1（`/AREA07-CV_0001/Belt` 的整圈 REVO，E3D 拆两条写，ISSUE_09 既有类）、只在 E3D 0、
种类不同 2（同一类）；aps250212 6 对全 exact（`aps_survey_aps250209-w7t8.txt`、`aps_survey_aps250212-w7t8.txt`）。

| 图 | 内容 |
|---|---|
| `01-stair-flight-strflt-facet-e3d-vs-ours-fixed.png` | STRFLT 自己一条面片（斜梁 + 踏步 + 平台板），世界盒对 E3D 0.05 mm |
| `02-stair-handrail-top-end-9mm-fixed.png` | 扶手顶端按 RPATH `DRNE` 竖切，末端 y = 36445.00 与 E3D 重合（+0.00） |
| `03-arc-kickplate-handrails-only-in-e3d-fixed.png` | HANDRA 1–5 全出，踢脚板在圆角拐角按拐点斜接 |
| `04-ctorus-equipment-template-radius-fixed.png` | 正环管与负体同一张 10 mm 粗格，布尔结果与 E3D 同形（盒 ±1968，体积 8.962e7 对 8.962e7 mm³） |
| `05-terrain-genpri-only-in-e3d-fixed.png` | 6 格地形的缓存面片照 E3D 写成 6 条 FacetGroup，多边形数 / 环数 / 包围盒逐格相同 |
