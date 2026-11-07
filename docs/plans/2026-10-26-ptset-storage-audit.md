# `pe_ptset`（管件 P 点表）现行存储方式审核 · 2026-10-26

只读审核，全部结论带 `file:line` 出处（本仓 `refactor` @ `83aa3c250` 之后，主 worktree 现被另一会话改着 5 个文件，本审核不碰 `src/`）。触发这次审核的是 plant-ui #2 那条链路：**模型面只落网格与世界矩阵，旧 `inst_info.ptset` 随烘焙网格退役后库里没有 P 点**，于是 SurrealQL 侧的长度函数没数据可算。

## 1. 它现在是怎么存的

| 维度 | 现行做法 | 出处 |
|---|---|---|
| 表 / 行 | 派生表 `pe_ptset`，**一个管件一行**：`{ pe, root, dbnum, noun, ptset: { "<P 号>": { pt:[x,y,z], dir:[x,y,z], bore, noun } }, count, unresolved, written_at }` | `src/fast_model/ptset_backfill.rs:10`、`resource/surreal/gy_common.surql:4` |
| 键 | `pe_ptset:⟨word0_word1⟩`，**与 `pe` 同键**（`refno_key` 与 `model_db_adapter::refno_key` 同式） | `ptset_backfill.rs:39`、`:621` |
| 坐标参考系 | 管件**局部系** mm；长度函数只做点距与夹角，与参考系无关 ⇒ **不落世界矩阵** | `ptset_backfill.rs:13-14` |
| 写时机 | 只有两条：① 生成根拿到 **durable ACK** 之后，去抖 2 s 按根重算；② 进程启动补扫。都**不进模型行的发布事务**（ADR-071 的块与回执一个字节不改） | `ptset_backfill.rs:16-24`、`model_write_behind.rs:999`、`src/lib.rs:995-997` |
| 写形状 | 一根一笔事务：`DELETE pe_ptset WHERE root = 根` + 逐件 `UPSERT`；`render_clear` 走同一条 DELETE；写包在 `publication_transaction` + `retry_surreal_write_operation` 里 | `ptset_backfill.rs:391-455`、`:585`、`:589` |
| 索引 | 运行时 `DEFINE INDEX IF NOT EXISTS pe_ptset_root ON TABLE pe_ptset FIELDS root`，worker 每次起来补一次（幂等）；**定义失败只 warn，清行退化成全表扫** | `ptset_backfill.rs:346-350`、`:605` |
| 表 DDL | `gy_common.surql` 里**只有注释与函数**，没有 `DEFINE TABLE` / 字段约束 ⇒ 行形状靠写入方自觉 | `gy_common.surql:1-11` |
| 数据源 | `open_set(dbnum)` = `build_set(dbnum, None)`，即**按文件最新会话**开一份 `DbSet`，与 HTTP `/element/ptset` 同口径 | `ptset_backfill.rs:456-460` |
| 节流 | `DEBOUNCE=2 s`、每轮 32 根、每根之间 `IDLE_GAP=30 ms`、轮询 500 ms；失败退避 30/60/120… s，**连续 5 次放弃**（下次 ACK 或重启补扫再来） | `ptset_backfill.rs:31-45`、`:482-495` |
| 启动补扫 | 等数据面就绪（首启 20 s，每 60 s 再看，最多 10 轮），**只排「库里一行都没有」的 BRAN / HANG 根**；已有行的根不动 ⇒ 重启不重写整库 | `ptset_backfill.rs:210-266`、`:316-345` |
| 读侧 | `fn::pe_ptset_of($pe)` = `select value ptset from only type::thing('pe_ptset', record::id($pe))`；缺行按 NONE 兜（`$arri.pt == none → 0.0`），`bran_comp_len` / `elbo_len` / `tee_len` 各自取 `ARRI` / `LEAV` | `gy_common.surql:8-12`、`:30`、`:58`、`:92` |
| 开关 / 观测 | `AIOS_PTSET_BACKFILL=0` 整块关掉（默认开，不动 `DbOption.toml`）；`/health.model_cache.ptset_backfill` 有 `pending / in_flight / enqueued_total / roots_written / roots_skipped / roots_cleared / rows_written / failures / abandoned / index_ready / last_error / last_success_at / sweep{status,rounds,candidate_roots,covered_roots,enqueued,error}` | `src/options.rs:1936-1940`、`ptset_backfill.rs:78-120`、`:189-210` |
| 边界 | 只处理 `BRAN` / `HANG` 根的直属成员（管件），`EQUI` / `SUPPO` 等跳过；根在文件里已不存在 ⇒ 只清名下行 | `ptset_backfill.rs:25`、`:45`、`:424-434` |

