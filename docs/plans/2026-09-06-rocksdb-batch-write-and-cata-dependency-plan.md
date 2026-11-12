# rocksdb 写回 · 元素数据行「批量先删后插」+ 依赖 CATA 按需入库 · 重构方案

- 日期：2026-09-06
- 提出：用户「对于写入到 rocksdb，尽量用批量写入，先提前删除，然后批量写入。不要单独去 upsert 或者 update。
  然后写回到 rocksdb 的数据，也要按需保存依赖的 CATA 数据。」
- 状态：**全文已批准**（2026-09-06 Plannotator 三轮：§1–§7 `approved`（共识 d-27）→ §8 追补一轮批注
  「CATA 需要保存，按 DESI 解析的依赖保存」已吸收进 §1.2 / D5–D8 → 复审 `approved`；Q1–Q9 全按推荐项生效；
  共识记 **d-29**，取代 d-27）。**服务端已实施**（2026-09-06 晚）：P0-3、P6（服务端）、P1、P2、P3、P4、P5 已落地，见 §9；
  **D2 有一处实测后的修正（`pe` 行不删、原地整行替换），见 §9.1**；P0-1/2/4（真库门）留给用户终端；
  **U1（plant-ui）已落地**（2026-09-07，见 §9.2 末行；plant-ui ADR-0025）；
  ADR **ADR-064** 已起草（含 ADR-058 D1 / S3、ADR-060 D4 的修订注记），CONTEXT.md 词条已加。
- 承接（不重复）：
  - ADR-060「模型行 id 就是记录 id；写入只有先删后插，没有 UPDATE」——模型几何行**已经**是本计划要的口径，
    `src/fast_model/model_db_adapter.rs::render_chunk` 是本计划的样板，本计划把同一口径推到数据面。
  - ADR-056（稳态增量窗口直写 rocksdb；顺序纪律 persist → invalidate → reverse_index → 窗口语句批 → finalize）、
    ADR-001 / ADR-021（`applied_sesno` 只承诺数据、水位须有数据支撑）——**一个字不改**。
  - ADR-058 D1 / S3（CATA 不入持久层）——**本计划修订它**（§3 D8）。
  - `docs/plans/2026-09-06-cascade-query-via-dab-ref-tables-plan.md`（P0–P2 已落地：15 个几何引用属性的覆盖表、
    `cata_cascade_window` 文件差分）——本计划的 CATA 依赖闭包复用其属性名单与窗口差分，不另造一份。
- 范围：gen-model `src/data_interface/{ele_ops,increment_pipeline,dbnum_stats,manual_update,cata_closure}.rs`、
  `src/versioned_db/{baseline,database}.rs`、新增 `src/data_interface/{element_batch,tx_limits,cata_dependency}.rs`、
  `src/fast_model/model_db_adapter.rs`（只加一笔依赖事务，不改几何行）、`src/test/fork_surreal_compat.rs`、
  `src/data_interface/direct_window_replay_parity.rs`；`../vendor/e3d-model/src/pipeline.rs`（`GenerationOutcome`
  多带一张目录访问名单，纯输出）。**只覆盖 `store_mode = external`（rocksdb）+ `data_face = ingest` 形态**；
  kv-mem / 读透形态的写透（`read_through.rs`）不在本计划内。

---

## 0. 一句话

数据面写回今天有**两种动词**：基线是 `INSERT IGNORE INTO … [..]` 批，增量是**逐元素** `UPSERT … CONTENT`
外加逐条 `UPDATE`（软删）。本计划把它收成**一种**：一组元素 = 先按 id 批量删掉整组表示（`pe` / `<NOUN>` /
`ATT_UDA` / `pe_owner` 边），再每张表一条 `INSERT IGNORE`，事务按字节 / 行数切块——与 ADR-060 给模型行定的
口径逐字相同；逐元素 UPSERT / UPDATE **从元素数据行上退出**。控制面单行（水位、`gen_root`、队列、统计）
是状态机，留在尾事务点写，边界写死（D1）。同一批写回里，DESI 元素几何链依赖的 CATA 元素
（SPRE → SPCO → CATR / PRTREF → SCOM → GMRE / GMSE / PTSE …）**按需**用同一个批量渲染器入库，
CATA 文件前移时被替换（D5–D7）。

## 1. 现状（2026-09-06 工作树 `e3d-direct@11d101c4` 实测）

### 1.1 写口清单

| # | 写口 | 位置 | 今天的动词 | 归类 / 处置 |
|---|---|---|---|---|
| 1 | 基线 `pe` / `<NOUN>` / `ATT_UDA` / `pe_owner` | `versioned_db/baseline.rs::render_baseline_batches` → `database.rs:877-938` 单写者 | `INSERT IGNORE INTO pe [..]`（经 `dbnum_stats::insert_ignore_batches`）、`INSERT IGNORE INTO <NOUN> [..]`、`INSERT RELATION IGNORE INTO pe_owner [..]` | 已是批量、无删除（靠 IGNORE「已有行赢」给父层补缺用）。**并轨**到统一渲染器 Fresh 模式（P2） |
| 2 | 增量元素终态 | `ele_ops.rs::ElementRows::upsert_statements`（:1579） | 逐元素 `UPSERT pe:{id} CONTENT …; UPSERT <NOUN>:{id} CONTENT …; UPSERT/DELETE ATT_UDA:{id}; DELETE pe:{id}<-pe_owner; INSERT RELATION INTO pe_owner [..]` | **要改**（P1）：批量先删后插 |
| 3 | 增量 `Add` 前置清理 | `increment_pipeline.rs::render_terminal_persist_statements`（:1489） | 每个 Add 三条 `DELETE`（`pe->pe_owner` / `pe_owner:[pe,NONE]..=[pe,..]` / `ATT_UDA`） | 并进批量删除块（P1） |
| 4 | 增量删除 | `ele_ops.rs::PersistElementChange::to_surql` Deleted 分支 | `UPDATE pe:{id} SET deleted = true, sesno = N`（软删，`<NOUN>` / `ATT_UDA` 行原样留下） | **要改**（P1，Q2）：删整组 + `INSERT IGNORE` 墓碑 `pe` 行 |
| 5 | 事务切块 | `dbnum_stats::batches`（:193） | `BATCH_ELEMENTS = 100`，字节 / 行上限借 `staging::executor::{TX_MAX_BYTES = 64 KiB, TX_MAX_WRITE_ROWS = 250}` | 上限是暂存期 `mem://` 口径、常量寄居在 spec 035 P3 要删的目录。**重定并搬家**（P1，Q6）。注：日志里印的 `PERSIST_TX_CHUNK = 500` 早已不是实际切法 |
| 6 | `ref_rev` 反向引用边 | `manual_update.rs::build_reverse_index_statements`（:995） | 每 referrer `DELETE x->ref_rev;` + 每 referrer 一条 `INSERT RELATION INTO ref_rev [..]` | 改成一批一条 `INSERT RELATION IGNORE`（P3）；ADR-063 把 `ref_rev` 降级后可整体退役 |
| 7 | `datacenter_version` 交付状态 | `increment_pipeline.rs::resolve_datacenter_statements_with` | 逐条 `UPDATE datacenter_version:x SET status = …` | 交付记录状态机（控制面），保留 UPDATE；同状态归组成一条多目标 `UPDATE a, b, c SET …`（P3，fork 兼容性在 P0 先验） |
| 8 | OWNER 搬迁 `anc` 修补 / 换 noun 旧表行 | `anc_repair_statements_for_window` / `retired_type_row_statements` | 逐条 UPDATE / DELETE | `anc` 修补是定点 UPDATE（控制性，保留）；旧 noun 表行并进批量删除块（P3） |
| 9 | CATA 按需解析（生产不可达） | `cata_closure.rs::render_content_replacements` | 逐行 `UPSERT {id} CONTENT` | 复活时**不走**这条，走统一渲染器（P4） |
| 10 | 读透写透 | `read_through.rs` → `ElementRows::guarded_upsert_statements` | 会话守卫 + 逐元素 UPSERT | 只在内存形态运行，rocksdb 不走；**不在本计划**，但 P1 把它调用的函数改名以免被当成生产口径 |
| 11 | 模型几何行 | `model_db_adapter.rs::render_chunk`（:1176） | `LET $ids=[..]; DELETE $ids;` → 每表一条 `INSERT IGNORE INTO … [..] RETURN NONE` | **已合规（ADR-060 D4），样板** |
| 12 | `inst_geo` 共享网格描述符 | `model_db_adapter.rs::RowBatch::render`（:1237） | `INSERT INTO inst_geo [..] ON DUPLICATE KEY UPDATE direct_model = …, aabb = …` | 模型面**唯一残留的 UPDATE**，为修正旧描述符而设。一次性归一后降为 `INSERT IGNORE`（P5，Q4） |
| 13 | 控制面单行 | `dbnum_watermark` UPSERT、`gen_root` UPDATE（fence / receipt）、`model_update_pending` UPSERT / DELETE、`spatial_epoch` bump、`dbnum_info_table` UPSERT（pe 统计）、`model_publish_ack` CREATE、`root_dependency` UPSERT、`side_effect_pending` | 单行状态机 | **保留**，但只许出现在尾事务 / 回执块，绝不逐元素（D1） |

