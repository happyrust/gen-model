# 通风 FLEX、GRIL 与支架 FITT 修复验收

2026-09-06。继续原 STRT / CPV 修复，完成此前其他 SITE 测试发现的 11 个 FLEX 中段缺失、3 个 GRIL 轴向错误，并修复 SCTN 所属 FITT 的安装标架。原始数据库只读，原生基准保留；没有执行 SAVEWORK。

## 结果与范围

- 其他 7 个 SITE 的 9 个范围：915 STRT、48 TRNS、25 FLEX、79 GRIL 均通过原生 RVM 边界对照；25 FLEX 的中段连续性全部通过。
- CAM、CAV、CCV、未命名 CSV SITE、CUP 的 1,001 个 SCTN/FITT 组合全部通过。安装修复前，按相同组合口径有 965 个边界不匹配。
- 原始 CPV SITE 重生成：806 个组合模型全部通过边界对照，其中 82 STRT、2 TRNS；72 个 FITT 已包含在所属 SCTN 内。
- 为避免长梁掩盖子件位置错误，额外逐个匹配全部 1,073 个 FITT 的独立生成组件与原生 Box：边界和体积均通过，最大边界误差 0.0014992 mm，最大相对体积误差约 2.35e-13。
- 实际 plant-ui 验收了 FLEX、GRIL、CAM 支架。后端运行时复验 14 个 FLEX/GRIL 和 5 个 SITE 的支架样本，均与已通过原生对照的离线网格边界一致。运行时比较误差小于 0.002 mm。

上述边界对照阈值为 1.1 mm；不能据此宣称全部曲面、孔洞和内部拓扑完全相同。运行时的 0.002 mm 是后端与离线网格之间的误差，不是所有曲面与原生解析体的误差。

其他 SITE 的最新组合口径统计如下。原生独立 FITT 组归入实际 SCTN 属主后进行比较，不再将已合并的子件计为缺失。

| 范围 | 生成模型 | 边界通过 | 原生缺件 | 边界差异 | 待核对 |
|---|---:|---:|---:|---:|---:|
| SSC 暖通 SL | 1173 | 1170 | 218 | 3 | 0 |
| SSC 暖通 LR | 1488 | 1488 | 81 | 0 | 0 |
| SSC 暖通 1SR | 870 | 869 | 44 | 1 | 0 |
| CAM | 490 | 490 | 2 | 0 | 0 |
| CAV | 2090 | 2090 | 0 | 0 | 0 |
| CCV | 4277 | 4277 | 28 | 0 | 0 |
| 未命名 CSV SITE | 4011 | 4011 | 3 | 0 | 0 |
| CUP | 262 | 262 | 0 | 0 | 0 |
| CSV / 8000 | 141 | 118 | 37 | 1 | 22 |
| 合计，不含 CPV | 14802 | 14775 | 413 | 5 | 22 |

仍未全部通过：5 个差异为 `24381/31126` SBFI、`24381/31106` 和 `24381/31100` WALL、`24381/24807` PANE、`24384/26250` PANE。CSV / 8000 的 22 个模型受当前原生几何审计能力限制，保持 `oracle_review`。413 项仍是原生几何记录缺失，不能视为本轮已解决。

## 算法与证据

**FLEX 表达式**：`e3d-io/src/record/catalogue_pml.rs` 接入 POW(1002)、MIN(1009)、GT(601)、MAT(1308)、TRIM(1314)、逻辑 STR(1410)。求值器保留数字、逻辑、字符串的类型，未知表达式和非有限结果继续失败，不用默认数字代替。MAT 返回从 1 开始的匹配位置，未找到返回 0；GT 是严格大于。

TTY 实际读出 `/CFLEX-BODY` 的 `SQRT(POW(DESP[10],2)+POW(DESP[11],2))`，以及柔性接头延伸部分的 `MAT(TRIM(STR(DESP[15] GT 150)),'true')` 和 `MIN(DESP[15],150)`。原生阈值探针确认 150 不通过、151 通过。core.dll 的表达式分派与 EXPOW 支持这些解释。

