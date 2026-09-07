# Issue #029: 初始化 run 里确定性失败的生成根没有终态——它把自己的 run 永久焊住，还占死一个生成槽位

## 📋 Issue 信息

- **Issue ID**: #029
- **类型**: Bug 🐛（活性缺陷：状态机缺终态）
- **优先级**: High 🟠
- **状态**: Open 📝
- **创建日期**: 2026-09-07
- **相关模块**：`data_interface/model_publish_queue/runtime.rs`（run 收口条件、`replay_job`、
  `max_active_runs` / `admit_runs`）、`data_interface/model_publish_queue/journal.rs`
  （`TaskState::GenerationHold`、`in_flight_runs`）、`data_interface/model_update_pending.rs`
  （`render_drain_claim_for_scope` / `render_drain_select_for_scope_with_active_cohorts`
  的 `init_run_id IS NONE`、`render_initialization_exclusion`）

## 🔍 问题描述

一个生成根在**初始化发布 run** 里几何失败，且失败是**确定性**的（重算多少次都一样，例如它引用的
目录件在盘上根本不存在），此时：

- 它在 journal 里落 `GenerationHold`；
- 它在 `model_update_pending` 里留一行，带 `init_run_id`；
- 这两样**互相等着对方先动**，而**没有任何自动路径能让其中一样先动**。

结果是该库的 run 永不收口：`model_sesno` 永远是 null、`/health` 永远 `degraded`、
`active_runs` 永久占着一个生成槽位。**数据面不受影响**（`applied_sesno` 照常推进，ADR-056 D1
在这一点上是成立的），坏的是模型面的活性。

### 现场

YCYK-E3D `/SLYK` 首次初始化（10 个 DESI，2438 个生成根）。2434 个发布成功，4 个 BRAN 卡住：

| 根 | 库 | 失败原因 |
|---|---|---|
| 14207/6162、14207/6182、14207/6200 | 6015 `*AVEVACN/TEMP_EQUIP` | `db 7004 会话 3 的索引里没有 15196/40431` 一族（FTUB / BEND） |
| 21491/17101 | 5107 `*IDW/IDW` | `db 212 会话 5 的索引里没有 16596/35338`（FLAN） |

那几个目录件是**真的不存在**——`bin/refno_locator_probe` 扫过 `acw000` 全部 80 个文件：
Ref0 15196 只有 `acw7004_0001` 一家认领（没有归属冲突，取数层挑对了文件），那几个号在最新会话
与整条保留会话链上一次都没出现过。**那是甲方的数据问题，本 issue 不是它**；本 issue 是
「gen-model 遇上一个注定失败的根之后怎么办」。

### 预期行为

一个根确定性失败，代价应当**局限在这个根**：其余根照常发布、run 照常收口、`model_sesno`
照常推进到它该到的地方，这个坏根按「已放弃」记账并在 `/health` 与 `/dbnums` 上并排陈述
（CONTEXT.md「模型水位」词条里的四格分类本来就留了「已放弃（死信或 `http_422`，不拦水位但必须
并排陈述）」这一格）。

### 实际行为

整个 run 停住，`model_sesno` 一直是 null，`/health` 一直 `degraded`，两个生成槽位（默认
`AIOS_INIT_MAX_ACTIVE_RUNS=2`）被两个各自只坏了一两根的 run 占死。现场从 14:01 一直保持到
15:00+ 无变化，`attempts` 始终为 **0**。

## 🔬 三道门锁成一个闭环

**① run 收口要求这个库的 `model_update_pending` 清零。**
`runtime.rs` 的收口段：读 `RETURN count(SELECT id FROM model_update_pending WHERE dbnum=…)`，
只有 `tasks == 0 && effects == 0` 才 `registry_on(...)` + `journal.finish(&m.run_id)`。

**② 那些行归 run 所有，普通消化永远认领不到。**
四条 drain 相关的 SQL 全带 `AND init_run_id IS NONE`
（`render_drain_select_for_scope_with_active_cohorts`、`render_drain_claim_for_scope`、
`has_claimable_work`、`clear_stale_with_protected_on`），再加一道
`render_initialization_exclusion(blocked_dbnums())` 把「有未收口 run 的库」整库排除。
所以 `attempts` 永远是 0，**`MAX_ATTEMPTS` 那套死信机制对 init 拥有的行不可达**——
它连一次尝试都不会烧，也就永远到不了死信。

既有测试 `initialization_ownership_survives_cleanup_and_blocks_only_completion` 钉的正是这条
所有权，名字里的 **"blocks only completion"** 就是现在发生的事；当时的假定是 completion 总会来。