### 1.2 CATA 今天在哪（现状描述；**本计划要改掉的正是这一条**）

- **今天的规则（ADR-058 D1 / S3，待修订）**：CATA 不解析进持久层，「任何把 CATA 元素写进 `pe` / `ATT_*` 的路径都是边界破坏」；
  模型面经 `E3dDbResolver` 从文件读。`cata_closure.rs` 的按需闭包（`CataClosureResolver`：全出向 refno + owner 链 +
  容器子树，`db_type` 收口 CATA）代码还在树上，生产路径不可达；唯一活着的 CATA 动作是 `apply_one` 水位之后的
  `SideEffectCompensator::enqueue_cata_ref_rev` → 补偿队列跑 `preload_cata_for_roots`，只为 `ref_rev`。
- **本计划的口径（评审批注 2026-09-06 定稿：「需要保存，按 DESI 解析的依赖的 CATA，按依赖保存」）**：CATA **必须**入库，
  但只入**被本次解析出的 DESI 元素所依赖的那部分**——每一批 DESI 写回（基线、稳态窗口、模型根发布）都以本批 DESI 元素为 seed
  算依赖闭包，把闭包里的 CATA 元素与 DESI 行一起写进 `pe` / `<NOUN>` / `ATT_UDA` / `pe_owner`（D5–D7）。
  「按依赖保存」是硬要求，不是可选优化：一个 DESI 元素落库而它的目录链在库里缺行，算本计划的缺陷（P4 验收门就是这条）。
  ADR-058 D1 / S3 随之修订为「CATA 只按 DESI 依赖入库，不做整库基线」（D8）。
- e3d-model 几何实读的引用属性共 15 个（`vendor/e3d-model/tests/reference_reads_guard.rs`）：
  有反向表 12 个 `SPRE CATR PRTREF PTRE GMRE GSTR PSTR HSTU LSTU POSREF STAREF ENDREF`，单元内 `VXREF`，
  无表跨单元 `HSRO LSRO`。其中指向 CATA 库的是目录链那一族（`SPRE / CATR / PRTREF / PTRE / GMRE / GSTR / PSTR /
  HSTU / LSTU / HSRO / LSRO`）。
- `GenerationOutcome.visited` 只记 DESI 子树访问序，**不记目录侧读了哪些元素**——今天没有任何一份
  「这根几何依赖了哪些 CATA refno」的机器可读答案。

### 1.3 已知数字（供 P0 对照）

- `docs/rocksdb-8000-validation-20260905.md`：ams8000 基线 `pe` 6,605 行、`BRAN` 560、`EQUI` 76；重启前后 152 表
  47,843 行哈希一致。
- 基线单写者（`BASELINE_WRITE_WORKERS = 1`）：SurrealDB 2.1 / RocksDB 乐观事务下并发多行 INSERT 在共享表 / 索引状态上
  持续冲突，7997 真库耗尽重试预算——**批量化不改变单写者**，只减少事务笔数与往返。
- 模型侧切块口径已被真库验证：`PERSIST_CHUNK = 5000` 条、渲染 > 2 MiB 劈半、单块 ≤ 8 MiB、ws 请求超时 30 s 起按体量加到 180 s。

## 2. 目标与非目标

**目标**

- G1 rocksdb 形态下，元素数据行（`pe` / `<NOUN>` / `ATT_UDA` / `pe_owner`、`ref_rev` 边）只有一种写法：**批量先删后插**。
- G2 基线与增量共用**一个**批量渲染器——宪法 II 从「行形状只有一份」扩到「语句形状只有一份」。
- G3 数据面事务切块参数按 rocksdb / ws 口径重定，并从 `staging/` 搬到中立模块。
- G4 写回 DESI 数据时，其几何链依赖的 CATA 元素按需入库；CATA 文件版本前移时已入库的依赖行被替换。
- G5 CATA 依赖闭包只有一份判据，e3d-model 实读集作对拍 oracle（`unexplained = 0`）。
- G6 崩溃重放逐表一致（`direct_window_replay_parity` 继续绿），水位纪律一个字不改。

**非目标**

- 不改模型几何行的写法（ADR-060 已合规）；不动 `gen_root` / 发布 CAS / cohort 语义。
- 不动 kv-mem / 读透形态；不改 `applied_sesno` 语义；CATA 库**不**进 `dbnum_watermark.applied_sesno`。
- 不做 CATA 整库基线（ADR-058「CATA 不做基线」保持）；不在本计划里做 `ref_rev` → dab 反向表的替换（ADR-063 另案）。
- 不引入新的读侧接口；plant-ui 零改动（行形状不变，只多一个 `pe.dependency` 字段，读侧忽略）。

## 3. 设计决策（D1–D8，带推荐项）

### D1 写法口径分两层，边界写死

| 层 | 名单 | 写法 |
|---|---|---|
| 元素数据行 | `pe`、各 `<NOUN>` 表、`ATT_UDA`、`pe_owner` 边、`ref_rev` 边；模型几何行（已合规） | **批量先删后插**：`LET $ids = […]; DELETE $ids;` → 每表一条 `INSERT IGNORE INTO … [..]`（边表 `INSERT RELATION IGNORE`）。禁止 UPSERT / UPDATE |
| 控制面单行 | `dbnum_watermark`、`gen_root`、`model_update_pending`、`increment_update_attempt`、`dbnum_info_table` / `dbnum_stats_state`、`spatial_epoch`、`model_publish_*`、`root_dependency`、`side_effect_pending`、`datacenter_version`、`cata_dependency_state`（新） | 点写（UPSERT / UPDATE / CREATE / DELETE），**只许出现在尾事务、回执块或独立的控制面事务里**，绝不逐元素 |

护栏：源码钉「`ele_ops.rs` / `element_batch.rs` 生产路径全文不含 `UPSERT ` 与 `UPDATE `」；
「`dbnum_stats::batches` 收到的写块只含 `LET` / `DELETE` / `INSERT`」（渲染期用 `surrealdb::sql::parse` 校验动词，
沿用 `dbnum_stats::write_rows` 已有的 AST 走法）。

### D2 统一批量渲染器 `ElementBatch`（新模块 `src/data_interface/element_batch.rs`）

输入：`Vec<ElementRows>`（行形状不变，仍由 `ElementRows::render` 产出）+ `Vec<Tombstone>` + `WriteMode`。

```text
WriteMode::Fresh    —— 库里还没有这一批（首次基线、父层补缺）：不删，只 INSERT IGNORE（已有行赢，语义与今天基线逐字相同）
WriteMode::Replace  —— 增量 / CATA 版本前移：先删整组表示，再 INSERT IGNORE
```

输出 `(delete_block, insert_block)`，一块的语句顺序：

```sql
-- delete_block（Replace 才有）：被引用的最后删，引用者先删
DELETE pe:⟨A⟩->pe_owner;                              -- 每个 Add 一条：refno 复用留下的旧出向边（今天 #3 的第一条）
DELETE pe_owner:[pe:⟨A⟩, NONE]..=[pe:⟨A⟩, ..];        -- 每个本批元素一条：它名下的子槽位（id 区间删，O(children)，无表扫）
LET $uda_ids  = [ATT_UDA:⟨A⟩, ATT_UDA:⟨B⟩, …];   DELETE $uda_ids;
LET $bran_ids = [BRAN:⟨A⟩, …];                    DELETE $bran_ids;      -- 每个 noun 一组
LET $pe_ids   = [pe:⟨A⟩, pe:⟨B⟩, …];              DELETE $pe_ids;
-- insert_block：被引用的先插，引用者后插
INSERT IGNORE INTO pe      [ {...}, {...} ] RETURN NONE;
INSERT IGNORE INTO BRAN    [ {...} ]         RETURN NONE;                  -- 每个 noun 一条
INSERT IGNORE INTO ATT_UDA [ {...} ]         RETURN NONE;
INSERT RELATION IGNORE INTO pe_owner [ {...}, {...} ] RETURN NONE;
```

- `pe_owner` 的删法沿用今天 #3 已在用的 **id 区间删**（`pe_owner:[owner, NONE]..=[owner, ..]`），不用 `WHERE out IN`
  ——`ele_ops.rs:1536` 的注释已经记录 SurrealDB 2.1 对 `DELETE … WHERE in = …` 走整表扫；模型侧对 `geo_relate`
  用的「先 SELECT 出 id 再 DELETE」也是同一个坑的绕法。
