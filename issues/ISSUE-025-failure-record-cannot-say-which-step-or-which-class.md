# Issue #025: 失败批次说得出「为什么」，仍说不出「走到哪一步」「是哪一类」「该去看哪个库」

## 📋 Issue 信息

- **Issue ID**: #025
- **类型**: 可观测性缺口 🔍
- **优先级**: High 🟠
- **状态**: Open 📝
- **创建日期**: 2026-08-27
- **相关模块**: `data_interface/batch_failure_log.rs`、`data_interface/batch_worker.rs`
  （`set_active_task_stage` / `render_failure_reason_lines` / `failure_reason`）、
  `data_interface/task_registry.rs`（`current_stage`）、`data_interface/debug_scope.rs`、
  `data_interface/staging/lifecycle.rs`、`web_service/handlers.rs`（`/health`、
  `/api/v1/batch-failures`）、`web/ops.html`

## 🔍 问题描述

2026-08-27 现场（`D:\release\9001\project\JEU\JEU000`，SYST **8191**，会话区间 36..=37）
一条数据批次失败，把整个 meta 相焊住，下游 DESI 那条 `apply_window` 在队列里
`blocked_by_phase: meta` 一直排着。同一天已经落地了一整条「说出原因」的链：

| 层 | 落点 | 解决的问题 |
|---|---|---|
| 控制台 | `render_failure_reason_lines` 紧跟完成行 | 人在机器前时不必再去取回执 |
| 磁盘 | `logs/batch-failures-*.jsonl` | 重启不丢、异机拷得走 |
| 接口 | `GET /api/v1/batch-failures` | 读盘，重启之后仍答得出上一次为什么失败 |
| 面板 | `ops.html` 逐库最新原因（带 `died_at` / `file_path` / 来源） | 不开 devtools 就看得见 |

这条链解决的是**「为什么」**。本 issue 记的是它之后仍然缺的四格——它们各自决定
一个排查动作能不能做完：

1. 死在哪一步，以及**每一步花了多久**；
2. 这次失败**属于哪一类**（机读，可聚合、可给针对性处置）；
3. 被卡住的那条任务**该去看哪个库**；
4. 失败当时的**暂存窗口**与**水位判定输入**。

### 预期行为

拿到一条失败记录（或打开面板上那张卡），不再需要回到现场控制台，就能回答：
走到了哪一步、哪一步慢、是哪一类故障、重跑有没有意义、被它挡住的是谁、
它留下的暂存窗口回收了没有。

### 实际行为

以上六问，当前一问都答不完整；答案要么只在**当时**的屏幕上（滚走即失），
要么要求**事前**就打开了追踪开关。

## 🔬 缺口逐条

### 一、`died_at` 只有一格，阶段行全部丢在 stdout（最值得补）

**现状**：`set_active_task_stage` 走 `TaskRegistry::set_stage` 写下 `current_stage`，
`finish` 不清它，所以终态那一格就是「死在哪一步」，`batch_failure_log` 的
`died_at` 取的正是它——这一格是对的，也刚补齐了六个阶段的中文标签
（`identity_check` / `wipe_reinit` / `initial_load` / `resolve_window` /
`collect_window` / `stage_apply`）。

**缺的是它的前半段**。8191 那一屏上最有价值的东西恰恰是那七八行阶段行：
「复核文件身份与水位 → 文件身份复核完成 → 解析回退合适配 → 收集增量 36..=37 →
初始化模型工作单（水位未推进）」，每行带时刻。它们全是 `println!`，
**一个字都不进 JSONL**。于是：

- 「死在收集」与「收集正常、跑了 40 分钟之后写回才死」，记录里长得一模一样；
- 哪一步慢说不出来——而「慢」和「卡住」在总耗时那一列上本来就长得一样
  （面板已经为 running 行单独报过「本阶段多久没有新进展」，失败记录里却没有对应物）。

**建议**：在任务登记表上挂一本**有界**的分步账，每次阶段切换结算上一步：

```jsonc
"steps": [
  { "name": "identity_check", "at": "…", "ms": 120,   "ok": true },
  { "name": "collect_window", "at": "…", "ms": 41_233, "ok": true },
  { "name": "stage_apply",    "at": "…", "ms": 8,      "ok": false }
]
```

`set_active_task_stage` / `set_active_task_stage_quiet` 是所有阶段切换的**唯一**汇流点，
钩子挂在那里即可，调用点一处不用改。上限建议 32 步并记溢出条数（「悄悄丢比丢本身更糟」，
沿用 `debug_scope` 环形缓存那条口径）。面板把它画成一条时间轴。

