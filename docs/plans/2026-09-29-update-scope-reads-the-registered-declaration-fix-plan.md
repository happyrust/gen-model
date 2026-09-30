# 修复计划：本期执行范围每次现算（`[ams-phase] phase=mdb_scope` 刷屏，每次约 4 s）

> 日期：2026-09-29
> 状态：**S1–S6 已实施（工作树，未提交），V1–V5 过（V3–V5 于 09-30 在本机一次性内存 SurrealDB 上跑），09-30 本机端到端验收过（E3D 夹具套件 10/10、真实保存经 watcher 应用、SYST 从盘重读），V6 待现场，见 §10。** §0–§9 是实施前写下的计划，原样留着。
> grill-with-docs 第一轮 Q1–Q5，用户「全部按推荐方案，然后生成开发计划」，随后「按推荐方案继续」。
> 第二轮 Q6–Q10 **没有单独问过**，按推荐定案（§3 标 ★），批注时请重点看。
> 起因：现场 `E:\aios-workspace\publish\12-…` 增量运行时控制台连刷
> `[ams-phase] phase=mdb_scope project=AvevaMarineSample mdb=/ALL source=sys-file dbnums=29 elapsed_ms≈4000`，行间没有别的输出。
> 决策：`docs/adr/ADR-085-the-update-scope-reads-the-registered-mdb-declaration-and-refreshes-it-when-a-syst-batch-lands.md`（同日新立，已接受）；
> CONTEXT.md 新增「MDB 声明」「成员绑定」，改写「本期执行范围」「MDB 闭包」（已写入）。
> 涉及：`src/data_interface/{mdb_membership,mdb_closure,update_scope,manual_update,db_model,batch_worker,increment_manager}.rs`、
> `src/options.rs` + `src/lib.rs`（退役变量点名）、`web/setup.html`、`src/web_service/setup.rs`（钉子）、`specs/021-mdb-scoped-catalogue/spec.md`（一句取代说明）。
> 约束：宪法 II / III / VI（§8）；工作树多会话共用，只 `git add <明确路径>`；禁止 `cargo clean`；changelog 按步落地时记。
> CONTEXT.md 里四处「落地前 / 待落地」注记（「MDB 闭包」「MDB 声明」「成员绑定」「本期执行范围」各一处）在 S2 + S3 落地的那笔提交里删掉。

## 0. 一句话

范围的来源不换（仍是 SYS 里的 **MDB 声明**），只把「每次取范围都重做一遍**成员绑定**」改成读登记表，
再把 09-15 起丢掉的声明刷新点接回 SYST 落库那一刻。顺手删掉一份 08-14 起就没人读的范围缓存，修掉 `setup.html` 的双请求轮询。

## 1. 事实（2026-09-29 核过源码）

