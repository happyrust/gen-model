# 模型问题跟踪清单（2026-11-06）

依据：39 站全量对拍（最初是 p2a 加上 e3d-model `46f8d8f`，共 196,329 对；现行基线 `b39/rref3`，见「回归门」）、e3d-model 与 gen-model 的 open issue。
状态栏：`待办` / `进行中` / `已修待推` / `待确认` / `已关`。改动后在「最近进展」里记一行日期和结论。

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
| P3 | e3d-model#20 | SCTN 端关节在 mode 0 下 NAPP 2 刀还切不切：13 处关节缺 E3D 真值 | 进行中 | 螺栓板那半已对上：影子工程重新导出 `23708/2046`、`23713/3470`、`23734/1124`（都是 `/MDS/FT17/2-V1`），对当前 `gen_ams` 三只都 exact、偏差 0 mm。端板那半出不了真值：`aps7166` / `aps7505` 不在 APS 的 MDB `/ALL` 里（打开 `/ALL` 会改其他成员的时间戳，这两份文件的时间和字节都没变）。按名字进 `/Endplates-Column-JOINT`、按 `=23550/1` 导出、以及 `!!CE.db().mdb().dblist()` 都是 des 0xC0000005（未挂载库上的导航是原生崩溃，ONERROR 接不住）。`Q MDB` 能打出 `MDB: /ALL`。`gen_ams --root =23550/1` 自己能跑（visited 1147、generated 305），没有 E3D RVM 可对。不再重试这三条宏 |
| P3 | e3d-model#21 遗留 | perp 分支（`\|v44·v38\| ≤ 5e-6`）整支留站位是经验规则：其中 42 件反编译可证（第一次求交分母为 0），另 100 件（VT10 的 `*-BAR-*`、FT04H / FT04V 的 `*-V1`）按反编译应能求交、E3D 仍留站位 | 已核实，不改 | 2026-11-06 用 `sjoi_gjtorg_probe --all --verbose` 对真实 `ams7329` / `ams7324` / `ams7350` 各配对应 E3D RVM（根 `23713/2343`、`15516/2`、`23734/1`）。这三库是同一批 MDS 关节的副本。每库 perp 约 65–68，其中走完求交的 33–36 只（名字在该根 RVM 里的那部分；探针跳过 RVM 里没有的名字，所以不是 100 只全量）。走完的那些，关节局部点乘等于世界点乘（例如 `/MDS/FT04V/9-V1` 两者都是 1.0，混坐标系是 −0.707；`/MDS/VT10/1-BAR-1` 两者都是 −0.4996，混坐标系是 0）。「perp 是跨坐标系点乘造成的」不成立。现行留站位与 E3D 一致（ours=ok），按反编译去求交则预测 off。`datum.rs:717` 不动 |
| P3 | gen-model#63 余项 | aps7200 `/INNER-KICKPLATE` 差 5.63 mm | 待办 | 在 E3D 里造同形样本 |
| P3 | gen-model#63 余项 | FN09 楼板 | 待办 | 等 E3D 按修正清单改完后重导、对拍 |
| P4 | gen-model#62 | 对拍覆盖面扩展：MDS AISC / DIN / SPECIALS、ams8000、APS、ams2510xx 的 FIXING 真值 | 本地 tag 已打，未推 | 第 0–5 步都已完成。本地 tag `v0.1.43` → `cb0fbd8a5`、`v0.1.44` → `83aa3c250`。不推：`v*` 会触发 `windows-binary.yml` |
| P4 | gen-model#86 | PML 未实现操作码里，只有 1012 有几何消费者 | 已修待推 | 普查 acp7320+acp7003 接上 1012 之后：读通 117674（+62）、被拒 11890（−62）、分组 23。`1012 101`（8 条 PTCA）和 `1012 804`（54 条 SBOX/SVER）两组都消失，其余 23 组没有 PTCA/PTMI/PTAX，保持拒读。1012 是 `exprlib/EXPORE`，打印 `REAL ( )`，数字上恒等，单测 `ptca_py_real_is_the_expore_cast` 通过。还没提交、没关 GitHub |
| — | e3d-model#16 | 算法文档（持续更新，不算缺陷） | 持续 | — |

## 三、生成流程与线上落地

