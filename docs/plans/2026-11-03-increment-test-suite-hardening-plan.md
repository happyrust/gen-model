# 增量更新自动化测试加固计划（2026-11-03）：把审核出的十条问题排成可独立验收的工作包

> 来源：用户 2026-11-03「审核 gen-model 现在实现的模型增量更新的自动化测试」→ 本会话审核结论（十条问题 + 一组正面）→「使用 plannatator 制定开发计划」。
> 状态：**已批准**（2026-11-03 09:34：plannotator `--gate` 回 `{"decision":"approved"}`、无批注，`.scratch/pencil/increment-test-hardening-gate-r1.json`）。§4 六条决策点全按推荐项定：D1 护栏三处都做、D2 宏静态列表、D3 时延只记不判、D4 仓根 `DbOption.toml` 为底 + `toml` 结构化改键 + 记指纹、D5 CI 跑 `--test increment_replay`（skip + 自测）、D6 WP-A ⑫ 单独先提（仍等用户口令）。
> 口径：本机 `refactor` 工作树（HEAD 之后 139 个脏文件；`tests/increment_replay/{harness,case,assertions}.rs` 与两条 catalogue 用例未跟踪），行号按 2026-11-03 工作树。事实来源：审核时的 `rg` / 读源、`cargo check --locked --test increment_replay --test increment_cases_registry`（绿，1 warning）、`output/increment-replay/*-20261102-*`（8 条链全绿）、`scripts/increment_budget.py check`（ok 11 / warn 0 / fail 0）、`tests/python/test_increment_budget.py`（13 过）。
> 上游：`docs/plans/2026-10-25-incremental-update-automated-test-suite-plan.md`（案例集合、L0–L4 分层、D1–D8），`docs/plans/2026-10-31-incremental-update-closure-plan.md`（WP-A ⑫ 提交 harness，WP-F F-1～F-6 测试门）。本文不重写它们，只接「审核发现、它们没接住的那一截」：安全护栏、门本身的结构缺陷、可复现性。覆盖缺口（C08 / F03 / G03 / H03 / I 组）仍归 WP-F，本文只写依赖。
> 编号：工作包 **TH-1～TH-9**（test hardening）；每包可独立提交、独立验收；决策点 D1–D6（§4）。

## 0. 一句话

L2 回放这一层已经能跑、能红、能判（8 条链 11-02 全绿，预算门 11 项 OK），但它站在三条不牢的腿上：**① 主库 `replay-init` 会穿硬链接写真工程**（只差一条用例配置写错）、**② 整套 harness 还在工作树里没提交**、**③ 门的结构让一条红遮住其余七条、CI 连编译都不碰它**。本计划先钉护栏（TH-1），再把资产提交进仓（TH-2 = WP-A ⑫），然后把门做成「每条独立、双向钉死、CI 保编译」（TH-3～TH-5），最后清掉四处会让门变成噪声的地方（TH-6～TH-9）。覆盖缺口按 WP-F 走，不在本文重复。

## 1. 审核结论回放（证据）

