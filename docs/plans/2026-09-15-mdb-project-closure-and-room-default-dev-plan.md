# 项目集合由 MDB 派生、项目目录在指定的搜索根里按项目码找：修复计划

> 来源：2026-09-15 部署实例 `_runs\ams-0.1.25` 上「显示模型」不出图的排障（会话 fable-5-1-5 → fable-5-1-11）。
> 向导只勾了 `AvevaMarineSample`、模板把 `room_membership` 写成 `true`，两处合起来把模型面全堵死；现场改配置已通，
> 本计划修的是**产品**。core.dll 的对照证据由 ida-bridge 取自 `D:\ida_scratch\plant3\core.dll.i64`（E3D 3.1，实例 `idalib-33060`），地址均可复核。
> 状态：**已批准**（2026-09-15，第 3 稿经 Plannotator 门审批准；§8 Q1–Q5 按推荐项定）。
> **实施进度（2026-09-15 晚，会话 fable-5-1-15）：S0–S6 全部落地、未提交**——`project_locator.rs` / `mdb_closure.rs` 新模块，
> `project_paths` / `mdb_membership` / `mdb_route_directory` / 横幅 / `/health` / `E3dDbResolver` 失败文案 / 向导（后端探测 + 页面）
> / 模板 / CONTEXT.md / ADR-016·046 注 / changelog 都改了；aios-core `DbOption.included_projects` 加 `serde(default)`（vendor 单独一笔）。
> 验证：隔离 worktree（`cfed5fcce` + 本批）`cargo test --lib` 2110 passed / 0 failed；`cargo check --all-targets` 绿；
> `watch_dirs_probe` 对真盘 `D:\AVEVA\Projects\E3D3.1`（`included_projects = []`）：MDB /ALL 71 条声明 → 闭包 5 项目
> AMS(主 31) / ZDJ(1) / SCB(4) / ACP→AvevaCatalogue(34) / APS→AvevaPlantSample(1)，5 个库目录全部定位、`unresolved = []`、347 ms。
> **§5.3 三条真部署用例已跑**（2026-09-15 20:13–20:38，会话 opus-5-36，见 §5.4 与
> `docs/evidence/2026-09-15-adr078-mdb-project-closure.md`）：`_runs\ams-0.1.25` 换本批 release 二进制、删掉
> `included_projects`，声明落空 40 → 0、1112 的「显示模型」直接出图；缺项目 / 第二个搜索根 / 掉线 UNC 三态都按设计报。
> 跑真部署时逮到一个定位器缺陷并当场修掉：目录联接（junction）在 `read_dir` 的 `file_type()` 里报 symlink 不报 dir，
> 整个搜索根会扫成空（`subdirs` 现在跟一步，新钉 `a_project_folder_behind_a_junction_is_still_scanned`）。
> ADR-078 已改 Accepted。面板批注两轮定了口径：
> ① `included_projects` 不再使用，按 MDB 的逻辑自动找路径（第 2 稿据此把自动派生改成主线）；
> ② 目录可能在远程，**所以还是要指定主目录路径（可多个），让它到这些路径里去搜索即可**（本稿据此把「环境变量 / evars 脚本」两层收掉，
> 定位器只在人指定的搜索根里找）。ADR-046 由本计划的 ADR-078 取代。代码事实对到 `codex/model-projection-cache@cfed5fcce`。

## 0. 一句话

**这个服务要开哪些项目，由 MDB 说了算；每个项目的目录，到人指定的几个搜索根里按项目码找。** SYS 里每条 CURD 声明自带项目码（`PROJ`），
core.dll 拿它拼 `%<项目码>000%` 去定位文件，从头到尾没有「项目名单」这回事。我们已经读到了项目码，却只在人手勾出来的 `included_projects` 里找文件；
少勾一个，MDB 声明成批落空，错误沿路变形三次，到现场只剩一句「db 7001 没注册进这个 DbSet」。修完之后：配置写 `project_path`（主搜索根）+
`project_search_paths`（其它搜索根，本地或 UNC，可空）+ `project_name`（主项目）+ `mdb_name`；其余项目由 MDB 闭包现算、在搜索根里按 `<code>000`
找目录；找不到的项目码按码点名、进 `/health`；`included_projects` 退役。模板的 `room_membership = true` 是独立的一行之错，顺手一并修。

