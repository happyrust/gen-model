# 模型问题跟踪清单（2026-11-06）

依据：39 站全量对拍（最初是 p2a 加上 e3d-model `46f8d8f`，共 196,329 对；现行基线 `b39/rref3`，见「回归门」）、e3d-model 与 gen-model 的 open issue。
状态栏：`待办` / `进行中` / `已修待推` / `待确认` / `已关`。改动后在「最近进展」里记一行日期和结论。

> 2026-11-13 晚按 GitHub 票面逐张核过状态栏（下面各表与「最近进展」第一条）。还开着的问题怎么排，见 Cursor 里的
> 开发计划「模型生成遗留问题开发计划（2026-11-13）」（计划文件不在仓库里）与 `docs/plans/2026-11-13-0.1.45-swap-checklist.md`；
> 计划里「开发计划 E1–E9」指要 E3D 或真机、执行前逐项批准的那一段，「D1–D13」是它的决策点。

## 一、几何对不上（39 站对拍）

| 优先 | Issue | 问题 | 涉及件 | 站 | 状态 | 下一步 |
|---|---|---|---|---|---|---|
| — | e3d-model#21 | SJOI 斜交连接梁的 IJOINT 平移：缺 GJTORG 第二次求交的闭式 | 9 件 off（3.4–12.7 mm）：`/MDS/VT01`、`VT02`、`VT05`、`FT13` 下的 `IJOINT1/2/3` | `23713/2343`、`15516/2`、`23734/1` | 已关 | `222a0ef`（main）：`GJTORG` 收尾按反编译重写（`JFRE` / 梁认领门 + 两次求交）。`rref3` 里这几站的 IJOINT 全 exact（如 `23713_2343` 的 `/MDS/VT05/2-BAR-1/S1/IJOINT1`、`/MDS/FT13/2-BAR-1/S1/IJOINT1`）。遗留的 perp 分支经验规则记在二节 |
| — | e3d-model#23 | HVAC 头槽位断面解不出时，E3D 写半径 0 的圆柱占位，我们记 unbuildable 不写 | 1 件只在 E3D：`TUBE 1 of BRANCH /-CAM-E-2-H-5302` | `24381/46775` | 已关 | `8e3cf67`（已推 main）：照 E3D 写 0 径 Cylinder 占位，只在 E3D 1 → 0 |
| — | e3d-model#24 | 隐式管身某段 E3D 不画，我们照画 | 1 件只在我们：`TUBI 9 /Copy-of-1RCS007MN-TUBE/801VP`（2315 mm） | `24381/101405` | 已关 | `5182aef`（已推 main）：共轴门在相对 1 mrad 之外再加绝对横偏不超过 1 mm。TUBI 9 两端只偏 0.61 mrad，横偏 1.405 mm。2026-11-07 普查 ams7997/7999/8000 全部 7902 个非零长槽位：过相对门的 6835 个里横偏超过 1 mm 的只有它和一根双方都不画的头管，E3D 画到 1.000000 mm。偏轴 ATTA、空离开键两条假设此前已否证；影子工程把 BEND 106139 原地挪动再 SAVEWORK，SPAMAP 没被重算、TUBI 9 仍不画，SPAMAP 那条线索与此无关。39 站对拍 only_generated 1 → 0，逐对零变化 |
| — | e3d-model#25 | 零长 SCTN 名下的 SJOI J1 E3D 照画，我们整串 skipped | 2 件配不上：`/MDS/SPECIALS/PIPE/3-BAR-2/S1/J1`、`4-BAR-2/S1/J1` | `23714/1` | 已关 | `b94615c`（已推 main）：零长轴取 +Z，两只 J1 exact，不可配对 2 → 0。规则只有 2 个样本，且 DRNS 恰为 +Z，遇到 DRNS ≠ +Z 的零长 SCTN 时复核 |
| — | e3d-model#22 | RTOR 内半径大于外半径时 E3D 照画，我们判退化 | 2 件（RTORUS 1/2 of SUBE 10 of `/Copy-of-R43-R540-008`） | `24381/101405` | 已关 | `46f8d8f`（github main）；39 站零退化 |
| — | e3d-model#26 | FRMW 名下 PNOD 里的 PJOI `GTYP` 为空值时，E3D 有时照画、我们整串 skipped | 3 件配不上：`/MDS/FT62/1-V1/JOINT1`、`/MDS/FT64/1-V1/JOINT1`、`/MDS/FT64/1-V2/JOINT1` | `15516/2`、`23734/1` | 已关 | `5775ecb`（已推 main）：判据是宿主 SCTN 的 `RREF` 有没有指到这只 PJOI 的宿主 PNOD。`GTYP` 空 + RREF 命中 → 画（ams7324/7350 各 3 只）；`GTYP` 空 + RREF 不命中 → 不画（ams7324 FT61 的 18 只、ams7999 的 500 只、ams8000 的 248 只，与 E3D 一致）。39 站零回归，6 件 unkeyable → exact。`RREF` 是新增读点，已进读取集合（181）与 `ancestor_reads_guard` 的 `WithinUnit` 登记 |

| — | e3d-model#27 | TUBI 取不到目录件外径时回落到达侧通径，半径差 3–9 mm | 162 对 `off`（全是 TUBI） | `24383/73927`、`24383/66456`、`24381/101405`、`24381/177373` | 已关 | **不是代码缺陷，是验证脚本用法错**：`b39_run.ps1` 给 `gen_ams` 传了两个 `--project-dir`（ams000 + acp000），`acp7000_0001` 与 `ams7000_0001` 撞号，`DirectoryDbResolver` 判「分不出正身，整组不外交」，库 7000/7600/6000 全解析不出，目录件取不到 → `tubes_catalogue_od=0` → 管子回落通径。改成「主项目目录 + `--main-project-dir`」走 `adopt_foreign_dbs` 后：`24383/66456` 的 `generated` 1155 → 4116（与 E3D 成员数一致）、`failed` 1605 → 0、`tubes_catalogue_od` 0 → 1422、对拍全 exact。39 站全量：`off` 162 → 0、`only_baseline` 5441 → 0、`exact` 191946 → 199180，两侧都有的键里零变差 |

### 可接受的残差（不单独跟踪）

| 判档 | 件数 | 内容 |
|---|---|---|
| chord | 5 | 2026-11-06 `revert1106` 重数后剩下 5 条（#21 落地前表里的 10 含当时的 IJOINT 弦，那些已变 exact）：`24381/34109` FLOO 2 `/1RX-FN03-FN` 2.1816 mm、CFLO 7 of `/1RX-FN07-FN` 0.0811 mm、CFLO 1 of `/1RX-FN11-FN` 0.0536 mm；`24381/42520` GWAL 6 `/1RB-WF04-W-C-RR001` 0.4130 mm；`24381/57476` TUBI 1 `/-CCV-E-3-L-1704` 0.0694 mm |
| reference_artifact | 4 | `/1RX-FN09-F001` FLOOR 2、`/1RX-FN03-FN` FLOOR 5、`/1RS-WF03-F-C-F001` FLOOR 12、`/1RS-WF05-F-C-F001` FLOOR 9 |
| film | 3 | `/1RX-FN01-F001` FLOOR 12、`/1LR-WF05-S-C` FLOOR 6、`/1RS-WF03-W-C-RR001` GWALL 13 |
| kind_differ | 4 | `23705/289` 的 `/MDS-Standards-ST59-PS`、`GT85-PS`、`GT86A-PS-1`、`GT87A-PS-1`：几何 exact，E3D 写一只布尔后的 FacetGroup，我们按图元写 Box/Cylinder/Facet（或 Pyramid）。原因在目录负体：ST59/GT85 的法兰孔 NSCY 是 `TUFL=false`，被我们的显示筛收走；腹板孔 `PDIA = PLOD + DESP[19]`，E3D 日志自己报 `DESP parameter number (19) out of range`；GT86A/87A 的 NSCY `PDIA = RPRO PLOD`，LSTU 链不在会话里时 POD 取 0，刀退化。我们一把刀都没扣上（`negatives_applied` 0），就走了按图元写出。E3D 侧本身报目录错，不是 e3d-model#7 的回归；2026-11-06 实验（负体只按 LEVE 筛、不按 TUFL 筛，39 站 xneg）：23705/289 kind_differ 4 → 2（ST59/GT85 改为整只面片），但 `23710/1` 新增 276 对 kind_differ，几何判档全不变。所以 E3D 对 TUFL=false 负体并非一律照扣，假设否证，不改。2026-11-06 干净构建复测（全部取已提交版本）：这 4 对已消失，但同一站出现 8 只 Z 偏移件（ST59/GT85 基准 z 0..200、生成 84.86..336.5；GT86A/87A 基准 z -100..0、生成 -184.5..-59.15），根因是 PLOD 依赖工作区未提交的 `vendor/e3d-io` `db_param_env.rs`（LSTU 链走不到时 POD 取 0）与 `vendor/e3d-model` `catalogue.rs` 的 `void_negatives`；这两处已提交（e3d-io `b6ff292`、e3d-model `022132c`）。2026-11-06 用当前 `gen_ams` 对 `acp7321` 的 `=23705/289` 重导，对上这份基准：32 对全 exact，off 0，8 只 Z 偏移没有回来 |