设计本身是自洽的：`src/data_interface/model_refresh.rs:876` 有一条断言专门钉住「body 里不得出现 `model_db_adapter::enqueue_roots_write_behind(`」，即**不许在发布事务里排队**——与 ADR-071 一致。

## 2. 审核意见（按要紧程度）

**A. 陈旧性没有判据，且没有任何路径会为「已有一行但过期」的根重算。**（要紧）
行里只有 `written_at`，**不带 `sesno` / 水位**（`ptset_backfill.rs:10`）；重算只有两个入口，而启动补扫明写「已经有行的根不动」（`:210`）。于是这些情形会让表停在旧值而查询侧看不出来：文件被回退到更早会话（`replay-step --rollback` 那条路，H04 / B06 演过）、`pe` 行单独变化但根没再拿 durable ACK、`unresolved` 的 P 点后来能解了。影响面是**长度类计算**（`bran_comp_len` / `elbo_len` / `tee_len` 静默给出旧数）。最小补法：行里带 `sesno`（+ 一根一笔事务时写入），读侧或巡检比对 `sesno != 库的 file_latest` 就重排；这比"每次重启全库重算"便宜得多。

**B. 验收面是空的：`tests/` 目录 0 命中。**
`git grep` 全仓 `pe_ptset|ptset` 在 `tests/` 下**一条都没有**；`ptset_backfill.rs` 内也没有 `#[test]`。沾到 ptset 的两处测试都不是它的账：`src/test/fork_surreal_compat.rs:1452` 还在读**退役前**的 `out.ptset.d.pt` 形状，`src/test/test_performance.rs:1413/1430` 是 `ptset: None` 占位。⇒ 补写、清行、索引降级、退避放弃四类分支全靠人工。现成可接的口子有两个：`/health` 账（见 D）和回放 harness 的 `log_must_not_contain` 字段（`tests/increment_replay/main.rs:197`，判在 `:1926`）——后者加一条 `"[pe_ptset] 定义 root 索引失败"` 就能把「索引没建上还在全表扫」判红。

**C. 表结构没有落进 `gy_common.surql`，索引是运行时补的。**
`DEFINE TABLE` 与字段约束都没有；`ensure_index()` 每次 worker 起时幂等补（`:605`），失败只 warn 继续（`:350`）。后果：行形状只由写入方一处注释约束；索引能不能用上取决于进程跑没跑到那一步，`index_ready` 只在本进程账里。建议把 `DEFINE TABLE` + `DEFINE INDEX` 落到 `gy_common.surql`（与其他表同一处出厂），代码里的 `ensure_index` 退化成兼容老库的兜底。

**D. 观测很齐，但没有任何东西读它。**
`abandoned` / `failures` / `index_ready` / `last_error` 全在 `/health.model_cache.ptset_backfill` 里（`:189-210`）——这点我核过，比 #72 里「模型侧失败无声反复重跑」的观测面好得多。但没有任何测试、巡检脚本或门去断言「`abandoned == 0`」之类。一条 `nightly` 巡检就能用。（顺带：这套 `DEBOUNCE + 每轮上限 + IDLE_GAP + 指数退避 + 5 次放弃 + 计数进 health` 正是 #72 里 A 案要的现成范式，可以照搬给模型侧生成失败。）

