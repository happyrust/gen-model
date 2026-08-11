# Python 调试接口（PyO3 绑定 + HTTP 客户端）方案

- 状态：已评审通过（2026-08-11，会话 gen-model-4，逐题决策见 §2）
- 日期：2026-08-11
- 目标读者：需要用 Python 脚本快速调试解析 / 模型生成 / 增量计算的开发者
- 相关文档：`docs/specs/web-service-api.md`（HTTP 面）、`docs/specs/manual-model-update.md`（手动更新语义）、ADR-011（队列合流）、ADR-017（暂存窗口）

## 1. 背景与目标

`aios-database` 的三条核心链路（dabacon 文件解析、模型生成、增量计算）目前只有三种调试形态：Rust 集成测试（每次改动都要编译）、HTTP API（粗粒度入口，细粒度内部函数不可达）、手写 Python 探针（在纯 Python 里复刻 dabacon 页级二进制解析，如 `gm_noun_caps_probe.py`——重复劳动且随 Rust 侧演进漂移）。

本方案给 Rust 内部接口加一层 Python 绑定，让调试脚本直接调用与生产完全同源的实现，消灭手写解析复刻；同时补一份 HTTP 薄客户端盖住「在跑服务」的观察面。

## 2. 决策记录（grill 逐题结论）

| # | 问题 | 结论 |
|---|------|------|
| Q1 | 总体形态 | **PyO3 全库绑定**（新建 `aios-py` 绑定 crate，maturin 构建）；不走「HTTP 加调试端点」路线，避免把调试面污染成服务 API |
| Q2 | 仓库形态 | **仓内 workspace 成员 `python/`**：根 `Cargo.toml` 加 `[workspace]`，`default-members` 钉在根 package。`[patch]`（vendor 本地重定向）天然作用于全图，共享 `Cargo.lock` 与 `target/`，OCC 只编一次 |
| Q3 | 初始化分层 | **三层 + 硬守护**（见 §3）；mutating 函数未 `full_init` 直接抛异常 |
| Q4 | 异步桥 | **同步包装**：绑定 crate 持有进程级 tokio 多线程 Runtime，每个函数 `block_on` 并在等待期间释放 GIL；已知代价是长任务期间 Ctrl+C 不能立刻中断 Rust 侧 |
| Q5 | 数据交换 | **serde → Python dict 直传**（pythonize），与 HTTP API「原样透传、单一权威」同哲学；refno 统一 `"a/b"` 字符串；大二进制（mesh 顶点等）走 `bytes` 或文件，不进 dict |
| Q6 | V1 范围 | **三链路全清单**（见 §4），含 5 个需在 Rust 侧新补的小入口（★） |
| Q7 | 构建形态 | **单一全量构建**：feature 与服务默认集一致（ws + gen_model + manifold + occ + project_hd），不搞 parse-only 轻变体（未测试的 feature 组合风险）；pyo3 开 abi3；日常 `uv run maturin develop --release` |
| Q8 | HTTP 客户端 | **要**：`python/aios_client.py`，REST 9 端点 + WebSocket tasks 订阅，纯 Python 零 Rust 改动 |
| Q9 | 落地顺序 | **M1 骨架 → M2 观察面 → M3 执行面 → M4 新入口**（见 §6） |

## 3. 初始化分层与单实例锁

事实依据：`run_app`/`run_cli` 启动即拿 Windows deny-share 锁（`<项目根>/.gen-model.instance.lock`，见 `src/lib.rs` `acquire_process_instance_lock`），同一项目同时只能有一个「完整初始化」进程。Rust 集成测试的先例（`tests/common/mod.rs`）证明不拿锁、只连 `SUL_DB` 即可直接调内部函数，但与在跑服务**并发写**（staging 窗口 / 队列 / pending 表）会互踩。

| 层 | 入口 | 能做什么 | 锁 | 与在跑服务共存 |
|---|---|---|---|---|
| 解析层 | `aios_db.parse.*` | 纯文件解析（头 / 会话 / collect_changes / 字典） | 无 | ✔ 完全共存 |
| 连接层 | `aios_db.connect(config)` | 连 `SUL_DB` + 加载 `DbOption`，只读查询 | 无 | ✔ 可边跑边查 |
| 执行层 | `aios_db.full_init(config)` | mutating 管线（增量 apply / drain、模型生成、房间重算） | 拿单实例锁 | ✖ 必须先停服务 |