- **换 noun 的 refno**（窗口内 A→B）：删除块按**旧 noun** 删旧表行（今天的 `retired_type_row_statements` 并进来），
  不再单独一条。
- **删除的元素 → 墓碑**（Q2 推荐项）：删整组表示后 `INSERT IGNORE INTO pe [{ id, dbnum, noun, owner, sesno, deleted: true }]`，
  行内容从 `DbSet@S`（窗口基侧，收集器已开着）渲染。`datacenter` 的 Deleted 分支读 `pe.noun / pe.owner`、pe 统计
  `count` 含墓碑（`dbnum_stats` 口径）、ADR-036 成员补删——三处读的都是 `pe` 行，不受影响；`<NOUN>` / `ATT_UDA` 行随删除
  消失（今天软删把它们留在库里，属性面板 / 名称搜索对已删元素本就不该命中）。
- 读透形态用的 `upsert_statements` / `guarded_upsert_statements` **改名** `read_through_statements` /
  `read_through_guarded_statements`，标注「非 rocksdb 口径」，避免下一个人把它当成生产写法照抄。

### D3 事务切块：常量搬家、上限按 rocksdb 重定

新模块 `src/data_interface/tx_limits.rs`（`staging/executor.rs` 的三个常量随 spec 035 P3 一起消失）：

| 常量 | 今天 | 推荐 | 依据 |
|---|---|---|---|
| `DATA_TX_MAX_BYTES` | 64 KiB（暂存期 `mem://` 口径） | **2 MiB** | 模型侧同一台 ws + rocksdb 上「> 2 MiB 劈半、≤ 8 MiB」已被 7997 真库验证；ADR-017 记录的事故是整窗口**单事务**（几十 MiB），不是 2 MiB 级 |
| `DATA_TX_MAX_WRITE_ROWS` | 250 | **5000** | 与 `PERSIST_CHUNK` 同量级 |
| `DATA_TX_ELEMENTS` | 100（`BATCH_ELEMENTS`） | **1000** | 一块的元素数上限；超过任一上限就**劈半**（沿用 `dbnum_stats::batches` 与 `render_chunk` 已有的劈半形状） |

- 三个值进 `DbOption.toml` 可调（`data_tx_max_bytes` / `data_tx_max_write_rows` / `data_tx_elements`），默认取上表；
  `/health` 报生效值。Q6 让用户在「2 MiB 直接上」与「先 1 MiB 保守」之间拍。
- **2026-11-12 补（GitHub #155）**：上表只说了「超过任一上限就劈半」，没说劈到只剩一个元素还超限怎么办——实现是报错，
  而目录里一个属主名下上万个成员是真数据（成员边一名成员一行）。现在这种元素的成员边分几笔写，每笔仍在上限以内；
  见 ADR-064 D1 的补记与 `element_batch::append_oversize_element`。
- `dbnum_stats::render` 的 per-ref0 `$before / $after` 观测块与写块同事务的纪律**不变**；它按 ref0 分组，
  行数上限升高后每块的观测 SELECT 也随之变多，P0 要量一下它占事务的比例。
- 冲突重试沿用 `versioned_db::database::conflict_retry_backoff`（指数 + 抖动）与 `surreal_retry`；写者仍是每进程一条数据链。

### D4 顺序纪律与重放语义不变

`apply_one` 仍是 **persist(批) → invalidate → reverse_index(批) → 窗口语句批 → finalize 尾事务（水位最后）**；
`prepare_attempt` 的 durable 恢复记录、按同一固定区间重放的语义不变。先删后插天然幂等：重放撞上上一轮写了一半的块，
删除块把它清掉、插入块重建，终态收敛——比今天「UPSERT 覆盖 + pe_owner 先删后插」少一种动词、多一个保证
（旧 noun 表行不再有残留路径）。`direct_window_replay_parity`（窗口中途 kill → 重放 → 逐表一致）改钉新渲染器，**必须先绿再删旧渲染器**。

### D5 CATA 依赖闭包：「按需」的定义只有一份

```text
seed(写回批)  = 本批 DESI 元素（Add / Modified 终态；模型根则是 GenerationOutcome.visited）上
                目录链引用属性的出向 refno：SPRE CATR PRTREF PTRE GMRE GSTR PSTR HSTU LSTU HSRO LSRO
                （名单直接取 e3d-model reference_reads_guard.rs 的常量，不手抄；落在非 CATA 库的丢弃）
closure(seed) = 沿 CATA 内部链闭包：SPCO → CATR / PRTREF → SCOM → GMRE / GMSE 成员 → PTSE / PTAX / DTSE …
                + 每个元素的 owner 链到 WORL（UI 树 / anc 要用）
                - 不取容器整子树（SPEC 下全部 SPCO、CATE 下全部 SCOM 都不要）——这是与旧 CataClosureResolver 的唯一差别
```

- 实现复用 `cata_closure.rs::CataClosureResolver`（refno 级 BFS、`CataDbLocator` 定位、`db_type` 收口 CATA）；
  把「容器子树」规则收窄成「几何链所需成员」（GMSE / PTSE / DTSE 的成员整取，其余容器只取被引用的那个）。
- **Oracle**：e3d-model `GenerationOutcome` 新增 `catalogue_visited: Vec<RefNo>`——生成时每一次经 `get_element / get_ref /
  get_ref_array` 解到非本库元素就记一笔（纯输出，不带句柄；ADR-057 B1 合规，B2 白名单追记这一项）。
  对拍测试：对 ams8000 / 7997 每个生成根，`catalogue_visited ⊆ closure(seed(visited))`，`unexplained` 必须为 0
  （与 `increment_planner_parity` 同款纪律）。闭包比实读**宽**是允许的（多备份几行），**窄**就是漏。

### D6 CATA 行怎么写、写在哪

- 行形状与 DESI 完全相同（`pe` / `<NOUN>` / `ATT_UDA` / `pe_owner`），经**同一个** `ElementBatch`；`pe` 行多一个字段
  `dependency: true`，标它是按依赖入库的，不代表该 CATA 库整库在场（读侧不认这个字段也无害）。
- 三个写入点：

| 写入点 | 位置 | 模式 | 与水位的关系 |
|---|---|---|---|
| (a) 稳态数据窗口 | `apply_one`：`persist_latest_main_data` 之前一段 `persist_cata_dependencies` | 首次见到的 refno `Fresh`；CATA 版本前移判为变动的 `Replace`（D7） | **排在 DESI 块之前、水位之前**：缺依赖的窗口不算落地（Q3 推荐）。「CATA 库不在 MDB / 文件不存在」只记 warning 不拦；I/O 错误拦 |
| (b) 基线 | `stream_baseline` 每块收集 seed，文件收尾一次闭包、分块写，排在 `finalize_baseline` 之前 | `Fresh` | 同上 |
| (c) 模型根发布 | `persist_root_on` 第一块之前一笔独立事务 | `Fresh`（幂等，`INSERT IGNORE` 命中即零成本） | 与模型发布事务分开，失败只记 warning、不拦发布（ADR-056 D1：模型面不拦水位，对偶地依赖块也不拦模型）。服务 ADR-056 N7 下 `pe` 零行的库走按需 `ensure` 时，属性面板也有目录属性可读 |

- (c) 用 `GenerationOutcome.catalogue_visited` 直接当 seed 闭包结果（它就是实读集，不必再算闭包）。
- 量级护栏：`cata_dependency_max_rows_per_batch`（默认 50,000）——超过就拆多块，不拒绝；`/health.cata_dependency`
  报每库行数、最近入库 sesno、最近一批耗时。

### D7 CATA 版本前移

- 每个 CATA dbnum 一行控制面记录 `cata_dependency_state:{dbnum} { sesno, rows, updated_at }`——它**不是**水位
  （不进 `dbnum_watermark`，不参与 ADR-021 三条判据），只记「已入库的依赖行是按哪一版文件渲染的」。
- 写回时若该 CATA 文件最新 sesno > 记录 sesno：用 `cata_cascade_window`（09-06 P2 已落地）对 `[记录 sesno, 最新]`
  取变动目录件集合，与 `pe WHERE dbnum = cata AND dependency = true` 求交，交集走 `Replace` 重渲染，其余不动，
  然后推进记录 sesno。被删的目录件按 D2 墓碑处理。
- 变动集合超 20,000（P2 的「只报总数」阈值）时退化为「该库全部依赖行 Replace」，回执点名 `cata_dependency_full_refresh`。

### D8 与 ADR 的关系

- **新 ADR（拟 ADR-064）**：「元素数据行的写法只有批量先删后插；CATA 按依赖闭包入库」。
- **修订** ADR-058 D1 / S3：「CATA 不入持久层」→「CATA 只按依赖闭包入库、由写回路径携带、`pe.dependency = true`；
  整库 CATA 基线仍不做」；实施约束 4 里「D1 的退役面」中 `cata_closure.rs` 的闭包解析器**保留并收窄**，不再整体删。
