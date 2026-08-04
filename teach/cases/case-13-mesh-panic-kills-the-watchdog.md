# 案例 13 · mesh 生成失败 panic 炸掉看门狗

<sub>族 D 水位与重放 · High · 已修 · 证据层 B（单测）+ C（实库故障注入）</sub>

## 一句话

一句 `.expect("更新模型数据失败")` 让「一个元件的网格生成失败」升级成「整个增量看门狗停止工作」。

## 现象

`gen_all_geos_data` 里：

```rust
process_meshes_update_db_deep(...).await.expect("更新模型数据失败");
```

mesh 失败时 **panic**，而不是返回 `Err`。panic 从 `async_watch` 循环里向上 unwind，
看门狗任务终止——此后**所有** dbnum 的增量更新都停了，而日志里只有一条 panic。

## 证据

缺陷登记：[`../../docs/specs/incr-gen-fixes/spec.md`](../../docs/specs/incr-gen-fixes/spec.md) **F2（High）**。

根因描述得很直白：`.expect()` 与**同文件上方**「不再 unwrap、错误必须向上传播」的设计自相矛盾。
也就是说，这条规则在同一个文件里被写下来又被违反。

放大机制还有一层：`db_model.rs:79/156` 两处调用方对 `async_watch` 的返回是 `.unwrap()`，
而 `while let Some(res) = rx.next().await` 结束后 `async_watch` 会静默返回 `Ok(())`——
**看门狗静默退出不会有任何告警**（这一条记录在
[`../../docs/2026-07-26_p3-t903-t904-assessment.md`](../../docs/2026-07-26_p3-t903-t904-assessment.md)，属相邻缺口、未修）。

## 根因

在一条**长期运行的后台循环**里，把「这次工作失败」当成了「程序状态不可信」。
`.expect()` 的语义是后者——它宣称「到这里还出错说明前提被破坏，不如崩掉」。
但一个元件的网格化失败完全是可预期的局部事件（几何退化、OCC 三角化失败、库连接抖动），
它不该动摇进程。

## 修法

F2 的需求（MUST）：

1. mesh 生成失败 MUST 以 `Err` 向上传播（`?`），使 `ModelRefreshPolicy::generate_roots` 返回 `Err`；
2. 该失败 MUST 使对应 `model_update_pending` 根任务标记为 failed 且**可重试**，并且
   **不回滚已成功的数据与水位**（ADR-001）；
3. 增量路径中 MUST 不存在 `.expect()/.unwrap()` 直接对可恢复的库 / 几何错误 panic；
   **全量路径（同函数另一分支）MUST 一并对齐**。

落地（[`tasks.md`](../../docs/specs/incr-gen-fixes/tasks.md) T201–T205）：

- `gen_model.rs` 增量分支与全量分支的 `.expect(...)` 都改 `?`；
- `occ_generate.rs` 的 `gen_inst_meshes` / `update_inst_relate_aabbs_by_refnos` / 入口查询改 `?`；
  `save_instance_data`（并行版，增量实际用的那个）本就把错误聚合为 `Err`；
- 失败由 `model_update_pending` 标记 failed 并重试；旧的 `SideEffectCompensator::ModelRefresh`
  只保留兼容历史记录。

**F2 是先做的那一项**——F1（删除清理）明确「依赖 F2 通道」：清理失败要能向上抛、被记为失败、
可重试，前提是这条错误通道存在。

## 验证

实库（2026-07-26）`live_generation_failure_keeps_pending_and_watermark`：
连续注入批量与逐根生成失败，断言

- 进程不崩、`async_watch` 继续运行；
- 对应根任务 `status = failed`、`attempts = 1`；
- 该 dbnum 的 `applied_sesno` **保持 42 不动**（数据已落库的部分不回退）。

## 规律

**在守护进程里，`unwrap` / `expect` 的作用域就是整个进程。** 判断该不该用它只有一个问题：
这个错误发生时，我是希望「这一件事失败」还是「所有事都停下来」？后台循环里几乎永远是前者。

**错误通道要先修通，再修依赖它的东西。** F1 的清理失败、F3 的补偿重试、案例 12 的失败隔离，
全都建立在「失败能被表达成 `Err` 并被记录成可重试任务」之上。在 panic 还会炸进程的时候，
这些机制都是纸面上的。修复顺序 F2 → F1 不是偶然。

## 关联

- [`spec.md F2`](../../docs/specs/incr-gen-fixes/spec.md) · [`tasks.md T201–T205`](../../docs/specs/incr-gen-fixes/tasks.md)
- 案例 [08 删除元素后旧几何残留](case-08-deleted-element-orphan-geometry.md)（F1 依赖本案例打通的通道）
- 案例 [12 drain 失败隔离](case-12-drain-failure-isolation.md)、[11 水位三段式](case-11-watermark-three-phase.md)
- 未修的相邻缺口：`watcher.watch(x).expect("文件监控设置失败")`（某个监控目录不可达即 panic，
  而不是跳过该目录继续监听其余目录）