**E. 同一份 P 点有两条供数路径，无对拍。**
HTTP `POST /api/v1/element/ptset` **不走库**，直读 dabacon 文件（`src/web_service/ptset.rs:1-6` 自己写明「这条端点不走库」）；SurrealQL 侧走 `pe_ptset`。⇒ 前端画的和后端算的长度可以不一致，而且不一致的时刻恰好就是 A 里那些陈旧/未补写场景。最小补法：一条 L0/L2 对拍（同一 refno 两条路径逐字段比 `pt` / `dir` / `bore`），或读侧在 `ptset == NONE` 时按 A 的方式回落直读并记一条 warning。

## 3. 本次没做的

纯静态审核，没跑构建也没跑回放（机器被另一会话的 L2 占着）；上面每条建议都只动 `tests/` `scripts/` 或 `resource/surreal/`，`src/` 的改动（A 的 `sesno` 字段）需要拍板后再动。

## 4. B / D 两条已落地（同日 13:3x，只动 `tests/` 与 `scripts/`）

- **`log_must_not_contain` 的射程修好了**：原来只读 `service.stdout_path` 一份（= 最后一次重启之后的 `stdout-restartN.log`），跨重启的行不在射程内——连现成的 `[模型目标核对]` 那一针也一样。现改成扫运行目录里全部 `stdout*.log`（新 `read_service_logs`，`tests/increment_replay/main.rs` 第 337 行起），并带「目录读不到就退回当前那份」的兜底。
- **三条链的末步各加两针**：`ams8000-equi-move-chain` 731 / `ams8000-equi-room-cross` 735 / `db8000-two-delete` 26 的 `log_must_not_contain` 现在是 `["[模型目标核对]", "[pe_ptset] 定义 root 索引失败", "次补写失败，放弃"]`（后两条原文见 `ptset_backfill.rs:350` 与 `:491`）。
- **`scripts/ptset_backfill_check.py`**（144 行，只用标准库）：读 `/health.model_cache.ptset_backfill` 这本账判红——`abandoned>0`、`index_ready=false`、`sweep.status ∈ {failed, gave-up}`、`--wait` 等满仍 `pending`/`in_flight`、以及 `rows_written=0` 而 `candidate_roots>0`（一根都没补上）为 FAIL；`failures` 与 `last_error` 默认只 WARN（瞬时读库失败会自愈，要严就 `--strict-failures`）；`--allow-disabled` 对应 `AIOS_PTSET_BACKFILL=0`；`--json` 给 nightly 收口用。**没做**「每个 BRAN/HANG 根都有一行」的覆盖度核对：那要裸 SurrealQL，而 `POST /api/v1/query` 是固定只读工具契约（`src/web_service/handlers.rs:1332`），不接任意查询——覆盖度要判得在 L2 用例里加断言（下一步）。
- 跑过的检查：`cargo check --offline --test increment_replay --features http_api` → `Finished dev profile in 11.72s`；`cargo test --offline --test increment_cases_registry` → `3 passed; 0 failed`；`python -m py_compile` + `--help` + 故意打空端点（`exit=1`、`[FAIL] 读不到 …`、`VERDICT: FAIL`）。**没跑**一次完整 L2 回放（那两针要在真日志上被扫一遍才算真生效）。

## 5. 存放剖析（位置 / 行形状 / 体积 / 生命周期 / 可观测）

**放在哪**：与模型行、`pe` 同一份 rocksdb 存储（surreal 实例的 `store` 目录），独立表 `pe_ptset`；键 `pe_ptset:⟨word0_word1⟩` 与 `pe` 同键 ⇒ 读侧按主键直取（`fn::pe_ptset_of` = `select value ptset from only type::thing('pe_ptset', record::id($pe))`），O(1)、不靠索引。体积参照：一次 L2 回放（会话 725..728、四步）的 `store` 里光 `000004.log`（WAL）就 **101,578,082 B ≈ 97 MB**，而被回放的 DESI 文件 `ams8000_0001` 只有 **10,823,680 B ≈ 10.3 MB** ⇒ 库里存的是源文件十倍量级的派生数据，`pe_ptset` 是其中一份常驻增量。

