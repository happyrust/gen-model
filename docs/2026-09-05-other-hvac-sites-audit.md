# 其他 SITE 通风模型对比测试

后续修复和最新复验见 [2026-09-06 验收报告](D:/work/plant-code/old/gen-model/docs/2026-09-06-hvac-flex-gril-fitt-fix.md)。本文保留修复前统计；下方 GRIL 原因描述已按原生图元证据更正。

2026-09-05。继 `/1CPV-HVACHB` 修复后，检查另外 7 个 SITE 的 9 个范围：SSC 的 3 个暖通 ZONE、CAM、CAV、CCV、CUP、CSV，以及包含 CSV 系统的未命名 SITE `24381/79977`。数据库为 7997、8000。

**未全部通过。** E3D TTY 原生 RVM 中有 16,216 个含几何的元素组；离线生成 14,780 个，其中 14,763 个包围盒匹配、17 个不匹配，另有 1,436 个原生几何记录缺失。额外的 FLEX 中段检查又发现 2 个包围盒匹配但中段缺失的元素。因此，已生成模型共确认 19 个几何异常，其中通风部件为 11 个 FLEX、3 个 GRIL，其余 5 个为范围内的墙、房间面板和 SBFI。

本轮是测试与问题定位，未修改生产 Rust 算法。不能将包围盒匹配或运行时数量匹配表述为曲面、孔洞和内部结构完全正确。

## 原生与离线结果

| SITE / 范围 | refno | 原生含几何 | 已生成 | 包围盒匹配 | 缺失 | 包围盒异常 |
|---|---|---:|---:|---:|---:|---:|
| SSC / 暖通专业SL | 24381/15342 | 1391 | 1173 | 1170 | 218 | 3 |
| SSC / 暖通专业LR | 24381/5010 | 1569 | 1488 | 1488 | 81 | 0 |
| SSC / 暖通专业1SR | 24381/24872 | 914 | 870 | 869 | 44 | 1 |
| /1CAM-HVACHB | 24381/46775 | 558 | 490 | 489 | 68 | 1 |
| /1CAV-HVACHB | 24381/48631 | 2234 | 2090 | 2087 | 144 | 3 |
| /1CCV-HVACHB | 24381/57476 | 4774 | 4277 | 4277 | 497 | 0 |
| 未命名 SITE（CSV 系统） | 24381/79977 | 4305 | 4011 | 4003 | 294 | 8 |
| /1CUP-HVACHB | 24381/100675 | 293 | 262 | 262 | 31 | 0 |
| /1CSV-HVACHB | 24384/24932 | 178 | 119 | 118 | 59 | 1 |
| 合计 | | 16216 | 14780 | 14763 | 1436 | 17 |

915 个 STRT、48 个 TRNS 均生成且包围盒匹配。372 个已生成 BEND 包围盒匹配，但 CSV 范围另有 1 个原生 BEND 未生成。

缺失记录按数据库类型归类：FITT 1188、STWALL 123、PFIT 41、SCTN 28、NOZZ 24、TUBE 21、SNODE 6、ELBO 3、WELD 1、BEND 1。这些是原生几何记录数量，不等同于 1,436 件独立完整设备；部分 TUBE 是导出器合成组。SSC 暖通 ZONE 也包含建筑构件。

包围盒与缺失明细：[differences.csv](D:/work/plant-code/old/gen-model/output/hvac-sites-audit-20260905/differences.csv)。14 个通风部件异常（含包围盒漏检项）：[vent-defects.csv](D:/work/plant-code/old/gen-model/output/hvac-sites-audit-20260905/vent-defects.csv)。全范围统计：[summary.json](D:/work/plant-code/old/gen-model/output/hvac-sites-audit-20260905/summary.json)。缺失身份统计：[identity-summary.json](D:/work/plant-code/old/gen-model/output/hvac-sites-audit-20260905/identity-summary.json)。

## FLEX：11 个中段缺失