| # | 问题 | 证据（2026-11-03 工作树） | 严重度 | 归 |
|---|---|---|---|---|
| 1 | 主库可能穿硬链接写坏真工程 | `scripts/replay/New-ShadowProjectRoot.ps1`：`-Replayed` 缺省只有 `AvevaMarineSample\ams000\ams8000_0001`，其余 ams000 文件 `New-Item -ItemType HardLink` 到 `D:\AVEVA\Projects\E3D3.1`；`src/bin/db_session_fixture/replay.rs::replay_init`：`out.exists()` → `overwrite_in_place`；`tests/increment_replay/main.rs:55-67` 主库直接 init，`:68-85` 只有伙伴库先 `remove_file` | 高·安全 | TH-1 |
| 2 | harness 未提交 | `git ls-files`：只有 `main.rs` 与 6 条用例 json 在库里；`harness.rs`（946 行）/ `case.rs`（383）/ `assertions.rs`（1138）/ `cases/ams8000-catalogue-*.json` 未跟踪；HEAD 的 `main.rs` 仍是 1883 行单文件 | 高·工程 | TH-2（WP-A ⑫） |
| 3 | 单 `#[test]` 跑全部用例 | `main.rs:346` 一个 test 顺序 `run_case`，首个 panic 终止其余（`:167-170` 注释自认） | 中 | TH-3 |
| 4 | 登记表钉弱、双向不一致 | `tests/increment_cases_registry.rs:79-124`：`lib:` 只查 `fn 名(`、`test:` 只查文件在、`replay:` 只查 json 在；`increment_cases.json` INC-A06 引 `replay:ams8000-equi-move-chain` 而用例 `registry` 无 A06；rotate-chain 自报 A06 / D01 / G01，JSON 只在 C08 引 | 中 | TH-4 |
| 5 | CI 不编译 `increment_replay` | `.github/workflows/windows-tests.yml` `offline-increment-regression` 列的 `--test` 里没有它（`:268-363`） | 中 | TH-5 |
| 6 | 覆盖缺口 | 登记表 todo B07 / C05 / C06 / G03 / H03 / I03 / I06，corpus C07 / E04，partial C08 / F03 / I01 / I02 / I04 / I05；harness 的伙伴库 / `companion_to` / `expect_in_sync` / `hold_primary` / `no_wait` / 独立 `half` 没有用例消费；L2 只有 rotate-chain、room-cross 做 rebuild parity，move-chain 没有 | 中 | WP-F（§3 末「覆盖缺口」） |
| 7 | 功能用例里硬时延断言 | `assertions.rs:1102-1109` `generate_budget_ms` 超即 problem；move-chain 5 处、room-cross 2 处、rotate-chain 1 处都是 250 ms，debug build，同机还跑着常驻服务 | 低 | TH-6 |
| 8 | `derive_config` 逐行改键 | `harness.rs:96-150`：以仓根 `DbOption.toml`（已跟踪，但读的是工作树含本地改动）为底、`line.split('=')` 匹配 6 个键、只认单行；`dbnum` 参数未用（唯一 warning） | 低 | TH-7 |
| 9 | `record_identity` 不唯一 | `assertions.rs:24` 取 `refno|tube`；`records_differ`（`:104`）排序后逐位比，同 refno 多行会错位 | 低 | TH-8 |
| 10 | 入口脚本原样跑不起来 | `scripts/replay/Run-L2Replay.ps1:52` 缺省 `D:\Rust\target-l2-m1cwd` 本机不存在，`:113` 直接拒绝；共享 `D:\Rust\target` 被常驻 `aios-database.exe` 占着 | 低 | TH-9 |

做得好的（不动）：SHA256 字节级会话夹具；H01 竞态靠 `AIOS_TEST_DELAY_BEFORE_FROZEN_VERIFY` 确定性复现；rollback / restart / rebuild 三类对拍；测试钩 release 下必须 `AIOS_ALLOW_TEST_HOOKS=1`（`src/data_interface/model_refresh.rs::test_hook_var`）；预算门对正则漂移的保护（必需指标无样本即 FAIL）；`docs/specs/incr-gen-audit-aux/` 的自审。

## 2. 原则

- 不碰产品运行路径（`src/data_interface/**`、`batch_worker` 等）。TH-1 的 CLI 改动在 `src/bin/db_session_fixture/`（工具二进制），不是服务。
- 每包 DoD：对应测试绿（列在各包「验收」）；`cargo test --locked --test increment_replay --test increment_cases_registry --no-run` 零 warning；用例 json 若改，`increment_replay` 的自测绿；`changelog.md` 一条；不新增 crate 依赖（D2 ② 若被选中除外）。
- L2 实跑统一走 `scripts/replay/Run-L2Replay.ps1`（TH-9 修好后），`-TargetDir` 指独立 target；不在共享 `D:\Rust\target` 上跑 L2。
- 共享文件约束沿用 10-31 计划 §3：`manual_update.rs` / `increment_manager.rs` / `increment_pipeline.rs` / `helper.rs` 本计划不碰。

## 3. 工作包

### TH-1 主库硬链接护栏（高·安全；独立于 WP-A，先做）

