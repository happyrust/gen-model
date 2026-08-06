# ADR-016：监控目录按项目解析，数据域边界按项目划

状态：Accepted（2026-08-04）

日期：2026-08-04

关联：`src/data_interface/project_paths.rs`（新增，监控目录与项目归属的唯一解析口径）；
`src/data_interface/increment_manager.rs`（`sweep_watch_dirs` / `async_watch` /
`should_process_database` / `duplicate_dbnums`）；
`src/data_interface/dbnum_state.rs`（`owner_project` / `FileAnomaly::ForeignProject`）；
`src/data_interface/manual_update.rs`（`in_scope` / `scan_project_candidates`）；
`src/bin/watch_dirs_probe.rs`（新增，不连库的 CLI/JSON 自检）；
ADR-001（DBNUM 更新状态）；ADR-007（SYS meta 不受 included_db_files 门控）；
ADR-011（手动与自动合流到一条数据批次队列）；提交 `c35e4ece`

## 背景

现场只有两行，而且紧挨着：

```
async_watch 使用 PollWatcher 定时轮询（间隔 30s），适配远程共享目录的增量发现
async_watch 退出，增量看门狗已停止: Error { kind: Generic("没有任何监控目录挂载成功，增量看门狗无法工作"), paths: [] }
```

中间**没有**任何一句「文件监控设置失败，跳过该目录 …」，说明
`for dir in self.watcher.watch_dirs` 一次都没进循环——不是「三个共享盘全掉线」，
是目录列表压根就是空的。而那句报错对这两种情况说的是同一句话。

顺着查下去，本轮开出来的七个缺陷不是七个独立 bug，是同一个边界模型错误的七个出口：

> `project` 一个字段同时承担四种语义——**配置主项目**、**SurrealDB 库名**、
> **文件来源归属**、**批次执行上下文**。前两个可以继续绑 `db_option.project_name`，
> 后两个必须从**实际发现路径**推导。

## 七个缺陷

| # | 缺陷 | 判据 |
|---|---|---|
| 1 | 一个项目不可读 → 全部监控目录被清空 | `aios_core::file_helper::collect_db_dirs` 把整批项目 `collect::<io::Result<Vec<_>>>()`，任何一个 `read_dir` 失败就让整批变 `Err`；唯一调用点 `AiosDBManager::init` 又是 `.unwrap_or_default()`，错误被吞成空列表 |
| 2 | 本地 / 绝对 / UNC 路径不能混排 | `project_dirs` 里的条目一律被拼到 `project_path` 之后；正斜杠 UNC（`//host/share`）在部分 Win32 调用上退化成「找不到网络路径」 |
| 3 | 一个项目下多个 `*000` 只取第一个 | `collect_db_dirs` 内层 `.next()`；余下的库既监听不到也摄入不了，数据落库后此后永不更新 |
| 4 | 共享盘晚上线 / 中途掉线不会自动重挂 | 挂载是一次性动作；且 `MountState` 早期版本用 `path_identity`（走 `canonicalize`）当主键，目录一掉线解析不出来就退化成字面量，同一目录在「在线」「掉线」两个时刻算出两个 key |
| 5 | 批次归属项目记错 → 跨项目库基线必然失败 | 两条发现路径都写 `let project = self.db_option.project_name.clone()`，与文件实际所在的监控目录无关；该值一路进 `DiscoveredBatch` → `FrozenBatch` → `execute_one_dbnum` → `initialize_project_dbnum_baseline`，于是拿主项目名去 `ams000` 里找 `acp7006_0001` |
| 6 | F6 把跨项目同号 sys 库判成重复 | `duplicate_dbnums` 只按 `dbnum` 判重，而 dbnum 在 AVEVA 里只在**项目内**唯一——amssys / acpsys / zdjsys 都是 8191 |
| 7 | 跨项目观察值写脏 `dbnum_watermark` 行 | 该表记录 id 就是裸 dbnum；阻断路径仍会写 `file_size` / `file_latest_sesno` / `scanned_at`，于是 8191 那一行挂着 amssys 的身份、`file_latest_sesno` 却是 zdjsys 的 52 |

## 决策

### 一、监控目录的解析必须逐项目容错，且结论要报出来

解析搬到 `data_interface::project_paths`，取代 `collect_db_dirs`：

- 逐项目独立解析，一个共享盘掉线只让它自己缺席；
- 认三类写法——相对项目名（拼到 `project_path` 下）、绝对本地路径、UNC
  （`\\host\share` 与 `//host/share` 归一到同一条），并且项目根本身就是 `*000`
  时直接认它（共享盘上把库目录单独共享出来是常见做法）；
- 收全部 `*000` 而不是第一个；按 `canonicalize` 后的路径去重；
- 启动打印「监控目录解析」逐项目结论（用了哪个根、几个库目录、失败原因）。

目录列表为空与「全都挂不上」是两条不同的错误，各自带上可执行的下一步。

