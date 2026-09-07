# AMS 管道 `data` 槽位复核（2026-09-06）

## 结论

AMS7997、AMS7999、AMS8000 的 2428 条 PIPE BRAN 中，审计报告剩余的 `data` 均可由原始路由关系或 E3D 原生门控解释，未发现新的生成算法缺陷。

`data` 是“槽位没有生成隐式 TUBI，但原因来自数据或原生规则”的分类，不表示整条 BRAN 生成失败。三库所有 PIPE BRAN 的 `bad/account` 仍为 0。

## 全量槽位分类

| 原因 | 槽位数 | 复核结论 |
|---|---:|---|
| built | 6955 | 满足原生规则并生成 TUBI |
| zero length | 8677 | 两端重合，没有可绘制管身 |
| misaligned | 877 | 两端未通过 E3D 的双端共轴门，正确省略 |
| reversed | 731 | 到达点位于离开方向后方，正确省略 |
| native head filtered | 70 | 原生头段条件不成立，正确省略 |
| missing P point | 20 | 缺少构造管身所需的目录 P 点，未臆造几何 |
| 合计 | 17330 | 与三库 PIPE 路由槽位总账一致 |

## E3D TTY/RVM 对照

既有原生导出已经覆盖比本次残余样本更严格的边界，并确定隐式管身的门控：离开端和到达端均须共轴、轴向为正，横向偏移必须严格小于轴向距离的 1‰。1.000 mrad 恰好不绘制。

- `24384/22803 /M-BR-1R330-E1`：2998 mm 反向槽位；原生为 17 FTUB、0 TUBE。
- `24384/23903 /M-YE-1R320-C`：包含 178° 反向、13° 和 90.5° 偏轴槽位；原生为 8 FTUB、0 TUBE。
- `24384/24609 /M-IY-1R322-B`：1000 mm 反向槽位；原生为 2 FTUB、0 TUBE。
- `24384/2500 /C-CO-1RX-3AH0A`：2000 mm 上横偏 2 mm，即 1.000 mrad；原生不绘制该尾管。
- `24384/2944 /C-GR-1RX-3020B` 与 `24384/3277 /C-GR-1RX-3000A`：反向和偏轴槽位被省略，满足门控的细管仍与原生分别达到 6/6、1/1 exact。

原生取证详见 `../vendor/e3d-model/evidence/20260902-ams8000-bran-audit/rvm-pending-items-2026-09-02.md`。本轮对 `/C-CO-1RX-3AH0A` 的追加导出在 E3D 选择 CE/导出阶段停滞，没有产生新的 RVM，因此没有把该次尝试计入证据；结论使用已落盘的原生 TTY/RVM 结果。

## plant-ui 复核

- `/M-BR-1R330-E1`：查询到 17 个元素、17 个网格实例，与原生 17 FTUB、0 TUBE 一致；反向空档没有补出 TUBI。
- `/M-YE-1R320-C`：查询到 8 个元素、8 个网格实例，与原生 8 FTUB、0 TUBE 一致；三段反向/偏轴空档均未补管。
- 既有 `/C-IY-1R330-B` 对拍为原生/生成 35/35 exact。该 BRAN 同时含 `missing P point` 槽位，说明此分类只是阻止无依据的隐式管身，不会丢失可生成的真实构件。

截图：

- `output/ams-pipe-tray-audit-20260906/plant-ui-m-br-reversed-isolated.png`
- `output/ams-pipe-tray-audit-20260906/plant-ui-m-ye-misaligned-isolated.png`

## 判定

当前剩余 `data` 可保留为源数据诊断信息。它们不应改成错误，也不应通过放宽共轴门、翻转方向或猜测 P 点来强行生成 TUBI，否则会重新引入 E3D 原生 RVM 中不存在的管身。

本轮结果已固化到 `scripts/verify_ams_pipe_tray_regression.py`。脚本离线检查三库 PIPE、桥架总账和原生证据；默认还会调用 plant-ui 使用的模型服务，逐一确保并检查 `/M-BR-1R330-E1`、`/M-YE-1R320-C`、`/C-IY-1R330-B`、`/C-OR-1R345-C` 与 AMS7999 阀门分支。2026-09-06 实跑结果为 `status: passed`，机器报告位于 `output/ams-pipe-tray-audit-20260906/regression-verification.json`。
