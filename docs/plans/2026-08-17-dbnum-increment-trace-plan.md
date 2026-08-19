# 增量链路 dbnum 追踪器开发计划

状态：**待裁决**（D1–D8 未决，grill 进行中）

日期：2026-08-17
提出背景：2026-08-17 两次 live 轮次都停在「第一道断言」，且两次都因为
**关键中间量在进程内不可见**而无法当场定位。

引用的既有决策：ADR-001（水位是承诺）、ADR-011（手动与自动共用一条队列、同一份
谓词）、ADR-021（水位必须有数据支撑，回退默认整库重建）、ADR-022（净窗口收集）、
ADR-023（启动重扫修复无数据支撑的应用水位）。本计划**不修改**任何既有决策。

## 1. 目标与不做什么

**目标**：给增量链路加一个按 dbnum 过滤的**只读追踪器**，让「这个库这一轮到底
经历了什么」在不改代码、不加断点的前提下可复核。

**不做什么（本期显式出界）**：
- 不改变任何判定、路由、入队、执行行为。追踪器只观察。
- 不做范围收窄。`manual_db_nums` 因为 issue #10 已被剥夺增量否决权
  （`handwritten_dbnum_lists_no_longer_narrow_the_increment_scope` 钉着），
  本计划不得以任何形式把它复活。
- 不在本轮修复 §2 的两个缺陷——本轮只保证它们**可被诊断**。

## 2. 触发本计划的两个现场

| # | 现场 | 看不见的中间量 |
|---|---|---|
| A | db7999 夹具九场景全 FAIL：`saved session N is absent from data task merged_sesnos`。水位 167→184 正常推进，数据全对，只有批次回执那一栏空。正式目录与隔离副本各复现一次，会话号不同、形状一致。 | 入队时冻结的 `previous_observed_sesno`。它决定 `sessions_merged_after` 过滤掉什么，但只活在内存里 |
| B | `test_rollback_reinit.py` 三条用例全部卡在模块级引导：7998 `file_latest=12` 而 `applied=0`。回执里没有任何提到 7998 的告警——没拒绝、没报错、也没干活 | 该 dbnum 有没有入队、`ScanGate` 判成了什么、路由选了基线还是增量、冻结点复核说了什么 |

两个现场的共同点：任务终态只在服务 HTTP 面上，**不落库**；服务一拆栈，证据全没。

## 3. 现状事实（已核实，不必再查）

| 事实 | 位置 |
|---|---|
| `src/main.rs` **没有** clap 子命令树，只调 `run_app` | `src/main.rs:64` |
| 仓内 CLI 惯例是 `src/bin/` 下 27 个探针 bin | `src/bin/` |
| HTTP 面是 axum，路由集中在一处 | `src/web_service/mod.rs:273` |
| 环境变量惯例：`AIOS_*` 常量 + `parse_bool_flag`，**认不出的值退回配置值，不猜** | `src/options.rs:236/264/357` |
| 进程级状态惯例：`static X: LazyLock<Mutex<..>>` + 小 `pub(crate) fn` 取值，经 `/health` 露出 | `batch_worker.rs:246` 的 `BATCH_FAILURES` |
| 日志淹没有先例与对策：范围外的库聚合成一句，不逐条刷屏 | `increment_manager.rs:1795` |

## 4. 六个追踪点（覆盖 §2 两个现场的全部盲区）

| # | 点 | 落点 | 要记的量 |
|---|---|---|---|
| T1 | 扫描裁决 | `increment_manager.rs::scan_and_check_file` | 旧 `applied`、库里存的 `file_latest`、本次观察到的 `file_latest`、`FileAnomaly`、`ScanGate` |
| T2 | 入队 | `manual_update.rs` 手动侧 + `batch_scheduler::enqueue` | `intent`、窗口两端、**`previous_observed_sesno`**、`Enqueued` 四态 |
| T3 | 冻结点 | `batch_queue::freeze_next` / `record_frozen_end` / `batch_reroutes_to_initial_load` | 重扫后的真右端、是否改走首次导入 |
| T4 | 路由 | `manual_update.rs::execute_one_dbnum` | `needs_initial_load`、pe 存在性探针结果、空基线凭据 |
| T5 | 收集 | `increment_pipeline.rs::collect_window` | 口径（Replay/Net）、`session_sesnos`、算出的 `merged_sesnos` |
| T6 | 终态 | `execute_one_dbnum` 收口 | `status`、`applied` 旧→新、`changed_elements`、失败原因 |