## 1. 现状（代码事实，2026-09-15）

### 1.1 core.dll 怎么找一个库文件（ida-bridge 反编译）

| 函数 | 地址 | 做什么 |
| --- | --- | --- |
| `DB_DB::makeDirectoryName(int n, QU_Buffer&)` | `0x590b4a0` | 项目码非空 → 追加 `"/%" + <PROJ> + n(≥3 位) + "%"`，即 **`/%ACP000%`**；为空 → `/%AVEVA_DESIGN_EXE%/databases`（模板库） |
| `DB_DB::makeFileName` | `0x590b5e0` | 上一条 + `/` + 小写项目码 → `/%ACP000%/acp` |
| `DB_DB::constructFileName(int, int dbno)` | `0x58f92c0` | 给了 dbno：`appendDBFileName`（`0x58f4f10`，≥3 位）拼成 `acp7001`；`foreign()`（`0x59042b0`，`DB_DB+172`）为真时先 `FHFIND("OLD,CLOSED")` 这个旧名，找不到再回退 `appendExtractFileName`（`0x58f4fb0`）的 `acp7001_0001`；没给 dbno 直接用 4 位 `_` 4 位形式 |
| `DB_DB::project()` / `setCurrentProject()` | `0x590eee0` / `0x5911fe0` | 项目码是 `DB_DB+200` 的 PMLTypeString：本项目库由 `UQPRJL`（登录项目）填，外部库由 SYS `PROJ` 字符向量填（仓里 `mdb_membership.rs:1386` 的 CHFRAN 注释说的就是它） |
| `DLL_PDMSProject::BuildProjectsList()` | `0x5840b80` | `GetEnvironmentStrings()` 逐条扫：名字第 4 个字符起是 `000=` 的就是一个项目（`AMS000=`），`000ID=` 是项目 ID。**E3D 的「已知项目全集」就是环境变量，没有第二张表** |

E3D 里 `%ACP000%` 由 `evars.bat` → `%projects_dir%custom_evars.bat` → 各 `evars<Project>.bat` 设定；现场脚本里外部项目常在 UNC
（`SCB\custom_evars.bat`：`\\<文件服务器>\<共享>\PDM000`、`REF000`、`MSP000`……）。这些脚本形态各异、还会 `call` 到远程，本计划**不解析它们**：
人把这些远程根填进 `project_search_paths`，我们只在根里找 `<code>000`。`set_project_evars.bat` 自己文档化了库目录的三种摆法（§D2 照搬）。
目录名大小写随手写（`acp000` / `ZDJ000` / `SCB000`），Windows 不区分。

### 1.2 仓里断在哪

- `mdb_membership.rs` **已经**从 SYS 读出 `PROJ`（`MdbDatabase.declared_project_code`，:61）。但 `locate_declared()`（:1417）只在
  `plan_watch_dirs(db_option).projects`——即 `included_projects`——里找「文件名前缀 = 项目码」的文件；没配这个项目 → `path = None`
  （:317–321 记一条 `declared project ACP has no matching configured source; same-dbnum foreign fallback disabled`）。
- `mdb_route_directory.rs::from_databases`（:290）把 `path = None` 的声明放进 `declared_without_file: BTreeMap<u32, String>`（:252）——
  **只留了 dbnum 和 SYS 名，项目码在这一步丢了**。`route_dbno`（:479）对它报 `RouteError::NoFileForDbnum { dbnum, mdb, name }`（:128）。
- 启动横幅 `lib.rs::print_route_directory_banner`（:703）逐条 `eprintln!`（:723–727）。部署首启 stderr **40 条**；
  全部日志按项目码分组：**ACP 34 / SCB 4 / APS 2 / ZDJ 1**（开发机那份四项目配置也漏了 APS）。