| 事实 | 位置 | 对本计划的影响 |
|---|---|---|
| 那一行出自 `fetch`：配置的 MDB 每次 `mdb_membership::resolve` + `install`，无条件 `println!` | `update_scope.rs:174-195` | S2 改这里 |
| 取范围的入口全是现取：`sweep_dirs` 每轮，`preview_manual_update` / `dbnum_statuses` / `enqueue_manual_update` 各一次 | `increment_manager.rs:3717`；`manual_update.rs:3542` / `:3672` / `:4572` → `:3922` | 改 `fetch` 一处即全部生效（宪法 II） |
| `ops.html` 每 3 个周期拉 `/dbnums?project&mdb`；上一次 `/dbnums` 失败、或 `/error-log` 从没取到过时每个周期都拉；plant-ui 拉无参 `/dbnums`，走骨架分支，不取范围 | `web/ops.html:584` / `:938` / `:948`；`handlers.rs:3097`；plant-ui `model_update_api.rs:535` | 现场刷屏的最可能来源；服务端修好即止 |
| `setup.html` 点「执行初始化」后 `setInterval` 每 4 s 发两次 `/dbnums`，不等上一轮 | `web/setup.html:593-604` | S6 |
| 09-15（1bf940840，ADR-078）起声明是进程缓存，只有配置向导会清；09-09 那版 `resolve` 还是每次重读 SYS | `mdb_closure.rs:122` / `:166`；`setup.rs:574` | 现在的 4 s 只花在重扫搜索根与重绑文件上 |
| 范围只看声明：`element_databases()` 只按 STYP 过滤，不看绑定 | `mdb_membership.rs:137` | 读登记表与现算结果逐字相同 |
| 成员绑定是 O(声明库数 × 目录条目数)：每个库把全部项目目录 `read_dir` 一遍、逐条 `is_file()`；闭包每次重扫搜索根。本机 `ams000` 452 个文件 | `mdb_membership.rs:441` / `:1556-1563`；`mdb_closure.rs:742` | 本次不优化（Q5）；4 s 的拆分是按代码估的，没实测 |
| 房间参照的缺件清单、生成来源清单读登记表里的绑定 | `room_topology.rs:378`；`e3d_model_service.rs:3059` | 绑定需要自己的刷新点（S3、S4） |
| SYST `applied` 时只调 `invalidate_scope_cache()` + `SCOPE_DIRTY`；那份缓存 08-14（d0e519699）起无读者 | `batch_worker.rs:3397-3400` | S3 换成重读声明，S5 删缓存 |
| SYS meta 不受监听 / 调试限定 | `increment_manager.rs` `watch_scope_admits` / `debug_scope_admits` | `watch_dbnums` 下 SYST 钩子照样触发 |
| `init_mdb` 启动时 `resolve` + `install`，失败只告警 | `db_model.rs:296-392`；`lib.rs:1061` | S1 启动那一行 `reason=startup` 挂这里 |
| 重挂轮补上目录后直接 `sweep_dirs("share-remount")` | `increment_manager.rs:4861` | S4 在补扫前重做绑定 |
| 改库侧 CURD 驱动的 live 用例对配置 MDB 已不成立（09-09 起范围不看库侧表） | `increment_manager.rs` `live_scope_refresh_baselines_a_db_the_mdb_just_declared` | S3 改写为声明驱动 |
| `MdbDatabase` 字段全 `pub`，`MdbMembership::test_with_databases` 已有 | `mdb_membership.rs:54-74` / `:114` | S2 用例不用加构造器 |

## 2. 完成定义

1. 服务稳态运行、`ops.html` 开着：控制台里 `phase=mdb_scope` 只在启动出现一次（`reason=startup`），此后零行。
2. `GET /api/v1/dbnums?project=…&mdb=…` 与每轮重扫不再付这约 4 s。
3. SYST 批次落库后，MDB 新声明的库不重启就进范围，控制台一行 `reason=syst-refresh added=[…]`。
4. 重读 SYS 失败时范围沿用旧声明，失败原文出现在那一批 SYST 任务的回执里。
5. 共享盘重挂补上目录后，原来缺件的库在补扫前绑到文件。
6. 源码里不再有 `SCOPE_CACHE` / `resolve_cached` / `update_scope_cached` / `invalidate_scope_cache`；`AIOS_SCOPE_CACHE_SECS` 设着时启动点名已退役。
7. `setup.html` 执行后每 4 s 至多一次 `/dbnums`，上一轮回来才排下一轮。

## 3. 决策

| # | 题 | 决议 | 出处 |
|---|---|---|---|
| Q1 | 配置 MDB 的范围以谁为准 | SYS 文件里的 **MDB 声明**；库侧 `MDB` 表只服务别的 MDB 的旧入口；改词表 | 第一轮 |
| Q2 | 术语 | 新增「MDB 声明」「成员绑定」；现有函数名不改，新函数按新词起名（`rebind` / `reload_declaration`） | 第一轮 |
| Q3 | `mdb_scope` 日志 | 只在真做了读声明 / 成员绑定时打，带 `reason` | 第一轮 |
| Q4 | 前端 | 只修 `setup.html`；`ops.html` 不动 | 第一轮 |
| Q5 | 成员绑定本身的开销 | 本次不做，另开一单（连同房间探针与重挂轮的闭包重扫，§7） | 第一轮 |
| Q6 ★ | 声明何时刷新 | SYST 批次 `applied` 时先读后换；失败留旧、进该批回执；只在摄入形态 | 第二轮，未单独问 |
| Q7 ★ | 成员绑定何时重做 | 启动 / 声明刷新 / 重挂补上目录；不做周期重绑 | 第二轮，未单独问 |
| Q8 ★ | 执行范围缓存 | 退役，连同 `AIOS_SCOPE_CACHE_SECS`（设着时点名） | 第二轮，未单独问 |
| Q9 ★ | ADR | 立 ADR-085 | 第二轮，未单独问 |
| Q10 ★ | 在哪验收 | 本机 AMS 复现为准；现场包只核日志与 `/dbnums` 延迟 | 第二轮，未单独问 |