### 待核实

- [x] 约 1,240 条只在基准（`24383/66456` 976 等）是 #27 之前目录库撞号的旧账，不是还要逐条排除的残差。#27 已经记过 `only_baseline` 5441 → 0。2026-11-06 `revert1106`（撤回偏轴门之后的 `gen_ams`，39 站，`--project-dir` 与 `--main-project-dir` 都是真实 `ams000`）复现：exact 199180、off 0、kind_differ 0、only_baseline 每站 0、only_generated 合计 1（仍是 #24 那只 `TUBI 9 /Copy-of-1RCS007MN-TUBE/801VP`，站 `24381/101405`）。chord 10 → 5（见上表），reference_artifact 4 与 film 3 与表内名单相同。

## 二、其他模型几何问题

| 优先 | Issue | 问题 | 状态 | 下一步 |
|---|---|---|---|---|
| P3 | e3d-model#20 | SCTN 端关节在 mode 0 下 NAPP 2 刀还切不切：13 处关节缺 E3D 真值 | 待办（缺 E3D 真值，开发计划 E2） | 螺栓板那半已对上：影子工程重新导出 `23708/2046`、`23713/3470`、`23734/1124`（都是 `/MDS/FT17/2-V1`），对当前 `gen_ams` 三只都 exact、偏差 0 mm。端板那半出不了真值：`aps7166` / `aps7505` 不在 APS 的 MDB `/ALL` 里（打开 `/ALL` 会改其他成员的时间戳，这两份文件的时间和字节都没变）。按名字进 `/Endplates-Column-JOINT`、按 `=23550/1` 导出、以及 `!!CE.db().mdb().dblist()` 都是 des 0xC0000005（未挂载库上的导航是原生崩溃，ONERROR 接不住）。`Q MDB` 能打出 `MDB: /ALL`。`gen_ams --root =23550/1` 自己能跑（visited 1147、generated 305），没有 E3D RVM 可对。不再重试这三条宏 |
| P3 | e3d-model#21 遗留 | perp 分支（`\|v44·v38\| ≤ 5e-6`）整支留站位是经验规则：其中 42 件反编译可证（第一次求交分母为 0），另 100 件（VT10 的 `*-BAR-*`、FT04H / FT04V 的 `*-V1`）按反编译应能求交、E3D 仍留站位 | 已核实，不改 | 2026-11-06 用 `sjoi_gjtorg_probe --all --verbose` 对真实 `ams7329` / `ams7324` / `ams7350` 各配对应 E3D RVM（根 `23713/2343`、`15516/2`、`23734/1`）。这三库是同一批 MDS 关节的副本。每库 perp 约 65–68，其中走完求交的 33–36 只（名字在该根 RVM 里的那部分；探针跳过 RVM 里没有的名字，所以不是 100 只全量）。走完的那些，关节局部点乘等于世界点乘（例如 `/MDS/FT04V/9-V1` 两者都是 1.0，混坐标系是 −0.707；`/MDS/VT10/1-BAR-1` 两者都是 −0.4996，混坐标系是 0）。「perp 是跨坐标系点乘造成的」不成立。现行留站位与 E3D 一致（ours=ok），按反编译去求交则预测 off。`datum.rs:717` 不动 |
| P3 | gen-model#63 余项 | aps7200 `/INNER-KICKPLATE` 差 5.63 mm | 待办（开发计划 E5） | 在 E3D 里造同形样本（要批准） |
| P3 | gen-model#63 余项 | FN09 楼板 | 待办（开发计划 E5） | 等你在 E3D 里按修正清单改完后重导、对拍 |
| P4 | gen-model#62 | 对拍覆盖面扩展：MDS AISC / DIN / SPECIALS、ams8000、APS、ams2510xx 的 FIXING 真值 | 本地 tag 已打，未推 | 第 0–5 步都已完成。本地 tag `v0.1.43` → `cb0fbd8a5`、`v0.1.44` → `83aa3c250`。不推：`v*` 会触发 `windows-binary.yml` |
| P4 | gen-model#86 | PML 未实现操作码里，只有 1012 有几何消费者 | 已修已推（票未关；开发计划 D8 推荐关票） | 2026-11-11 核：1012 的修复是 e3d-io `74bd1ee`（11-06），已在 GitHub `main`；单测 `ptca_py_real_is_the_expore_cast`。其余 23 组没有 PTCA / PTMI / PTAX，保持拒读。票面当晚补了这条记录；关不关等人定 |
| — | e3d-model#16 | 算法文档（持续更新，不算缺陷） | 持续 | — |
| — | e3d-model#29 | APS 电缆桥架三处：`CTSTRA` 直段断面取错（148 件 off）、弯头锚点取错端（off）、`RADI ≠ 0` 的圆弧弯头 `CTBEND` / `CTRISE` 一件不画（67 件只在 E3D） | 已关（已推送） | 断面 `e2a596c`（已在 GitHub main）、锚点 `db65e1c`、圆弧弯头 `df3cbe5`（后两个只在本地 main）。三个桥架根 aps250179 `=2013286723/4`、aps250176 `=2013286720/1`、aps250182 `=2013286726/1` 的 off / only_baseline 都归零（833 对全 exact），仍判 fail，差的是 e3d-model#32、#33。2026-11-10 15:56 已由另一个会话关单，三个根转 pass 的事归这两张票 |
| — | e3d-model#30 | APS 管道支撑 `/PS-*` 沿 z 平移 28–162 mm（36 件 off）；关单时两根仍 fail：13 只柱脚垫板 `PJOI`（宿主立柱是 `GENSEC`）E3D 画了、我们没画 | 已关（已推送） | z 平移 e3d-io `03414fd`，垫板 e3d-model `42661d6`，都只在本地 main。aps250104 `=2013286648/1`（194 / 194）、aps250102 `=2013286646/1`（48 / 48）转 pass |
| — | e3d-model#31 | SLCY 斜切圆柱 2 件从 `Snout` 退成 `FacetGroup`、底端差 11.71 mm | 已关 | `560cc2c`（已在 GitHub main）。aps250154 `=2013286698/1` 635 / 635 exact |
| 待定 | e3d-model#32 | 电缆几何缺省不生成：APS 三个桥架根 46 根电缆（`/CBLP-*`、`/CBLC-*`）只在 E3D 基准里，记成发不出主键的基准成员 | 已关（11-11，已推送） | 修复在 `main@b999e6c`。遗留（接头转弯处的局部偏差；穿过 OFFS 型节点或带偏移 `RATTA` 的电缆没有样本）在 e3d-model#40 |
| 待定 | e3d-model#33 | 路由节点标记不生成：同三个根 28 个 `*-RNODE-*` 标记只在 E3D 基准里（基准里各 3 条几何记录） | 已关（11-11，已推送） | 带规格的 28 个节点按目录件出实体，与 E3D 一致（`main@b999e6c`）。遗留（不带规格的 83 个节点 E3D 画线框）在 e3d-model#41 |
| 待定 | e3d-model#34 | 两处规则照样本接：竖直关节朝向按梁的种类分支（`42661d6`：`GENSEC` 取宿主 Y，`SCTN` 取世界 +X）、立弯「重复最低一层」按层数分支（`df3cbe5`：单层不重复，多层重复） | 已关（11-11，已推送） | 主关节朝向照 Core3D 重写（`main@b999e6c`）。遗留在 e3d-model#42（原点）、e3d-model#43（E3D 画、我们不画）、e3d-model#44（没有依据或样本的几支） |
| 待定 | e3d-model#36 | aps250110 `=2013286654/1` 仍判 fail：`GENSEC 1 /AREA02_PR_AXIS_P2.2`（`2013286654/1821`）两端各比 E3D 短 0.54 mm | 已关（11-13 推送） | `43d0bbd`（型钢上刀面与宿主面夹出的薄楔不让刀，照 libgm 按原尺寸布尔），2026-11-11 并进本地 `main@9b74e66`。这根梁 off → exact（差 0.0000016 mm），APS 39 根 fail 1 → 0，AMS 56 根逐项相同。还差：推送；带修复的后端编出来后用 rvm-check 复跑 aps250110。墙的样本对 0.051 的疑问另开 e3d-model#48 |
| 待定 | e3d-model#37 | 桥架托盘空间里 E3D 有、我们没有的分支：多层非直角立弯、钝角立弯、CTCOUP、圆截面（`VSHAPE`）、斜三通、偏心变径等 | 已关（11-13，照 Core3D 的建体规则补齐） | 2026-11-10 反向分析 Core3D.dll 的两个建体器得到。现有 APS / AMS 数据里都没有出现，不影响现在的根。各个旋转落在哪根轴没有读到，逐项要有样本才做 |
| 待定 | e3d-model#38 | SLYK acw6015：6 个 BEND E3D 画了、我们生成失败 | 已关（11-13 推送） | `e9df8ef`：悬空的 `SPRE` 记目录链断，BEND 照 E3D 写占位记录。SLYK 106 个单元配对全 exact。2026-11-11 并进本地 `main@9b74e66`。还差：推送；换后端后看 SLYK 卡住的四个根是否照常发布（按代码推的，没实测） |
| 待定 | e3d-model#39 | SLYK acw7015：1 个 OLET 没成体（`PTPOS` 整类没接） | 已关（11-13 推送）；留下的增量缺口另开 gen-model#161 | `22771b2`，接了「取连接支管头 / 尾」的那一种。留一个在案的增量缺口（支管挪了、OLET 不动时不重算，与 gen-model#143 同形）；`PTSET_EVAL_VERSION` 1 → 2；其余四种写法没接 |
| 待定 | e3d-model#45 | 目录引用指到 MDB 之外的库时整件记失败（SLYK 15 只 NOZZ） | 已关（11-13 推送） | e3d-model `2af2525` + e3d-io `71ac836` + gen-model 服务侧 `outside_mdb`（主工作区，未提交）。还差：推送；换后端后看那三台设备是否放行 |
| 待定 | e3d-model#42 | 主关节（PJOI）原点：被接的梁与关节轴不平行时偏 2–10 mm | 已关（11-13 推送）；只有反编译依据的几只归 e3d-model#44 | `91e0713`、`186873b`。ams251173 一只（修复后挪 20.5 mm）与 ams251183 `/402-STRCT` 六只只有反编译依据，E3D 基准已排进 11-11 晚的取证 |
| 待定 | e3d-model#43 | E3D 画、我们不画的主关节：ams251183 两个 ZONE 320 只里 204 只 | 已关（11-13） | 2026-11-11 晚开工：先找 Core3D 判「画不画」的反编译依据，不按样本凑规则 |
| 待定 | e3d-model#44 | 主关节朝向修复的遗留：没有 E3D 依据或没有样本的几支 | 待办（缺真值，开发计划 E3） | 与 e3d-model#43 同一位在做：有依据的实现，缺样本的列清单，没依据的如实写 |
| 待定 | e3d-model#40 | 电缆在接头转弯处与 E3D 有局部偏差：46 根里 29 根，最大 149 mm | 已关（11-13，`059915b`） | 对拍只比包围盒，所以三个桥架根现在是通过。转弯这部分有样本；OFFS 型节点、带偏移 `RATTA` 的没有样本 |
| 待定 | e3d-model#41 | 不带规格的路由节点 E3D 画 19 条线的线框，我们不出（APS 三个桥架库 83 个） | 已关（11-13，出线框） | RVM 写出器加线图元、对拍能比只有线的组。CNODE 没有样本 |
| 待定 | e3d-model#46 | 管外径（POD）取不到时按 0 求值，报告里不留记录 | 已关（11-13，`967886a`） | 诊断入口 e3d-io 里已有，没人调 |
| 待定 | e3d-model#47 | 支撑件夹在电缆桥架上时管外径取不到、按 0（APS 32 条）：E3D 取什么值没查过 | 已关（11-13；e3d-io `f4da465`：取不到时取离开点口径，不再按 0） | 排在 11-11 晚的取证里 |
| 待定 | e3d-model#48 | 墙上 22 µm 的贴面皮 E3D 吞了，对的是 0.051；元素建模路上读到的归一化容差是 0.01 | 待办（缺样本，开发计划 E4） | 2026-11-11 新开。读墙、板扣负体时建的是哪一类组合、按哪个容差。现在没有不过的件 |
| — | e3d-model#53、#55、#56、#57、#61 | 吊架拉杆只写离开侧的槽位（#53）；退化环拉伸、无规格电缆从「失败」改成「不出几何 / 只画中心线」（#55、#56）；管夹箍带分段（#57）、圆管型钢内外弧各按各自半径分段（#61） | 已关（11-13 推送，e3d-model `main@643b7b2`） | 包围盒看不见的那几项（#57、#61）要用 gen-model#127 的体积比对复核 |
| — | e3d-model#59 | 直墙、楼板、面板上成员配件开的洞 | 已关（11-13，`a0ff056`，提交标题写着「未验收」） | 只接了 `STWALL` 的 FITT / CMFI、`FLOOR` / `PANE` 的 PFIT；没覆盖的两类另开 e3d-model#63、#64 |
| 待定 | e3d-model#54 | aps7343 四份基准实为 aps7324 同名 ZONE 的导出（按名字补导认错了库） | 待办（不是生成缺陷） | 基准登记加注；两道 rvm-check 护栏另开 gen-model#162、#163 |
| 待定 | e3d-model#58 | 梁端连接件模板里的端板没有螺栓孔（360 块、1774 个孔） | 待办（缺样本，开发计划 E1） | 板上 mode 7 没有样本，要在 E3D 里造 |
| 待定 | e3d-model#62 | GENPRI 面片：aps250162 15 种里 3 种形状错（环被填实、细圆棒只剩两头），12 种分格不同 | 待办（开发计划 2.3） | `src/genpri.rs` 的三角化；真值是现成的 E3D 面片，`tests/fixtures/genpri/` 有 4 种夹具 |
| 待定 | e3d-model#63 | aps7200 三块板经 `PANE → CMPF → SBFI` 的开孔没开；`CFRA → COFI` 跨板开孔也没接 | 待办（开发计划 2.1，11-13 新开） | `host_fitting_mode` 不认这条成员链 |
| 待定 | e3d-model#64 | 三堵弧墙上经 FIXING 挂的门窗没开洞（aps250110、ams7997） | 待办（开发计划 2.2，11-13 新开） | 弧墙只收自带 `DESP` 的套管式 FIXING，没收 FIXING 的目录负体 |

