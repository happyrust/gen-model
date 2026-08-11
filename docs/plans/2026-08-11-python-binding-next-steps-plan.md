# Python 绑定后续开发计划（V1 落地后的下一步）

- 状态：待评审（2026-08-11，会话 gen-model-1）
- 前置：`docs/plans/2026-08-11-python-binding-api-plan.md`（M1–M4 已全部落地并冒烟通过）
- 现状快照（2026-08-11 15:37 实测）：`import aios_db` / 解析层 / 连接层 / 硬守护 /
  HTTP 客户端全部可用；代码未提交；SurrealDB fork 在 8009 端口运行；9099 端口跑着
  `test-worklspace` 的 issue29 部署包（v0.1.16，同一 SurrealDB、同一 AMS 工程）

## 1. 背景

V1（M1–M4）已交付三层初始化 + 三链路全函数清单 + HTTP 客户端 + 示范脚本，
实测可用。本计划收尾三类事情：

1. **资产入库与环境修复**（P0）——代码还在工作区裸奔、验收数据目录还是坏的；
2. **验证深度补齐**（P1）——正式目录复验、release 构建、跨部署互踩盲区；
3. **长期可维护性**（P2）——取消点、CI 防腐、版本漂移护栏。

## 2. 工作项

### P0-1 资产入库（半天）

- `python/` 全目录 + 方案文档 + 主 crate 配套改动（根 `Cargo.toml` workspace 化、
  `acquire_process_instance_lock` / `staging::query_valid_insts` 改 pub）+
  README 类名笔误修复（`Client` → `AiosClient`，已改好），整理成一个独立 commit，
  与工作区里其他在途改动（staging / batch_worker 等）分开提交。
- `.gitignore` 补 `/python/.venv/`（当前只靠 uv 写在 `.venv` 内部的自我忽略）。
- 验收：`git status` 里 python 相关全部干净；推送前 vendor patch 用
  `Toggle-LocalDeps.ps1` 关回，pre-push 守卫通过。

### P0-2 ams-8009 数据目录恢复（1–2 天，含决策）

- 现状：`.surreal/ams-8009` 被 PATH 上的 SurrealDB 3.x 打开过，RocksDB 写入了
  format_version 7 的 SST，fork 2.1.4 无法打开；M4 验收改在
  `ams-7997-e3d-test-20260805` scratch 副本上完成。
- 三选一（建议顺序）：
  1. **快照顶替**——若有事故前快照，成本最低；
  2. **重建基线**——用 `sync.baseline` 全库重解析，最干净但耗时（可后台跑）；
  3. RocksDB 工具降级修复——风险最高，不建议。
- 防再踩：`.surreal/` 下放 README 警示；`Start-Surreal8009.ps1` 启动前校验
  serve 二进制版本，拒绝 3.x 打开 2.x 数据目录。
- 验收：fork 2.1.4 正常打开；连接层 `watermark` / `pe` 计数与事故前基线一致。

### P1-1 正式目录全量复验（半天，依赖 P0-2）

- 停掉在跑服务后（注意：当前 9099 是 `test-worklspace` 的 issue29 包，
  停之前先确认没有其他会话在用它），在恢复后的正式数据目录上按顺序跑
  `smoke_m1..m4` + 3 个 demo 脚本，结果回填 V1 计划文档。
- 顺带消化 M3 冒烟遗留的 195 行 data pending（`incr.drain_data()`）。
- 验收：4 个冒烟全绿；pending 表清零。

### P1-2 release 构建验证（半天，可与 P1-1 并行）

- M3/M4 冒烟用的是 debug 构建；生成类重操作（`gen_dbnum` 整库）按 V1 决策
  锁定 release。首次 `maturin develop --release` 走共享 target 缓存，验证
  OCC release 编译与 abi3 wheel 正常，记录首编耗时。
- 验收：release pyd 上 `model.ensure(force=True)` 与 debug 结果一致、耗时下降。

### P1-3 跨部署互踩防护（1 天）

- 本次实测暴露的盲区：单实例锁按「项目根」隔离，`test-worklspace` 部署包与
  本仓库各持各的锁，却写同一个 SurrealDB（8009）+ 同一工程——锁挡不住互踩。