**一行装什么**（`render_root` 的 `UPSERT … CONTENT`，`ptset_backfill.rs:514-585`）：`pe`（同键引用）、`root`（所属 BRAN / HANG，清理按它删）、`dbnum`（取自 `member_refno.dbno()` 而不是任务参数 ⇒ 多库同进程不串）、`noun`、`ptset`（`"<P 号>": { pt:[x,y,z], dir:[x,y,z], bore, noun }`）、`count`（点数冗余存着）、`unresolved`（**只有 P 号列表，没有原因**）、`written_at: time::now()`。数字先量化到 1e-6 mm 再用 Rust `Display`（注释原话「再细的位没有工程意义，只会把语句撑长」⇒ 体积被当成一等公民考虑过）。

**体积（按渲染格式推算，非实测）**：单个 P 点的串 `"12": { pt: [-1234.5678, 0, 21.5], dir: [1, 0, 0], bore: 76.2, noun: "SOCK" }` ≈ 90–130 B；行头 ≈ 130–170 B ⇒ **一个单点件 ≈ 0.22–0.30 KB，四点件 ≈ 0.5–0.7 KB**（未压缩，rocksdb 落盘另计压缩）。

**写放大**：每根每次重算都是整根全量重写——`DELETE pe_ptset WHERE root = …`（走 `pe_ptset_root` 索引）+ 每个管件一条 `UPSERT`，一根一笔事务（`publication_transaction` 包起来，外层 `retry_surreal_write_operation`）。三个缓解：`DEBOUNCE = 2 s` 合并同一任务里的反复 ACK；`ROOTS_PER_ROUND = 32` + 每根后 `IDLE_GAP = 30 ms`（注释：「补写是后台活，不跟生成与发布抢库」）；`Skip`（非 BRAN / HANG）与 `Gone`（根已不在文件里 ⇒ 只清行）不产生 UPSERT。

**生命周期（存放上最该警惕的一段）**：只有两个写入口——ACK 后 `enqueue_roots`，与启动补扫（**只排库里零行的根**，`:210`）；`DELETE` 只发生在该根再算一次或该根消失。于是：
1. 没有 TTL、也没有「全量重刷」入口：想重刷整库 P 点只能靠重启补扫，而它跳过已有行的根；
2. `AIOS_PTSET_BACKFILL=0` 关掉后**旧行不会消失**，`fn::pe_ptset_of` 照旧吃到它们（`enabled` 只存在于 `/health`，不在行里）⇒ 「关功能」≠「停供数」，读侧分辨不出；
3. **缺行与「真的没有 P 点」不可区分**：`points.is_empty() && unresolved.is_empty()` 的管件一行都不写（`:541-543`）；`catalogue_point_set` 解不出的整件跳过、只打 debug（`:533-541`）；非有限坐标的点被 `is_finite` 静默丢弃（`:551-553`）⇒ 库里既无 tombstone 也无诊断行，读侧一律 NONE，`elbo_len` / `tee_len` / `bran_comp_len` 回 `0.0`。**「算不出」与「真是零」在存放层是同一个形状**，而这正是 plant-ui #2 立项要治的症状（旧管线退役后长度算 0）——尾巴留在存放上。
4. **等待本身没有出口**：`schedule_startup_sweep` 在 `SWEEP_INITIAL_DELAY = 20 s` 之后进 `while !coordinator.data_ready() { sleep(SWEEP_RETRY) }`（`:223-226`）——**没有超时、没有日志、`sweep.status` 一直停在 `waiting`**，只有读 `/health` 才看得见。L2 冷启动 init 就要 100 s 以上，所以「等不到就绪」在这套环境里是常态而非意外；光看日志分不清「不需要补」与「永远没等到」，`scripts/ptset_backfill_check.py` 能分开（对 `sweep.status` 非 `done`/`disabled` 判 FAIL，正是这一形态）。

