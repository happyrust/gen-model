# 开票审核：gen-model 的 issues 还剩哪些遗留问题

> 审核时间：2026-10-26 00:35–01:05（本机时区）。审核会话：Bridge（AstraFable MCP Bridge 3.2.0）接入后**全程只读**——
> 未改任何源文件、未跑 cargo（另一会话在 `gen-model-issue68` 上跑 `cargo test --test increment_cases_registry`，不抢 target 锁）。
> 口径：`workspace=d:\work\plant-code\old\gen-model`，分支 `refactor`。审核开始时 HEAD `b2f61ddcb`，审核期间
> 落了一笔 `01870f8dd`（P1 第六步），本文件按 `01870f8dd` 的状态写。
> 事实来源：GitHub API（`gh api repos/happyrust/gen-model/issues/N`）、`git log/branch/ls-files`、`findstr` 命中数、
> `tests/fixtures/increment_cases.json` 逐条统计。**每条结论后面的括号就是可重放的证据。**

## 0. 一句话

9 张开票里：**1 张代码已落该关**（#65）、**1 张已被新票取代该关**（#31）、**2 张票面比事实旧**（#62 / #63 的交付项）、
**2 张零进展的性能票**（#68 / #69）、**2 张卡外部前置或语料**（#34 / #45）、**1 张是追踪票本体**（#61）。
另外有三处**不在任何票面上**的遗留：增量测试套件的 P2/P3 共 21 条未落、D6 必败注入钩「有钩子没消费者」、
交接文档停在 10-16 没记这 10 天的四条工作线。

## 1. 逐票结论

| 票 | 标签 | 真实剩余 | 证据 |
|---|---|---|---|
| #65 按需显示的新旧判据把未触根判成过期 | bug, increment | **代码已落、票该关**；只剩「有目录项的根走便宜路」在现有工程里**没有样本** | `5c70896fa fix(model-freshness): 按需显示的新旧判据听库级承诺，文件身份改记冻结锚，重放闸放行停在基线之前的回执（#65，ADR-089）`；ADR-089 首行状态「**已接受**（2026-10-24）」；`target_current_under` / `memory_root_current` / `republish_gate` 在 `room_read_through.rs`/`on_demand_model.rs`/`rigid_motion.rs` 命中 26 处；#65 的 10-25 评论：「NOZZ 不读目录，读目录的 BRAN / STRU 走不了刚体路（HPOS / TPOS）……Q4 到此为止」 |
| #68 规划器 plan 费用随 roots_T 线性（7997 一根小窗 13.8 s） | enhancement, increment | **零进展**。只有一条空分支 | `git log --grep="#68"` 仅计划文档 `c04571d1c`；`fix/issue-68` @ `448c1c268`，`git rev-list --left-right --count refactor...fix/issue-68` = `2 0`、`git diff --stat refactor...fix/issue-68` 为空；登记表 `INC-I02` 仍 `todo`（P3） |
| #69 每次 SAVEWORK 连带一条 ~11 s 的 amssys 批次，与设计库窗口同队列 | enhancement, increment | **零进展**，连分支都没有（评论数 0） | `git log --grep="#69"` 仅 `c04571d1c`；`INC-I03` `todo`（P3） |
| #34 结构连接级联（JOIS/JOIE ↔ CREF）默认关 | bug, increment, ready-for-human | 默认仍关；4 条验收全未勾；卡在外部前置「SCTN 关节负体 / 端部切割第 4 阶段落地」。前置 #33 已关 | `src/options.rs:2337`「`model_structural_connection_cascade`，**默认 false**」、`:2349` 取值函数；门在 `window_root_plan.rs:1240` / `:1256`；关闭提示 `structural_connection.rs:411` |
| #45 Extract（`_NNNN`）/ Global FLUSH 引起的主库会话重编 | enhancement, increment, ready-for-human | **无进展、无语料**：要「用带 extract 的项目录一条 FLUSH 链」再把结论写进 ADR-028 追记，第一步就没做 | 无相关提交；票面 3 条验收全未勾 |
| #63 模型生成问题汇总（已修 58 / 待解决 6） | — | 6 项里 2 项**票面已过期**：项 6「0.1.43 待推送」实际已推——`7fdc4c15c` 在 `origin/refactor`、`origin/publish`、`ci/linux-build-20261024/20261025` 上，publish HEAD `cb0fbd8a5`，且有 `deliver/gen-model` worktree 停在它上面。**但**：`git tag --sort=-creatordate` 最新仍是 `v0.1.38`，且部署后核对串 `0.1.43+g` 在 `changelog.md` 与本仓 orientation 验证文档里 **0 命中** → 缺 tag 与部署复核留痕。其余 4 项都在 e3d-model / 数据侧：3 条支管管身条数（e3d-model#8）、e3d-model#1 可关、FN09 负体在 E3D 里改后重导、#59 的 `0xc0000409` 崩溃根因 | `git branch -a --contains 7fdc4c15c`、`git tag`、`findstr /C:"0.1.43+g" changelog.md verification\model-orientation-increment-20261016\verification.md` |
| #62 对拍覆盖扩展计划 | enhancement | 第 0–4 步票面都写了「已完成 / 已定」，只剩第 5 步交付收尾（与 #63 项 6 同一件事）；票内「aps7200 整库（副本）还没对拍」已被 #63 记成对完（2352 对、off 3 归 e3d-model#7）→ **票面比 #63 还旧一格** | issue 正文逐条 |
| #61 增量更新剩余问题清单（10-22 复核，10-25 更新） | increment | A 类两项 = #34、#45；B 类**还有 3 条未勾**：① 真机 `SAVEWORK → HTTP execute → 队列 / worker` 整链里「数据批次那半没有活」；② 多项目同进程的窗口计划缓存命中 / 隔离（§13.4）；③ 隐式管身的增量腿在真库门上**没有判别力**（`e3d-model/src/increment.rs:116-117`，要等目录库进门）。另：目录核对三条入口只剩 `catalogue_file_advanced` 一种触发没走过。C 类留档；D 类 8 条「记录在案、不改即接受」 | issue 正文勾选框 + 10-25「B·真实工程两库」评论 |
| #31 增量更新未覆盖项清单（09-22 审阅） | increment | 16 子项 14 已关，余 #34 / #45，已被 #61 接管。按 #61 自己写的关闭条件「或被更新的复核快照票取代」→ **#31 该关** | 两票正文首段 |