- `full_init` 等价 `run_cli` 前置段：拿锁 → `define_common_functions` → `selfcheck_surreal_functions` → `ensure_increment_state_storage` → `init_inst_relate_indices`（含回填/清扫） → 空间树加载；**不启动** watcher / 批次 worker / Web 服务——Python 脚本自己就是驱动者。
- 硬守护：绑定层维护初始化状态机，mutating 函数在未 `full_init` 时抛 `RuntimeError`，错误信息明确说「停服务 + full_init」。误用第一时间炸在脚本里，而不是事后炸在数据里。
- 服务进程内存中的任务注册表 / 队列快照（TaskRegistry）Python 进程拿不到，一律走 HTTP 客户端问在跑的服务。

## 4. V1 函数清单

★ = 需要在 Rust 侧新补的小入口，其余全部是现成 pub 函数的直接包装。

### 4.1 解析层 `aios_db.parse`（无连接、无锁）

| Python API | Rust 来源 |
|---|---|
| `db_header(path)` | `DbPageBasicInfo` 头解析 |
| `is_db_file(path)` | `increment_manager::is_candidate_db_file` / `is_pdms_db_file_name` |
| `sessions(path)` | 会话页读取（saves-and-times 契约） |
| `collect_changes(path, start, end)` | `IncrementPipeline::collect_changes`（纯函数、无副作用） |
| ★ `element(path, refno)` | 从文件直读单元素属性 dump（不进库） |
| ★ `noun_dict(attlib_path)` | attlib 字典 / noun 能力矩阵（替代手写 `gm_noun_caps_probe.py`） |

### 4.2 连接层 `aios_db.db`（`connect` 后可用，只读）

| Python API | Rust 来源 |
|---|---|
| `query(sql, binds=None)` | `SUL_DB.query` 直通（万金油） |
| `by_name(name, dbnum=None)` / `child_of(parent, noun, dbnum=None)` | 对齐 `tests/common/mod.rs` 先例 |
| `pe(refno)` / `members(refno)` / `owner_chain(refno)` | 复用 `query_service::QueryService`（`e3d_mcp` 同源） |
| `inst(refno)` | inst_relate 行 + aabb |
| `watermark(dbnum)` | `SesnoRangeResolver::query_watermark` |
| `dbnum_statuses(project=None)` | `AiosDBManager::dbnum_statuses` |
| `preview_manual_update(project=None)` | `preview_manual_update`（只读预览） |
| `pending_model_units()` | `load_pending_model_units` |
| `window_blocks()` / `root_attempts(dbnum)` | `staging::attempts::load_window_blocks` / `load_root_attempts` |

### 4.3 执行层 `aios_db.incr` / `.model` / `.room` / `.sync`（`full_init` 后可用）

| Python API | Rust 来源 |
|---|---|
| `incr.resolve_window(dbnum)` | `SesnoRangeResolver::resolve` |
| `incr.apply(dbnum, start=None, end=None)` | `IncrementPipeline::apply` |
| `incr.execute_manual(project, dbnums=None)` | `enqueue_manual_update` + `batch_worker::drain_queue_until_empty`（扫描 + 入队 + 当场消费到空） |
| `incr.drain_data()` | `model_update_pending::drain_data_phases` |
| `model.ensure(refno, force=False)` | `ensure_model_generated`（与 HTTP `/model/ensure` 同源） |
| `model.gen(refnos, replace=False)` | `process_meshes_update_db_deep` |
| `model.gen_dbnum(dbnum)` | `process_meshes_by_dbnos` |
| `model.update_aabbs(refnos)` | `update_inst_relate_aabbs_by_refnos`（返回真变化集） |
| ★ `model.export_obj(refno, dir)` | 接 `debug_obj_export` 能力，导出 OBJ 目视 |
| `room.build_all()` | `build_room_relations` |
| `room.drain()` | `model_update_pending::drain_rooms` |
| ★ `sync.baseline(dbnum)` | 单库按需基线解析（首次入库） |

### 4.4 HTTP 客户端 `python/aios_client.py`（纯 Python）

按 `docs/specs/web-service-api.md` 1:1 封装：`health` / `update/preview` / `update/execute` / `tasks`(+详情) / `model/ensure` / `update/pending-units`(+retry) / `dbnums` / `queue`(+pause/resume)；WebSocket 订阅 tasks 事件（`task_started` / `task_progress` / `task_finished`），提供 `watch_tasks()` 迭代器。

分工口径：**服务在跑 → 用 `aios_client` 观察；服务停了 → 用 `aios_db` 深入。** 两边字段同源（同一份 serde 输出）。

## 5. 布局与构建

