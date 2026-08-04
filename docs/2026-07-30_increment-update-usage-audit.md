# 模型增量更新「使用面」审核（2026-07-30）

- 审核对象：`gen-model` 增量更新对外接口、其调用方、内部执行链路，以及词表 / ADR 与实现的一致性
- 方法：静态源码审核（未起服务、未跑测试）。所有结论都给出文件与行号，可直接复核
- 相关仓：`../plant-ui`（独立壳 + 契约类型）、`../rs-plant3-d`（真正发版的宿主）
- 未纳入：`../gen-model-mem-staging`（并行副本，未审）

前几轮审核（`2026-07-26_increment-update-chain-audit-report.md`、`round2`、
`2026-07-27_queue-layer-audit-round3.md`、`2026-07-27_increment-update-interface-audit.md`）
覆盖的是链路内部与队列层。本轮刻意换一个切面：**这套东西是怎么被用的**——规格怎么写、
服务端怎么实现、客户端怎么调、词表怎么定义，四份说法之间对不对得上。

---

## 0. 结论摘要

四面各有实质问题，其中两条会直接影响使用者看到的画面。

| # | 严重度 | 面 | 一句话 |
| --- | --- | --- | --- |
| B1 | 高 | 调用方 | `/api/v1/model/ensure` 全仓没有任何客户端调用点，只有 PowerShell 脚本在打它 |
| B2 | 高 | 调用方 | 房间泳道收敛之后永远显示上一轮的待重算数，30 分钟后还会变红报「饥饿」 |
| A1 | 高 | 契约 | 静态资源目录不存在时整个 Web 服务静默不启动，直接违反 spec §7 明文 |
| A2 | 高 | 契约 | spec §4.6.1 的 `pending-units/retry` 从未实现，死信没有任何复活入口 |
| A3 | 高 | 契约 | ensure 的超时 / 忙碌语义在规格、服务端、客户端三处各不相同 |
| A5 | 中 | 契约 | 请求体新增的 `mdb` / `namespace` 与 `identity_mismatch` 拒绝，spec 零记载 |
| C1 | 中 | 内部 | ensure 生成前不落 durable pending，进程中途挂掉这次工作无痕迹 |
| C2 | 中 | 内部 | 批量收口失败会给生成**成功**的根记失败、涨 attempts，5 次后进死信 |
| D1 | 中 | 文档 | 词表里的「冻结吸收」找不到实现 |
| D2 | 中 | 文档 | 词表里的「模型变更通告」找不到实现 |
| B3 | 中 | 调用方 | 宿主 `rs-plant3-d` 的 preview/execute 不带 `mdb`，范围可静默错开 |
| B4 | 中 | 调用方 | 宿主丢掉错误包封里的 `code`，全部压成 `internal` |
| A4 / A6 / A7 / A8 / A9 | 低-中 | 契约 | spec 全文停留在 ADR-011 合流之前的形状，四处字段与实现对不上 |
| B5 / B6 / C3 / D3 / D4 | 低 | 各面 | 见正文 |

同时有两条值得记下来的**正面结论**，它们是这套设计里最容易写坏、而这里没写坏的部分：
revision 收口没有任何绕过路径（C4），合批锁序在两条批量路径上一致、不会互锁（C5）。

---

## A. 对外使用契约面

基准：`docs/specs/web-service-api.md`；实现：`src/web_service/`。

### A1 · 静态资源缺失会让整个对外面静默不启动（高）

`web_service/mod.rs:212-218` 在启动时对资源目录做了硬校验：

```rust
let asset_root = resolve_asset_root();
if !asset_root.is_dir() {
    anyhow::bail!("PLANT_ASSET_ROOT 不存在或不是目录：{}", asset_root.display());
}
```

而 spec §7 是这么写的：

> 静态前端资源是可选能力……目录缺失时只记录一次告警，静态路径返回 404，REST/WS 仍正常启动。
> 无需为此增加配置开关，**也不得因 `PLANT_ASSET_ROOT` 缺失或无效终止服务**。

