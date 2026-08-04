# 案例 12 · drain 的失败隔离：一次删除抖动拖垮整轮队列

<sub>族 D 水位与重放 · High · 已修 · 证据层 B（单测 + 注入）</sub>

## 一句话

函数注释承诺「单个坏任务不阻塞队列」，但**删除队列行**失败是裸 `?`——一次数据库抖动就让
本轮后面所有任务全不跑，而且已经生成成功的那个下轮还要再跑一遍完整几何。

## 现象

`run_one` 的设计意图写在紧邻的文档注释里：

> Run one job on its own, recording a durable failure rather than aborting the drain,
> so a single broken target cannot stall the rest of the queue.

几何生成失败（`execute_item` 返回 `Err`）确实被收进 `failures` 向量、不中断循环——这部分符合注释。
但删除 pending 记录失败走的是另一条路：

```rust
match execute_item(mgr, job).await {
    Ok(()) => {
        delete_work(job).await?;   // ← 唯一会让本函数返回 Err 的地方
        *done += 1;
    }
    Err(error) => { /* 记进 failures，继续 */ }
}
```

三个调用点也全是 `?`：批量成功后逐条清理、批量失败回退逐根、singles / 非 regen 任务。

## 证据

Oracle 审核 **A1（High）**，出处
[`../../docs/2026-07-26_increment-update-chain-audit-report.md`](../../docs/2026-07-26_increment-update-chain-audit-report.md) 第一节；
round2 复核确认「四处仍是裸 `?`」。

**触发条件**：任一任务几何已生成成功，但删除队列行时 SurrealDB 抖动（连接中断、超时、权限、事务冲突）。

**后果**：本轮 drain 提前返回 `Err`，后面所有排队任务这一轮全不跑。数据不会错（任务仍在队列里，
下一轮 watcher 会重来），但：

1. 一个 dbnum 的偶发抖动会拖住同轮**所有其它 dbnum** 的模型任务；
2. 已经生成成功的那个任务下轮会**重复跑一次完整几何生成**（`gen_all_geos_data` 是重操作）；
3. 上层只看到一条 drain failure，真实失败点（删除而非生成）在日志里不显眼。

## 根因

「失败隔离」这个设计意图只覆盖了**它想到的那种失败**（几何生成失败），没有覆盖同一段代码里
另一种同样可恢复的失败（清理队列行失败）。注释写的是意图，`?` 写的是实现，两者不一致时
没有任何机制会指出来。

## 修法

把 `delete_work` 的失败与生成失败**同等对待**，降级为 `failures.push(...)`。
现行 [`../../src/data_interface/model_update_pending.rs`](../../src/data_interface/model_update_pending.rs)：

```rust
// run_one（:462-471）：先执行、再删除，两种失败合并成一个 outcome
let outcome = match execute_item(mgr, job).await {
    Ok(()) => delete_work(job).await,
    Err(error) => Err(error),
};
match outcome { Ok(()) => *done += 1, Err(e) => record_failure(job, &e, failures).await }

// 批量成功路径（:533-537）：逐条清理，失败也只记不抛
for job in &batchable {
    match delete_work(job).await {
        Ok(()) => done += 1,
        Err(error) => record_failure(job, &error, &mut failures).await,
    }
}
```

`record_failure`（`:436`）把错误写进 `failures`，并顺带 `mark_failed` 落库；
`mark_failed` 自己失败时也只是把这一层写进消息串，不上抛。

代价是一次**重复生成**：任务残留会在下一轮被重新执行。比中断整轮划算。

顺带澄清一条 Oracle 的误报：`drain` 在单侧失败时并没有丢 action 类型——
被透传的 `error` 来自 `drain_where`，消息体由 `failures` 拼出，每条都以 `job.action.as_str()` 开头。

## 验证

`live_failed_queue_cleanup_does_not_stall_the_rest`（`model_update_pending.rs:1009`，
标注 `manual live`）：用测试钩子 `fail_deletes_for_test(n)` 强制前 n 次 `delete_work` 失败，
断言排在坏任务后面的任务照常执行完。

## 规律

**「不中断整轮」这类承诺，必须覆盖该路径上的每一种可恢复失败，而不只是主要的那一种。**
一个实用的自检：在声称失败隔离的函数里，把所有 `?` 数一遍——每一个都问「这个错误发生时，
我真的想让整轮停下来吗」。多数情况下答案是否定的。

**注释与实现的矛盾要么修实现、要么修注释，不能留着。** 这条与审核 A2（「整窗口单事务」的注释
早已与分块提交实现矛盾）是同一个毛病：过时的注释比没有注释更危险，因为它会让下一次审核
（人的或 AI 的）从错误的前提出发。

## 关联

- [`../../docs/2026-07-26_increment-update-chain-audit-report.md`](../../docs/2026-07-26_increment-update-chain-audit-report.md) A1 / A2
- 案例 [13 mesh panic 炸看门狗](case-13-mesh-panic-kills-the-watchdog.md)（同一条链上更严重的失败传播问题）
- 案例 [07 死信复活](case-07-cascade-expand-and-dead-letter.md)（drain 的另一半：毒任务防护）