重挂轮（`AIOS_WATCH_REMOUNT_SECS`，默认 60s，设 0 关闭并退回旧的报错退出）：
重新解析 → 复查已挂目录健康 → 失联的先 `unwatch` 再重挂 → 有新挂上就补一次重扫。
**重新解析**不是顺手：共享盘在启动那一刻不可达时，`plan_watch_dirs` 连它的
`*000` 子目录都列不出来，只重试老列表永远等不到它。**先 unwatch** 也不是顺手：
重复 `watch()` 同一个目录会让 PollWatcher 把它列两遍，F6 立刻按缺陷 6 整库阻断。

`MountState` 的主键因此固定为**字面路径**，`path_identity` 只用来挡「同一物理目录
的两种写法」。这一条是写用例时被测出来的——按 identity 做主键的版本在
「掉线→恢复」这一轮上直接红。

### 二、文件归属由路径决定，数据库命名空间由配置决定

`plan_watch_dirs` 本来就知道每个库目录属于哪个项目，把这份映射登记下来
（`record_watch_dir_owners` / `project_of_path`），两条发现路径改用
`self.owning_project(path)`。`project_name` 继续做 Surreal 库名，不参与文件归属判断。

F6 的判重键随之从 `dbnum` 改成 `(归属项目, dbnum)`：跨项目同号不算重复，
同项目内的人手副本（`ams1112_0001 copy`）照旧拦住。

### 三、项目依赖数据可以跨项目进入，项目运行态数据不能

这是本 ADR 真正要钉死的一条边界，也是缺陷 6/7 的根：

| 类型 | 跨项目摄入 | 理由 |
|---|---|---|
| DICT | ✅ | 目录库是被主项目依赖的数据，dbnum 也不冲突 |
| DESI | ✅ 按依赖关系 | 同上 |
| SYST / GLB / GLOB | ❌ | 描述的是「那个项目自己怎么组织」，对本库无意义；而且 dbnum 只在项目内唯一，本库状态层（`dbnum_watermark` 记录 id、`dbnum_info_table`、`pe.dbnum` 聚合）全部按裸 dbnum 做键 |

落点在摄入范围门（`increment_manager::in_scope_with`；2026-08-06 之前叫
`should_process_database`，见文末「后续变更」），**不是**监控目录解析层——那一层只回答
「有哪些东西存在」，不回答「哪些东西业务需要」。而且这**不是「异常阻断」而是
「不在数据域内」**，两个语义不能合并：阻断意味着系统期待处理它却发现异常，
不在范围意味着系统根本不负责。所以日志是单独一句、措辞明确不会被当成「漏扫了」：

```
[init] 忽略非主项目的运行态系统库: project=AvevaCatalogue db_type=SYST dbnum=8191 file=acpsys
       （dbnum 只在项目内唯一，本库只承载主项目 AvevaMarineSample 的系统库）
```

作为纵深防御，`dbnum_watermark` 行带 `owner_project`；`classify_scan` **先**比归属，
不符直接出 `FileAnomaly::ForeignProject`，`record_observation` 对它**一个字都不写**。
顺序是关键：两个项目的文件放一起比「回退 / 类型变更」，判据本身没有意义——
实测里 acpsys 就被判成了「回退（file_latest=45 < applied=169）」，只是结论恰好安全。
旧行（`owner_project` 为空）不做校验，被自己的项目扫一次自然补上，不需要迁移脚本。

## A/B 实证

把 `plan_watch_dirs` 临时换回旧口径，同一份混合配置（相对名 + 绝对本地 + UNC 正斜杠 +
UNC 反斜杠直指 `ams000` + 一台不可达主机）跑 `watch_dirs_probe`：

| 解析器 | watch_dir_count | 诊断信息 |
|---|---|---|
| 旧 | **0** | **空** |
| 新 | 4 | 1 条，点名 `RemoteDown` + os error 53 |

五个项目里只有一个不可达，旧口径把另外四个（含两个本地目录）一起清成 0，且一句话不留
——正是现场那两行的形状。

活服务实测（真实 AMS 目录，四轮）：

| | 修复前 | 修复后 |
|---|---|---|
| 挂载 | —（0 个目录，看门狗退出） | `已挂载 3/3 个监控目录` |
| 跨项目 DICT 批次 | 6 个全 failed，日志写 `开始解析 AvevaMarineSample 的 [DICT]` | 6 个全 **succeeded**，写 `开始解析 AvevaCatalogue / ZDJ 的 [DICT]` |
| dbnum=8191 | `F6 发现同 dbnum 的多个文件，阻断该 dbnum` | 误判消失，改为单独一句「忽略非主项目的运行态系统库」 |
| 共享盘晚上线 | 不会接管 | 15s 内 `重挂轮发现新的监控目录` → `补挂了 1 个监控目录，开始补扫` → `[share-remount]` 重扫 |
| `dbnum_watermark:8191` | `file_name=amssys` 而 `file_latest_sesno=52`（zdjsys 的值） | 修回 169，重跑后不再被写脏；各行 `owner_project` 逐行落下 |

用**改动前的 release 二进制**跑同一份配置做对照，6 个 DICT 批次同样 failed
——确认缺陷 5 是预先存在的，不是本轮引入。

## 回退即红