★ 的推荐理由见 ADR-085「为什么不是别的做法」。Q6 若改判「取范围时比对 SYS 文件戳」，只有 S3 换写法，其余步骤不受影响；
Q7 若改判「加周期重绑」，只在 S4 多挂一个对账拍，代价是每拍约 4 s 与一行日志。

## 4. 步骤

依赖：S1 → {S2, S3, S4}；S5 在 S2、S3 之后；S6 与其余全部独立。每步单独可提交、单独可回退。

### S1 · 成员绑定的唯一重做入口（`mdb_membership.rs`、`db_model.rs`）

- 新增 `RebindReason { Startup, SystRefresh, Remount, Fallback }` 与
  `rebind(db_option, project, mdb, reason) -> anyhow::Result<Arc<MdbMembership>>`：计时 `resolve` → 与登记表里的旧份比出新增 / 移除的元素库号 →
  `install` → 打一行（新增 / 移除只在 `SystRefresh` 时带）：

  ```
  [ams-phase] phase=mdb_scope project=AvevaMarineSample mdb=/ALL source=sys-file reason=syst-refresh dbnums=30 unbound=0 added=[9001] removed=[] elapsed_ms=4120
  ```

  `unbound` = 元素库里没绑到文件的个数。它是阻塞调用，async 侧一律 `tokio::task::spawn_blocking`（`get_db_option()` 是 `&'static`，可直接移进闭包）。
- `init_mdb` 改用 `rebind(…, RebindReason::Startup)`，删掉末尾的 `install`；其余打印原样（钉子 `a_missing_dictionary_is_printed_not_only_logged` 不受影响）。
- 钉子：全仓 `src/` 里 `phase=mdb_scope` 只出现在 `rebind` 里（源码钉，字面量用 `concat!` 拆开，免得钉子自己算一处）。

### S2 · 取范围读登记表（`update_scope.rs`）

- `fetch` 配置 MDB 分支：

  ```
  if name == 配置的 MDB:
      membership = mdb_membership::get(project, name)
                   ?: spawn_blocking(rebind(option, project, name, Fallback)).await??
      return ([name], membership.element_databases().dbnum)
  ```

  删掉 `fetch` 里的计时与 `println!`。
- 改写 `fetch`、`file_side_declared` 的文档：去掉「每次 fresh 扫描重新解析」「事件路径仍由 TTL 和 SYS 更新失效控制频率」「缓存未命中时」三处，指向 ADR-085。
- 回退即红：`the_configured_mdb_scope_reads_the_registered_declaration`。按全局配置的 `(project, mdb)` 登记一份假声明
  （DESI 7001、ISOD 7002、DICT 5001，全不绑文件），`UpdateScope::resolve(mdb)` 的 `declared_desi()` 必须恰为 {7001, 7002}；
  现算的旧写法会拿到真名单或报错。用例持锁，结束还原登记表（加一个 `#[cfg(test)]` 的 `mdb_membership::uninstall`）。

### S3 · SYST 落库重读声明（`mdb_closure.rs`、`mdb_membership.rs`、`batch_worker.rs`、`increment_manager.rs` live 用例）

- `mdb_closure::reload_declaration(main_db_dirs, project, mdb) -> anyhow::Result<Arc<MdbDeclaration>>`：
  `read_unbound_declaration` 成功才 `install_declaration`；失败原样返回，缓存不动。