| 优先 | Issue | 问题 | 状态 | 下一步 |
|---|---|---|---|---|
| P1 | gen-model#88 | 批次 worker：7997 `model_only` 停在 `durable_pending`（6772/6772 已 ACK），挡住后面排队的 7998 / 8000 | 已修待推（本机已验） | 卡点是栅栏之后的 `refresh_equip_derived`。已改成独立阶段 `equip_derived`、5 分钟预算、整库或根数 ≥ 2000 跳过空间反查、节点按 1000 分块，`/health.equip_derived` 报推迟数。全库单测 2844 通过。2026-11-07 本机 8022 换成本分支二进制，#73 触发 7997 整库重排：6772/6772，`durable_pending` 15.5 s，`equip_derived` 115.8 s（`library_scale`，推迟 0），8 分 25 秒收尾，7998、8000 随后出队 |
| — | gen-model#73 | 初始化时，对「模型目标没变」的根沿用旧 vendor 生成的行，所以换了 e3d-model 的几何修复也不会落地 | 本机已验，线上未部署 | `43dd85c5a`（ADR-093）：生成器指纹改读 vendor 构建身份，水位行加库级落款 `model_generator`，落款不一致就整库重排；在 `origin/publish` 分支上，但 11-05 那次 publish 构建失败（CI 从 `ci/gen-model-linux-20260918` 拉的 e3d-model 只到 `b9324e0`，缺 `e3d_model::pipeline::Stop`），最后一次成功部署仍是 10-26 的 `83aa3c250`，不含本修复。2026-11-07 本机 8022 验过：首启日志点名 7997/7998/8000 `reason=model_generator_changed`，三库 `dbnum_watermark.model_generator` 改为新指纹，再重启 `generator_stale_dbnums` 为空、不再排。同日补漏：指纹原先只含 e3d-model，现加 e3d-io、e3d-attlib 两段（未提交），本机同样验过首启重排、再启不排 |
| P2 | gen-model#74 | 一根 failed 让整库 lagging，`model/records` 对整库返回 0 条（ams7330：364 根里 4 根失败） | 已修待推（本机已验） | 按根回读和 `ready_source` 已在 refactor。初始化回执对 `projection.partial` 的最后一条 `Generated` 写成 `published survivors; {reason}`，单测 `published_survivors_are_named_on_the_generated_unit_only` 在全库 2844 里通过。2026-11-07 本机 8022 用 `AIOS_TEST_FAIL_ROOTS=24384/2529` 让 8000 整库重建里一根整根失败：任务 `partial`，其余 953 根就绪，对照根 `24384/2541` 照常读到 6 条，失败根继续给出库里旧的 8 条；进程内该库判 lagging（追赶 1 根），去掉钩子重启后三库 `in_sync`、8000 `durable_ready_total` 954/954。钩子不接初始化路径，7330 式「部分件失败」只有单测，没有本机复现。没关 GitHub |
| P2 | gen-model#78 | release 二进制在范围生成中段 `0xc0000409` 崩溃，每次同一偏移，根因未定 | 已修待推 | 几何池和主 rayon 池 64MB 栈，`the_geometry_pool_runs_on_large_stacks` 通过。崩溃笔记记当前根、阶段、工作集；分配失败、栈溢出、访问违例和 `SIGABRT` 写 `logs/crash-notes.log`。发布脚本留行号 PDB，`Start-AMS` 默认 `RUST_MIN_STACK` 128MB，并有 `-EnableCrashDumps`。没在生产进程上复现 |
| P3 | gen-model#72 | 模型侧生成失败没有停跑和退避的记录，和数据批次的 `DATA_BATCH_PARK_AFTER` 不对称 | 已修待推 | 按根 30 秒起、翻倍、10 分钟封顶，只活在进程里；退避中的根仍挡水位，本轮不生成、不增加尝试、不把退避再往后推。新的 `source_end_sesno` 立刻复活。`/health.model_root_backoff` 报样本。`backing_off_roots_skip_generation_without_pushing_their_backoff_further` 在全库 2844 里通过。ADR-090 正文不在这个 worktree，追记写在 changelog。2026-11-07 本机注入一根失败后观察 7 分钟：没有新窗口时这根不会被自动重试，所以退避只在第一次失败后的 30 秒里出现在 `/health.model_root_backoff`；重复尝试下的退避节奏只有单测 |
| P3 | gen-model#34 | 结构连接级联（JOIS/JOIE ↔ CREF）默认关闭，关节负体落地后存在正确性缺口 | 待办（默认不翻） | 同库 FIXING ↔ GENSEC、同库 SJOI ↔ SCTN 在开关关着时已经由 e3d-model `plan_update` 把对端根放进 regen（`structural_connection.rs` 文件头）。开关还补得上的是 PJOI/SUBJ → SCTN、没有标架样本的 SJOI、`CMPF.CFRA`，以及跨库交接。语料目标已定位、还没改位：SCTN `=23734/3557`（`/MDS/FT54A/4-BAR-2`）的 JOIE 是 PJOI `=23713/5493`（PNODE `=23713/5492` 的唯一成员），CREF 指回这根梁，跨库 7329↔7350。短名 `/MDS/FT54A` 和 `COLL ALL SJOI` 会让 des 0xC0000005；按 refno 和成员路径查询正常。没有 SAVEWORK，默认仍是 false（`options.rs`、`DbOption.toml`、ADR-080、CONTEXT）。不补 NGMR→GENSEC 边 |

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
| — | gen-model#87 | E3D TTY 写完 L3-DONE 后不退出，后端白等满 1200 s | 已进 refactor，待部署 | `b4b6f725c` 随合并 `da97d03eb` 在 `origin/refactor`：宽限到期强制收尾，并按成功返回；`publish` 还没有 |
| P3 | gen-model#87 余项 | QUIT 后 `des.exe -tty` 不退出的根因 | 根因未证实 | 包装宏已改成 `QUIT` / `FINISH` / `FINISH`，宽限 60 s → 5 s，只是兜底。2026-11-06 影子工程导出 `24381/106125`（RVM 已写完）后，这三条放在包装宏里等满 90 s、再放进用户宏（`ALPHA LOG END` 之后）都不返回，`FINISH` 没跑到。登录探针能退，真实导出不能。控制台被 `PDMS_HIDECONSOLE` 藏着，没看到提示 |
| P3 | gen-model#79 | CI 有四道门长期坏掉，还有一次把账户配额撑爆 | 已进本分支（已推 origin），未进 `refactor` | `windows-tests.yml` 已在 `origin/refactor`。`linux-binary.yml`、`windows-binary.yml` 的 cargo 缓存 key 已去掉 `run_id`（按 `Cargo.lock` 哈希），并且只在 cache miss 时保存。在 `fix/model-open-issues-1106`，没跑过 CI |
| — | — | publish 构建拉不到能编过的 vendor（默认 ref 在 e3d-model 不存在，plant-mbd 缺提交） | 已修（本分支），待进 `publish` | linux-binary 默认 `VENDOR_REF=main`、`PLANT_UI_REF=master`，windows-tests 去掉 old-aios-core 的 caliber 钉选；五仓 `main` 已推齐，按 CI 拉法检查编译通过（见第六节） |

