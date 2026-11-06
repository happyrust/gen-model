# 模型问题跟踪清单（2026-11-06）

依据：39 站全量对拍（p2a 加上 e3d-model `46f8d8f`，共 196,329 对）、e3d-model 与 gen-model 的 open issue。
状态栏：`待办` / `进行中` / `已修待推` / `待确认` / `已关`。改动后在「最近进展」里记一行日期和结论。

## 一、几何对不上（39 站对拍）

| 优先 | Issue | 问题 | 涉及件 | 站 | 状态 | 下一步 |
|---|---|---|---|---|---|---|
| P1 | e3d-model#21 | SJOI 斜交连接梁的 IJOINT 平移：缺 GJTORG 第二次求交的闭式 | 9 件 off（3.4–12.7 mm）：`/MDS/VT01`、`VT02`、`VT05`、`FT13` 下的 `IJOINT1/2/3` | `23713/2343`、`15516/2`、`23734/1` | 待办 | P3 第三条线已否证，需要新的推导或 IDA 取证 |
| — | e3d-model#23 | HVAC 头槽位断面解不出时，E3D 写半径 0 的圆柱占位，我们记 unbuildable 不写 | 1 件只在 E3D：`TUBE 1 of BRANCH /-CAM-E-2-H-5302` | `24381/46775` | 已关 | `8e3cf67`（已推 main）：照 E3D 写 0 径 Cylinder 占位，只在 E3D 1 → 0 |
| P2 | e3d-model#24 | 隐式管身某段 E3D 不画，我们照画 | 1 件只在我们：`TUBI 9 /Copy-of-1RCS007MN-TUBE/801VP`（2315 mm） | `24381/101405` | 待办 | 两条假设已否证（见 issue 评论）：偏轴 ATTA（40 个同类槽位只此一个不画）；离开侧 SPAMAP 第二字为空（aps250142 STIF `2013286686/123` 同样为空但 E3D 照画，首版 `4b38b0c` 已 revert）。APS 239 库扫过：空离开键可画槽位 4 个（2 个 HVAC STIF，E3D 照画；2 个 aps7200 1 mm 吊架段），没有第二个管道样本。需 E3D 侧单件取证 |
| — | e3d-model#25 | 零长 SCTN 名下的 SJOI J1 E3D 照画，我们整串 skipped | 2 件配不上：`/MDS/SPECIALS/PIPE/3-BAR-2/S1/J1`、`4-BAR-2/S1/J1` | `23714/1` | 已关 | `b94615c`（已推 main）：零长轴取 +Z，两只 J1 exact，不可配对 2 → 0。规则只有 2 个样本，且 DRNS 恰为 +Z，遇到 DRNS ≠ +Z 的零长 SCTN 时复核 |
| — | e3d-model#22 | RTOR 内半径大于外半径时 E3D 照画，我们判退化 | 2 件（RTORUS 1/2 of SUBE 10 of `/Copy-of-R43-R540-008`） | `24381/101405` | 已关 | `46f8d8f`（github main）；39 站零退化 |
| — | e3d-model#26 | FRMW 名下 PNOD 里的 PJOI `GTYP` 为空值时，E3D 有时照画、我们整串 skipped | 3 件配不上：`/MDS/FT62/1-V1/JOINT1`、`/MDS/FT64/1-V1/JOINT1`、`/MDS/FT64/1-V2/JOINT1` | `15516/2`、`23734/1` | 已关 | `5775ecb`（已推 main）：判据是宿主 SCTN 的 `RREF` 有没有指到这只 PJOI 的宿主 PNOD。`GTYP` 空 + RREF 命中 → 画（ams7324/7350 各 3 只）；`GTYP` 空 + RREF 不命中 → 不画（ams7324 FT61 的 18 只、ams7999 的 500 只、ams8000 的 248 只，与 E3D 一致）。39 站零回归，6 件 unkeyable → exact。`RREF` 是新增读点，已进读取集合（181）与 `ancestor_reads_guard` 的 `WithinUnit` 登记 |