- 按需生成走 `fast_model/e3d_model_service.rs::E3dDbResolver`（:545，`impl DbFileResolver` :554）：`select_dependency_file`（:167）
  查 `InMemoryCataLocator + ProjectDbFiles`，**不问路由目录**，回 `None`；e3d-io 就地报 `DbElementError::DbNotRegistered`
  （`vendor/e3d-io/src/db_element.rs:89`「db 7001 没注册进这个 DbSet，resolver 也给不出它的文件」）。「属于项目 ACP、没配」这个原因已经不在了。
- 房间那一坑同源：`room_membership = true` 时常驻房间要 /ALL 全部 DESI，6003 SCBDESI 在 SCB 里 → 房间计算器 blocked →
  每根 `room calculator unavailable for this task`（`batch_worker.rs:2572`、`dbnum_initialize.rs:467`）→ 1112 `18/18` 失败、7997 `6772/6772` 失败。
- 模板 `scripts/templates/DbOption.factory.toml:128` 写 `room_membership = true`，上面的注释「数据、模型增量与房间归属计算缺省开启」
  已过期；代码默认是 `false`（`options.rs:1700 effective_room_membership … unwrap_or(false)`，:1634 文档「默认 false」），
  仓里 `DbOption.toml` 也是 `false`。向导（`setup.rs:395–413` 管的键）不碰这个键，所以现场无从发现。
- 向导 `setup.rs::scan_project_path`（:454）只列 `project_path` 的子目录和它们的 `code_prefixes`（`scan_e3d_shape` :486），不读 SYS；
  `setup.html::gatherPatch`（:402）一个没勾时退成 `[project_name]`（:424）。
- 共享盘的容错哲学已经写在 `project_paths.rs` 头注释：逐项目容错、每个根的结论要报出来（掉线的共享盘只让它自己缺席）。搜索根沿用这条。

### 1.3 `included_projects` 今天的消费者（退役要逐个改）

| 处 | 用途 |
| --- | --- |
| `project_paths.rs`（整模块：`resolve_project_root` :73、`plan_watch_dirs` :231、`project_of_path`） | 项目根、监控目录、文件归属 |
| `mdb_membership.rs::resolve` :286 → `locate_declared` :1417 | 声明绑定文件 |
| `mdb_read_store.rs:119`、`lib.rs` 启动链、`manual_update.rs:3841/3917`、`increment_manager.rs:3421/4694` | `add_dependency_dir` 的项目来源；`select_catalogue_candidates` 的名单校验（`initialization_phase.rs:950`） |
| `cata_closure.rs:403` | 无清单时的依赖项目集合 |
| `increment_manager.rs:3681/4995` | 报错文案「检查 project_path / included_projects」 |
| `web_service/setup.rs`（补丁键、子集、测试）、`web/setup.html` | 向导 |
| `bin/l3_suite/fixture.rs:795`、`bin/manual_scan_probe.rs:16`、`bin/watch_dirs_probe.rs:71/107`、`bin/direct_attmap_probe.rs:339` | 夹具与探针 |
| `aios_core::options::DbOption`（`included_projects` :108、`project_dirs` :110、`catalogue_project_priority`） | 字段 |

### 1.4 约束

- **ADR-046**（Accepted 2026-08-24）：`included_projects` 是当期扫描范围的唯一名单，每个值必须是 `project_path` 下的单个文件夹名。本计划以 **ADR-078** 取代它；
  它拒绝 UNC 的理由（名单外路径混进扫描）在新形状下不成立——搜索根是人显式给的，范围仍由人定，只是「哪些项目」由 MDB 定。
- **ADR-016 §三**：项目依赖数据可以跨项目进入，项目运行态数据不能。MDB 声明的外部 DESI（如 6003 SCBDESI）是本 MDB 的成员，今天勾上 SCB 时就按
  「MDB ∩ watch_dbnums」摄入；本计划不改这条边界，只是把「勾上」换成「MDB 声明了」。
