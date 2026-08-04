# 增量更新链路第三方审核报告（Oracle / GPT-5.5 Pro）

日期：2026-07-26
审核对象：`gen-model` 增量模型更新链路（watcher → sesno 区间 → `IncrementPipeline::apply` → `model_update_pending` 队列 → `ModelRefreshPolicy`）
审核方式：Oracle MCP（`@steipete/oracle` 0.16.0）browser 引擎 + ChatGPT GPT-5.5 Pro
关联：`docs/specs/incr-gen-fixes/spec.md`（F1–F9）、`docs/adr/ADR-001-dbnum-update-state.md`、`ADR-008-catalog-reverse-propagation.md`

> 本报告只记录**审核结论与取证**，不含代码改动。修复排期见文末「建议处置」。

## 结论

- 链路的恢复状态机方向正确：`prepare_attempt → PE 落库 → finalize_attempt` 三段式成立，`applied_sesno` 没有发现提前推进的路径，attempt 存的是**固定 plan** 而非重算，符合 ADR-001。
- 本轮确认 **1 个 High、2 个 Medium**，均为真实缺陷、均有行号取证。最该先修的是 **A1**：`drain` 的注释承诺「单个坏任务不阻塞队列」，但 `delete_work` 失败会 `?` 上抛打断整轮消费。
- Oracle 报的 High-2（`datacenter_version` 脱离 finalize 事务）**事实描述准确，但严重度应降为 Medium**——该写是幂等 `UPDATE`，重放收敛，且即使全链路成功它本来也早于几何重生成。
- Oracle 报的 Medium-2（drain 单侧失败丢 action 类型）**核实为误报**，已在下文说明理由。
- 本轮额外发现一处 Oracle 没提的**注释与实现矛盾**（A2）：`persist_latest_main_data` 顶部与 `wrap_in_transaction` 的文档都还在宣称「整窗口单事务、要么整体回滚」，而实现早已改成 500 条一块的分块提交。

## 审核元数据与可信度限制

| 项 | 值 |
|---|---|
| session slug | `incr-update-audit-40` |
| 模型 | gpt-5.5-pro（browser，thinking time = Extended） |
| 送审文件 | 19 个（12 个 `.rs` + CONTEXT.md + spec/plan/tasks + ADR-001/003/008），zip 打包上传 |
| 输入规模 | ~137,278 tokens |
| 实际耗时 | **72 秒** |

**可信度限制（重要）**：Oracle 工具自身对本次运行打出了告警——`Large browser Pro run completed quickly (72s for ~137,278 input tokens); verify the stored model selection evidence before claiming Pro Extended output`。137k tokens 输入只跑 72 秒，说明这一轮很可能没有真正吃满 Extended 思考预算，**深度属于「一遍扫读」而非穷尽审计**。

佐证是它完全没有触碰 prompt 里点名的四个硬骨头（见文末「本轮未覆盖」）。因此本报告的价值在于**它报出来的那几条是真的**，而不在于「没报的就是安全的」。

## 一、A1 · `drain` 的失败隔离承诺与实现不一致 — High

**位置**：`src/data_interface/model_update_pending.rs:391-421`、`:481-483`、`:492`、`:498-499`

`run_one` 的设计意图写在紧邻的文档注释里：

```rust
/// Run one job on its own, recording a durable failure rather than aborting the
/// drain, so a single broken target cannot stall the rest of the queue.
async fn run_one(...) -> anyhow::Result<()> {
    match execute_item(mgr, job).await {
        Ok(()) => {
            delete_work(job).await?;   // ← 唯一会让本函数返回 Err 的地方
            *done += 1;
        }
        Err(error) => { /* 记进 failures，继续 */ }
    }
    Ok(())
}
```

几何生成失败（`execute_item` 返回 `Err`）确实被收进了 `failures` 向量、不中断循环——这部分符合注释。但**删除 pending 记录失败是裸 `?`**，而三个调用点也全是 `?`：

- `:482` 批量成功后逐条清理：`delete_work(job).await?;`
- `:492` 批量失败回退逐根：`run_one(mgr, job, &mut done, &mut failures).await?;`
- `:499` singles / 非 regen 任务：`run_one(mgr, job, &mut done, &mut failures).await?;`

**触发条件**：任一任务几何已生成成功，但删除队列行时 SurrealDB 抖动（连接中断、超时、权限、事务冲突）。

**后果**：本轮 drain 提前返回 `Err`，后面所有排队任务这一轮全不跑。数据不会错（任务仍在队列里，下一轮 watcher 会重来），但：