| P2 | e3d-model#27 | TUBI 取不到目录件外径时回落到达侧通径，半径差 3–9 mm | 162 对 `off`（全是 TUBI，偏差只在 X/Y 且对称）：`24383/73927` 76、`24383/66456` 66、`24381/101405` 14、`24381/177373` 6 | `24383/73927`、`24383/66456`、`24381/101405`、`24381/177373` | 待办 | `tubes_catalogue_od=0`，116 根管子全回落通径。差值与目录件 `IPAR[1]` 逐一对得上（168.3→150 差 9、114.3→100 差 7、88.9→80 差 4.5）。`route::departure_tube_outer_diameter` 从离开侧 `LSTU`/`LSRO` 取 `PARA[2]`，恒取不到；与 #24 的「离开侧目录件缺失」同族。环境侧另有 `db 7000/7600/6000` 未解析导致 2941 个构件 failed（`ams-equiv` 合并目录里 `acp*` 与 `ams*` 撞号），但换成两个真实目录后 `tubes_catalogue_od` 仍为 0，说明半径差与库解析无关。后端 `rvm-check` 对 66456/73927 判 pass（`counts.off=0`），漏掉了这 66/76 条，门禁口径待查 |

### 可接受的残差（不单独跟踪）

| 判档 | 件数 | 内容 |
|---|---|---|
| chord | 10 | MDS `IJOINT2/3` 0.1–0.5 mm；FLOOR、GWALL 0.05–2.18 mm；`TUBI 1 /-CCV-E-3-L-1704` 0.07 mm |
| reference_artifact | 4 | `/1RX-FN09-F001` FLOOR 2、`/1RX-FN03-FN` FLOOR 5、`/1RS-WF03-F-C-F001` FLOOR 12、`/1RS-WF05-F-C-F001` FLOOR 9 |
| film | 3 | `/1RX-FN01-F001` FLOOR 12、`/1LR-WF05-S-C` FLOOR 6、`/1RS-WF03-W-C-RR001` GWALL 13 |
| kind_differ | 4 | `23705/289` 的 `/MDS-Standards-ST59-PS`、`GT85-PS`、`GT86A-PS-1`、`GT87A-PS-1`：几何 exact，E3D 写一只布尔后的 FacetGroup，我们按图元写 Box/Cylinder/Facet（或 Pyramid）。原因在目录负体：ST59/GT85 的法兰孔 NSCY 是 `TUFL=false`，被我们的显示筛收走；腹板孔 `PDIA = PLOD + DESP[19]`，E3D 日志自己报 `DESP parameter number (19) out of range`；GT86A/87A 的 NSCY `PDIA = RPRO PLOD`，LSTU 链不在会话里时 POD 取 0，刀退化。我们一把刀都没扣上（`negatives_applied` 0），就走了按图元写出。E3D 侧本身报目录错，不是 e3d-model#7 的回归；2026-11-06 实验（负体只按 LEVE 筛、不按 TUFL 筛，39 站 xneg）：23705/289 kind_differ 4 → 2（ST59/GT85 改为整只面片），但 `23710/1` 新增 276 对 kind_differ，几何判档全不变。所以 E3D 对 TUFL=false 负体并非一律照扣，假设否证，不改。2026-11-06 干净构建复测（全部取已提交版本）：这 4 对已消失，但同一站出现 8 只 Z 偏移件（ST59/GT85 基准 z 0..200、生成 84.86..336.5；GT86A/87A 基准 z -100..0、生成 -184.5..-59.15），根因是 PLOD 依赖工作区未提交的 `vendor/e3d-io` `db_param_env.rs`（LSTU 链走不到时 POD 取 0）与 `vendor/e3d-model` `catalogue.rs` 的 `void_negatives`；提交这两处后需复测确认 |

### 待核实