- d-25「增量数据面 = MDB ∩ watch_dbnums」不变。

## 2. 目标与非目标

目标：

1. 配置只需 `project_path`（主搜索根）+ `project_search_paths`（其它搜索根，可空）+ `project_name`（主项目）+ `mdb_name`；其余项目由 MDB 闭包派生，
   `included_projects` 不再参与任何解析。
2. 项目码 → 目录：按搜索根顺序，在每个根里找 `<code>000` 库目录（三种摆法）；先到的根赢，后面的记影子；一个根里同码两处算冲突、不选；三处都没有的按码点名，不猜。
3. 远程根掉线只让它自己缺席并报出来，不拖死启动。
4. 缺口可见：启动横幅一句、`/health` 一段、按需生成的错误里带原因。
5. 向导第①步改成「主搜索根 + 其它搜索根 + 主项目 + MDB → 看闭包」，没有勾选框。
6. 模板与代码默认一致：出厂 `room_membership = false`，单测钉住。

非目标：房间门政策（`room_membership=true` 且输入不全时全判死）不改，见 §8 Q1；`project_dirs` 不复活；不改数据面口径；不读环境变量、不解析 evars（§8 Q5）。

## 3. 设计决策

### D1　项目集合 = 主项目 + MDB 闭包，现算不落库

`MdbClosure::resolve(roots, project_name, mdb_name) -> ClosurePlan`：
主项目根 = 在搜索根里按 D2 找 `project_name`（文件夹名，兜底当项目码找，§8 Q4），从它的 `<code>000` 目录里找 SYS（沿用 `sys_candidates`），
读该 MDB 的 CURD（沿用 `read_declaration`，含 DBSET 展开），按 `declared_project_code` 分组；`None` 归主项目。每个外部项目码交给 D2 的定位器。结果：

```text
ClosurePlan {
  roots:      [{ path, reachable: bool, codes_seen: [...], problem: Option<String> }],   // 每个搜索根一条结论
  projects:   [{ code, root_index, project_dir, db_dirs, dbnums: [...], shadowed: [dir...] }],
  unresolved: [{ code, dbnums, names }],
  problems:   [...]   // CURD 重复、认不出的 STYP、同根同码两处
}
```

闭包永远从 SYS 现算，只缓存在进程内（与 `mdb_membership::get` 同一处）；配置文件里不写第二份真值。

### D2　项目码 → 目录：只在搜索根里找，顺序即优先级

搜索根 = `[project_path] ++ project_search_paths`（每项本地路径或 UNC，经 `normalize_path_input` 归一）。对每个根、每个项目码，按
`set_project_evars.bat` 文档化的三种摆法找库目录（目录名比较不区分大小写，`<code>000` 的判据沿用 `ends_with_db_suffix`——`000` 前一位不是数字）：

```text
root\<code>000                      // NOMAIN：库目录直接摆在根下
root\<Project>\<code>000            // 标准：AvevaCatalogue\acp000
root\<Project>\<path2>\<code>000    // 多一层：E3D3.1-slyk-project\YCYK-E3D\acw000
```

规则：

- 一个根里同码命中两处 = 冲突：记 `problems`、这个根对该码不给答案，继续看下一个根。
- 跨根同码：**先到的根赢**，其余进 `shadowed` 并在横幅 / health 里可见（本地副本压过服务器副本这种取舍由根的顺序表达）。
- 每个根一次性扫完（深度 ≤ 3、只看目录名、跳过 `back` / `backup`），结果缓存给全部项目码用；不开任何库文件——文件自证仍归路由目录（不变量 3）。
- 根不可达（UNC 掉线、没权限、路径写错）：单独一个线程扫、带超时（默认 15 s），超时或出错记 `roots[i].problem`，其余根照常；
  依赖这个根的项目码进 `unresolved`，原因写「根 X 不可达」而不是「找不到」。
- 三处摆法都没有 → `unresolved`。**不**回退到环境变量或 evars（§8 Q5）。

### D3　`included_projects` 退役，字段保留、启动一句

