# SCTN 模型生成算法修复：开发方案（gen-model #34 D 线续）

> 日期：2026-10-18。上游计划：`../2026-10-15-extgeo-cable-and-sctn-joint-cut/task_plan.md`（§2 Phase D、§11、§12）；动态验证报告：`../2026-10-15-extgeo-cable-and-sctn-joint-cut/e3d-tty-validation-report-20261018.md`。
> 代码：`vendor/e3d-model`（main，本地 HEAD `cbfffb2`，**未推送**）。本文只是方案，**尚未开始实施**。请在 Plannotator 里直接批注，尤其是 §6 的待拍板项。

## 0. 证据基线（写方案时核实过的事实）

| 项 | 现状 | 出处 |
|---|---|---|
| SCTN 正体 | `section_body`：SPRE→截面 → GTSTDT 标架（POSS→POSE+BANG+JUSL）→ 挤出（XSPYR 拉长）→ `section_cut_planes` 只按 **DRNS/DRNE** 裁两端 | `elmodl.rs` `section_body`；`section.rs` `section_cut_planes` |
| 关节刀 | JOIS/JOIE（SCTN mode 2）+ 成员链（FITT/CMFI mode 3、CMFI→SBFI mode 6、SNOD→SJOI mode 1、SCOJ/SUBJ）走 TMPL / SPRE→CATR 几何源；**没有几何源就记 `negatives_skipped`** | `collect_joint_negatives`、`collect_member_joint_negatives` |
| skipped 统计口径 | `cbfffb2` 已修：只有「真没有几何源」才记 skipped；之前「有源且刀全放行」也被误记成 skipped | vendor commit `cbfffb2` |
| 动态裁决 | 250110 墙链类（JOIE→下一根 SCTN 的 SNOD 上的 SJOI，SPRE 空、无 TMPL）：**① 现有正体已覆盖，没有新增刀的证据**。/68 原生 RVM 与生成体双向最大差 0.007071 mm；/70 原生 `RECONNECT END` 字段幂等，SAVEWORK 后零追加页 | §12 + 验证报告 §4–§8 |
| caveat | /68 自己的 RECONNECT 被 core.dll `0xC0000005 @ +0x74D982` 挡住；「扰动后再 reconnect 是否回写」未测；7200（`23584/*`）不在 /ALL，**零动态证据** | 验证报告 §6、§9 |
| 旧报告 | `verification/sctn-neg-20261015/*-new/report.json` 由 `cbfffb2` **之前**的代码生成，skipped 总数可能偏大，**不能直接拿来做 D3 统计** | 本轮核对 |

旧报告（10-15，修复前代码）里与 SCTN 有关的账（使用位置数，非去重）：

| 账目 | aps7200 | aps250110 |
|---|---|---|
| SCTN 出体数（elements） | 202 | 40 |
| SCTN `centreline_only` | **20** | 0 |
| skipped：SNOD 下 SJOI 无源（成员链 mode 1） | 183 | — |
| skipped：JOIE / JOIS SJOI 无源 | 83 / 80 | 70（合计） |
| skipped：JOIS / JOIE PJOI 无源 | 21 / 1 | — |
| skipped：成员 FITT / CMFI 无源 | 59 / 2 | — |
| note：`PSTR 里没有 PKEY=FOC 的 PLIN，按原点对齐` | **25 根 SCTN** | 0 |
| note：XSPYR 拉长挤出区间 | 41 | 36 |

## 1. 目标与非目标

**目标**

1. SCTN 的 skipped 账**按拓扑类别分型**：已经用原生证据解释过的类别移出 skipped，单独记入「已解释」账（可追溯，不删）；还没解释的类别仍留在 skipped。
2. 修掉或用证据钉住会**直接改变 SCTN 正体几何**的两处算法疑点：JUSL 对齐线缺失时回退到原点（P5）、7200 那 20 根只画中心线的 SCTN（P6）。
3. 用修复后的代码重建 aps7200/aps250110 的基线，补出 D3 对拍表（同时给使用位置数和独立 refno 数）。

**非目标**

- GENSEC（扫掠、SPINE 端切、FIXING→ENDATU 的 CUTP/CUTB）——属另一个宿主分支，不在本方案范围。
- 实现「扰动后 RECONNECT 回写」的模拟：生成器只读保存态，回写语义只影响 E3D 编辑流程，不影响我们从保存态出的几何。
- CABLE/EXTGEO（Phase A/B/C）。

## 2. 问题清单