- 方案：`full_init` 在拿锁后、初始化前，对 DbOption 配置的 web 端口 + 已知
  常用端口（8022 / 9099）做一次 `/api/v1/health` 探测；发现**同工程**
  （health.project 一致）的活服务直接报错拒绝，`force=True` 可显式跳过。
  纯绑定侧改动，不动服务端。
- 验收：9099 服务在跑时 `full_init` 报错且文案指名端口与工程；停服后正常。

### P2-1 长任务取消点（1–2 天，按需）

- `gen_dbnum` / `room.drain` / `sync.baseline` 三个最长操作加协作式取消：
  Python 侧捕获 KeyboardInterrupt → Rust 侧 AtomicBool 检查点（refno 边界处检查）。
- 验收：整库生成中 Ctrl+C 在下一个 refno 边界内返回，库内无半写状态。

### P2-2 CI 防腐（半天）

- `windows-tests.yml` 加一个 job：`maturin build --release`（abi3 wheel）+
  仅解析层的最小冒烟（不依赖 SurrealDB / 测试工程，用仓内样例数据文件）。
- 验收：CI 绿；wheel 作为 artifact 可下载给同事。

### P2-3 版本漂移护栏（2 小时）

- `aios_client` 连接时比对 `health().version` 与内置 expected_version，
  不一致打 warning（不报错）。回应本次实测：0.1.13 绑定 vs 0.1.16 在跑服务。
- 验收：对 9099（0.1.16）连接打出 warning；对本仓库自建服务无告警。

### P0-3 纯 Python 闭环缺口补齐（已落地，2026-08-11 追加）

用户目标「纯 Python 跑通解析→生成→房间」核查后新增的工作项（缺口分析与
决策见会话 gen-model-1）：

- **落地内容**：新 `spatial` 子模块（`status` / `reconcile` / `persist` /
  `rebuild`）+ `incr.resolve_window`（V1 计划遗漏项）/ `incr.drain_side_effects` /
  `incr.queue_status` + `room.code` / `nodes` / `names`（fn:: 直通）。
- **动机**：提交后副作用三件套（SystDerived / RefRevMaintain / SpatialReconcile）
  只活在 batch worker 出队门里——`execute_manual` 队列闭环自动收尾，但零售组合
  （`apply_file` → `drain_data` → `room.drain`）会把副作用滞留 pending 表、
  内存空间树不落盘。
- **附带修正**：`connect` 补灌编译期内置函数快照（与 `run_cli` 同款，D11/ADR-010
  的 hd/hh 矫正——否则连接层 `fn::room_code` 停在 hh 语义）；`full_init` 的
  hd 重放改走同一内置入口；`selfcheck_surreal_functions` 失败改为中止初始化
  （对齐 `run_cli` 的 `?` 传播）。
- **验收**：`scripts/smoke_m5.py` 连接层段全绿（守护 8 项 + 只读 5 项，
  房间穿越实测 R304/R346）；执行层段（`--full`）待停服后随 P1-1 一起跑。

## 3. 里程碑

| 阶段 | 内容 | 预估 | 备注 |
|---|---|---|---|
| M5 资产与环境 | P0-1 + P0-2 + P0-3 | 2–3 天 | P0-1 ✔ / P0-3 ✔（2026-08-11）；P0-2 需先定恢复策略 |
| M6 验证补齐 | P1-1 + P1-2 + P1-3 | 2 天 | P1-2 提前起编译与 P1-1 并行；P1-1 顺带跑 smoke_m5 --full |
| M7 可维护性 | P2-1 + P2-2 + P2-3 | 2–3 天 | 可按需裁剪，P2-1 优先级最低 |

## 4. 不做什么（明确出界）

- 不迁移存量 `output/*.py` 旧探针（V1 已定）。
- 不做 wheel 对外发布 / PyPI；内部 CI artifact 即止。
- `todo.md` 里的主 crate 项（生成效率优化 / 全文检索 / 表达式求解 bug 等）
  不进本计划，另立条目。

## 5. 风险

| 风险 | 对策 |
|---|---|
| ams-8009 重建基线耗时超预期 | 优先快照顶替；重建放后台跑，不阻塞 M6 其余项 |
| release 首编 OCC 耗时长 | P1-2 与 P1-1 并行，提前起编译 |
| health 探测误伤（端口被别的程序占用） | 响应必须是合法 health JSON 且 project 匹配才拒绝 |
| 停 9099 服务影响其他会话 | 停服前用 `aios_client.tasks()` 确认无 running 任务 |
