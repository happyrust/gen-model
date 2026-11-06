# 模型问题跟踪清单（2026-11-06）

依据：39 站全量对拍（最初是 p2a 加上 e3d-model `46f8d8f`，共 196,329 对；现行基线 `b39/rref3`，见「回归门」）、e3d-model 与 gen-model 的 open issue。
状态栏：`待办` / `进行中` / `已修待推` / `待确认` / `已关`。改动后在「最近进展」里记一行日期和结论。

## 一、几何对不上（39 站对拍）

| 优先 | Issue | 问题 | 涉及件 | 站 | 状态 | 下一步 |
|---|---|---|---|---|---|---|
| — | e3d-model#21 | SJOI 斜交连接梁的 IJOINT 平移：缺 GJTORG 第二次求交的闭式 | 9 件 off（3.4–12.7 mm）：`/MDS/VT01`、`VT02`、`VT05`、`FT13` 下的 `IJOINT1/2/3` | `23713/2343`、`15516/2`、`23734/1` | 已关 | `222a0ef`（main）：`GJTORG` 收尾按反编译重写（`JFRE` / 梁认领门 + 两次求交）。`rref3` 里这几站的 IJOINT 全 exact（如 `23713_2343` 的 `/MDS/VT05/2-BAR-1/S1/IJOINT1`、`/MDS/FT13/2-BAR-1/S1/IJOINT1`）。遗留的 perp 分支经验规则记在二节 |
| — | e3d-model#23 | HVAC 头槽位断面解不出时，E3D 写半径 0 的圆柱占位，我们记 unbuildable 不写 | 1 件只在 E3D：`TUBE 1 of BRANCH /-CAM-E-2-H-5302` | `24381/46775` | 已关 | `8e3cf67`（已推 main）：照 E3D 写 0 径 Cylinder 占位，只在 E3D 1 → 0 |
| P2 | e3d-model#24 | 隐式管身某段 E3D 不画，我们照画 | 1 件只在我们：`TUBI 9 /Copy-of-1RCS007MN-TUBE/801VP`（2315 mm） | `24381/101405` | 待办 | 两条假设已否证（见 issue 评论）：偏轴 ATTA（40 个同类槽位只此一个不画）；离开侧 SPAMAP 第二字为空（aps250142 STIF `2013286686/123` 同样为空但 E3D 照画，首版 `4b38b0c` 已 revert）。APS 239 库扫过：空离开键可画槽位 4 个（2 个 HVAC STIF，E3D 照画；2 个 aps7200 1 mm 吊架段），没有第二个管道样本。需 E3D 侧单件取证 |
| — | e3d-model#25 | 零长 SCTN 名下的 SJOI J1 E3D 照画，我们整串 skipped | 2 件配不上：`/MDS/SPECIALS/PIPE/3-BAR-2/S1/J1`、`4-BAR-2/S1/J1` | `23714/1` | 已关 | `b94615c`（已推 main）：零长轴取 +Z，两只 J1 exact，不可配对 2 → 0。规则只有 2 个样本，且 DRNS 恰为 +Z，遇到 DRNS ≠ +Z 的零长 SCTN 时复核 |
| — | e3d-model#22 | RTOR 内半径大于外半径时 E3D 照画，我们判退化 | 2 件（RTORUS 1/2 of SUBE 10 of `/Copy-of-R43-R540-008`） | `24381/101405` | 已关 | `46f8d8f`（github main）；39 站零退化 |
| — | e3d-model#26 | FRMW 名下 PNOD 里的 PJOI `GTYP` 为空值时，E3D 有时照画、我们整串 skipped | 3 件配不上：`/MDS/FT62/1-V1/JOINT1`、`/MDS/FT64/1-V1/JOINT1`、`/MDS/FT64/1-V2/JOINT1` | `15516/2`、`23734/1` | 已关 | `5775ecb`（已推 main）：判据是宿主 SCTN 的 `RREF` 有没有指到这只 PJOI 的宿主 PNOD。`GTYP` 空 + RREF 命中 → 画（ams7324/7350 各 3 只）；`GTYP` 空 + RREF 不命中 → 不画（ams7324 FT61 的 18 只、ams7999 的 500 只、ams8000 的 248 只，与 E3D 一致）。39 站零回归，6 件 unkeyable → exact。`RREF` 是新增读点，已进读取集合（181）与 `ancestor_reads_guard` 的 `WithinUnit` 登记 |