- **修订** ADR-056 D8 一句：CATA 闭包解析进 Surreal「只服务 `ref_rev` 与 UI」→「服务 `ref_rev`、属性面板与依赖备份」。
- **承接** ADR-060 D4：模型行口径升为全库口径；ADR-060 D2「共享的东西只增不删」不变。
- **不动** ADR-001 / ADR-021 / ADR-025 §7 / ADR-054 / ADR-057（e3d-model 只多一个纯输出字段）。

## 4. 分阶段实施

每阶段收尾 `changelog.md` 一条、`docs/adr/ADR-064` 追记一行。**P1 是唯一改变生产写法的阶段**，P0 不落地之前不开工。

### P0 取证与基线数字（0.5 d）

1. `scripts/Verify-RocksDb8000.ps1` 复跑一次，留 `before-manifest.json` 作 P1/P2 的对照哈希。
2. 记一窗 SAVEWORK（7998 或 8000）的 `timings.report`：persist 耗时、事务笔数、每笔字节数（加一行日志即可）。
3. `fork_surreal_compat.rs` 加 4 条 mem / rocksdb 双跑用例：`LET $ids=[..]; DELETE $ids` 数组删；
   `INSERT RELATION IGNORE INTO pe_owner`；多目标 `UPDATE a, b SET …`（#7 归组要用）；`INSERT IGNORE` 撞旧行保留旧行。
   任何一条在 rocksdb 侧不成立，对应处置改回逐条并在本计划记一笔。
4. 量 ams8000 一窗与整库的 `seed` 数与 `closure` 行数（只算不写），给 D6 的默认护栏一个真实数字。

### P1 批量渲染器 + 增量路切换（2 d）

- 新 `element_batch.rs`（D2）、`tx_limits.rs`（D3）；`dbnum_stats::batches` 的入参从 `(Option<id>, sql)` 改为
  块级 `ElementBatchChunk { ids, delete_block, insert_block }`，劈半逻辑不变。
- `persist_latest_main_data` 改调 `ElementBatch::render(changes, Replace)`；墓碑从 `DbSet@S` 渲染
  （`CollectedWindow` 多带一份 Deleted 元素的 S 侧 `EleData`）；`render_terminal_persist_statements` /
  `retired_type_row_statements` 的删除并进删除块后删除。
- 护栏：`apply_one_has_no_staging_fork` 改钉 `Self::persist_element_batches(`；新增 D1 的两条源码钉；
  `direct_window_replay_parity` 改钉新渲染器并保持绿；`versioned_db/baseline.rs` 的宪法 II 行级测试原样通过。
- 验收门：7998 一窗 SAVEWORK 前后 `pe` / 各 noun 表 / `ATT_UDA` / `pe_owner` 行数与内容哈希 = 旧路径结果
  （P0 留的哈希）；`applied_sesno` 推进相同；事务笔数下降、总耗时不高于 P0。

### P2 基线并轨（1 d）

- `render_baseline_batches` 改调 `ElementBatch::render(rows, Fresh)`；`SenderJsonsData` 的三种行消息合成一种 `Batch(sql)`；
  有界队列、单写者、`record_write_error` 账本不变；父层补缺仍 `Fresh`（已有行赢）。
- 验收门：7997 / 8000 基线 `pe` / noun / edge 行数与 P0 哈希一致；`docs/rocksdb-8000-validation` 的重启验证复跑通过。

### P3 `ref_rev` / `datacenter` / 旧表行批量化（0.5 d）

- `build_reverse_index_statements`：一批 = 所有 referrer 的 `DELETE x->ref_rev` + **一条** `INSERT RELATION IGNORE INTO ref_rev [..]`。
- `datacenter` 同状态归组多目标 UPDATE（P0 验过才做）。
- 验收门：`build_reverse_index_statements_*` 三条测试改断言后绿；`increment_pipeline` 的 datacenter 测试绿。

### P4 CATA 依赖闭包 + 入库（3 d）

- vendor `e3d-model`：`GenerationOutcome.catalogue_visited`（`pipeline.rs`），`reference_reads_guard.rs` 加一条
  「每个读点都记进 `catalogue_visited`」的钉；上游提交、升 rev（ADR-056 约束 7 同款纪律，不带 patch 推 main）。
- gen-model 新 `cata_dependency.rs`：seed 提取（属性名单 `include!` 自 e3d-model 常量或经 e3d-model pub 函数取）、
  `CataClosureResolver` 收窄容器规则、`persist_cata_dependencies`、`cata_dependency_state`。
- 三个写入点接线（D6）；`/health.cata_dependency`、`/dbnums` CATA 行加 `dependency_rows`。
- 护栏：ADR-058 S3 的源码钉**翻过来**——「`pe` 上 `dbnum ∈ CATA` 的行只能由 `cata_dependency.rs` 产出」；
  对拍测试 `catalogue_visited ⊆ closure`，`unexplained = 0`（ams8000 全部生成根 + 7997 抽 200 根）。
- 验收门：ams8000 基线后任选 10 个 BRAN，其 SPRE → SPCO → SCOM → GMRE 链上每个 refno 在 `pe` 有行、
  `pe.dependency = true`、noun 表行字段与 TTY `Q ATT` 一致；plant-ui 属性面板对这些目录件不再空。

### P5 CATA 版本前移 + 收尾（1 d）

- `cata_cascade_window` 接 D7 的 Replace 重渲染；`cata_dependency_full_refresh` 回执。
- `inst_geo` `ON DUPLICATE KEY UPDATE` → 一次性归一 sweep 后降为 `INSERT IGNORE`（Q4 批准才做）。
- ADR-064 定稿、`CONTEXT.md` 新词条（元素批 / 依赖入库 / 依赖状态 / 墓碑行）、ADR-058 / ADR-056 对照注记。
- 最终门：`Verify-RocksDb8000.ps1` 重启验证 + 一次 SAVEWORK 增量 + 一次 CATA 文件前移（ams5052 184→185 那窗现成），
  三段 before / after 哈希与行数进 `docs/rocksdb-8000-batch-write-validation-<date>.md`。

## 5. 验证与验收（总表）

| 门 | 怎么验 | 通过判据 |
|---|---|---|
| 动词纪律 | 源码钉（D1 两条）+ 渲染期 AST 动词校验 | 元素数据行路径零 UPSERT / UPDATE |
| 终态等价 | P0 哈希 vs P1/P2 哈希（`rocksdb8000_evidence.py` 逐表） | 行数与内容哈希一致（`pe.dependency` 新字段除外） |
| 重放收敛 | `direct_window_replay_parity`：中途 kill → 重放 | 逐表一致 |
| 双引擎 | `fork_surreal_compat` 新增 4 条 | mem 与 rocksdb 结果相同 |
| 依赖闭包 | `catalogue_visited ⊆ closure` 对拍 | `unexplained = 0` |
| 持久性 | `Verify-RocksDb8000.ps1` 重启前后 | 152+ 表哈希一致 |
| 性能 | P0 vs P5 同一窗 `timings.report` | 事务笔数下降；总耗时 ≤ P0；单笔 ≤ `DATA_TX_MAX_BYTES` |

## 6. 风险与取舍

- **事务变大 → 冲突面与 ws 报文变大。** 用模型侧已验证的 2 MiB / 劈半口径，不是新数字；数据链仍单写者。
  真撞上 `read or write conflict`，退避重试已在（`conflict_retry_backoff`）。
- **墓碑改写删掉 `<NOUN>` / `ATT_UDA` 行。** 读者只有搜索与属性面板会碰已删元素，它们本就不该命中；
  但这是行为变化，Q2 要拍板。回退路径：`Tombstone::KeepNounRow` 一个开关即可保留旧行为。
- **CATA 依赖入库的量级未知。** P0 先量再定护栏；最坏情形（一个 DESI 库引用了大半个目录）也是有界的
  （目录件数 × 链长），且 `INSERT IGNORE` 命中后零成本。
- **ADR-058 反向修订的文档面**：D1 / S3 / 实施约束 4 三处要改，`CONTEXT.md`「按需解析元件库」一章从 retired
  改回「收窄后在用」。
- **e3d-model vendor 变更**要走上游提交，排期上 P4 依赖它先落。
- **`ref_rev` 批量化可能白做**：ADR-063 若在 P3 之前落地，#6 直接退役。P3 只花半天，接受。

## 7. 待拍板（Q1–Q6）