字段留在 `DbOption` 只为旧配置反序列化不炸（与 ADR-046 §5 对 `project_dirs` 的处置相同）。非空时启动打一句
「`included_projects` 已不再参与项目解析（ADR-078），本次由 MDB /ALL 派生：AMS(主) / ACP→AvevaCatalogue / SCB→SCB / APS→AvevaPlantSample / ZDJ→ZDJ」。
`catalogue_project_priority` 保留：它管的是**未声明**依赖库同号跨项目时的取舍，改成对闭包项目码 / 文件夹名的校验；MDB 声明的库本就绑定到声明的项目码，没有取舍。
新字段 `project_search_paths: Vec<String>`（默认空）进 `DbOption` 与出厂模板，注释写清「远程根填这里，顺序即优先级」。

### D4　依赖目录扫描（`add_dependency_dir`）的项目来源改为闭包

今天对每个 `included_projects` 项目的库目录扫 CATA / PROP 头，MDB 没声明的补成 `RouteOrigin::Dependency`。改后对闭包里每个项目做同一件事——
对「勾了四个项目」的现状零行为变化。要不要收紧成「只开 MDB 声明的库」（core.dll 就是这么做的），见 §8 Q2。

### D5　项目码跟着「声明落空」一路走到底

`declared_without_file` 改存 `MissingMember { name, project_code: Option<String> }`；`RouteError::NoFileForDbnum` 加 `project_code`；`Display` 写成
「MDB /ALL 声明了 dbnum 7001（*MASTER/STLCATA，项目 ACP），搜索根里没有 acp000（找过：D:\AVEVA\Projects\E3D3.1、\\srv\proj）」。
`project_code = None` 表示主项目自己的库丢了文件，措辞分开。`E3dDbResolver::resolve` 回 `None` 之前问一次
`mdb_route_directory::active(project, mdb)?.route_dbno(dbno)`，把这句追加到该根的失败消息与 `/model/ensure` 的 `error`——只加文字，不改解析结果。

### D6　可观测：横幅一句、health 一段、stderr 不刷屏

横幅：「MDB /ALL：搜索根 2（1 不可达：\\srv\proj 超时）；闭包 5 个项目（AMS 主 / ACP、SCB、APS、ZDJ 均在根#1），41 个外部库全部定位；未定位项目 0」。
`/health.mdb_closure = { roots, projects, unresolved }`，`unresolved` 非空或有根不可达时 `degraded_sections += "mdb_closure"`。
逐条明细在 `/health`，stderr 只留按码归并的一段。

### D7　向导第①步：搜索根 + 主项目 + MDB，看闭包

`POST /api/v1/setup/probe/mdb { project_path, project_search_paths, project_name, mdb_name }` 直接跑 D1（向导态没有服务，函数不依赖全局 `DbOption`），
返回 `ClosurePlan`。页面：主搜索根一行、其它搜索根多行、主项目单选（从各根扫到的项目里选）、MDB 名；任一变动就刷新一张表——项目码 / 所在根 / 目录 / 库数 / 影子；
根不可达红字；`unresolved` 红字带 dbnum 清单和一句「把装着 <code>000 的目录（本地或 \\服务器\共享）加进搜索根」。保存不再写 `included_projects`；
仍有 `unresolved` 或根不可达时保存前确认。

## 4. 切片