- `mdb_membership::reload_declaration_and_rebind(db_option, project, mdb)`：取主项目库目录（与 `resolve` 同一取法）→
  `reload_declaration` → `rebind(…, SystRefresh)`。
- `batch_worker.rs:3397` 的 SYST `applied` 分支：

  ```
  let option = aios_core::get_db_option();
  match spawn_blocking(move || reload_declaration_and_rebind(option, &option.project_name, &option.mdb_name)).await {
      Ok(Ok(_)) => {}
      失败（含 JoinError）=> {
          msg = "SYST 已落库，但重读 MDB {mdb} 声明失败，本期执行范围沿用旧声明（下一次 SYST 落库或重启时重试）: {error:#}"
          log::warn! + eprintln! + warnings.push(msg)
      }
  }
  SCOPE_DIRTY.store(true, Ordering::SeqCst);   // 必须在重读之后：重扫要看到新声明
  ```

- 钉子：`a_syst_batch_invalidates_the_scope_cache_without_booking_a_derived_sync` 改名为
  `a_syst_batch_rereads_the_mdb_declaration_without_booking_a_derived_sync`，断言直写路径里 `reload_declaration_and_rebind(` 排在
  `SCOPE_DIRTY.store(true, Ordering::SeqCst)` 之前，且不再含 `invalidate_scope_cache`；「不得再排 SYST 派生作业」那两条断言照留。
- 回退即红：`a_failed_declaration_reload_keeps_the_previous_declaration`。`seed_declaration` 种一份 → 对没有 SYS 文件的临时目录
  `reload_declaration` → 返回 Err，`cached_declaration` 仍是种下的那份。改成「先清后读」就红。
- live 用例 `live_scope_refresh_baselines_a_db_the_mdb_just_declared` 改为声明驱动：取当前声明、去掉目标库后 `install_declaration` +
  `rebind` → 重扫不入队、不写观察值 → 装回原声明 + `rebind(…, SystRefresh)` → `resweep_for_scope_change` 入队并走基线。
  `detach_dbnum_from_mdb` / `restore_mdb_curd` 与 CURD 备份表随之删除；用例头的前置条件注释同步改。

### S4 · 重挂补上目录后重做绑定（`increment_manager.rs`）

- `remount_watch_dirs`：`added > 0` 时，在 `sweep_dirs("share-remount", …)` 之前 `spawn_blocking(rebind(…, Remount))`。
  失败 `log::warn!` + `eprintln!`，沿用旧登记表照常补扫（补扫本身不依赖绑定）。
- 钉子：源码顺序——`RebindReason::Remount` 排在 `"share-remount"` 之前。

### S5 · 执行范围缓存退役（`update_scope.rs`、`manual_update.rs`、`options.rs`、`lib.rs`、`specs/021…/spec.md`）

- 删 `CachedScope` / `SCOPE_CACHE` / `scope_cache_ttl` / `cache_get` / `cache_store` / `invalidate_scope_cache` / `UpdateScope::resolve_cached`；
  `finish` 去掉 `stale_fallback` 入参与陈旧回退档（配置错误照旧上抛）；删 `manual_update::update_scope_cached`，
  以及 `update_scope` 文档里「它们本身就是缓存的刷新点」那句（两者都在 `manual_update.rs` 的 `impl AiosDBManager` 里）。
- 删缓存的五个用例与 `CACHE_TEST_LOCK` / `fetched`。「名字打错必须报错」已由 `a_non_empty_store_never_consults_the_file_side` 覆盖。
- `AIOS_SCOPE_CACHE_SECS` 仍在环境里时，启动通告点名一句「已退役、不再生效（ADR-085）」——照 `store_mode_notices()` 的形态，由 `lib.rs` 启动时打。
- 钉子：`update_scope.rs` 源码不含 `SCOPE_CACHE`、`resolve_cached`（`concat!` 拆字面量）。
- `specs/021-mdb-scoped-catalogue/spec.md` FR-007 后加一句「2026-09-29 起由 ADR-085 取代：范围不再有独立缓存」。
- `.github/context-cold.md` 是生成的索引，不手改。