| # | 问题 | 推荐 |
|---|---|---|
| Q1 | 控制面单行（水位 / `gen_root` / 队列 / 统计 / `datacenter_version`）保留 UPSERT / UPDATE 点写，只限尾事务与回执块？ | **保留**。它们是状态机，不是「一批行」；改成先删后插会让 CAS 守卫失去读写同边界 |
| Q2 | 删除元素改为「删整组 + `INSERT IGNORE` 墓碑 `pe` 行」，还是维持单条 `UPDATE pe SET deleted = true`？ | **墓碑**。唯一残留的逐元素 UPDATE 就此退出；noun / UDA 行随删除消失 |
| Q3 | CATA 依赖块在数据窗口里排在水位之前、失败拦水位？ | **拦**（与 DESI 块同纪律）；「库不在 MDB / 文件缺失」只 warning 不拦 |
| Q4 | `inst_geo` 的 `ON DUPLICATE KEY UPDATE` 一次性归一后降为 `INSERT IGNORE`？ | **降**。模型面从此零 UPDATE |
| Q5 | CATA 依赖的 owner 链整链入库到 WORL？容器子树只取几何链成员？ | **是 / 是**。UI 树与 `anc` 要 owner 链；整子树是旧闭包过宽的地方 |
| Q6 | 数据面事务上限直接取 2 MiB / 5000 行 / 1000 元素，还是先 1 MiB 保守？ | **2 MiB**，P0 实测后若单笔超 3 s 再降 |

## 8. 二轮追补（2026-09-06）：读侧按 dbnum 就绪度选源——未初始化走 API 供数，已初始化以 rocksdb 为准

> 用户补述：「对于 dbnum 没有初始化过的模型，客户端在查询模型时，如果 rocksdb 里还没有准备好，则需要使用 API
> 提供数据给 plant-ui 去加载。如果已经初始化过了，则使用 rocksdb 的查询接口为准。」本节只加不改 §1–§7。

### 8.1 现状（实测）

| # | 事实 | 位置 |
|---|---|---|
| a | `POST /api/v1/model/records` 的取数源**按进程介质**选：`store_mode().is_durable()` → `model-database`（`model_record_reader::records_of_root(&SUL_DB)` 读 `inst_relate` / `tubi_relate`），否则 `model-memory`（`ModelMemoryStore`）。**没有 dbnum 粒度** | `web_service/handlers.rs:2279` |
| b | `POST /api/v1/model/ensure` 在摄入形态下：`generate_roots_report` 先 `model_publish_queue::wait_for_dbnum(dbnum)`——该库的初始化发布 run 还活着就**原地等**；miss 走 `initialization_phase::require_model_generation()`，启动全量模型在飞（`model_in_flight`）时**直接报错**；命中后 `persist_root` 落 rocksdb 再回 | `model_refresh.rs:129`、`on_demand_model.rs:274`、`initialization_phase.rs:607` |
| c | `ensure` 的「已可用」判据在摄入形态下读库（`generation_root_cache_current` 查 `gen_root`、`instance_counts` 查 `inst_relate`），direct 读模式再叠一层 `memory_root_current`（投影里有回执） | `on_demand_model.rs:249-262` |
| d | plant-ui 装模型只有一条路：`ensure_model` → `model_records`（API），**刻意没有 SurrealDB 回退**（「混两个纪元会让一次视口装载内部不一致」）；`plant_ui_data::model_instances_anc`（ws 直连 `query_insts_flat` / `query_insts_slim` / `query_tubi_insts_by_brans` 平表快路）仍在，但主界面已不走它，只有测试与 `scope_probe` 在用 | `plant-ui-app/src/data.rs:862-957`、`plant-ui-app/src/model_update_api.rs:585`、`plant-ui-data/src/lib.rs:105` |
| e | `GET /api/v1/dbnums` 每行已有 `initialized` / `applied_sesno` / `model_sesno` / `model_verdict`（`dbnum_consistency` 三态），**没有「这个库的模型该从哪儿读」**这一格 | `handlers.rs:1906-1929` |
| f | 网格不依赖 rocksdb：生成期 `ensure_mesh_file` 就把 `<mesh id>.mesh` 落盘，浏览器口径 `GET /api/v1/meshes/{hash}.glb` 现场转码 | `e3d_mesh_store.rs:80`、`web_service/mesh_glb.rs` |

一句话：**一个 dbnum 正在初始化（基线在写、或初始化发布 run 在飞）的那段时间，plant-ui 对它要么等、要么拿到错误**，
而进程内本来就能从文件把它的模型算出来。

### 8.2 决策（R1–R6）

| # | 决策点 | 结论 |
|---|---|---|
| R1 | 「已初始化」的判据 | **一个谓词、一处实现**（挂在 `dbnum_consistency`，新列 `model_source`）：`Database` ⇔ `applied_sesno > 0` ∧ `!model_publish_queue::blocks_dbnum(dbnum)` ∧（该库 `model_publish_run.status = 'done'` ∨ 库里已有安定 `gen_root` 行）；其余一律 `Memory`。读者（`/dbnums`、`/model/records`、`/model/ensure`、`/health`、ops 页）只消费它，不自己拼（同 ADR-060 M8 的纪律） |
| R2 | `model_source = Memory` 时 `/model/ensure` 怎么走 | **不等初始化队列、不进 durable 队列、不落 rocksdb**：直接从文件生成进 `ModelMemoryStore`（ADR-054：显式 sesno 或文件最新；若该库有活着的初始化 `RunManifest`，钉 **同一个** `source_sesno`，让按需投影与初始化发布逐字相同，`commit_prepared_frozen_projection` 覆盖时零差异）。`require_model_generation()` 这道门对 Memory 库**不适用**（它挡的是 durable 工作与启动全量，按需内存生成两者都不是）。`.mesh` 照常落盘（内容寻址，初始化发布直接复用）。回执多两格：`model_source: "memory"`、`durable: false` |
| R3 | `model_source = Memory` 时 `/model/records` 怎么走 | 该根所属 dbnum 为 Memory → 从 `ModelMemoryStore` 回（`source: "model-memory"`）；投影里没有这根 → `409 not_generated`（让客户端先 `ensure`，与今天的调用顺序一致）。**同一响应只有一个源**：请求的根跨多个 dbnum 且源不同，按 dbnum 拆页、每页带 `source` 与 `dbnum`，客户端按 dbnum 分桶装（plant-ui 的几何缓存 / 可见性本就按 dbnum 分桶） |
| R4 | `model_source = Database` 时 | **rocksdb 为准，一个字不改**：`/model/records` 走 `model_record_reader`（rocksdb 查询）；`ensure` 照今天（`persist_root` 落库后读库）；plant-ui 的 ws 直连平表快路（`model_instances_anc`）作为已初始化库的**可选**批量读路，见 Q7 |
| R5 | Memory → Database 切换 | 初始化发布 run 收口（`finish_ready_initializations`）那一刻谓词翻面；`/dbnums` 行与 WS `tasks` 主题事件 `model_source_changed {dbnum, source}` 同时露出。**不强制客户端重载**：内存投影与刚发布的行是同一版文件算出来的，下一次 get_work / 范围重载自然读到 rocksdb。相反方向（Database → Memory）**只在** `fast_delete` 回退重建清库时发生，同样出事件 |
| R6 | 与 §3–§4 的关系 | 读侧路由不改任何写法；P6 与 P1–P5 **无依赖**，可并行先做（它解的是用户今天就撞到的等待 / 报错） |

### 8.3 实施（P6 服务端 + U1 plant-ui）

**P6 gen-model（1.5 d）**

1. `dbnum_consistency` 加 `model_source` 列与 `ModelSource::{Memory, Database}`；`/dbnums`、`/health.dbnum_consistency` 露出；
   `append_cold_read_through_dbnums` 那种冷行恒 `memory`。
2. `model_records`：按请求根解 dbnum（`E3dModelService::dbnum_for_roots`）→ 每 dbnum 查谓词 → 分源取数、按 dbnum 拆页；
   `source` 字段保留今天的两个字面值，新增 `dbnum` 与 `model_source`。
3. `model_ensure` / `generate_roots_report`：Memory 库分支——跳过 `wait_for_dbnum`、跳过 `require_model_generation`、
   跳过 `ensure_regen_pending_current` 与 `persist_root`；生成用 `E3dModelService::from_frozen_source(初始化 RunManifest 的源)`
   或 `from_current()`；回执 `model_source` / `durable`。Database 库分支零改动。
4. WS 事件 `model_source_changed`，发点两处：`finish_ready_initializations` 收口后、`fast_delete::wipe_dbnum_for_reinit` 之后。
5. 护栏：源码钉「`model_records` / `model_ensure` 体内不含 `store_mode().is_durable()`」（介质判定退出读侧路由，只剩谓词）；
   `mem://` 行为测试：同一根在 Memory 库上 `ensure → records` 回 `model-memory` 且 rocksdb 零行、`model_update_pending` 零行；
   翻面后同一根回 `model-database`。
6. 文档：`docs/specs/web-service-api.md` §4.5 / §4.7 加字段；新增 §4.12「模型取数源路由」。

