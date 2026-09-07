# /1CPV-HVACHB 全 SITE 模型核对

2026-09-05。SITE `24381/76564`，AMS 数据库 7997、session 106，MDB `/ALL`。

**结论：SITE 尚未全部通过。** E3D 原生 RVM 有 878 个含几何的元素；生成器和实际 plant-ui 后端均生成 806 个，另有 72 个 FITT 缺失。本轮修复了额外发现的两件 TRNS 变形。806 个已生成元素全部通过身份、位置和外形范围核对，最大范围误差 0.002346 mm；这个结果不等同于逐面曲面/孔洞完全一致的证明。

## 范围和结果

沿用 STRT 修复时 E3D TTY 导出的整个 SITE：HVAC representation 6，insulation/obstruction off，tube on，holes on。原生文件包含 1280 个组，其中 878 个含几何。生成器遍历 1337 个数据库元素；组、数据库元素和有几何元素的数量不能混用。

| 类型 | E3D 有几何 | 已生成且范围通过 | 缺失 |
|---|---:|---:|---:|
| EXTR | 296 | 296 | 0 |
| CYLI | 200 | 200 | 0 |
| SCTN | 89 | 89 | 0 |
| STRT | 82 | 82 | 0 |
| BEND | 50 | 50 | 0 |
| BOX | 48 | 48 | 0 |
| TAPE | 11 | 11 | 0 |
| OFST | 10 | 10 | 0 |
| BRCO | 8 | 8 | 0 |
| DAMP | 7 | 7 | 0 |
| TRNS | 2 | 2 | 0 |
| CAP | 2 | 2 | 0 |
| THRE | 1 | 1 | 0 |
| FITT | 72 | 0 | 72 |
| 合计 | 878 | 806 | 72 |

完整逐元素结果：[audit.json](D:/work/plant-code/old/gen-model/output/cpv-site-audit-20260905/audit.json)。每个匹配元素保留 refno、名称、原生期望范围、生成范围和误差；无多余生成元素。

核对按名称、未命名元素序号及 OWNER 路径关联，统一 RVM 的长类型名。把原生图元变换到生成元素局部坐标后比较范围。Cylinder、Snout、Pyramid 使用原生参数计算范围，其余图元使用导出面片。验收阈值 1.1 mm，实测最大仅 0.002346 mm。此流程覆盖全部 878 个原生可见元素的存在性及 806 个匹配元素的范围，不声称完成每张面的拓扑、内部重叠、法线和孔洞等价验证。

离线生成必须加载 `ZDJ/ZDJ000/zdj7600_0001`：AMS 引用的 7600 库属于 ZDJ。同数据库编号的 AvevaPlantSample 文件不能替代它；漏配会额外漏掉 89 个 SCTN。实际 `/ALL` 后端可正确解析这 89 个 SCTN。

## 本轮修复：两件 TRNS

| 元素 | 支路 | 修复前最大范围误差 | 修复后 |
|---|---|---:|---:|
| 24381/76829 | /-CPV-S-2-L-1306 | 60.000100 mm | 0.000200 mm |
| 24381/76836 | /-CPV-S-2-L-1307 | 60.000145 mm | 0.000288 mm |

正确局部整体范围为 `[-165,-165,0]` 到 `[165,165,600]` mm；旧模型横向错误扩展到 ±225 mm。

目录 `/SQRD-BODY1`（LSNO `15194/4150`）的 POFF 使用 SQRT。E3D TTY `Q POFF` 已直接取得该公式：本样本的偏移量为 `sqrt(125² + 125²) = 176.776695... mm`。二进制公式流末尾 opcode 1001 原先未被解析。PTCA 的 PTCD 还使用 opcode 906（ATAN）；只补 SQRT 会保留错误的方向，必须同时恢复角度计算。ATAN 返回角度值，不能返回弧度。

另一个错误是 `required_number` / `optional_number` 在显式公式无法求值时，把占位的全零 tuple 当作合法零值。现在只有 `AttributeNumber::NotNumeric` 才允许沿用无公式的零 tuple 兼容路径；`Unresolved` 保持错误。

修改位置：

- [catalogue_pml.rs](D:/work/plant-code/old/vendor/e3d-io/src/record/catalogue_pml.rs)：SQRT / ATAN 解析、渲染和数值求值。
- [catalogue.rs](D:/work/plant-code/old/vendor/e3d-model/src/catalogue.rs)：限制零 tuple 回退。
- [strt_rvm.rs](D:/work/plant-code/old/vendor/e3d-model/tests/strt_rvm.rs)：两件真实 TRNS 回归，保留原有 STRT 原生圆柱/法兰回归。
- [e3d_model_service.rs](D:/work/plant-code/old/gen-model/src/fast_model/e3d_model_service.rs)：生成指纹增加 `catalogue-pml-sqrt-atan-v1`，使旧缓存失效。

