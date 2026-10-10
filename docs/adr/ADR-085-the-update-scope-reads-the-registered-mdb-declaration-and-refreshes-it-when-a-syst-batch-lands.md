# ADR-085：本期执行范围只读已登记的 MDB 声明——声明随 SYST 落库先读后换，成员绑定只在三个时刻重做

状态：**已接受**（2026-09-29 grill-with-docs 一轮，用户「全部按推荐方案，然后生成开发计划」。第一轮 Q1–Q5 问过；
D2 / D3 / D4 对应的 Q6–Q8 属第二轮，**没有单独问过**，按推荐定案——批注时请重点看这三条。**号到提交时复核**。）
日期：2026-09-29
落地：2026-09-29 已实施（工作树，未提交），计划 `docs/plans/2026-09-29-update-scope-reads-the-registered-declaration-fix-plan.md`
S1–S6，验证见计划 §10。`src/data_interface/{mdb_membership,mdb_closure,update_scope,manual_update,db_model,batch_worker,increment_manager}.rs`、
`src/options.rs` + `src/lib.rs`（退役变量点名）、`web/setup.html` + `src/web_service/setup.rs`。
关联：
- **修订** c5de1040b（2026-09-09）给 `UpdateScope::fetch` 定的「每次 fresh 扫描重新解析」：来源不变（仍是 SYS 文件里的声明），取法改成读登记表。
- **补全 ADR-078 D1**「闭包从 SYS 现算、只缓存在进程内」：那份缓存从此有刷新点（D2），不再是进程内只读一次。
- **退役** 2026-08-06 的执行范围缓存（50d8d4672：`SCOPE_CACHE` / `resolve_cached` / `AIOS_SCOPE_CACHE_SECS`）。它唯一的读者 2026-08-14 就被拿掉了（d0e519699）。
- `docs/plans/2026-09-07-update-scope-file-side-mdb-fallback-plan.md` 的库侧路径不动：它只服务查询别的 MDB 的旧管理入口。
- CONTEXT.md 新增「MDB 声明」「成员绑定」，「本期执行范围」「MDB 闭包」随本文改写。

## 背景

2026-09-29 现场增量运行时，控制台连着刷
`[ams-phase] phase=mdb_scope project=AvevaMarineSample mdb=/ALL source=sys-file dbnums=29 elapsed_ms≈4000`，
每行结果一样，行与行之间没有别的输出。这一行出自 `UpdateScope::fetch`：配置的 MDB 每取一次范围，就调一次
`mdb_membership::resolve` 并 `install`。取范围的入口全是现取：每轮重扫（启动 / watch / 周期对账 / scope-refresh / 重挂），
以及 `GET /api/v1/dbnums?project&mdb`、`/update/preview`、`/update/execute`。运维面板 `ops.html` 每 3 个周期拉一次 `/dbnums`，
拉失败时每个周期都拉。

三件事叠在一起：

1. **4 秒换不来新信息。** 09-09 这么写时，`resolve` 每次确实重读 SYS。09-15 ADR-078 把声明读进进程缓存
   （`mdb_closure::declaration`，只有配置向导会清），此后每次重做的只剩重扫搜索根、给每个声明的库重新找文件。
   范围只看声明，库号一个都不会变。
2. **MDB 里新加的库要重启才进范围，而且不出声。** 同一份声明缓存从不刷新；SYST 落库时 `batch_worker` 调的
   `invalidate_scope_cache()` 作废的是另一份早就没人读的缓存。`batch_worker` 那条钉子测试的文档写的正是要避免的形状（issue #10）。
3. **每次取范围都顺带刷新了成员绑定。** `fetch` 每次 `install` 一份新的成员表，房间参照的缺件清单
   （`room_topology::latest_frozen_sources`）与生成来源清单（`e3d_model_service::current_mdb_source_inventory`）读的就是它。
   这是没人写下来的副作用：取范围一旦不现算，绑定就得另有刷新点。

## 决定

### D1　配置的 MDB：取范围只读登记表

`UpdateScope::fetch` 对配置的 MDB 取 `mdb_membership::get(project, mdb)` 的 `element_databases()`，不调 `resolve`、不读盘。
登记表里没有（`init_mdb` 失败过）才现做一次成员绑定并登记（`reason=fallback`）。范围的来源仍是 **MDB 声明**（主项目 SYS 文件），
不回到库侧 `MDB` 表；声明了却没绑到文件的库照样在范围里。

### D2　声明只在 SYST 批次落库时刷新，先读后换

SYST 批次 `applied` 那一刻（`batch_worker` 现有的范围失效点）：从主项目库目录重读 SYS 里该 MDB 的声明；**读成功才替换**声明缓存，
再重做成员绑定并登记（`reason=syst-refresh`，日志带新增 / 移除的库号），然后照旧置 `SCOPE_DIRTY` 让空闲轮重扫。
读失败时旧声明与旧登记表原样留着，失败原文进这一批的回执 `warnings` 并 `eprintln!`；下一次 SYST 落库或重启时再试。

