# 实施方案：rvm-check 按 ZONE 生成即比对（tty 逐 ZONE 导出基准，不写 generated.rvm）

- 日期：2026-11-10
- 状态：2026-11-10 用户确认方案与第 8 节缺省。阶段 1（任务 1–3）代码完成，E3D 当场导出的对账在 ams1112、ams7999、ams7352、ams7333（全部 1062 个 ZONE）上通过，检查点 1 只差和用户过一遍。应用户要求把任务 16（结果入库与查询）提前做完，ams7333 全量已入库并核对过；又加了构件级结果入库（新表 `rvm_check_element`，一个带几何的构件一行，记录 id 由 refno 得出），ams7333 全量 159,876 行对过账。批量「比对失败」的原因种类（gen-model#123）已修。父进程等子进程退出的 0.5 秒轮询与导出秒数的合计（gen-model#124）已修。阶段 2–4 未开工。见第 11 节。
- 代码基线：gen-model `0890e488d`；`../vendor/e3d-model` `db65e1c`。两边相关源码与 HEAD 一致。
- 依据：2026-11-10 会话的五路只读审核（融合比对、ZONE 单元、tty 导出、`rvm_baseline`、内存预算）。
- 任务清单写在本文第 6 节，不另建 `tasks/`。

## 1. 目标

用户原话：「我现在希望的是在内存里进行比对，然后我们的程序不用生成 rvm 文件，按 ZONE 和 tty 导出的 RVM 进行比对。」

| # | 要求 | 本方案的落点 |
|---|---|---|
| ① | 比对在内存里完成 | 生成成员流直接进 `StreamingCompare`；判定不经文件回读；比对预算超了判超预算，不溢写 |
| ② | 不生成 RVM 文件 | 融合下缺省不写 `generated.rvm`，也不写 `elements.json` / `report.json` |
| ③ | 按 ZONE 比对 | 比对单元从 SITE / APPLDW 下沉到 ZONE（`--unit zone`） |
| ④ | 基准来自 tty 导出 | tty 只读宏对每个 ZONE 执行 `export =<refno>`，一个 ZONE 一份基准 |

已拍板（2026-11-10）：

- 基准由 tty **逐个 ZONE** 导出，没选「SITE 导一份、程序在内存里按 ZONE 切片」。
- 先出方案，确认后再改代码。

「我们的程序」指 rvm-check 现在用的 vendor `export_rvm` 生成流，不是服务落库后在页面显示的那份模型。

## 2. 现状

### 已经有的

- `--fused-compare --emit-rvm false`：vendor 不建 RVM writer，成员流直接喂 `StreamingCompare`（vendor `rvm_export.rs` 2107–2123 装载基准）。
- `src/rvm_check/macro.rs` 的 `export_batch_body`（36–69）只校验 refno 格式，对 ZONE 同样适用；窄口径句子含 `representation level 6` / `representation level hvac 6`。
- SITE 级基准加 `--scope subtree:<ZONE>` 比单个 ZONE 已真机通过（vendor `docs/plans/recon-task10-scope-verify.md`，2026-11-09，debug 构建）：

  | 库 | ZONE | 配对 / exact | 生成峰值 |
  |---|---|---|---|
  | ams1112 | `/1RS-WF05` | 1801 / 1800 | 210 MB（加融合 243 MB） |
  | ams7333 | `/MDS/FH01`、`/MDS/FH06` | 84 / 84、105 / 105 | 118 MB |
  | ams7999 | 中文名 ZONE | 447 / 447 | 124 MB |

  整根对照：ams1112 `=9304/2` 444 MB；ams7333 `=23717/2` 9088 MB（`output/rvm-check-batch/20261107-full/summary.txt`）。
- 生成子进程的整段峰值已按 `PeakPagefileUsage` 读（`src/rvm_check/batch.rs` 约 1814 行），可直接用于验收。

### 缺的

| 缺口 | 位置 |
|---|---|
| 根只取 WORLD 直接成员里的 SITE / APPLDW；`--only` 传 ZONE 被忽略，全部被忽略时报错 | `batch.rs` `enumerate_roots` 473–515；`run` 686–709 |
| 融合必须给 `--baseline-dir`，且只允许 `--stage full` | `batch.rs` `run` 641–650 |
| 先生成、后等基准 | `batch.rs` `process_root` 1347–1430 |
| 缺基准时融合失效，退回写 `generated.rvm`，最后仍记 `no-baseline` | `process_root` 1360–1369 的 `.filter(\|path\| path.is_file())` |
| 导出落 `<out>/<tag>/<w0>_<w1>/e3d.rvm`，`--baseline-dir` 只认 `<dir>/<tag>_<w0>_<w1>.rvm` | `root_dir` 457–459；`place_baseline_from_dir` 1166–1196 |
| 判定先写 `judgement.json` 再读回，`--in-process` 也一样 | `src/rvm_check/mod.rs` `generate_ours_scoped` 608–724，返回 `Result<(), String>` |
| `elements.json`、`report.json` 无条件写；入库的 `gen_ledger` 读 `ours/report.json` | vendor `rvm_export.rs` 2480、2535；`src/rvm_check/store.rs` `gen_ledger_summary` |
| 已留负载超出窗口后溢写到临时文件；待判批与已留负载各按一个窗口判，最坏约两倍窗口 | vendor `rvm_compare.rs` `consume` 3171、`append_payload` 3234–3252 |
| 无名 SITE 的 `SITE n of WORLD` 序号只在根本身是 SITE 时设置 | gen-model `resolve_node` 542–552；vendor `seed_world_ordinal` 797–826、`begin_root` 1323–1325 |
| 每个根一个子进程；`export_rvm` 每次重开库、扫项目目录、冷启动缓存 | `batch.rs` `generate_in_child`；vendor `export_rvm` 开库段 |
| 每库一次 tty 会话，只有一个 `ALPHA LOG` 块；缺件逐根补导、没有熔断，补导前先删文件 | `batch.rs` `export_all` 1199–1325（补导删文件在 1290） |

## 3. 与既有计划的关系

- vendor `docs/plans/2026-11-08-stream-emit-rvm.md` 的 Phase D，以及任务清单里的「任务 9：以 ZONE 为批次比对」，目标和本方案相同。那里的做法是「SITE 级基准 + 生成侧按 ZONE 分批」，2026-11-09 延后，原因是配对必须整根做，`generated` / `members` 两份数据压不下来。
- 本方案改为 ZONE 直接作根、每个 ZONE 一份基准，配对范围随根降到 ZONE，延后的原因不再存在。方案确认后，在 vendor 任务 9 下注明「由本方案取代」（任务 17）。
- 沿用 vendor 计划已定的口径：
  - D-3：验收认进程真实峰值 `PeakPagefileUsage`，不自记账。
  - D-5：一个 ZONE 一个单元，不合并、不往下拆；单元超预算判超预算，信息里写至少需要多少，不拆。
  - Phase B：融合下缺省不写 `generated.rvm`，`--emit-rvm true` 留作取证。
- 未纳管的 `.kiro/specs/site-split-rvm-compare/requirements.md` 把「拆到 ZONE 级」列在范围外，本方案不改它。

## 4. 设计决定

1. **单元模式。** 新增 `--unit {root,zone}`，缺省 `root`，即现行为（SITE / APPLDW 作根）。
2. **单元划分。** 从 WORLD 下的 SITE / APPLDW 往下走：名词属于 {SITE, APPLDW, APPLDA, APPDAR} 的纯容器继续下钻，其余每个子节点成一个单元。
   - 常态是 ZONE（`unit_kind = zone`）。
   - 非 ZONE 子节点（若有）各成一个兜底单元（`unit_kind = fallback`），与 ZONE 走同一条导出、比对路径。
   - 覆盖由构造保证。APPLDW 下的 ZONE 在第 4 层（APPLDW、APPLDA、APPDAR、ZONE，据 7352 等库的生成报告与基准容器链推断），一并纳入。
3. **基准。** tty 对单元 refno 执行 `export =<refno>`，句子沿用 `export_batch_body`，文件仍落 `<out>/<tag>/<w0>_<w1>/e3d.rvm`。E3D 导 ZONE 时把祖先 SITE 写成空容器（`test_data/rvm/1RX07-LCT.rvm.json` 159 个成员里有 1 个空的 SITE 外壳），比对不看无几何容器，不影响结果。
4. **ZONE 模式固定融合。**
   - 只允许 `--stage full`（导出 + 生成即比对）与 `--stage export`（只导基准）。
   - 先等该单元的基准就位再生成；缺基准记 `no-baseline`，不生成。
   - 比对范围 `All`。
5. **配对范围 = 单元。** 依据：
   - 主键只由成员名派生，序号在属主内计数（vendor `rvm_compare.rs` 的 `Key`，`rvm_export.rs` 的 `owner_keys`）。
   - 隐式管按属主容器分组配对（`pair_tubes_by_centre`），一个容器只属于一个 ZONE。
   - 世界矩阵沿属主链折叠到顶（vendor `transform.rs` `world_matrix`），与生成起点无关。

   是否真的没有跨 ZONE 配对，由检查点 1 的对账确认。
6. **比对预算。** ZONE 模式下 `--compare-memory-mb` 是单元的硬上限：待判批与已留负载合并记账，超了判 `over-budget`，不溢写。`0` 表示不设上限，也不溢写。SITE 模式维持现行为。
7. **进程形态。**
   - 目标态：每个上级根（SITE / APPLDW）一个子进程，子进程内开一次库会话、顺序跑各单元，判完一个释放一个；可提交内存护栏和超时仍按子进程。
   - 阶段 1–3 先沿用每个单元一个子进程。
8. **落盘（目标态）。**
   - 保留：`e3d.rvm`（基准本身）、`run.json`、`summary.json` / `summary.txt`、`ours/gen.log`；子进程模式另有 `judgement.json`（进程间运输件）与 `gen.stdout.log` / `gen.stderr.log`。
   - 去掉：`generated.rvm`、`elements.json`、`report.json`、`arena-spool-*.bin`。
   - `compare.json` 只在 `--emit-compare true` 时写。

目标态命令（在有 `DbOption.toml` 的目录下运行；E3D 口令只走环境变量 `RVM_CHECK_PASSWORD`）：

```powershell
aios-database rvm-check --dbnum 7333 --unit zone --install-dir <E3D 安装目录> --out <产物目录>
aios-database rvm-check --dbnum 7333 --unit zone --install-dir <E3D 安装目录> --out <产物目录> --resume
```

## 5. 依赖关系

- 任务 1 → 任务 2 → 检查点 1。任务 3 与任务 1、2 并行，检查点 1 之前完成。
- 检查点 1 之后，阶段 2（任务 4 → 5，任务 6 可选）与阶段 3（任务 7 → 8；任务 9、10 独立）可以并行。
- 阶段 4 依赖检查点 3（任务 8 的开关要并进会话选项）：任务 11 → 12 → 13 → 14。
- 任务 15 在任务 2 之后即可开工；任务 16 依赖任务 15；任务 17 收尾。

## 6. 任务清单

### 阶段 1：纵切——小库上按 ZONE 跑通「tty 导出 → 生成即比对」

#### 任务 1：单元枚举与普查（`--unit zone`、`--list-units`）

**说明：**