1. 一个 dbnum 的偶发抖动会拖住同轮所有其它 dbnum 的模型任务；
2. 已经生成成功的那个任务下轮会**重复跑一次完整几何生成**（`gen_all_geos_data` 是重操作）；
3. 上层只看到一条 drain failure，真实失败点（删除而非生成）在日志里不显眼。

**修复方向**：把 `delete_work` 的失败与生成失败同等对待，降级为 `failures.push(...)`；批量成功路径 `:481-483` 同样处理。任务残留会在下一轮被重新执行，代价是一次重复生成——比中断整轮划算。

**置信度**：高。代码路径直接可达，`?` 的语义无歧义。

## 二、A2 · 「整窗口单事务」的注释已与分块提交实现矛盾 — Medium

**位置**：`src/data_interface/increment_pipeline.rs:83-87`、`:683-686`、`:713-730`

`persist_latest_main_data` 顶部仍写着（`:683-686`）：

> 收集本文件本窗口的全部落库语句，作为「一个事务」原子提交：要么整体成功、要么整体回滚，**绝不留下半写状态**。这样 ADR-001「失败批次不推进水位、按同一窗口重试」才安全——重试永远从干净状态开始

`wrap_in_transaction` 的文档（`:83-85`）同样写着：

> Wrap rendered SurrealQL statements into a single atomic transaction so a **per-file incremental persist is all-or-nothing**

而三十行之下的实现（`:713-730`）是分块的：

```rust
const TX_CHUNK: usize = 500;
for chunk in statements.chunks(TX_CHUNK) {
    if let Some(tx_sql) = wrap_in_transaction(chunk) { /* 每块自身原子提交 */ }
}
```

`:713-717` 的新注释才是准确的：改成分块是为了绕开 SurrealDB ws 通道上限（amssys 冷启动窗口会撑爆），真实语义是「**每 500 条一块原子，跨块非原子**」。

**为什么这不只是文档洁癖**：`spec.md` 的 F4 条目（`:75`）正是建立在「分块提交、整窗口非单事务」这个前提上推导出的收敛性要求。上面那两处旧注释直接否定了这个前提，下一个读代码的人（或下一次 AI 审核）会据此得出错误的安全性结论——本次 Oracle 就是靠读实现而非注释才发现的。

**修复方向**：删掉/改写 `:683-686` 与 `:83-85` 的「整体回滚」表述，改为陈述分块语义 + 依赖幂等写收敛，并指向 F4。

**置信度**：高。两处注释与同文件实现直接冲突，可当场比对。

## 三、A3 · `datacenter_version` 更新在 finalize 事务之外 — Medium（Oracle 原评 High）

**位置**：`src/data_interface/increment_pipeline.rs:565-576`（datacenter）、`:581-589`（finalize）

datacenter 版本更新失败只记 warning，随后 `finalize_attempt` 才在单事务里写 pending 任务 + 推水位 + 删 attempt。因此存在窗口：

```
datacenter_version = 已标 Modify/Delete
applied_sesno      = 旧水位
pending 任务        = 未落库
```

**为什么降级为 Medium**（这是本报告与 Oracle 结论的分歧点）：

1. `update_datacenter_version` 发的是 `UPDATE ... SET status`，只命中已发布的交付记录，**幂等**。崩溃后同窗口重放会收敛到同一状态。
2. 即使全链路成功，datacenter 标记本来也**早于**几何重生成——重生成发生在后续的 `model_update_pending::drain`。所以「datacenter 领先于模型」不是这个事务边界引入的新问题，而是链路的既有形态。
3. 真正的残留风险只剩一种：该文件在崩溃后被 `Rollback` / `Duplicate` 判定阻断、**永远不再重放**，此时 datacenter 的标记就成了孤儿。这是小概率复合场景。

**修复方向**（二选一）：

- 方案 A：把 datacenter 语句并进 `render_finalize_transaction`，与水位、pending 任务原子提交。改动集中，但需回归 datacenter 相关测试。
- 方案 B：明确 datacenter_version 是非权威投影，加 `status = pending|committed` 两阶段标记。

**置信度**：中高。事务边界事实明确；严重度判断依赖「UPDATE 幂等 + 只命中已发布记录」这两条，已由 `:738-743` 的文档注释与 `tasks.md` F7 结论佐证。

## 四、Oracle 的 Medium-2 · 核实为误报

Oracle 认为 `model_update_pending.rs:512-523` 的 `drain` 在单侧失败时丢失了 action 类型，排障时分不清是 regen 队列坏还是 cascade 队列坏：

