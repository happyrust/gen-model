# ADR-075：校审数据落在 `SUL_DB` 的同一个 ns·db 上，并且只在 durable 介质上启用校审域

状态：**已接受**（移植计划 Q6「跨重启保留是硬约束；a/b 由工程在 T1 以 ADR 定」已于 2026-09-12 由 Plannotator
批准，本 ADR 是那一格的落地：**取 (a)**）。**号到提交时复核**——本仓 ADR-004 / 006 / 008 / 060 都撞过号。
**2026-09-18 修订（ADR-083）**：判据一个字没动（仍是 `is_durable()`），但改法的说法变了——介质只由环境变量
`AIOS_STORE_MODE` 决定，`DbOption.toml` 的 `store_mode` 不再生效。本文里「配置 / 改成 `store_mode=rocksdb`」
一律读作「以 `AIOS_STORE_MODE=rocksdb` 重启」，「默认 `store_mode=mem`」读作「不设这个环境变量 = `mem`」。
**2026-10 修订（ADR-083 二次修订）**：`store_mode` 恢复生效（向导第②步 external / memory 落盘）。
「改成 `store_mode=rocksdb`」重新可按字面读（向导选 external 即写它），`AIOS_STORE_MODE=rocksdb` 仍是
等价的命令行覆盖；默认档维持 `rocksdb` 不变（不设 = 连不上当场报错，不会落在 mem）。
日期：2026-09-13
计划：`docs/plans/2026-09-12-review-backend-port-plan.md`（§1.4 第 2 条、§2 G3、§6 判据 A4/A6/A9、§7 风险 2/3、Q4/Q6）
落地：T1b —— `src/web_service/review/db.rs`、`src/web_service/review/schema.rs`、`rs_surreal/review/review_tables.surql`、
`src/options.rs`（`[review_auth]` / `[platform_auth]` / `[review_attachments]` 三段 + `PLANT_REVIEW_ALLOW_EPHEMERAL_STORE`）、
`src/web_service/mod.rs`（两条挂载臂 + `/files/review_attachments` 的 ServeDir）。
关联：
- **沿用** 宪法 I「持久层唯一是 SurrealDB」，并按 ADR-074 修订为 `mem` / `rocksdb` 两档：校审不引入第二种存储，
  也不引入第二条 Surreal 连接。
- **沿用** 宪法 III「静默失效是最高级别缺陷」：介质活不过重启时**拒绝启用**整个校审域，而不是先跑起来等人重启一次才发现。
- **不动** ADR-071（库是 dbnum 任务内存结果的同步序列化）：那条讲的是**模型数据面**的写法。校审数据不是任何任务算出来的中间结果，
  它是人在界面上填的一手输入，库对它而言是**唯一真值**而不是序列化副本——两者共用一个 `SUL_DB` 句柄，但不共用那套「内存镜像 + 水位」纪律。

## 背景

旧端 plant-model-gen 的校审持久层是 `src/web_api/review_db.rs`（295 行）：每次操作前 `fresh_review_db()` **新建一条独立 ws 连接**，
外面套 5s / 8s / 30s 三档超时，并用两个 `OnceCell` 懒加载索引与 `review_workflow_history` 的 schema 升级。那条「每次新建连接」不是设计，
是绕障：旧端锁的 SurrealDB fork（github happyrust dev-3.1）的 ws 客户端在高并发下 channel 会堵。它依赖的
`aios_core::{use_ns_db_compat, options::DbConnMode}` 在本仓的 old-aios-core 里**根本不存在**（计划 §1.3 那一行标的「难」就是这个）。

gen-model 这侧只有一个持久层入口：`ensure_store_connected` 连上的 `aios_core::SUL_DB: Surreal<Any>`。ADR-074 合入后介质只有两档
（`src/options.rs` 的 `StoreMode`）：

| 介质 | 活过重启 | 别人连得上 |
| --- | --- | --- |
| `rocksdb`（连已由外部启动的持久库） | 是 | 是 |
| `mem`（进程内嵌 `mem://`） | **否** | 否 |