### 二、`reason` 是自由文本，没有机读的 `reason_code`

**现状**：`failure_reason()` 产出 `(reason, reason_from)`，`reason_from` 区分的是
**出处**（`result.batch.message` / `warnings.last()` / 无），不是**类别**。而
`render_failure_reason_lines` 的文档自己写着：收集口有「十几个各自具名的硬失败出口」。

**后果**：

- 面板无法聚合。「这个库连败 5 次全是同一码」和「5 次 5 个码」是两种完全不同的诊断，
  现在都只显示最后一句话；
- 处置建议只能一视同仁。连败卡片上的「解除路径（三选一）」对**确定性**失败是误导——
  文件长出新会话确实会清账，但同一句话下一轮照样出现（卡片正文已经写了这个免责声明，
  可它没有能力分辨到底是不是确定性失败）。

**建议**：在错误类型上带一个稳定字符串码（形如 `collect.window_incomplete`、
`identity.file_moved`、`stage.apply_conflict`），随记录落一格 `reason_code`；
未归类的落 `unclassified` 而不是空——空会和「老版本记录」混淆。
面板据此做同类计数，并把「重跑有没有意义」写成按码分支的一句话。

### 三、blocker 是字符串数组，被挡住的那条任务跳不到肇事者

**现状**：协调器内部存的是 `blockers: Vec<(DataPhase, String)>`——**相是知道的**，
但对外快照 `InitializationSnapshot::blockers` 压成了 `Vec<String>`，相被丢掉，
dbnum 从来就没进去过；`ops.html` 再只显示 `blockers[0]`。队列行那边只带
`blocked_by_phase: "meta"`。

**后果**：8191 现场那条 DESI 任务，看得见「被 meta 挡着」，看不见**是 8191 挡的**。
人得自己在水位表里找哪个 meta 库没追平，而 meta 库往往不止一个。

**建议**：blocker 结构化为 `{ phase, dbnum, project, task_id, reason_ref }`
（`reason_ref` 指向失败记录的 `task_id`），同时保留一份渲染好的字符串兼容旧消费者。
入队侧记 blocker 的几处（`manual_update` 的 `phase_blockers.push(...)`）本来就握着
dbnum，现在是在拼字符串时把它拌进去了。面板上把被挡住那行的说明做成可点的跳转，
直接落到肇事库的失败卡。

### 四、暂存窗口的下落，与「水位未推进」这个判定的输入

**4a 暂存窗口**：8191 那屏写着「使用 kv-mem 暂存窗口 `staging_8191_1`（sesno 36..=37）」。
`/health` 有 `staging_windows`（`staging::lifecycle::resource_snapshots()`），但
**面板没画，失败记录里也没有这一格**。失败之后窗口是回滚了还是残留着，直接决定
重跑会不会踩同一堵墙，也是资源泄漏的第一现场。建议：失败记录带 `staging`
（窗口名 + 存活状态 + 区间），面板把 `staging_windows` 摆在熔断卡旁边。

**4b 水位判定输入**：「水位未推进」的判据（收集出的会话页清单、并入名单、旧→新水位）
只在 `debug_scope` 的 `TracePoint::Collect` / `Terminal` 里，而
`debug_scope::trace()` 在限定域为空或不含该 dbnum 时**直接 return，连载荷闭包都不执行**。
也就是说这份证据要求**事前**就带着 `--debug-dbnum` 启动——可失败已经发生了，
事后再开追踪什么也追不到。建议：非成功终态时**无条件**把 Collect / Terminal 两点的
裁决摘要（有界：页数、区间、并入条数、旧→新水位）塞进失败记录，与限定域是否开启无关。

### 五、以登录的那个项目为准，别让人在 dbnum 上认项目

**先更正一个说法**：`8191` 不是撞号。它是 E3D 给每个项目的系统库保留的号，库号空间本来
就只在项目内唯一，`amssys` 与 `acpsys` 同为 8191 是设计如此，不是异常。摄入侧也早就
做对了——`in_scope_with` 用 `is_foreign_runtime_sys` 把非主项目的 SYST/GLB/GLOB 挡在
门外，实测 `/api/v1/dbnums` 的 42 行里 8191 只有一行（`amssys`），没有任何重复。

残留的两处都在**展示与记录层**，共同的病是「明明知道登录的是哪个项目，却还按裸号取」。