- `batch.rs` 在 `enumerate_roots` 旁加 `enumerate_units`，按设计决定 2 划分单元。
- `RootJob` 加 `unit_kind` 与 `parent`（上级根的 refno、noun、name）。
- `main.rs` 加 `--unit {root,zone}`（缺省 `root`）和 `--list-units`。后者只枚举，写 `<out>/units.json` 并打印摘要后退出，不碰 E3D、不生成。
- 普查内容：每个上级根的单元数、兜底单元数及其 noun、SITE 是否有名。

**验收：**

- [x] 不给 `--unit` 时作业列表与改动前相同（代码路径就是原 `enumerate_roots`；ams1112 `=9304/2` 回归计数逐字相同）
- [x] ams1112 `=9304/2` 下列出 3 个 ZONE（`/1RS-WF05`、`/1RS-WF04`、`/1RS-WF03`）；ams7333 SITE `=23717/2` 下列出 531 个，APPLDW `=23717/188132` 下的单元如实列出（`上下文/会话-2026-08-30-接力DR6W-9DOL.md` 记全库 ZONE 1062 个）
- [x] 兜底单元单列计数，为 0 也写出

**验证：**

- [x] `cargo test --lib rvm_check::batch`
- [x] `aios-database rvm-check --dbnum 1112,7333,7352 --unit zone --list-units --out <目录>`（实跑加了 7999）

**依赖：** 无

**可能改动：** `src/main.rs`（`RvmCheckArgs`、`run_rvm_check`）、`src/rvm_check/batch.rs`（`enumerate_units`、`RootJob`、`BatchOptions`）

**规模：** M

#### 任务 2：ZONE 单元走通「tty 导出 → 先等基准 → 生成即比对」

**说明：**

- `run`：`--unit zone` 时作业来自 `enumerate_units`。`--only` 接受单元 refno 和上级根 refno，后者展开成它的全部单元；未命中照旧告警，文案改成按单元说。
- `run` 641–650：融合不再强制 `--baseline-dir`，仍只允许 `--stage full`。`--unit zone` 时强制融合，只允许 `full` / `export` 两个阶段。
- `--emit-rvm` 改成 `Option<bool>`，缺省值为「不融合才写」（Phase B 已决）。
- `process_root`：融合时在生成之前 `board.wait(index)`。
  - `Missing`：记 `no-baseline` 并返回。
  - `Ready`：用 `<单元目录>/e3d.rvm` 组装 `FusedCompare`。
  - 融合请求下去掉 1360–1369 的 `.filter(is_file)` 退回路径。
- `export_all` 逻辑不变，导出对象换成单元 refno。
- `run.json` 先记 `unit_kind`、`parent`，供检查点 1 对账；入库字段在任务 16。

**验收：**

- [x] `--unit zone` 不给 `--baseline-dir`，一次运行完成 tty 导出与生成即比对；整个 `--out` 下没有 `generated.rvm`（ams1112、ams7999、ams7352 实跑）
- [x] 某单元缺基准时记 `no-baseline`，没有生成段，也没有 `generated.rvm`（`--baseline-dir` 下缺文件验证；E3D 导出失败走同一个 `Missing` 分支）
- [x] 不给 `--unit` 时，原有参数组合的结果不变；新放开的「融合 + tty 导出」对 SITE 根同样可用（ams1112 `=9304/2`、ams7999 `=24383/80211` 实跑）

**验证：**

- [x] `cargo test --lib rvm_check::batch`（新增：融合先等基准；缺基准不生成；`--emit-rvm` 缺省值）
- [x] ams1112 实跑（ZONE 单元与 SITE 根都走 tty 当场导出）

**依赖：** 任务 1

**可能改动：** `src/main.rs`、`src/rvm_check/batch.rs`（`run`、`process_root`）

**规模：** M

#### 任务 3：无名 SITE 下 ZONE 单元的序号

**说明：** E3D 把无名 SITE 显示为 `SITE n of WORLD /*`，n 按整个 MDB 的 WORLD 成员数。

- gen-model `resolve_node`：对单元根上溯到 SITE 祖先，按 MDB 顺序算 n（现在只在 noun 是 SITE 时算，542–552）。
- vendor `seed_world_ordinal`：从根沿属主链找到属主为 WORL 的 SITE 祖先，给它播种（现在根不是 SITE 就直接返回，804–808）。
- `RvmExportOptions.root_world_ordinal` 的说明改为「根所在 SITE 的 n」。

**验收：**

- [x] 根是 SITE 时结果不变（ams1112 `=9304/2` 计数与 11-09 真机记录逐字相同；有名 SITE 不再算序号，vendor 本来就不用）
- [x] vendor 新单测：序号播在根所在的 SITE 上（根是 SITE 播它自己，ZONE 及更深上溯到 SITE，APPLDW 链不播，成环报错）。成员名的端到端核对等语料里出现无名 SITE 再做
- [x] 任务 1 普查出的无名 SITE（若有）实跑一个单元：1112、7333、7352、7999 四个库的 SITE 都有名，暂无可跑

**验证：**

- [x] vendor `cargo test --lib`（496 passed）
- [x] `cargo test --lib rvm_check`（45 passed）

**依赖：** 无（与任务 1、2 并行）

**可能改动：** vendor `src/rvm_export.rs`；`src/rvm_check/mod.rs`

**规模：** S

#### 检查点 1：阶段 1 之后

- [x] 同一二进制、同一库会话下，ams1112 `=9304/2` 三个 ZONE 单元的计数之和，与同次跑的 SITE 级融合（`--only 9304/2`）一致。比的字段：`both`、`exact`、`chord`、`reference_artifact`、`film`、`off`、`only_baseline`、`only_generated`、`kind_differ`（`baseline_members` 不在判定计数里，没有比）。两边都是 5135 / 5132，没有兜底单元
- [x] ams7999 `=24383/80211` 同上：普查是 19 个 ZONE，两边都是 8829 / 8829（2 个无名空 ZONE 判 `empty`）。vendor 计划 D-0 记的 18 个、34,325 件与实测不符
- [x] ams7333：先小跑 24 个 ZONE（两个上级根各 12 个，refno 取自 `units.json` 传给 `--only`），全 pass，3996 / 3996；随后全部 1062 个 ZONE 跑完，两个上级根各自的 531 个 ZONE 之和与同次整根逐格相同（各 79,938 / 79,938），没有兜底单元（release 构建，见第 11 节「ams7333 全量验收」）
- [x] ams7352 APPLDW `=23736/82` 下的 ZONE `=23736/85` 跑通（SITE 与 APPLDW 各一个 ZONE，48 / 48）
- [x] 每个单元的生成子进程峰值写进 `run.json`：实测单 ZONE 113–270 MB（24 个单元）
- [ ] 和用户过一遍，再进阶段 2、3（11-10 已汇报实测结果；阶段 2、3 还没得到开工确认）

这一关同时验证两件事：逐 ZONE 导出没有丢件或重复（开孔、隐式管），以及配对确实不跨 ZONE。对不上就停在这里查。

### 阶段 2：导出规模化（几百到上千个单元）

#### 任务 4：分块导出会话、按块交接与熔断

**说明：**

- `export_all` 改为每 N 个单元一次会话（`--e3d-chunk N`，起步 100，检查点 2 校准）。
- 每块会话结束就逐个 `board.set`，生成在第一块落盘后即可开始。现在要等整库会话结束才交接。
- 熔断：
  - 一块会话没登录成功、超时，或一个完成标记都没有：这一块全部记 `no-baseline`（原因 `export-session-failed`），不逐个补导。
  - 同一库连续 2 块失败：余下的块直接记同样原因。
- 单个补导只在本块会话正常、且缺的不超过 5 个时做；补导前不删已有的完整文件。

**验收：**

- [ ] 单测：分块（含余数）、熔断判据、补导上限
- [ ] 人为制造登录失败时，登录次数不超过「块数 + 补导上限」，不出现逐单元补导
- [ ] ams7333 全部单元导出：记下块数、总耗时、每块耗时；没有原因不明的 `no-baseline`（分块还没做。现有的单会话导出 11-10 实测：1062 个单元 1 次会话 191.1 秒，没有 `no-baseline`，见第 11 节）

**验证：**

- [ ] `cargo test --lib rvm_check::batch`
- [ ] 实跑，含一次人为失败

**依赖：** 检查点 1

**可能改动：** `src/rvm_check/batch.rs`（`export_all`）、`src/main.rs`

**规模：** M

#### 任务 5：导出续跑与基准过期护栏

**说明：**

- `--resume` 时导出阶段跳过「导出结果为 batch / refno / name、`e3d.rvm` 仍在」的单元。现在 `--resume` 只跳已比对的根，`exported` 不跳（773–785）。
- 过期护栏的口径要先查清：每次 tty 登录都会给打开的库追加一条只含 DBMDAT 的尾会话（`src/rvm_check/mod.rs` 208–209 的预检说明），所以库的会话号会随导出本身变化，不能直接拿「导出时的会话号」与「当前会话号」比。
  - 本任务先确认能不能读到「最后一个带数据的会话」（`DirectTreeService::source_metadata` 或库文件会话表）。
  - 能读到：把它写进单元 `run.json` 的导出段，比对前不一致记 `stale-baseline`、不生成。
  - 读不到：只做续跑，护栏留空，在本文记下原因。

**验收：**

- [ ] 中断后 `--resume` 只导没导完的单元
- [ ] 护栏落地时：库里有新的数据会话后，旧基准不会被拿来比

**验证：** 单测，加一次中断后的实跑

**依赖：** 任务 4

**可能改动：** `src/rvm_check/batch.rs`（`RootRecord`、`StageRecord`、续跑分支、`process_root`）

**规模：** S–M

#### 任务 6（可选）：基准完整性判据

**说明：** 现判据是完成标记加文件大于 32 字节（1280 行）。宏开头是 `onerror continue`，`export ={refno}` 失败后完成标记照样打，只写了文件头的残件可能被当成成功。加一条：文件尾有 RVM 结束块。

**验收：**

- [ ] 截断的 RVM 判缺失；完整文件不受影响

**依赖：** 检查点 1

**可能改动：** `src/rvm_check/batch.rs`

**规模：** S

#### 检查点 2

- [x] ams7333 全部单元一次跑完导出与比对；记总耗时、E3D 登录次数、`no-baseline` 原因分布（11-10，release 构建：墙钟 1199.9 秒，登录 1 次，`no-baseline` 0 个。用的是现有的单会话导出，任务 4–6 都还没做；见第 11 节）
- [x] 与检查点 1 的同一批 ZONE 结果一致（小跑的 24 个 ZONE 与全量里的同一批单元，终态与 16 个计数逐格相同）

### 阶段 3：去掉落盘回读

#### 任务 7：判定留在内存里

**说明：**

- `generate_ours_scoped` 返回生成结果（判定 + 生成侧账本计数），不再只是 `Result<(), String>`。
- `generate_in_process` 直接用这份结果，进程内不写也不读 `judgement.json`。
- 子进程模式仍写 `judgement.json` 做运输，账本计数并进去；`store::gen_ledger_summary` 不再读 `ours/report.json`。
- 页面 `execute` 跟着改签名，行为不变。

**验收：**

- [ ] `--in-process` 跑完单元目录里没有 `judgement.json`
- [ ] 判定与子进程模式的 JSON 序列化逐字节相同；入库的 `gen_ledger` 不变

**验证：** `cargo test --lib rvm_check`（含 `judging_the_struct_matches_judging_its_json`）

**依赖：** 检查点 1

**可能改动：** `src/rvm_check/mod.rs`、`src/rvm_check/batch.rs`、`src/rvm_check/store.rs`