| — | e3d-model#27 | TUBI 取不到目录件外径时回落到达侧通径，半径差 3–9 mm | 162 对 `off`（全是 TUBI） | `24383/73927`、`24383/66456`、`24381/101405`、`24381/177373` | 已关 | **不是代码缺陷，是验证脚本用法错**：`b39_run.ps1` 给 `gen_ams` 传了两个 `--project-dir`（ams000 + acp000），`acp7000_0001` 与 `ams7000_0001` 撞号，`DirectoryDbResolver` 判「分不出正身，整组不外交」，库 7000/7600/6000 全解析不出，目录件取不到 → `tubes_catalogue_od=0` → 管子回落通径。改成「主项目目录 + `--main-project-dir`」走 `adopt_foreign_dbs` 后：`24383/66456` 的 `generated` 1155 → 4116（与 E3D 成员数一致）、`failed` 1605 → 0、`tubes_catalogue_od` 0 → 1422、对拍全 exact。39 站全量：`off` 162 → 0、`only_baseline` 5441 → 0、`exact` 191946 → 199180，两侧都有的键里零变差 |

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
| P3 | e3d-model#20 | SCTN 端关节在 mode 0 下 NAPP 2 刀还切不切：13 处关节缺 E3D 真值 | 进行中 | MDS 三处（7324 / 7329 / 7350 的 `/MDS/FT17/2-V1`）其实在 39 站里，E3D 不切、exact（issue 评论 11-05）。剩 aps7166 `/Endplates-Column-JOINT` 那 10 处：aps7166 不在 APS 的 `/ALL`，要在项目副本里挂库后导出 |
| P3 | e3d-model#21 遗留 | perp 分支（`\|v44·v38\| ≤ 5e-6`）整支留站位是经验规则：其中 42 件反编译可证（第一次求交分母为 0），另 100 件（VT10 的 `*-BAR-*`、FT04H / FT04V 的 `*-V1`）按反编译应能求交、E3D 仍留站位 | 待办 | 用 `examples/sjoi_gjtorg_probe.rs --all --verbose` 记下 `v44` / `v38` / `v45` 各在哪个坐标系里点乘；活桥断点前不改 `datum.rs` 那一支（笔记 §54.3） |
| P3 | gen-model#63 余项 | aps7200 `/INNER-KICKPLATE` 差 5.63 mm | 待办 | 在 E3D 里造同形样本 |
| P3 | gen-model#63 余项 | FN09 楼板 | 待办 | 等 E3D 按修正清单改完后重导、对拍 |
| P4 | gen-model#62 | 对拍覆盖面扩展：MDS AISC / DIN / SPECIALS、ams8000、APS、ams2510xx 的 FIXING 真值 | 只差 tag | 第 0–5 步都已完成，0.1.43（`cb0fbd8a5`）与 0.1.44（`83aa3c250`）都已上线并抽查对上；缺 `v0.1.43` / `v0.1.44` 两个 tag（最新仍是 `v0.1.38`） |
| P4 | gen-model#86 | PML 还有 3,676 条程序用到未实现的操作码（1402/112/103/1006/1012/105/1309/108/104；原写「约 8000」是算错，见 issue 更正） | 待办 | 普查（`vendor/e3d-model/examples/pml_opcode_census.rs`）先给每组写全文代表词、按第二词分桶，再逐桶反推 |
| — | e3d-model#16 | 算法文档（持续更新，不算缺陷） | 持续 | — |

## 三、生成流程与线上落地

