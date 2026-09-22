# ADR-078：项目集合由 MDB 派生，项目目录在指定的搜索根里按 `<code>000` 找

- 状态：Accepted（2026-09-15。计划 `docs/plans/2026-09-15-mdb-project-closure-and-room-default-dev-plan.md` 第 3 稿经 Plannotator
  门审批准，S0–S6 已接线——`data_interface::project_locator` / `data_interface::mdb_closure` 及 §1.3 表里的全部消费者；
  计划 §5.3 三条真部署用例已在 `_runs\ams-0.1.25` 上跑过，结果见该计划 §5.4）
- 日期：2026-09-15
- 取代：ADR-046「`included_projects` 以项目文件夹名限定扫描范围」
- 修订：ADR-016「监控目录按项目解析，数据域边界按项目划」§三的措辞（「配置纳入的项目」→「MDB 声明的项目」）
- 引用：ADR-016、ADR-028（extract family）、d-25（增量数据面 = MDB ∩ watch_dbnums）、d-682（本共识的登记）

## 背景

2026-09-15 的部署实例只勾了 `AvevaMarineSample`，AMS `/ALL` 声明的 41 个外部库（ACP 34 / SCB 4 / APS 2 / ZDJ 1）全部落空：
启动 stderr 40 条「声明了 dbnum … 项目目录里却没有它的文件」，按需生成报「db 7001 没注册进这个 DbSet，resolver 也给不出它的文件」，
房间输入不全又把两个库的全部模型根判死。现场把四个项目手工写进 `included_projects` 才通——而开发机那份写了四个的配置同样漏了 APS。

core.dll 没有「项目名单」这个概念（ida-bridge，`D:\ida_scratch\plant3\core.dll.i64`）：

- `DB_DB::makeDirectoryName`（`0x590b4a0`）用声明自带的项目码拼 `/%<PROJ>000%`，`DB_DB::makeFileName`（`0x590b5e0`）再接 `/<proj>`，
  `DB_DB::constructFileName`（`0x58f92c0`）拼 `<proj><dbno>` / `<proj><dbno:04>_<extract:04>`；外部库的项目码来自 SYS `PROJ` 字符向量。
- `DLL_PDMSProject::BuildProjectsList`（`0x5840b80`）扫进程环境变量，名字形如 `???000=` 的就是一个项目——项目全集就是环境变量。

我们已经读出了 `PROJ`（`mdb_membership.rs` 的 `declared_project_code`），却只在 `included_projects` 圈定的目录里找文件。名单是人手维护的
E3D 环境替身，少一个就成批失败，而且失败原因在路由目录 → resolver → e3d-io 三跳里丢光。

ADR-046 把 `included_projects` 定为唯一名单并拒绝 UNC，理由是「名单外路径不能混进扫描」。现场外部项目常在远程共享（SCB 的 `custom_evars.bat`
全是 `\\10.181.11.245\…`），用户口径是：**目录可能在远程，所以指定主目录路径（可多个），让服务到这些路径里去搜索即可**。

## 决策

1. **要开哪些项目由 MDB 说了算。** 主项目 SYS 里该 MDB 的 CURD 声明按 `PROJ` 项目码分组（空 = 主项目），得到「项目闭包」；
   闭包从 SYS 现算、只缓存在进程内，配置里不写第二份真值。
2. **项目码 → 目录只在人指定的搜索根里找。** 搜索根 = `[project_path] ++ project_search_paths`（本地或 UNC，顺序即优先级）。每个根按
   `set_project_evars.bat` 文档化的三种摆法找库目录：`root\<code>000`、`root\<P>\<code>000`、`root\<P>\<x>\<code>000`；目录名不区分大小写，
   `<code>000` 的判据沿用「`000` 前一位不是数字」。
3. **不猜。** 同一根里同码两处 = 冲突，该根对此码不给答案；跨根同码先到的根赢、其余记影子并可见；三种摆法都没有的进 `unresolved`，
   按码点名（带 dbnum 清单和找过的根），不回退到环境变量、不解析 evars 脚本、不执行任何 `.bat`。
4. **根不可达只让它自己缺席。** 每个根在独立线程里扫、带超时；超时或出错记到该根的结论上，其余根照常，依赖它的项目码以「根不可达」为由进 `unresolved`。
5. **`included_projects` 退役。** 字段保留只为旧配置反序列化不炸（同 ADR-046 §5 对 `project_dirs` 的处置）；非空时启动打一句「已忽略，本次由 MDB 派生：…」。
   `catalogue_project_priority` 保留，只管**未声明**依赖库同号跨项目时的取舍，校验对象改为闭包。