- 为什么：§1 #1。今天没出事只因 8 条用例的 `db_file` 都是 `ams8000_0001`，恰好是影子根缺省 `-Replayed` 的那一个；WP-F F-2（H03 第二库）/ F-3（G03 在 7997）一来就不是了。
- 改什么（按 D1）：
  1. `tests/increment_replay/main.rs::run_case`：主库 init 前与伙伴库同款——文件存在就 `fs::remove_file` 再 `replay-init`（此时服务未起、surreal 每用例新建，不存在要保的 `StableFileId`；`:342` 注释「下一条用例的 replay-init 会就地重置」改为「删后重写」）。
  2. `scripts/replay/New-ShadowProjectRoot.ps1`：`-Replayed` 缺省改为从 `tests/increment_replay/cases/*.json` 的 `db_file` + `companions[].db_file` 推导（并集），显式传参仍优先；造完后对每个被回放路径断言「不存在」，对其余路径断言是硬链接。
  3. `src/bin/db_session_fixture/replay.rs::replay_init`：覆写前用 `GetFileInformationByHandle` 取 `nNumberOfLinks`，> 1 即拒绝并打印「这是硬链接，会写穿到 N 个路径」，`--allow-hardlink` 才放行。`windows-sys` 已是 `cfg(windows)` 依赖，加 feature `Win32_Storage_FileSystem`（不动 `Cargo.lock` 版本）；非 Windows 用 `std::os::unix::fs::MetadataExt::nlink`。
- 验收：① 新自测 `replay_init_refuses_hardlink`（临时目录 `std::fs::hard_link` 两路，`replay-init` 退出非 0、原件字节不变）；② 手工一次：把一条用例的 `db_file` 临时改成未列入 `-Replayed` 的伙伴文件，只跑 init 段，真工程文件 sha256 前后一致，记进 changelog；③ 8 条链 `Run-L2Replay` 全绿不退化。
- 风险：`fsutil hardlink` 非管理员下可能不可用——自测只用 `std::fs::hard_link`。
- 实现备注（2026-11-03 落地）：第 3 条没加 `windows-sys` feature，直接用 lib 已开的 nightly `windows_by_handle`（`std::os::windows::fs::MetadataExt::number_of_links`；bin 的 `cfg_attr` 从 `all(test, windows)` 放宽到 `windows`），`replay_step` 的三处就地覆写同受护栏；第 1 条抽成 `clear_for_replay_init`，顺带拒绝相对路径上任何一级是 junction / 符号链接（往 junction 里 replay-init 等于写真工程）；第 2 条的脚本还会把上次用别的 `-Replayed` 留下的独立文件删掉重链，并把不在 `<MainProject>\<DbDir>` 下的回放路径直接拒绝。验收 ① 的自测实名：`replay::tests::overwriting_a_hard_linked_file_is_refused_unless_allowed`（CLI）+ `clearing_for_replay_init_detaches_the_link_without_touching_the_original`（harness）。验收 ①②③ 2026-11-03 全过（② 改在隔离副本上做，真工程 sha256 不变；③ 停掉常驻服务后在共享 target 跑整档，5 passed / 1662 s / 无 skip，按 `Run-L2Replay.ps1` 三条判据真绿），详见 changelog 2026-11-03。

### TH-2 把 harness 提交进仓库（= 10-31 计划 WP-A ⑫；本文只定门与顺序，见 D6）

- 为什么：§1 #2。11-02 全绿是工作树的绿；另一会话在同一棵树上边改边提（10-31 计划 §7），未跟踪文件最容易丢。
- 改什么：按 WP-A ⑫ 清单提交 `tests/increment_replay/*`（含未跟踪三件）、`cases/*.json`（catalogue 两条在 WP-A 归 ⑦，若 ⑦ 不先走就随 ⑫ 一起）、`increment_cases.json` 对应行、`scripts/replay/*`、`scripts/e3d/**`、`src/bin/l3_suite.rs`、`tests/fixtures/**/rvm-truth/`。提交前把 `harness.rs:103` 的 `dbnum` 要么用上（写进派生配置的注释头）要么删掉，让 `cargo check` 零 warning。
- 验收（WP-A ⑫ 原门 + 本文追加）：`cargo test --lib` 全绿；`cargo test --locked --test increment_cases_registry` 绿；`cargo test --locked --test increment_replay --no-run` 零 warning；`AIOS_REPLAY_CASE` 逐条跑 move-chain / room-cross / two-delete 退出 0（11-02 已有全档绿，提交前只跑这三条）。
- 顺序：TH-1 之后、其余 TH 之前——后面每包都在已提交的 harness 上改，否则 diff 没法审。

### TH-3 每条回放用例一个 `#[test]`（D2）