## 2. 不在票面上的遗留

1. **增量测试套件（`docs/plans/2026-10-25-incremental-update-automated-test-suite-plan.md`）**：P0 / P1 已收（P1 第六步 `01870f8dd`，2026-10-26）。
   登记表 `tests/fixtures/increment_cases.json` 共 55 条 = `has` 32 / `partial` 7 / `todo` 14 / `corpus` 2。未落的 23 条精确分组：
   - **P2 13 条**：`B07` `E02` `E03` `G03` `H02` `H03`（todo）+ `F03` `F05` `G01` `G02` `H05` `J01` `J02`（partial）
   - **P3 8 条**：`C05` `C06` + 预算 `I01`…`I06`
   - **P5 2 条**：`C07` `E04`（`corpus`，如实记账）
2. **P3 / P4 一行没写**：`tests/fixtures/increment_budget_baseline.json` 不存在、`scripts/nightly-local.ps1` 不存在；
   `src/bin/l3_suite.rs` 的场景仍是 `m1 m2 m3 f4 f5 f6 f7 f8` 八个，计划 §5.3 要加的 `x1 / l1 / e1 / p1 / n1 / u1`
   与 4 个新 `Expect` 变体一个都没有。
3. **D6 必败注入钩断档**：`AIOS_TEST_FAIL_ROOTS` 已落在 `src/data_interface/model_refresh.rs`
   （`injected_generation_failures`，本仓唯一仍脏的源文件 +51 −1），但 `findstr /S "AIOS_TEST_FAIL_ROOTS"
   tests\increment_replay\main.rs` 命中 **0** —— 钩子有了、用例没接，所以 `H02` 仍 `todo`、`H05` / `F05` 仍 `partial`。
4. **计划自相矛盾**：头部修正已把「L2 进 CI」改成「L2 本机 nightly，进 CI 待自包含语料」，但 §9 验收总门 ② 仍写
   「CI `offline-increment-regression` 含 L2 档并连续绿两周」。
5. **交接文档过期**：`AGENTS.md` 与 `docs/agent-handoff/*` 停在 10-16（最后一条记录是「合并执行 E1–E终」），
   这 10 天的四条线（`fix/issue-68` 分支、`fix/nearby-coverage`、replay P0–P1、0.1.43 交付）都没进交接；
   协作仍标 PAUSED / 仅留档，未恢复 A/B 调度。
6. **工作树脏 69 项**（`git status --short`，含 `output/` 下 45 个未跟踪脚本与 `?? 开发计划/`）。
   Bridge 的 changed_files 基线是空的新窗口，不代表 Git 干净。

## 3. 建议顺序（按成本 ÷ 收益）

1. 关票面过期项（零代码风险）：#65 附 `5c70896fa` + ADR-089 已接受后关闭，把「目录项便宜路无样本」并进 #61 B 类；
   #31 按自己的关闭条件关闭并入 #61；#62 / #63 的交付项改写成「已推送、缺 `v0.1.43` tag 与部署后复核留痕」。
2. 补交付最后一米：打 tag（或明确「按提交号发布」并写进票）+ 跑部署后核对（版本串 / vendor 短哈希 /
   `model/rebuild` 分专业抽查）并留一份 verification。这是 #58 / #62 / #63 共同卡的同一件事。