- [ ] `rvm_compare` 原始结果里还有约 1,240 条只在基准的 SCTN、HELEMENT、ATTA，集中在 `24383/66456`（976）、`24381/48631`（152）、`24381/57476`（62）、`24381/79977`（26）、`24381/101405`（14）、`24381/76564`（6）、`24383/73927`（3）这几站。rvm-check 对这些站判的是 pass，说明后端在判定时排除了它们，但还没有逐条核实排除的理由。核实之后，要么记为可接受，要么另开 issue。

## 二、其他模型几何问题

| 优先 | Issue | 问题 | 状态 | 下一步 |
|---|---|---|---|---|
| P3 | e3d-model#20 | SCTN 端关节在 mode 0 下 NAPP 2 刀还切不切：13 处关节缺 E3D 真值（不在 39 站内） | 待办 | 在 E3D 里取这 13 处的真值 |
| P3 | gen-model#63 余项 | aps7200 `/INNER-KICKPLATE` 差 5.63 mm | 待办 | 在 E3D 里造同形样本 |
| P3 | gen-model#63 余项 | FN09 楼板 | 待办 | 等 E3D 按修正清单改完后重导、对拍 |
| P4 | gen-model#62 | 对拍覆盖面扩展：MDS AISC / DIN / SPECIALS、ams8000、APS、ams2510xx 的 FIXING 真值 | 待办 | — |
| P4 | gen-model#86 | PML 还有约 8000 条程序用到未实现的操作码（1402/112/103/1006/1012/105/1309/108/104） | 待办 | — |
| — | e3d-model#16 | 算法文档（持续更新，不算缺陷） | 持续 | — |

## 三、生成流程与线上落地

| 优先 | Issue | 问题 | 状态 | 下一步 |
|---|---|---|---|---|
| P1 | gen-model#73 | 初始化时，对「模型目标没变」的根沿用旧 vendor 生成的行，所以换了 e3d-model 的几何修复也不会落地 | 待办 | 生成行里记下 vendor 版本，版本不同就重算；修好之前，每次发版都要手动 `model/rebuild` |
| P2 | gen-model#74 | 一根 failed 让整库 lagging，`model/records` 对整库返回 0 条（ams7330：364 根里 4 根失败） | 待办 | — |
| P2 | gen-model#78 | release 二进制在范围生成中段 `0xc0000409` 崩溃，每次同一偏移，根因未定 | 待办 | — |
| P3 | gen-model#72 | 模型侧生成失败没有停跑和退避的记录，和数据批次的 `DATA_BATCH_PARK_AFTER` 不对称 | 待办 | — |
| P3 | gen-model#34 | 结构连接级联（JOIS/JOIE ↔ CREF）默认关闭，关节负体落地后存在正确性缺口 | 待办（ready-for-human） | 卡在 SCTN 关节端切（SJOI/PJOI 的 `CUTP`/`CUTB`）和真机 SAVEWORK |

## 四、增量更新

| 优先 | Issue | 问题 | 状态 |
|---|---|---|---|
| P2 | gen-model#75 | BRAN 只改 HPOS：增量回执 derived=1，但管子没有变化，网页和 RVM 都不变 | 待办 |
| P3 | gen-model#76 | 夹具 data：只改 NAME 再改回，恢复后仍留下一条 Pos 几何关系 | 待办 |
| P3 | gen-model#45 | 核查 Extract（_NNNN）和 Global FLUSH 引起的主库会话重编在增量链上的表现 | 待办（ready-for-human） |
| — | gen-model#61 | 增量更新剩余问题清单（追踪贴） | 追踪 |
| — | gen-model#77 | 增量更新测试案例与 2026-10-27 结果（记录贴） | 记录 |

## 五、工具链与 CI

| 优先 | Issue | 问题 | 状态 | 下一步 |
|---|---|---|---|---|
| — | gen-model#87 | E3D TTY 写完 L3-DONE 后不退出，后端白等满 1200 s | 已修待推 | `b4b6f72`（`feat/rvm-check-inprocess`，本地）：宽限 60 s 后强制收尾，并按成功返回 |
| P3 | 新开（待建） | QUIT 后 `des.exe -tty` 不退出的根因：实测 5/5 站都会发生，每站固定多等一个宽限期（60 s） | 待办 | 查清根因；或者先调小 `E3D_MCP_DONE_GRACE_SECS` |
| P3 | gen-model#79 | CI 有四道门长期坏掉，还有一次把账户配额撑爆 | 待办 | — |