**U1 plant-ui（1 d）**

1. `DbnumReport` 行加 `model_source`；`ModelRecordsPage` 加 `source` / `dbnum`；`ModelRecordUnion::extend_scope` 按 dbnum 分桶，
   同一 dbnum 只接受一个源（混源直接报错，沿用「不混纪元」的纪律）。
2. 任务队列 / ops 面板：库行上并排显示「模型来源：内存（初始化中）/ 数据库」；收到 `model_source_changed` 只刷这一格，不重载场景。
3. Q7 选 (b) 时：Database 库的批量读切到 `plant_ui_data::model_instances_anc`，Memory 库仍走 API。

### 8.4 验收门

| 门 | 怎么验 | 通过判据 |
|---|---|---|
| 初始化期可读 | ams8000 从空库起，基线 + 初始化发布 run 在飞时 `POST /model/ensure` + `/model/records` 一个 BRAN | 5 s 内回 `model-memory`，rocksdb `inst_relate` 对该库仍为 0 行或只有 run 写的行，`model_update_pending` 无按需行 |
| 翻面 | run `done` 后同一请求 | 回 `model-database`，记录集与翻面前逐条相等（`GeomInstQuery` JSON 序列化对比） |
| 不混源 | 请求跨一个 Memory 库与一个 Database 库的两根 | 两页、各自 `source`，客户端分桶装载 |
| 无回归 | Database 库上 `ensure` / `records` 的回执与 P0 前一致 | 字段只增不改 |

### 8.5 待拍板（Q7–Q9）

| # | 问题 | 推荐 |
|---|---|---|
| Q7 | 已初始化库的批量读走哪条：(a) 仍经 `/model/records`（服务端按谓词选 rocksdb）；(b) plant-ui 直连 ws 平表快路 `model_instances_anc`，Memory 库才走 API | **(a)**：客户端一条路、谓词只在服务端一处、不混纪元；(b) 快（53k 行 0.7 s vs 10.7 s 那档是解引用差异，API 读的也是平表副本）但把「谁初始化过」的判断复制到客户端。若你更看重首屏速度选 (b)，服务端谓词照样通过 `/dbnums` 提供 |
| Q8 | Memory 库上的按需生成要不要**顺手**落 rocksdb（`persist_root`，Fresh 模式） | **不落**。初始化发布 run 正在用同一版文件按 `INSERT IGNORE` 写同一批行，再来一个写者只会撞乐观事务；按需只负责「现在就能看」，持久由 run 负责 |
| Q9 | Memory 库上 `ensure` 生成时点：钉初始化 run 的 `source_sesno`，还是文件最新 | **钉 run 的 `source_sesno`**（有 run 时）；没有 run（库尚未开始初始化）取文件最新。两个投影同版才能在翻面时零差异 |

## 9. 实施记录（2026-09-06）

### 9.1 D2 修正：`pe` 行不删，一条批量 `INSERT … ON DUPLICATE KEY UPDATE <全部字段>` 原地整行替换

**现场**：`direct_window_replay_parity::production_window_replay_converges_from_every_crash_point`（元素上限压到 1、
每个元素自成一块）——干净跑完 EQUI 名下就少了 `reused` 那条 `pe_owner` 边，整窗口重放后两条边全没。
`fork_surreal_compat::dual_pe_rows_replace_in_place_and_keep_their_edges` 的对照断言在 mem 与 rocksdb fork
上都复现：**`DELETE pe:X` 会把挂在 X 上的边一并删掉**（`pe` 是图顶点，`pe_owner` / `ref_rev` / `tubi_relate` /
房间边都以它为端点）。同一块里「先删 pe 再插」没事；可 X 的 owner 边是 **owner 的块**重插的、X 的 `ref_rev`
入边由**别的 referrer** 的窗口维护、`tubi_relate` 由**模型面**写——它们不在 X 的块里，X 的 `pe` 一删，这些边就
静默消失，重生成之前 Plant UI 看不到那根管。这不是块边界能修的：`tubi_relate` 根本不属于数据窗口。

**处置**：`pe` 行按同一 id **原地整行替换**——一块一条 `INSERT INTO pe [..] ON DUPLICATE KEY UPDATE name = $input.name, …`，
字段清单 = `SPdmsElement::gen_sur_json` 写出的全部字段（`element_batch::PE_REPLACE_ASSIGNMENTS`，测试钉「清单覆盖
每个渲出字段」）；语义 = 整行覆盖，id 与边都在；墓碑走同一条（`deleted = true`）。**仍然没有逐元素的 UPSERT / UPDATE**；
其余三类行（noun 行、UDA 行、`pe_owner` 边）没有边挂着，照 D2 先删后插。`Fresh`（基线）不受影响，仍是 `INSERT IGNORE`。
D1 的护栏相应改为「不许 UPSERT、不许逐记录 `UPDATE <表>:<id>`；唯一放行的 UPDATE 是 `pe` 那条批量原地替换」。

**顺带修正**：删除块里 Add 的「自己的旧出向边」清理（`DELETE pe:X->pe_owner`）与墓碑的同款动的是**别的元素**名下的边，
新 owner 的块若先跑会被它删掉刚插的新边——单独成**前置块**排在所有元素块之前（今天的 `render_terminal_persist_statements`
把全部清理排最前面，是同一条纪律）。

### 9.2 已落地

