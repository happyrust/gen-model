# 增量更新实现与接口审核（2026-07-27）

审核对象：`gen-model` 增量模型更新链路的**实现与对外接口**
审核方式：源码静态追踪（未连任何 SurrealDB 实例、未跑构建/测试）
配图：[`docs/diagrams/2026-07-27-increment-update-audit.drawio`](./diagrams/2026-07-27-increment-update-audit.drawio)（4 页，PNG/SVG 同目录）
关联：ADR-001/003/008/009、`docs/2026-07-26_increment-update-chain-audit-report.md`、`…-round2.md`、`docs/2026-07-27_increment-update-backlog-reaudit-and-fixes.md`

> 与前两轮一样，本报告只记录**结论与依据**，不含代码改动。

---

## 1. 结论

**架构层面这条链路已经是健康的**。前两轮点名的 9 条问题（A1/A2/A3、B1~B6）逐条复核，**全部已修**，且修法都留下了守护测试或注释说明。核心不变量——「applied_sesno 只在数据与模型计划原子收口后推进」——现在有一个真正的收口事务（`render_finalize_transaction`）在兜底，而不再靠调用方自觉。

本轮从**接口契约与失败模式**这两个角度重新过了一遍，查出 **9 条新问题：2 条 Medium、6 条 Low、1 条提示**。没有一条会破坏水位不变量，但 F1/F2 都能让某个 dbnum 的水位**永久冻结**且不带诊断信息——严重度不在于概率，而在于发生后没有自愈路径、错误信息还指错方向。

| 编号 | 严重度 | 一句话 |
|---|---|---|
| F1 | Medium | 崩溃恢复记录与「文件路径迁移」互斥，两个组件对同一事实结论相反，命中即永久阻塞 |
| F2 | Medium | datacenter 语句并入收口事务后成了水位硬前置，而它依赖的 SurrealDB 自定义函数没有任何代码负责加载 |
| F3 | Low | `range_eles` 全量随 `IncrFileSuccess` 常驻，唯一消费方在非默认 feature 里 |
| F4 | Low | `IncrResult` 的两个查询方法全仓无生产调用方 |
| F5 | Medium(性能) | `DeleteCleanup` 逐 refno 建任务、逐任务各走一次子树 BFS，大规模删除接近平方级 |
| F6 | Low | 死信没有主动告警出口 |
| F7 | Low | `drain` 一次性 SELECT 全表待办，无 LIMIT |
| F8 | Low | 自动路径的 MySQL 同步失败只 `println!`，不进 warnings 也不入补偿队列 |
| F9 | 提示 | 手动执行按 project 加锁，但 `drain_non_regen` 是全局的 |

---

## 2. 前两轮问题的现状核对（全部已修）

| 项 | 当前实现 | 依据 |
|---|---|---|
| A1 `delete_work` 失败中断整轮 drain | 已修：`run_one` 刻意做成不可失败，队列行删不掉也只记 `mark_failed` | `model_update_pending.rs:496-517` |
| A2 「整窗口单事务」注释漂移 | 已修：注释改为准确描述分块事务，并写明 ADR-001 不依赖整窗口原子 | `increment_pipeline.rs:91-106`、`:722-731` |
| A3 `datacenter_version` 脱离 finalize 事务 | 已修：改为纯渲染 + 交给 `finalize_attempt` 作为 `window_statements` | `increment_pipeline.rs:837-864`、`model_update_pending.rs:226-244` |
| B1 几何清理中途失败后永久孤儿 | 已修：三条语句包进 `BEGIN/COMMIT` | `helper.rs:48-71` |
| B2 无 `geo_relate` 的 `inst_info` 永不删除 | 已修：引用计数守卫内显式 `delete $old_inst` | `helper.rs:65-68` |
| B3 `record_scan` 早于重复 dbnum 判定 | 已修：两条自动路径都调序为「先判重、再落库观察」 | `increment_manager.rs:771-791`、`:1068-1090` |
| B4 init 递归 / watch 只查一层 | 已修：`init_watcher` 也降为 `max_depth(1)`，并把「只有直属文件参与增量」写成注释约定 | `increment_manager.rs:710-718` |
| B5 死信复活依赖 SET 子句顺序 | 已修：改用 `IF … THEN … ELSE … END` 显式表达，不再依赖求值顺序 | `model_update_pending.rs:119-136` |
| B6 派生根按目录库 dbnum 记账 | 已修：`derived_regen_item` 按根**自身**所属设计库记账，`source_end_sesno = 0` | `model_update_pending.rs:419-443` |

B5 的修法值得单独记一笔：与其加一条断言把书写顺序钉死，不如让语义不再依赖顺序。现在 `attempts` / `last_error` / `source_end_sesno` 三行任意重排结果都一样，这条脆性被从根上消掉了。

---

## 3. 本轮新发现

### F1 · 路径迁移 + 中断的持久化 = 该 dbnum 永久阻塞（Medium）