T2 直接回答现场 A，T1/T3/T4 直接回答现场 B。

## 5. 铁律

**只能看，不能筛。** 追踪调用不得出现在任何 `if` / `match` 的判定表达式里，不得
新增 `continue` / 提前返回 / `_ =>`。它叫「监控变量」而不是「范围变量」正是这个
意思——`manual_db_nums` 当年就是从「方便调试」长成隐形黑名单的，不能重演。

这条要有源码顺序/形状断言钉住，不能只写在文档里。

## 6. 验收标准

1. 关掉追踪器时，链路的可观察行为与本计划实施前逐字节相同（现有单测全绿）。
2. 打开 `AIOS_TRACE_DBNUM=7998` 重跑现场 B，能从输出直接读出「7998 这一轮为什么
   没有建立水位」，不需要再加任何临时打印。
3. 打开 `AIOS_TRACE_DBNUM=7999` 重跑现场 A，能直接读出入队时冻结的
   `previous_observed_sesno` 与最终 `merged_sesnos`，据此判定缺陷在哪一段。
4. 纯函数单测覆盖：目标集解析（含非法值退回）、节流、缓存淘汰。
5. 「只看不筛」有断言钉住，回退到把追踪当条件用就会红。
6. `cargo fmt` + `cargo check` 过。

## 7. 待决问题（grill 清单）

| # | 问题 | 推荐 |
|---|---|---|
| D1 | 输出介质：stdout 纯文本行 / stdout JSON 行 / JSON 行 + 进程内环形缓存 + HTTP 端点 | JSON 行 + 环形缓存 + HTTP。两个现场都栽在「服务拆栈后证据没了」，只写 stdout 等于把同一个坑留着 |
| D2 | 开关形态：只环境变量 / 只配置键 / 两者（env 覆盖 config） | 只环境变量。它是调试器不是运行策略，进了配置就要进 `Test-DbOptionDrift` 的漂移检查，还多一处「装置状态与预期不符」 |
| D3 | CLI 形态：新 probe bin / 给服务加 clap 子命令 / 只 HTTP | 新 probe bin。`main.rs` 压根没有子命令树，硬加一套是为一个调试工具改服务入口 |
| D4 | 覆盖点范围：先做 T1/T2/T4 三个点 / 一次做全六个 | 一次做全六个。少任何一个，下次卡住就要再改一轮同样的六个文件 |
| D5 | 要不要 `all` 档 | 要，但带上限与节流。258 个库逐条刷屏会把追踪器自己变成噪声源 |
| D6 | trace 记录要不要落库 | 不落。落库就有写路径，与「只看不改」冲突；证据靠 HTTP 拉出来写进 `docs/evidence/` |
| D7 | 「只看不筛」怎么钉 | 源码形状断言 + 纯函数单测，两条都要 |
| D8 | 本轮范围：只做工具 / 顺带修那两个缺陷 | 只做工具 + 用它产出两份诊断。修复要动 `src/data_interface/` 下五个未提交文件的那条链路，混在一轮里说不清是谁修好的 |

## 8. 裁决记录

- **D1（2026-08-17）：采纳推荐**——JSON 行 + 进程内环形缓存 + HTTP 端点三者都做。
  理由是两个触发现场都栽在「服务拆栈后证据没了」，只写 stdout 等于把同一个坑原样
  留着；量本身是结构化的（两个会话号、一个枚举、一个数组），拼成句子再用正则拆
  回来是白费力气。
- **D2（2026-08-17）：否决推荐，改走命令行**——不引入环境变量，开关以参数形式传给
  `aios-database`，挂在一个子命令下。后果：`src/main.rs` 目前完全不看 argv
  （无条件 `run_app(None)`），本决议要求在那里新建 clap 子命令树，并保证**无参调用
  仍旧起服务**——仓内所有脚本、`l3_suite` 夹具、部署包都是裸调它的。同时意味着每个
  拉起服务的地方要显式传参才能开追踪（环境变量本可以零改动继承），这笔改动成本
  由 D3 一并定形。