**GRIL 负高度 LPYR**：Core3D `QLPYRA@0x103C07B4` 在 PTDI−PBDI 为负时调用 `HCHECK@0x103C56B4`，交换上下尺寸并反转 PBAX，保留 PAAX 和有符号轴向中心。修复 `catalogue.rs` 后，原生端点区间 `[-5,-2]` 不再落到 `[2,5]`。更正先前报告的描述：原生有一个 Pyramid 和两个 Box；问题是 Pyramid 翻到错误一侧，并非生成器多加了一个 Box。

**SCTN/FITT 安装**：取消按单个 AMS8000 样本推定的固定 Rx(+90°)。`section.rs::section_fitting_placement` 按 POSS→POSE、SCTN.BANG、中性轴 PLAX、POSL 安装线、JUSL 对齐、ZDIS 和 DELP 构造标架。安装线的 DX/DY 进入倾斜方向；原点是安装线与轴向距离平面的交点，再加旋转后的 DELP。FITT.BANG 在安装标架上旋转。

IDA 对应路径：`GFIORI@0x103B27D4`、`GFIORG@0x103B370C`、`GFIPOS@0x103B60AC`、`GSPLNU@0x1039C5C8`、`GPLSOF@0x103A35C8`、`GSPVEC@0x103A38FC`。GFIORI 的 ORNMAT 以安装线法向作为局部 Z，以安装线方向作为局部 Y。`elmodl.rs` 用此标架放置已求值的 FITT 目录几何。

反例 `24381/47187` / `24381/47188`：原生端板为 370×25×110 mm，世界中心 `(-18780,-45272.5,6344.92)`；新算法的三个目录轴为 `(+Z,+Y,-X)`，与原生 RVM 一致。定向 TTY 查询确认 SCTN.BANG=-90、JUSL=NA，FITT.POSL=NA、ZDIS=0、DELP=0、BANG=0。先前宽泛查询导致原生进程异常退出，改用这些定向属性查询后正常完成，未修改数据库。

生成指纹加入 FLEX、LPYR 和 FITT 算法版本，防止沿用旧模型缓存。当前验证后端为 `output/hvac-flex-gril-fix-20260905/aios-database-fitt.exe`，端口 18206，项目命名空间 `strt_cpv_ui_20260905`。

## 自动检查与复现

- PML 模块 20 个测试通过，包含 3 个从真实目录程序提取且修复前失败的回归测试。
- catalogue 模块 29 个测试通过，包含 LPYR 负高度端点回归。
- section 模块 10 个测试通过，包含原生 CAM 安装轴、中心点及倾斜安装线/ZDIS/DELP 回归。
- `gen_ams` release 构建与 `aios-database` 构建通过；根仓库及 e3d-model 的 `git diff --check` 通过。

工作目录为 gen-model。先构建实际依赖 `../vendor/e3d-model` 的 `gen_ams`，目标目录指定 `D:/work/plant-code/old/target-e3d-model`，随后运行：

```text
python scripts/hvac_flex_gril_regression.py
python scripts/hvac_owned_fittings_audit.py
python scripts/hvac_cpv_regression.py
python scripts/hvac_fitting_parts_audit.py
python scripts/hvac_flex_continuity.py --audit-root output/hvac-flex-gril-fix-20260905 --require-complete
python scripts/hvac_flex_gril_runtime.py
python scripts/hvac_fitting_runtime.py
```

运行时检查需要本任务的 18206 验证后端。TTY 探针为 `hvac_flex_gril_tty_probe.py` 和 `hvac_fitting_tty_probe.py`。原始独立组审计仍保留在各范围的 `audit.json`；新组合口径在 `owned-fittings-audit.json`，映射在 `owned-fittings-mapping.json`，逐 FITT 对照在 `fitting-parts-audit.json`。隔离的 PML 反事实副本仅为定位证据，不是生产依赖。

全部本轮证据位于 [输出目录](D:/work/plant-code/old/gen-model/output/hvac-flex-gril-fix-20260905)。

## plant-ui 实拍

![FLEX 中段连续](D:/work/plant-code/old/gen-model/output/hvac-flex-gril-fix-20260905/plant-ui-flex-57347-fixed.jpg)

![GRIL 负高度实体归位](D:/work/plant-code/old/gen-model/output/hvac-flex-gril-fix-20260905/plant-ui-gril-57342-fixed.jpg)

![SCTN 支架端板方向修复](D:/work/plant-code/old/gen-model/output/hvac-flex-gril-fix-20260905/plant-ui-fitt-47187-fixed.jpg)