`resolve_asset_root()`（`mod.rs:249-271`）的最后一档是相对路径 `"assets"`，即相对**当前工作目录**。
在仓库根目录跑没问题（`gen-model/assets/` 在），把 exe 拷到别处、CWD 里没有 `assets/` 就会命中这条 bail。

血溅范围比字面小一点：`lib.rs:305-313` 把 `serve_if_configured` 包在 `tokio::spawn` 里，
错误只 `eprintln!` 一句「Web 服务异常退出」，进程与 worker 照常活着。所以真实症状是
**队列在跑、数据在进，但 8022 端口从头到尾没起来**，界面上表现为 plant-ui 那句
「读不到任务队列」。一句 stderr 要和满屏启动日志抢注意力，这基本等于没提示。

顺带：`mod.rs:211` 的 `PLANT_UI_WEB_ROOT` 缺省值是 `"../plant-ui/web"`——开发机上兄弟仓库的
相对路径被烧进了服务端默认值（A9）。

### A2 · `pending-units/retry` 从未实现（高）

spec §4.6.1 用整节定义了 `POST /api/v1/update/pending-units/retry`：只允许操作已存在的
`(action, target_refno)`、原子地 `revision += 1` / `attempts = 0` / 清 `last_error`、返回 202。

全仓搜索 `pending-units/retry`，只有 spec 自己命中一次。路由表（`mod.rs:224-236`）里
没有它，`handlers.rs` 里没有对应 handler。

这不是一个可有可无的端点。`model_update_pending.rs:817-826` 的 drain SELECT 带
`(attempts?:0) < MAX_ATTEMPTS`（5），到顶的行成为死信、自动路径永不再碰；
`render_drain_select` 的注释说得很清楚：「manual preview/retry reads the table without this
cap and remains the way to inspect or revive it」——**inspect 有了（`GET /pending-units`），
revive 没有**。今天一个根攒够 5 次失败，除了直接改库没有第二条路。

### A3 · ensure 的超时 / 忙碌语义三处各说各话（高）

| 情形 | spec §4.5 | 服务端实现 | 客户端 `error_packet` 会怎么理解 |
| --- | --- | --- | --- |
| 等满 120s | `202 { code: "generation_pending", generation_root }` + `Retry-After` 头 | `504 { code: "timeout" }`（`handlers.rs:208-214` → `mod.rs:119-125`） | `FailForm::Timeout`，文案「模型服务没有响应。**没有任何数据被改动**，可以直接重试」 |
| 同根正在生成 | 「命中已有 pending 或正在执行的同根任务时**等待同一份工作**」 | `409 { code: "conflict" }`（`handlers.rs:229-231`，`on_demand_model.rs:108-116` 用 `try_lock_owned`） | `FailForm::Internal`，把 anyhow 原文摊给人看 |

两处都是**说反了**：超时那条对用户说「没有任何数据被改动」，而实际后台生成正在跑
（`await_background_without_cancelling` 就是为了不取消它才写的，`handlers.rs:162-182`）；
按它的建议立刻重试，第二发必然撞上还没放开的根锁，拿到 409 → 归 internal → 一段
Rust 错误链。实测 AMS 8000 的 SUPPO / 风管 BRAN 冷生成 99–104 秒，贴着 120s 线，
这条路不是理论路径。

`handlers.rs:209` 那句注释「生成根忙时后续请求会收到 conflict，不会排队」是诚实的，
它承认了实现选的是 A 方案；问题在于 spec 写的是 B 方案，而客户端两个方案都没接。

### A4 · `/health` 字段两个方向都对不上（中）

spec §4.1 的样例里有 `static_assets` 与 `ref0_affiliation_conflicts`，实现（`handlers.rs:59-71`）
两个都没有；实现多出 `mdb` / `namespace` / `worker_alive` / `worker_idle_secs`，spec 一个都没写。

