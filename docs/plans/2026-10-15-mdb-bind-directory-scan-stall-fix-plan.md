# MDB 成员绑定逐库扫盘致启动假死修复计划

- 日期：2026-10-15
- 关联：ADR-028（extract 族选主）、ADR-078（项目闭包）、ADR-085（成员绑定登记表）、`docs/plans/2026-09-29-update-scope-reads-the-registered-declaration-fix-plan.md`（同族：取范围曾每轮重扫全盘）
- 现场：`web-offline-v0.2.4-backend-v0.1.39-20261015` 在另一台机器上初始化，项目根为 UNC `\\<文件服务器>\<共享>\E3D2.1\9001`，JEU /ALL `decoded_databases=480`。日志停在 `included_projects = ["JEU"] 已不再参与项目解析` 一行之后，长时间无输出。

---

## 1. 问题定位

启动链最后一条可见日志由 `AiosDBManager::init`（`src/data_interface/db_model.rs`）打出。其后代码顺序：

1. `parse_error::note_dir_scope` / `flush`（`SUL_DB` 一次查询；mem 档为进程内、秒回；rocksdb 档此前已连库，连接失败会在更早处报错退出——均非悬挂点）；
2. `DbFileRegistry::new` / `new_mqtt_inst`（纯内存构造，不碰盘、不联网）；
3. `init_mdb` → `mdb_membership::rebind` → `resolve` → `bind_declared`（`src/data_interface/mdb_membership.rs`）。

**卡点**：`bind_declared` 对 480 个声明库逐个调 `locate_declared` → `pick_within_project`，后者每次都：

- `std::fs::read_dir(JEU000)` 完整枚举一次共享目录；
- 对每个目录项 `path.is_file()`——`Path::is_file` = `fs::metadata`，UNC 上是逐文件的 SMB 元数据往返（`entry.file_type()` 才是枚举自带的免费属性，`SysMetadataResolver::discover` 与 `subdirs` 已用正确写法）。

合计 ~480 ×（一次目录枚举 + 目录内全部文件逐个 stat）。链路正常但延迟高时是几十分钟的静默；SMB 会话半死时 `read_dir`/`is_file` 无超时**可无限期阻塞**（`src/data_interface/project_locator.rs` 头注实测「掉线 SMB 上 `read_dir` 能挂 20–40 s」，`scan_roots` 因此套了 15 s 线程超时，而这条路径没有）。两种情形在控制台外观相同：卡死。

**次级问题**：`rebind` 是纯同步 IO，`init_mdb` 却直接在 async 上下文里同步调用（`rebind` 自身注释要求放 `spawn_blocking`），阻塞 tokio worker；`run_read_through_core` 里首次 `poll_generations`（对全部路由文件逐 stat）与 `startup_route_directory`/`verify_all_headers`（逐路由读 60 字节头）同样未包 `spawn_blocking`。

---

## 2. 目标

- 成员绑定对**每个项目库目录只枚举一次**：枚举产物建 `dbnum → 文件列表` 内存索引，480 次定位全部走内存；
- 目录项类型判定改用枚举自带属性（`DirEntry::file_type`），消掉逐文件 stat；
- 选主语义（最高 extract 叶 > 无后缀 master、排序钉死、shadowed 归账、PROJ 项目码过滤）**逐字不变**；
- 启动链上的同步远程 IO 一律进 `spawn_blocking`，async 运行时不被占；
- 绑定过程有 begin/每目录/总量级的进度与耗时行，「慢」与「死」可分；单目录枚举给出与 `scan_roots` 同款的限时语义。

---

## 3. 任务分解

### T1 目录清单索引（核心）

`src/data_interface/mdb_membership.rs`：

- 新增 `DirInventory`：`from_dirs(&[PathBuf]) -> Self` 一次 `read_dir` 收全量条目，条目过滤 `is_project_runtime_db_file_name`/`parse_extract_file_name` 可解析的库文件，按 `dbnum` 建 `BTreeMap<u32, Vec<(Option<u32> extract, PathBuf)>>`；类型判定用 `entry.file_type()`，`file_type` 失败才回落 `path.is_file()`（与 `discover`/`subdirs` 同例）。
- `pick_within_project(&DirInventory, dbnum)` 改为纯内存挑主，排序规则、叶/母归账、`rest`/`shadowed` 拼装逐字不变。
- `bind_declared` 先对每个 `(project, dirs)` 建一次索引，再逐声明查表；签名随之改为收索引，`locate_declared` 同样改纯函数。
- 目录枚举失败的目录：今天静默 `continue`。改为记入 `membership.problems`（「N 个声明落进读不动的目录 X」一条即可，不逐库铺开）——这个失败面与「声明了没文件」长得一样却原因不同，按本仓「失败要出声」的纪律补一行，不静默新增行为细节。