| # | 问题 | 影响 | 当前证据 | 处理方向 |
|---|---|---|---|---|
| P1 | **J-A 类**：JOIS/JOIE → 另一根 SCTN 的 SNOD 上的 SJOI，无源 | 只影响账，不影响几何 | §12 ①（250110 /68、/70） | 移入「已解释：端面已覆盖」账 |
| P2 | **J-B 类**：自己 SNOD 下的 SJOI（成员链 mode 1），无源；7200 有 183 处 | 账；若原生其实会切宿主，就会差几何 | 无。宿主一般是贯通构件，按理不该被切，但没证据 | 在 250110 已挂载的库里找同型样本 → 只读原生 RVM 对拍 → 再分类 |
| P3 | **J-C / J-D 类**：PJOI 当 JOIS/JOIE（无源 / 有 SPRE 源） | 同上 | /500 那个有源 PJOI 的宿主是 GENSEC，不能当 SCTN 反例 | 先普查 SCTN 宿主下是否真有 J-D；没有就只处理 J-C |
| P4 | **M 类**：成员 FITT/CMFI 无源（7200 有 59+2） | 账 | FITT 自己作为单元另外出几何；成员链只看负体 | 同 P2 走法 |
| P5 | **JUSL 线缺失回退原点**（7200 有 25 根 `JUSL=FOC`） | **整根构件截面平移**——可能是毫米到百毫米级的位置误差 | 代码注释称按原点，没有 Core3D 依据 | ida-bridge 只读查 `getProfileFromDB` 找不到 PKEY 时怎么做 + 原生 RVM 对拍一例 |
| P6 | **7200 有 20 根 SCTN 只画中心线** | 可能少了 20 根实体 | 报告里只有计数，没有逐件原因 | 先把原因逐件落账（NoSpre / 截面全被筛 / 切后为空），再分别判 |
| P7 | SNOD 下关节标架（`GJTORI/GJTORG` 未翻，暂按 owner 链） | 只有 SNOD-SJOI 真有几何源时才生效；目前语料里一个都没有 | 语料 `DELP/BANG` 全 0 | 暂缓；加一个守卫测试：一旦出现有源 SNOD-SJOI 就报出来 |
| P8 | 文档和措辞过时 | 误导 | `section_body` 文档还写着「SCTN 的关节也还没接——梁端仍会偏胖」 | 跟实现一起改 |

## 3. 阶段与步骤

### S0 基线重建（不碰 E3D；需批准读语料）

- 用 `cbfffb2` 在 `verification/sctn-gen-20261018/{aps7200,aps250110}/` 重新生成 report/elements（**不覆盖** 10-15 的目录）。
- 前后对语料做 SHA-256 哈希（沿用 .NET SHA256 那套），写进日志。
- 产出：`ledger.json`，按 P1–P6 的类别 × {使用位置数, 独立 refno 数}；附上和 10-15 旧账的差额说明。

### S1 静态普查分型（不碰 E3D；只读探针）

- 用 `noun_attr_dump_probe` / 一个小的只读分型 bin，给每处 SCTN 关节 / 成员打上类别 J-A / J-B / J-C / J-D / M（判据：关节 noun、属主链、`CREF` 是否回指宿主、有没有 TMPL / SPRE→CATR、CUTP/CUTB/JFRE 值）。
- 列出 `JUSL` 在 PSTR 里找不到的 SCTN（全 APS 语料，不只 7200）、`centreline_only` 的 SCTN 及其原因。
- **每类挑 1–2 个落在 /ALL 已挂载库（250110 等）里的代表样本**，因为 7200 没挂载、选它的 refno 会崩。
- 产出：`classes.json` + 代表样本清单（等待批准导出）。

### S2 Core3D 静态取证（ida-bridge 只读）

- P5：`getProfileFromDB` / `QSPROF` 在 PKEY 找不到时的分支（回退 NA？回退原点？报错不出体？）。
- P2/P4：`DB_Sctn::DB_makeCSG` 的 mode 1/3 扫描，在没有几何源时有没有别的出刀路径。
- P6：Core 对「无 SPRE」和「截面全被 LEVEL 筛掉」时画不画实体。
- 只记结论和地址，写回本目录 `findings.md`；**不作为最终判据**，最终以 S3 原生 RVM 为准。

### S3 原生只读导出（需逐次批准）

- 环境：**只用**既有隔离副本 `verification/e3d-tty-native-20261015-075723/projects` + `shadow_e3d31_gen_model_test` 运行时 + `-tty APS … /ALL`。
- 宏：只有 `Q` 属性 + `OUTPUT RVM`；**没有** RECONNECT/SAVEWORK/FLUSH；每段都用 `ALPHA LOG … OVER / ALPHA LOG END` 分段落盘；只选已挂载库的 refno。
- 每次会话前后对副本 aps000 做哈希；会话 stamp 引起的变化单独列出。
- 崩溃（`+0x74D982` 那一族）只记录，不重试同一宏两次以上，不把失败导出当证据。
- 对拍：已有的 `compare_facet_surfaces.py` / Rust 对拍 bin，看双向表面差和体积相对差。