**规模：** M

#### 任务 8：融合时不写 `elements.json` / `report.json`（vendor 开关）

**说明：**

- vendor `RvmExportOptions` 加 `side_files: bool`，缺省 true，现有调用方不变。
- 关掉时不写 `elements.json`、`report.json`，也不收集只为它们服务的 invariants 与点集记录；`RvmExportOutcome` 的两条路径改成 `Option`。
- gen-model 在融合且不写 RVM 时关掉。

**验收：**

- [ ] 开关缺省时产物逐字节不变
- [ ] 关掉时 `ours/` 下只剩 `gen.log`，判定不变

**验证：** vendor `cargo test --lib`；gen-model `cargo check`、`cargo test --lib rvm_check`

**依赖：** 任务 7（账本计数改从内存拿）

**可能改动：** vendor `src/rvm_export.rs`，以及引用这两条路径的 `src/bin/gen_ams.rs`；`src/rvm_check/mod.rs`

**规模：** S

#### 任务 9：比对预算改成硬上限，不溢写

**说明：**

- vendor `StreamingCompare` 把待判批（`consume` 3171）与内存里的已留负载（`append_payload` 3237）合并记账。
- 加溢写策略（允许 / 拒绝）。拒绝时超预算返回 `CompareOverBudget { need_bytes }`，不建 `ArenaSpool`。
- gen-model 在 ZONE 模式下用「拒绝」，新增状态 `over-budget`：不算干净，信息里写至少需要多少 MB。
- 顺手改掉「负载全在内存」的过时注释与打印（`main.rs` 184–188，`batch.rs` 139–143、812–816）：vendor 早已接好溢写。

**验收：**

- [ ] vendor 单测：预算内时「拒绝溢写」与现状报告逐字节相同；超预算返回错误且不产生临时文件；合并记账后待判批加已留负载不超过一倍预算
- [ ] 小预算跑一个 ZONE 判 `over-budget`，信息里有所需字节数；目录里没有 `arena-spool-*`

**验证：** vendor `cargo test --lib`；`cargo test --lib rvm_check::batch`

**依赖：** 检查点 1

**可能改动：** vendor `src/rvm_compare.rs`、`src/rvm_export.rs`；`src/rvm_check/batch.rs`、`src/rvm_check/mod.rs`、`src/main.rs`

**规模：** M

#### 任务 10：`--baseline-dir` 直接读源文件，并认导出目录布局

**说明：**

- `place_baseline_from_dir` 先找扁平名 `<tag>_<w0>_<w1>.rvm`，找不到再找 `<dir>/<tag>/<w0>_<w1>/e3d.rvm`。这样前一次 `--stage export` 的 `--out` 可以直接当 `--baseline-dir`。
- 融合且不写 RVM 时不再硬链接或复制，把源路径交给 `FusedCompare`；源和目标是同一路径时不删（现在 1180 行会先删目标）。
- `--emit-rvm true`、`--stage compare` 仍按旧行为放进 `e3d.rvm`。

**验收：**

- [ ] 用前一次导出的 `--out` 当 `--baseline-dir` 能跑通
- [ ] 融合跑完，单元目录里没有基准副本；其余组合不变

**验证：** `cargo test --lib rvm_check::batch`

**依赖：** 检查点 1

**可能改动：** `src/rvm_check/batch.rs`

**规模：** S

#### 检查点 3

- [ ] 进程内模式下单元目录只剩 `e3d.rvm`、`run.json`、`ours/gen.log`；子进程模式另有 `judgement.json` 和两份生成日志
- [ ] 全部判定与检查点 2 一致；没有 `arena-spool-*` 残留

### 阶段 4：性能——每个上级根一个子进程，会话内顺序跑单元

现状每个单元都要起一次子进程（约 0.3–0.9 秒）、开一次库（约 0.1–0.35 秒），还要冷启动缓存。ams7333 粗算光开销就有 4–9 分钟，而整个 SITE 的生成只要 49–74 秒（`20261107-full` 记 52.0 秒与 73.6 秒）。

#### 任务 11：vendor 会话式导出入口

**说明：**

- 把 `export_rvm` 拆成「开会话」与「跑一个根」。会话持有 `DbSet`、resolver、attlib、UDA、UDET、目录网格缓存。
- `export_root` 每次带自己的 root、基准、比对范围、`root_world_ordinal`、截止时间，返回内存里的比对报告与账本。
- `export_rvm` 改成「开会话 + 跑一个根」的薄包装。vendor 内部的 `for root in roots`（2324）本来就共享库句柄和缓存，缺的是每个根各自一份基准、一份报告。

**验收：**

- [ ] vendor 现有测试全过，`export_rvm` 产物逐字节不变
- [ ] 同一批根「会话内顺序跑」与「逐个 `export_rvm`」，报告逐字节相同

**验证：** vendor `cargo test --lib`

**依赖：** 检查点 3

**可能改动：** vendor `src/rvm_export.rs`（可拆出会话子模块）

**规模：** M

#### 任务 12：会话内跨根缓存

**说明：**

- 元素缓存跨根保留，受上限约束。现在随 `export_rvm` 返回即丢（`src/rvm_check/mod.rs` 666–667 的注释）。
- 目录网格缓存加条数或字节上限。现在一次调用内没有上限（vendor `catalogue.rs`）。
- 每个根结束时释放根内状态（`begin_root` 已清掉大部分）。

**验收：**

- [ ] 跨根后元素缓存、目录网格缓存都不超过各自上限
- [ ] 报告与逐根独立跑逐字节相同

**验证：** vendor `cargo test --lib`

**依赖：** 任务 11

**可能改动：** vendor `src/rvm_export.rs`、`src/catalogue.rs`

**规模：** S

#### 任务 13：子进程按上级根分组

**说明：**

- 作业按 `parent` 分组，每组一个子进程。`--child` 接一份单元清单（单元 refno、基准路径、单元目录）。
- 子进程开一次会话顺序跑，每个单元比完写该单元的 `judgement.json` 并释放；父进程逐单元收结果、写 `run.json`。
- 可提交内存护栏照旧按子进程。超时改为单元内截止时间，加整组封顶。
- 单元 panic 后重建会话，同组其余单元照跑。

**验收：**

- [ ] ams7333 `=23717/2`（531 个 ZONE）的判定与阶段 3「每单元一个子进程」逐字节相同
- [ ] 总耗时不超过同次 SITE 级融合的 2 倍；每组子进程峰值不超过 2 GB。这两个是起步值，检查点 3 后按实测改（改造前的基线，11-10 release 构建：按 ZONE 墙钟 1199.9 秒，整根 247.7 秒，4.8 倍；见第 11 节）
- [ ] 人为让一个单元 panic，同组其余单元照常出结果

**验证：** `cargo test --lib rvm_check::batch`；实跑

**依赖：** 任务 11、12

**可能改动：** `src/rvm_check/batch.rs`（分组、`run_child`、`generate_in_child`、`process_root`）、`src/rvm_check/mod.rs`、`src/main.rs`

**规模：** M–L

#### 任务 14：进程内模式按组跑，调整并发参数

**说明：**

- `--in-process` 同样按组用会话。
- `--jobs` 改为同时在飞的组数；`--threads` 缺省按逻辑核数除以 `--jobs`。
- 进程内没有可提交内存护栏，帮助文本写明 ZONE 模式推荐子进程。

**验收：**

- [ ] 进程内与子进程模式判定逐字节相同
- [ ] `--jobs 2` 时同一时刻最多两个组在跑

**验证：** `cargo test --lib rvm_check::batch`

**依赖：** 任务 13

**可能改动：** `src/rvm_check/batch.rs`、`src/main.rs`

**规模：** S

#### 检查点 4

- [ ] ams7333 全部单元跑完；总耗时、E3D 登录次数、每组峰值记进本文
- [ ] 全部判定与检查点 3 一致

### 阶段 5：结果记录、查询与文档

#### 任务 15：`run.json` / `summary` 按单元与上级根汇总

**说明：** `RootRecord` 收口 `unit_kind`、`parent`；`Summary` 加按上级根的汇总和兜底单元清单；`summary.txt` 加列。

**验收：**

- [ ] 单测断言每个上级根的计数等于其单元之和

**依赖：** 任务 2

**可能改动：** `src/rvm_check/batch.rs`

**规模：** S

#### 任务 16：入库字段、历史查询与逐单元入库

**状态：** 2026-11-10 应用户要求提前做完（不依赖任务 15：按上级根的汇总放在库的查询里做）。字段名与原稿不同，按落地的写。

**说明：**

- `RunRow` 加 `unit`（`root` / `zone` / `fallback`）、`root_refno`、`root_noun`、`root_name`：单元行记它的上级 SITE / APPLDW，根行记它自己，所以「某个根的整根行 + 它下面的全部单元」是一个条件。`MismatchRow` 加 `root_refno`。表是 SCHEMALESS，不用迁移；页面单根与此前的批量行没有这几格，按上级根查时把自己的 `refno` 当上级根。
- 两张表各建 `root_refno` 索引；`RunFilter` 加 `root_refno`、`unit`，`MismatchFilter` 加 `root_refno`（`history` / `mismatches` / `reasons` 三个接口随之多出这些参数）。
- 新增按上级根汇总：`store::rollup_on`，`GET /api/v1/rvm-check/rollup`。整根跑法与按 ZONE 拆开的跑法分开成行（`mode`），两行的计数可直接对账。
- 新增命令行 `aios-database rvm-check-query rollup|runs|mismatches|reasons`：直接连库、只读、服务不必开着，应答与 HTTP 接口同一份（`store::answer_on`）。
- 每个作业跑完就入库（`store::StoreWorker`，独立线程、连一次库），交出去之后放掉不截断的差异清单。`--resume` 时把跳过的作业里库里还没有的补进去，差异清单取作业目录里全量的那份（`judgement.json`，或从 `compare.json` 重判）。

**验收：**

- [x] 按 SITE refno 查得到它全部 ZONE 的行（`rvm-check-query runs --root 9304/2`：3 个 ZONE 单元行 + 整根行，连同 11-08 写进来的老行）
- [x] 长批次边跑边落库（ams7999 19 个单元，约每 8 秒查一次库：2、4、8、12、15、17、19 行，与盘上 `run.json` 个数同步）
- [x] 跑 ams7333 全部单元时，父进程峰值不随单元数增长（1062 个单元：第一个单元出结果时 106.9 MB，最后一个 107.8 MB，峰值 107.8 MB，每 100 个单元涨 0.08 MB；见第 11 节）

**验证：** `cargo test --lib rvm_check`（52 passed）；实跑见第 11 节

**依赖：** 任务 2

**可能改动：** `src/rvm_check/store.rs`、`src/rvm_check/batch.rs`、`src/rvm_check/report.rs`、`src/web_service/rvm_check.rs`、`src/web_service/mod.rs`、`src/main.rs`

**规模：** M

#### 任务 17：帮助文本与文档

**说明：** 更新 `main.rs` 的 rvm-check 说明与参数帮助；`docs/guides/rvm-check-batch.md` 加 ZONE 模式；在 vendor 计划任务 9 下注明由本方案取代。

**验收：**

- [ ] `aios-database rvm-check --help` 与代码行为一致

**依赖：** 任务 1–16

**规模：** S

#### 检查点 5：完成

- [ ] 全部任务验收打勾
- [ ] 九个库（7999、7326、7327、7331、7332、7333、7352、7354、7356）按 ZONE 全量跑一遍，与同一二进制的 SITE 级融合逐库对账
- [ ] 用户评审