## 三、生成流程与线上落地

| 优先 | Issue | 问题 | 状态 | 下一步 |
|---|---|---|---|---|
| P1 | gen-model#88 | 批次 worker：7997 `model_only` 停在 `durable_pending`（6772/6772 已 ACK），挡住后面排队的 7998 / 8000 | 已关（11-11） | 卡点是栅栏之后的 `refresh_equip_derived`。已改成独立阶段 `equip_derived`、5 分钟预算、整库或根数 ≥ 2000 跳过空间反查、节点按 1000 分块，`/health.equip_derived` 报推迟数。全库单测 2844 通过。2026-11-07 本机 8022 换成本分支二进制，#73 触发 7997 整库重排：6772/6772，`durable_pending` 15.5 s，`equip_derived` 115.8 s（`library_scale`，推迟 0），8 分 25 秒收尾，7998、8000 随后出队 |
| — | gen-model#73 | 初始化时，对「模型目标没变」的根沿用旧 vendor 生成的行，所以换了 e3d-model 的几何修复也不会落地 | 已推送、本机已验，线上未部署；还差开发计划 D6（推荐不做「只重盖章的根单独记」，换后端后关） | `43dd85c5a`（ADR-093）：生成器指纹改读 vendor 构建身份，水位行加库级落款 `model_generator`，落款不一致就整库重排；在 `origin/publish` 分支上，但 11-05 那次 publish 构建失败（CI 从 `ci/gen-model-linux-20260918` 拉的 e3d-model 只到 `b9324e0`，缺 `e3d_model::pipeline::Stop`），最后一次成功部署仍是 10-26 的 `83aa3c250`，不含本修复。2026-11-07 本机 8022 验过：首启日志点名 7997/7998/8000 `reason=model_generator_changed`，三库 `dbnum_watermark.model_generator` 改为新指纹，再重启 `generator_stale_dbnums` 为空、不再排。同日补漏：指纹原先只含 e3d-model，现加 e3d-io、e3d-attlib 两段（未提交），本机同样验过首启重排、再启不排 |
| P2 | gen-model#74 | 一根 failed 让整库 lagging，`model/records` 对整库返回 0 条（ams7330：364 根里 4 根失败） | 已推送（`origin/refactor`），真机只验了一部分；还差开发计划 D7（推荐接受第 ② 点以单测为准，换后端时验第 ① 点） | 按根回读和 `ready_source` 已在 refactor。初始化回执对 `projection.partial` 的最后一条 `Generated` 写成 `published survivors; {reason}`，单测 `published_survivors_are_named_on_the_generated_unit_only` 在全库 2844 里通过。2026-11-07 本机 8022 用 `AIOS_TEST_FAIL_ROOTS=24384/2529` 让 8000 整库重建里一根整根失败：任务 `partial`，其余 953 根就绪，对照根 `24384/2541` 照常读到 6 条，失败根继续给出库里旧的 8 条；进程内该库判 lagging（追赶 1 根），去掉钩子重启后三库 `in_sync`、8000 `durable_ready_total` 954/954。钩子不接初始化路径，7330 式「部分件失败」只有单测，没有本机复现。没关 GitHub |
| P2 | gen-model#78 | release 二进制在范围生成中段 `0xc0000409` 崩溃，每次同一偏移，根因未定 | 已关（11-13）：根因是整机可提交内存耗尽时的分配失败（`docs/analysis/2026-11-12-issue78-crash-root-cause.md`） | 几何池和主 rayon 池 64MB 栈，`the_geometry_pool_runs_on_large_stacks` 通过。崩溃笔记记当前根、阶段、工作集；分配失败、栈溢出、访问违例和 `SIGABRT` 写 `logs/crash-notes.log`。发布脚本留行号 PDB，`Start-AMS` 默认 `RUST_MIN_STACK` 128MB，并有 `-EnableCrashDumps`。没在生产进程上复现 |
| P3 | gen-model#72 | 模型侧生成失败没有停跑和退避的记录，和数据批次的 `DATA_BATCH_PARK_AFTER` 不对称 | 已关（11-13 推送） | 按根 30 秒起、翻倍、10 分钟封顶，只活在进程里；退避中的根仍挡水位，本轮不生成、不增加尝试、不把退避再往后推。新的 `source_end_sesno` 立刻复活。`/health.model_root_backoff` 报样本。`backing_off_roots_skip_generation_without_pushing_their_backoff_further` 在全库 2844 里通过。ADR-090 正文不在这个 worktree，追记写在 changelog。2026-11-07 本机注入一根失败后观察 7 分钟：没有新窗口时这根不会被自动重试，所以退避只在第一次失败后的 30 秒里出现在 `/health.model_root_backoff`；重复尝试下的退避节奏只有单测 |
| P3 | gen-model#34 | 结构连接级联（JOIS/JOIE ↔ CREF）默认关闭，关节负体落地后存在正确性缺口 | 待定（开发计划 D5：本轮只做门 (ii)，门 (i) 后半归 E6；翻缺省的补丁在 `opt/flow-next-c2-default-on-0003`） | 同库 FIXING ↔ GENSEC、同库 SJOI ↔ SCTN 在开关关着时已经由 e3d-model `plan_update` 把对端根放进 regen（`structural_connection.rs` 文件头）。开关还补得上的是 PJOI/SUBJ → SCTN、没有标架样本的 SJOI、`CMPF.CFRA`，以及跨库交接。语料目标已定位、还没改位：SCTN `=23734/3557`（`/MDS/FT54A/4-BAR-2`）的 JOIE 是 PJOI `=23713/5493`（PNODE `=23713/5492` 的唯一成员），CREF 指回这根梁，跨库 7329↔7350。短名 `/MDS/FT54A` 和 `COLL ALL SJOI` 会让 des 0xC0000005；按 refno 和成员路径查询正常。没有 SAVEWORK，默认仍是 false（`options.rs`、`DbOption.toml`、ADR-080、CONTEXT）。不补 NGMR→GENSEC 边 |
| P1 | gen-model#142 | 「重新生成模型」、带「一并删除网格文件」的清理、升级清库，会把别的根、别的库的 `geo_relate`、`tubi_relate` 行一起删掉 | 已关（11-13 推送） | 2026-11-11 下午单独合进主工作区并换上本机后端；换前换后各一次只读体检，没查到被带走的行。还差：提交、推送。已经被带走的行不会自己回来，正式库要不要体检在票里跟 |
| 待定 | gen-model#146 | 库没初始化时在节点上按需生成过，之后这个库初始化被拒 | 已关（11-13 推送） | 入口先清后建（清完到复查之间落进来的行再清，最多三遍）、两条清理接口如实回报、文档改口。主工作区未提交。换后端那次重启就是线上验证（现场那个库由系统自己清残留再初始化） |
| 待定 | gen-model#147 | 整库模型清理在已初始化的库上清不掉管身行：`tubi_relate` 的复合键落不进按 Ref0 的字符串区间，清完的核对也数不到 | 已关（11-13 推送） | 2026-11-11 新开，读代码发现。先把单测里的管身行改成生产的 id 形状（应当先红），再按 `dbnum` 列删 |
| — | gen-model#148、#149、#150、#151、#154、#155、#156 | 关节级联认主关节（#148）、不成单元的关节变了被切的型钢重算（#149）、宿主向名下另一件单元取刀的两条级联（#150 / #156）、整库重建清库先顺指针删（#151）、整库重排停下时说出是哪个根（#154）、成员边超过一笔事务上限时分笔写（#155） | 已关（11-13 推送） | — |