客户端 `task_queue::Health`（`plant-ui/crates/plant-ui/src/task_queue.rs:138-158`）接的是**实现**
那一份，所以今天不出事——但它是照着代码写的，不是照着规格写的。`ref0_affiliation_conflicts`
的缺席值得单拎：spec §3 给了 409 `ref0_affiliation_conflict`，界面却没有任何地方能看出
「这个项目现在有几个冲突 Ref0」。

### A5 · 请求体的身份三元组与 422 拒绝，spec 零记载（中）

`ProjectReq`（`handlers.rs:18-31`）现在收 `project` / `mdb` / `namespace` 三个字段，
`ServiceIdentity::validate`（`mod.rs:45-71`）对**显式传入且不等于服务端配置**的任何一个
回 `422 identity_mismatch`。缺省不传仍按旧契约放行。

这是一次会打到调用方脸上的行为变更（传错 MDB 从「静默按服务端范围跑」变成「422 拒绝」），
而 spec §4.2 / §4.3 的请求体仍然只写 `{ "project": "HD" }`，§3 的错误码表里也没有
`identity_mismatch`。plant-ui 那边已经按新契约接好了（`model_update.rs` 的 `FailForm::IdentityMismatch`
有专属画面），文档是唯一没跟上的一环。

### A6 · `pending_model_retries` 的示例字段是错的（中）

spec §4.2 给的样例：

```json
{ "action": "regen_root", "target_refno": "24381/100817", "dbnum": 7997,
  "revision": 4, "noun": "BRAN", "source_dbnum": 7997, "source_end_sesno": 81, ... }
```

实际序列化的 `PendingModelUnit`（`manual_update.rs:1865-1880`）字段是
`dbnum / root_refno / noun / source_end_sesno / attempts / last_error`——
SQL 里显式 `target_refno AS root_refno`（`:1898`），`revision` 标了 `skip_serializing`，
`action` 与 `source_dbnum` 根本不存在。

照 spec 写客户端会解不出 `root_refno` 而整份 `/pending-units` 反序列化失败。
plant-ui 侥幸没踩，是因为它照代码写的（`model_update.rs:144-154`）；而它一旦解析失败，
`poll_queue` 会把 `pending_known` 置 false、面板上「欠 N 个单元」整格消失
（`model_update_api.rs:103-105`）——静默降级，没有报错。

### A7 / A8 / A9 · 三处小漂移（低）

- **A7**：spec §6 写 TaskId 是 `db-{yyyyMMdd-HHmmss}-{4位随机hex}`，实现是
  `db-{ts}-{6位序号}`（`task_registry.rs:150-157`）。实现有充分理由（287 条同秒入队时
  u16 随机的生日碰撞约 47%，撞了就是一整行任务凭空消失），是 spec 没跟上。
- **A8**：spec §2 说「`console` feature 可包含它」，实际 `Cargo.toml:14` 的 `console`
  不含 `http_api`，`default`（`:12`）也不含。整个对外使用面必须显式 `--features http_api`
  才存在——这件事只在 spec 里以「可包含」的口吻提了一句。
- **A9**：见 A1 末尾，`PLANT_UI_WEB_ROOT` 缺省 `../plant-ui/web`。

### A10 · spec 顶部那条修订注记一直没兑现（这是 A2/A4/A6/A7 的根）

spec 第 8-16 行的修订注记写着「全文修订随服务端 5–9 项（暂停端点 / health / dbnums 扩展）
**一并做**」。那批服务端改动早已落地（`/queue/pause`、`/queue/resume`、`/dbnums` 的
`blocked`/`excluded` 都在），全文修订没做。于是 §4.1 / §4.2 / §4.6.1 / §6 全部停在合流前的形状，
一份「已评审通过」的规格现在有四处会把照它实现的人带沟里。

---

## B. 调用方使用面

### B1 · `/model/ensure` 没有任何客户端调用点（高）

全仓搜索 `model/ensure` 与 `ensure_model`，客户端侧的命中只有三处，而且**全都是否定式的**：