| 切片 | 动什么 | 不动什么 |
| --- | --- | --- |
| **S0 ADR-078** | 「项目集合由 MDB 派生，项目目录在指定的搜索根里按 `<code>000` 找」，Supersedes ADR-046，修订 ADR-016 §三的措辞；写清 D1–D4 与 §8 的裁决 | 先批后接线 |
| **S1 模板** | `DbOption.factory.toml:128` → `false`，改上方注释；加 `project_search_paths = []`；单测 `factory_template_room_membership_matches_code_default` | 向导键集合（S6 再改） |
| **S2 定位器** | `data_interface/project_locator.rs`：D2（根扫描、三种摆法、冲突 / 影子、超时容错），纯函数 + 单测；`scan_e3d_shape` 的判据搬来共用 | 任何调用方 |
| **S3 闭包** | `data_interface/mdb_closure.rs`：D1；`mdb_membership::resolve / locate_declared` 改吃 `ClosurePlan`；`project_paths` 的项目根 / 监控目录 / 文件归属改由闭包给 | 数据面口径 |
| **S4 接线** | §1.3 表里的每个消费者改到闭包；`included_projects` 只留字段 + 启动一句（D3）；`DbOption` 加 `project_search_paths`；报错文案；L3 fixture 与探针 | `watch_dbnums` |
| **S5 可观测** | D5 + D6：路由目录、横幅、`/health`、`E3dDbResolver` 与 `/model/ensure` | resolver 返回值 |
| **S6 向导** | D7：新探测 + 页面第①步重排；去掉勾选框；`SetupConfigPatch` 去掉 `included_projects`、加 `project_search_paths` | 保存 / 重启流程 |

顺序 S0 → S1 → S2 → S3 → S4 → S5 → S6；S1 与 S2 可并行；S3–S4 一个提交序列（中间态编译得过，行为按切片切）。

## 5. 验证与验收

### 5.1 单测（名字即判据）

- `factory_template_room_membership_matches_code_default`：解析模板，`room_membership == effective_room_membership(None, None)`；模板含 `project_search_paths`。
- `locator_finds_the_three_documented_layouts`：同一临时根下造 `x000`、`P\y000`、`P\sub\z000`，三个码都命中且 `project_dir` 正确。
- `locator_is_case_insensitive_on_code_dirs`：`ZDJ000` 命中 `zdj`、`SCB000` 命中 `scb`。
- `same_code_twice_in_one_root_is_a_conflict_not_a_pick`：一个根下两处 `cat000` → `problems` 一条、该根对 `cat` 无答案。
- `earlier_root_wins_and_later_copies_are_shadowed`：两个根都有 `cat000` → 选根#1，根#2 的进 `shadowed`。
- `an_unreachable_root_is_reported_and_does_not_block_the_others`：一个根指向不存在的 UNC（或注入的慢 `read_dir`）→ `roots[i].problem` 有值、其余根正常、依赖它的码 `unresolved` 原因是「根不可达」；耗时 ≤ 超时 + 余量。
- `closure_groups_declarations_by_project_code`：CURD 夹具（ACP×3、SCB×1、主项目×2）→ `projects` 三个、`dbnums` 分组正确；`declared_project_code = None` 归主项目。
- `unresolved_codes_keep_their_dbnums`：定位器全空 → `unresolved = [{code: "ACP", dbnums: [...]}]`。
- `a_declaration_without_file_keeps_its_project_code`：`route_dbno` 的错误 `Display` 含「项目 ACP」与找过的根；`None` 时是「本项目」。
- `included_projects_is_ignored_and_announced`：配置写了 `included_projects = ["X"]`，闭包结果与不写时逐字相同，启动日志有那一句。
- `on_demand_failure_names_the_unresolved_project`：活动目录含 7001 落空，resolver 回 `None` 时失败消息含「项目 ACP」。
- `probe_mdb_returns_the_closure_without_a_running_service`（S6）：临时目录 `Main\mai000\<sys 夹具>` + 第二个根里的 `Cat\cat000`，探测返回 `cat → 根#2`，未知码进 `unresolved`。
- 现有 `project_paths.rs` 的用例改为对闭包断言，`a_project_outside_included_projects_has_no_root` 改名为「不在闭包里的项目没有根」。

### 5.2 回归

`cargo test --lib` 全绿；`cargo check --all-targets`（bins / python 绑定 / L3 fixture 的调用方签名）；`git ls-files --eol` 工作树 = 索引。

### 5.3 真部署

- 把 `_runs\ams-0.1.25` 复制一份，配置只留 `project_path` + `project_name = AvevaMarineSample` + `mdb_name = ALL`（删掉 `included_projects`）：
  横幅报闭包 5 项目（ACP/SCB/APS/ZDJ 均在根#1）、声明落空 40 → 0；`/health.mdb_closure.unresolved = []`；1112 的「显示模型」不改配置就出。
