# AMS 管道与桥架模型审计（2026-09-06）

## 结论

- 管道生成侧缺陷已经清零：AMS 7997、7999、8000 共审计 2428 条 PIPE BRAN，未发现 `bad` 或 `account`。
- 桥架生成侧全量通过：13 个 CWAY 设计库、1365 条 CWBRAN、2067 个 CTELE；其中实际存在的 1652 个 CTRAY 全部成功生成，无几何 0、错误 0。
- 不能把管道报告里的 `data` 当成生成器错误。它表示设计库原始连接关系中的反向、错位、零长、缺 P 点或原生头段过滤。生成器保留这些事实并阻止无依据管身进入模型。

## 修复内容

1. 隐式 TUBI 只在 E3D 原生绘制条件成立时生成；反向、错位、头端管径不匹配、头端方向未设置或头端 stub 为空的槽位不再显示错误管身。
2. 目录 PML 操作码按 IDA/Core3D 取证修正：`1005 = ABS`，`1008 = MAX`，用于桥架横档与轮廓表达式。
3. `PTAX.PAXI = Pn` 可递归引用目录 P 点的位置和方向，同时支持负向点编号，递归深度受限。
4. `ATTRIB DESP[n]` 对未存储但由 DTRE 声明的数值槽位按 0 求值。`DPRO` 不作为 PTRE 的运行时替代值。
5. 桥架表示过滤按 E3D 的 `LEVE/OBST/TUFL/CLFL` 与 `RPRO COVR/OUTA/OUTB` 处理。

## IDA 与 E3D 原生证据

- Core3D `getattlib_des/DESPRE` 的原始 DESP 分支在属性未存储时仍报告 unset；目录表达式层负责数值空槽语义。
- AMS7999 分支 `24383/74956` 中，阀 `/1RCV071VP` 的 ATT 显示 `DESP=unset`。其 PTRE 为 `DESP[1] + PARA[2] * 0.5`，`PARA[2]=230`。
- 原生 E3D RVM 中阀前后管段长度为 129.997589 mm、126.001907 mm；修复后的生成值为 129.997584 mm、126.001907 mm，最大绝对差约 0.000005 mm。该分支 7 个槽位全部为 `built`。
- 管道 `/C-OR-1R345-C`：原生 RVM 与生成模型 9/9 边界对拍通过；生成树为 7 FTUB + 2 BEND，错误 TUBI 为 0。
- 桥架 `/C-IY-1R330-B`：原生 RVM 与生成模型 35/35 边界对拍通过。

## 全库结果

| 设计库 | PIPE BRAN | 路由成员 | 生成成员 | 生成侧 bad/account |
|---|---:|---:|---:|---:|
| AMS7997 | 614 | 7676 | 7061 | 0 |
| AMS7999 | 1256 | 11321 | 7659 | 0 |
| AMS8000 | 558 | 2606 | 2321 | 0 |
| 合计 | 2428 | 21603 | 17041 | 0 |

注：三库总 BRAN 数为 2646，其中 218 条属于 HVAC 容器（7997 为 216 条、8000 为 2 条；此处表格只统计 PIPE）。HVAC 已在独立全 SITE 审计中验证，不能用管道目录审计的 `db_not_open` 评价其几何。

管道源数据账：

| 设计库 | ok | data | 主要 data 槽位 |
|---|---:|---:|---|
| AMS7997 PIPE | 502 | 112 | misaligned 62、reversed 105、missing P point 2、zero length 1633、native head filtered 9 |
| AMS7999 PIPE | 508 | 748 | misaligned 633、reversed 506、missing P point 12、zero length 4534、native head filtered 31 |
| AMS8000 PIPE | 334 | 224 | misaligned 182、reversed 120、missing P point 6、zero length 2510、native head filtered 30 |

桥架全量覆盖：

- 设计库：251142、251144、251145、251146、251149、251150、251151、251156、251157、251158、251159、251160、251161。
- CWBRAN 1365；CTELE 2067；POSREF 缺失 0；放置错误 0。
- CTRAY 1652；成功 1652；无几何 0；错误 0。

## plant-ui 验证

- `/C-OR-1R345-C`：树展开 9 个真实成员，查询 9 个元素和 9 个网格实例，未显示错误 TUBI。
- `/C-IY-1R330-B`：展开 36 个子元素，查询 35 个元素和 35 个网格实例，与原生 RVM 计数一致。
- AMS7999 `=24383/74956`：展开 13 个成员，按需生成 1/1；查询 14 个元素和 18 个网格实例，界面日志 ERROR 0，隔离视图中的管路、阀门与三通连续。
- `/M-BR-1R330-E1`：查询 17 个元素和 17 个网格实例，与原生 17 FTUB、0 TUBE 一致；2998 mm 反向槽位未生成错误 TUBI。
- `/M-YE-1R320-C`：查询 8 个元素和 8 个网格实例，与原生 8 FTUB、0 TUBE 一致；178° 反向及 13°、90.5° 偏轴槽位均未生成错误 TUBI。

## 残余 `data` 复核

三库 17330 个 PIPE 路由槽位已完成原因归账：built 6955、zero length 8677、misaligned 877、reversed 731、native head filtered 70、missing P point 20。E3D 原生 RVM 已覆盖反向、偏轴、1 mrad 边界、头段过滤和缺 P 点场景；plant-ui 的反向与偏轴样本计数也与原生完全一致。未发现新的生成算法缺陷，详见 `docs/2026-09-06-ams-pipe-data-recheck.md`。

## 自动验证

- `e3d-io`: `db_param_env` 7/7；桥架 PML `ABS/MAX` 1/1。
- `e3d-model`: catalogue point 3/3；route 28/28；cable tray 8/8。
- 发布版 `aios-database` 编译通过并已重启到 plant-ui 验证环境。
- `scripts/verify_ams_pipe_tray_regression.py` 将三库 PIPE 总账、13 库桥架总账、原生 E3D 证据锚点、五个 plant-ui 运行时样本及截图合并成失败即退出的回归门；结果写入 `regression-verification.json`。

## 证据文件

- `output/ams-pipe-tray-audit-20260906/bran-7997-final/`
- `output/ams-pipe-tray-audit-20260906/bran-7999-final/`
- `output/ams-pipe-tray-audit-20260906/bran-8000-final/`
- `output/ams-pipe-tray-audit-20260906/ctray-all-cway-generate.txt`
- `output/ams-pipe-tray-audit-20260906/native-1rcv0243a/`
- `output/ams-pipe-tray-audit-20260906/plant-ui-c-or-isolated-final.png`
- `output/ams-pipe-tray-audit-20260906/plant-ui-c-iy-3d.png`
- `output/ams-pipe-tray-audit-20260906/plant-ui-db7999-valve-isolated.png`
- `output/ams-pipe-tray-audit-20260906/plant-ui-m-br-reversed-isolated.png`
- `output/ams-pipe-tray-audit-20260906/plant-ui-m-ye-misaligned-isolated.png`
- `output/ams-pipe-tray-audit-20260906/regression-verification.json`