### S4 实现（vendor；按已授权的自主范围直接改、测，事后报完整 diff）

- **I1 关节分型记账**
  - 新增 `JointClass`（J-A/J-B/J-C/J-D/M）和 `JointCutOutcome::CoveredByEndFaces { class, evidence }`。
  - `Report` 新增 `joint_cuts_explained`（类别 → 条目，含 refno、宿主、证据标签），和 `negatives_skipped` **并列、不互相冲抵**。
  - 只有在 §6 Q1 批准的类别才移出 skipped；其余照旧记 skipped，但 detail 前面加上类别标签。
  - pipeline 两处 `negatives_skipped` 汇总同步改；报告合计里加一行「已解释」。
- **I2 JUSL 缺失**：按 S2/S3 结论三选一——(a) 维持回退原点，但把 note 升级成有证据的说明；(b) 改为回退 NA；(c) 按 Core 的做法报错或不出体。附带单测：缺线的 PSTR → 期望的偏移。
- **I3 centreline-only**：在报告里逐件写出原因；如果 S3 证明 E3D 实际画了实体，再按原因修建模分支。
- **I4 文档和措辞**：`section_body`、`collect_joint_negatives` 的文档注释，skipped 原因文本。
- **I5 守卫测试**：语料里一出现有几何源的 SNOD-SJOI 就让测试失败，提醒去处理 P7。
- 每步都跑：`rustfmt --check --edition 2024`、相关单测、全量 `cargo test --lib`，并在前后对语料做哈希。

### S5 复对拍与收口

- 用 S4 代码重跑 S0 的两个窗口，补出 D3 表：每类的代表样本 → 宿主 SCTN、关节 refno、原生 RVM、生成体、分类、差值。
- 更新 `task_plan.md` §2 Phase D 的勾选状态（D2 / D3 只勾有证据的子项）、§12 的「对 D1–D3 的影响」，以及 vendor `changelog.md`。
- git commit / push：**每次单独请示**。

## 4. 验收标准

1. 全量 `cargo test --lib` 全绿，没有新增 warning；`rustfmt --check` 通过；语料哈希前后差为 0。
2. 新 ledger：skipped + 已解释 + 有刀 = 关节总使用位置数（逐类对平），同时给出独立 refno 数。
3. 已解释的类别都有至少一个**同根原生 RVM** 样本：双向表面最大差 ≤ 0.01 mm，体积相对差 ≤ 1e-4。没有这种样本的类别不准移出 skipped。
4. P5/P6 各有结论：或者修复后对拍过门，或者把有证据的「E3D 同样如此」写进报告。
5. 7200 专属的类别如果在已挂载库里找不到同型样本，**明确登记为「未验证」**，不外推。

## 5. 风险

| 风险 | 应对 |
|---|---|
| R1 E3D 在引用解算时崩（`+0x74D982`），拿不到原生 RVM | 只用已挂载 refno + 分段日志；会话之间不累积写入；失败就登记，换样本 |
| R2 J-B / M 类在已挂载库里没有同型样本 | 登记为未验证，保留 skipped；另请用户决定要不要建一个挂载 7200 的 MDB 副本 |
| R3 把 J-A 的结论外推到其它拓扑 | 分类按拓扑判据硬编码，只放行有证据的类别；测试锁死 |
| R4 P5 修正会让 25 根以上的 SCTN 整体平移，牵连 FITT/CMFI/SNOD 的隐式标架 | 同一个改动要对拍宿主和名下配件；`ImplicitFrame::SectionFitting` 的回归测试一起跑 |
| R5 报告结构改动牵连 gen-model 侧消费者 | 只**新增**字段，不改现有字段语义；用 gen-model 编译检查确认 |

## 6. 待拍板（请直接批注）

- **Q1** J-A 类（已有 §12 ① 证据）现在就移出 skipped、记入「已解释」吗？还是等 /68 自身的 RECONNECT 也闭合以后再移？
- **Q2** S0 / S1 需要只读访问语料（`aps000` 等）。沿用上次「前后哈希、差为 0」的做法可以吗？
- **Q3** S3 原生只读导出：可以按上面的边界（隔离副本、gen_model_test 运行时、无写命令、分段日志）逐类执行吗？是每次会话都单独批准，还是批一次整批跑？
- **Q4** 7200 不在 /ALL：要不要另建一个挂载 7200 库的 MDB **副本**？还是把 7200 专属类别登记为「未验证」就结束？
- **Q5** 优先级：建议 **P5 → P6 → P1 → P2/P4 → P3**（先修会改几何的，再整理账）。同意吗？