**位置**：`increment_pipeline.rs:366-392`（`validate_prepared_attempt`）、`:504-511`（唯一调用点）、`model_update_pending.rs:239`（`DELETE increment_update_attempt` 的**唯一**出现位置）、`increment_manager.rs:564-569`（`PathMigrated` 处置）

`increment_update_attempt:{dbnum}` 这行记录只在 `render_finalize_transaction` 里被删除。也就是说：**持久化中断 → 该行留存 → 下一轮走恢复分支**。恢复分支第一件事是 `validate_prepared_attempt`，其中有一条：

```rust
if attempt.db_type != db_type || attempt.file_path != file_path { anyhow::bail!(...) }
```

而同一时间，`check_file_against_state` 对路径变化的判定是 `FileAnomaly::PathMigrated`，自动路径明确按「良性」处理：打一行日志、`record_scan` 把登记路径改成新的、返回 `true` 继续。

于是两个组件对同一事实给出相反结论：**状态机认为迁移无害并且已经接受了新路径，恢复校验却认为路径不符必须中止。** 只要「持久化被打断」和「文件改名 / 换目录」这两件事先后发生，该 dbnum 此后每一轮都在同一处 bail，水位冻结，`IncrResult.errors` 里只有一句 `belongs to type=… path=…`。没有任何自动路径会清掉这行 attempt，唯一出路是手工删表。

对 `Rollback` 阻断是有意为之（水位不能回退），但**路径迁移不是回退**。建议二选一：恢复校验只比对 `db_type` 与 `end_sesno`、路径不一致时告警并接受新路径；或者在 `record_scan` 确认迁移时同步把 attempt 行的 `file_path` 一起改写。

### F2 · datacenter 语句成了水位推进的硬前置，而它的依赖没人负责加载（Medium）

**位置**：`increment_pipeline.rs:787-864`、`model_update_pending.rs:226-244`、`resource/surreal/common.surql:553-554`

A3 把 `datacenter_version` 的状态更新并进收口事务，方向是对的——旧实现「水位推过去了、状态写丢了、此后没有任何窗口会再碰这个元素」确实是数据损坏。但并进来之后，这些语句从「可失败的副作用」变成了「水位推进的必要条件」，而其中一条依赖自定义函数：

```
let $pe = fn::find_ancestor_types(pe:…, ['BRAN','HANG','SUPPO','EQUI','ZONE'])[0];
```

`fn::find_ancestor_types` 定义在 `resource/surreal/common.surql`，而**全仓的 Rust 代码里没有任何地方引用或执行这份脚本**（`rg 'common.surql' --glob '*.rs'` 无结果）。它是一份需要部署时手工灌进去的库级产物。

后果：新环境漏跑 `common.surql`，或有人重命名了这个函数，则**每一个 DESI 窗口的收口事务都会失败**。表现是水位永不推进、同一区间无限重放，而错误信息是 `finalize increment attempt dbnum=… statement failed: …`——完全不指向 datacenter，排查的人会先去怀疑水位和模型队列。

建议：启动期做一次自检（`INFO FOR DB` 或直接试调一次），缺函数时用清晰的错误顶在前面；或者把 `common.surql` 的加载纳入代码路径。

### F3 · `range_eles` 全量随 `IncrFileSuccess` 常驻（Low）

**位置**：`increment_pipeline.rs:41`（字段）、`increment_manager.rs:614`（唯一生产消费方）

`IncrFileSuccess.range_eles` 是整个窗口的完整解析结果。`IncrResult` 会一直持有到整批文件跑完，而唯一读它的地方在 `#[cfg(feature = "sql")]` 的 MySQL 同步里。默认特性下，一个 17k 元素的窗口（如实测的 dbnum=250206）就是纯占内存。

要么把它收窄成 `#[cfg(feature = "sql")]` 字段，要么在 `apply_one` 结束时按需丢弃。

### F4 · `IncrResult` 的两个查询方法已无调用方（Low）

**位置**：`increment_pipeline.rs:66-80`

`all_changed_refnos()` / `geometry_changed_refnos()` 全仓无生产调用点。后者的名字（"eligible for mesh refresh"）会让读者以为下游还有一条按几何刷新的路径，而实际上模型工作早就改由 `model_update_plan` 决定了。`changed_refnos` 本身也只剩 `web_service::events` 拿它取一个 `len()`。

留着这两个方法的代价不是几行代码，是它们描述的是一套**已经不存在的**刷新模型。

### F5 · `DeleteCleanup` 的规模是近似平方级（Medium，性能）

**位置**：`model_update_plan.rs:107-119`（每个净 Deleted 一条工作项）、`model_update_pending.rs:459-461`（逐条执行）、`helper.rs:131-147` + `:17-46`（每条自己走一次 BFS，`SUBTREE_QUERY_BATCH = 20`）

删一棵子树时，父节点和它的每一个后代都会各自被记成一条净 `Deleted`，于是生成 N 条 `DeleteCleanup`。而每条执行时又要沿 `pe_owner` 从自己出发做一次完整的向下 BFS——同一棵子树被重复遍历 N 次，每次还按 20 个一批发 SQL。`drain` 又是逐条串行的。