## 7. 风险与应对

| 风险 | 影响 | 应对 |
|---|---|---|
| 逐 ZONE 导出时开孔、隐式管丢件或重复 | 高 | 检查点 1 的「各单元之和 = 整根」对账；对不上停在阶段 1 |
| 生成侧把件挂错属主，配对跨 ZONE | 中 | 表现为两个单元各一条 `only_*`，对账可见；这本身就是生成缺陷 |
| 无名 SITE 序号 | 中 | 任务 3；普查语料里的无名 SITE |
| 无名 SITE 下在 ZONE 内再用 `--scope`，锚点路径口径不一致（vendor `scope_anchor`） | 低 | ZONE 模式缺省不用 `--scope`；记为已知限制 |
| 每次 tty 登录给库追加一条只含 DBMDAT 的尾会话，会话号随导出变化，增量监听可能空转一轮 | 中 | 分块会话数可控（任务 4）；过期护栏不直接比会话号（任务 5） |
| `ALPHA LOG` 到会话结束才落盘，会话崩溃会丢掉全部完成标记 | 中 | 分块缩小影响面；任务 6 的完整性判据 |
| 每单元固定开销乘以单元数 | 高 | 阶段 4 的会话入口与按组子进程 |
| 进程内模式没有可提交内存护栏 | 中 | ZONE 模式推荐子进程（任务 14 写进帮助） |
| 生成器修复让结果与历史批次不可比 | 中 | 对账一律用同一二进制、同一库会话的 SITE 级跑做参照 |
| vendor 工作树里有别人未提交的 `examples/` 改动 | 低 | vendor 只改 `src/`，单独提交，不碰 `examples/` |

## 8. 待确认

1. 兜底单元：每个非 ZONE 子节点各成一个单元（本方案缺省），还是按 vendor D-5 把一个根的兜底成员合成一份？合成需要一次会话把多个元素导进同一个 RVM 文件，E3D 是否支持还没验证。
2. ZONE 模式是否固定融合（本方案缺省固定；`--emit-rvm true` 留作取证）。
3. ZONE 模式下 `--compare-memory-mb 0` 的含义：不设上限且不溢写（本方案缺省），还是给一个缺省硬上限。
4. APPLDW 下的 ZONE 是否纳入。本方案缺省纳入；不纳入则 7326、7327、7331、7332、7333、7352、7354 七个库少约一半件。
5. 阶段 4 的门限起步值（总耗时不超过 SITE 级融合 2 倍、每组峰值不超过 2 GB）：是否先按起步值做，检查点 3 后再改。

## 9. 不在本方案里做

- 页面单根比对（`POST /api/v1/rvm-check/runs`）改融合，以及前端。
- `src/rvm_baseline` 路线（表面距离）。以后若要验服务显示的模型，可以挂在 `E3dModelService::generate_snapshot_source` 的产物上做附加判据。
- SITE 级基准按 ZONE 切片（2026-11-10 未选）。
- 判档口径（容差、主键）的改动。
- 生成内存（9 GB 归因），归 vendor 计划的「另立的计划」。

## 10. 验证命令约定

- gen-model：`cargo check`、`cargo clippy`、`cargo test --lib rvm_check`（默认 feature）。
- vendor：`cargo test --lib`。
- 实跑：在有 `DbOption.toml` 的部署目录或仓库根下运行 `aios-database rvm-check …`。E3D 口令只走环境变量 `RVM_CHECK_PASSWORD`，不进命令行、日志与文档。

## 11. 进展记录

### 2026-11-10：阶段 1 代码完成

改动（未提交）：

- gen-model `src/rvm_check/batch.rs`：单元划分（`collect_units`、`units_of`）、`--only` 认上级根（`only_keeps`）、参数约束（`check_modes`）、融合先等基准（`fused_from`）、`--list-units` 写 `units.json`、`run.json` / `summary.json` 的单元字段。
- gen-model `src/rvm_check/mod.rs`：`resolve_node` 为单元根算它所在 SITE 的序号，只在 SITE 无名时才扫 MDB 的 SITE 根表。
- gen-model `src/main.rs`：`--unit`、`--list-units`；`--emit-rvm` 改为可不给，缺省随融合。行为变化：`--fused-compare` 不再缺省写 `generated.rvm`（Phase B 已决）。
- vendor `src/rvm_export.rs`：`seed_world_ordinal` 播在根所在的 SITE 上，找 SITE 的逻辑抽成 `site_of` 并加单测；`src/bin/gen_ams.rs`、`tests/ancestor_reads_guard.rs` 只改说明文字。

实测（debug 构建，`--no-store`）：

| 跑法 | 结果 |
|---|---|
| `--unit zone --list-units`，库 1112 / 7333 / 7352 / 7999 | 9 个上级根、1092 个单元，3 秒。1112 `=9304/2` 3 个 ZONE；7333 SITE 与 APPLDW 各 531 个；7352 的 APPLDW 链找到 1 个；7999 `=24383/80211` 19 个。没有兜底单元，SITE 全部有名 |
| `--unit zone --only 24383/91690,24383/84086 --baseline-dir`（`/1RX07-LCT` 的基准用 `test_data/rvm/1RX07-LCT.rvm`） | `/1RX07-LCT` pass，137 / 137，生成 1.5 秒、115 MB；`/5RX01-LCT` 缺基准记 `no-baseline`，没有生成；全目录没有 `generated.rvm` |
| 同一 ZONE 走 SITE 级基准 + `--scope subtree:24383/91690` | pass，137 / 137，与上一行一致 |
| SITE 根回归：`--only 9304/2 --fused-compare --baseline-dir` | pass，5135 / 5131，与 11-09 真机记录相同；`run.json` 没有单元字段；没有 `generated.rvm` |

E3D 当场导出的对账见下一小节。

附带发现：vendor `tests/ancestor_reads_guard.rs` 的 `every_owner_walk_is_registered_and_reads_only_what_it_declares` 在 HEAD 上就是红的。`key_for`（`1ac43b4`，11-08）和 `scope_anchor`（`345841e`，11-09）调用了 `.owner()`，却没在护栏里登记。与本次改动无关，没有修。

### 2026-11-10：AMS 验收（E3D 当场导出）

debug 构建，E3D 安装目录 `E:\reverse\e3d\shadow_e3d31_gen_model_test`，每个库一次 `des.exe -tty` 会话当场导出，`--no-store`。E3D 口令取启动脚本 `scripts/e3d/run_ams_c_entrymacro.bat` 的缺省登录串，只进当次会话的环境变量。产物在 `output/rvm-check-zone/accept-20261110/`（原始汇总另存为各目录的 `summary.e3d-run.json` / `.txt`）。

| 跑法 | 结果 | 墙钟 | 生成 | 导出 | 生成峰值 |
|---|---|---|---|---|---|
| ams1112 `=9304/2`，3 个 ZONE | pass 3，5135 / 5132 | 21.2 s | 8.5 s | 11.1 s | 232 MB |
| 同一 SITE 整根（`--fused-compare`） | pass，5135 / 5132 | 18.3 s | 7.0 s | 10.4 s | 470 MB |
| ams7999 `=24383/80211`，19 个 ZONE | 17 pass、2 empty，8829 / 8829 | 72.5 s | 54.0 s | 17.2 s | 270 MB |
| 同一 SITE 整根 | pass，8829 / 8829 | 54.9 s | 33.5 s | 19.8 s | 790 MB |
| ams7352，SITE 与 APPLDW 各 1 个 ZONE | pass 2，48 / 48 | 12.4 s | 3.0 s | 8.3 s | 115 MB |

- 各 ZONE 之和与整根逐格相同（ams1112 的 `reference_artifact` 2、`film` 1 也相同）；五个产物目录里都没有 `generated.rvm`。
- 父进程峰值约 100 MB，`des.exe` 峰值 215–241 MB，可提交余量最低 37 GB。
- ams7999 按 ZONE 的生成从 33.5 秒涨到 54.0 秒：19 次子进程、19 次重开库（阶段 4 要收掉的固定开销）。
- 每次 E3D 登录都给 SYS 库（8191）追加了一个会话，当时开着的服务把它们各当一批 `data_batch` 处理（`logs/task-history-2026-11-10.jsonl`，全部 succeeded）。

### 2026-11-10：结果入库与查询（任务 16 提前）

改动（未提交）：

- `src/rvm_check/store.rs`：运行行加 `unit`、`root_refno`、`root_noun`、`root_name`，差异行加 `root_refno`，两张表各加一个 `root_refno` 索引；`StoreWorker`（逐个作业落库）、`stored_run_ids_on`（续跑补缺）、`rollup_on`（按上级根汇总）、`Query` / `answer_on` / `query_blocking`（命令行与 HTTP 共用的查询入口）；删掉整批结束才写的 `persist_batch_blocking`。
- `src/rvm_check/batch.rs`：作业跑完即交入库线程并放掉全量差异清单；`--resume` 补缺（`resumed_facts` / `full_judgement`）；入库事实带单元种类与上级根。
- `src/rvm_check/report.rs`：`RvmCounts::add`（批量汇总与库里的汇总共用）。
- `src/web_service/rvm_check.rs`、`src/web_service/mod.rs`：新增 `GET /api/v1/rvm-check/rollup`；四个查询接口改走 `store::answer`。
- `src/main.rs`：新增子命令 `rvm-check-query`。
- `docs/guides/rvm-check-batch.md`：入库与查询一节按现状重写。

连库方式：入库线程沿用原来的旁路入口 `attach_store_connected`（连上时会重载 `resource/surreal` 下的函数定义，与此前批量入库相同，只是从批末提前到批首）；命令行查询另走只读连接（只连接、登录、`SELECT`，不重载函数、不建表）。

实测（本机 `ws://localhost:8009` 那台库，库里原有 11-08 写入的 31 行批量记录）：

| 操作 | 结果 |
|---|---|
| 上面五次验收用 `--resume --baseline-dir` 补入库（不重跑、不碰 E3D） | 26 行：批次 `accept-20261110/1112-zone`（3）、`/1112-site`（1）、`/7999-zone`（19）、`/7999-site`（1）、`/7352-zone`（2） |
| 同一条命令再跑一遍 | 「这次没有新结果要写，库里已经是全的」，不重复写 |
| ams7999 19 个 ZONE 新跑一批（批次 `store-check-20261110/7999-zone`，基准用验收时导出的），期间约每 8 秒查一次库 | 库里行数 2、4、8、12、15、17、19，与盘上 `run.json` 个数同步；批量在跑时只读查询照常可用 |
| `rvm-check-query rollup` | 1112：按 ZONE 3 个单元 5135 / 5132，整根 5135 / 5132；7999：19 个单元 8829 / 8829，整根 8829 / 8829 |
| `rvm-check-query runs --root 9304/2` | 8 行：今天的 3 个 ZONE 单元行与整根行，加 11-08 的 4 条老行（没有 `root_refno`，按自己的 `refno` 命中） |
| `rvm-check-query reasons` / `mismatches --root …`（11-08 的失败批次） | 按原因计数与差异明细都查得出 |

没做的：

- `GET /api/v1/rvm-check/rollup` 没有起服务实测（它与命令行共用 `store::answer_on`，后者有单测并实测过）。
- ams7333 全部单元时父进程峰值是否持平，没量。
- 任务 15（`summary.json` 里按上级根汇总）没动；按上级根的合计目前从库里查。