### T2 限时与进度

- 每目录枚举复用 `project_locator::scan_roots` 的线程 + 截止时刻模式（或同构小封装），超时按该目录不可达处理并进 `problems`——与 T1 的出声合流。
- `rebind` 已有 `[ams-phase] phase=mdb_scope ... elapsed_ms=` 完成行；补一行 begin（`dbnums=480 dirs=1`）与每目录枚举完成行（`dir=… entries=… elapsed_ms=…`），位置仍在 `rebind`/`bind_declared` 一处出口，照 `report_sys_info` 既 `log::info!` 又 `println!` 的惯例。

### T3 spawn_blocking 归位

- `init_mdb` 里 `mdb_membership::rebind` 包 `tokio::task::spawn_blocking`（照 `rebind` 自身注释的要求兑现）。
- `run_read_through_core` 的 `startup_route_directory`（含 `verify_all_headers` 逐路由读头）与启动期那次 `poll_generations` 同样包 `spawn_blocking`（2 s 轮询循环内已是这个写法，启动那次漏了）。
- 只包调用点、不改被调函数签名；`resolve`/`rebind` 保持同步函数。

### T4 验证

- 单元测试（沿用本文件现有 fixture 风格，临时目录真建文件）：
  - `exact_project_binding_never_falls_back_to_same_dbnum` 等在用例改走索引后逐字通过——选主语义回归门；
  - 新增：一个目录含 `x8000_0001`（extract）、`x8000`（master）、非库文件若干、另一项目同号文件——断言选中最高 extract、master 与其余进 `rest`/`shadowed`，顺序与旧实现一致（可对旧实现抽 pure 版本做差分对拍，或直接断言选定结果）；
  - 新增：**每目录一次枚举**的结构性断言——`DirInventory::from_dirs` 是唯一 `read_dir` 点，用例对 `bind_declared` 喂 480 条声明断言完成且不二次触盘（实现侧可给 `DirInventory` 记 `dirs_scanned`/`entries` 计数进 `[ams-phase]` 行，用例断言计数 = 目录数）；
  - 目录不可读 → `problems` 有一行、其余目录照常绑定。
- 源码钉：禁 `pick_within_project`/`locate_declared` 内出现 `std::fs::` 调用（本仓已有多处 `include_str!` 钉）。
- 全量：`cargo test --lib`。
- 真机：在那台卡住的机器上重跑 `web-offline` 后端——`stage=mdb elapsed_ms` 与 `[ams-phase] elapsed_ms` 从「无输出」降到秒级；对照本机同配置。

---

## 4. 验收门

- [ ] 单元测试全绿（选主语义逐字不变 + 单目录一次枚举）
- [ ] `[ams-phase] phase=mdb_scope` 携带 `elapsed_ms` 与目录枚举明细行
- [ ] 共享盘拔线场景下绑定在限时内报出 `problems` 而不是挂死
- [ ] 真机 JEU /ALL（480 库）启动到 `stage=mdb done` 总耗时落回秒级

---

## 5. 范围外

- `SysMetadataResolver::discover`（本就单次枚举 + `file_type()`，合规）；
- 2 s 轮询 `poll_generations` 逐路由 `stat`（成本与库数成正比、每轮一次，属设计内）；
- `run_cli` 主链除 T3 所列点以外的 IO 重排；
- 部署侧绕行（把 `JEU` 项目目录拷到本地盘改 `project_search_paths`）——现场立即可用，不阻塞本修复。

---

## 6. 风险

- **选主语义回归**：`pick_within_project` 的排序/归账顺序决定下游 `shadowed` 内容与「歧义拒绝」报错，必须逐字保留——T4 差分断言是主门；
- `file_type()` 与 `is_file()` 在 junction/符号链接目录项上判定不同：库目录条目按 `parse_extract_file_name` 过滤，非目录名天然出局，实际面为零，但用例钉一条「非文件条目不入选」；
- 目录限时复用扫描线程模式时，超时线程泄漏语义与 `scan_roots` 一致（接收方放弃、线程自行收尾），不新增资源模型。

---

## 7. 相关文件

- `src/data_interface/mdb_membership.rs`（`bind_declared` / `locate_declared` / `pick_within_project` / `rebind` / `DirInventory` 新增点）
- `src/data_interface/db_model.rs`（`init_mdb` 调用点包 `spawn_blocking`）
- `src/data_interface/project_locator.rs`（`scan_roots` 超时模式、注释中的 SMB 挂起实测）
- `src/lib.rs`（`run_read_through_core`：`startup_route_directory` / 首次 `poll_generations`）
- `src/data_interface/mdb_route_directory.rs`（`poll_generations`、`verify_all_headers`）