| 优先 | Issue | 问题 | 状态 | 下一步 |
|---|---|---|---|---|
| P1 | gen-model#88 | 批次 worker：7997 `model_only` 停在 `durable_pending`（6772/6772 已 ACK），挡住后面排队的 7998 / 8000 | 进行中 | 卡点不在 durable 栅栏：栅栏在 6772/6772 时已返回，任务卡在其后、`task_end` 之前的 `refresh_equip_derived`（设备派生量增量重算，11-01 `5b6eca135` / `40583bcea` / `d85c2cf61` 起）。它沿用 `durable_pending` 阶段名，没有进度、心跳和期限；整库 `model_only` 时对全部根做子树展开和楼板柱子空间反查。`wipe_reinit` 不走这段，所以能跑完。修法：独立阶段 + 进度、合作式预算、整库走全量重算 |
| — | gen-model#73 | 初始化时，对「模型目标没变」的根沿用旧 vendor 生成的行，所以换了 e3d-model 的几何修复也不会落地 | 已落地，待线上复核 | `43dd85c5a`（ADR-093）：生成器指纹改读 vendor 构建身份，水位行加库级落款 `model_generator`，落款不一致就整库重排；已在 `origin/publish`。部署后核 `/health.core.e3d_model_build_id`、`dbnum_watermark.model_generator` 全等于当前指纹、启动日志 `reason=model_generator_changed`，再重启不再排，即可关单 |
| P2 | gen-model#74 | 一根 failed 让整库 lagging，`model/records` 对整库返回 0 条（ams7330：364 根里 4 根失败） | 进行中 | 三点的代码大多已提交：按根回读 durable（`4fa7ef4ab`），初始化冷根幸存件照投影、roots 的 `ready_source` / `durable_ready_total`（`5133bc271`）。剩初始化回执写 `published survivors` 的那段（原在主工作区未提交）和专门单测 |
| P2 | gen-model#78 | release 二进制在范围生成中段 `0xc0000409` 崩溃，每次同一偏移，根因未定 | 进行中 | Windows 上 `0xc0000409` 是 Rust abort 的统一出口：栈溢出、OOM、panic 后 abort 都落在同一偏移，「同偏移」推不出确定性路径。最可疑的是几何 rayon 池没设栈大小（默认 2 MB），而发布包 `Start-AMS.ps1` 不像其它脚本那样设 `RUST_MIN_STACK`。先补崩溃留痕（VEH + minidump、alloc error hook、release 出 PDB）再复现 |
| P3 | gen-model#72 | 模型侧生成失败没有停跑和退避的记录，和数据批次的 `DATA_BATCH_PARK_AFTER` 不对称 | 进行中 | ADR-090 D1 已拍板：按根指数退避（30 s 起、翻倍、10 min 封顶），不 park；判死线 `DEAD_ROOT_STREAK = 5` 不动 |
| P3 | gen-model#34 | 结构连接级联（JOIS/JOIE ↔ CREF）默认关闭，关节负体落地后存在正确性缺口 | 待办（ready-for-human） | ADR-090 D2：先过语料门（在项目副本里改 MDS 关节 FIXING → 被截 GENSEC 所在根进 regen、两态对照、D10 分桶时延不慢）再翻默认。SCTN 端面斜切走的是 SCTN 自己的 `DRNS` / `DRNE`，`CUTB` 只挪关节原点；J-C（无源 PJOI）与 CMFI 空组仍记 skipped |

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
| P3 | gen-model#87 余项 | QUIT 后 `des.exe -tty` 不退出的根因：实测 5/5 站都会发生，每站固定多等一个宽限期（60 s） | 进行中 | 头号假设：包装宏只有一条顶层 `QUIT`，导出宏停在 `export` 模块里，这一条只退出模块；仓里能自己退出的导出宏收尾都是 `QUIT` / `FINISH` / `FINISH`，登录探针不进模块所以能退 |
| P3 | gen-model#79 | CI 有四道门长期坏掉，还有一次把账户配额撑爆 | 大部分已修 | `windows-tests.yml` 的触发分支、缓存 key、Save 条件、junction 自检和两个护栏脚本已修（`c566dce03`…`a10272347`，在 `origin/refactor`）；`linux-binary.yml`、`windows-binary.yml` 的 cargo 缓存 key 仍带 `run_id`，每跑一次多存一份 |

## 六、待推送的本地提交

2026-11-06 晚核对：上面原列的 `b4b6f72`、`87dee7b`、`108f08fd`、`b77ff105` 都已在 `origin/refactor`；e3d-model、e3d-io 的 `main` 与 GitHub 一致。还没推的：