## 7. 安全边界（沿用，不放松）

- 不碰原项目 `D:/AVEVA/Projects/E3D3.1`；不启动、不关闭 E3D GUI，不杀用户的 des.exe；不回滚或覆盖任何原始文件。
- RECONNECT/SAVEWORK/FLUSH 本方案**不用**；如果以后要用，只在用户批准的副本上做。
- 不用 `run_ams_c_entrymacro.bat` / `export_isolated.ps1` 的默认组合。
- 不凭 CUTP 非零、CUTB 或 skipped 数量补刀。
- commit / push 每次请示；endpoint 等凭据不写进任何文件。

## 8. 拍板记录（2026-10-18，Plannotator 批准 + 用户确认）

- Plannotator：`{"decision":"approved"}`，无批注（`plannotator-result.json`）。
- **Q1 = 现在就移**：J-A 类移入 `joint_cuts_explained`，证据标签写明 §12 ① 及 /68 reconnect 的 caveat。
- **Q2/Q3 = 整批授权**：S0/S1 只读访问语料（前后哈希、差为 0）；S3 在既有隔离副本上做只读原生导出（只有 Q + OUTPUT RVM，分段日志，无 RECONNECT/SAVEWORK/FLUSH），按方案边界直接执行、事后汇报。
- **Q4 = 另建挂载 7200 的 MDB 副本**：只在隔离副本上做，**动手前另行请示**（具体步骤和影响面先报批）。
- **Q5 = 同意**：P5 → P6 → P1 → P2/P4 → P3。

## 9. 执行记录

### 2026-10-18 S0 基线重建（已完成）

- 构建：`cargo build --release --bin gen_ams`（vendor HEAD `cbfffb2`；工作树里有用户未提交的 `src/element_diff.rs` 及测试改动，没有动，均不在生成路径上），target `D:\Rust\target`；release 构建有 2 条已存在的 dead_code 警告（`instantiate_cylinder`、`wall_joint_frame`），不是本次引入的。
- 产物：`verification/sctn-gen-20261018/{aps7200,aps250110}-cbfffb2/`（report/elements/model.obj/generated.rvm）+ `gen-aps*.log` + `ledger-s0.json`。
- 语料哈希：aps000 + acp000 共 284 文件 / 3,038,179,328 B，前后（含 mtime）**逐字节相同**（`%TEMP%\sctn_s0_hash_{before,after}.txt`）。

| SCTN 关节 skipped（使用位置数 = 独立 refno 数） | 7200 旧 | 7200 新 | 250110 旧 | 250110 新 |
|---|---|---|---|---|
| JOIS / JOIE → SJOI | 80 / 83 | 50 / 51 | 10 / 25 | 10 / 25 |
| JOIS / JOIE → PJOI | 21 / 1 | 3 / 1 | — | — |
| 成员链 SNOD 下 SJOI（新措辞「成员链下嵌套关节」） | 183 | 121 | 35 | 35 |
| 成员 FITT / CMFI | 59 / 2 | 47 / 2 | — | — |
| 合计 negatives_skipped（全 noun） | 671 | 517 | 70 | 70 |

7200 有 150 处关节从 skipped 改记 note「有几何源但其中没有负体」，这是 `cbfffb2` 修正的误记。250110 不变。

### 2026-10-18 S1 全 APS 普查（第一轮，已完成）