```
gen-model/
├── Cargo.toml            # 加 [workspace] members=["python"]，default-members 钉根 package
├── python/
│   ├── Cargo.toml        # aios-py 绑定 crate（cdylib），path 依赖 aios_database
│   ├── pyproject.toml    # maturin backend；uv 管理 venv
│   ├── src/lib.rs        # pyo3 模块：parse / db / incr / model / room / sync 子模块
│   ├── aios_client.py    # HTTP 薄客户端（REST + WS）
│   ├── aios_db.pyi       # 类型存根（M4）
│   └── scripts/          # 调试脚本存放处（示范脚本也放这里）
```

- pyo3 取 maturin 添加时的最新稳定版，开 abi3；tokio Runtime 进程级单例；`pythonize` 做 serde→dict。
- feature：`aios_database` 以默认集引入（与服务一致），单一构建形态。
- 日常命令：`uv run maturin develop --release`（几何/布尔运算 debug 构建慢到不可用，锁定 release）。
- 存量 `output/*.py` 探针不迁移；新脚本一律用新库。

## 6. 里程碑与验收

| 阶段 | 内容 | 验收 |
|---|---|---|
| M1 骨架 | workspace 化 + `aios-py` 最小模块：`connect` + `db.query` + `parse.db_header/is_db_file/sessions/collect_changes` | Python 三行拿到真实库文件的增量窗口变更，与 Rust 侧结果一致 |
| M2 观察面 | 连接层全量（QueryService 复用 / 水位 / 预览 / pending / 阻断）+ `aios_client.py`（REST+WS） | 服务在跑时，Python 同时走只读查询与任务进度订阅 |
| M3 执行面 | `full_init` + 硬守护 + `incr.*` / `model.*` / `room.*` 全部现成函数绑定 | 停服务后，纯 Python 脚本跑完「解析→增量应用→单根生成→房间 drain」完整链路 |
| M4 新入口 | 5 个★（`parse.element` / `noun_dict` / `export_obj` / `sync.baseline`）+ `.pyi` 存根 + README | 用新库重写三个典型探针场景作为示范脚本 |

## 7. 实施注记（M1 落地时对计划的修正，2026-08-11）

1. **refno 字符串形态改为 `a_b`**（原计划 `a/b`）：与库内 `pe:` record id 一致，
   调试脚本拿到即可直接拼下一条 SurrealQL；web 层的 `a/b` 形态仅在 HTTP 客户端出现。
2. **解析层并非零配置**：`collect_changes` 深处读全局 DbOption（debug 选项），
   配置不可达直接 panic。新增 `aios_db.set_config(path)`（等价设 `DB_OPTION_FILE`），
   必须在任何触碰配置的调用之前执行；三层表的「解析层」修正为「需配置文件、不需连接」。
3. **OCCT 是动态链接**（TK*.dll + tbb12/jemalloc/freetype 散在多个目录），而
   Python 3.8+ 加载扩展模块不查 PATH。绑定改为 maturin 混合工程：Rust 扩展名
   `aios_db._aios_db`，外层 `pysrc/aios_db/__init__.py` 先把 PATH 上所有目录
   `os.add_dll_directory` 注册（与主程序 exe 的 legacy 搜索对齐）再导入扩展。
4. **`connect(config=None, cwd=None)` 增加 `cwd` 参数**：`init_surreal` 内部的
   `define_common_functions` 按 CWD 相对路径读 `resource/surreal/`，通常传仓库根。
5. **debug 构建已够 M1/M2**（全量 44s / 增量 ~10s，走全局 `D:\Rust\target` 缓存）；
   `--release` 留到 M3 生成类操作前再首编（OCC release 首次全量编译耗时长）。
6. `db.query` 干净 JSON 的实现路径：`take::<surrealdb::Value>` → `into_inner()`
   →核心 `sql::Value::into_json()`（SDK 包装类型的 Serialize 是 tagged 枚举，不可直用）。

M2 落地补充（2026-08-11）：

7. **元素查询不走 `QueryService`**：其 `e3d.element.*` 工具背后是 E3D TTY 驱动
   （拉起真实 E3D 进程跑 PML），是另一套环境依赖。连接层的 `pe / members /
   owner_chain / inst / by_name / child_of` 全部直查 SurrealDB（`inst_relate`
   是 `pe->inst_relate->inst_info` 边，FETCH 展开 aabb/world_trans）。
8. `dbnum_statuses` / `preview_manual_update` 通过进程级惰性单例
   `AiosDBManager`（首次构造做监控目录解析 + 头扫描 + 解析簿记，默认 feature
   下无 MQTT/MySQL 副作用），与服务端预览端点同源同义。
9. HTTP 客户端 REST 用标准库 urllib（零依赖），`watch_tasks()` 懒加载
   `websocket-client`；已对真实拉起的 Web 服务（`cargo run --bin aios-database
   --features http_api`）验收 REST 6 只读端点 + WS 订阅/心跳。