移植计划 G3 要求校审数据（提资单、确认记录、评论、批注状态、附件元数据、工作流历史）**跨 gen-model 重启保留**。
而模型数据面在内存两档下是**有意**可丢的——它随时能从 e3d 重新解析出来（ADR-071 D7 的「启动水化 = 反序列化」）。
校审数据没有这个来源：一条写进去就没了的提资单，谁也重放不出来。于是同一个 `SUL_DB`，两类数据对「重启即空」的容忍度相反。
Q6 把选择交给工程：(a) 复用 `SUL_DB` 但要求 durable 介质，还是 (b) 校审自己开一条持久连接 / 独立 ns·db。

## 决策

### D1　落点 = `SUL_DB`，同一个 ns·db，不另开连接、不另开库（取 Q6 的 (a)）

`fresh_review_db()` 返回 `aios_core::SUL_DB.clone()`。`Surreal` 是 Arc 句柄，clone 不建连接，也就没有「连接超时」这一类失败；
ns·db 就是 `ensure_store_connected` 已经 `use_ns(surreal_ns) / use_db(mdb_name)` 的那一个，校审表与模型表同库，靠 `review_` 表名前缀分辨。

**为什么不取 (b)。** 独立连接意味着在 gen-model 里再造一份 store 生命周期：`mem` 那一档**没有端口**，只能再连一台外部库；
`rocksdb` 那一档则要多一份地址、多一份凭据、多一份备份策略，而运维在屏幕上看到的「库在哪儿」会有两个答案。
校审数据量小（人手填的单子与批注，不是百万级元素行），同库不构成容量或争用理由。宪法 I 说的是「持久层唯一是 SurrealDB」，
(b) 没有违反字面，但它违反了那条规则想要的东西——**一个库、一份备份、一句「数据在哪儿」**。

(a) 的代价是显式的：**校审域的可用性被 `store_mode` 绑住**。这正是 D2 要让它在启动那一秒就喊出来的原因。

### D2　持久化闸：介质不 durable 就拒绝启用整个校审域

判据只有一条 `StoreMode::is_durable()`——全仓「这一档活不活得过重启」的唯一出处（护栏
`only_rocksdb_is_durable` 一类 StoreMode 护栏）。校审**不另立标准**，否则介质表一改，
`/health` 与校审域就会讲两套话。

`web_service::serve` 在挂路由**之前**判一次（`review::db::review_persistence()`），两条臂：

- **Ready**：挂 `review::router()` 与 `/files/review_attachments` 的 ServeDir，启动日志一行说明介质。
- **Refused**：四个前缀 `/api/review`、`/api/users`、`/api/auth`、`/files/review_attachments` 下的**任意路径、任意方法**
  一律 `503` + `{success:false, code:"review.persistence.refused", message, storeMode}`，业务路由与附件静态服务都不上。

为什么是 503 而不是「干脆不挂」：不挂就是 404，而 404 与「这个后端还没移植这一版接口」长得一模一样，前端只会去怀疑后端地址配错了。

拒绝时打的是**多行横幅**而不是一行 warning（判据 A9 的原话）：一行告警在几百行启动日志里等于没说。横幅必须让运维不看代码就知道
**哪一档介质**、**哪些接口没了**、**为什么**、**改哪儿**——这四件由 `the_refusal_banner_says_which_medium_which_routes_why_and_how_to_fix` 看着。

### D3　逃生门是环境变量，不是配置键

`PLANT_REVIEW_ALLOW_EPHEMERAL_STORE=1` 时，`mem` 照常启用校审域，但启动横幅**每次都点名它**（`startup_line` 的 `***` 那一行）。

它是环境变量而不是 `DbOption.toml` 里的一格，正因为这不该是一个能躺在配置文件里过一个月的决定：环境变量进程一停就没了，
下一个人起服务时它不会替他做决定。用途只有演示与联调。

### D4　超时与慢查询口径逐字沿用旧端，但不再有「新建连接超时」