只在摄入形态生效。读透形态没有 SYST 批次，也不在热路径上取范围，维持启动时读的那一份。

### D3　成员绑定只在三个时刻重做

启动（`init_mdb`）、D2 声明刷新、共享盘重挂轮补上了目录（`added > 0`，补扫之前）。不做周期重绑。

### D4　执行范围缓存退役

`SCOPE_CACHE`、`UpdateScope::resolve_cached`、`AiosDBManager::update_scope_cached`（`manual_update.rs`）、`invalidate_scope_cache`、`finish` 的陈旧回退档，
以及环境变量 `AIOS_SCOPE_CACHE_SECS` 一并删除。环境里仍设着这个变量时，启动通告点名一句「已退役」。
库侧路径（别的 MDB）照旧每次查一次 SurrealDB。

### D5　`[ams-phase] phase=mdb_scope` 只记真实的工作

只在 D1 兜底、D2、D3 真做了读声明或成员绑定时打，带 `reason=startup|syst-refresh|remount|fallback`。稳态一行不出
（稳态指 SYS 没有变更；E3D 会话会写 SYS，见后果 2）。

## 为什么不是别的做法

- **回到库侧 `MDB` 表**（一次 SurrealQL，毫秒级）：库侧 CURD 是摄入进来的历史副本，同名 `/ALL` 会并存两条（设计库与目录库各一条，
  见 `MDB_DBNOS` 的注释），范围与房间参照、路由目录就不是同一份名单了。09-09 选文件侧的理由今天仍成立。
- **取范围时比对 SYS 文件戳**：每次取范围多一次 stat（共享盘上是一次往返），而且 SYS 变不等于 MDB 变。
  SYST 批次落库本来就是「SYS 变了」的权威信号，而且已经有一个被钉住的钩子。
- **只在启动读**：就是 issue #10 的形状，现场只表现为「这个库怎么不更新」。
- **让热路径走 TTL 缓存**（原 `resolve_cached`）：每个 TTL 仍要付一次 4 秒；而名单的变化点是一个已知事件，不需要猜时长。
- **周期重绑**：绑定的变化来自 MDB 变更（D2 已覆盖）或目录可达性变化（D3 已覆盖）；剩下的情形代价与频率不成比例（见后果 3）。

## 后果

1. 稳态下取范围是一次读锁，`/dbnums` 与每轮重扫各省约 4 秒；控制台只在启动、SYS 变更、重挂补上目录时各出一行 `mdb_scope`。
2. SYS 的非 MDB 变更（TEAM / USER 等）也会触发一次重读（约 4 秒，一次）。接受。
   **2026-09-30 本机实测补记**：不止管理操作——E3D DESIGN 的每个会话都会在 SYS 里记一笔（comment `Saving Meta Data.`），
   按会话记、不按保存记（一个会话两次 `SAVEWORK`，SYS 只多一笔）。于是每个写了 SYS 的 E3D 会话一次重读、一行
   `reason=syst-refresh added=[] removed=[]`（本机 1.3 s，现场按 4 s 估，在 `spawn_blocking` 里）。仍接受：取代的是每次 `/dbnums`、
   每轮重扫各 4 秒。证据见计划 §10.2「端到端 B / C」。
3. 声明了的库启动时没有文件、之后**不经 SYS 变更**就出现在盘上（例如手工拷回备份）：重启或重挂轮补上目录之前，成员绑定仍当它缺件。
   09-09 以来每次取范围都会顺带补上，这是有意放弃的。
4. 读透形态下 MDB 变更仍要重启（与今天相同）。
5. `AIOS_SCOPE_CACHE_SECS` 不再生效；设着的部署启动时会看到一句点名。
6. 一个新声明的项目码（MDB 新引用了别的项目）：D2 刷新后闭包随之变化，下一拍重挂轮挂上它的目录，D3 在补扫前把绑定补齐。

## 实施约束

1. 取范围不重做绑定：`the_configured_mdb_scope_reads_the_registered_declaration`——登记一份假声明，`UpdateScope::resolve` 返回的正是它；回退到现算就红。
2. 先读后换：`a_failed_declaration_reload_keeps_the_previous_declaration`——读不到 SYS 时声明缓存原样；回退成「先清后读」就红。
3. SYST 钩子：`batch_worker` 那条钉子改为断言 SYST `applied` 分支先重读声明、再置 `SCOPE_DIRTY`，且源码里不再有 `invalidate_scope_cache`。
4. 重挂：`remount_watch_dirs` 里成员绑定重做排在 `share-remount` 补扫之前（源码顺序钉）。
5. 日志：`phase=mdb_scope` 只在 `mdb_membership::rebind` 一处出现（源码钉）。
6. live：`live_scope_refresh_baselines_a_db_the_mdb_just_declared` 改为声明驱动，不再改库侧 CURD。
