# 方案：默认不自动初始化新 dbnum——增量只维护旧库，新库由 plant-ui 手动初始化（开关可回自动）

- 日期：2026-09-09（UTC+8）
- 起因：用户「修改服务端现在的模式：默认不启动初始化——增量更新默认不做新 dbnum 的初始化、只维持旧的 dbnum；
  加一个开关后才自动处理新 dbnum 的初始化。这样默认模式下可以去前端 plant-ui 里手动初始化」。
- **一句话现状**：这件事的服务端主体**已经在工作树里落地（未提交）**——配置键 `auto_initialize_new_dbnums`
  默认 `false`、自动发现单点门、`/dbnums` 逐行 `awaiting_manual_initialization`、手动路径不受门控，都在。
  本方案不是从零设计，是**钉牢语义 → 收口缺口 → 验证 → 提交切分**。
- 关联：refactor 计划 §4 S7 行「门控已落地」（`docs/plans/2026-09-08-external-increment-refactor-plan.md`）；
  plant-ui 侧 d-69（U2 只吃 `/dbnums` 现有字段 + execute `dbnums[]`，已落地）。

## 1. 语义钉牢（默认形态 / 开关形态 / 三个不受门控的例外）

**默认（`auto_initialize_new_dbnums = false`）**：
- 自动发现（启动重扫 / 周期全集轮 / watch 文件事件 / SYS meta 落库后的 scope-refresh 重扫，四条路共用
  `discover_batch`）遇到**从未应用过**（`DbnumState.initialized == false`，即任何来源都解不出水位）的库：
  **不入队**，只记 sweep 日志一行「等待前端手动初始化」；数据面零动作。
- 模型面对这类库本来就一根不排：启动覆盖核对只把它列进 `no_credential_dbnums` 报数
  （ADR-058 D6「等按需 ensure，启动一根不排」，`model_update_pending.rs::dbnums_without_credentials`）。
- `/dbnums` 该行 `awaiting_manual_initialization = true`（口径：开关关 ∧ `initialized=false` ∧ 非
  excluded ∧ 非 not_in_project，`handlers.rs:2153`）。

**开关打开**（配置 `true` / 环境变量 `AIOS_AUTO_INITIALIZE_NEW_DBNUMS` / CLI `serve --auto-initialize-new-dbnums`，
env 双向可压、CLI 是 OnceLock 单向开）：恢复 ADR-023 §4 的历史形态——重扫自动入队，worker 的
`needs_initial_load` 把基线接管过去，全程无人工放行。

**不受门控的三类（全是「维持旧库」或「人说了算」，一个都不许被新门挡住）**：
1. **已初始化库的维护**：增量窗口、回退重建（`reinit_batch` 直入队，绕过 `discover_batch`）、
   追平幽灵水位修复（登记行还在 → `initialized=true`）——照旧全自动。
2. **手动路径**：`/update/preview`＋`/update/execute`（`enqueue_manual_update` 不看开关）。plant-ui
   手动更新向导今天就能跑「首次导入」行（`initialization_required` → 段一 init_row → 勾选执行），
   这就是默认模式下新库的初始化入口，**前端零改动即闭环**。
3. **已在队列里的行**：门只拦「新发现」，不清已接受的手动入队（`automatic_discovery_admits` 第三臂
   `already_queued`——手动初始化一旦入队，必须扛得住 Meta→Design 清单刷新，否则刷新一轮就把人点的活弄丢）。

**正交关系**：`startup_autorun` 管「启动要不要消费队列」（默认 true，继续维护旧库）；`watch_dbnums`
管「哪些库在监听域内」；本开关只管「从未应用过的库要不要自动入队」。三者叠加，互不替代。

## 2. 已落地映射（工作树未提交，核对过的实位）

| 部件 | 位置 | 状态 |
|---|---|---|
| 配置键 + env + CLI + 默认 false | `options.rs:73/1047/1051/1092`（`effective_…` 默认 `false`） | ✅ 带单测 `new_dbnum_initialization_requires_explicit_opt_in` |
| `serve --auto-initialize-new-dbnums` | `main.rs:88/130` | ✅ |
| 自动发现单点门 | `increment_manager.rs:51 automatic_discovery_admits` + `discover_batch:4107` | ✅ 真值表单测 + 日志一行 |
| 四条自动发现路全走此门 | init 重扫 / watch 事件共用 `discover_batch`（`:4084` 文档注明）；scope-refresh 复用 sweep | ✅ live 用例点名 |
| 回退重建 / 幽灵水位不受门 | `reinit_batch`（`:3808` 绕过 discover）；幽灵水位行 `initialized=true` | ✅ live 用例分界注释写明 |
| `/dbnums.awaiting_manual_initialization` | `handlers.rs:2153` | ✅ |
| 手动路径不看开关 | `manual_update.rs:4851 enqueue_manual_update`；SYS meta 永远随批 | ✅（原有行为，未改） |
| `DbOption.toml` 键 + 注释 | `:161-164`（`auto_initialize_new_dbnums = false`） | ✅ |
| live 两拍用例 ×3 | `increment_manager.rs` tests：never-parsed 两拍 / new-beside-applied / scope-refresh（均 `#[ignore]` manual live） | ✅ 代码在，未见运行记录 |