`REVIEW_QUERY_TIMEOUT = 5s` / `REVIEW_QUERY_LONG_TIMEOUT = 8s`（分页与批量）/ `REVIEW_DDL_TIMEOUT = 30s`，
超过 `REVIEW_SLOW_QUERY_WARN = 1000ms` 打一条 `[REVIEW_DB.slow]`；超时打 `[REVIEW_DB.timeout]`，查询本身失败打 `[REVIEW_DB.error]`。
三个前缀与三个时限都取自旧端 `review_db.rs:17-19`，改它们等于改运维已经在 grep 的那几个词。

与旧端的唯一语义差异：旧端每次新建 ws 连接、并把它计进 `review.fresh_connection` 超时；这里是 Arc clone，不会失败，那条超时随之消失。
`Response` 级的 5s / 8s 不变。

### D5　首启建表挂在 `fresh_review_db()` 上；DDL 以旧端**运行时** DDL 为真

`schema::ensure_review_schema` 用进程内 `OnceCell` 保证只真跑一次，失败**只告警不挡路**（索引缺失退化成慢查询，把整域打死代价更大），
且失败不写 `OnceCell`，下一次调用自动重试。

挂在 `fresh_review_db()` 而不是启动序里，是因为六个业务子模块本来每次操作前就取一次句柄——挂在这里，它们零改动就获得「首次用到即建好」。

DDL 本体在 `rs_surreal/review/review_tables.surql`，`include_str!` 冻进二进制（同 `data_interface::embedded_surql` 的纪律：
表定义随代码版本走，不靠部署目录里那份会漂移的副本）。旧端有**两份互相打架**的 surql（一份 SCHEMAFULL、一份 SCHEMALESS），
而两份**都没被加载过**——旧端跑起来时的实际形态只有 `review_db.rs:186-216, 241-277` 的运行时 DDL。因此以运行时 DDL 为真，
两份旧 surql 只当史料；由此产生的三处有意差异（`review_attachment` 保持 SCHEMALESS、不建无人读写的 `review_form` / `review_opinion`、
显式 `DEFINE TABLE IF NOT EXISTS`）写在那个文件头上。

`review_workflow_history` 是全域唯一的 SCHEMAFULL 表，且必须用 `OVERWRITE`：旧字段 `operator_id` / `operator_name` / `timestamp`
曾是必填，而 RUS-244 之后的写入路径只给 `actor_*` 一族——不改成 `option`，SCHEMAFULL 会**静默拒收**整条历史
（2026-05-09 生产事故的原样复现条件，`workflow_history_accepts_rows_that_omit_the_legacy_operator_fields` 就是那道绊线）。

### D6　附件二进制落盘不落库；落盘目录与静态 URL 只有一个出处

文件本体写磁盘，库里只存元数据（`review_attachment` 表）。目录 = `asset_root()` + `[review_attachments].dir`
（默认 `review_attachments`，写绝对路径则原样使用），由 `options::review_attachments_dir()` **唯一解析**——
上传 handler 与 ServeDir 挂载点都从这一个函数取。

此前上传那侧写死的是相对路径 `assets/review_attachments`：只要设了 `PLANT_ASSET_ROOT`，两边就指向不同目录，
症状是「传得上去、下不来」，而且两条路径**各自都成功**。那正是宪法 III 说的静默失效，所以它是一个函数而不是两处字面量。

URL 前缀 `/files/review_attachments/` 与 50MB 单文件上限**不是配置项**：前者是 plant3d-web 写死的契约、且落库的 `download_url`
里带着它，后者连错误文案一起被判据 A4/A5 钉住。能配的只有「文件放在磁盘哪个目录」这一件与契约无关的事。

## 不变量

- **S1 一个库**：`src/web_service/review/` 下不存在第二个 Surreal 连接、第二个 ns·db、第二种存储；取句柄只有 `fresh_review_db()` 一条路。
- **S2 不 durable 就不开门**：介质活不过重启且无显式放行时，四个前缀一律 503；没有「能用但重启就没」的中间态。
- **S3 拒绝是响亮的**：拒绝路径上必然产生一段多行横幅（介质 / 受影响前缀 / 原因 / 两种改法 / 本 ADR 路径），不是一行 warning。
- **S4 建表幂等**：整份 DDL 每个进程重放一次，不报错、不清行（`IF NOT EXISTS` + `OVERWRITE` 只改定义不重建表）。
- **S5 附件一个目录**：落盘位置与静态服务指向同一个 `options::review_attachments_dir()`；URL 前缀是常量。