- 反向：把 `AvevaCatalogue` 临时改名 → `unresolved = [ACP …]`、`degraded_sections` 含 `mdb_closure`、ensure 的错误里带「项目 ACP」；改回名字重启即恢复。
- 远程根：把 `AvevaCatalogue` 拷到另一处（或本机共享 `\\localhost\share`），主根里改名、把那一处填进 `project_search_paths`，验证 ACP 在根#2 命中；
  再把 `project_search_paths` 指向一个不存在的 UNC，验证启动不卡死、横幅报「根不可达」、其余项目照常。
- 开发机 `DbOption.toml`：删 `included_projects`，`cargo test --lib` 与 8009 那台的启动行为不变（那份配置漏的 APS 由闭包补上，横幅里能看到）。

### 5.4 §5.3 实测结果（2026-09-15 20:13–20:38）

台子：`_runs\ams-0.1.25`（原样的那一台，不是复制品——它的 rocksdb 店里已经有 1112 / 7997 的数据与模型，省一轮初始化），
`aios-database.exe` 换成本批在隔离 worktree 编的 release（旧的留在 `aios-database.exe.bak-0.1.25`），`web\setup.html` 同样换新（旧的留 `.bak-0.1.25`），
配置删掉 `included_projects`（备份 `DbOption.toml.bak-adr078-*`）。缺项目 / 第二个搜索根两态用**目录联接**搭出来（`D:\adr078-verify\rootA`
装 AMS / ZDJ / SCB / APS 的联接，`rootB` 只装 AvevaCatalogue 的联接），没有改动真盘 `D:\AVEVA\Projects\E3D3.1` 一个字节——8009、8022 上别人的服务照常。

| 用例 | 配置 | 结果 |
| --- | --- | --- |
| ① 真根、无名单 | `project_path = D:/AVEVA/Projects/E3D3.1`，删 `included_projects` | 横幅「搜索根 1；闭包 5 个项目（AMS 主 / ZDJ / SCB / ACP→AvevaCatalogue / APS→AvevaPlantSample 均在根#1），外部库 40/40 已定位；未定位项目 0」；路由目录 71 条、**声明了没文件 0**（改前 40）；`/health.mdb_closure.status = ok`、`unresolved = []`、`problems = []`；`degraded_sections` 不含 `mdb_closure`；`POST /model/ensure {17496/8518}`（用户当初点的那个 ZONE）→ `Generated · 1,731 实例 · 6 个生成根 · durable · publication ready`；1112 / 7997 `in_sync`、`dead_roots 0` |
| ② 缺 ACP | `project_path = rootA`（没有 AvevaCatalogue） | 闭包 4 个项目、`unresolved = [ACP, 34 个 dbnum 全列]`，横幅「外部库 6/40 已定位；未定位项目 1：ACP(34 库)」；`degraded_sections` 含 `mdb_closure`；`ensure … force` 500，消息尾部带**路由目录给出的原因**：「MDB /ALL 声明了 dbnum 7001（\*MASTER/STLCATA，项目 ACP），搜索根里没有 acp000 库目录……加进 `project_search_paths`，详见 `/health.mdb_closure`」——改前只有一句「db 7001 没注册进这个 DbSet」 |
| ③a 第二个搜索根 | `project_path = rootA` + `project_search_paths = [rootB]` | ACP 在**根#2**命中（`dir = rootB\AvevaCatalogue`），闭包 5 项目、40/40、`unresolved = []`、`status = ok` |
| ③b 掉线 UNC | 真根 + `project_search_paths = ["//10.255.255.1/PDM/Projects"]` | 启动 28 s 就绪（**没卡死**），根#2 `reachable = false`、`problem = 找不到网络路径。(os error 53)`，横幅「搜索根 2（1 不可达：…）」，其余 5 个项目照常 40/40，`degraded_sections` 含 `mdb_closure` |