### 2026-11-10：ams7333 全量验收（1062 个 ZONE，release 构建，入库）

全量跑通：1062 个 ZONE 全部 `pass`，两个上级根的 ZONE 之和与同一二进制的整根结果逐格相同，1062 行全部入库，父进程内存持平。

**构建。** 这一小节的数字都来自 release 构建，上面两小节是 debug，耗时不能直接比。

- `cargo build --release --bin aios-database`。`Cargo.toml` 的 `[profile.release]` 是注释掉的，用的是 cargo 缺省档。首次 6 分 07 秒，改完代码后增量 3 分 33 秒。
- 可执行文件复制到 `output/rvm-check-zone/accept-20261110/bin/aios-database.exe` 再运行，下面几次运行用的是同一份（build_id `0.1.44+g0890e488dc24.1794288764.dirty`，e3d-model `gdb65e1ca4942.dirty`）。
- E3D 安装目录、口令取法同上一小节；每次 E3D 当场导出。

```powershell
aios-database rvm-check --dbnum 7333 --unit zone --install-dir <E3D 安装目录> --out output\rvm-check-zone\accept-20261110\7333-zone
aios-database rvm-check --dbnum 7333 --fused-compare --install-dir <E3D 安装目录> --out output\rvm-check-zone\accept-20261110\7333-site
```

| 跑法 | 结果 | 墙钟 | E3D 导出 | 生成（记录值合计） | 生成峰值 | 父进程峰值 |
|---|---|---|---|---|---|---|
| 小跑 24 个 ZONE（两个上级根各 12 个） | pass 24，3996 / 3996 | 36.6 s | 1 次会话 13.0 s | 21.0 s | 137 MB | 104 MB |
| 全部 1062 个 ZONE（`--jobs 1`） | pass 1062，159,876 / 159,876 | 1199.9 s | 1 次会话 191.1 s | 1000.5 s | 162 MB | 107.8 MB |
| 两个整根（`--fused-compare`） | pass 2，159,876 / 159,876 | 247.7 s | 1 次会话 180.6 s | 63.5 s（32.0 + 31.5） | 2287 MB | 151.6 MB（内核累计 191.9 MB） |
| 全部 1062 个 ZONE，`--jobs 4`，基准取第一遍导出的，`--no-store` | pass 1062，159,876 / 159,876 | 271.0 s | 不碰 E3D | 1066.0 s（4 路并发，生成段墙钟 266.7 s） | 162 MB | 105.8 MB |

全量那一次（13:37:45 开始，13:57:45 结束）：

- **墙钟 1199.9 秒**：启动约 3 秒，导出会话 191.1 秒，生成段 1003 秒，收尾约 1 秒。
- **导出。** 1062 个单元一次会话、一次登录（SYS 库的会话号 1444 → 1445），191.1 秒走完，退出码 0，1062 份基准全部落盘（合计 860 MB），没有补导，每秒约 5.6 个单元。`des.exe` 私有内存从第 30 秒的 219 MB 涨到结束时的 274.6 MB。比两个整根的会话（180.6 秒）多 6%。
- **生成。** 记录值合计 1000.5 秒（SITE 下 509.0 秒，APPLDW 下 491.5 秒）。每单元的记录值只有三档：0.5 秒 124 个，1.0 秒 937 个，1.5 秒 1 个，因为父进程每 0.5 秒才看一次子进程退没退。按文件时间戳算（`gen.stdout.log` 建立到 `judgement.json` 写完），子进程实际合计 582.7 秒：均值 549 ms，中位 536 ms，p90 626 ms，p99 783 ms，最大 1098 ms。生成段约四成时间花在等轮询上。每单元的构件数中位 132，最大 1155。
- **子进程的 582.7 秒花在哪。** 子进程自己的计时（`ours/gen.log` 的「计时」行，1062 个单元都有）：生成循环合计 75.5 秒（每单元均值 71 ms，最大 311 ms），循环外的开库与扫描合计 116.8 秒（均值 110 ms）；其余约 390 秒是进程启动、读配置与 schema、绑定 MDB。真正的生成量与整根的 63.5 秒相当，多出来的都是每单元一次的固定开销。
- **子进程峰值。** 最小 113 MB，中位 118 MB，p90 122 MB，p99 146 MB，最大 162 MB（`/MDS/VH189` 和 APPLDW 下的同名 ZONE，各 1155 件）。整根是 2287 MB。
- **父进程私有内存。** 导出段平均 106.5 MB；第一个单元出结果时 106.9 MB，跑到一半 107.1 MB，最后 107.8 MB，峰值 107.8 MB。对已出结果的单元数做回归，每 100 个单元涨 0.08 MB，全程涨 0.9 MB；句柄数一直是 219。父进程内存不随单元数增长。
- **终态。** pass 1062；empty、fail、no-baseline、gen-failed 都是 0。
- **其它。** 可提交余量最低 35.5 GB。产物目录 1.25 GB，其中基准 860 MB；没有 `generated.rvm`、`compare.json`、`arena-spool-*`。

对账（同一二进制，16 个计数字段，盘上的 `run.json` 和库里的 `rollup` 两处都比过）：

| 上级根 | 531 个 ZONE 之和 | 整根 | 是否一致 |
|---|---|---|---|
| `=23717/2` SITE `/MDS/HANGERS` | both 79,938，exact 79,938，其余 0 | both 79,938，exact 79,938，其余 0 | 逐格相同 |
| `=23717/188132` APPLDW `/SUPPORT/HANGERS/PIHA` | both 79,938，exact 79,938，其余 0 | both 79,938，exact 79,938，其余 0 | 逐格相同 |

- 小跑的 24 个 ZONE 在小跑、全量、`--jobs 4` 三次运行里终态和计数相同。
- `--jobs 4` 那一遍与全量的 1062 份 `judgement.json` 逐字节相同。

与整根比：

- 墙钟 4.8 倍，生成记录值 15.8 倍（按子进程实际耗时 9.2 倍），导出 1.06 倍，生成峰值约 1/14。
- 任务 13 的起步门限（总耗时不超过整根的 2 倍）现在不达标。原因是每个单元起一次子进程（实际约 0.55 秒，其中生成循环只有 71 ms，其余是启动、读配置、绑定 MDB、开库），再加 0.5 秒一轮的轮询。这正是阶段 4 要收掉的固定开销。
- `--jobs 4` 把生成段从 1003 秒压到 267 秒。按「导出 191 秒 + 生成 267 秒」推算约 7.7 分钟，是整根的 1.9 倍；这一条没有整条实跑。

干扰：同一台机器上另有会话在编译。

- 全量期间 17% 的采样点上有 `rustc` 在跑，最多同时 8 个，集中在开头 73 秒（导出段）；整机 CPU 平均 26.5%（32 个逻辑核）。
- 生成段在有干扰的采样点上每单元 0.967 秒，没有干扰时 0.942 秒，折合全程不到 1%。
- 整根那次 40% 的采样点上有 `rustc`，最多 4 个。

入库核对（`rvm-check-query`，只读）：

- 库里现在 `rvm_check_run` 1164 行，17 个批次，全部是批量行；`rvm_check_mismatch` 132 行，都属于 11-08 的两个批次。

  | 批次 | 行数 | 终态 | 差异行 |
  |---|---|---|---|
  | `accept-20261110/7333-zone` | 1062 | pass 1062 | 0 |
  | `accept-20261110/7333-site` | 2 | pass 2 | 0 |
  | `accept-20261110/7333-smoke`（这次的小跑） | 24 | pass 24 | 0 |
  | `accept-20261110/1112-zone`、`1112-site`、`7999-zone`、`7999-site`、`7352-zone` | 3、1、19、1、2 | pass；`7999-zone` 另有 empty 2 | 0 |
  | `store-check-20261110/7999-zone` | 19 | pass 17，empty 2 | 0 |
  | `rvm-check-fused/all` | 15 | pass 10，fail 3，empty 1，no-baseline 1 | 66 |
  | `rvm-check-fused/oldpath-79` | 8 | pass 3，fail 3，empty 1，no-baseline 1 | 66 |
  | `rvm-check-fused/a`、`b`、`big`、`knob`、`knob-lean`、`oldpath` | 1、1、2、1、1、2 | pass | 0 |

- `accept-20261110/7333-zone`：1062 行，`run_id` 没有重复，对照 `units.json` 没有缺的也没有多的，两个上级根各 531 行。
- 字段：`unit` 全是 `zone`，`root_refno`、`root_noun`、`root_name`、`counts`（16 格）、`seconds`（三段）、`gen_peak_private_mb`（113–162）、`gen_ledger`、`e3d_export_by`（全是 `batch`）每行都有。`sesno`、`started_at` 每行都没有：批量行不填这两格，只有页面单根会写。
- 逐行与盘上的 `run.json` 比终态、16 个计数、生成秒数、峰值，没有不同。
- 边跑边落库：跑到第 295 秒前后查库，这一批已有 106 行，当时控制台出到第 102–108 个单元。
- `rollup --batch-id accept-20261110/7333-zone` 与产物目录的 `summary.json` 一致：单元数、终态、16 个计数、生成秒数、导出秒数、峰值。

改动（未提交）：

- `src/rvm_check/batch.rs` 的 `generate_in_child`：子进程退出后再读一次累计峰值。原先循环是「看退没退 → 读峰值 → 睡 0.5 秒」，release 构建下不到 0.5 秒就跑完的单元只在刚起进程时读到过一次，记成 1 MB（第一次小跑 24 个单元里有 6 个）；跑得久的也漏掉最后一个间隔。改后同一批重跑是 114–137 MB，全量 1062 行里没有低于 113 MB 的。
- 同文件新增单测 `peak_private_bytes_are_still_readable_after_the_child_exits`。`cargo test --lib rvm_check`：53 passed。`cargo clippy` 没跑。

发现的问题（只记录，没有改）：

1. 生成段的 0.5 秒轮询。每个单元白等 0.4 秒上下，1062 个单元合计约 420 秒。把 `generate_in_child` 里的定时睡眠换成「等子进程句柄、带超时」就能收回来，不必等阶段 4。库里的 `seconds.generate` 也因此只有 0.5 秒的分辨率。
   **已修（gen-model#124，2026-11-10）。** 同一份基准、`--jobs 1` 前后各跑一遍：生成段记录值合计从 971.6 秒降到 573.2 秒，记录值比子进程实际耗时多出来的部分从 365.8 秒降到 14.5 秒（每单元 0.344 秒 → 0.014 秒），整批墙钟从 979.2 秒降到 577.7 秒；每单元的生成秒数记到毫秒，1062 个单元有 248 个不同取值。详见本节末尾「子进程一退出就接着处理」那一小节。
2. `summary.json` 和库里的导出秒数偏大。单元的 `export.seconds` 是会话耗时按单元均摊后四舍五入到 0.1 秒，再相加就成了 212.4 秒（0.2 × 1062），实测会话是 191.1 秒。
   **已修（gen-model#124，2026-11-10）。** 会话耗时摊给各单元时记到毫秒、各份之和等于会话耗时，汇总相加之后才取一位小数。191.1 秒的会话摊给 1062 个单元，每份 0.179 或 0.180 秒，汇总得 191.1 秒。只有单测，没有带 E3D 实跑。