3. #68 / #69 一起做：两者都缺逐段计时，先把 §5.4 的预算采集与基线文件落下来（顺带翻起 P3 的 `I02` / `I03`），
   有数字再谈优化；空分支 `fix/issue-68` 可以直接用。
4. 接 D6 钩子：`AIOS_TEST_FAIL_ROOTS` 透传进 harness 的 `service_env`，写 `INC-H02`（同右端连败 5 次停自动重跑、
   新会话 / 人工执行清零）、翻 `H05` / `F05`，把仍脏的 `model_refresh.rs` 与用例收进同一笔提交。
5. #34 / #45 保持开着，各补一条评论说明卡在哪条外部前置，避免被误当成可动工项。

## 4. 可重放的命令（只读）

```cmd
gh api repos/happyrust/gen-model/issues/61 --jq .body
git log --oneline -1 refactor
git log --grep="#65" --oneline
git branch -a --contains 7fdc4c15c
git tag --sort=-creatordate
git rev-list --left-right --count refactor...fix/issue-68
git ls-files --eol docs/plans
findstr /S /N /C:"model_structural_connection_cascade" src\options.rs
findstr /N /C:"id: \"" src\bin\l3_suite.rs
python -c "import json;d=json.load(open('tests/fixtures/increment_cases.json',encoding='utf-8'));c=d['cases'];print(len(c));print([x['id'] for x in c if x.get('status')!='has'])"
```

---

## 附：P2 §6 D6 必败注入钩的实测结论（2026-10-26 上午补）

用例 `tests/increment_replay/cases/ams8000-equi-fail-inject.json` 在本机影子根连跑四轮（`ams8000-equi-move-chain` 夹具：725 → 726 → 727 注入 → 728 清钩子）。跑法要点：`AIOS_REPLAY_PROJECT_ROOT` 要指到**含 `AvevaMarineSample` 的那一层**（本机 `..\_runs\ams-replay\E3D3.1`）；指到上一层会整档 skip 且 exit 0——日志里的「1 passed」是跳过，不是跑绿。

| # | 事实 | 证据 |
|---|---|---|
| 1 | 钩子确实生效 | 服务日志 `[测试钩] 根 24384/24776 按 AIOS_TEST_FAIL_ROOTS 记成生成失败，不调用生成器` |
| 2 | 注入窗的口径是 **`partial`，不是 `failed`**：任务落 `partial`、`model_sesno` 停在 726，`applied` 与 `file_latest` 都到 727 | 测试日志 `[step → 727] 任务 db-20261026-…-000000 落终态 partial，model_sesno 停在 726（applied=Some(727) file_latest=Some(727)）`；与 lib 侧 `an_abandoned_root_leaves_the_task_partial_the_watermark_unstamped_and_the_marker_finished` 同口径。用例与登记表 H05 标题均已按此改 |
| 3 | 冷重启之后**不能**判 records | `records_present` 读的是进程内投影，冷重启抹掉投影（未触根读成 0 行）；`records_absent` 还连库上 ADR-088 那类 `inst_relate` 老行一起判（实测留着 1 条）。两条都不是本用例要证的，已从注入窗去掉，行的证据挪到 728 |
| 4 | 清钩子后模型侧真的重跑并补上 | restart2 日志 `[增量] 执行完毕 task=… 状态=succeeded 会话区间=727..=727`、`模型生成完成 dbnum=8000 成功=1 失败=0`、`[ams-phase] phase=room_panel_preload dbnum=8000 sesno=728 … records=1`，`model_sesno` 由 726 推到 727 |
| 5 | 恢复窗要先等一轮对账 | `AIOS_WATCH_RECONCILE_SECS` 默认 **300 s**（`batch_worker.rs` 的注释写明「周期对账重扫会装新 epoch，把水位没动的库重新入队」）。默认间隔下 728 的数据批次在 300 s 整点前不入队——那次超时是用例没调间隔，不是产品卡死。已按 `ams8000-equi-data-fail-park` 的做法给本用例加 `service_env`（reconcile 2 / gap 1 / poll 1） |
| 6 | **生成失败不进数据批次连败账**，会被每个对账轮重跑 | 调短间隔后，注入窗「等最新一条任务落终态」永不满足：`/tasks` 里几乎总有一条在跑的重试。`DATA_BATCH_PARK_AFTER = 1` 只管数据批次，模型侧没有停跑机制。harness 因此加了 `latest_terminal_task`（预期失败窗的等待与断言都改看「最近一条已落终态的」）。要不要给生成失败也加 park / 退避，是产品决定，另开票 |
| 7 | 顺带修掉一处编译错 | `22b7a7e0a` 里 H02 的连败账断言 `tests/increment_replay/main.rs:1127` 格式串写了 `{}` 却没给实参，`cargo test --test increment_replay` 直接编不过——那段断言在此之前从未被执行过。补 `case.dbnum` 后 `Finished test profile` |