| refno | 元素 / 分支 | 缺失中段长度 mm |
|---|---|---:|
| 24381/47029 | /1CAM501FL | 200 |
| 24381/57347 | /-CAV-E-2-H-5A04 下 FLEX | 50 |
| 24381/57379 | /-CAV-E-2-H-5A08 下 FLEX | 50 |
| 24381/88893 | /1CSV110FL | 200 |
| 24381/88902 | /1CSV109FL | 200 |
| 24381/88962 | /1CSV104FL | 200 |
| 24381/88995 | /1CSV103FL | 200 |
| 24381/91023 | /1CSV108FL | 200 |
| 24381/91035 | /1CSV107FL | 200 |
| 24381/91372 | /1CSV106FL | 200 |
| 24381/91466 | /1CSV105FL | 200 |

`24381/47029` 实际只有两端圆柱：局部 Z 范围 `[0,50]`、`[250,300]`。原生 RVM 除两端外还有半径 175 mm、长 200 mm 的 Snout 中段，以及半径 185 mm 的三条薄环。plant-ui 中可见中间断开。

目录诊断定位到 LSNO `15194/5489` `/CFLEX-BODY`：直径与端点参数能求值，但 POFF 表达式未能求值；原始表达式包含尚待核实的 opcode 1002。另有圆柱/薄环条件表达式求值失败。日志表明部分子图元被跳过，顶层 `failed=[]` 并不能证明 FLEX 完整。尚未在本轮完成这些表达式的算法修复，不把 opcode 的推测当成已验证结论。

两个 CAV FLEX 的整体包围盒仍匹配，故补充中段检查：对原生正半径、长于 2 mm 的 Snout，转换到元素局部坐标，确认其轴向后，检查是否存在生成子图元穿过其中平面。25 个 FLEX 中发现上述 11 个缺失；这是针对当前圆形 FLEX 的缺失检测，不是通用曲面相等判定。

[连续性结果](D:/work/plant-code/old/gen-model/output/hvac-sites-audit-20260905/flex-continuity.json)；[plant-ui 实拍：CAM FLEX 断开](D:/work/plant-code/old/gen-model/output/hvac-sites-audit-20260905/plant-ui-flex-47029.png)；[plant-ui 实拍：CAV FLEX 57347 的 50 mm 缺口](D:/work/plant-code/old/gen-model/output/hvac-sites-audit-20260905/plant-ui-flex-57347.png)。后者整体包围盒匹配，但实际显示仍断开。

## GRIL：3 个轴向位置错误

CAV 的 `24381/57342`、`24381/57345`、`24381/57377`：原生含一个 Pyramid 和两个 Box。后续 IDA 分析确认是 LPYR 负高度时反转了错误的轴，使应在 `[-5,-2]` 的图元落到 `[2,5]`，并非额外增加 Box。修复前 `24381/57342` 的实际 plant-ui 截图保留如下。

[plant-ui 实拍：GRIL](D:/work/plant-code/old/gen-model/output/hvac-sites-audit-20260905/plant-ui-gril-57342.png)。

## 同范围的其他异常

| refno | 类型 | 最大包围盒误差 mm |
|---|---|---:|
| 24381/31106 | WALL | 751.753 |
| 24381/31100 | WALL | 736.404 |
| 24381/31126 | SBFI | 1000.002 |
| 24381/24807 | PANE | 6.246 |
| 24384/26250 | PANE | 6837.9 |

以上为范围比较异常，尚未完成逐个算法归因。

## plant-ui 与后端运行时

在隔离的 plant-ui 实际窗口中，完成 CAM FLEX `24381/47029`、CAV GRIL `24381/57342`、CAV FLEX `24381/57347` 的显示、定位和截图。没有将后端 API 调用当成全 SITE 的视觉验收。

重建验证后端后，逐个请求上述 9 个范围的 `/api/v1/model/ensure`。本轮二进制 SHA-256 为 `8c1efe42e9093886dd25d6059bca7ce6e7741fc8652067f0021a5f365001b8cb`，端口 18206，隔离项目命名空间 `strt_cpv_ui_20260905`。记录见 [runtime-build.json](D:/work/plant-code/old/gen-model/output/hvac-sites-audit-20260905/runtime-build.json) 和 [运行时结果](D:/work/plant-code/old/gen-model/output/hvac-sites-audit-20260905/runtime-summary-current.json)。