### S6 · `setup.html` 轮询（`web/setup.html`、`src/web_service/setup.rs`）

- `loadDbnums()` 返回 `rows`（早退分支返回 `[]`）；`executeInit` 改成 `setTimeout` 串行：每拍 `await loadDbnums()` 一次，
  用返回的 `rows` 判同步，没完才排下一拍；`finish()` 改用 `clearTimeout`。
- 钉子：`executeInit` 段内不含 `setInterval(`，也不含直接的 `api("/api/v1/dbnums`。现有向导钉子（`the_wizard_*`）不受影响。

## 5. 验收

| # | 判据 | 怎么判 |
|---|---|---|
| V1 | S1–S6 的钉子与新用例全绿 | `cargo test --lib update_scope`、`mdb_membership`、`mdb_closure`、`a_syst_batch`、`web_service::setup`、`increment_manager::tests` |
| V2 | 全库不回退 | 实施前后各跑一次 `cargo test --lib -- --test-threads 1`，passed 不少于实施前。最近一次记录是 2453 passed / 0 failed / 128 ignored（changelog 2026-09-29「房间归属计算默认开」条），以实施前现跑为准 |
| V3 | 稳态零行 | 本机 AMS：`scripts/Start-Surreal8009.ps1` 起库 → `scripts/Start-AiosDatabaseManual.ps1 -Config DbOption -Port 8022`（仓根 AMS `/ALL` 配置，无扩展名与脚本默认值同形；脚本写死 `rocksdb` + `ingest`；`mem` 档派生读透，重扫与 `/dbnums` 都不取范围，复现不出来）→ 浏览器开 `http://127.0.0.1:8022/ops.html` 10 分钟 → `Select-String -Path output\manual-increment\<时间戳>\aios-database.stdout.log -Pattern 'phase=mdb_scope'` 恰好一行 `reason=startup`。改前同法记一次基线（预期每 10–14 s 一行） |
| V4 | `/dbnums` 省下约 4 s | 同机同配置，改前改后各 `1..10 \| % { (Measure-Command { Invoke-RestMethod 'http://127.0.0.1:8022/api/v1/dbnums?project=AvevaMarineSample&mdb=ALL' }).TotalMilliseconds }` 取中位数，差值约等于改前那行的 `elapsed_ms` |
| V5 | 声明刷新 | S3 改写的 live 用例 `-- --ignored` 绿（要 SurrealDB 与 AMS 文件，环境变量按用例头注释给） |
| V6 | 现场 | 发布包替换后，控制台只见启动一行 `mdb_scope`；有条件时在 E3D Admin 给 `/ALL` 加一个库并保存，出现 `reason=syst-refresh added=[…]`，`/dbnums` 不重启即列出该库 |

## 6. 风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| E3D 在非管理操作时也写 SYS（我按库类型推断「登录与会话记在 COMM、不写 SYS」，没在现场日志核对） | SYST 批次变多，每次重读约 4 s | V6 看一天日志里 `reason=syst-refresh` 的次数；多了再议 Q6 |
| 声明了的库启动时没文件、之后不经 SYS 变更出现（ADR-085 后果 3） | 重启或重挂补上目录前一直当缺件 | 接受；启动日志与 `/health` 本来就点名缺件 |
| 登记表是进程级单例，S2 用例与其他读登记表的用例互踩 | 用例偶红 | 持锁、结束还原；V2 用 `--test-threads 1` |
| S3 让 SYST 批次多花约 4 s | 该批耗时变长 | 只在 SYS 变更时发生；在 `spawn_blocking` 里跑，不堵 runtime |

## 7. 不做

- 成员绑定与闭包的扫盘开销（Q5）：`bind_declared` 的 O(N×M)、房间探针 `current_mdb_source_inventory` 每次 `plan_watch_dirs`
  （`e3d_model_service.rs:3065`）、重挂轮每 60 s 一次 `plan_watch_dirs`。另开一单。
- `ops.html` 节奏（Q4）。
- 读透形态的声明刷新（ADR-085 D2）。
- 库侧路径（别的 MDB）与 `interpret` 的四个出口。
- 现有函数改名（Q2）。