| 位置 | 内容 |
| --- | --- |
| `plant-ui/crates/plant-ui-app/src/main.rs:2342-2343` | 测试断言命令处理器里**不含** `ensure_model` / `/api/v1/model/ensure` |
| `rs-plant3-d/src/plant_ui_host.rs:1615-1631` | `Cmd::RetryModelUnit` 只写一条日志「宿主尚未接入单元重生成」 |
| `plant-ui/crates/plant-ui-app/src/model_update_api.rs` | 全文没有 ensure |

真正在打这个接口的是 `scripts/Invoke-EnsureSweep.ps1` 与 `Invoke-GenRootSweep.ps1`。

这条口子牵着两份设计：plant-ui ADR-0009（显示缺失模型必须走服务，不在渲染进程里现生成）
与 `design/MODEL-UPDATE-FIELD-MAP.md:143` 的「重试」按钮。两者都建立在 ensure 之上，两者都没接。
`docs/2026-07-29_test-ams-incremental-update-summary-report.md:291` 已经把这条列为 D-12
必须实机证明的项，**从 7-29 到今天没有变化**。

服务端这一侧倒是齐全的：错误分型（`handlers.rs:227-238`）、`force` 语义、空单元的
`NoRenderableGeometry` 终态（`on_demand_model.rs:139-155`，G4 已修）、成功后收口 pending
（`:78-86`，G9 已修）。**服务端把台阶修好了，没人走上来。**

### B2 · 房间泳道收敛后永远显示旧数字，30 分钟后变红（高）

链条是这样的：

1. `batch_worker.rs:682` 建行时把 `{panels, elements, dead_letters}` 写进任务行的 `detail`；
2. `batch_worker.rs:717` 收尾调 `registry.finish(...)`，而 `finish`（`task_registry.rs:328-335`）
   只写 `state` / `finished_at` / `result`——**从不动 `detail`**；
3. 下一个空闲轮 `room_round` 先查 `count_room_targets()`，`live == 0` 就直接 `return`
   （`batch_worker.rs:674-677`），**不会建新行**；
4. 客户端 `lane()`（`plant-ui/crates/plant-ui/src/task_queue.rs:832-856`）读的正是
   最近一条 `room_recalc` 的 `detail`：`live = panels + elements`，`running == false` 且
   `live > 0` → `LaneState::Waiting`，再配上 `waited = now - finished_at`，
   超过 30 分钟就 `starving = true`，泳道刷成 `danger_bg` 并打出
   「队列一直不空，房间收敛排不上……材料表读到的房间号可能是旧的，且不会有任何报错」。

也就是说：房间**已经全部收敛干净**的那一刻，泳道会开始显示「N 块面板待重算」，
半小时后变成红色饥饿告警，并且**永远不会自己恢复**——因为没有新的房间轮来覆盖它。
顶栏那格「房间待收敛 N」（`rooms_pending()`，`:400-410`）同源，同样一直挂着。

当前 `DbOption.toml:38` 的 `gen_spatial_tree = true`，这条是活的，不是理论风险。

客户端这边不是没设防：`rooms_pending()` 明确说了「详情缺席时退回任务自己的分母」，
`Lane.seen` 也区分了「从没收过一轮」和「0 块待重算」。它设防的是缺字段，防不了
**字段在、但停在了一轮之前**。

修法二选一，都在服务端：`room_round` 收尾时用剩余计数覆盖 `detail`（需要
`TaskRegistry` 加一个 `set_detail`）；或者收敛到 0 时也建一条 `done=0` 的行。
让客户端改读 `result` 的 `{done, total}` 不行——那里没有 `dead_letters`，
而死信数是「自动路径不会再碰它们」的唯一出口。

### B3 · 宿主不带 MDB，范围可以静默错开（中）

`rs-plant3-d/src/plant_ui_host.rs:262-274` 的 `model_post` 只发得出 `{ "project": project }`。
preview 与 execute 两条都走它（`:194-199`）。

