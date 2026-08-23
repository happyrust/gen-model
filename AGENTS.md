# AGENTS.md

`aios-database` —— 把 AVEVA E3D / PDMS 的 dabacon 库**增量**解析进 SurrealDB，并据此增量生成 / 更新三维几何的 Rust 服务；可选 axum REST+WebSocket 接口（feature `http_api`）与 PyO3 调试绑定（`python/`）。

Rust edition 2024，**必须用 nightly**（`.cargo/config.toml` 带 `-Z threads=8`，源码用了 `#![feature(...)]`）；主库是 fork 版 SurrealDB 2.1.4；开发机是 Windows / PowerShell。

完整行为准则见 `.specify/memory/constitution.md`（宪法，六条原则）；术语以 `CONTEXT.md` 为准；决策见 `docs/adr/`，规格见 `docs/specs/`，计划见 `docs/plans/`。

## Project map

- `src/` —— 库 `aios_database` + `src/main.rs` + `src/bin/` 下 27 个探针/工具 bin
  - `data_interface/` —— 增量引擎核心：批次队列 / 调度 / worker、`dbnum_state` 水位、`manual_update`、`model_impact` 影响分类、`generation_root` 生成根解析、`cata_closure`、`staging/`（ADR-017 暂存窗口提交）
  - `fast_model/` —— 几何生成：`gen_model` / `cata_model` / `occ_generate` / `manifold_bool` / `pdms_inst` / `aabb_tree`·`spatial_state` 空间索引 / `room_*` 房间归属
  - `web_service/` —— `http_api` 的 axum 面：`mod.rs`(serve)、`handlers.rs`、`ws.rs`、`events.rs`
  - `versioned_db/` —— 版本化落库：`pe.rs`、`database.rs`、`attmap.rs`、`member_prune.rs`
  - `data_to_file/` —— 回写 dabacon 文件格式（`modify/`、`increment/`）
  - `rvm/` + `rvm_baseline/` —— RVM 导出与基准对拍；`pcf/` PCF 导出；`plug_in/` 虚拟孔洞·浸水·穿舱件等领域插件
  - `api/`、`graph_db/`、`cata/`、`mqtt_service/`、`options.rs`、`test/`
- `python/` —— workspace 成员 `aios-py`（PyO3 + maturin，abi3-py310，Python 包名 `aios_db`）；含 `aios_client.py` 零依赖 HTTP/WS 客户端、`pysrc/` 存根、`tests/`、`testbed/`
- `docs/` —— `adr/` 决策、`specs/` 规格、`plans/` 计划、`evidence/` 证据、`diagrams/`，加根目录 `YYYY-MM-DD_*.md` 审计/台账
- `scripts/` —— PowerShell 运维脚本 + `e3d/`（`.mac` 宏与 C# 加载项）、`e2e/`、`live-batches/*.json`
- `tests/` —— 10 个集成测试目标 + `common/` + `fixtures/`（issue-019/020 会话录制归档，带 `SHA256SUMS`）+ `tests/python/`
- `db_options/` —— 面向测试场景的 `DbOption-*.toml`，靠 `DB_OPTION_FILE` 选中
- `specs/` —— spec-kit 特性目录（与 `docs/specs/` 不同）；`issues/` —— 本地 markdown 议题库
- `resource/` —— 运行期资产（`surreal/*.surql` 按相对 CWD 加载、xls 配置表）；`test_data/` —— RVM 对拍基准
- `rs_surreal/` —— 分专业物料清单 `.surql`；`teach/` —— core.dll 逆向学习材料
- `bin/`、`vendor/` —— **未纳入 git**。`bin/surreal.exe` 是那份 fork 的 2.1.4，新克隆的仓库没有

<important if="you need to run commands to build, test, lint, clean, or start the database">

**禁止 `cargo clean`**（宪法「运行环境」条）。`target-dir` 已指到仓库外的 `../target`，编译产物是共享的。