跑①之前先按原配置起过一次（`included_projects = ["AvevaMarineSample", ...]` 还在），横幅与①逐字相同——名单确实不再参与解析。

**②③ 逼出的缺陷（已在本批里修掉）**：`project_locator::subdirs` 用 `entry.file_type().is_dir()` 判子目录，而 Windows 的目录联接与目录符号链接
在这里报 symlink 不报 dir，于是 rootA 整个扫成空、主项目都定位不到（第一版 ② 的结论是「主项目未定位」而不是「缺 ACP」）。现在跟一步 `path.is_dir()`，
新钉 `a_project_folder_behind_a_junction_is_still_scanned`（建不了联接的机器上自动跳过）。顺带确认主项目定位不到时不是死循环：
「模型完整性凭证检查失败」重试 26 次后停，日志 23 KB 封顶。

未做：开发机 `DbOption.toml` 删 `included_projects` 后起 8009 那台——8009 上是别的会话的服务，没去动它；仓里那份配置的这一行本批已经删了，
单测与 `watch_dirs_probe` 覆盖同一条路。

## 6. 不做（本轮）

- 不改房间门的判死政策（§8 Q1）。
- 不复活 `project_dirs`；不读 `<CODE>000` 环境变量、不解析 evars 脚本（§8 Q5）。
- 不执行任何 `.bat`；不猜目录。

## 7. 风险与取舍

- 改动面大（§1.3 一整列），S3–S4 期间中间态要能编译：先让闭包给出与「今天勾满」等价的 `WatchDirPlan`，再逐个消费者切换，最后删旧路径。
- 远程根扫描的超时值是拍的（15 s）：`read_dir` 在掉线的 SMB 上可能挂 20–40 s，所以必须在独立线程里等、到点放弃，不能在启动主线程里裸调。
- 深度 ≤ 3 的目录扫描在大共享盘上可能慢：只看目录名、不 stat 文件、跳过 `back` / `backup`；结果按根缓存一次。
- 主项目目录仍要人填（`project_name`），因为 SYS 在那里；这是保留的人手输入之一，另一处是搜索根本身。
- 「MDB 没声明但目录里有的 CATA / PROP」继续按 Dependency 并进（D4）——比 core.dll 宽；收紧与否见 Q2，本轮不改行为。
- 41 条里 APS 那两条说明开发机配置也不完整——切换后开发机第一次启动横幅会多出 APS，属预期。

## 8. 待定

- **Q1** 房间门：`room_membership = true` 且房间输入不全时，改成「模型照生成、房间跳过、health 标黄」还是维持全判死（ADR-073 现状）？
  推荐：另开一条计划评估 ADR-073 的影响面，本计划不动。
  **已定（2026-09-16，ADR-073 追记 D8–D11）**：取第三种——**降级不判死、房间照算**。缺 DESI 文件按有的文件建（缺的库不在册）；
  缺几何的在册面板不进索引、逐根写边时作为保护集让开、旧边不删；生命周期 `Degraded` 放行计算器；`/health` 标黄并点名缺口。
  两道整份闸（`require_design_files()`、`MemoryRoomCalculator::new` 里的 `ensure_complete()`）删除。见 changelog 2026-09-16。
- **Q2** 依赖目录扫描要不要收紧成「只开 MDB 声明的库」（与 core.dll 一致）？推荐：本轮不收，ADR-078 记为已知差异，留给后续观测。
- **Q3** `unresolved` 非空或有根不可达时要不要拒绝启动？推荐：不拒绝，只 `degraded_sections`（已定位项目的数据面照常），需要时再加 fail-fast 开关。
- **Q4** 主项目的 `project_name` 是文件夹名还是项目码？推荐：文件夹名（现状）优先，找不到时按项目码在搜索根里找，两种都支持。
- **Q5** 要不要保留 `<CODE>000` 环境变量作为零配置的兜底层（core.dll 原生做法，几行代码）？推荐：本轮不做，搜索根足够；以后有现场从 E3D 壳里起服务再加。