证据：[SQRT TTY 输出](D:/work/plant-code/old/gen-model/output/cpv-site-audit-20260905/trns-sqrt-tty.log)、[目录属性和公式诊断](D:/work/plant-code/old/gen-model/output/cpv-site-audit-20260905/trns-catalogue.log)、[修复前核对](D:/work/plant-code/old/gen-model/output/cpv-site-audit-20260905/before-sqrt-audit.json)。补充 ATAN 的 TTY 查询进程退出异常，未取得有效输出；ATAN 依据公式流及修复后的原生 RVM 对比验证，未把该失败查询当作证据。

## IDA 对原生几何约定的核实

通过 ida-bridge 对已有 `expdri.dll.i64` 做只读反编译：`expdri_DRSNOU` 地址 `0x1000175c`，`expdri_DRPYRA` 地址 `0x10004636`。确认偏心图元的两个端面中心采用正负半偏移，不能把底面中心放零、顶面放全偏移。旧 RVM 面片转换器的这种约定偏差会造成 OFST 假阳性，因此本核对脚本直接按原生参数计算 Snout/Pyramid 范围。

[IDA 导出证据](D:/work/plant-code/old/gen-model/output/cpv-site-audit-20260905/ida-expdri-snout-pyramid.txt)。没有修改 IDB，也没有对 E3D 设计数据库执行 SAVEWORK。

## 尚未修复：72 个 FITT

这 72 个元素是 SCTN 下的结构附件，其中 66 件有名称、6 件按类型/序号/OWNER 路径标识。例如 `24381/76998`，`/-RX-CPV-S2001-V1/F1`，OWNER 为 `24381/76997`。它们在 E3D 原生 RVM 中含几何，在数据库树中存在，但生成报告列入 `catalog_pending`，实际后端没有对应实例。

原因在建模覆盖范围：`category.rs` 将 FITT 分类为 Catalog，但 `is_model_unit` 不接纳 FITT，`pipeline.rs` 将其计入待实现目录类型。这不是显示开关或网格下载失败。本轮没有实现该结构附件类型，因此必须保留 SITE 不通过状态。

[全部 72 件缺失清单（CSV，含 refno 和 OWNER）](D:/work/plant-code/old/gen-model/output/cpv-site-audit-20260905/missing-fitt.csv)。已检查清单名称集合与 RVM 缺失集合完全相同。

## 测试和实际 plant-ui 验证

- `e3d-io cargo test --lib`：228 通过。
- `e3d-model cargo test --release --lib`：242 通过。
- `e3d-model cargo test --release --test strt_rvm -- --ignored`：2 通过，使用本机真实 AMS/CATA 数据，包含两件 TRNS 和原有六件 STRT。
- 新后端编译成功，运行于 `http://127.0.0.1:18206`，命名空间 `strt_cpv_ui_20260905`。整个 SITE 强制生成 806 个实例，280 个生成根。
- 实际 plant-ui 对两件 TRNS 分别执行“重新生成模型”和“定位模型”，观察方圆过渡、两端法兰及相邻风管连接，并保存截图。原有 82 件 STRT 的数值核对继续通过。
- 实际后端逐根读取模型记录，验证 806 个有实例元素的全部网格文件存在，72 个 FITT 无实例；两件 TRNS 的世界范围与修正后的离线结果一致。

[运行时验证结果](D:/work/plant-code/old/gen-model/output/cpv-site-audit-20260905/runtime-audit.json)记录后端二进制 SHA-256。此验证使用独立实例；用户原来的 9099 后端未替换。

![TRNS 24381/76829](D:/work/plant-code/old/gen-model/output/cpv-site-audit-20260905/plant-ui-trns-76829.png)

![TRNS 24381/76836](D:/work/plant-code/old/gen-model/output/cpv-site-audit-20260905/plant-ui-trns-76836.png)

复跑脚本：[cpv_site_audit.py](D:/work/plant-code/old/gen-model/scripts/cpv_site_audit.py)、[cpv_site_runtime_audit.py](D:/work/plant-code/old/gen-model/scripts/cpv_site_runtime_audit.py)。`cpv_site_audit.py --generate --require-complete` 会重新生成并核对；只要仍有缺失、额外元素或范围失败就返回非零。当前 `--require-complete` 返回 1 是正确的不完整状态，不能改为通过。