服务端把缺省当「用服务端自己那份配置」（`ServiceIdentity::validate` 的 legacy 分支），
于是本期执行范围由**服务端**的 `mdb_name` 定。`handlers.rs:23-30` 的注释点名了这个坑：

> 服务端与客户端各有一份 `DbOption.toml`，都写 `mdb_name = "ALL"` 纯属巧合，
> 改一边不改另一边，界面显示的范围与真跑的范围会静默错开。

独立壳（`model_update_api.rs:58-88`）三个字段都带，是对的；**真正发版的宿主没带**。
两个壳走的是同一套后端、同一条 ADR-0013，行为却不一样。

### B4 · 宿主丢掉 `code`，所有失败压成 internal（中）

`plant_ui_host.rs:289-306` 的 `model_response` 只从错误包封里取 `message`，`bail!("HTTP {status}: {message}")`；
`host_failure`（`:881-883`）随后一律标成 `internal`。代码注释坦白了这件事，理由也成立
（硬猜 code 会指错出路）。但后果要摆在台面上：宿主上 `identity_mismatch` 没有专属画面，
将来接了 ensure，`container`（契约要求客户端展开一层逐个 ensure，ADR-0009）同样会被压平成
一句 500 文案。B3 与 B4 是同一个根：宿主那条 HTTP 链路是简版，没跟上独立壳。

### B5 · 批次 panic 的原因在界面上丢了（低）

`batch_worker.rs:215-219` 把隔离住的 panic 写成 `{"error": message}` 存进任务 `result`。
客户端 `Outcome`（`model_update.rs:519-530`）只有 `project/status/batch/units/warnings`，
没有 `error` 字段；行的说明列回落到 `batch.message`（`task_queue.rs:687-699`）→ 空。
于是面板显示「失败」，而唯一说清楚为什么的那句话不会出现在任何地方。

### B6 · `/dbnums` 的 15s 客户端超时（低）

`model_update_api.rs:133-141` 的 `get()` 统一 15s，而 `/dbnums` 要重扫项目目录，
调用点自己的注释也承认它是四个里最慢的。取不到只是少画「本期不执行」那一格，不致命；
但在 287 个库的积压态下，这一格恰恰是「某个库为什么水位一直不动」的唯一出处。

---

## C. 内部链路

### C1 · ensure 生成前不落 durable pending（中）

spec §4.5 写的是：

> 短路未命中时**先写入** `(regen_root, generation_root)` durable pending，再同步等待共享生成执行器的结果。

实现（`on_demand_model.rs:73-87`）是反过来的：先 `current_regen_revision` **读**一次现有行，
生成完再 `settle_regen_work`。没有现成行时 `expected_revision` 为 `None`，
而 `settle_regen_work`（`model_update_pending.rs:607-609`）对 `None` 直接 `return Ok(())`。

后果：一次纯按需生成（表里本来没有这个根的 pending）在进程中途崩溃后**不留任何持久痕迹**，
没有任何 drain 会把它捡回来，只能靠人再点一次。这与「所有生成入口共享 durable pending」
（spec §6 并发约束）也是矛盾的。

需要说清的是 G9 那半确实修好了：如果表里**有**行，成功后会按 revision 精确清掉，
旁路生成不再永久残留假账。缺的是另半边——先落行。

### C2 · 批量收口失败会给成功的根记失败（中）

`model_update_pending.rs:887-900`：

```rust
Ok(()) => {                                   // 生成成功
    match clear_regen_work_batch(&settlements).await {
        Ok(()) => done += batchable.len(),
        Err(error) => {
            for job in &batchable {
                record_failure(job, &error, &mut failures).await;   // → mark_failed → attempts + 1
            }
        }
    }
}
```

生成明明成功了，只是删行这一步失败，却给批里每个根 `attempts + 1` 并写 `status = 'failed'`。
一条 flaky 的 DELETE 连撞 5 次，一批健康的根就全进死信；而死信没有复活入口（A2）。