**可观测 / 实测缺口**：`/health.model_cache.ptset_backfill` 字段很齐（本轮已给 `scripts/ptset_backfill_check.py` 去读它），但更硬的一条实测证据是：三个 L2 运行目录（`ams8000-equi-fail-inject-20261026-115605`、`ams8000-equi-move-chain-20261026-002905`、`db8000-two-delete-20261026-003632`）的 `stdout*.log` 里 `findstr /S /C:"[pe_ptset]"` **0 命中**（`exit=1`）⇒ 补写在 L2 回放环境里没跑到出账那一步，**行数、单行大小、根覆盖数现在都没有实测值**，上面的体积只是推算。要拿真数：在某条链末步等 `rows_written > 0`（或跑巡检带 `--wait 120`），再把 `rows_written` 与 `pe` 里 BRAN / HANG 直属成员数对上；这条同时把「补写根本没被触发」钉死，并且排除了「只是日志级别没开」（`[startup] stage=inst-anc-backfill event=done elapsed_ms=62` 这类 info 行在里面，共 1057 行 ⇒ info 级是通的；而 `log::info!("[pe_ptset] 启动补扫…")` 在成功路径上无条件打印（`:246-251`，不在 `enqueued > 0` 守卫下）⇒ 补扫一轮都没跑完）。

## 6. 为什么不放回 `inst_info`（决策依据，全部是代码原文）

先纠正一个前提：**「以前放在 `inst_info`」不是 gen-model 放的，是旧管线的遗留形状**。`src/web_service/ptset.rs:5-7` 写得很死——浏览器端 P 点原来只有两条来源：「legacy 模型包里的 `ptsets.parquet`，或旧后端 `:3100` 的 `/api/pdms/ptset/{refno}`（SurrealDB `inst_relate.out.ptset`）。**gen-model-v1 的读透形态两样都没有——投影里只存网格与世界矩阵，`ptset` 从未落库**」。所以「放回 `inst_info`」在这里不是回退，而是**新造一个字段，并要求它的形状与已退役的 `out.ptset.d.pt` 一致**。四条理由说明为什么不划算：

1. **发布事务是字节契约**（`ptset_backfill.rs:16-17`）：「写入时机（「空闲补写」，**不进模型行的发布事务**，ADR-071 的块与回执一个字节不改）」，ADR 文件名即 `ADR-071-the-database-is-the-synchronous-serialization-of-dbnum-task-results.md`。而 P 点来自 catalogue 解算，会整件失败（`:533-541`）、会丢非有限坐标（`:551-553`）⇒ 塞进 `inst_info` 只有两种收场：附属数据的失败拖红模型发布，或允许半写（行存在但点集缺）。派生表把这两种都推给了「可重算的后台账」。
2. **`inst_info` 的读路有「逐字相同」纪律**：`model_record_reader.rs:258-262` 注释——「行集与顺序和原来那一条 OR **逐字相同**（6 根现场对过），读路的游标偏移不变」。往实例行投影里加字段，动的正是这条已被现场对过账的等价性（回放对账、游标偏移、`records` 类断言全在这一层）。
3. **清理是 ADR-088 那类复杂度的来源**：ADR 标题原文 `ADR-088-the-slice-delete-also-replaces-same-root-legacy-rows-that-carry-geometry-pointer ... never-follows-their-pointers`——「按 key 替换携带几何指针的 same-root legacy 行」正是几何数据嵌进行里之后必须付的账。`pe_ptset` 把它换成一条 `DELETE pe_ptset WHERE root = pe:⟨k⟩`（`render_clear:585`）。
4. **读取形状不该塞进热行的投影里**：`staging/mod.rs:87/99` 与 `model_record_reader.rs:268` 都是「每个实例都要读」的路径（`WHERE aabb.d != NONE AND world_trans.d != NONE`），0.22–0.7 KB/件的点集放进去，拾取/剖切/渲染这些**不需要 P 点**的读路会一起把它拉出库。分表后长度函数只在真要算时按主键取一次。