| 场景 | 命令 |
|---|---|
| 装工具链 | `rustup toolchain install nightly-2026-08-02 --profile minimal; rustup default nightly-2026-08-02` |
| Release 构建（CI 口径） | `cargo build --release --locked --bin aios-database --no-default-features --features ws,gen_model,manifold,occ,project_hd,http_api` |
| CentOS 7 交叉编译 | `cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.17` |
| 快速质量门 | `cargo check` |
| 单测（CI 口径，注意**不带 `occ`**） | `cargo test --locked --lib <测试名> --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture` |
| 集成测试（CI 跑这四个） | 同上，把 `--lib <测试名>` 换成 `--test db8000_two_delete_fixture` / `db_session_fixture_selfcheck` / `db8000_session_pairs` / `pdms_record_boundary` |
| 单条 live 用例 | `$env:DB_OPTION_FILE = 'python/testbed/DbOption-pytest'` 然后 `cargo test --lib --features http_api <测试名> -- --ignored --exact --nocapture` |
| 批量 live | `powershell -File scripts\Run-LiveBatch.ps1 -Manifest scripts\live-batches\<批次>.json` |
| 纯 Python 用例 | `python -m unittest discover -s tests/python -p "test_*.py" -v` |
| 装绑定（日常调试） | `cd python; uv venv .venv; uv pip install maturin --python .venv; $env:VIRTUAL_ENV = (Resolve-Path .venv).Path; .venv\Scripts\maturin.exe develop` |
| 装绑定（跑生成类操作前） | `.venv\Scripts\maturin.exe develop --release`（OCC 布尔运算 debug 太慢） |
| 绑定离线档（CI 只跑这档，秒级） | `cd python; .venv\Scripts\python.exe -m pytest -m offline -q` |
| 绑定全档（需 `bin/surreal.exe`） | `cd python; .venv\Scripts\python.exe -m pytest -q` |
| 构建 wheel | `cd python; maturin build --locked --out <目录>` |
| 起 SurrealDB | `.\scripts\Start-Surreal8009.ps1`（加 `-Memory` 起一次性空库） |
| 查 SurrealDB | `.\scripts\Invoke-Surreal8009.ps1 -Sql "<语句>"` |
| 起 pytest 沙箱库(8019) | `.\python\testbed\Start-TestSurreal.ps1` |
| 本地依赖重定向 | `.\scripts\Toggle-LocalDeps.ps1 -On\|-Off\|-Status` |
| 查配置漂移 | `powershell -File scripts\Test-DbOptionDrift.ps1 -Mode Staged` |
| 装 git 钩子 | `git config core.hooksPath .githooks` |

CI 不跑 `cargo clippy` / `cargo fmt`，但改过 Rust 文件后按计划文档惯例应手动跑 `cargo fmt` + `cargo check`。
</important>

<important if="you are about to start SurrealDB, or a test you are running needs a live database">

不要用 `PATH` 上的 `surreal`。`PATH` 上通常是 `cargo install` 的 3.x，会用「存储版本 2 vs 3」的升级提示诱导你做不可逆迁移，换二进制才是解。

- 一律走 `scripts\Start-Surreal8009.ps1`。它刻意不回退到 PATH，查找顺序是 `-SurrealExe` → `$env:AIOS_SURREAL_EXE` → `bin/surreal.exe`，并硬拒非 2.x、在 rev 与 `Cargo.lock` 不符时告警。
- `.surreal/ams-8009`（正式库）**已被 3.x 写坏且决定不修**，测试一律新建独立数据目录。
- 改端口要同时改 `DbOption.toml`（连接参数按 `v_ip` / `v_port` / `surreal_ns` / `project_name` 对齐）。
</important>

<important if="you are choosing which config the run should use, or adding a required key to DbOption.toml">

`DB_OPTION_FILE` 选配置文件，**不带 `.toml` 后缀**，默认 `DbOption`，例如 `$env:DB_OPTION_FILE = "db_options/DbOption-l3-suite"`。