## 8. 宪法核对

| 条 | 本计划 |
|---|---|
| II 一条规则只有一份实现 | 范围判定仍只在 `UpdateScope`，改在 `fetch` 一处，手动与自动两条路同时生效；成员绑定的重做收成 `rebind` 一个入口 |
| III 静默失效是最高级别缺陷 | 重读失败进该批回执 `warnings`；退役的环境变量启动点名；声明了没文件的库照样在范围里，不被悄悄缩掉 |
| VI 不变量由可执行的守护看住 | S1–S6 每步各有一条回退即红的用例或源码钉；live 用例改写后仍钉「MDB 加库不重启进范围」 |

## 9. 回退

每步各一笔提交，`git revert` 即回。无数据格式变化、无新配置键；S5 回退即恢复缓存与 `AIOS_SCOPE_CACHE_SECS`。

## 10. 实施与验证（2026-09-29，工作树，未提交）

### 10.1 改了什么

- **S1** `mdb_membership.rs`：`RebindReason`、`rebind`、`rebind_phase_line`（阶段行多了 `reason=` 与 `unbound=`，SYST 刷新再带
  `added` / `removed`）、`reload_declaration_and_rebind`、从 `resolve` 抽出的 `own_db_dirs`、`#[cfg(test)]` 的
  `REGISTRY_TEST_LOCK` / `uninstall` / `RegistryRestore`；模块头文档改写。`db_model.rs::init_mdb` 改走 `rebind(…, Startup)`，删掉末尾的 `install`。
- **S2** `update_scope.rs::fetch`：配置的 MDB 读 `mdb_membership::get`，未登记才 `spawn_blocking(rebind(…, Fallback))`；
  删掉计时与打印；`fetch`、`file_side_declared` 的文档改写。
- **S3** `mdb_closure.rs::reload_declaration`（先读后换，`declaration` 的现读分支复用它）；`batch_worker.rs` 的 SYST `applied` 分支改成
  `spawn_blocking(reload_declaration_and_rebind)`，失败进本批 `warnings` 并 `eprintln!`，然后 `SCOPE_DIRTY`。live 用例改为声明驱动，
  `MDB_CURD_BACKUP` / `detach_dbnum_from_mdb` / `restore_mdb_curd` 删除。
- **S4** `increment_manager.rs::remount_watch_dirs`：补扫前 `spawn_blocking(rebind(…, Remount))`，失败告警、照常补扫。
- **S5** `update_scope.rs` 删 `CachedScope` / `SCOPE_CACHE` / `scope_cache_ttl` / `cache_get` / `cache_store` / `invalidate_scope_cache` /
  `resolve_cached`，`finish` 去掉陈旧回退；`manual_update.rs` 删 `update_scope_cached`；`options.rs` 加 `RETIRED_SCOPE_CACHE_SECS_ENV` 与
  `retired_scope_cache_notice`，`lib.rs` 启动时 `eprintln!`；spec 021 FR-007 加取代说明。
- **S6** `web/setup.html`：`loadDbnums` 交回行；`executeInit` 改 `setTimeout` 串行，加 `pollGen`，重复执行或完成向导时旧链自己停；`finish` 用 `clearTimeout`。
- 文档：CONTEXT.md 的四处「落地前 / 待落地」注记已删；ADR-085 落地行已改；changelog 记了一条。

守护：新增 9 条（`the_configured_mdb_scope_reads_the_registered_declaration`、`a_fetch_error_is_raised_not_papered_over`、
`the_scope_cache_stays_retired`、`the_rebind_line_counts_element_databases_and_diffs_only_on_syst_refresh`、
`the_mdb_scope_phase_line_is_built_only_by_rebind`、`a_failed_declaration_reload_keeps_the_previous_declaration`、
`a_remount_rebinds_members_before_the_catch_up_sweep`、`the_init_poll_asks_for_dbnums_once_per_tick_and_never_overlaps`、
`a_retired_scope_cache_env_is_named_not_swallowed`），改名 1 条（`a_syst_batch_rereads_the_mdb_declaration_without_booking_a_derived_sync`），
删掉缓存的 5 条。