| 仓库 | 分支 | 提交 | 内容 |
|---|---|---|---|
| gen-model | `refactor`（本地） | `972b06cfa` | feat(mbd): layout_pipe 在 meta.source_version 填入读取版本 |
| gen-model | `fix/model-open-issues-1106`（worktree `gen-model-fix1106`） | 见分支 | 本清单「三、五」各项的修复，过回归门后并入 `refactor` |

`origin/refactor` 比 `origin/publish` 多 11 个提交（含 #87 的修复）。#73 的修复已在 `publish`，部署了它的环境不必再在换 vendor 后手动 `model/rebuild`。

## 回归门（每次合入前跑）

- e3d-model：`cargo +nightly-2026-10-25 test --release --lib`（当前 463 个通过），以及 `attribute_reads_guard`、`ancestor_reads_guard`、`reference_reads_guard`、`noun_coverage`。
- 39 站对拍：`gen-model/.scratch/rvm-fix/b39_run.ps1`，产物放 E 盘，和 `b39/rref3` 逐站逐对比较。判档只许变好，不许变差。
  **`--project-dir` 只传主项目目录（`ams000`），同时传 `--main-project-dir`**：外项目库由 `adopt_foreign_dbs` 按 `amssys` 的外项目表补。两个目录一起传会让 `acp*` 与 `ams*` 撞号，整组不外交，目录件全取不到（见 #27）。
- gen-model：`cargo +nightly-2026-10-25 test --lib -- e3d_query rvm_check model_impact::tests options::tests::rvm_check`（当前 84 个通过）。

## 最近进展

- 2026-11-06（晚）：按代码与 `rref3` 核对改口：e3d-model#21 已由 `222a0ef` 修好并关闭（P1 几何清零，perp 遗留降为 P3）；#73 已落地（`43dd85c5a`，在 `publish`）；#74 三点大多已提交；#87 已在 `origin/refactor`；#79 只剩两个二进制工作流的缓存 key；#62 只差 tag；新增 #88（卡点在设备派生量重算，不在 durable 栅栏）。修复在 worktree `gen-model-fix1106` 的 `fix/model-open-issues-1106` 分支上做，主工作区的未提交改动不动。
- 2026-11-06：SITE RVM 复测（干净构建，独立后端 8122）：10 个原失败站里 7 个全过（`24381/46775`、`23714/1`、`24381/535`、`23705/1`、`23713/2343`、`24381/177373`、以及 `24381/101405` 只剩 #24 那 1 件）；`15516/2`、`23734/1` 各剩 3 件 unkeyable（新开 e3d-model#26）；`23705/289` 因依赖未提交的 e3d-io / e3d-model 改动而出现 8 只 Z 偏移。
- 2026-11-06：#26 定位到宿主 SCTN 的 `RREF` 判据，`5775ecb` 推 main 并关闭；39 站零回归，6 件 unkeyable → exact。
- 2026-11-06：#27 查明为验证脚本用法错（两个 `--project-dir` 导致 `acp7000_0001`/`ams7000_0001` 撞号、库 7000/7600/6000 全不外交），非代码缺陷，已关。`b39_run.ps1` 改为「主项目目录 + `--main-project-dir`」走 `adopt_foreign_dbs`。39 站全量重跑：`off` 162 → 0、`only_baseline` 5441 → 0、`exact` 191946 → 199180，两侧都有的键零变差。新基线 `b39/rref3`。
- 2026-11-06：#22 合入后复验并关闭。#24 补 APS 全库取证（无管道第二样本），评论在 issue。kind_differ「负体不按 TUFL 筛」实验在 23710/1 多出 276 对 kind_differ，否证，未改代码。
- 2026-11-06：#23（`8e3cf67`）、#25（`b94615c`）推到 e3d-model main 并关闭，`46f8d8f`（#22）随之在 github main；39 站对 `m1076db`、`m1076` 零退化，`=24381/46775` 只在 E3D 0、`=23714/1` 不可配对 0。#24 两条假设都被否证，首版已 revert，issue 保持开启。`23705/289` 的 4 对 kind_differ 定为可接受残差（见上表）。
- 2026-11-05：#22 本地修复（`46f8d8f`），39 站零退化；#87 本地修复（`b4b6f72`），实机 5 站全部 pass。两个 issue 下都已留评论，都没关。