3. 单会话导出这次成立，但仍是单点。会话没走完时（超时、崩溃、登录失败）1062 个单元会逐个补导，每个 1–2 次登录，没有熔断。这次全量没有触发，也没有演练过失败路径；护栏只在采样脚本里（「E3D 补导」超过 10 行，或会话行里出现 `done=false` / `timeout=true` 就杀掉整次运行）。任务 4 仍然要做，但它不是 ams7333 全量的前提。
4. 第 2 节引用的整根 9088 MB 是 11-07 不走融合、要写 `generated.rvm` 的跑法。整根走 `--fused-compare` 的生成峰值是 2287 MB（11-08 入库的 `rvm-check-fused/all` 也是 2292 MB）。
5. 单元目录里仍有 `ours/elements.json`、`ours/report.json`（任务 8 未做）：1062 个单元目录里基准之外的部分约 390 MB。
6. release 首次构建失败过一次：`manifold-csg-sys` 的构建脚本要从 GitHub 克隆 manifold 源码，本机 git 全局代理指向的本地端口当时没有程序在监听。只对构建进程用环境变量绕开代理后通过，没有改任何配置。debug 构建已有缓存，不受影响。

没做的：

- 导出失败路径（会话超时、崩溃）没有在 ams7333 上演练；`--resume` 分块续跑没有用到。
- 带 E3D 导出的 `--jobs 4` 没有整条实跑；`--jobs` 更大的档没试。
- 九个库的全量（检查点 5）没跑。
- 库里的小跑批次 `accept-20261110/7333-smoke`（24 行）与此前的 `store-check-20261110/7999-zone`（19 行）都是验收数据的重复，留着没删，等用户决定。

产物（都在 `output/rvm-check-zone/accept-20261110/` 下）：

- `7333-zone/`（全量，含 `summary.json`、`summary.txt`）、`7333-site/`（整根）、`7333-smoke/`（小跑）、`7333-zone-j4/`（4 路并发）、`baselines-flat-7333/`（1062 份基准的硬链接）。
- `logs-7333/`：每次运行的 `<名字>.console.log`、`.samples.csv`（每秒一行：父进程、子进程、`des.exe`、可提交余量、整机 CPU、别的重进程个数、已出结果的单元数）、`.measure.json`、`.analysis.json`；`reconcile-7333.json`（对账）；`db-check-7333-zone.json`（入库核对）；`db/`（各条查询的应答原文）。
- 脚本：`run-sampled.ps1`（采样与护栏，不加 `--no-store`、不删产物目录）、`analyze-run.ps1`、`db-check.ps1`。
- `bin/aios-database.exe`：这次验收用的 release 二进制（223 MB，可删）。

### 2026-11-10：构件级结果入库（`rvm_check_element`）

用户要求：ZONE 节点或某个 refno 指定的节点核对完之后，它下面每个几何体都要有核对信息存进数据库表，不只是 SITE / ZONE 这些上层；可以用 refno 做主键。

做成了：一个节点核对完，它下面每个带几何的构件在新表 `rvm_check_element` 里各有一行，通过的也有，记录 id 由 refno 得出。判定没有变：ams7333 全量 1062 份 `judgement.json` 与上一小节那次逐字节相同；三个不干净的根的差异行与 11-08 那批逐行相同；运行行没有多出字段。表的字段、键和查询写在 `docs/guides/rvm-check-batch.md` 的「构件级结果」，这里只记做法、改动和实测。

**做法。**

- **refno 怎么来。** 先核实了一件事：比对结果里的 `Pair.refno` 不是 refno，是生成侧的成员路径（`/GENERATED/<显示名>`）。RVM 里没有 refno，配对靠名称。所以 refno 分两头接：
  - 生成侧：vendor 每发出一个成员就记一条 `MemberSource`（成员路径、显示名、出自哪个元素的 refno 和名词；隐式管身记属主的 refno、管号、两端的构件）。比对完按成员路径接回去。
  - 只在基准里的：`resolve_member_names` 按 E3D 的显示名逆着解。命名元素查库的 NAME 表；`BOX 2 of EQUIPMENT /E` 先解属主，再在它的成员里数同名词的第 2 个；`TUBE n of BRANCH /B` 记成 BRAN 名下的派生件。解不出来的照样留一行，标 `refno_resolved = false`。
- **键。** 有自己 refno 的是 `<w0>_<w1>`（与 `pe` 的键同一写法）。隐式管身是 `<属主>:TUBI:<管号>`，只在基准侧的那根是 `…:TUBI:E<n>`；吊架的轴线管是 `<HANG>:TUBI:1`；容器管身是 `<属主>:TUBES` 与 `…:TUBES:R<n>`。没解出来的是 `<单元>~<判档>~<显示名>`，同一件每次得同一个键。
- **归在谁名下。** 每行带 `unit_refno`、`unit_kind`、`root_refno`。`--scope` 和页面单根只核对了单元里一块时，行仍归到它所在的结构单元名下（页面用 `locate_unit`，与批量划分单元同一条规则），另记 `scope_kind`、`scope_refno`。
- **覆盖与清理。** 按 `finished_unix_ms` 只留最后完成的那一次。单元核对完，清掉记在它名下、更早、这次没再出现的行；整根核对清这个根下的全部旧行；只核对一块时只清同一范围上次留下的。`--resume` 补进来的老结果，如果那个单元已经有更晚的一次核对，构件行整个不写。没比对出结果的运行（生成失败、缺基准、超时）不动这张表。
- **子进程怎么交给父进程。** 生成即比对时，子进程把构件级结果写成作业目录下的 `element-checks.json`（紧凑 JSON，每件约 0.5 KB），父进程读进来交给入库线程后就放掉；`--resume` 补入库也读它。不走融合时生成侧另落 `ours/member-sources.json`，比对那一步按它接回 refno。
- **写库。** 每 1000 行一个事务，构件行先写，运行行最后写（运行行在，就说明这次的构件行是全的）。行在入库线程里按块现造现写；排队的行数超过 5 万时，交结果的一方等一等，所以父进程里攒着的行数有界。

**改动（未提交）。** 基线：gen-model `0890e488d`，vendor `42661d6`。

- vendor `src/rvm_export.rs`：`MemberSource`、`DerivedMember`；`RvmExportOutcome.member_sources`；`RvmEmit::push_member`（四处发成员的地方都走它）；`resolve_member_names` 与 `NameResolver`。新代码没有沿 owner 链上溯，护栏不用登记。
- vendor `src/rvm_compare.rs`：`CompareReport` 加两格不序列化的成员路径（`only_generated_members`、`unkeyable_generated_members`），`compare.json` 不变；`noun4`、`parse_default_display_name` 放开到 crate 内。
- gen-model `src/rvm_check/element.rs`（新）：判档常量、`ElementCheck`、`element_checks`（把比对报告铺成一件一行）、`resolve_baseline_side`、`counts_of`、`element-checks.json` 与 `member-sources.json` 的读写。
- gen-model `src/rvm_check/store.rs`：建表与 8 个索引；`ElementRow`、`ElementFacts`、`ElementRowStream`、`element_keys`；`persist_elements_on`、`StaleSweep`、`newer_run_on`；入库线程的排队上限；`ElementFilter`、`list_elements_on`、`element_grades_on`、`Query::Elements` / `Query::ElementGrades`。
- gen-model `src/rvm_check/mod.rs`：`compare_rvms_with_elements`、`element_checks_of`、`baseline_member_sources`、`page_element_facts`、`locate_unit`；融合时子进程写 `element-checks.json`。
- gen-model `src/rvm_check/batch.rs`：作业结果带构件行，交入库后放掉；`resumed_facts` 读 `element-checks.json`；不走融合的 `compare_files`；`run.json` 多 `element_checks`（行数）与 `scope`；收尾多打一行入库用时。
- gen-model `src/rvm_check/report.rs`：把「一件为什么没过」的归因抽成函数，差异行与构件行共用，所以两张表里同一件的 `reason_kind` 和 `reason` 逐字相同。
- gen-model `src/main.rs`、`src/web_service/rvm_check.rs`、`src/web_service/mod.rs`：`rvm-check-query elements` / `element-grades` 与六个过滤参数；`GET /api/v1/rvm-check/elements`、`/element-grades`。
- `docs/guides/rvm-check-batch.md`：入库与查询一节补上第三张表和两种查询。

**测试。** gen-model `cargo test --lib rvm_check`：69 passed、1 ignored（原来 53）。那 1 个要本机 AMS 库文件，带 `--ignored` 单独跑过，通过：`ams_page_nodes_are_filed_under_the_units_enumerated_here` 在真树上拿 ams1112、7333、7999、8000 的 11 个上级根、1 个容器层、27 个单元、单元之下 83 个节点，对页面的归属规则与批量划分的单元，全部一致。vendor `cargo test --lib`：504 passed、5 ignored（开工前 503）；`cargo check --bins` 通过（`gen_ams` 是库外唯一用到导出结果的地方）。vendor 的 owner 链护栏仍然只红在 `key_for`、`scope_anchor` 两处（开工前就红），属性与引用两个护栏通过。`cargo clippy` 没跑。`sigmap review-pr` 只看已提交或已暂存的改动，这次的改动两样都不是，它审不到。

**真实数据。** 都用 `--baseline-dir`，不碰 E3D，写进 `ws://localhost:8009` 那台库，批次号是 `element-20261110/<名字>`。下表各行最后一遍用的都是最终的 release 二进制（`aios-database 0.1.44`，17:02 构建）；只有「两个整根」那一行是 16:54 的前一版。「对账」指每个单元各档的构件行数与它运行行里的 `counts` 逐格相比。

| 跑法 | 结果 | 构件行 | 对账 |
|---|---|---|---|
| ams1112 的 3 个 ZONE（`--unit zone`） | pass 3，5135 / 5132（另有 reference_artifact 2、film 1） | 5135 | 3 / 3 一致 |
| 三个不干净的整根（`--fused-compare`）：ams8000 `/1RX03-EQUI`、`/1CSV-HVACHB`，ams7999 `/1WCC-PIPEBJ` | fail 3；计数与 11-08 相同，差异行 65 行（40、21、4）与 11-08 那批逐行相同 | 6288，其中没通过 65 | 3 / 3 一致 |
| ams8000 的两个根，不走融合（页面单根用的就是这条比对路径） | pass 1、fail 1 | 664 | `/1CSV-HVACHB` 的 `element-checks.json`（150 行）与融合那次逐字节相同 |
| `--unit zone --only 17496/8516 --scope subtree:17496/105799` | pass，113 / 112 | 113，带 `scope_*`；这个 ZONE 另外 1775 行没动 | — |
| `--scope node:24383/66459`、`--scope subtree:24383/66458`（都在 `/1WCC-PIPEBJ` 下） | fail | 2、3 | — |
| ams7333 的 1062 个 ZONE，`--jobs 4` | pass 1062，159,876 / 159,876 | 159,876，不多不少 | 1062 / 1062 一致 |
| ams7333 的两个整根 | pass 2 | 159,876（原地覆盖上一行写的） | 2 / 2 一致 |