- 为什么：§1 #3。全档 1100–1500 s，一条红就丢掉后面几条的信息；`Run-L2Replay` 的判定也只能按整档。
- 改什么（D2 ①）：`main.rs` 里 `replay_case!("ams8000-equi-move-chain")` 式宏逐条展开成 `#[test] fn replay_ams8000_equi_move_chain()`；每个 test 自己调 `environment()`，缺则 skip；`known_red` 仍按 `AIOS_REPLAY_INCLUDE_KNOWN_RED` 决定 skip。新自测 `every_case_json_has_a_test`：遍历 `cases/*.json`，名字都在宏列表里（漏列即红）。`AIOS_REPLAY_CASE` 保留为兼容（等价于 cargo 名字过滤）；`--test-threads=1` 仍必须（端口 / 影子根共享），写进 `Run-L2Replay.ps1`。
- 验收：`cargo test --test increment_replay -- --list` 列出 8 条链 + 自测；故意让一条用例红（改 `expect` 值），其余 7 条照跑并各自报结果；`Run-L2Replay.ps1` 摘要按 test 名逐条给 ok / FAILED。
- 风险：每条 test 都重新起 surreal / 服务——现在 `run_case` 内部就是这样，无新增开销。

### TH-4 登记表双向钉 + `lib:` 钉到 `#[test]`（独立）

- 为什么：§1 #4。登记表是案例集合的「单一事实」，但现在允许 JSON 指向一个不是测试的函数（只要有同名 `fn`）、允许用例与登记表各说各话。
- 改什么：`tests/increment_cases_registry.rs`：
  1. `lib:` → 要求 `src/**` 里该 `fn` 前最近的属性块含 `#[test]`（或 `#[tokio::test]`）且不含 `#[ignore]`；live 用例在登记表条目加 `live: true`，允许 `#[ignore]`。
  2. `test:` → 文件存在且含至少一个 `#[test]`；允许 `test:<file>::<fn>` 精确到函数。
  3. `replay:` → 读用例 json 的 `registry`，必须含该 INC 编号；反向：遍历 `cases/*.json` 的 `registry`，每个编号在 `increment_cases.json` 对应条目的 refs 里必须有 `replay:<case>`。
  4. 顺手修两处已知不一致：A06 加进 move-chain 用例的 `registry`（A06 的 L2 半边确在 move-chain 的登录 / 尾窗 `unit_count == 0`，10-25 计划「P1 收完」段记过）；rotate-chain 的 A06 / D01 / G01 写进 JSON 对应条目（status 仍随 C08 一起翻）。
  5. `matrix:` 前缀（`docs/specs/incr-gen-audit-aux/t-i-3-missing-l2-chains.md` 建议）：指向 `2026-09-05-pipe-hvac-ctray-equi-increment-test-matrix.md` 的 C-编号，只查文档里有该编号。
- 验收：`cargo test --locked --test increment_cases_registry` 绿；负向：把一条 `lib:` 改成非 test 的 fn 名 → 红；把一条用例 `registry` 删一项 → 红。CI 已有 `Pin the increment test-case registry` 步，无需新步。

### TH-5 CI 编译 `increment_replay`（D5）

- 为什么：§1 #5。harness 一改坏，只有本机跑 L2 的人知道；三条自测（用例 json 解析、`records_differ` 等）也没人在 CI 跑。
- 改什么（D5 ①）：`.github/workflows/windows-tests.yml` `offline-increment-regression` 在 `Pin the increment test-case registry` 之后加一步 `cargo test --locked --features <同 registry 步> --test increment_replay -- --test-threads=1`：没有 `AIOS_REPLAY_PROJECT_ROOT` 时每条链 skip（打印原因），自测照跑。TH-3 落地后 skip 条数 = 用例数，CI 日志里能数。
- 验收：PR 上该步绿；本地模拟 `Remove-Item Env:AIOS_REPLAY_PROJECT_ROOT; cargo test --test increment_replay` 退出 0 且日志有 N 条 skip。
- 风险：编译时间 +1～2 min（与 registry 步共享 target 缓存）。

### TH-6 功能用例里的时延断言降为记录（D3）