- `src/options.rs` 会把同一个文件**再读一次**取扩展字段（`aios_core::get_db_option()` 只反序列化基础 `DbOption`，扩展字段会被丢弃）。加扩展字段要动 `DbOptionExtFields`。
- 环境变量覆盖：`AIOS_STARTUP_AUTORUN`、`AIOS_ROOM_INCREMENTAL`、`PLANT_ASSET_ROOT`、`AIOS_SKIP_STARTUP_ROOM_BUILD`。取值不认识时回落到配置值，不猜。（`AIOS_WATERMARK_REALIGN` 已随 ADR-021 退役：回退默认整库重建，不再有档位。）
- 根配置增删**必填**键时，`python/testbed/DbOption-pytest.toml` 要跟着改，否则 config 反序列化报 missing field。
</important>

<important if="you are advancing applied_sesno, or writing a side effect that must land together with a watermark">

水位是承诺不是进度：`applied_sesno` 表示数据确实落库了，不表示尝试过。

- 写失败不得推进水位；同一窗口必须能幂等重放（ADR-001）。应用水位在增量路径内单调不降；文件回退默认整库重建（ADR-021）——扫描只分类入队，worker 冻结点复核后 `wipe_dbnum_for_reinit` 清库并按首次导入重解析，水位随重建归零再建立；仅 `TypeChanged` / `Duplicate` / `Missing` / `ForeignProject` 身份歧义照旧阻断。
- `applied_sesno > 0` 必须有数据支撑：该 dbnum 在 `pe` 里零行（幽灵水位）按首次导入重建，不得从水位往后接增量；存在性查询失败上浮为批次失败，不得吞成任一默认值。
- 与水位共命运的副作用（交付状态、模型工作单）必须与水位推进在**同一个事务**；只能单独提交的，落进持久补偿队列——不允许只留一句 warning。
- 扫描观察值 `file_latest_sesno` 与权威值 `applied_sesno` 严格区分，互不替代。
</important>

<important if="you are adding a gate, filter, or predicate that the manual path and the watcher path could both hit">

手动触发与自动 watcher 共用同一个队列、同一个派发器（ADR-011；默认单在飞批次，`data_batch_workers` 可配至 8）。「哪些文件算库文件」「哪些库在本期范围内」「什么异常要阻断」这类判定只能有一处权威实现。

- 新增门控必须同时接进两条路径，或抽成共享谓词。
- 引入第二条数据批次消费路径的改动，要先改 ADR-011。
- 纯函数钉不住的时序约束（如「重复 dbnum 阻断必须先于 `record_scan`」），用源码顺序断言测试钉住，仓内已有先例。
</important>

<important if="you are writing a match arm, an early continue, or unwrap_or_default on a path that decides something">

静默失效是最高级别缺陷：宁可报错阻断，不可无声跳过。判定为「异常」的分支不允许落进 `_ => 放行`。

- 任何落在判定路径上的 `continue` / `_ =>` / `unwrap_or_default()`，都要能回答「它跳过的东西，谁会发现」。答不上来就是缺陷。
- 只有 `println!` 而调用方回执里看不见的失败，视同没有报告。
- 判定与它依赖的基准数据不得互相覆盖：先裁决，后落库。
</important>

<important if="you are enqueueing to or draining a persistent queue">

队列里每一行都要有三条明确出路：

- **可消费**：每种 action 正好被某一个 drain 阶段的过滤器覆盖。
- **可收口**：成功即删除、失败即计数；寻址按行内实际字段，不要重算 id。
- **可复活**：到重试上限的死信必须有「新触发到来时清零重试」的路径。不认领会话号的任务（跨库派生、房间重算）无条件复活。
</important>

<important if="you need to resolve a Ref0 to a dbnum, or are about to fill in an identifier you could not look up">

`Ref0` 不是 `dbnum`，`file_stem` 不是 `file_name`，观察值不是权威值。

- `ref0 → dbnum` 走 `cata_closure` 反查，**不得**用 `RefU64::get_0()` 顶替。
- 拿不到真值就留空（`0` / `None`）并说明「未解析」，绝不填一个看着像真的近似值。
</important>

<important if="you are interpolating an external string into a SurrealQL statement">