6. **边界不变。** 数据面仍是 MDB ∩ `watch_dbnums`（d-25）；MDB 声明的外部 DESI 是本 MDB 的成员，照旧摄入（ADR-016 §三只是把「配置纳入」改说成
   「MDB 声明」）；依赖目录扫描（MDB 没声明的 CATA / PROP 补成 `Dependency` 路由）对闭包里的项目照做，本 ADR 不收紧（已知比 core.dll 宽）。
7. **缺口可见。** 声明落空的条目带着项目码一路走到底：路由目录的 `NoFileForDbnum` 带项目码与找过的根；启动横幅一句按码归并；
   `/health.mdb_closure = { roots, projects, unresolved }`，`unresolved` 非空或有根不可达时进 `degraded_sections`；按需生成的失败消息与
   `/model/ensure` 的错误追加路由目录给出的原因。`unresolved` 不拒绝启动。
8. **向导第①步**改为：主搜索根、其它搜索根、主项目（从各根扫到的项目里选）、MDB 名 → 展示闭包表与红字缺口；不再有项目勾选框，不再写 `included_projects`。

## 结果

- 配置只需 `project_path` / `project_search_paths` / `project_name` / `mdb_name`；换 MDB 或 MDB 加了外部库，不用改配置。
- 「少勾一个项目」这一类事故消失；剩下的部署错误只有「搜索根里没有这个码的目录」或「根不可达」，两者都按码点名。
- ADR-046 的「名单外路径不进扫描」换成「搜索根外路径不进扫描」：范围仍由人定，只是「哪些项目」由 MDB 定。
- `project_paths.rs` 的项目根 / 监控目录 / 文件归属改由闭包给出；`mdb_membership::locate_declared` 在闭包给的目录里绑定文件。

## 回滚

恢复 ADR-046 的 `included_projects` 名单解析与对应单测，把 `project_search_paths` 与闭包模块下线；回滚前要确认没有部署已经删掉 `included_projects`。

## 追记 2026-09-22：闭包项目里的 CATA 前移能不能被 watcher 观察并进目录版本核对（GitHub #42）

核查结论（代码走读，钉源码的测试 `increment_manager::tests::the_ingest_sweep_hands_catalogue_sessions_to_the_model_face` 已在）：

- **观察得到。** 监控目录 = 闭包里每个项目的 `<code>000` 库目录（`project_paths::plan_watch_dirs`，含补挂进来的 `discovered_watch_dirs`）；
  PollWatcher 在这些目录上的 Create / Modify / Remove 事件 → `sweep_watch_dirs("watch")` → `sweep_dirs(.., SweepSurface::Full)`，周期 `reconcile` 拍同一条路。
  sweep 里每个**清单选中的 CATA 正本**（`catalogue_manifest_for_dirs`：扫全部监控目录、按文件头认 CATA、同号跨项目按 `catalogue_project_priority` 选主，
  **不受 MDB 声明与 `watch_dbnums` 限制**）记 `catalogue_observed[dbnum] = 最新会话`，Full 收尾交 `reconcile_catalogue_versions_after_sweep` 与上一轮比，
  前移即开一次目录版本核对（`reconcile` 拍强制）。外项目 CATA 与主项目 CATA 走的是同一条路——前提只有一条：它的项目在闭包里且目录在搜索根里解析得到
  （否则 `/health.mdb_closure.unresolved` 点名，本 ADR 决策 7）。
- **两条边界**（都不是「监听不到」，是核对那一步的口径）：
  1. 核对解文件走 `model_source::latest_source_version(dbnum)` → `current_mdb_sources()` = **MDB 声明的成员**。决策 6 里「MDB 没声明、只靠依赖目录扫描补成
     `Dependency` 路由」的 CATA：sweep 观察得到、核对却解不出文件 → 落 `unreadable_catalogue_dbnums`、**不判过期、不级联**。core 对这类库根本读不到，
     所以不是相对 core 的少算；但我们既然读了它生成几何，它前移就是我们自己的过期——要么核对改走路由目录（含 `Dependency` 路由）解文件，要么把
     「生成读到了未声明的 CATA」本身报出来。→ GitHub 追踪票见 #31。
  2. 同号跨项目的 CATA：sweep 观察的正本按 `catalogue_project_priority` 选，生成时 `E3dDbResolver` / `select_dependency_file` 按 `preferred_project`
     （目标 DESI 所在项目）选——两处选法不是同一个函数，`build_set` 注释里 APS7600 vs ZDJ7600 就是这一形状；观察到的会话与生成读的会话可能不是同一份文件。
     → 同上追踪。
- 退路 (ii)（目录库没有反向引用表 → 该库全部依赖根判过期）按 dbnum 在 `reverse_reference::expand_reverse_cascade` 里做，不分本项目 / 外项目。