| 改回旧写法 | 失败的测试 |
|---|---|
| `plan_watch_dirs` 退回 `collect_db_dirs(..).unwrap_or_default()` | `one_unreadable_project_does_not_erase_the_others`、`every_db_dir_under_a_project_is_collected`、`a_root_that_is_itself_a_db_dir_is_used_directly`、`a_project_without_db_dirs_is_reported_not_silent`、`short_project_dirs_yields_none_instead_of_panicking`、`a_share_that_comes_back_is_replanned_and_mounted_once`（10 条里红 6 条） |
| 发现阶段改回 `db_option.project_name` | `both_auto_paths_take_the_owning_project_from_the_watch_dir`（钉源码：`owning_project` 必须先于 `discover_batch`，且 `discover_batch` 之前不得出现 `db_option.project_name.clone()`） |
| F6 判重键去掉项目维度 | `same_dbnum_in_different_projects_is_not_a_duplicate`；同时 `a_copy_inside_one_project_is_still_blocked` 保证防副本能力没被顺手削掉 |
| sys-meta 恢复无条件放行 | `foreign_project_runtime_sys_databases_are_out_of_scope` |
| 归属校验挪到阻断落库之后 | `a_foreign_project_observation_is_not_persisted_at_all`（钉源码两处顺序） |
| `MountState` 用 `path_identity` 当主键 | `a_dropped_directory_is_marked_lost_then_remounted_exactly_once` |

第一条与「旧 release 二进制 A/B」互相印证：一个在单测层、一个在真实二进制层。

验证口径：`cargo test --lib project_paths` 12 passed、`--lib increment_manager`
16 passed / 2 ignored、`--lib dbnum_state` 20 passed / 2 ignored；
`cargo check --lib` 在默认与 `mqtt` 两种 feature 下均过。
不连库的自检：`cargo run --bin watch_dirs_probe -- --pretty`（按当前配置报逐项目结论）
与 `-- --remount-selftest`（在临时目录上跑真实的 `plan_watch_dirs` + `MountState::mount`，
把「共享盘恢复」换成「把目录建出来」）。

## Consequences

- 共享盘掉线不再拖垮其余项目；掉线与「配置里压根没解析出目录」在日志上是两件事。
- `project_dirs` 成为「物理位置」的表达面：项目名留在 `included_projects`，
  每个项目可独立写相对名 / 绝对路径 / UNC。两者按下标一一对应。
- 摄入侧（启动重扫、重复 dbnum 复查、手动候选扫描）读的是「启动列表 ∪ 重挂轮补挂」，
  与 PollWatcher 实际监听的集合一致——补挂进来的目录不会只被轮询、不被摄入。
- `in_scope` / `should_process_database` / `scan_project_candidates` 都多了 `project`
  参数，四条触发路径（启动重扫、文件事件、手动预览、手动执行）共用同一个谓词。
  （2026-08-06：前两者合并成 `increment_manager::in_scope_with`，见文末「后续变更」。）
- 非主项目的 SYST/GLB/GLOB 从此不进队列。它们本来也没被摄入（先被 F6 误判为重复、
  后被回退门拦下），区别是现在**理由是真的**，而且不再连坐其他库。
- 同一份数据被本地路径与 UNC 路径各列一次时，因为必须占用两个不同的项目名，
  F6 的 `(项目, dbnum)` 键不会再整库阻断；代价只剩重复轮询。

## 记账（本 ADR 不做）

- **状态层重新键成 `(project, dbnum)`**：只有当「一个 Surreal 库需要长期承载多个项目
  的全部数据库生命周期」成为真实需求时才做，届时必须一次性改 `dbnum_watermark`、
  `dbnum_info_table`、`pe.dbnum` 聚合、兼容播种、F6、队列键与模型缓存键——不要半改。
  在此之前，边界由上面第三条决策承担。
- **Windows `FILE_ID_INFO` 真文件身份**（volume serial + file id）用于识别「同一台机器
  的两条访问路径」：需要引入 `windows` crate 或开 nightly 的 `windows_by_handle`，
  而 `(项目, dbnum)` 键已经把它的破坏力拆掉，成本收益不划算。降级顺序若将来实现，
  应为 `FILE_ID_INFO` → `canonicalize` → 字面路径，不可颠倒。

## 后续变更

**2026-08-06 — 摄入范围门收敛为「MDB 声明的 DESI」。** 本 ADR 写的时候，范围门是
`in_scope = should_process_database && UpdateScope::admits`，前者串着类型白名单、
`only_sync_sys`、`exclude_db_nums`、`manual_db_nums`。两者合并成
`increment_manager::in_scope_with`，只剩本 ADR 第三条决策（非主项目的运行态系统库）
与 `UpdateScope::admits`。

动机是 issue #10：手写的 `manual_db_nums` 把 7999 挡在增量之外，而它与「MDB 里没有
这个库」在日志上是同一句话——现场看到的只是每 30 秒重复一次的「不在本期执行范围，
跳过数据库: 类型=DESI, 编号=7999」，模型树则表现为「检测到增量但不更新」。
本 ADR 的第三条决策没有变化，只是换了个落点函数名；那几个配置项仍供全量模型生成与
按需基线解析使用，与「这个库要不要增量」无关。