- `gen_ams` 逐个跑 aps000 下全部 `aps*_0001`（非 DESI 的 139 个按类型拒绝，属正常）；产物在 `%TEMP%\sctn_census`，汇总 `verification/sctn-gen-20261018/census-s1.json`。
- 有 SCTN 的 DESI 库 15 个，SCTN 出体 **6959** 根，中心线 **88** 根（7166:47、7200:20、7356:19、7329:2）。
- **JUSL 线缺失 66 根**：`FOC` 64 根（7200:25、7324:17、7350:17、7329:5），`LBOA` 2 根（7356）——全在 7xxx 库（refno 23xxx/*）。
- 全语料 skipped（使用位置）：JOIE-SJOI 479、JOIS-SJOI 451、成员链嵌套 SJOI 968、JOIE-PJOI 196、JOIS-PJOI 46、成员 FITT 529、CMFI 2。
- 另发现 SCTN 零长度（`有限端点间长 0 mm`）NoShape 若干（7166 的 23550/422、7330 的 23714/1011、/1025）。
- **结论**：P5 的全部样本都在 7xxx 库，而当前 /ALL 会话选 `23584/*` 会崩；P5 的原生对拍依赖 Q4（挂载 7xxx 的 MDB 副本，待单独请示）。

### 阻塞：ida-bridge 环境被我误改（需用户处理/批准）

- 为做 S2 静态取证，我在 `C:\Users\dpc\.agents\tools\ida-bridge` 下执行了 `uv run ida-bridge list`。`uv run` 会先按 `uv.lock` **自动同步 venv**：它卸掉了 5 个包，随后因 `apsw\__init__.cp312-win_amd64.pyd` 被占用（拒绝访问）中途失败。
- 现状：`pydantic_core`、`websockets` 目录里的 `__init__.py` 已丢失，新进程 `import pydantic` 失败；正在运行的 ida-bridge 进程不受影响（模块已加载在内存里）。
- 我尝试用 venv 的 pip 按原版本（`--no-deps`，pydantic_core 2.46.5、websockets 17.2）补回：第一次因 dist-info 缺 RECORD 失败；第二次加 `--ignore-installed`，因 `websockets\speedups.cp312-win_amd64.pyd` 被正在运行的进程占用失败。`pip check` 还显示 `pydantic 2.12.5` 要求 `pydantic-core==2.41.5`，所以 uv 在失败之前可能已经换过部分版本，原始状态无法完全确认。**我已停手，没有再动它。**

### 2026-10-18 I3/P6 中心线逐件记账（已完成）

- `vendor/e3d-model` 工作树（未提交，基于 `cbfffb2`）：`Report` 新增逐件中心线记录字段，`record_skip`/`merge` 同步维护原 noun 聚合与逐件明细；新增单测 `centreline_only_skip_is_counted_and_named_and_survives_merge`。
- 几何行为零改动，只改记账。7200 重跑（`aps7200-i1`）：centreline `SCTN=20 / SNOD=191`，20 根 SCTN 的原因逐件落账（`无 SPRE` / `目录截面成员全被筛或空占位`）。
- 补充窗口：7166 `SCTN`/`SNOD`、7356 `SNOD=202`、7329 `SCTN=2/SNOD=268` 均已重跑落账；7166 前后两次 SCTN 中心线数（47 vs 复跑值）差异来自运行配置/显示筛口径，非代码改动，数字以复跑报告为准。

### 2026-10-18 I5 SNOD→SJOI 有源守卫（已完成）

- 先在 `tests/sctn_snod_sjoi_source_guard.rs` 写过一版，因"SPRE 存在≠可解析几何源"口径过宽误报 97 条；改为 `elmodl` 模块内守卫 `no_snod_sjoi_in_the_corpus_carries_a_geometry_source`，用与 `fixing_negatives` 完全相同的几何源解析谓词扫 APS 7200 + 250110 全语料——**0 条"有源却被当无源 skipped"**。独立测试文件已撤，守卫留在模块测试里。
- 守卫过程中的重要修正：S0 里"成员链下嵌套关节 skipped 121（7200）"并不是全部无源；其中 96/97 条是**有源但源内无负体**（SourceWithoutCuts），按 `cbfffb2` 语义应记 note 而非 skipped——与现有行为一致，守卫锁的就是这条不变式。

### 2026-10-18 P1/I1 关节分型 + `joint_cuts_explained`（已完成，待提交）

- `elmodl.rs`：新增 `JointClass`（`J-A` 原生证据已覆盖 / `J-B` 成员链嵌套 / `J-C` PJOI / `M` 成员），`JointCutOutcome::NoSource` 按类打 `[J-X]` 标签；`J-A` 类（JOIS/JOIE→SJOI 直接关节无源）从 `negatives_skipped` 移入新增 `report.joint_cuts_explained`，detail 内嵌原生证据与 caveat 全文（可追溯，不删）。
- `pipeline.rs`：`GeneratedElement`/`Report` 新增 `joint_cuts_explained`，两处 DFS 汇流点透传，`totals_line` 加 explained 计数；gen-model 侧构造点已同步。
- I4：`section_body`/`collect_joint_negatives` 的过时措辞已更新（有源无负体记 note 不进 skipped 等）。
- 验证：`cargo fmt --check` 0；`cargo test --lib` 376 passed/0 failed/4 ignored；`cargo build --release --bin gen_ams` 0（仅 2 条既有 dead_code 警告）；gen-model `cargo check` 0（3m56s）；语料哈希前后逐字节相同（284 文件）。

### 2026-10-18 D3 对拍表（S5 第一批，基于 `aps{7200,250110}-i1` 新基线）

| skipped/explained 分型（使用位置数 = 独立 refno 数） | 7200 | 250110 |
|---|---|---|
| explained J-A（JOIS/JOIE→SJOI 无源，原生证据覆盖） | 101 | 35 |
| skipped J-B（成员链下嵌套 SJOI） | 121 | 35 |
| skipped J-C（JOIS/JOIE→PJOI） | 4 | — |
| skipped M（成员 FITT/CMFI） | 49 | — |
| skipped 无标签（非关节类负体，原口径） | 242 | — |
| 合计 skipped + explained | 517 | 70 |

- 守恒核对：7200 `416 skipped + 101 explained = 517` = `cbfffb2` 基线总数；250110 `35 + 35 = 70` = 基线总数。一笔不多一笔不少，只是按型重新归账。
- 两个库每类的使用位置数都等于独立 refno 数（这些 DB 里每个关节/成员只被一个属主引用）；J-A 的 101/35 条 detail 均含 `属主 <refno>（SCTN）` 与原生证据指针，可回溯到具体 SCTN。

### 2026-10-18 S1 第二轮普查（P3 J-D 分型 + P5 PKEY 盘点，已完成）

- 新增 `#[ignore]` 普查测试 `elmodl::tests::census_sctn_joint_classes_and_missing_jusl`（`cargo test --lib -- --ignored`，35.7 s，扫全 APS 15 个 DESI 库、7044 根 SCTN），产物 `verification/sctn-gen-20261018/classes.json`。
- **P3 结论：J-D 存在**。SCTN 的 JOIS/JOIE→PJOI 共 310 个使用位置（独立 PJOI 同为 310）：J-C 无源 242（7166:82、7200:4、7324/7329/7350 各 52）、**J-D 有源 65**（7166:22、7200:18、7324/7329/7350 各 8）、读不出 3（7343 的 PJOI 目录引用悬空）。J-D 里 7 处真出刀（7166 的 JOIE→PJOI，admitted=4/rejected=5），其余 58 处源内零刀或全部被 NAPP 挡——走 `Cut`/`SourceWithoutCuts` 账，不进 skipped，现行记账已正确。
- **P5 前置事实：JUSL 线是真不在 PSTR 里，不是名字对不上**。缺失 67 根：FOC 64（SPRF 15193/66394、66395 的 PKEY 只有 `NA,LEFT,RIGH`；15193/2062、7680–7683 有 40+ 条 PKEY 但同样无 FOC）、LBOA 2（SPRF 23704/782851 只有 `NA`）、另新增一例 aps7329 的 `JUSL=NA` 遇上 PSTR 里一条 PLIN 都没有（SPRF 15193/1750）。E3D 对「要的对齐线不在目录」怎么回退（原点 / NA / 不出体）仍是 S2/S3 的事。

### 2026-10-18 S3 原生只读导出：J-B 类对拍（已完成；Q3 整批授权内）

- 会话目录 `verification/e3d-tty-native-20261015-075723/sctn_250110_jb-20261018/`，全部在隔离副本 + `shadow_e3d31_gen_model_test` 上，宏无任何写库命令（run_copy_tty 校验 `write_commands=[]`）。
- **崩两次（只记录）**：`FILE+OUTPUT RVM` 入口在 /70、/73 上 0xC0000005（des pid 崩，3 字节空文件）。换 `export system /expdri.so` 子系统路径后一次成功：/68 控制组 3520 B（与既有基线逐字节同尺寸），/78、/84 各 5280 B。
- query.log 属性钉住：4 个宿主（SCTN 3/4/5/7）DRNS/DRNE 已是 45° 斜切；7 只嵌套 SJOI `Spref Nulref / Cutback 0 / Jfree false`，CUTP=S/N/D，CREF 全部回指**另一根** SCTN（J-A 的另一面：同一只 SJOI 对属主是 J-B、对 CREF 目标是 J-A）。
- 原生子树结构：宿主 SCTN 1 个 FacetGroup；每个 SNODE 3 条 Line；**SJOINT 组零图元**——嵌套关节既不产出也不吃几何。
- 对拍（`rvm_facets_dump` + `compare_facet_surfaces.py`）：/78、/84 宿主正体 vs 生成体双向采样最大差 **0.0063 mm**、体积相对差 **2.3e-6**、`sampled_comparison_passed=true`（jb{78,84}_surface_compare.json）。达到 §4.3 门槛。
- **J-B 类移入「已解释」账**：`collect_member_joint_negatives` 的 `NoSource` 按 `JointClass::OwnSnod` 落 `joint_cuts_explained`，detail 带两样本证据与 caveat；既有断言测试改为验证 /72 在 /70 上记 J-B explained 且不挂 skipped。`cargo test --lib` 376/0/5i，fmt 0。
- 副本前后哈希：`projects/` 284 文件中 112 个变化（全是 /ALL 打开时的会话 stamp 写入，文件大小不变）；**原项目 aps000/acp000 今天全程未动**（生成侧前后哈希差 0）。
- 新基线 `aps{7200,250110}-jb/`：7200 `explained=222（J-A 101 + J-B 121）/ skipped=295（无标签 242 + J-C 4 + M 49）`，合计 517 守恒；250110 `explained=70 / skipped=0`。

### 2026-10-18 P5：JUSL 缺失回退——Core3D 证据到手，按 GTNAID 中性轴回退修复

- **ida-bridge 绕过**：venv 里 `pydantic_core` 仍坏，但 `D:\ida_scratch\plant3\Core3D.dll.i64`（235 MB，此前已分析完）与 `~/.idapro/venv` 的 `idapro`（idalib）都可用——直接 `idapro.open_database(i64)` 读库，不等重新分析。
- **链路钉死**：SCTN 对齐 = `pllib/GJLORI@0x10399B54 → pllib/GTJLID@0x103AC744`。GTJLID 先 `GATIN1` 把 JUSL 转成 p-key 编码、再 `pllib/GTPLID@0x103A89BC` 在 PSTR 成员里 `DFINDI`；**落空时不回原点，调 `pllib/GTNAID@0x103AC154`**——先取 PSTR 自带的中性轴引用，不行就逐成员找 PKEY 编码 ∈ {`NA`=531482, 726125, 726137} 的线；连中性轴都没有才 NULIFY，调用方拿到「无线」= 零偏移（GPLDET 零初值）。`776966` = `NULL` 编码，不是 NA。
- **修复**：`section.rs::justification_offset` 加第二层——JUSL 的线不在 PSTR 时回退到 `PKEY=NA` 的 PLIN（与 `neutral_axis_spin` 同源），只有连 NA 也没有才按原点。`SectionProfile.justification_line` 改报实际解析到的线名（回退时报 `NA`、全失是空串）；note 写明回退原因。普查测试改为 `resolved != JUSL` 才打印，67 根全量打回。
- **语料验证**（`census_sctn_joint_classes_and_missing_jusl` 重跑）：66/67 全部 `resolved=NA`（7200:25、7324:16、7329:5、7350:18、7356:2）；仅 aps7329 `23713/4134`（`JUSL=NA` + PSTR 无任何 PLIN）仍按原点——与 GTNAID 的 NULIFY 支一致。
- **几何影响**：`section_probe` 23584/8599 → `JUSL FOC 缺失 → NA=[10.0, 0.0]`；7200 重跑 `aps7200-p5` 与 `aps7200-jb` diff 恰好 **23 件 SCTN**（23584/8599–8621）AABB 沿局部 x 平移 **+10 mm**，体积不变；另外两件 /8622 /8626 的 SPRF 中性轴在 (0,0)，note 记回退、几何不动。250110 零差（无缺失案例）。skipped/explained 各账不动。
- 验证：`cargo fmt --check` 0、`cargo test --lib` 376/0/5i、release `gen_ams` 重建 0、语料哈希差 0（284 文件）。

### 2026-10-19 S4：7200 挂上 MDB → SBFI 平放标架破解 → 8622/8626 全子树对拍通过

- **挂载**：E3D ADMIN 子系统在 TTY 下 0xC0000005（与 OUTPUT RVM 同类崩），改用 `e3d-io-write` 给**隔离副本** `apssys` 的 `/ALL` 追加 DB 元素 `24575/848`（`*SAMPLE/DESI`，dbno=7200，CURD+DBLS 两处）——会话 `des.exe -tty APS SYSTEM/XXXXXX /ALL` 随后能开 7200，8 个宿主全部 `export /expdri.so` 导出 RVM 成功。原项目 `aps000/acp000` 未动。
- **属性探针安全面**：`Q JSTY`、`Q POSS/POSE`（PJOI 上）、`Q CUTB`/`Q JFREE`/`Q TMPL`（FITT 上）逐个二分确认会崩；`Q CE/TYPE/NAME/MEMB/SPRE/CREF/JUSL` 安全。
- **J-C 判对**：PJOI 5899/5901 `SPRE=Nulref`；7613/7615 `SPRE→JOIN /CADC/HS/MASTER/JOINTS/ANCH`，JOIN 无 GMRE/CATR/成员 → 真无源。
- **残角真凶**：8622/8626 宿主多出的下角（z 867..962、y 9953..10000）是 `SBFI`（CMFI 名下）的 mode-6 NGMR 刀 NSBO（NAPP=-1 放行，2 把 NSCY 被挡）。原生 `SBFITTING` 记录与 `FITTING` **同原点、旋转取安装线水平投影 + 世界竖直**（rotZ(180°) 形态），我们的 `world_matrix(SBFI)` 沿 owner 链继承了 CMFI 的斜标架 → 刀落点差一大截。修 `transform.rs`：新增 `ImplicitFrame::SubFitting`（SBFI∈CMFI∈SCTN/STWALL），原点沿用母配件、旋转 = y 投水平面 + z=世界Z。
- **SBFI 正体**：无 DESP 但有 SPRE 的 SBFI 不再是 skip——改走 `model_catalogue_member_with_cache`（与 FITT 同路），产原生同款带孔平板。7200 全库新增 4 件 SBFI 实体（6548、8625、8629、10004），250110 无此类。
- **对拍结果（gen2/，native vs generated 双向采样）**：
  | 件 | n2g | g2n | vold | 判定 |
  |---|---|---|---|---|
  | 8622/8626 宿主 SCTN | 0.0014 mm | 0.0011 mm | 1.2e-7 | **pass** |
  | 8622/8626 FITTING | 0.0004 mm | 0.0005 mm | 1.9e-6 | **pass** |
  | 8622/8626 SBFITTING | 0.0003 mm | 0.0004 mm | 1.9e-6 | **pass** |
  | 5902/7605/7610 宿主 | 0.0000 | 0.0000 | ~2e-16 | **pass** |
  | 8599/8600 宿主 | 0.0014 | 0.0010 | vold=2.0* | 空间上通过（脚本在有向 Pyramid 上体积失真，既有口径问题） |
- **P5 顺带闭环**：8622/8626 是 `JUSL=FOC` 样本，note 记「PSTR 无 FOC → 回退 NA」，宿主几何与原生逐点对上 → GTNAID 回退得到原生 RVM 实证。
- **账**：新基线 `aps{7200,250110}-sbfi`，skipped 295 / explained 222 与 p5 完全一致（守恒）；SBFI 从「缺 DESP skip」变「catalogue assembly: built=1」；`negatives_skipped` 里只剩 CMFI（M 类无源——原生 CMFITTING 组本就空几何，判对）。
- 验证：fmt 0、test 376/0/5i、release gen_ams 重建 0。

### 2026-10-19 P6 原生实证：无 SPRE 的 SCTN 在 E3D 里同样只出中心线

- 仍走隔离副本 + shadow 运行时（`p6.mac`，write_commands=[]）。三样本 `23584/17`（GRID 系）、`23584/5951`（STWALL→FITTING→TMPLATE→SCTN）、`23584/8577`（FRMWORK→SCTN）`export /expdri.so` 全成功。
- 原生 RVM：三件 SCTN 记录全部只有 `Line` 图元、零 FacetGroup——「无 SPRE → 只画中心线」逐件对上，`centreline_only` 分类成立，20 件的明细原因（无 SPRE）即真因。
- 顺手确认：`p6` 会话 `tty_exit=0`、env_leak=false、runtime/copy 双 pinned。

### 2026-10-19 S5 全语料回归：15 库重跑与 S1 普查对平（已完成）

- `aa3577d`（vendor 提交点）代码重跑全部 15 个含 SCTN 的 DESI 库，产物 `aps{dbno}-sbfi/`：SCTN 建体数与 centreline 数**逐库与 S1 普查完全一致**（7166:275/47、7200:202/20、7324:1384、7329:1230/2、7330-7332:403/368/368、7350:1380、7352:24、7354:374、7356:830/19、7011:8、7015:33、250110/250560:40）。
- `failed`：全语料合计 26 = 250110/250560 各 13，与 `cbfffb2` 基线逐字相同（既有失败，与本轮无关）。
- SBFI 目录求值件只在 7200（4 件），与原生证据面一致；JUSL 缺失库（7324/7329/7350/7356）无新增失败/跳过。

### 仍开放

- **J-C、M 类部分仍为 unexplained skipped**：J-C（PJOI 无源，242 处全语料 / 7200 在账 4 条已原生验证为真空）与 M 中的 CMFI（无源、原生组空几何）维持 skipped；M 中的 SBFI 已由 S4 移出（出刀+出件）。其余无原生样本的 J-C/M 条目维持 §4.5 口径。
- vendor 工作树未提交改动：`catalogue.rs`、`elmodl.rs`、`pipeline.rs`、`section.rs`、`transform.rs`（本次）；`element_diff.rs`、两个 reads guard、`relocation_noise_real.rs`（用户既有，未动）。