## 六、待推送的本地提交

2026-11-06 晚核对：上面原列的 `b4b6f72`、`87dee7b`、`108f08fd`、`b77ff105` 都已在 `origin/refactor`；e3d-model、e3d-io 的 `main` 与 GitHub 一致。还没推的：

| 仓库 | 分支 | 提交 | 内容 |
|---|---|---|---|
| gen-model | `refactor`（本地） | `972b06cfa` | feat(mbd): layout_pipe 在 meta.source_version 填入读取版本 |
| gen-model | `fix/model-open-issues-1106`（worktree `gen-model-fix1106`） | 见分支 | 本清单「三、五」各项的修复，过回归门后并入 `refactor` |
`origin/refactor` 比 `origin/publish` 多 11 个提交（含 #87 的修复）。#73 的修复在 `publish` 分支上，但 11-05 的 publish 构建失败，生产仍是 10-26 的 `83aa3c250`。

publish 构建的 vendor 口径（2026-11-07 定）：linux-binary 默认 `VENDOR_REF=main`、`PLANT_UI_REF=master`，windows-tests 去掉 old-aios-core 的 `codex/libgm-snout-caliber` 钉选，改动在本分支。同日推齐了各仓主线：plant-mbd `main` 快进到 `3db2d92`（MBD / isodim 工作线的 6 个提交，含本分支编译要用的 `fff0f6e` 契约 `source_version` 与 `5bef17f` `adjust_len`；推前 plant-mbd 全部测试通过），old-aios-core `main` 快进到 caliber 的 `7c0c54e`，e3d-model `main` 到 `d139a9d`。之后按 CI 拉法从 GitHub 取五仓 `main` 与 plant-ui `master`，用 CI 同款 features 检查编译通过，`Cargo.lock` 不变；本地未提交的 vendor 改动都用不到。

此前的卡点（同日检查编译查出）：默认 ref `ci/gen-model-linux-20260918` 在 e3d-model 仓不存在，克隆这一步就失败；plant-mbd 远端 `main` 缺上面 6 个提交，编不过（4 处）；old-aios-core 旧 `main` 落后 caliber 32 个提交，编不过（27 处）。

还差：workflow 改动随本分支并入 `refactor`、再进 `publish` 才生效。plant-ui `master` 的内嵌前端比旧 CI 分支（10-13 的 0.1.14）新，下次部署前端会一起换新。以后推 `publish` 之前，各 vendor 仓的 `main` 要先推齐。

## 回归门（每次合入前跑）

- e3d-model：`cargo +nightly-2026-10-25 test --release --lib`（2026-11-07，`d139a9d`：464 通过、0 失败、5 忽略），以及 `attribute_reads_guard`（3）、`ancestor_reads_guard`（5）、`reference_reads_guard`（2）、`noun_coverage`（8），同一次 release 构建，全过。
- 39 站对拍：`gen-model/.scratch/rvm-fix/b39_run.ps1`，产物放 E 盘，和 `b39/rref3` 逐站逐对比较。判档只许变好，不许变差。
  **`--project-dir` 只传主项目目录（`ams000`），同时传 `--main-project-dir`**：外项目库由 `adopt_foreign_dbs` 按 `amssys` 的外项目表补。两个目录一起传会让 `acp*` 与 `ams*` 撞号，整组不外交，目录件全取不到（见 #27）。
- gen-model：`cargo +nightly-2026-10-25 test --lib -- e3d_query rvm_check model_impact::tests options::tests::rvm_check`（当前 84 个通过）。

## 最近进展

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