M3 落地补充（2026-08-11）：

10. **`full_init` 在绑定内复刻 `run_cli` 前置段**（锁→连接→hd `fn::room_code`
    重放→selfcheck→增量状态表→dbnum 事件→inst_relate 索引/回填/清扫→空间树→
    manager），主 crate 仅一处改动：`acquire_process_instance_lock` 改 `pub`。
    与 `run_cli` 的漂移风险由「顺序注释 + 冒烟」看住。
11. `drain_rooms` 实际返回 `DrainReport`（无 serde 派生，手工转 dict）；
    `queue_pause/resume` 走 `BatchScheduler::set_paused_persistent`（与 HTTP 同源）。
12. **`db.inst` 语义修正**：实例边挂在具体图元上，交付单元根自身通常没有直接
    `inst_relate`；查询改为 `in = pe:refno OR anc CONTAINS <u64>`（`anc` 是
    RefU64 的 u64 数组，`(ref0<<32)|ref1`）。
13. `gen` 是 edition 2024 保留字：Rust 侧函数名 `gen_models` + `#[pyo3(name="gen")]`。
14. M3 冒烟实录（AMS 测试工程）：硬守护如期拒绝；rollback 现场 `apply_file`
    正确判 `up_to_date`（水位 238 > 文件 102）；`ensure` 幂等复用 /
    `force=True` 真重生成 17 实例（debug 构建 1.5s）；`room.drain` 消化 256 个
    真实积压目标（全成功）。积压的 195 行 pending 留给使用者自行 `drain_data`。

M4 落地补充（2026-08-11）：

15. **4 个★新入口全部落地**（`parse.element` / `parse.noun_dict` /
    `model.export_obj` / `sync.baseline`）+ `.pyi` 存根（`pysrc/aios_db/*.pyi`
    + `py.typed`）+ `python/README.md` + 3 个示范脚本（`demo_noun_caps` /
    `demo_element_diff` / `demo_export_obj`，替代典型手写探针场景）。
16. **`model.export_obj` 放宽为连接层可用**（原计划在执行层）：它只读库 + 读
    mesh 文件 + 写用户目录，不碰模型/增量数据，设 full_init 门只会挡住
    「服务在跑时导出目视」这个最常见用法。
17. `parse.element` 基于 `PdmsIO::search_latest_refno`：**最新索引里没有 ≠
    从未存在**——被删元素要用 `sesno=` 读历史版本（M4 冒烟里 24381_179751
    即此形态：会话 100 被移动、101 移回、之后被删）。
18. `sync.baseline` 直接绑定现成公开入口
    `AiosDBManager::initialize_project_dbnum_baseline`（与自动 watcher /
    手动更新同源）；主 crate 第二处改动：`staging::query_valid_insts` 改
    `pub`（export_obj 复用其「有效实例」口径，`world_trans × inst.transform`
    变换到世界坐标，缺法线时降级 `f a b c` 面）。
19. `noun_dict` 输出 `AttrDataFile::all_noun_capabilities()`（1384 noun ×
    86 字段，含 base_type 继承与默认表兜底），比原 Python 探针的 5 字段
    NounFlags 更全。
20. **环境事故记录**：M4 验收时发现 `.surreal/ams-8009` 被 PATH 上的
    SurrealDB 3.x 打开过，RocksDB WAL 恢复写入了 format_version 7 的 SST，
    fork 2.1.4 无法再打开（`Corrupt or unsupported format_version: 7`）。
    验收改在 `ams-7997-e3d-test-20260805` 的 scratch 副本上完成；README
    已加告警。恢复方案待定（重建基线 / 快照顶替 / 尝试修复）。

## 8. 风险与对策

| 风险 | 对策 |
|---|---|
| maturin 与共享 target 不合（OCC 重编） | M1 第一件事验证；必要时显式设 `CARGO_TARGET_DIR` |
| Python 进程 mutating 与在跑服务互踩 | 三层硬守护 + 单实例锁（执行层拿锁，锁被服务持有时 full_init 直接报错） |
| 长任务期间 Ctrl+C 不能中断 Rust 侧 | 已知代价，文档写明；后续可按需给最长的操作加取消点 |
| pyd 里全局单例与服务行为漂移 | feature 集与初始化序列严格对齐 `run_cli` 前置段，不自创第二套初始化 |
| `[patch]` vendor 开关对绑定失效 | workspace 化后天然生效；`Toggle-LocalDeps.ps1` 行为不变 |
| pre-push 守卫（Cargo.lock source 行） | workspace 共享同一 Cargo.lock，守卫逻辑不受影响，M1 验证一次 |