所有进入语句字面量的外部字符串必须过 `dbnum_state::escape_surql_str` —— Windows 路径里的反斜杠会破坏字面量。
</important>

<important if="you are classifying an attribute change into a model impact">

模型影响三态 `Regen` / `TransformOnly` / `Skip` 由单一权威 `classify_operation_impact` 判定。

- 原则是**宁多勿漏**：未知属性一律保守触发。
- 外部权威是 core.dll / Core3D 逆向结论 + E3D 字典 DCHC 快照（ADR-002 / ADR-004），不要凭直觉改分类。
</important>

<important if="you are writing a test, fixing a bug, or documenting an invariant in a comment">

每条写进注释的不变量都要有对应测试。

- 优先纯函数单测（不连库、进得了 CI）。依赖实库的行为用 `#[ignore]` live 测试补，前置条件写进测试名。
- 修 bug 必须附一条「若回退到旧写法就会红」的回归测试。
- 涉及水位 / 队列 / 模型生成的改动，另需跑对应 live 测试并在 `docs/evidence/` 留痕。
</important>

<important if="you ran, changed, or added an #[ignore] live test">

`docs/2026-08-12_live-test-ledger.md` 是 live 用例的**唯一事实来源**：没有「最近通过」记录的用例视同未验资产。动过 live 用例或点亮新批次必须同步更新台账。
</important>

<important if="you are naming a domain concept in code, comments, or docs">

`CONTEXT.md` 是术语表，每条术语带 `_Avoid_:` 禁用同义词清单（如「生成根」不要写成 regen root / 目标根，「Ref0 库归属」不要写成 Ref0 数据库号）。命名先查它。它只管词汇，不含决策与流程。
</important>

<important if="you are starting work that changes architecture, or writing a plan or spec">

流程是 ADR → spec → plan → tasks：

1. 有架构决策就写 `docs/adr/ADR-NNN-*.md`，并列出本次引用到的既有决策。注意现存 ADR 编号有重复（004/006/008 各两份），编号不是唯一键。
2. 改动前先有 `specs/NNN-*/spec.md`，只写「要什么、怎样算成功」，不写实现。
3. plan 要过 Constitution Check；违反宪法原则的设计要么改，要么在 Complexity Tracking 里写明为什么无法避免。
4. tasks 每条带具体文件路径，并标出能否并行。
</important>

<important if="you are writing a commit message, creating a branch, or recording a change">

- 提交信息用 Conventional Commits，**subject 用英文小写**：`feat(spatial): close the tree-consistency loop with a V2 snapshot and a state machine`。在用的 type 有 `feat` / `fix` / `test` / `docs` / `ci`。
- 分支：`codex/<topic>`、`codex/issue-N-<topic>`、`fix/issue-N-<topic>`、`feat/<topic>`。
- 变更记入 `changelog.md`（中文，`## YYYY-MM-DD` 倒序，分 `### 新增` / `### 修复`）。`COMMIT_LOG.md` 是 2025 年的死文件，不要往里写。
- 仓库同时挂着十几个 worktree（部分在 `.scratch/` 下），改文件前先确认自己在哪个 worktree。推 `main` 会被 `.githooks/pre-push` 拦下带本地依赖重定向的提交。
</important>

<!-- sigmap-creation-workflow:start -->
## Creation workflow (SigMap)

When creating or changing code, run the grounded-creation pipeline so each step is verified against the live index:

1. **`sigmap scaffold "<name>"`** — propose a convention-matched file/structure (refuses if conventions are inconsistent).
2. **`sigmap verify-plan <plan.md>`** — check the plan against the live index (files/symbols exist, blast radius, scope).
3. **`sigmap verify-ai-output <answer.md>`** — flag fake files/symbols/imports in the generated output (offline).
4. **`sigmap review-pr`** — audit the diff for scope drift, god-node edits, missing tests, and security files.

Or run all four in one pass with **`sigmap create "<task>"`** (`1/4`…`4/4` numbering, single pass/fail).

<sub>Generated by `sigmap --init` · refresh by re-running it.</sub>
<!-- sigmap-creation-workflow:end -->