- 范围核对那三次的行：`subtree:17496/105799` 的 113 行都带 `scope_kind = subtree`、`scope_refno = 17496/105799`，记在 ZONE `17496/8516` 名下，这个 ZONE 另外 1775 行仍属于按 ZONE 跑的那一批。`node:24383/66459` 的 2 行是那个 BRAN 名下的两根管身；`subtree:24383/66458` 的 3 行多一个 CAP。之后整单元、整根各再跑一遍，这些行被收回，范围两格跟着消失。
- 没过的 65 行构件与 65 行差异逐条对过：原因种类、严重度、原因原文、轴差、两侧图元和包围盒都相同。
- 运行行对过：`accept-20261110/7333-zone`（加表之前的二进制写的）与这次的 1062 行，字段名集合相同（32 个），17 格判定相关的内容逐行相同。
- refno 抽查，拿正在运行的服务的树接口（`/api/v1/tree/ancestors`、`/children`）对名词、名称、属主下的序号：ams7333 抽 60 行对上 60；ams7333 的派生件抽 20 行对上 20；不干净的三个根抽 30 行对上 30；管身派生件抽 25 行对上 25。
- 基准侧逆解的对拍（草稿库，把基准换成别的单元的，让全部构件都落到「只在基准」）：ams1112 `/1RS-WF04` 1446 件全部解出，与真 refno 逐个相同；ams7999 `/1WCC-PIPEBJ` 4118 件解出 4117，与真值相同，剩下那 1 件库里已经没有了。
- 现在表里 171,813 行：ams1112 5135，ams7333 159,876，ams7999 4118，ams8000 2684。通过 171,748，没通过 65。
- 没有自己 refno 的派生件 2074 行：ams7333 1058（全是吊架的轴线管），ams7999 895、ams8000 121（隐式管身）。
- 只在基准里的 3 件：1 件解回属主（`TUBE 2 of BRANCH /1WCC1135/B1`，键 `24383_66459:TUBI:E2`）；2 件没解出（`WELD 1 of BRANCH /1WCC1136/B1`、`BOX 1 of EQUIPMENT /1-LNR-Y302-PJ`），照样各存了一行。
- 陈旧行清理、老结果不盖新结果、`--resume` 补入库、`--in-process` 四样在草稿库（另起的一台 SurrealDB，用完已停掉并删除）上拿真实数据演练过：换掉基准再核对，清掉 1446 行；整根跑完再按 ZONE 跑，行的归属跟着换；补入一份老结果时提示「已经有更晚的一次核对」，构件行没动；正常 `--resume` 从 `element-checks.json` 补进 1446 行，再跑一遍什么都不写。

**规模（ams7333，`--jobs 4`，基准取上一小节导出的）。**

| 跑法 | 墙钟 | 写库合计（其中构件行） | 跑完后还等了 | 父进程私有内存 | 子进程峰值 |
|---|---|---|---|---|---|
| 1062 个 ZONE，`--no-store`（同一二进制的对照） | 284.2 s | — | — | 第 100 个单元时 106.4 MB，末了 109.3 MB，峰值 109.4 MB | 150.5 MB |
| 1062 个 ZONE，第一次写 | 284.6 s | 45.3 s（42.4 s，约每秒 3800 行） | 0.0 s | 114.9 → 120.3 MB，峰值 120.5 MB | 142.9 MB |
| 1062 个 ZONE，最终二进制，原地覆盖 | 281.2 s | 103.1 s（99.6 s） | 0.1 s | 114.5 → 117.1 MB，第 400 个单元之后持平，峰值 121.9 MB | 148.4 MB |
| 两个整根，原地覆盖（行按块现造之前的二进制） | 109.1 s | 70.1 s | 33.9 s | 峰值 317.4 MB | 2323 MB |
| 两个整根，最终二进制，草稿库，第一次写 | 92.0 s | 45.4 s | 24.2 s | 峰值 253.6 MB，两个根之间回到 109 MB | 2324 MB |
| 只补入库（`--resume`，1062 个单元都已跑完），草稿库 | 45.9 s | 40.8 s（38.6 s） | 12.8 s | 峰值 182.6 MB | — |

- 按 ZONE 跑时入库不拖长整批：带不带入库墙钟一样，写库在另一个线程里边跑边写，跑完时已经写完。
- 父进程带入库比不带高 8–11 MB，不随单元数涨（最终二进制第 400 个单元之后每 100 个单元 0.19 MB，不带入库是 0.21 MB）。
- 与上一小节旧二进制的 271.0 秒比，三次都多 10–13 秒（约 4%）。这几次运行期间别的会话一直在编译（最多同时 30 个 `rustc`），没有分清是干扰，还是子进程多做的事（记成员来源、写 `element-checks.json`）。
- `element-checks.json`：1062 个单元合计 77.6 MB，最大 630 KB；整根一份约 40 MB。

**发现。**

1. 基准比库旧时会出现解不出 refno 的行。上面没解出的两件都是这样：11-08 导出基准之后，`/1WCC1136/B1` 这个 BRAN 下只剩一个 CAP，`/1-LNR-Y302-PJ` 这台设备在库里查不到了。它们本来就是「只在基准」的差异，行留着，只是没有 refno。
2. 覆盖已有的行比第一次写慢一倍多（159,876 行 99.6 秒对 42.4 秒）：每块要先按键删旧行再插入。
3. 整根 8 万件一次交过来，写库落后于生成，批量结束时要多等 24–34 秒；父进程在这期间到 254 MB，写完回到 109 MB。按 ZONE 跑没有这个问题。
4. `--scope` 的运行在 `rvm_check_run` 里不记范围（原来就是这样），看起来像一次计数很小的整单元运行。构件行里有 `scope_kind`、`scope_refno`。
5. 16:09 本机虚拟内存耗尽过一次。16:09:16 Windows 记了资源耗尽事件，占用最多的是三个 `rustc`：8.7 GB（这边在编 lib 测试）、4.8 GB 和 2.8 GB（别的会话）。16:09:21 两个 `rustc` 和 8009 那台 SurrealDB 以 0xc0000409 崩溃退出。16:15 用仓库的 `scripts/Start-Surreal8009.ps1` 原样拉起（同一个二进制、同一个数据目录、同样监听 127.0.0.1:8009）；`rvm_check_run` 原有的 1164 行、`rvm_check_mismatch` 原有的 132 行都在，后端 `/api/v1/health` 正常，别的表没有逐张核对。后端和前端两个进程没有动。之后这边的编译都套了一层看门狗（可提交余量低于 20 GB 不开工，低于 6 GB 就杀掉自己）。

**没做的。**

- 页面单根没有端到端实跑：它要 E3D 当场导出，而且正在运行的服务是旧二进制，不能重启。它的三段分别验过：比对路径（不走融合）在批量里实跑过，结果与融合逐字节相同；归属规则在真树上对过（见「测试」）；入库与批量是同一个函数。没验的是把这三段接起来的那几行，以及 E3D 导出之后的整条链。
- 两个新 HTTP 接口没有起服务实测。它们与命令行共用 `store::answer_on`，后者有单测并实测过；要用到得重启服务。
- 加这张表之前跑的产物目录没有 `element-checks.json`，`--resume` 补不了构件行，要重跑。
- 阶段 3 要去掉落盘回读，`element-checks.json` 是新多出来的一份落盘；阶段 4 改成一个上级根一个子进程时，它要跟着换成会话内交付。
- 只在一侧的件（`only_*`、`unkeyable_*`）没有包围盒和轴差：比对报告里它们只有名字。要补得改 vendor 的比对报告。
- 这次往库里写的验证批次留着没删，等用户决定：`element-20261110/` 下 9 个批次，运行行 2137 行，差异行 91 行。构件行现在挂在 `7333-zone-final`（159,876）、`fail-roots`（6288）、`1112-zone`（5135）、`file-compare-8000`（514）四个批次名下；另外五个批次（`7333-zone`、`7333-site`、三个范围批次）的构件行已被后来的运行覆盖，只剩运行行和差异行。

**产物**（都在 `output/rvm-check-zone/element-20261110/` 下）：各批次的产物目录；`logs/`（测试与构建日志、各小批的控制台、对账与抽查结果、`scope-final-evidence.txt`、`scratch/` 草稿库各场景的控制台、8009 重启日志）；`logs-7333/`（每次运行的控制台、每秒采样、`measure.json`、对账与抽查）；脚本 `reconcile.ps1`（对账）、`spot-check.ps1`（refno 抽查）、`guarded-cargo.ps1`（带看门狗的 cargo）；`bin/aios-database.exe`（这次用的 release 二进制，可删）。草稿库、它的产物目录和 `--no-store` 那一遍的产物目录已删，日志留着。

### 2026-11-10：批量「比对失败」的原因种类（gen-model#123）

**现象。** 批量核对里比对这一步没跑完的记录（终态 `compare-failed`），入库后那一行 `stage_failure` 的原因种类一律是兜底的 `run_failed`，看不出是 panic、主键重复还是别的。

**原因。** 同一个阶段有三种写法。批量侧（`batch.rs` 的 `run_facts`）给这类记录写的阶段名是 `compare_failed`；归类侧（`store.rs` 的 `classify_failure`）认的是 `compare` 和 `compare-failed`；页面单根、库里已有的行、前端的阶段标签用的都是 `compare`。

**改法。** 阶段名只留 `compare` 一种。库里 `rvm_check_run.stage` 的取值仍是原来那四个：`resolve` / `generate_ours` / `e3d_export` / `compare`。

- 选 `compare` 的理由：库里 3301 行运行行的阶段只有 `compare`（3299 行）和 `e3d_export`（2 行）；前端的阶段标签表只有这四个名字，运行详情里「停在」那一格直接显示得出；`compare-failed` 本来是终态的名字，不拿来兼作阶段。
- 停在比对这一步的有两种情形，阶段名相同，靠终态分。`fail` 是比对跑完判红，差异在清单里。`compare-failed` 是比对没跑完，留一行 `stage_failure`，原因种类按报错文本分五种：`compare_panic`、`compare_duplicate_key`、`compare_duplicate_name`、`compare_unknown_bbox_frame`、`compare_failed`。
- 四个阶段名只有 `mod.rs` 里那一份常量。页面单根、批量的「终态 → 阶段」、归类侧三处都引用它，不再各写各的字面量。

**改动（未提交）。**

- `src/rvm_check/mod.rs`：四个 `STAGE_*` 常量加说明，值没变。
- `src/rvm_check/batch.rs`：新增 `status::stage`（终态 → 阶段）；`RootRecord::run_facts` 改用它，`compare-failed` 的阶段从 `compare_failed` 变成 `compare`；新测试 `every_stage_the_batch_writes_is_one_the_classifier_knows`。
- `src/rvm_check/store.rs`：`classify_failure` 的阶段名改引用常量，比对那一支不再认 `compare-failed`；`build_rows` 按终态认出「比对没跑完」；测试 `failure_kinds_follow_stage_and_message` 补了几条，新测试 `a_compare_that_did_not_finish_is_told_apart_from_one_that_paired_nothing`。
- `docs/guides/rvm-check-batch.md`：运行行的 `stage` 列出取值；原因种类表补上比对失败的五种、`compare_nothing_paired`、`run_failed`。

**测试。** `cargo test --lib rvm_check`：71 passed、1 ignored（原来 69）。新测试拦不拦得住，各试了一次：把批量侧的阶段名改回 `compare_failed`，红在「阶段名不在那四个里」；把归类侧那一支改成只认 `compare-failed`，红在「落库后阶段没被认出来」。两处都已改回。

**实跑。** 批次 `element-20261110/compare-failed-123`，读文件比对，写进正式库。两个根各给一份故意弄坏的基准（放在产物目录的 `scratch-123/baselines-broken/`，原件没动）：

| 根 | 基准怎么弄坏的 | 终态 / 阶段 | 原因种类 |
|---|---|---|---|
| ams8000 `=24384/24932` | 把一个构件改成与另一个同名 | `compare-failed` / `compare` | `compare_duplicate_key` |
| ams8000 `=24384/46` | 截掉后一半 | `compare-failed` / `compare` | `compare_failed` |