## 3. 缺口与要做的事

- **O1 changelog**：`changelog.md` 2026-09-09 段没有本特性条目——补一条（语义、开关三来源、门的位置、
  不受门控的三类、`/dbnums` 新字段）。
- **O2 验证闭环**：全量 `cargo test --lib`（照仓库惯例带 `--skip damaged_…`）+ 定向跑
  `automatic_discovery_…` / options 两条单测；live 三条是 manual live（要 AMS 沙箱 + 真库），列入
  验收清单由用户择机跑，不挡提交。
- **O3 可见性汇总（决策点 D1）**：现在只有逐库 sweep 日志一行 + `/dbnums` 逐行字段。按监听限定域先例
  （启动横幅 / `/health` / 回执各有各的嗓音），建议补两处小改：sweep 收尾汇总一行
  「N 个库等待前端手动初始化：样本…」（对齐 `out_of_scope` / `watch_excluded` 的写法）；`/health`
  数据面加 `awaiting_manual_initialization` 计数一格。**建议做**（改动小、排障省一轮翻日志）。
- **O4 plant-ui 徽标（决策点 D2，另一仓）**：plant-ui 目前零处消费 `awaiting_manual_initialization`。
  最小闭环已通（手动更新向导跑首次导入行），本项只是可见性增强：本期执行范围列表给待初始化行一个
  「待初始化」徽标 / 直达入口。**建议后排**，跟 plant-ui 自己的节奏（d-69 U2 已收，U3–U5 在等 S6–S9）。
- **O5 CONTEXT.md 词条（决策点 D3）**：建议加一小条「初始化准入门 (Initialization Admission Gate)」，
  钉住「管新库自动入队，不管旧库维护、不管手动路径」，避免与 `startup_autorun` / 监听限定域混称。
- **O6 提交切分**：本特性 hunks 与同树在途工作（T4 房间搬家等，约 1.9 万行未提交）交织在同几个文件里
  （尤其 `increment_manager.rs` / `handlers.rs`）。提交时只挑本特性相关 hunks（§2 表列出的位置 +
  changelog + DbOption.toml），`git add -p` 逐块核对；参照 d-73 / T4 协调前提，别把别的会话的在途块
  一起收进来。

## 4. 不做 / 边界

- **全新空部署**：SYS meta（SYST/DICT/COMM/MISC/GLB/GLOB）从未应用过时同样被门拦住——**不开例外**。
  理由：判定单点、语义一条；且手动执行时 SYS meta 永远随批（不是可勾选对象），在 plant-ui 点一次
  全范围执行即完成引导。生产场景本就是「既有部署 + 新增设计库」。（决策点 D4，建议照此）
- **按需 ensure / 懒生成不动**：观看端主动要某根时照旧现场生成（ADR-058 D6），那是「人要了」，不是自动初始化。
- **`manual_db_nums` / `exclude_db_nums` 不动**：它们 2026-08-06 起就不参与增量判定（issue #10）。
- **读透形态（data_face=read-through）不涉**：数据面不摄入、水位恒 0，`/dbnums` 注释已写明
  「`initialization_required` 不表示要导入」；门在入队侧，读透没有入队。

## 5. 验收标准

- **A1 默认拦住**：默认配置起服，监控目录放一个从未应用的库 → sweep 日志出现「等待前端手动初始化」、
  队列无该库行、`/dbnums` 该行 `awaiting_manual_initialization=true`、启动覆盖核对只报 `no_credential`。
- **A2 手动初始化通**：plant-ui 手动更新向导选中该库执行 → 基线完成、水位建立、模型生成登记；
  完成后 `/dbnums` 该行 `initialized=true`、`awaiting_manual_initialization=false`。
- **A3 开关三来源等效**：配置 `true` / env / CLI 任一 → 同一个库重扫自动入队走基线（live 两拍用例第二拍）。
- **A4 旧库维护不受影响**：增量窗口照跑；回退重建、幽灵水位修复照旧自动（live 用例）。
- **A5 测试不降**：全量 `cargo test --lib`（带惯例 skip）通过，含本特性单测。

## 6. 执行步骤

| # | 事 | 大小 |
|---|---|---|
| S1 | changelog 条目（O1） | 小 |
| S2 | D1 采纳则补 sweep 汇总行 + `/health` 计数格（O3），带两条小单测 | 小 |
| S3 | 全量 `cargo test --lib` + 定向单测（O2） | 跑一轮 |
| S4 | 提交切分（O6）：只收本特性 hunks，一笔 commit | 中（要细心） |
| S5 | D3 采纳则 CONTEXT.md 词条（O5），随 S4 同笔 | 小 |
| S6 | （后排、另一仓）plant-ui 徽标（O4 / D2） | 另计 |
| S7 | （用户择机）live 三条 manual 用例 + A1–A4 现场过一遍 | 人工 |

## 7. 风险

- **R1 提交切分误伤**：`increment_manager.rs` 同文件混着别的会话在途改动，挑错 hunk 会把半成品收进历史。
  对策：按 §2 实位逐块挑，提交前 `git diff --cached` 全读一遍。
- **R2 live 用例成本**：三条 manual live 要 AMS 沙箱、会清库重建指定 dbnum——只在隔离项目跑，不碰生产目录。