删一个装了上万子件的 ZONE，这一步足以把整轮 drain 拖成分钟级，而其中绝大部分查询是重复的。

修法两条，可以叠加：建工作项前先做一次祖先归并（后代被祖先覆盖时不再单独建项）；或让 `DeleteCleanup` 接受一批 refno，一次 BFS 把整批的子树并集算出来。

### F6 · 死信没有主动告警出口（Low）

**位置**：`model_update_pending.rs:519-528`（`attempts < 5` 门槛）、`:599-605`（drain 的失败汇总）

`attempts` 满 5 之后，这条工作从自动路径彻底消失：`drain` 不再选中它，因此也不会再进 `failures`，`IncrResult.warnings` 里从此看不到它。只有主动打开手动预览或 `GET /update/pending-units` 才知道有这么一个根一直没生成出来。

一个生成不出来的交付单元在库里就是「没有模型」，这类静默是最难被发现的。建议 `drain` 结束时顺带 `count()` 一下死信条数，打进日志与 WS 摘要。

### F7 · `drain` 一次性 SELECT 全表待办，无 LIMIT（Low）

**位置**：`model_update_pending.rs:540-548`

`render_drain_select` 没有 `LIMIT`，`drain_where` 把结果一次性 `take` 成 `Vec<PendingModelWork>`。与 F5 叠加时，一次大规模删除会把上万行拉进内存再逐条串行跑完，中途没有任何进度可观测、也无法中断。

### F8 · MySQL 同步失败只 `println!`（Low）

**位置**：`increment_manager.rs:611-621`

紧邻的 SYST 派生同步失败时做了三件事：打日志、`push` 进 `incr.warnings`、`fail_syst_jobs` 记进补偿队列。MySQL 同步失败只做了第一件。于是 SurrealDB 与 MySQL 的 `pdms_element` 可以静默分叉，调用方（含 WS 摘要）完全看不出这一批有问题。

至少应该 `push` 进 warnings；如果 MySQL 侧被当作可信数据源，那还需要一条补偿路径。

### F9 · 手动执行按 project 加锁，但 `drain_non_regen` 是全局的（提示）

**位置**：`manual_update.rs:1967-1986`（`ProjectExecGuard`）、`:2735-2744`

`ProjectExecGuard` 只保证「同一个 project 不并发执行」。但 `drain_non_regen` 消费的是全局队列：A 项目的手动执行会顺带把 B 项目的位姿 / 删除 / 级联工作一起做掉。这些动作目前都是幂等的，所以不构成缺陷，但它意味着「手动更新只影响本项目」这个直觉是不成立的——两个项目并发手动更新时，工作归属和进度事件对不上。属于需要写明的边界，不是需要马上改的代码。

---

## 4. 接口契约小结（配图第 ① 页）

审核过程中把对外接口的语义整理如下，供后续会话省去重新摸底：

| 接口 | 契约 |
|---|---|
| `IncrementPipeline::collect_changes` | 纯读，不写任何库。预览与应用共用，保证两条路径看到的窗口一致 |
| `IncrementPipeline::apply / apply_with_precollected` | 逐文件隔离；交入的 `precollected` **仅在区间完全相等时**采信（崩溃重放走持久化的固定区间，永远重新收集） |
| `build_model_update_plan` | 必须在落库前调用——此刻 owner 图与 `ref_rev` 还是旧态。DESI 出单元归并，CATA 只出反向级联种子，其余类型出空计划 |
| `finalize_attempt` | 唯一推进 `applied_sesno` 的地方；交付状态、模型工作、水位、恢复记录同生共死 |
| `finalize_baseline` | 同上但不删 attempt 行——基线不是可重放窗口，删只会误伤别人的崩溃恢复状态 |
| `model_update_pending::drain` | 先非重生成、后重生成（级联展开会入队新的 RegenRoot）；单个目标失败不影响整轮 |
| `preview_manual_update` | 只读语义的例外：会刷新扫描观察字段（`record_scan`），但从不推进 `applied_sesno` |
| `execute_manual_update` | 永不返回 `Err`；前置失败与逐批失败都落在 `ManualUpdateResult` 里，前端只需渲染一种形状 |
| `SesnoRangeResolver::resolve*` | 水位为 0 时只有 `SYST/DICT/GLB/GLOB` 允许冷启动；DESI/CATA 的首次装载必须走全量解析或手动基线 |

---

## 5. 本轮未做的验证

- **未构建、未跑测试**：工作区没有 `target/`，冷构建代价过高。最近一次记录在案的结果是 `cargo test --lib` 189 passed / 0 failed / 38 ignored（2026-07-27 上一轮）。本报告的全部结论均来自源码。
- **未连库**：F2 关于 `fn::find_ancestor_types` 缺失后果的推断，基于「未定义函数会让语句失败、`.check()` 上浮为 Err」这一 SurrealDB 常规语义，未做实测复现。
- **跨仓未提交仍是最大的工程风险**：复核时 `git status --short` 有 **206 项**改动未提交，本报告新增的文档又叠在其上。