- 为什么：§1 #7。O5 的裁判是预算门（基线 + 1.5× 中位数 / 上限），功能用例里再放一个 250 ms 绝对值是第二个裁判，而且是 debug build、同机还跑着常驻服务——它红的时候不说明增量错了。
- 改什么（D3 ①）：`assertions.rs:1102-1109` 改为把 `elapsed` 写进运行目录 `budget-samples.jsonl`（`metric=generate_cached_ms`），不进 `problems`；`scripts/increment_budget.py collect` 已采日志里的 `generate_cached_ms`，这里只是多一路样本。用例 json 的 `generate_budget_ms` 字段保留、语义改为「只记」，`case.rs:243` 注释同步；`budget_gate: true` 的三条链不变。
- 验收：三条带字段的用例跑绿；把一条的 `generate_budget_ms` 改成 1 不再红；预算门 `check` 仍能对 `generate_cached_ms` 判 FAIL（负向对照沿用 `.scratch/pencil/budget-baseline-bad.json` 那套）。

### TH-7 `derive_config` 结构化 + 记底本指纹（D4）

- 为什么：§1 #8。逐行 `split('=')` 改键：多行数组、带注释的行、同名子表键都会漏或误改；底本读的是工作树的 `DbOption.toml`，本地未提交改动会悄悄进入回放配置，日后对不上「那次为什么红」。
- 改什么（D4 ②）：用已有的 `toml` crate（`Cargo.toml:246`，非可选）解析为 `toml::Table`，按路径设 6 个键，序列化到运行目录（派生文件是一次性产物，丢注释无妨）；派生文件头部写 `# base = DbOption.toml sha256:<…> git:<clean|dirty>`；删掉未用的 `dbnum` 参数。不把 `toml_edit` 引进 dev-deps。
- 验收：新自测 `derive_config_rewrites_only_six_keys`（固定输入 → 固定输出）；三链回放绿；派生文件头有指纹。

### TH-8 `record_identity` 唯一化

- 为什么：§1 #9。`refno|tube` 不唯一时，`records_differ` 的稳定排序把 A 的第 2 行和 B 的第 3 行配对，差异会被错报或抵消。
- 改什么：先量——在 11-02 的运行目录里对 `/model/records` 输出统计同 identity 多行的比例；若为 0，本包只加「identity 重复即 panic」的护栏；若 > 0，identity 加入可判别字段（几何 kind + 序号，或 `inst_relate` 的子件 refno），`records_differ` 改为按 identity 分组、组内多重集合比较。
- 验收：新自测 `records_differ_handles_duplicate_identity`；三链 `restart_parity` / `rebuild_records_equal` 结果不变。

### TH-9 `Run-L2Replay.ps1` 入口可用性

- 为什么：§1 #10。「唯一跑法入口」在本机原样拒绝执行（缺省目录不存在），每个人都得先知道一个没写进文档的目录名。
- 改什么：`-TargetDir` 缺省改为 `<仓根>\..\target-l2`（或 `$env:AIOS_L2_TARGET_DIR`），不存在就创建并提示「首次从零编译」（`-AllowNewTargetDir` 从「拒绝」改为「确认提示」）；保留「拒绝共享 target」；启动前探 `aios-database.exe` 是否锁着目标 exe 仍只提醒；把 TH-3 的逐 test 摘要接进四类判定；顶部注释里「0xc0000409 崩溃」的事实保留，改成风险提示而不是拒绝理由。
- 验收：干净机器（无 `D:\Rust\target-l2-m1cwd`）`.\scripts\replay\Run-L2Replay.ps1 -Case db8000-two-delete` 一次跑通退出 0；共享 target 仍被拒。

### 覆盖缺口（归 WP-F，这里只写依赖）

- F-1 C08 rotate-chain：11-02 已跑绿一次，TH-4 落地后同一笔把 `partial` → `has`（需 JSON 补 A06 / D01 / G01 的引用，TH-4 ④）。
- F-2 H03 / F-3 G03：**必须在 TH-1 之后**——两者都会用非 `ams8000_0001` 的 `db_file`，正是 #1 的触发条件。
- F-4 F03、F-5 I 组：不依赖本文；F-5 刷基线建议在 TH-6 之后（样本口径统一）。
- move-chain 没有 rebuild parity：F-5 刷基线那次顺手给 `ams8000-equi-move-chain` 加 `rebuild_records_equal: ["24384/24776", "24384/23257"]`，让 O1 风格的对拍覆盖平移路（成本约 +60 s / 次）。

## 4. 决策点（请在 plannotator 里批注；每条第一项是推荐）