## 四、增量更新

| 优先 | Issue | 问题 | 状态 |
|---|---|---|---|
| P2 | gen-model#75 | BRAN 只改 HPOS：增量回执 derived=1，但管子没有变化，网页和 RVM 都不变 | 已关（11-13 推送） |
| P3 | gen-model#76 | 夹具 data：只改 NAME 再改回，恢复后仍留下一条 Pos 几何关系 | 已关（11-13 推送） |
| P3 | gen-model#45 | 核查 Extract（_NNNN）和 Global FLUSH 引起的主库会话重编在增量链上的表现 | 待办（开发计划 E8，要一个能起来的 Admin） |
| 待定 | gen-model#132 | e3d-io 在设计件上读的 `COMPRE` 不在触发重新生成的名单里，只改它时模型可能不重算 | 已推送（`d9ff0e88f`），等换后端后关 |
| 待定 | gen-model#143 | 被支撑的管件变了，经 `COMPRE` 指着它的支撑件不重算（跨元素、多数跨库；APS 234 条边里 224 条跨库） | 已关（11-13 推送） |
| 待定 | gen-model#144 | 级联拉进 regen 的根自己也有改动时，单元级发布只重建点名的单元 | 已推送；PJOI 那一半已随 #148 接上，跑一次手动真库测试后关（见票上 11-13 晚的评论） |
| 待定 | gen-model#153 | 房间的墙板在一个库里改了，别的库里成员的房间边要等那个库下一次有任务 | 已推送（`6e5b274fb`，每 60 秒对 2 个欠账库整库重算），回放用例要随回放环境恢复后补跑，换后端后关 |
| 待定 | gen-model#161 | 支管头尾挪了、OLET 自己没动时，OLET 经 `CREF` 取的 P 点不重算（e3d-model#39 留下的在案缺口） | 待办（开发计划 3.1，11-13 新开） |
| 待定 | 回放 `t4-whole-library-retry-limit` | 整库重排连败到上限之后来新会话，数据批次去开会话 0 而停跑（r7 方案 3.7） | 没有票（开发计划 3.2） |
| — | gen-model#61 | 增量更新剩余问题清单（追踪贴） | 追踪 |
| — | gen-model#77 | 增量更新测试案例与 2026-10-27 结果（记录贴） | 记录 |