### 10.2 验证

| # | 结果 |
|---|---|
| V1 | 定向跑 `update_scope` / `mdb_membership` / `mdb_closure` / `a_syst_batch` / `a_remount_rebinds` / `web_service::setup` / `a_retired_scope_cache_env` / `a_missing_dictionary_is_printed`：**67 passed / 0 failed / 3 ignored** |
| 负例 | `fetch` 临时改回「每次现做绑定」：`the_configured_mdb_scope_reads_the_registered_declaration` **红**，拿到本机 AMS 的真名单 29 个库（`[1112, 6003, …, 250705]`）而不是登记的 {7001, 7002}；那一轮的阶段行是 `reason=fallback dbnums=29 unbound=0 elapsed_ms=1246`。已还原 |
| V2 | `cargo test --lib`（默认并行）：**2457 passed / 0 failed / 128 ignored**，共 2585 条。对照 changelog 同日「房间归属计算默认开」条的 2453 / 0 / 128：+9 新增 −5 删除 = +4，逐条对得上。计划写的是 `--test-threads 1`，默认并行已全绿，登记表用例的锁在并行下也成立 |
| 真文件 | ignored 用例 `ams_mdb_initialization_scope_without_database`（真 AMS 文件，不连库）**绿**：兜底路径打一行 `reason=fallback dbnums=29 unbound=0 elapsed_ms=1244`，29 个库与房间参照的冻结源一致。本机本地盘一次成员绑定约 1.2 s；现场约 4 s，多半是盘更慢或在共享盘上 |
| 页面 | `web/setup.html` 抽出脚本 `node --check` 通过 |
| 编译 | `cargo check --lib` 通过；本 crate 没有新告警（剩下的 vendor `e3d-model` 两条 dead_code 与 `cc` 未用依赖是既有的） |
| 环境（V3–V5） | 2026-09-30 本机 AMS `/ALL`，仓根 `DbOption`。SurrealDB 用 `gen-model\bin\surreal.exe` 起一次性 `memory` 库占 8009，每组换一台新的（没碰 `.surreal/ams-8009`）；服务 `AIOS_STORE_MODE=rocksdb` + `AIOS_DATA_FACE=ingest` + `AIOS_SKIP_STARTUP_ROOM_BUILD=1`，端口 8022。改前 = 09-28 release 包的副本，改后 = 本工作树的 release 构建（两包之间还差着别的会话的未提交改动，这里只比 `mdb_scope` 行数与 `/dbnums` 延迟）。`ops.html` 的节奏用 PowerShell 模拟：每拍 `/health` + `/queue` + `/tasks?limit=12`，每 3 拍加一次 `/dbnums?project&mdb` + `/error-log`，拍间 2 s，队列排空后跑 150 s——不是 §5 写的浏览器开 10 分钟 |
| V3 | **过**。改前：启动 1 行，稳态 22 次 `/dbnums` 打 22 行，测延迟 10 次再 10 行，共 33 行，一次 `/dbnums` 一行，每行 `dbnums=29 elapsed_ms=846–897`。改后：全程只有启动那一行 `reason=startup dbnums=29 unbound=0 elapsed_ms=862`，稳态 25 次加测延迟 10 次 `/dbnums` 零行；stderr 无报错 |
| V4 | **过**。`/dbnums?project=AvevaMarineSample&mdb=/ALL` 连打 10 次取中位数：改前 **883 ms**，改后 **27 ms**，省下 856 ms，与改前那行的 `elapsed_ms`（约 860）对得上。现场那行约 4 s，省下的应同量级（现场没量） |
| V5 | **过**。live 用例 `-- --ignored` 1 passed，6.7 s；`AIOS_STARTUP_APPLIED_DBNUM=1112`、`AIOS_STARTUP_NEW_DBNUM=7998`、`AIOS_ROOM_MEMBERSHIP=0`。阶段行依次：`reason=startup dbnums=29` → 摘掉后 `reason=syst-refresh dbnums=28 added=[] removed=[7998]`，重扫点名 7998「不在声明名单里，本轮不入队」 → 装回后 `reason=syst-refresh dbnums=29 added=[7998] removed=[]`，`scope-refresh` 新排 7998 `1..=465`，首次按需初始化 succeeded。边界：第二拍是进程内装回声明（SYS 文件没改），`reload_declaration_and_rebind` 从盘上重读那一步不在 V5 里，靠 `a_syst_batch_rereads…`、`a_failed_declaration_reload…` 两条单测与 V6；1112 被仓根 `DbOption.toml` 的 `watch_dbnums`（7998、8000）挡在重扫外，用例只拿它当同目录夹具；本机 7998 只有 4 个 PE、没有交付单元根，模型 lane 空转 |
| 端到端 A（09-30） | **过**。仓库的 E3D TTY 夹具套件 `scripts\Run-E3DFixtureSuite.ps1`（靶 7998，隔离栈 18028 / 18048，工作树 debug 构建）**10/10 PASS**，6 分 15 秒，`output/e3d-fixture/20260930-102755`。夹具服务控制台的 `mdb_scope` 全程 **1 行**（`reason=startup`）；09-29 同一套件改前那一轮（`20260929-184111`）是 **70 行**，两轮都是 21 个数据批次、20 次 `scope-refresh`。本机缺 `bin\surreal.exe`，用 `L3_SURREAL_EXE` 指到 `..\gen-model\bin\surreal.exe`；第一次没设，停在起栈（`20260930-102555`），E3D 的 setup / teardown 已各跑一遍、夹具 SITE 已删干净 |
| 端到端 B（09-30） | **过**。一次性内存库占 8009，`initialize_ams_dbnums 7998` 先落 SYS meta 与 7998 基线，工作树 debug 服务经 `Start-AiosDatabaseManual.ps1` 起在 8022、监控真实 AMS 目录。`l3_suite --check-driver` 在 7998 上做两次真实 E3D TTY 保存（建一个 SITE，再删掉）：watcher 自己发现并应用，**没有调 `/update/execute`**，水位 511→513 用 18.7 s、513→514 用 14.8 s（PollWatcher 间隔 30 s）。两个 E3D 会话各在 `amssys` 记了一笔 SYS 会话（comment `Saving Meta Data.`，795 / 796）→ SYST 批次落库 → **从盘重读声明的真路径**两次：`reason=syst-refresh dbnums=29 unbound=0 added=[] removed=[] elapsed_ms=1274 / 1302` → `SCOPE_DIRTY` 那轮 `scope-refresh` 把 7998 的窗口排上。5 个批次全 succeeded，本轮 error-log 零条；期间约 90 次 `/dbnums`，零行。日志 `output/manual-increment/20260930-103510` |
| 端到端 C（09-30） | 一个 TTY 会话里两次 `SAVEWORK`（建 SITE 存一次、删掉再存一次）：7998 前进两个会话（514→516），`amssys` 只长了 38 912 字节，按 B 里每笔 +40 960 推算只多一笔（这一拍没起服务，SYS 头没读）。**SYS 会话按 E3D 会话记，不按保存记**。所以 §2 第 1 条的「此后零行」只在没有 SYS 变更时成立：每个写了 SYS 的 E3D 会话一行 `reason=syst-refresh added=[] removed=[]`、一次重读（本机 1.3 s，现场按 4 s 估，在 `spawn_blocking` 里）。这正是 §6 第一行猜的风险，ADR-085 后果 2 已补记 |
| V6 | **没跑**：现场包。控制台预期改为：启动一行 `reason=startup`，之后每个 E3D 会话结束一行 `reason=syst-refresh added=[] removed=[]`，`ops.html` 开着也不再随 `/dbnums` 出行。本机端到端 B 已走通「SYST 批次落库 → 从盘重读声明」，现场只剩 MDB 真加一个库（E3D Admin 改 `/ALL` 并保存 → `added=[…]` → `/dbnums` 不重启即列出）：仓里没有改 MDB 的宏，TTY 驱动也只起 DESIGN 模块 |