## 实施约束（源码钉）

1. `db.rs` 的 `review_persistence_for` 只以 `StoreMode::is_durable()` 为判据；两档介质各走哪一臂由
   `each_store_mode_lands_on_the_arm_its_durability_dictates` 钉死。
2. 拒绝路由对四个前缀下的任意方法/路径回 503、且**不吞**校审域之外的路径（`/api/v1/*` 仍是 404）——两条测试分别看着。
3. `web_service/mod.rs` 的 ServeDir 目录必须来自 `crate::options::review_attachments_dir()`，源码文本护栏
   `review_attachments_are_served_under_the_frozen_url_prefix` 会拒绝在挂载点另写一份路径。
4. `REVIEW_ALLOW_EPHEMERAL_STORE_ENV` 的变量名进了本 ADR 与启动横幅，改名要三处一起改（`options.rs` 有一条测试说这句话）。
5. 整份 `review_tables.surql` 必须能在本仓锁定的 2.1.4 fork 上一次跑通（`mem://` 实跑，不是「编译过」）：
   `DEFINE TABLE IF NOT EXISTS` / `DEFINE INDEX IF NOT EXISTS` / `DEFINE … OVERWRITE` 三种写法在两个 fork 上的支持度是本次移植的已知风险
   （计划 §7 风险 1），换 SurrealDB 版本时 `schema.rs` 的测试先红。

## 后果

- ADR-074 后默认 `store_mode=mem`，因此默认部署只提供模型服务，校审域明确返回 503。生产启用校审前必须先启动持久 SurrealDB，
  再显式配置 `store_mode=rocksdb`；判据 A6「CREATE 一条 → 两进程重启 → 仍读得到」由该外置持久库保证。
- 用 `mem` 起服务的场景**默认没有校审接口**。这是有意的代价：要它就改为 `rocksdb`，或者仅在演示/联调时显式设逃生门。
- 校审表进了模型库的同一个 ns·db：备份、导出、`REMOVE TABLE` 一类运维动作要知道这个库里现在有两类数据。
  **凡是整库级的清理动作（换项目、重建库）都会把校审数据一起带走**，而它没有第二个来源。
- 旧端那条「每次新建 ws 连接」的并发绕障随 fork 一起留在旧端；本仓若将来在 2.1.4 fork 上撞到同形状的 channel 堵塞，
  要改的是这一个函数，不是六个业务子模块。

## 未决

- **从旧端 `:3100` 整库迁移**不在本次范围。`DEFINE TABLE IF NOT EXISTS` 有意不动既有定义，因此把旧端那个库
  （`review_attachment` 是 SCHEMAFULL、缺 `file_name` / `file_size` / `mime_type`）直接接过来时，附件写入会被旧表定义挡下。
  真要迁移，得先出一份把那三张表转 SCHEMALESS 的迁移脚本，届时另开 ADR。
- **碰撞数据源待接**（Q4，计划 T4 / 判据 A10）：旧端 `review_integration.rs` 的 `GET /api/review/collision-data` 直读旧模型库的
  `collision_events` 表，而 gen-model 的模型数据面已改为 e3d-direct/RocksDB，**没有同形表**。按 Q4 的定稿，在数据源接上之前
  该接口返回**显式空集 + `reason` 字段**，而不是 404、也不是空数组装作查过了。真实数据源（谁算碰撞、算完写哪儿）待定，
  接上时按本节更新——本节内容由 T4 的负责人提，文件仍归 T1b 维护。
- **`/api/dashboard/activities`**（Q3 的 P2 可选项）读的是 `review_workflow_history`，表已经在本 ADR 的落点里建好了；
  接不接那条接口是范围问题，不是持久化问题。

## 不做

第二条 Surreal 连接或第二个 ns·db（D1）；第二种存储（宪法 I）；把附件二进制存进库（D6）；启动时自动迁移旧端数据（见「未决」）；
把「内存介质也让校审跑起来」做成 `DbOption.toml` 里的一格（D3）。