构件表没有动，前后都是 171,813 行。另外三种（panic、E3D 名称重复、bbox 帧认不出来）没有造出真实数据，只有单测。

**库里的旧行。** 改之前查过，没有受影响的行：3301 行运行行里没有终态 `compare-failed` 的，阶段没有 `compare_failed`，差异行里没有 `run_failed`。

**前端。** 不用跟着改。阶段标签、比对失败五个原因种类的标签、`run_failed` 的兜底标签都已经有。`rvmStoreFormat.ts` 里 `stageFailureClass` 的注释和 `rvmStoreFormat.test.ts` 里那条说明还写着「批量的 compare-failed 落库后原因种类是笼统的 run_failed」，已经过时；它「先看终态」的逻辑仍然对。

**发现的同类问题（没改，等用户定）。** 四条都用一条临时测试当场取过证，测试已删，输出在 `logs/issue-123/probe-sibling-kinds.log`。

1. 缺基准（终态 `no-baseline`）的原因种类是 `e3d_export_failed`，不是指南里列的 `baseline_missing`。库里有 2 行（11-08 的 `rvm-check-fused/all` 和 `oldpath-79`，同一个根 `=24384/26547`）。
2. 批量的生成超时（终态 `timeout`）记成 `generate_failed`，不是 `generate_timeout`。库里没有这样的行。
3. 页面单根的比对没跑完（例如比对时 panic）记成 `compare_nothing_paired`，与「跑完却一件都没配上」分不开。
4. 生成即比对时，比对段的报错（基准装载失败、主键重复一类）出在生成子进程里，终态是 `gen-failed`，原因种类是 `generate_failed`。按 ZONE 跑走的就是这条路。

前两条同一个原因：`classify_failure` 里按终态写的那几支（`no-baseline`、`timeout` 等）走不到，因为 `build_rows` 传进去的是阶段名，而批量超时的报错里没有「超时」两个字。让 `build_rows` 先按终态认、再按阶段认，两条一起好。第 3 条要页面侧把「比对没跑完」传给入库。第 4 条要么让归类侧在生成阶段也认比对器那三句话，要么让子进程告诉父进程败在哪一段。

### 2026-11-10：子进程一退出就接着处理，导出秒数对齐会话（gen-model#124）

**现象。** 父进程盯生成子进程的循环是「看退没退 → 采样 → 睡一个采样间隔」，生成段的间隔是 0.5 秒。子进程退出后，父进程要睡完剩下的那一截才发现。按 ZONE 跑时一个单元只有半秒上下，每个单元白等 0.3–0.4 秒，记下来的生成秒数也只有 0.5 / 1.0 / 1.5 几档。

**改法。**

- 采样循环不再定时睡，改成等在子进程的句柄上，最多等一个采样间隔（`wait_for_exit`，Windows 上是 `WaitForSingleObject`）。到点照常醒来采样；子进程先退出就立刻醒。采样的节拍（生成段 0.5 秒、比对段 20 毫秒）、超时判定、内存护栏、退出后补读峰值这几处的代码都没有动。
- 生成段的耗时记到毫秒（`StageRecord::to_the_millisecond`）。别的段照旧记到 0.1 秒。
- 一次导出会话的耗时摊给各单元时记到毫秒，各份之和等于会话耗时（`session_shares`）；补导的单元在自己的补导耗时上加这一份。
- 汇总把各单元的秒数相加之后才取一位小数，`summary.json` 与库里的 `rollup` 用同一个算法（`store::total_seconds`）。

**改动（未提交）。**

- `src/rvm_check/batch.rs`：`generate_in_child` 的循环把 `std::thread::sleep` 换成 `wait_for_exit`，生成段记录改记到毫秒；`generate_in_process` 同样记到毫秒；新增 `wait_for_exit`、`StageRecord::to_the_millisecond`、`round3`、`session_shares`；`export_all` 改用 `session_shares`；`Summary::build` 的六个合计改用 `store::total_seconds`。四条新测试。
- `src/rvm_check/store.rs`：新增 `total_seconds`，`rollup_on` 改用它；一条新测试。
- `docs/guides/rvm-check-batch.md`：生成那一步的说明、`seconds` 的说明。

**测试。** `cargo test --lib rvm_check`：76 passed、1 ignored（#123 之后是 71）。

- `the_parent_moves_on_as_soon_as_the_generation_child_exits`：整段走一遍 `generate_in_child`，拿测试程序自己当子进程（它不认那些参数，报错即退）。把循环改回定时睡试过一次，它红了，五次都是 0.503 秒；已改回。
- `waiting_on_the_child_returns_the_moment_it_exits`：马上退出的子进程不等满间隔；还在跑的等到点；杀掉之后立刻看得到。
- `peak_private_bytes_are_still_readable_after_the_child_exits`（原有）仍然通过。
- `generation_seconds_are_kept_to_the_millisecond`、`export_seconds_add_up_to_the_session_they_were_shared_from`、`rollup_adds_the_seconds_up_before_rounding_them`：秒数的精度与合计。

**实跑：ams7333 按 ZONE 全量单路。** 1062 个单元，`--jobs 1 --no-store --baseline-dir`，基准用全量验收时导出的那 1062 份。修改前后各跑一遍，两份 release 二进制只差 #124 的改动（修改前那份已含 #122 与 #123）。「实际耗时」按文件时间戳算，与全量验收同一个办法：`gen.stdout.log` 建立到 `judgement.json` 写完。

| | 修改前 | 修改后 |
|---|---|---|
| 整批墙钟 | 979.2 s | 577.7 s |
| 生成段记录值合计 | 971.6 s | 573.2 s |
| 子进程实际耗时合计 | 605.8 s | 558.7 s |
| 记录值比实际多出来的 | 365.8 s（每单元 0.344 s） | 14.5 s（每单元 0.014 s，最大 0.044 s） |
| 每单元记录值 | 0.5 s 217 个、1.0 s 809 个、1.5 s 34 个，另有 1.1 s、2.0 s 各 1 个 | 最小 0.420 s，中位 0.527 s，最大 1.109 s；248 个不同取值 |
| 终态与计数 | pass 1062，159,876 / 159,876 | 相同；1062 份 `judgement.json`、`element-checks.json` 逐字节相同 |

- 记录值与实际耗时现在只差每单元 14 毫秒上下，是起子进程、子进程收尾退出、读回报错日志这几步。
- 两次的子进程实际耗时合计差 47 秒，不是这次改动带来的：别的会话在编译，修改前那一遍有 10% 的采样点上同时有 4 个以上 `rustc`，165 个单元比修改后那一遍慢 0.1 秒以上，合计多出 51.6 秒。中位数只差 0.012 秒（0.526 → 0.514）。
- 全量验收时的旧数（1000.5 秒 / 582.7 秒）是加构件级入库之前的二进制、带 E3D 当场导出跑的，与这里的修改前那一遍不是同一份二进制、同一种跑法，只作参照。

**实跑：`--jobs 4`，写进正式库。** 批次 `element-20261110/7333-zone-j4-after124`。对照是 #122 那一版在同样参数下的 `element-20261110/7333-zone-final`。

| | `7333-zone-final`（修改前） | `7333-zone-j4-after124`（修改后） |
|---|---|---|
| 整批墙钟 | 281.2 s | 182.7 s |
| 生成段墙钟 | 276.0 s | 178.2 s |
| 生成段记录值合计 | 1093.2 s | 709.5 s |
| 子进程实际耗时合计 | 754.2 s | 693.0 s |
| 记录值比实际多出来的 | 339.0 s | 16.5 s |
| 写库 | 103.1 s，跑完后等 0.1 s | 92.4 s，跑完后等 0.1 s |

- 终态、16 个计数、构件行数逐单元相同：pass 1062，159,876 / 159,876，构件行 159,876。1062 份 `judgement.json` 与 `element-checks.json` 逐字节相同。
- 库里对账：1062 个单元各档的构件行数与运行行的 `counts` 逐格一致。
- 库里这一批的 `seconds.generate` 有 305 个不同取值，没有一个落在 0.5 的整数倍上；逐行与盘上的 `run.json` 相同。`rollup` 两个上级根的生成秒数是 346.8 与 362.7，相加 709.5，与 `summary.json` 相同。
- ams7333 的 159,876 行构件行现在挂在这一批名下。`7333-zone-final` 名下的被它覆盖了（只留最后完成的那一次），内容没有变。

**别的路径。** 都是 `--no-store`，修改前后两份二进制各跑。

| 跑法 | 修改前 | 修改后 |
|---|---|---|
| ams7333 两个整根，生成即比对，各跑三遍 | 墙钟 65.3 / 47.4 / 52.6 s；每根生成 23.0–32.5 s；峰值 2309–2328 MB | 墙钟 54.7 / 46.3 / 62.1 s；每根生成 22.1–31.6 s；峰值 2315–2326 MB |
| 超时：`--gen-timeout-secs 3` 跑一个要 30 秒的整根 | `timeout`，3.1 s | `timeout`，3.141 s |
| 内存护栏：`--min-commit-free-gb 4000` | `commit-guard`，第一次采样就杀 | 相同 |

- 整根的耗时每遍都在跳，前后两份二进制的范围重叠，看不出差别。父进程自己用掉的 CPU，修改前 0.9–1.2 秒，修改后 0.9–1.4 秒。
- 读文件比对（ams8000 两个根）与小库按 ZONE（ams1112 三个 ZONE）用修改后的二进制各跑一遍，终态、计数、构件行数与 #122 那一版相同，`element-checks.json` 逐字节相同。
- 比对段的密采样靠子进程落的阶段标记文件触发。现在的生成即比对不落这两个文件，修改前后都走不到这条路，只有单测。

**没做的。**

- 导出秒数那一条没有带 E3D 实跑，只有单测。
- 每个单元起一次子进程的开销还在：子进程实际耗时平均 0.53 秒，其中大半是起进程、读配置与 schema、绑定 MDB、开库。这要等阶段 4。
- 没有补导出与比对两段的精度：不走会话的导出（取现成基准）和生成即比对的比对段每单元只有一两毫秒，照旧记成 0.0。

**库里新增（#123 与 #124 合计）。** 两个批次，等用户决定留不留：`element-20261110/compare-failed-123`（运行行 2 行、差异行 2 行，没有构件行）；`element-20261110/7333-zone-j4-after124`（运行行 1062 行，没有差异行，构件行 159,876 行是覆盖原有的）。现在 `rvm_check_run` 4365 行，`rvm_check_mismatch` 225 行，`rvm_check_element` 仍是 171,813 行。单路那两遍用的是 `--no-store`，没有进库。

**产物**（`output/rvm-check-zone/element-20261110/` 下）：`7333-zone-j1-before124/`、`7333-zone-j1-after124/`、`7333-zone-j4-after124/`；`logs-124/`（三次运行的控制台、每秒采样、`measure.json`、`analysis.json`，逐单元比对与对账的结果）；`logs/issue-123/`、`logs/issue-124/`（测试、构建、各条探针的输出）；脚本 `analyze-124.ps1`（记录值对实际耗时）、`compare-runs.ps1`（两次运行逐单元比）、`probe-run.ps1`；`bin-123/`、`bin-124/`（修改前后两份 release 二进制）；`patches/`（三张快照与按 issue 分开的补丁）。