| 阶段 | 落地 | 验证 |
|---|---|---|
| P0-3 | `fork_surreal_compat` 加 5 条 mem / rocksdb 双跑：`DELETE $array`、`INSERT RELATION IGNORE` 撞 id 保留旧行且边是图边、多目标 `UPDATE a, b SET`、整块先删后插幂等、`pe` 原地替换保边（含「删顶点会级联删边」的对照） | `AIOS_COMPAT_REQUIRE=1 cargo test --lib fork_surreal_compat::dual_` 16 passed（含既有 11 条；真起 `bin/surreal.exe` rocksdb） |
| P6 | `data_interface::model_read_route`（谓词 / 任务级只写内存作用域 / 翻面通告）；`model_ensure` / `model_records` 按 dbnum 路由；`/dbnums` 行加 `model_source` / `model_source_reason`；`model_refresh` 与 `on_demand_model` 的三道门对 Memory 库不适用；`model_publish_queue::initialization_source`；`finish_ready_initializations` / `wipe_dbnum_for_reinit` 发 `model_source_changed`；spec §4.12 | 源码钉 `model_read_handlers_route_by_dbnum_not_by_store_medium`；`model_read_route` 6 条单测；handlers 43 passed |
| P1 | 新 `element_batch.rs`（Fresh / Replace、前置块、墓碑、退化墓碑计数）、`tx_limits.rs`（2 MiB / 5000 / 1000，`DbOption` 三键）；`dbnum_stats` 上限改吃 `tx_limits`；`persist_latest_main_data` → `terminal_persist_batches` → `element_batch`；换 noun 的旧表行并进删除块；`ele_ops::PersistElementState::Deleted { base }` + 收集器读基侧渲墓碑；`upsert_statements` 改名 `read_through_statements`（非 rocksdb 口径）；`window_repair` 同源 | `direct_window_replay_parity`：生产路每个事务边界 kill → 重放 → 逐表一致（元素上限 1 / 2 / 1000 三种切法）；`the_production_window_lands_the_expected_shapes`（墓碑保留 noun / owner、noun 与 UDA 行随删除消失、旧 noun 行消失、边终态）；`terminal_production_batches_replay_statistics_before_watermark`；全库 `cargo test --lib --features http_api` 1710 passed / 0 failed |
| P2 | `versioned_db::baseline::render_baseline_batches` → `element_batch` Fresh，每条队列消息 = 一笔事务（`SenderJsonsData::ElementBatch`），`PEJson / AttJson / PERelateJson` 退役；`pe_chunk` / `att_chunk` 不再参与切块 | `baseline` 5 条单测重写（已有行赢、每表一条、边序、按元素上限切块、UDA 行）；`database` 护栏改钉新消息 |
| P3 | `build_reverse_index_statements`：全部 referrer 的邻接 DELETE 在前，整窗口的边攒成 `INSERT RELATION IGNORE`（2000 边一条）；`datacenter_version` 的 Modify 归组成多目标 UPDATE（500 目标一条），Delete 仍逐条（各带 `belong_zone`） | `manual_update` 新测试；`datacenter_tests` 两条改断言；`fixed_target_updates_hit_the_rows_the_server_side_walk_hit` 语义对拍照旧绿 |
| P4 | 新 `cata_dependency.rs`：seed（`CATALOGUE_CHAIN_ATTRIBUTES` 与 e3d-model 护栏名单同源）→ `CataClosureResolver`（precise 容器白名单、只收 CATA、排除本批 DESI）→ `element_batch` Fresh 入库（`pe.dependency = true`）→ `cata_dependency_state:{dbnum}`；三个写入点：`apply_one`（DESI 块之前、拦水位）、基线（同一条写者队列）、模型根发布前（独立、只 warning）；`/dbnums` 加 `dependency_rows` / `dependency_sesno` | `cata_dependency` 9 条单测（含 `mem://` 上重跑不翻倍、三个写入点接线钉、名单同源钉）；`apply_one_has_no_staging_fork` 加「依赖块排在 DESI 块之前」；全库 1718 passed |
| P5 | `cata_dependency::refresh_moved_catalogues`：每个写入点先看目录文件最新是否 > `cata_dependency_state.sesno`；前移则 `cata_cascade_window` 差分变动目录件 ∩ 已入库依赖行（`dependency_rows_among`），被改的重解析闭包经 `element_batch` **Replace** 重渲、被删的落最小墓碑，推进 `sesno`；差分不可用（会话不在链上 / 候选超 20,000）退化为该库全部依赖行 Replace 重渲，回执 `cata_dependency_full_refresh`。`inst_geo` `ON DUPLICATE KEY UPDATE` → `INSERT IGNORE`（Q4）。ADR-064 起草；ADR-058 D1 / S3、ADR-060 D4 加修订注记；CONTEXT.md 加「元素批」「墓碑」「依赖入库」「依赖状态」并把「按需解析元件库」章改为旧章 | `cata_dependency` 加前移单测（改 / 删 / 退化三路）；`CataDependencyReport::warning` 单测；`model_db_adapter` 钉 `inst_geo` 渲 `INSERT IGNORE` 且全文无 `ON DUPLICATE KEY UPDATE`，`…_repairs_legacy` 改钉 `…_existing_rows_win`（手改过的原型行再发布不被修回）；全库 `cargo test --lib --features http_api` 1721 passed / 0 failed；clippy gen-model 零警告 |
| D5 oracle（2026-09-07） | **e3d-io** `read_stats::trace_element_reads()` / `ReadTrace::finish()`：线程局部、可嵌套的「读了哪些元素」记录器，`DbElement::parsed` / `extraction` 读成功即记（缓存命中也算，悬空引用 `NotFound` 不算）；`DbSet::db_type(dbno)`、`DbElement::db_set()`。**e3d-model** `GenerationOutcome::catalogue_visited`：DFS 前开、DFS 后收，过滤「不在根所在库 ∧ 所在库为 CATA」；源码钉 `catalogue_visited_is_traced_around_the_whole_dfs`；真库门 `tests/catalogue_visited_real.rs`（只有目录侧 / BRAN 成员 SPRE→SPCO 一个不漏 / 链走到 SCOM 与点集几何集）。**gen-model** `resolve_dependency_rows_with(locator, …)`（闭包只有一条路，定位器可注入）+ live 对拍 `live_ams8000_catalogue_reads_are_covered_by_the_dependency_closure`；对拍抓到并修掉：`on_demand_db::open_single` 用 `descriptor().noun` 当库类型（真库上所有目录文件被拒）→ `db_type_name()`；`precise()` 名单补正体 SBOX / SCYL / LCYL / SCON / LSNO / LPYR / SCTO / SRTO / SSPH / SSLC / SDSH / SDIS（名下负体）与 `PTSS`（p-line 集，此前误写属性名 `PSTR`）；顺手补 `dbnum_stats::rebuild_on`（`baseline` 测试引用而未定义，lib test 编不过） | e3d-io `cargo test --lib` 234 passed；e3d-model `cargo test --lib` 269 passed、`--test catalogue_visited_real` 1 passed（ams8000：visited 6,120 / catalogue_visited 5,009 / BRAN 成员 SPRE 抽查 2,634 条全中）；gen-model live 对拍 **`unexplained = 0`**（catalogue_visited 5,009 ⊆ closure 13,995，6 个目录库，closure missing 4 = 1 个库不在项目 + 3 个悬空引用）；`precise_config_limits_container_subtree_and_adds_desi` 加钉 |
| U1（plant-ui，2026-09-07） | `plant_ui::task_queue`：`DbnumStatus.model_source` / `model_source_reason`（宽松反序列化，认不出归 `None` 整格不画）、`ModelSource::{Memory, Database}`（`parse` / `from_records_label` / `label`）、`Vm::set_model_source`（翻面只改一格、清理由）、`RowVm.model_source` → 库行类型旁字形 + 悬停整句 + 行内明细首行 + 说明列空着时整句；`model_update_api`：`EnsureReply.model_source` / `source_note`、`ModelRecordsPage.source` / `dbnum`、新 `model_record_union.rs`（按 dbnum 分桶、同库两源报错、同根分页中途换源报错、老服务端无 `dbnum` 整次一桶）、`ModelRecords.sources` / `memory_note`；`model_update_ws::decode_model_source_changed` → `Evt::ModelSourceChanged` → `main.rs` 只改那一格、不在手上就提前一拍轮询、日志一句；`sim.rs` 剧本按同一判据给 `model_source`。Q7 取 (a)：已初始化库仍经 `/model/records`。文档：plant-ui ADR-0025、CHANGELOG「未发布」两节、CONTEXT.md「模型来源」「翻面」 | `cargo check --workspace --tests` 干净（既有 `search_index` dead-code 警告与此无关）；`cargo test -p plant-ui-app -- model_record_union model_update_api model_update_ws` **14 passed**（含分桶 / 拒混源 / 无 dbnum 一桶 / WS 翻面解码四条新钉）；`cargo test -p plant-ui -- task_queue` **40 passed**。未 live（真库门同上） |
| Q2（2026-09-07）① 惰性落库切统一渲染器 | `cata_closure::ensure_cata_refnos_parsed`（`cata_ref_rev` 补偿队列 / resolve 层 miss / staging 预加载共用的惰性兜底）不再自己渲染：闭包走新 `cata_dependency::resolve_catalogue_rows_with`（与三个写入点同一个 `closure_config`：`precise()` 容器白名单、db_type **只收 CATA**、无「本批 DESI 库」可排除），落库走从 `persist_cata_dependencies` 拆出的 `persist_resolved_rows`（D7 前移先 `Replace` 刷新 → `element_batch` `Fresh`、`pe.dependency = true` → `cata_dependency_state`）。删掉 `render_content_replacements` / `render_uda_*` / `INSERT_CHUNK` / `OwnerReplaceBatches` / `UdaReplacementBatches`（`render_pe_owner_replace` 留给 `fork_surreal_compat` 对拍）。**起因**：D5 对拍修好 `on_demand_db` 库类型字之后，这条路在真库上第一次真的写行（plant-1 `run2-forget-cache` 04:52 的日志还是 `missing=705 … noun 0x00000002 is not a database type`），而每个 DESI 窗口收口后补偿队列都会跑它：`UPSERT pe:X CONTENT {gen_sur_json}` 整行覆盖、没有 `dependency` 字段 → P4 落的标记被抹 → P5 `dependency_rows_among` / `all_dependency_refnos` 找不到行、目录前移不再刷新；`precise()` 原样跑还会把回指到别的 DESI 库的元素连 owner 链写进 `pe`。补偿队列本身留着，去留随 ADR-063 | 源码钉 `lazy_fallback_lands_rows_through_cata_dependency_and_renders_no_upsert`（惰性兜底体内走 `resolve_catalogue_rows_with` + `persist_resolved_rows`、无 `UPSERT` / `execute_model_write`；`cata_closure.rs` 生产正文零 `UPSERT`）、`closure_config_collects_only_cata_for_every_entry`；`cargo test --lib --features http_api -- cata_closure:: cata_dependency:: element_batch::` 46 passed；全库与 live 对拍见 changelog |
| Q2（2026-09-07）② `cata_ref_rev` 补偿队列退役 | 用户拍板「直接退役、不等 ADR-063（P4 已在水位前落行）」。`apply_one` 收口后不再 `enqueue_cata_ref_rev`（函数删除）；`SideEffectKind::CataRefRev` 变体只为认得旧库里已排进表的行，drain 到它直接 `Ok(())` 收口（不解析、不写）；`cata_ref_rev_summary` 删除；`/health.side_effect_pending.by_kind` 不再报这一档。**影响评估**：(a) 目录行——P4 三个写入点已按 D5 唯一判据在水位**之前**落行，oracle 证明覆盖全部几何实读；退役后不再落的只有几何链之外的目录引用（ISPEC / TSPEC 那类），属性面板对它们只显 refno——要就把属性名加进 `CATALOGUE_CHAIN_ATTRIBUTES`，不另开路；(b) `ref_rev`——这条路从未写过边（只写 `pe` / noun / UDA / `pe_owner`），DESI→CATA 边在 reverse_index 步、CATA→CATA 边只有 `rebuild_reverse_index`，维护点一个没变（级联计划 §现状 B 行的「CATA→CATA 边靠 cata_ref_rev 补」是写错了，已更正）；(c) `CataDepCache` / `DEFERRED_CATA_CACHE` 失去唯一生产写者，`discard_deferred_cache` 空转，仓内无别的读者——随 ADR-063 / 级联计划 P3 一起清；(d) 每个 DESI 窗口少一条副作用作业、少一遍按根子树全出向引用的闭包重解析。ADR-056 D8 / ADR-034 / ADR-058 约束 4 / spec 035 T126 / 级联计划 P5 范围各加注记 | 源码钉 `cata_ref_rev_is_retired_and_legacy_rows_are_only_settled`（`enqueue_cata_ref_rev(` 在 `side_effect_pending.rs` 与 `increment_pipeline.rs` 生产正文里都不再出现；drain 仍认得 `cata_ref_rev` 且臂内无 `preload_cata_for_roots` / `ensure_cata_refnos_parsed`；`/health` by_kind 无此键）；`side_effect_pending:: increment_pipeline:: cata_closure:: cata_dependency:: element_batch::` 107 passed；全库见 changelog |
| Q2（2026-09-07）③ 那一簇死代码清掉 | 用户拍板「现在就清，不等 ADR-063」。`cata_closure.rs` 删去：根子树播种 `collect_design_subtree_outbound` / `collect_database_subtree_outbound(_on)` / `run_cata_closure_pass_for_refnos`、`ensure_cata_parsed_for_roots`、`try_lazy_cata_fallback`、依赖缓存 `CataDepCache` / `DEFERRED_CATA_CACHE` / `publish_deferred_cache` / `discard_deferred_cache` / `preload_cata_for_roots` / `DependencyCacheContext` / `dependency_manifest_fingerprint` / `require_all_roots_resolved`、env 开关 `cata_closure_enabled`（`AIOS_CATA_CLOSURE_MODE`）、开/关几何冒烟 `GeoSmokeReport` / `geo_smoke_digest`；`bin/cata_smoke.rs` 整个删除（前提就是那个开关）；`batch_worker` 不再 `discard_deferred_cache`；`bin/node_gen_room_probe` 不再要求开关、不再 `CATA-PRELOAD`（目录行由该库数据批的依赖块落）。留下的：定位器与闭包引擎（依赖入库在用）、`ensure_cata_refnos_parsed` 薄壳（`staging::preload` 引用）、`render_pe_owner_replace`（双引擎对拍标准形态）。cata_closure.rs 2 897 → 1 774 行。ADR-004 状态改读「Phase 1 / 2 在用，Phase 3 ～ 6 已退役」；CONTEXT.md 旧章加注 | 删掉只钉旧机制的 8 条测试（dep cache ×4、context、required roots、staging subtree、preload 段）；`lazy_fallback_lands_rows_through_cata_dependency_and_renders_no_upsert` 的边界改到 `#[cfg(test)]`；`cargo check --lib --tests --bins` 干净；`cata_closure:: cata_dependency:: side_effect_pending:: increment_pipeline:: element_batch:: batch_worker::` 145 passed；全库 / clippy / live 对拍见 changelog |