**但这条决策留了个尾巴，值得单开一条意见（F）**：三处老读路仍在 `SELECT out.ptset.d.pt AS pts`（`staging/mod.rs:87`、`:99`，`model_record_reader.rs:268`，另有 `test/fork_surreal_compat.rs:1452`），在新库里它**恒为 NONE**——即同一个"P 点"在库里有两种读取形状，老的那种永远空。要么按 ADR-088 的口径把这些恒空投影清掉，要么让 `pe_ptset` 成为唯一来源并在三处注释指路；现在两边都没写"哪个是权威"。（**措辞修订**：`test/fork_surreal_compat.rs` 是 1:1 对 vendor/rs-core 的**兼容基准**——它自己 `INSERT RELATION INTO inst_relate [{objs}]` 造假数据再跑旧投影形态，那里的 `out.ptset.d.pt` 不是生产读路；`staging/mod.rs:87/99` 与 `model_record_reader.rs:268` 的 `pts` 槽位是「与 legacy 逐字节同形」纪律要求带着的（`gy_common.surql:7` 明说长度函数不再读它们）。所以 F 的准确说法是：**形状留在投影里、生产侧恒 NONE、权威没写死**，不是有人在读退役字段。)（`docs/adr/` 里 `findstr /S /I "ptset"` **0 命中**，本次存放位置的选择没有任何 ADR 留痕——与审核 C「无 `DEFINE TABLE`」是同一个交代缺口。）

## 7. 分表换到了什么（优点清单）与没换到什么

前提先钉住：**分表 ≠ 分库**。`pe_ptset` 与 `pe` 在同一 namespace/db、同一份 rocksdb 里，只多一个表名前缀 ⇒ 物理 IO、压缩、备份粒度都没变（§5 那个 97 MB 的 `store` 就一份）。所以下面条条是**逻辑层面**的收益，不是「数据搬走了」：

1. **失败域隔开**（最大的一条）。`pe` 行由 `UPSERT pe:{id} CONTENT {gen_sur_json(...)}` 写（`ele_ops.rs:1702`、`:1694` 注释「幂等 create-or-replace」），行体来自发布事务的字节契约；P 点来自 catalogue 解算，会整件失败、会丢非有限坐标（`ptset_backfill.rs:533-541`、`:551-553`）。放同一行 ⇒ 附属数据能拖红模型发布；分开 ⇒ 补写失败只进 `failures`/`abandoned` 账，模型行照发。
2. **不必碰「逐字节同形」纪律**。仓库里加字段是有正规口子的——`element_batch::stamp_owner_columns`（计划 §9-6），但它只为 `dbnum`/`sesno` 两格开，且测试断言写死了「CONTENT 本体仍与 **legacy 逐字节同形**——补列只在开头那个 `{` 后面插两格」（`ele_ops.rs:950` 与其上方注释）。把 0.22–0.7 KB/件的点集塞进发布块，等于让这条纪律去背容量与 parity 比对的账；分表则完全不进这条链（`model_refresh.rs:876` 直接断言补写不在发布事务里排队）。
3. **清理是 O(1) 的一条语句**。`DELETE pe_ptset WHERE root = pe:⟨k⟩`（`render_clear:585`）按 `pe_ptset_root` 索引删；嵌进行里就得靠 ADR-088 那套「按 key 替换 same-root legacy 行、绝不跟随指针」的机制，复杂度和误删面都大一个量级。
4. **读侧零成本，且不污染热路径**。键与 `pe` 同键 ⇒ `fn::pe_ptset_of` 是 `select value ptset from only type::thing('pe_ptset', record::id($pe))`（`gy_common.surql:9-12`）——按主键取，不需要 join、不需要索引；而 `staging/mod.rs:87/99`、`model_record_reader.rs:268` 这类「每个实例都要读」的路径不必顺带把点集从库里拉出来。
5. **生命周期与节流可以独立于发布路径调**。`DEBOUNCE 2 s` / `ROOTS_PER_ROUND 32` / `IDLE_GAP 30 ms` / 退避 30-60-120 s / `MAX_ATTEMPTS 5` / `AIOS_PTSET_BACKFILL` 开关 / `/health` 一本账——这些旋钮挂在派生表上是免费的；挂在模型行上每一个都要过发布事务的评审。
6. **老库不用重生成即可用**：启动补扫按 `pe` 里现存的 BRAN/HANG 根回补（注释原话：「根从 `pe` 取而不是常驻投影（重启后的进程投影是空的，根按需才回驻），已经初始化过的老库不必重生成也能查到」）。这是**分表才做得到**的一件事——往 `pe` 加字段的话，旧库的旧行没有那格，只能重跑发布。