## 五、工具链与 CI

| 优先 | Issue | 问题 | 状态 | 下一步 |
|---|---|---|---|---|
| — | gen-model#87 | E3D TTY 写完 L3-DONE 后不退出，后端白等满 1200 s | 已关（11-11；生产未部署） | `b4b6f725c` 随合并 `da97d03eb` 在 `origin/refactor`：宽限到期强制收尾，并按成功返回；`publish` 还没有 |
| P3 | gen-model#87 余项 | QUIT 后 `des.exe -tty` 不退出的根因 | 根因未证实 | 包装宏已改成 `QUIT` / `FINISH` / `FINISH`，宽限 60 s → 5 s，只是兜底。2026-11-06 影子工程导出 `24381/106125`（RVM 已写完）后，这三条放在包装宏里等满 90 s、再放进用户宏（`ALPHA LOG END` 之后）都不返回，`FINISH` 没跑到。登录探针能退，真实导出不能。控制台被 `PDMS_HIDECONSOLE` 藏着，没看到提示 |
| P3 | gen-model#79 | CI 有四道门长期坏掉，还有一次把账户配额撑爆 | 已关（11-13）。但 Windows tests 自 08-12 起没绿过：第 12 步 Python 守卫红，后面的 Rust 步骤全被跳过；修法在分支 `work/p0-ci-guard-docs`（开发计划 0.3） | `windows-tests.yml` 已在 `origin/refactor`。`linux-binary.yml`、`windows-binary.yml` 的 cargo 缓存 key 已去掉 `run_id`（按 `Cargo.lock` 哈希），并且只在 cache miss 时保存。在 `fix/model-open-issues-1106`，没跑过 CI |
| — | — | publish 构建拉不到能编过的 vendor（默认 ref 在 e3d-model 不存在，plant-mbd 缺提交） | 已修（本分支），待进 `publish` | linux-binary 默认 `VENDOR_REF=main`、`PLANT_UI_REF=master`，windows-tests 去掉 old-aios-core 的 caliber 钉选；五仓 `main` 已推齐，按 CI 拉法检查编译通过（见第六节） |
| 待定 | gen-model#121 | `refactor` 最新提交 `0890e488d` 从干净检出编不过：`manual_update.rs` 调用的 `fast_delete::wipe_dbnum_for_upgrade_on` 只定义在 `src/data_interface/fast_delete.rs` 的未提交改动里；测试代码用的 `roots_of_nouns` 只定义在 `src/data_interface/direct_tree.rs` 的未提交改动里 | 已关（11-13，`2418032c9`） | 缺的定义在主工作区未提交的改动里，由维护者提交。上一提交 `7f3eb75e3` 的普通构建能编过；lib 测试在 `7f3eb75e3` 也编不过（`roots_of_nouns` 的调用是 `c493ab821` 引入的）。`0890e488d` 还没推（见第六节） |
| 待定 | e3d-model#35 | `ancestor_reads_guard` 红灯：`rvm_export.rs` 的 `key_for`（`1ac43b4` 引入）、`scope_anchor`（`345841e` 引入）沿属主链上溯，没在 `OWNER_WALK_SITES` 登记 | 已关（11-13 推送） | `82f34b7`，只动 `tests/ancestor_reads_guard.rs`，断言一条没改；顺带补上被这盏红灯盖住的一处不符（`rpath.rs::resolve_anchor`）。合并树整套 40 个测试程序 651 过 / 0 败 / 16 忽略。还差：推送 |
| 待定 | gen-model#126 | rvm-check 不校验基准与项目数据的 E3D 版本：`_deploy/ams-server/backend/baselines` 是 E3D 2.1 导出（见其 `PROVENANCE.txt`），拿去对 3.1 的库会出假差异，工具没有提示 | 已关（11-13，`ea73933e4`） | 2026-11-08 下午两批（`output/rvm-check-fused/all`、`oldpath-79`）就是这样：ams8000 `=24384/24932`、`=24384/22399` 与 ams7999 `=24383/66456` 判 fail，`=24384/26547` 记 no-baseline，是 gen-model#116 的镜像（#116 已于 2026-11-10 关闭）。同四个根用 2026-11-07 的 3.1 导出作基准，在 11-10 全部 pass。要做：基准带版本标记、比对前校验；库里这两批的 132 条明细给处理结论 |
| 待定 | gen-model#122 | rvm-check 核对结果按构件 refno 入库（现在只到 SITE / ZONE 汇总加不一致明细） | 已关（11-13 推送） | 维护者 2026-11-10 提的要求。新表 `rvm_check_element`，一个带几何的构件一行，记录 id 由 refno 得出；ams7333 按 ZONE 全量写入 159,876 行，逐单元与汇总计数一致，原有判定逐字节不变。遗留：页面单根入口的端到端和两个 HTTP 接口的实测要等后端重启。gen-model#123–#126 都排在它后面（同一批文件），#123、#124 已开工 |
| 待定 | gen-model#123 | rvm-check 批量「比对失败」入库后原因种类全落到 `run_failed`：`batch.rs` 写的阶段名是 `compare_failed`，`store.rs` 的归类只认 `compare` / `compare-failed` | 已关（11-13 推送） | 阶段名统一成一种写法，加测试保证批量侧写出的阶段名能被归类侧认出 |
| 待定 | gen-model#124 | rvm-check 按 ZONE 核对时父进程每 0.5 秒才看一次子进程退没退（`GEN_SAMPLE_INTERVAL`）：ams7333 的 1062 个单元每个白等约 0.4 秒，合计约 420 秒 | 已关（11-13 推送） | 子进程一退出就接下一个单元，内存峰值采样不受影响。实测数字出自 `docs/plans/2026-11-10-rvm-check-zone-fused-compare-plan.md` |
| 待定 | gen-model#125 | rvm-check 一个库的全部 ZONE 压在一次 E3D 会话里导出（ams7333 是 1062 个）：会话没走完时逐个补导，每个 1–2 次登录，没有熔断 | 待办（开发计划 D10：等 gen-model#162、#163 落地再开工） | 分块导出、按块交接、失败熔断；失败路径至今没有演练过 |
| 待定 | gen-model#127 | rvm-check 对配上的构件只比包围盒和图元种类：构件摆错但包围盒不变时判成一致（48 根竖杆的切口摆反 180° 就是这样漏掉的） | 已关（11-13，`ea73933e4`；门限 0.05% 还要拿全量分布复核，开发计划 1.5） | 配对成功且包围盒通过的构件再比体积；门限按 APS 39 根、AMS 56 根的体积差分布定；比不了体积的构件要有标记 |
| 待定 | gen-model#128 | 前端的「库内结果」只到 ZONE 一级，看不到单元下面每个构件的结果，也不能按 refno 查 | 待办（前端在 plant3d-web 仓库，后端接口已有） | 后端接口已有（`/api/v1/rvm-check/elements`、`/element-grades`），要重启换上新二进制才可用；重启定在 gen-model#123、#124 修完之后 |
| 待定 | gen-model#129 | ams251142 没有 E3D 基准：圆弧弯头修复后新出的 26 件弯头（`CTBEND` 18、`CTRISE` 8）没有对拍过 | 已关（11-12） | 它不在 MDB 里，要像 aps250209 / aps250212 那样在项目副本里登记后导出 |
| 待定 | gen-model#130 | 只核对一块（`--scope`）的运行在单元汇总表里不记范围，看起来像一次计数很小的整单元运行 | 改动已写，在主工作区 `stash@{0}` 里，待提交（开发计划 4.3） | 汇总记录带上范围；按上级根汇总时单独计数，不冒充整单元的结论 |
| 待定 | gen-model#145 | rvm-check：`--projects-dir` 与配置的 `project_path` 不一致时，一次核对混用两份项目数据，差异照常记成模型差异，没有提示 | 已关（11-13 推送） | 2026-11-11 新开。直接影响对拍结果可不可信，验收前值得先修 |
| 待定 | gen-model#162 | rvm-check 按名字补导时不核对名字解到的是不是要导的那一只（e3d-model#54 的来源） | 待办（开发计划 4.1，11-13 新开） | 按名字导的宏读回 refno 比对，不是就判失败 |
| 待定 | gen-model#163 | rvm-check 对拍不核对基准与生成是不是同一个单元 | 待办（开发计划 4.2，11-13 新开） | 比 `scope_anchor.name_path` 与基准根组的名字路径 |