状态：`cargo check --offline --test increment_replay --features http_api` 绿。我这边四次 live 都没跑成绿（末两次是被同一影子根上另一会话的 L2 顶住，`link.exe` 1104 = 产物占用）；**随后 `8e055d4e3` 用这套改动跑成全绿并把 `INC-F05` / `INC-H02` / `INC-H05` 翻成 `has`**（登记表标题写明「2026-10-26 回放全绿」，`replay:ams8000-equi-fail-inject` 在 tests 里），所以上面第 2 / 3 / 5 条的口径已是仓里现行口径，不再是「待验」。第 6 条已单独开票：**#72**（模型侧生成失败没有停跑 / 退避账，与 `DATA_BATCH_PARK_AFTER` 不对称；复现与三个备选方案在票面）。

---

## 附二：#68 根枚举裁剪的只读复核（2026-10-26 午后）

`c04571d1c` 之后本审计里「#68 零进展」那条已过期：并发会话把票面的路 3 与路 1 都落了，均在 `refactor` 线上——`c6e213736`（`plan=` 拆段计时，量出「两端枚举根占六成、两端建集占两成」）、`b661e1f88`（S 端复用上一窗根集 + T 端只重枚举被触到的顶层子树）。实测连续窗 `roots_S=0ms[cached]`，`roots_T` 扣掉对拍后 1–2 ms（DBMDAT-only）/ ≈78 ms（移一根 EQUI），对照全枚举 120–216 ms。评论：https://github.com/happyrust/gen-model/issues/68#issuecomment-6415852819

- 护栏逐条读过：`incremental_target_roots` 六种形状全 `return Ok(None)` 回全枚举（`full_rebuild`、`unresolved` 非空、WORL 自身在账上、候选顶层对不上 T 端 WORL 表、T 端多出不在账上的顶层成员、S 端有根的顶层子树在 T 端还在却没被遍历到）；改挂场景 S/T 两端顶层都进 `touched`，「从哪来 / 到哪去」两个 SITE 都重枚举。
- **缺口 1：`parity-mismatch` 只出声、不判红。** `git grep parity-mismatch` 只命中 `changelog.md:48-49` 与 `window_root_plan.rs:608/611/1546` 的模式串本身，没有任何测试或门盯它；`root_enumeration_parity_enabled()` = `cfg!(debug_assertions) || AIOS_ROOT_ENUMERATION_PARITY=1` ⇒ **release 默认不对拍**。零成本补法：给 `ams8000-equi-move-chain` / `ams8000-equi-room-cross` / `db8000-two-delete` 加 `log_must_not_contain: ["增量根枚举与全枚举不一致"]`（字段 `tests/increment_replay/main.rs:197`，判在 `:1926`；同机制已用来钉 `[模型目标核对]`）。
- **缺口 2：预算门看不见 mode。** `scripts/increment_budget.py:47` 只解析 `collect=(\d+)ms plan=(\d+)ms`，新增的 `[cached|full]` / `[…parity…]` 后缀一概不读 ⇒ 复用退化成每窗 full（#68 白做）只能靠 `plan` 变慢间接抓，而增量少算根会让 `plan` 更快、更像修好了。建议把 `roots_s_cached/full`、`roots_t_incremental/parity_mismatch` 采成 required 指标——用的正是 `tests/fixtures/increment_budget_baseline.json` 的 `_doc` 自立的规矩（「required 的指标一个样本都没有也 FAIL，日志格式一变门不能静默变绿」）。
- **边角：缓存无失效路径。** `ROOT_SET_CACHE` 里没有 `remove`；`full_rebuild` 窗 T 端回全枚举，但 `roots_s` 仍可能吃到上一世代的 cached 格。「从备份恢复同一 sesno、中间层 TYPE 变了」这种顶层与 `owners.len()` 都不变的改动会让那一格陈旧。修法：reinitialize 入口清该文件的格，或写明 `full_rebuild` 窗 S 端也一律 full。
- **未闭合**：`INC-I02` 仍 `partial`——7997（6772 根、13.8 s）没有回放语料，门只盯得住 8000 的 953 根；`8e055d4e3` 自陈「rooms_changed 与预算门两项尚未重跑」；`scripts/nightly-local.ps1` 仍缺（`NIGHTLY_MISS`）。
- **发布留痕**：`0.1.44` 已由 `83aa3c250 chore(release)` 出厂，但 `git tag --points-at HEAD` 与 `git tag -l "v0.1.4*"` 都空 ⇒ #63 的 tag 缺口照旧，版本号从 0.1.43 变 0.1.44。登记表 55 条 `has` 占比 63.6 %（§9 门 ① 要 ≥ 80 %）。