**没换到的（对价，按痛感排序）**：
- **同事务同代保证**——旧形状里点集与几何一起落，天然同代；现在靠「ACK 之后重算」，且**行里不带 `sesno`**、启动补扫只补零行根 ⇒ 过期行无法识别（审核 A）。注意可操作性：`sesno` 就在同键的 `pe` 行上（既有写法 `UPDATE pe:{id} SET sesno = {n}`，`ele_ops.rs:1234/1289/1309`），抄进 `pe_ptset` 每行只多 ~12 B。
- **一份行头冗余**：`pe`/`root`/`dbnum`/`noun`/`count` 都在重复 `pe` 已有的信息，约 130–170 B/件；这是「按 root 一条 DELETE」买的价格，值，但要记在账上。
- **schema 与权限没拿到**：分表在别的库里能靠 DDL 约束形状，这里 `resource/surreal/*.surql` 全仓 `DEFINE TABLE` **0 命中**（唯一命中的 DDL 是 `common.surql:12` 给 `pe` 的 `DEFINE INDEX pe_dbnum_noun`），`pe_ptset` 的形状只活在 `gy_common.surql:5` 的注释与 `render_root` 的代码里；`pe_ptset_root` 索引还是运行时 `DEFINE INDEX IF NOT EXISTS` 建、失败只 warn（审核 C）。
- **两份真相并存**：三处 `SELECT out.ptset.d.pt AS pts` 在新库恒 NONE（审核 F），而 `gy_common.surql:7` 已经写了「这里不再读它们」——代码在读，文档说不读。

## 8. 旧数据库兼容：为什么「沿用旧位置」更糟，以及兼容的正解

结论：**兼容要做在「搬」上，不要做在「放」上**。把 P 点放回旧位置（`out.ptset.d.pt`，即挂在 `out` 指向的几何行上）会踩三条硬账：

1. **生命周期不归它管**。几何行是**内容寻址**的产物行，「清理模型」会连它一起删（`mesh_purge.rs:1-3`：「范围的产物行删掉之后，这片范围引用过的内容寻址几何——库里的 `inst_geo` 行与盘上的 `.mesh`——也一起清掉，靠重新生成写回」）。而写回方只有 `pdms_inst::render_inst_geo_upsert` + `e3d_mesh_store::ensure_mesh_file`——**它们只写网格，不写 ptset** ⇒ P 点会随一次右键「清理模型」永久消失（右键「重新生成模型」`force = true` 的先删一律连几何清）。派生表的清理与重建都由 `ptset_backfill` 自己管，不会被别人顺带删。
2. **共享几何会互相踩**。同一几何行被多个实例复用，且**单位几何是全项目共用**（`mesh_purge.rs:14-16`：「全项目所有隐含直管段共用 `inst_geo:⟨2⟩`（`TUBI_GEO_HASH`）/ `⟨1⟩`（`BOXI_GEO_HASH`」）⇒ 按「这个管件」的语义往几何行写点集，会覆盖同型号所有实例；反过来按几何写，又会在「同网格、不同目录 P 点集」时**误去重**（网格哈希相同但 PTSE 不同，目录版本换掉就会撞上）。
3. **身份错配**：`geo_hash` 是网格内容的哈希，P 点不是网格的一部分。把附属语义塞进内容寻址对象，等于让它参与哈希所服务的一切比对（复用、去重、`out.meshed` 判定、ADR-088 的「不跟随指针」）。