## 六、待推送的本地提交

**2026-11-13 晚核对：三个仓库都推齐了。** gen-model `origin/refactor` = `6e5b274fb`（含 #153），e3d-model `main` = `643b7b2`，
e3d-io `main` = `2866221`，本地与远端相同。主工作区的本地 `refactor` 另有并行会话刚提交、还没推的两笔（护栏去空白比对、
0.1.45 核对清单）。下面 11-11 与 11-06 的两张表留档，里面列的提交都已在远端。

2026-11-11 晚核对（`git ls-remote` 当场取远端头）：下面表里 11-10 记的 e3d-model `db65e1c`、`df3cbe5`、`42661d6` 与 e3d-io `03414fd` 都已推送（e3d-model `main` 在 `b999e6c`，e3d-io `main` 在 `03414fd`）。当天的修复经统一合并都到了本地主线，一个都还没推：

| 仓库 | 本地 | 远端 | 没推的 |
|---|---|---|---|
| e3d-io | `main` = `71ac836` | `03414fd` | 1 个提交：e3d-model#45 用的 resolver「不在 MDB 里」 |
| e3d-model | `main` = `9b74e66` | `b999e6c` | 8 个提交：e3d-model#38、#39、#45、#35、#42（两笔）、#36，加合并提交 |
| gen-model | `refactor` = `e8636c4a6`；当天全部补丁套在主工作区，未提交 | `4f2fd920f` | 9 个提交，加未提交的补丁（gen-model#72、#75、#76、#132、#142、#144、#146 等）。推送之前要先解决 gen-model#121 |

合并后的测试：e3d-model 整套 40 个测试程序 651 过 / 0 败 / 16 忽略（45 分钟那条真实增量测试在合并树上还没跑）；gen-model 库单测 3162 过 / 0 败 / 151 忽略，读取集合 204 项。本机后端还是当天下午换上的只带 gen-model#142 的那一版。

2026-11-06 晚核对：上面原列的 `b4b6f72`、`87dee7b`、`108f08fd`、`b77ff105` 都已在 `origin/refactor`；e3d-model、e3d-io 的 `main` 与 GitHub 一致。还没推的：

| 仓库 | 分支 | 提交 | 内容 |
|---|---|---|---|
| gen-model | `refactor`（本地） | `972b06cfa` | feat(mbd): layout_pipe 在 meta.source_version 填入读取版本 |
| gen-model | `fix/model-open-issues-1106`（worktree `gen-model-fix1106`） | 见分支 | 本清单「三、五」各项的修复，过回归门后并入 `refactor` |
| e3d-model | `main`（本地；2026-11-10 核对领先 GitHub 3 个提交，GitHub 在 `e2a596c`） | `db65e1c`、`df3cbe5`、`42661d6` | e3d-model#29 的弯头锚点、圆弧弯头，e3d-model#30 的柱脚垫板。修复分支 `fix/aps-model-closure` 也只在本地 |
| e3d-io | `main`（本地；2026-11-10 核对领先 GitHub 1 个提交，GitHub 在 `3987a08`） | `03414fd` | e3d-model#30 的 z 平移：`POD` 接上 `COMPRE` 两条链 |
| gen-model | `refactor`（本地；2026-11-10 核对领先 origin 3 个提交，origin 在 `4f2fd920f`） | `bf85b7785`、`7f3eb75e3`、`0890e488d` | rvm-check `--scope` 参数面及其合并、增量规划链按任务库读写。`0890e488d` 从干净检出编不过（gen-model#121），推之前先补上缺的定义 |
`origin/refactor` 比 `origin/publish` 多 11 个提交（含 #87 的修复）。#73 的修复在 `publish` 分支上，但 11-05 的 publish 构建失败，生产仍是 10-26 的 `83aa3c250`。

publish 构建的 vendor 口径（2026-11-07 定）：linux-binary 默认 `VENDOR_REF=main`、`PLANT_UI_REF=master`，windows-tests 去掉 old-aios-core 的 `codex/libgm-snout-caliber` 钉选，改动在本分支。同日推齐了各仓主线：plant-mbd `main` 快进到 `3db2d92`（MBD / isodim 工作线的 6 个提交，含本分支编译要用的 `fff0f6e` 契约 `source_version` 与 `5bef17f` `adjust_len`；推前 plant-mbd 全部测试通过），old-aios-core `main` 快进到 caliber 的 `7c0c54e`，e3d-model `main` 到 `d139a9d`。之后按 CI 拉法从 GitHub 取五仓 `main` 与 plant-ui `master`，用 CI 同款 features 检查编译通过，`Cargo.lock` 不变；本地未提交的 vendor 改动都用不到。

此前的卡点（同日检查编译查出）：默认 ref `ci/gen-model-linux-20260918` 在 e3d-model 仓不存在，克隆这一步就失败；plant-mbd 远端 `main` 缺上面 6 个提交，编不过（4 处）；old-aios-core 旧 `main` 落后 caliber 32 个提交，编不过（27 处）。

还差：workflow 改动随本分支并入 `refactor`、再进 `publish` 才生效。plant-ui `master` 的内嵌前端比旧 CI 分支（10-13 的 0.1.14）新，下次部署前端会一起换新。以后推 `publish` 之前，各 vendor 仓的 `main` 要先推齐。

## 回归门（每次合入前跑）

- e3d-model：`cargo +nightly-2026-10-25 test --release --lib`（2026-11-07，`d139a9d`：464 通过、0 失败、5 忽略），以及 `attribute_reads_guard`（3）、`ancestor_reads_guard`（5）、`reference_reads_guard`（2）、`noun_coverage`（8），同一次 release 构建，全过。
- 39 站对拍：`gen-model/.scratch/rvm-fix/b39_run.ps1`，产物放 E 盘，和 `b39/rref3` 逐站逐对比较。判档只许变好，不许变差。
  **`--project-dir` 只传主项目目录（`ams000`），同时传 `--main-project-dir`**：外项目库由 `adopt_foreign_dbs` 按 `amssys` 的外项目表补。两个目录一起传会让 `acp*` 与 `ams*` 撞号，整组不外交，目录件全取不到（见 #27）。