| 范围 | 重建后运行时结果 |
|---|---|
| SSC / SL | HTTP 500；FITT 24381/31370、24381/31371 引用数据库 6000 无法解析 |
| SSC / LR | HTTP 500；FITT 24381/5335 引用数据库 6000 无法解析 |
| SSC / 1SR | 870 个，身份与离线一致；共有元素范围最大差异 0.001563 mm |
| CAM | 490 个，身份与离线一致；共有元素范围最大差异 0.001719 mm |
| CAV | 首次超时，后续请求 HTTP 500；root 24381/51595 发布事务冲突 |
| CCV | 多次 HTTP 504；未取得全量完成响应，未通过加载验收 |
| 未命名 CSV SITE | HTTP 500；root 24381/100060 发布事务冲突 |
| CUP | HTTP 500；root 24381/100677 发布事务冲突 |
| CSV / 8000 | 请求成功，130 个实例；离线 119 个，共有元素范围最大差异 0.001952 mm |

CSV 的 11 个额外运行时实例已落实：BRAN `24384/26229` 下 6 段 TUBI；WELD `24384/26230`；ELBO `24384/26231`、`24384/26238`、`24384/26239`；BEND `24384/26246`。因此它有 125 个不同 refno、130 个实例，不能把这两种数量混用，也不能把 11 个计数差直接判为 11 个通风形状错误。这些部件尚未在本轮完成运行时对原生的单独曲面验收。前文 1,436 个缺失属于离线生成结果，不能直接当作此后端版本的缺失总数。

运行时记录按生成根读取会包含重叠范围，汇总页面的记录数也不能当成全 SITE 实例数。身份与范围检查已按 refno 归并；原始页面、归并结果保存在成功范围的 `runtime-records-current.json` 和 `runtime-identities-current.json` 中。

后端启动日志明确列出数据库 6000、6001、6003 已声明但当前项目解析器找不到磁盘文件。事务冲突在重建后仍出现，未完成根因修复；早期超时后的重复 ensure 可能影响现场，因此记录的是本次验证环境的失败，不能由此断言所有部署都会失败。测试脚本已改为超时后记录结果，不再用重复提交生成请求轮询同一范围。

测试结束后保存后端日志，重启本次隔离后端，结束未完成的后台生成。原用户后端未操作。上述结果是重启前的测试证据，重启后的内存缓存不代表已重新完成全量生成。

## 测试方法与复现

每个范围均重新用 E3D TTY 只读宏导出 RVM，并保留 `tty.log`、`export.mac`、`native.rvm`、`native.json`、`generated/elements.json` 和 `audit.json`。沿用前次 ida-bridge 核对的图元变换/偏移规则。HVAC representation 6，insulation/obstruction off，tube on；导出设置保存在宏和日志。未执行 SAVEWORK。原生 CAM 导出日志也含目录 PDIA 告警，须与生成器失败区分。

身份匹配使用类型别名、父级路径和同类型序号，并将已知目标 SITE 的终端序号归一化，避免 MDB 与离线数据库中的 SITE 序号差异造成误报。忽略零半径 Cylinder/Snout 占位图元；未把真实正半径的薄环排除。

审计数据根目录为 `output/hvac-sites-audit-20260905`，各范围子目录按 refno 将 `/` 替换为 `_`。

```powershell
rtk proxy python scripts/hvac_sites_audit.py --require-complete
rtk proxy python scripts/hvac_sites_inventory.py
rtk proxy python scripts/hvac_flex_continuity.py --require-complete
rtk proxy python scripts/hvac_vent_defects.py
```

第一项和第三项在当前缺陷存在时应返回非零。重新采集/生成可在第一项加 `--capture --generate`。身份补全脚本在审计脚本之后运行。

本轮重新运行全范围检查和 FLEX 检查，均按预期返回 1；Python 脚本语法编译通过。运行时脚本也返回 1，保留加载失败及数量不一致的结果。