- **D1 硬链接护栏放哪**：① **harness 删后重写 + 影子根脚本从用例推导 `-Replayed` + `replay-init` 拒绝多链接文件（三处都做）**；② 只做 harness + 脚本；③ 只做 `replay-init` 拒绝。推荐 ①：harness 那一处挡本计划的用例，脚本那一处让影子根天然没有可写穿的路径，CLI 那一处挡手工误用（`db_session_fixture` 也被人直接敲）；三处加起来不到 60 行，没有新依赖（windows-sys 加一个 feature）。
- **D2 每条用例一个 test 怎么实现**：① **宏静态列表 + 「每个 json 都有 test」自测**；② `libtest-mimic` 动态生成；③ 保持单 test，`catch_unwind` 逐条收集。推荐 ①：零新依赖，`cargo test <name>` 原生过滤、`--list` 可见、CI 日志逐条；② 要新 dev-dep 且 harness 要换 main；③ 看起来最小但 `Service` 的 Drop 在 unwind 里杀进程的时序不好保证，且 cargo 仍只报一条。
- **D3 `generate_budget_ms` 怎么处理**：① **改为只记样本，预算门唯一裁判**；② 保留、放宽到 1000 ms；③ 删字段。推荐 ①：保留字段让用例作者仍能表达「这步应该是缓存命中」的意图，判定交给有基线的门；② 只是把抖动阈值挪远；③ 丢了意图。
- **D4 `derive_config` 底本**：① 仓内最小模板；② **仍以仓根 `DbOption.toml` 为底，`toml` 结构化改键 + 头部记 sha256 / dirty**；③ 现状。推荐 ②：模板会与真配置漂移（32 KB，含模板 / 排除 / 生成开关），而「记指纹」已经解决「那次红为什么」的追溯；结构化改键解决多行键。
- **D5 CI 接线**：① **`cargo test --locked --test increment_replay`（skip + 自测）**；② 只 `--no-run`；③ 不接。推荐 ①：比 ② 多跑三条自测（用例 json 解析正是最常改坏的）且成本相同。
- **D6 TH-2 的提交节奏**：① **WP-A ⑫ 单独先提（不等其余 14 笔口令）**；② 按 WP-A 整包一起。推荐 ①：⑫ 不碰 10-31 计划点名的四个共享文件，与 ①–⑪ 无 hunk 交叉（附录 A 已逐 hunk 归类），先提就先有审 diff 的基线；仍要用户一句口令。

## 5. 顺序与依赖

```
TH-1 ──► TH-2(WP-A ⑫) ──┬─► TH-3 ──► TH-9（逐 test 摘要）
                         ├─► TH-4 ──► WP-F F-1（C08 翻 has）
                         ├─► TH-5
                         ├─► TH-6 ──► WP-F F-5（刷基线）
                         ├─► TH-7
                         └─► TH-8
TH-1 ──────────────────────► WP-F F-2 / F-3（H03 / G03）
```

TH-1 可以在 TH-2 之前单独提（它改的 `main.rs`、`replay.rs`、脚本都已跟踪）。TH-3～TH-8 互不依赖，可并行。

## 6. 验收总门

① TH-1 的负向自测在 CI 绿；手工写穿试验真工程 sha256 不变。② `git status` 里 `tests/increment_replay/` 无未跟踪文件；HEAD 上 `cargo test --locked --test increment_replay --no-run` 零 warning。③ `cargo test --test increment_replay -- --list` ≥ 8 条链 + 自测；故意弄红一条，其余照跑。④ `increment_cases_registry` 负向两例能红；JSON 与用例 `registry` 零不一致（自测即证）。⑤ CI `offline-increment-regression` 多一步且绿。⑥ 八条链在 `Run-L2Replay.ps1` 新缺省下全绿一次，预算门 fail 0。

## 7. 不做 / 风险

- 不录新语料、不碰 E3D（归 WP-F / WP-H）；不动 L3 `l3_suite`、L1′ vendor 门。
- 不把 L2 搬进 GitHub runner（D5 ① 只是编译 + skip）。
- 不改测试钩语义、不加新钩。
- 风险：TH-2 与另一会话共用工作树——提交前按 10-31 附录 A 重核 hunk；TH-3 改 test 结构后 `Run-L2Replay.ps1` 的日志解析要同步（归 TH-9）；TH-7 若 `DbOption.toml` 有 `toml` 0.8 不接受的语法（如重复键）会在第一次解析时暴露——那本身是要修的。