- gen-model：`cargo +nightly-2026-10-25 test --lib -- e3d_query rvm_check model_impact::tests options::tests::rvm_check`（当前 84 个通过）。
- 2026-11-10 复核（e3d-model `42661d6`，debug 构建，在隔离工作目录里跑）：lib 502 通过、0 失败、5 忽略（修复前 `db65e1c` 是 495）；`attribute_reads_guard`（3）、`reference_reads_guard`（2）、`noun_coverage`（8）通过，`ancestor_reads_guard` 4 过 1 败（e3d-model#35）；集成测试 27 组里 26 组通过。gen-model 的 lib 测试这次没跑成（测试代码编不过，gen-model#121）。APS 39 根、AMS 56 根的对拍结果见「最近进展」。

## 最近进展

- 2026-11-13（晚，状态核对与开发计划）：
  - **推送**：11-13 白天三个仓库推齐，gen-model 关 20 张、e3d-model 关 20 张（见 gen-model#63 当天两条评论）；
    晚上 #153（房间欠账清扫器）也推了。e3d-model `643b7b2` 带着提交标题写「未验收」的 `a0ff056`（#59），
    推送时只记了 lib 588 个测试与四道读取护栏，三套回归没有记录。
  - **线上没换**：r7 方案（`output/release-r7-20261112/SWAP-PLAN-r7.md`）没有执行记录；本机运行记录里的 r5 进程已经不在，
    8022 现在是 `.scratch/stack-remote` 的测试实例（17:56 起，`cd25b5f50.dirty` + e3d-model `809e20f`，只监听 7998）。
    换哪台、从哪个文件换，是开发计划 D1，等用户定。
  - **CI**：Windows tests 自 08-12 起没有一次绿。第 12 步 Python 守卫按字面找 `Operation::begin("…"`，rustfmt 换行后找不到，
    后面的 Rust 步骤全被跳过。修法在分支 `work/p0-ci-guard-docs`（`e0d243159`），CI 在那条分支上跑。
  - **回放环境丢了**：`output/increment-suite/local.json` 指向的 `D:\work\plant-code\incr-suite-20261112`（11-12 那一轮的私有副本、
    各人的用例目录、r5 / r6 / r7 后端副本）在本机已经不存在，D、E、F 盘都没找到。仓库里 `tests/increment_replay/cases` 的 8 条
    还能跑；11-12 录的 `t4-*`、T1、T2 那几批要重录（要 E3D）。
  - **新开的票**：e3d-model#63（板上 `CMPF → SBFI` 开孔）、e3d-model#64（弧墙经 FIXING 挂的门窗）、gen-model#161（OLET 级联缺口）、
    gen-model#162 / #163（rvm-check 的两道护栏）；#144 补了一条评论（PJOI 那一半已随 #148 接上，跑手动真库测试后关）。
  - **还卡着的**：`stash@{0}`（并行会话做回归门时收的）里有远端共享性能补丁、#130 的改动和本地 `DbOption.toml`，等那个会话做完再放回。