### 9.3 未做与欠账

- **P0-1 / P0-2 / P0-4**（真库基线哈希、一窗计时、CATA 闭包量级）与 P1 / P2 / P4 的**真库验收门**（`Verify-RocksDb8000.ps1` 复跑、
  逐表哈希对照 `output/rocksdb-8000-20260905/before-manifest.json`、任选 10 个 BRAN 查目录链行）本轮没跑——要起 fork surreal +
  `aios-database serve` 跑 ams8000 基线，属长驻进程，按会话协议留给用户在自己的终端里跑；命令见 changelog。
- ~~**D5 的对拍 oracle** 未做~~ → 2026-09-07 已落地，见 §9.2 末行（e3d-io 读记录器 + e3d-model `catalogue_visited` +
  gen-model live 对拍；首次对拍抓到 `on_demand_db` 库类型字读错、`precise()` 名单缺正体与 PTSS 三处，已修，`unexplained = 0`）。
  vendor 两仓（e3d-io / e3d-model）的改动仍在各自工作树上未提交，按 ADR-056 约束 7 走上游提交、升 rev。
- ~~U1（plant-ui）未开工~~ → 2026-09-07 已落地，见 §9.2 末行；§8.4 的四道验收门（初始化期可读 / 翻面 / 不混源 / 无回归）
  仍要真库起来才能过，与真库门一起留给用户终端。
- `PersistElementChange::to_surql` 与 `render_terminal_persist_statements` 已删；兼容渲染器 `render_persist_statements`
  （old-pdms-io 差分口径，诊断 / 测试载体）原样保留。~~旧的按需 CATA 惰性落库（`cata_closure::ensure_cata_refnos_parsed`，
  UPSERT 逐行，只服务 `cata_ref_rev` 补偿队列）没动——它的去留挂在 ADR-063 上，记为 ADR-064 Q2~~ → 2026-09-07 两步都做了
  （§9.2 末两行）：落库切到 `cata_dependency::persist_resolved_rows`（§1.1 #9 的处置「复活时不走这条，走统一渲染器」），
  `cata_ref_rev` 补偿队列退役（用户拍板不等 ADR-063）；~~仍留着但已无生产调用方的 `preload_cata_for_roots` /
  `ensure_cata_parsed_for_roots` / `try_lazy_cata_fallback` / `CataDepCache`~~ → 同日也清掉了（§9.2 ③）。
  还留在树上的只有 `ensure_cata_refnos_parsed` 薄壳，等 spec 035 P3 拆 `staging::preload` 时一起走。

## 附 · 语句形状对照（一个窗口两个 BRAN 被改、一个 PIPE 被删）

**今天**（逐元素，6 + 1 条语句 / 元素，每 100 元素一笔事务）

```sql
UPSERT pe:8000_101 CONTENT {...}; UPSERT BRAN:8000_101 CONTENT {...}; UPSERT ATT_UDA:8000_101 CONTENT {...};
DELETE pe:8000_101<-pe_owner; INSERT RELATION INTO pe_owner [ {...} ];
UPSERT pe:8000_102 CONTENT {...}; UPSERT BRAN:8000_102 CONTENT {...}; DELETE ATT_UDA:8000_102;
DELETE pe:8000_102<-pe_owner; INSERT RELATION INTO pe_owner [ {...} ];
UPDATE pe:8000_77 SET deleted = true, sesno = 286;
```

**本计划**（前置块 + 一块一笔事务，块内每表一条；`pe` 行原地替换、不删——§9.1）

```sql
-- 前置块（单独一笔）：Add / 墓碑自己的旧出向边
DELETE pe:8000_77->pe_owner;
-- 元素块
DELETE pe_owner:[pe:8000_101, NONE]..=[pe:8000_101, ..]; DELETE pe_owner:[pe:8000_102, NONE]..=[pe:8000_102, ..];
DELETE pe_owner:[pe:8000_77, NONE]..=[pe:8000_77, ..];
LET $uda_ids  = [ATT_UDA:8000_101, ATT_UDA:8000_102, ATT_UDA:8000_77]; DELETE $uda_ids;
LET $del_n0   = [type::thing('BRAN','8000_101'), type::thing('BRAN','8000_102')]; DELETE $del_n0;
LET $del_n1   = [type::thing('PIPE','8000_77')];                                  DELETE $del_n1;
INSERT INTO pe [ {id: pe:8000_101, ...}, {id: pe:8000_102, ...}, {id: pe:8000_77, noun: 'PIPE', deleted: true, sesno: 286, ...} ]
  ON DUPLICATE KEY UPDATE name = $input.name, noun = $input.noun, dbnum = $input.dbnum, sesno = $input.sesno,
  status_code = $input.status_code, cata_hash = $input.cata_hash, lock = $input.lock, deleted = $input.deleted,
  refno = $input.refno, owner = $input.owner RETURN NONE;
INSERT IGNORE INTO BRAN    [ {...}, {...} ] RETURN NONE;
INSERT IGNORE INTO ATT_UDA [ {...} ] RETURN NONE;
INSERT RELATION IGNORE INTO pe_owner [ {...}, {...} ] RETURN NONE;
-- 同一块之前的独立事务（D6 (a)）：这两个 BRAN 的 SPRE → SPCO → SCOM → GMRE 链，Fresh 模式
INSERT IGNORE INTO pe   [ {id: pe:5052_9001, dependency: true, ...}, ... ] RETURN NONE;
INSERT IGNORE INTO SPCO [ {...} ] RETURN NONE;  INSERT IGNORE INTO SCOM [ {...} ] RETURN NONE;  INSERT IGNORE INTO GMRE [ {...} ] RETURN NONE;
INSERT RELATION IGNORE INTO pe_owner [ ... ] RETURN NONE;
```