## 六、待推送的本地提交

| 仓库 | 分支 | 提交 | 内容 |
|---|---|---|---|
| gen-model | `feat/rvm-check-inprocess` | `b4b6f72` | fix(e3d-query): L3-DONE 后宽限到期强制收掉 E3D 会话，按成功返回（#87） |
| gen-model | `feat/rvm-check-inprocess` | `87dee7b` | feat(rvm-check): 生成与比对改在后端进程内跑 |
| gen-model | `refactor` | `108f08fd` | docs(plans): 模型问题跟踪清单：#22/#23/#25 关单、#24 证据、kind_differ 残差 |
| gen-model | `refactor` | `b77ff105` | docs(plans): 跟踪清单补 #26 行、SITE 复测结果与 kind_differ 备注 |

推送之后：gen-model 更新 vendor，过回归门（见 gen-model#63「下一步顺序」）。在 #73 修好之前，线上需要手动跑一次 `model/rebuild`。

## 回归门（每次合入前跑）

- e3d-model：`cargo +nightly-2026-10-25 test --release --lib`（当前 463 个通过），以及 `attribute_reads_guard`、`ancestor_reads_guard`、`reference_reads_guard`、`noun_coverage`。
- 39 站对拍：`gen-model/.scratch/rvm-fix/b39_run.ps1`，产物放 E 盘，和 `b39/p2a` 逐站逐对比较。判档只许变好，不许变差。
- gen-model：`cargo +nightly-2026-10-25 test --lib -- e3d_query rvm_check model_impact::tests options::tests::rvm_check`（当前 84 个通过）。

## 最近进展

- 2026-11-06：SITE RVM 复测（干净构建，独立后端 8122）：10 个原失败站里 7 个全过（`24381/46775`、`23714/1`、`24381/535`、`23705/1`、`23713/2343`、`24381/177373`、以及 `24381/101405` 只剩 #24 那 1 件）；`15516/2`、`23734/1` 各剩 3 件 unkeyable（新开 e3d-model#26）；`23705/289` 因依赖未提交的 e3d-io / e3d-model 改动而出现 8 只 Z 偏移。
- 2026-11-06：#26 定位到宿主 SCTN 的 `RREF` 判据，`5775ecb` 推 main 并关闭；39 站零回归，6 件 unkeyable → exact。
- 2026-11-06：39 站 off 162 条全部归因到 TUBI 保温外径缺失，新开 e3d-model#27。同时查清 `only_baseline` 5441 的主体是 `ams-equiv` 缺 db 7000/7600/6000 解析（2941 个 failed 构件）与 E3D 隐式管段命名键不匹配（`TUBE n of BRANCH` vs `TUBI n of BRAN`），非几何缺陷。
- 2026-11-06：#22 合入后复验并关闭。#24 补 APS 全库取证（无管道第二样本），评论在 issue。kind_differ「负体不按 TUFL 筛」实验在 23710/1 多出 276 对 kind_differ，否证，未改代码。
- 2026-11-06：#23（`8e3cf67`）、#25（`b94615c`）推到 e3d-model main 并关闭，`46f8d8f`（#22）随之在 github main；39 站对 `m1076db`、`m1076` 零退化，`=24381/46775` 只在 E3D 0、`=23714/1` 不可配对 0。#24 两条假设都被否证，首版已 revert，issue 保持开启。`23705/289` 的 4 对 kind_differ 定为可接受残差（见上表）。
- 2026-11-05：#22 本地修复（`46f8d8f`），39 站零退化；#87 本地修复（`b4b6f72`），实机 5 站全部 pass。两个 issue 下都已留评论，都没关。