- 2026-11-11（统一合并与票面更新）：当天修掉 13 张——gen-model#72、#75、#76、#132、#142、#144、#146，e3d-model#35、#36、#38、#39、#42、#45——晚上经统一合并到了本地主线（e3d-io `main@71ac836`、e3d-model `main@9b74e66`、gen-model 主工作区未提交），合并后的测试 0 失败，都没推送，线上后端没换。GitHub 上：关 gen-model#87（修复早在 `origin/refactor`，生产未部署）；新开 gen-model#147、e3d-model#48；17 张票各加一条状态评论；gen-model#61 正文 A 类补了五个勾选框。e3d-model#32、#33、#34 当天凌晨已随 `main@b999e6c` 推送并关闭，gen-model#88 上午关闭。在做：gen-model#143、#129，e3d-model#43、#44、#47、#20。盘点全文见 `output/model-issues-status-20261111/STATUS.md`。
- 2026-11-10（APS 整库对拍收口）：2026-11-09 第一次把 APS 的 MDB `/ALL` 下全部 39 个 SITE 根（53 个 DESI 库）整库对拍（`rvm-check --all-desi --site-only`，E3D 3.1 TTY 导出作基准），首轮 23 pass / 9 empty / 7 fail，开了 e3d-model#29（电缆桥架）、#30（管道支撑）、#31（斜切圆柱）。11-10 把剩下两项修完，e3d-model 本地 `main` 从 `db65e1c` 快进到 `42661d6`（两个提交，未推）：`df3cbe5` 圆弧弯头——基准逐件量下来，圆弧只画在目录托盘 `CTRAY` 上，弯头本体仍是尖角臂板；去掉 `RADI ≠ 0` 的闸门，新增 30° / 45° 立弯的斜臂，67 件全 exact。`42661d6` 柱脚垫板——#30 关单时两个支撑根其实还是 fail，13 只垫板 E3D 画了、我们没画，判画条件只认 `SCTN` 立柱而这批是 `GENSEC`；补上后两根转 pass。修复前（`db65e1c`）对修复后（`42661d6`），基准是 2026-11-10 E3D 3.1 TTY 导出的现成文件：APS 39 根 24 pass / 6 fail → 26 pass / 4 fail（9 empty 不变），both 33011 → 33091、exact 33005 → 33085、chord 5、off 1、only_baseline 67 → 0、发不出主键的基准成员 87 → 74；修复前已有的 32869 条生成记录逐条不变，新增 80 条（`CTBEND` 34、`CTRISE` 33、`PJOI` 13）。AMS 24 个库 56 个根（基准是 2026-11-07 的 E3D 3.1 导出）50 pass / 6 empty，修复前后 322667 条生成记录逐条相同。`ams251142`（没有 E3D 基准）原有 230 条不变，新出 `CTBEND` 18、`CTRISE` 8，没有真值可对。e3d-model lib 单测 495 → 502；集成测试 27 组过 26 组，1 组是既有红灯（e3d-model#35）。还没过的 4 个根：三个桥架根差 46 根电缆（e3d-model#32）和 28 个路由节点标记（e3d-model#33），11-09 的对拍报告把它们记成「非几何元素」，实际都有面片；aps250110 `=2013286654/1` 是 W7f 那 1 件 0.54 mm（`docs/plans/2026-10-21-model-remaining-issues-fix-plan.md`，不改）。两处规则照样本接，记在 e3d-model#34。顺带查出 gen-model `refactor` 的 `0890e488d` 从干净检出编不过（gen-model#121），这次验证因此退到 `7f3eb75e3` 构建。证据在本机隔离工作目录的对拍产物和测试日志里，未入库。
- 2026-11-10（基准版本与 rvm-check）：`_deploy/ams-server/backend/baselines` 是 E3D 2.1 导出（`PROVENANCE.txt`：`e3d210`，批次 `20261108-e3d21-full`）。2026-11-08 下午两批（`output/rvm-check-fused/all` 15 根、`oldpath-79` 8 根）拿它对 3.1 的库，各有 3 fail + 1 no-baseline：ams8000 `=24384/24932`（只在生成 21）、`=24384/22399`（off 1、只在基准 1、只在生成 38）、ams7999 `=24383/66456`（off 2、只在基准 2）、`=24384/26547`（2.1 基准里没有这个根）。件数与两版数据逐根吻合（129 + 21 = 150，1981 + 38 = 2019、1981 + 1 = 1982，4116 + 2 = 4118），是 gen-model#116 的镜像；同四个根在 11-10 的 AMS 回归里用 3.1 基准全部 pass。没有做「同一个二进制只换基准」的对照。gen-model#116 当天关闭，工具侧校验归 gen-model#126。同日按 ZONE 核对的实测另带出 gen-model#122–#125（见五节）。GitHub 上相应加了评论：e3d-model#29、#30，gen-model#116、#63、#62。
- 2026-11-07（比对内存）：e3d-model `d139a9d`（已推）让 `rvm_compare` 基准侧面片的连通分量只记顶点区间，不再按坐标另存一份。`24381/101405`（34325 对）峰值私有内存 7.24 → 3.77 GB，`23717/2`（79938 对）6.66 → 4.88 GB，耗时不变；39 站用 `lateral1107` 的生成件重跑比对，`compare.json` 与改前逐字节相同（`D:\e3d-shadow-1106\runs\b39\mem1107`）。e3d-model 回归门（release lib 464 与四道读取守卫）全过。本机 8022 换上 `10a0288d6` 的 debug 二进制，三库按 `model_generator_changed` 重排约 10 分钟；服务端 rvm-check `24381/101405`（735 s）与 `23717/2`（732 s）通过，计数与前一轮相同（全部 exact）。比对段服务私有内存峰值约 5.5 GB（含常驻约 2 GB）；`23717/2` 的进程内生成段（`generate_ours`，173 s）把服务冲到 10.6 GB，整机可用提交额度一度只剩 1.8 GB，这是 rvm-check 下一处内存热点（生产的分批生成不走这条路径）。同日按 CI 拉法做了检查编译，查出 publish 的 vendor 卡点；CI 改为跟各仓主线并推齐各仓 `main`，见第六节。
- 2026-11-07（SITE RVM 全量）：本机 8022 换成带 e3d-model `5182aef` 内容的二进制（`c4bfd1c2e` 构建），按 `rvm-batch/rerun.ps1` 的口径对 `tree/roots` 下全部 39 个 SITE 逐个跑 `/api/v1/rvm-check/runs`（E3D TTY 当场导出 + 进程内生成 + 比对）：39/39 `succeeded`、判定全部通过；两侧 199192 对，exact 199180，chord 5、reference_artifact 4、film 3（即上表的可接受残差），off、only_baseline、only_generated、kind_differ、unkeyable、容器差异全为 0。结果在 `D:\e3d-shadow-1106\runs\site-rvm-1107\after-lateral.json`。`24381/101405`（34325 对）第一次跑时整机提交额度被其他进程占满，服务以 `alloc_failed` 退出（崩溃笔记留痕）；同一比对用命令行做过，`rvm_compare` 峰值私有内存 7.23 GB，对 11-07 新导出全部 exact；额度回升后服务端重跑通过（678 s）。`rvm_compare` 把基准面片顶点按 f64 存两份，大 SITE 的内存可以再压，记为后续项。
- 2026-11-07（续）：生成器指纹加 `e3d-io:`、`e3d-attlib:` 两段（`build.rs` `vendor_identity`，ADR-093 追记），单改 e3d-io 的几何修复也会触发整库重排；12 个相关单测通过，本机 8022 首启三库按 `model_generator_changed` 重排（约 10 分钟），再重启不排。8022 现从 `gen-model-fix1106\target\_run\aios-database.exe` 运行，重编不再撞文件锁。#74 本机验收见三节。publish 构建修法要先定 vendor 分支口径，未改 workflow。
- 2026-11-07：本机 8022 换成本分支 `668ddf89d` 的 debug 二进制（e3d-model `1e56a42`；工作目录、参数、`AIOS_STORE_MODE=rocksdb` 同前，另加 `AIOS_MODEL_GENERATION_INPUT_MAX_DBNUMS=1`、`AIOS_MODEL_ELEMENT_CACHE_MAX_ENTRIES=60000`）。#73 首启排三库整库重排，#88 那段在 7997 上 115.8 s 走完，三库约 10 分钟回到 `model_ready`，内存峰值约 1.9 GB；再重启不再排。抽查：#22 的 `24381/129910`、`129911` 两只 RTOR 在库里有几何，网页 `show_refno=24381/129625` 能看到；#24 的 `24381/106125` 仍是 15 条管段记录（含 TUBI 9）；#23 的 `24381/47065` 前后都是 13 条构件记录，零径占位只进 RVM，不进网页用的模型记录。证据在 `target/_verify-1107/`（不入库）。旧二进制（主工作区 `target/debug`，e3d-model `6f2b68a`）若再起，会因落款不一致把三库按旧几何重排，且不带 #88 的修复。
- 2026-11-06（夜）：e3d-model release：lib 463 通过、5 忽略；`attribute_reads_guard` 3、`ancestor_reads_guard` 5、`reference_reads_guard` 2、`noun_coverage` 8，全过。#86 普查接上 1012 后两组消失（+62 读通）。#34 默认保持关闭，语料门没跑。
- 2026-11-06（夜）：`revert1106` 39 站（撤回偏轴门的 `gen_ams`）复现 #27 的总数：exact 199180、off 0、kind_differ 0、only_baseline 0、only_generated 1（#24）。约 1240 条只在基准是撞号旧账，勾销。#21 探针：走完求交的 perp 关节局部点乘等于世界点乘，留站位与 E3D 一致，不改 `datum.rs:717`。#20 端板：`=23550/1` 导出和 `dblist()` 都崩，7166/7505 不在 `/ALL`。
- 2026-11-06（夜）：`23705/289` 用已提交的 POD / `void_negatives` 重导，32 对全 exact，8 只 Z 偏移没回来。#20 的三只 `/MDS/FT17/2-V1` 重新导出也对上（exact，0 mm）；`/Endplates-Column-JOINT` 在 `aps7505`，按名字进去 des 崩溃。
- 2026-11-06（夜）：#24 影子工程单件。删掉槽内两只透传 ATTA 再导出，2315 mm 圆柱仍不出现（14080 → 12320 字节，只少两只球），偏轴 ATTA 不是原因；写 `SPAMAP` 时 des 0xC0000005。两刀都没 SAVEWORK，`ams7997_0001` 按哈希还原。规则不落地。#87：同一条导出之后 `QUIT` / `FINISH` / `FINISH` 不让 des 返回，宽限仍是实际收尾。
- 2026-11-06（晚）：按代码与 `rref3` 核对改口：e3d-model#21 已由 `222a0ef` 修好并关闭（P1 几何清零，perp 遗留降为 P3）；#73 已落地（`43dd85c5a`，在 `publish`）；#74 三点大多已提交；#87 已在 `origin/refactor`；#79 只剩两个二进制工作流的缓存 key；#62 只差 tag；新增 #88（卡点在设备派生量重算，不在 durable 栅栏）。修复在 worktree `gen-model-fix1106` 的 `fix/model-open-issues-1106` 分支上做，主工作区的未提交改动不动。
- 2026-11-06：SITE RVM 复测（干净构建，独立后端 8122）：10 个原失败站里 7 个全过（`24381/46775`、`23714/1`、`24381/535`、`23705/1`、`23713/2343`、`24381/177373`、以及 `24381/101405` 只剩 #24 那 1 件）；`15516/2`、`23734/1` 各剩 3 件 unkeyable（新开 e3d-model#26）；`23705/289` 因依赖未提交的 e3d-io / e3d-model 改动而出现 8 只 Z 偏移。
- 2026-11-06：#26 定位到宿主 SCTN 的 `RREF` 判据，`5775ecb` 推 main 并关闭；39 站零回归，6 件 unkeyable → exact。
- 2026-11-06：#27 查明为验证脚本用法错（两个 `--project-dir` 导致 `acp7000_0001`/`ams7000_0001` 撞号、库 7000/7600/6000 全不外交），非代码缺陷，已关。`b39_run.ps1` 改为「主项目目录 + `--main-project-dir`」走 `adopt_foreign_dbs`。39 站全量重跑：`off` 162 → 0、`only_baseline` 5441 → 0、`exact` 191946 → 199180，两侧都有的键零变差。新基线 `b39/rref3`。
- 2026-11-06：#22 合入后复验并关闭。#24 补 APS 全库取证（无管道第二样本），评论在 issue。kind_differ「负体不按 TUFL 筛」实验在 23710/1 多出 276 对 kind_differ，否证，未改代码。
- 2026-11-06：#23（`8e3cf67`）、#25（`b94615c`）推到 e3d-model main 并关闭，`46f8d8f`（#22）随之在 github main；39 站对 `m1076db`、`m1076` 零退化，`=24381/46775` 只在 E3D 0、`=23714/1` 不可配对 0。#24 两条假设都被否证，首版已 revert，issue 保持开启。`23705/289` 的 4 对 kind_differ 定为可接受残差（见上表）。
- 2026-11-05：#22 本地修复（`46f8d8f`），39 站零退化；#87 本地修复（`b4b6f72`），实机 5 站全部 pass。两个 issue 下都已留评论，都没关。