**5a `batch_failure_log::recent()` 只按 `dbnum` 筛，不按项目。**
这本账落在**进程工作目录**下的 `logs/`，跨重启、跨配置改动都活着。同一个工作目录先后
跑过两个 `project_name`（切项目、切站点、e2e 夹具复用同一个 bin 目录）时，
`?dbnum=8191` 会把上一份配置留下的记录一并端出来。而记录里**本来就写着 `project`**，
判据是现成的，只是没用上：

```rust
// handlers.rs —— 默认只给本服务这个项目；要看全部显式 ?project=all
let project = match query.project.as_deref() {
    Some("all") => None,
    Some(explicit) => Some(explicit.to_string()),
    None => Some(state.identity.project.clone()),
};
Json(batch_failure_log::recent(kind, project.as_deref(), dbnum, limit))
```

回执里要把**用了哪个筛子**一起回出来（如 `"project_filter": "AvevaMarineSample"`），
否则「这个库没失败过」与「被筛掉了」又长成同一副样子——本文件通篇在防的就是这件事。

**5b 面板的 `byDb` 是「任意挑一个」。**
`new Map(dbs.map(r => [r.dbnum, r]))` 同号时后来者静默覆盖前者，代码注释自己也承认
「按号取一行等于从几份 sys 文件里任意挑一个」。当前后端不会给出重复行，所以它今天是对
的——但它的正确性依赖一个不写在这里的前提。改成显式择优：同号时取
`projOf(r) === S.health.project` 的那一行，取不到再退回第一行；「撞号候选按 `file_path`
摆出来让人自己认」那段只在**当前项目一个都对不上**时才出现。一句话理由：
**服务自己知道它登录的是哪个项目，这件事不该问人。**

**验证**：往同一个 `logs/` 里手工塞一条别的 `project` 的 `batch_failure` 记录，
`?dbnum=8191` 默认取不到它、`?project=all` 取得到；面板上 8191 那一行恒为 `amssys`。

### 小尾巴：日志目录在哪，`/health` 没有这一格

`batch_failure_log::DIRECTORY` 是相对进程工作目录的 `logs/`。落盘那句话打了完整路径，
但它同样会滚走；`/health` 里没有「本进程的日志目录」这一格，面板底栏也就无从显示。
现场被要求「把 logs/ 拷走」时，得先猜服务是从哪个目录起的。

## 🛠️ 落地顺序建议

1. **五**（按登录项目筛）——两处各几行，判据都是现成字段，先做；
2. **三**（blocker 结构化）——改动小，直接解掉 8191 现场「跳不过去」那一步；
3. **一**（分步账）——收益最大，`set_active_task_stage` 是现成的单一汇流点；
4. **四**（暂存窗口 + 水位输入）——都是「把已有事实塞进已有记录」；
5. **二**（`reason_code`）——要碰错误类型，面最宽，放最后。

## 🧪 验证标准

- 造一条在 `collect_window` 失败的批次：`logs/batch-failures-*.jsonl` 那一行同时给出
  完整 `steps`（含每步耗时）、`reason_code`、`staging`、水位判定摘要；
- **停掉服务再起**，`GET /api/v1/batch-failures?dbnum=8191` 仍取得到上面这一整条；
- 面板上被挡住的那条任务，一次点击能落到肇事库的失败卡；
- 把 `--debug-dbnum` 摘掉重跑同一条失败，水位判定摘要**照样在**（这条专门钉 4b）。

## ⚠️ 一条纪律

新加的任何一格都必须**落盘**并经 `/api/v1/batch-failures` 读得回来。只活在进程内的
账本（`/health` 的 `batch_failures`、`/tasks` 的回执）已经被 8191 证明过一次不够用：
人发现问题时往往已经重启过了。同理，别把新证据挂在需要**事前**打开的开关上。

## 📚 相关文档

- `src/data_interface/batch_failure_log.rs` 模块头的三本账分界表（`/health` 连败次数 /
  `queue-stalls-*.jsonl` 队列姿态 / 本模块失败原因）——本 issue 补的是第三本账的字段
- `docs/specs/web-service-api.md`（`/api/v1/batch-failures` 口径）
- ADR-025（严格阶段屏障）——缺口三是它的可观测性配套
- ISSUE-024（同一身份的三种拼法）——同属「静默失效」族

## 🏷️ 标签

observability diagnostics batch-failure staging watermark ops-panel

---

**发现方式**: 2026-08-27 现场 SYST 8191 卡住 meta 相的照片，对照本仓工作区里当天
落地的失败原因链（控制台行 / JSONL / `/api/v1/batch-failures` / 面板）逐格核对，
列出其后仍然答不出的问题。