旧形状唯一的真优点值得记下来——**按几何去重**（同型号管件只存一份），比 `pe_ptset` 每件一行省。但它抵不过上面三条，而且省下的量在 0.22–0.7 KB/件这个量级上不足以换回生命周期失控。

**兼容的正解（新增意见 G）**：老库里 `inst_relate.out.ptset` 的点集**还在**，而现行补算路径要求能读到 dabacon 文件（catalogue 链 `SPRE → SCOM → PTRE → PTSE`，`web_service/ptset.rs:8-10`）——老库的文件常已归档/不在盘上 ⇒ **在老库上重算根本补不出来**。所以对老库唯一可行的动作是**搬运**：把 `inst_relate.out.ptset` / `out.ptset.d.pt` 的形状归一后写进 `pe_ptset`（不碰 catalogue、不读文件），跑一次就退役那条旧路径；搬运时把老形状里的纯点数组补成 `{pt, dir, bore, noun}` 需要的字段，解不出的记进 `unresolved`（不猜）。这条同时解决三件事：老库长度不再算 0（不必等文件回归）、`pe_ptset` 成为唯一权威（F 收口）、以及给 A 的 `sesno` 一个可填的值（搬运行可以标 `written_at` + 源行 `sesno`）。

## 9. A 的真实成本（更正 §7 里「加个字段」的说法）

`findstr /N /I "sesno" src\fast_model\ptset_backfill.rs` 只命中两行 `tokio::task::` ⇒ **这条链里完全没有代次概念**，A 不是「往行里抄一格」，而是把 `sesno` 引进整条补写路径：

| 位置 | 现状 | A 要做的 |
| --- | --- | --- |
| `:150 enqueue_roots(roots: impl IntoIterator<Item = RefNo>)` | 入参只有 refno | 改成 `(RefNo, source_sesno)` 一类。调用点 `model_write_behind.rs:999` 传的是 `durable_roots`（`:937` 建、`:965` push，**只有根没有代次**），但同一模块的写后条目上本就有 `source_sesno: u32`（`:47`，取自 `receipt.source_sesno` `:56`）⇒ 有现成的格子可传，不用新造 |
| 基准取哪一个 | — | **不能用 `model_sesno`**：`model_write_behind.rs:7` 与 `:1722` 都写明「这个模块只刷根，不盖水位——`model_sesno` 全库只有任务收尾那一笔写」，而补写在 durable ACK 之后就跑、去抖 2 s，那一刻水位可能还没盖 ⇒ 拿它比对会假过期。基准必须是该根自己的 `source_sesno` |
| `:79 pending: BTreeMap<(u32, i64), PendingRoot>` | 队列键 `(dbnum, packed)` | `PendingRoot` 加一格（同根多次入队时以新 `sesno` 覆盖，与 `DEBOUNCE` 的合并语义天然一致） |
| `:407 render_root(&set, root)` | 渲染时拿不到代次 | 多一个入参，写进 `CONTENT` |
| `:238-241` 补扫筛选 `!covered.contains(&packed_refno(*root))` | `covered_roots()`（`:316`）只返回「有行的根集合」 | 改成返回 `根 → sesno`，筛成「库里没有行 **或** 行上的 `sesno` ≠ 该根当前 `sesno`」 |

收益面不变（过期行从此可识别，文件回退与 `pe` 单变两种撞法都能自愈），代价比「12 B/行」大：四处签名/形态改动，且 `covered_roots` 的查询要多选一列。仍不需要动 `gy_common.surql`，也不需要动 ADR-071 的块与回执。**顺序前置**：先做「让补写在 L2 落地」（末步等 `rows_written > 0`），否则 A 改完无法证明生效。 另记一条：A 的这格 `source_sesno` 也顺手把 G 的「搬运行要标什么代次」定了（搬运时写源行的 `sesno`，没有源行就标 `null` 并让补扫视为「可被重算覆盖」）。