**③ `GenerationHold` 的唯一出口是手动 replay，而 replay 只是重跑。**
`journal.rs` 里 `GenerationHold` 只被 `RetryGeneration` 事件移除，触发者是
`runtime::replay_job`（`POST /api/v1/model-publish/jobs/{id}/replay`）。它做两件事：
`journal.retry_generation(id)` 删掉那条 journal 记录，`r.producing.remove(&run)` 把整个 run
放回待启动集——于是 run **整个重跑一遍**。

### 实测：replay 是重试循环，不是出口

对 `14207/6162` 打了一次 replay：

```
before  generated=2438 committed=2434 failed_hold=4 active_runs=2 model_ready=False
HTTP 202 {"job_id":"f64de0e18e85cf43…","status":"ready"}
after   generated=2438 committed=2434 failed_hold=4 active_runs=2 model_ready=False   （×5，每 5 秒一次）
held errors now:
  root 14207/6162 has 2 failed geometry elements: by noun [BEND×2]; first 2: 14207/6163 BEND: db 7004 会话 3 的索引里没有 15196/40431 | …
```

run 重跑了 63 个根，那一根带着**一模一样**的报错回到 `generation_hold`。对确定性失败，
文档给的这条恢复路把状态原样还原，什么都没解决。

## 📊 后果不止一个库

`journal.rs::in_flight_runs` 只在**一个 run 的根全部 held** 时才把它排除在「在飞」之外：

```rust
.filter(|m| held.get(m.run_id.as_str()).copied().unwrap_or(0) < m.roots.len())
```

现场是 3/63 与 1/650，两个 run 都算在飞，把默认的两个生成槽位全占死，`admit_runs` 此后
一个新 run 都放不进来。这次剩下的库恰好都已经跑完，所以没显出来——**换一个大 MDB（YCYK-E3D
的 `/IDW` 声明 33 个 DESI），头两个库各出一根这样的坏根，后面 31 个库的初始化就永远排不进来。**
这一条尚未实测，是从上面这段代码推的。

## 🛠️ 处置方向（未拍板）

三条互不排斥，按代价从小到大：

1. **给 `GenerationHold` 一个终态。** 确定性失败重试到上限后转「已放弃」：清掉它的
   `model_update_pending` 行让 run 得以收口，同时在 `gen_root` 上记下放弃原因。
   CONTEXT.md 的模型水位四格里「已放弃……不拦水位但必须并排陈述」已经预留了这个语义，
   缺的是让它真的发生的那一步。注意这会改变「run 收口 = 所有根都发布了」的当前含义，
   收口回执与 `/health` 要能说清「这个库追平了，但有 N 个根被放弃」。
2. **让在飞判定按「还有没有根在动」而不是「是不是全 held」。** 一个 run 只要没有任何根在
   生成，就不该占着生成槽位。这条只解决「拖累别的库」，不解决本库不收口。
3. **区分确定性失败与瞬时失败。** 「目录件在索引里不存在」重算一万次也一样，与
   「撞上乐观冲突」「连接抖动」不是一类，不该共用同一条 replay 语义。

## 🧪 复现

1. 找一个 DESI 库，其中某根 BRAN 的目录链指向一个盘上不存在的目录件
   （YCYK-E3D `/SLYK` 的 6015 与 5107 现成）；
2. 全新 store + 全新 namespace，`watch_dbnums` 覆盖该库，跑一次初始化；
3. 等数据面 `applied_sesno` 追平文件后观察：`/health` 的 `model_publish_queue.failed_hold > 0`、
   `active_runs` 不归零、`initialization.model_ready` 恒 false、该库 `model_sesno` 恒 null、
   `model_update_pending` 里那几行的 `attempts` 恒 0；
4. 打 `POST /api/v1/model-publish/jobs/{id}/replay`，观察它原样回到 `generation_hold`。

现场留在 `plant-1/runycyk2-onepass/`（namespace `ycyk_init5107_20260907_140037`，
库 18095 / 服务 18096），`verification.json` 与 `service.stdout.log` 都在。

## 📚 相关文档

- `docs/plans/2026-09-07-update-scope-file-side-mdb-fallback-plan.md` §8 / §9——同一次
  YCYK-E3D 初始化实测，那份计划解决的是另一件事（新项目要跑两趟），本 issue 是它 §8 记下的遗留项。
- `CONTEXT.md`「模型水位」——四格分类里「已放弃」那一格的语义。
- `docs/adr/ADR-056`——D1「模型失败不阻断窗口」。数据面确实没被阻断，本 issue 说的是模型面自己的活性。
- `bin/refno_locator_probe`——判定目录件是真缺还是找错库的探针。

## 🏷️ 标签

`model-publish-queue` `initialization` `liveness` `generation-hold` `model_update_pending`