- **D3（2026-08-17）：扩大为「限定 + 追踪」双职责开关**——用户裁定这个参数还要能
  **把本轮增量检查圈到单个 dbnum**，只分析它。定名 `--debug-dbnum`，形状：

  ```
  aios-database                           # 无参：照旧全范围起服务
  aios-database serve --debug-dbnum 7998  # 只检查 7998，并全程追踪它
  aios-database trace --dbnum 7998        # 客户端，走 HTTP，不拿实例锁
  ```

  机器已存在：`execute_manual(dbnums=[..])` 就是这个语义，缺的只是 CLI 说法与让
  启动重扫 / watcher 也听它。`trace` 子命令必须是纯客户端——`run_app` 一上来就
  `acquire_process_instance_lock`，走那条路的话服务跑着时它根本执行不了，而那正是
  唯一想用它的时候。

  **粒度（同轮裁定）**：只圈**数据批次**（扫描 / 入队 / 应用）。模型生成与房间重算
  不圈——W3 要答的「5 类模型任务漏根」正是模型任务规划级断言，圈掉就全测不成。
  **SYS meta 不受限制**：MDB 的成员名单存在 SYST/DICT 库里，不解它就解不出「7998
  在不在范围内」，圈掉只会得到一个「什么都没发现」的假现场。

  §5 那条「只能看，不能筛」的铁律**随本裁决作废**，由 D7 重写为「筛必须响亮」。
- **D4（2026-08-17）：采纳推荐**——一次做全六个追踪点。少任何一个，下次卡住就要
  再改一遍同样的六个文件。
- **D5（2026-08-17）：采纳推荐**——`--debug-dbnum` 收逗号列表（`7998,8000`）。
  跨库交互的缺陷盯单个库看不出来。
- **D6（2026-08-17）：采纳推荐**——trace 不落库。落库就多一条写路径；证据靠
  `aios-database trace` 拉出来存进 `docs/evidence/`。
- **D7（2026-08-17）：三条护栏全要**——
  1. 调试排除的理由字符串与 `out_of_scope_reason` 的输出**无交集**，且必含子串
     `--debug-dbnum`；回退到复用那句就红。
  2. 调试限定为空时，入范围判定**逐位等于**本特性引入前的行为。
  3. 开关非空时，回执 `warnings` 里**一定**有那句声明——只有 `println!` 而调用方
     回执里看不见的报告，视同没有报告（宪法·静默失效）。

  第 3 条比前两条难写（要造一个带开关的回执对象来断言），但它是唯一直接对准
  issue #10 那个病的：前两条管「说法对不对」，它管「到底说没说」。
- **D8（2026-08-17）：采纳推荐**——本轮只做工具，随后用它产出两份诊断
  （merged_sesnos 空、7998 `applied=0`）。两个缺陷的修复各自单独一轮：那条链路上
  还压着五个未提交文件，混在一轮里说不清是谁修好的。

## 9. 任务

| # | 内容 | 文件 | 可并行 |
|---|---|---|---|
| T-1 | 调试域 + trace 环形缓存 + JSON 行发射器（含目标集解析、节流、淘汰） | `src/data_interface/debug_scope.rs`（新） | – |
| T-2 | 入范围判定接入调试限定，理由字符串独立 | `src/data_interface/increment_manager.rs` | 依赖 T-1 |
| T-3 | 六个追踪点接线 | `increment_manager.rs`、`manual_update.rs`、`batch_scheduler.rs`、`batch_queue.rs`、`batch_worker.rs`、`increment_pipeline.rs` | 依赖 T-1 |
| T-4 | 回执声明 + `/health` 一栏 + `GET /api/v1/trace` | `manual_update.rs`、`web_service/handlers.rs`、`web_service/mod.rs` | 依赖 T-1 |
| T-5 | clap 子命令树（无参 = serve；`serve --debug-dbnum`；`trace --dbnum`） | `src/main.rs` | 依赖 T-4 |
| T-6 | D7 三条护栏测试 + 纯函数单测 | 各自模块的 `#[cfg(test)]` | 依赖 T-2/T-4 |
| T-7 | 夹具透传 `--debug-dbnum` | `src/bin/l3_suite.rs`、`src/bin/l3_suite/fixture.rs` | 依赖 T-5 |
| T-8 | 用工具产出两份诊断 | `docs/evidence/` | 依赖 T-1..T-7 |