对照 `batch_worker.rs:554-559` 那条同构路径：同样是 `clear_regen_work_batch` 失败，
它只置 `settlement_failed = true` 并把批次状态降级，**不涨 attempts**。
同一件事在两个 drain 入口有两套处置，至少该统一——我倾向于按 `batch_worker` 那套，
「收口失败」不是「生成失败」。

### C3 · `ManualEnqueueReceipt` 上的无效 serde 属性（低）

`batch_scheduler.rs:81-92`：结构体只 `derive(Serialize)`，`mdb` / `namespace` 上却挂着
`#[serde(default)]`——那是反序列化属性，这里不产生任何效果。无害，但会让读的人
以为这个回执有反序列化契约。

### C4 · revision 收口没有绕过路径（正面）

逐条查过所有会删 / 改 pending 行的地方：`delete_work` → `render_delete_work` →
`render_delete_revision`（`:452-454`）、`mark_failed` → `render_mark_failed_revision`（`:496-498`）、
`clear_regen_work_revision`（`:533-549`）、`clear_regen_work_batch`（`:567-579`），
全部走 `settle_predicate(action, target, revision)`。没有一条按 id 或按 target 裸删的路径。
CONTEXT.md 的「revision 收口」在代码里是**真的**闭合的，这条不用动。

### C5 · 合批锁序一致，不会互锁（正面）

`batch_worker.rs:530-540` 与 `model_update_pending.rs:874-883` 两条批量路径都先
`sort_unstable()` 再依次加锁，全局序一致；逐根回退前都先 `drop(guards)`；
`ensure` 那侧用 `try_lock_owned` 不参与等待。三个入口凑不出环。

---

## D. 词表 / ADR 与实现的一致性

先说对得上的部分，免得下面显得一边倒：「待重试单元」的 `(action, target_refno)` 身份
（`record_id_of`，`:82-86`）、「最小交付单元」的默认集与 `FTUB` / `WORL·SITE·ZONE` 恒拒
（`generation_root.rs:27-32, 62-68`）、「Fresh 根 / 重试根」的纯谓词
（`root_joins_regen_batch`，`:832-834`）、「同轮吸收」的封闭性判据
（`absorption_is_closed` / `absorption_verdict`，`model_update_pending.rs:1174-1203`）、
「合批重生成」的整批同成同败与逐根回退——这些都与词表逐字对得上，而且都有单测钉着。

### D1 · 「冻结吸收」找不到实现（中）

CONTEXT.md 第 77-79 行：

> **冻结吸收 (Freeze-time Absorption)**：数据批次在执行起点重扫得到更高上界时，
> 将该上界已经覆盖的后继排队区间并入当前运行批次。**完全覆盖的后继任务以「已被当前批次吸收」
> 成功终止**，部分覆盖的后继区间从冻结上界之后继续。

实现里只有 `record_frozen_end`（`batch_scheduler.rs:320-342`），它做的是：把重扫得到的
新上界写回**运行中**那一行与对应任务行。仅此而已。后继排队行一个字都没碰——
它的 `start_sesno` 仍是入队时按**冻结前**的 `running_end + 1` 算的，既不会被
「已被当前批次吸收」终止，也不会「从冻结上界之后继续」。全仓搜「吸收 / absorb」，
命中的全是房间的同轮吸收，与这条无关。

实际危害有限：执行侧一律按水位重算窗口（`refresh_candidate` + `execute_one_dbnum`），
所以不会重复应用、不丢数据。症状是队列面板上留一条区间读不通的行，跑起来必然
up-to-date/skipped——正是 `batch_queue.rs:45-53` 那段注释想避免的「幽灵行」，只是它
防的是入队那一侧，防不了冻结之后才变成幽灵的那些。

要么补实现，要么把词表这条改成实际语义。**不能两个都不做**——词表当前这条描述会让
后来的人以为队列有一套并不存在的自愈能力。

### D2 · 「模型变更通告」找不到实现（中）