```rust
match (non_regen, regen) {
    (Ok(non_regen), Ok(regen)) => Ok(non_regen + regen),
    (Err(error), Ok(_)) | (Ok(_), Err(error)) => Err(error),
    ...
}
```

**不成立**。被透传的那个 `error` 来自 `drain_where`，其消息体是由 `failures` 拼出来的，每条都以 `job.action.as_str()` 开头（`:407-417`、`:502-507`）。action 类型一直在错误串里，没有丢。

## 五、Oracle 核实为安全的设计点

以下五处 Oracle 重点查过且未发现问题，记录在案以避免重复审：

1. **`applied_sesno` 推进位置符合 ADR-001**。水位推进、pending 任务写入、attempt 删除三者在 `render_finalize_transaction`（`:232-249`）的同一个 `BEGIN/COMMIT` 内，不存在「先推水位后写任务」。
2. **attempt 恢复用的是固定 plan**。`prepare_attempt`（`:199-230`）持久化 `plan_json`，恢复路径（`increment_pipeline.rs:470-476`）直接复用 `attempt.plan`，不在可能半写的库上重算 owner 图。
3. **pending `record_id` 已按 dbnum 隔离**。`record_id`（`:62-69`）由 `dbnum + action + target_refno` 组成，不同 dbnum 的同一 refno 不会互相覆盖。
4. **SurrealQL 插值口径已统一转义**。`render_upsert`、`prepare_attempt`、`mark_failed` 等处的外部字符串均过 `escape_surql_str`。
5. **CATA 净新增跳过与 ADR-008 一致**。`build_cata_cascade_plan`（`model_update_plan.rs:130-157`）只对净 Modified/Deleted 落 `CascadeExpand` 种子，属业务决策而非缺陷。

## 六、本轮未覆盖（下一轮审核的入口）

送审 prompt 里点名要查、但 Oracle 这一轮**完全没有触及**的四个问题。结合 72 秒的运行时长，判断是深度不足而非「查过没问题」，需要拆成小批次重审：

1. **窗口折叠的语义等价性**。`fold_window` / `fold_modified_run` / `fold_attr_namespace`（`increment_pipeline.rs:98-313`）的 last-writer-wins 折叠与逐条回放是否严格等价，尤其是 `children_changed`、`deleted_attrs`、explicit/UDA 三套命名空间、以及 `Add → Modified → Deleted` 混合序列。
2. **共享 `inst_info` 的引用计数脏读**。`delete_inst_relate_cascade`（`helper.rs:65-101`）在同一批 SQL 里连续处理多个引用同一 `inst_info` 的 refno 时，`array::len($old_inst<-inst_relate) = 0` 是否会读到中间态；并发 drain 下是否成立。
3. **`init_watcher` 重复 dbnum 的逃逸路径**。`increment_manager.rs:707-715` 是先 `seen_dbnums.insert` 再 `continue`——第一个同名 dbnum 文件此时**已经进了 params**，靠后面 `:766-770` 的 `retain` 兜底。该兜底是否覆盖所有分支（含 `#[cfg(feature = "mqtt")]` 的 archive 副作用已经发生的情况）。
4. **`cascade_expand` 派生任务的 `attempts` 重置语义**。`render_upsert`（`:136-153`）用 `source_end_sesno` 比较来决定是否把 `attempts` 归零；`CascadeExpand` 派生出的 `RegenRoot` 继承父任务 sesno（`:368-387`），在反复展开时这个「新会话才复活死信」的语义是否还成立。

## 建议处置

| 项 | 严重度 | 建议 |
|---|---|---|
| A1 `delete_work` 失败中断整轮 drain | High | 优先修；改动小（一个 match 分支），配一个失败隔离单测 |
| A2 单事务注释漂移 | Medium | 与 A1 一并处理；纯注释，零风险 |
| A3 datacenter 脱离 finalize 事务 | Medium | 排期；倾向方案 A，需回归 datacenter 测试 |
| 未覆盖的四项 | 未知 | 拆小批次重跑 Oracle，每次只送 2–3 个文件以换取真实的深度思考 |

## 复现命令

```powershell
# 查看本次会话完整记录
# （Oracle sessions 存于 ~/.oracle/sessions，与 CLI 共享）
oracle sessions --id incr-update-audit-40 --detail
```

MCP 侧等价调用：`oracle.consult`，`engine: browser`、`browserBundleFiles: true`、`browserBundleFormat: zip`、`browserThinkingTime: extended`，`files` 为上表 19 个文件的绝对路径。