CONTEXT.md 第 93-95 行定义了「模型变更通告」：某个 refno 的模型产物已落库、
与观看端手上那份不再相同的对外告知，三种口径（重生成 / 仅位姿 / 已删除）。

WS 那边只有 `Topic::Tasks` 一个主题（`events.rs:15-29`），事件只有
`task_started` / `task_progress` / `task_finished` / `pong`。全仓搜「通告 / notice /
model_change」，零命中。

这是词表里唯一一个**纯对外协议**概念却没有任何实现的条目。它要么是还没做的设计
（那该进 `docs/plans/` 而不是词表），要么是已经被别的机制取代了（那该从词表删掉）。
现状下，一个读词表理解系统的人会以为服务端会主动告诉观看端「这个 refno 变了」，
而实际上观看端只能靠轮询 `/tasks` 里的 `ModelUnitResult.old_owner` / `new_owner` 自己推——
`MODEL-UPDATE-FIELD-MAP.md:144` 还注明「现在的实现完全忽略了这两个字段」。

### D3 · 派生根的 `dbnum = 0` 哨兵值词表没提（低）

CONTEXT.md 的「待重试单元」说 `dbnum` 是「由 Ref0 库归属得到的路由与校验字段」。
而反向级联派生出的根显式填 `dbnum = 0`（`model_update_pending.rs:695-706`），
表示「来源库未解析」，靠空闲轮的 `drain_data_phases` 统一消化。代码注释把理由讲得很透，
词表一个字没提这个哨兵值——直接读表的人会把 0 当成一个真库号。

### D4 · spec 的修订注记一直挂着

见 A10。这是 A2/A4/A6/A7 的共同来源，单独列在这里是因为它本质上是文档流程问题，
不是某一个字段写错。

---

## E. 建议的处理顺序

按「改动小 / 收益大」排，不按严重度排：

1. **B2 房间泳道**——服务端一个 `set_detail` 加一行调用。今天就在误报，且 `gen_spatial_tree` 是开的。
2. **A1 静态资源 bail**——把 `bail!` 改成 warn + 记一个 `static_assets: false` 交给 `/health`
   （spec §4.1 本来就要这个字段，A4 一并解决）。
3. **C2 收口失败不涨 attempts**——照 `batch_worker` 的口径统一，五行以内。
4. **A3 ensure 三态**——先定下到底走哪个方案（202 轮询 / 504+409），三处一起改。
   在 B1 接上之前，这条只影响 sweep 脚本；一旦 B1 接上就是每次显示都会撞到的路径，
   所以要赶在 B1 前面定。
5. **A2 retry 端点 + C1 ensure 先落 pending**——这两条是同一件事的两面（durable 化），
   一起做比分开做省事。做完死信才有出口。
6. **B1 客户端接 ensure**——工作量最大，且依赖 4 与 5 定型。
7. **B3 / B4 宿主补齐 MDB 与 code 透传**——宿主那条 HTTP 链路整体对齐独立壳。
8. **A 组文档修订 + D1 / D2 / D3 词表处置**——建议一次性做完 spec 全文修订，
   把顶部那条挂了三天的修订注记摘掉。

## F. 本轮没有覆盖的

- 全部结论均为静态审核，**没有起服务验证**。B2（房间泳道）与 A3（ensure 超时）
  最值得实机复现一次，各自十分钟以内。
- `increment_pipeline.rs`（116KB）与 `manual_update.rs`（226KB）只按需读了与本次切面
  相关的片段，没有通读。若要做第四轮内部链路审核，这两个文件是主战场。
- `../gen-model-mem-staging` 是并行副本，本轮完全没碰。它的 `web-service-api.md` 与
  `web_service/` 与主仓已有差异（例如那份 spec §4.5 还没有 202 契约那一段），
  合并时要留意别把旧规格带回来。
- CATA 按需解析（引用闭包 / 惰性兜底 / 闭包漏边）与房间归属的几何判定这两块，
  本次只核对了词表与实现的存在性，没有审内部正确性。
