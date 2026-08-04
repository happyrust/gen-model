# 案例 07 · CascadeExpand 种子与死信复活：SET 子句顺序即功能

<sub>族 B 反向级联 · Low（但极脆）· 已钉断言 · 证据层 B（SurrealDB 一次性实例实测）</sub>

## 一句话

把 `attempts = …` 这一行挪到 `source_end_sesno = …` 之后，所有死信任务将**永远失去复活能力**——
没有报错、没有告警、没有任何测试会红。

## 现象

模型工作队列用 `attempts` 做毒任务防护：连续失败 5 次的任务进死信，被 drain 的
`(attempts?:0) < 5` 门槛排除，不再每个 watcher 周期白付一次完整几何生成。

复活机制是：**新会话再次触及同一目标时把 `attempts` 归零**，任务自动复活。这条语义完全依赖
`UPSERT … SET` 子句的**书写顺序**——SurrealDB 对 SET 子句是顺序求值，后面的子句读得到前面刚写的值。

```sql
-- 现有实现（model_update_pending.rs:146-148）
attempts         = IF {end_sesno} > (source_end_sesno?:0) THEN 0 ELSE attempts?:0 END,
last_error       = IF {end_sesno} > (source_end_sesno?:0) THEN NONE ELSE last_error END,
source_end_sesno = math::max([source_end_sesno?:0, {end_sesno}]),
```

`attempts` 的判定读的是 `source_end_sesno` 的**旧值**。一旦 `source_end_sesno` 先被写成新值，
`{end_sesno} > (source_end_sesno?:0)` 恒为假，`attempts` 永远保持 5。

## 证据

用 `bin/surreal.exe` 2.1.4 起一次性内存实例（`127.0.0.1:8098`，全程未触碰 `:8009`）实测，
出处 [`../../docs/2026-07-26_increment-update-chain-audit-round2.md`](../../docs/2026-07-26_increment-update-chain-audit-round2.md) 第五节：

先确认复活语义在反复展开下**本身是成立的**：

| 场景 | `attempts` | 判定 |
|---|---:|---|
| 首次入队（sesno 100） | 0 | — |
| 连续失败 5 次 | 5 | 进死信，被 drain 门槛排除 |
| **同 sesno 再次 upsert**（cascade 重复展开继承父 sesno） | **5** | 保持死信，未被误复活 ✅ |
| 更新会话（sesno 120） | **0**，`last_error` 清空 | 正常复活 ✅ |

再对同一条记录（`source_end_sesno=100, attempts=5`）用两种写法 upsert 到 sesno 120：

| SET 子句顺序 | 结果 |
|---|---|
| `attempts` 在前（= 现有实现） | `attempts = 0` ✅ 复活 |
| `source_end_sesno` 在前 | `attempts = 5` ❌ 永不复活 |

审核时的原始状况：现有测试只覆盖 `render_drain_select`、`joins_regen_batch` 和 finalize 事务的组装，
**没有一条断言 `attempts` 归零语义**。

## 根因

一条**功能性约束**被写成了普通的代码格式。三行 SET 子句的相对顺序决定了整个死信复活机制是否工作，
但它在源码里看起来只是三行并列的赋值——一次自动格式化、一次「把字段按字母序排一排」的整理就能毁掉它，
而后果（所有死信永久消失在自动路径里）不会以任何形式冒出来。

## 修法

不改语句顺序（现有顺序是对的），而是**把顺序钉成断言**。
[`../../src/data_interface/model_update_pending.rs:612`](../../src/data_interface/model_update_pending.rs)
的测试直接在渲染出的 SQL 串上找三个子句的位置并比较先后：

```rust
let attempts_at   = sql.find("attempts = IF")...;
let last_error_at = sql.find("last_error = IF")...;
let sesno_write_at = sql.find("source_end_sesno = math::max")...;
assert!(/* attempts_at < sesno_write_at && last_error_at < sesno_write_at */);
```

doc 注释同时写明了**为什么**（SurrealDB 顺序求值 + 复活条件读旧值），让下一个读到它的人不必再去实测一遍。

同批还有一处相邻加固（毒根防放大）：drain 的 SELECT 带 `attempts < MAX_ATTEMPTS(5)` 门槛
（与 `side_effect_pending` 同策略）；`attempts > 0` 或 `target_refno` 解析失败的根**不进批量、单独逐根跑**，
避免一个已知坏根把整批健康根反复拖进「批量失败 → 逐根回退」的双倍代价。
死信对手动更新路径仍然可见（`load_pending_model_units` 不带该门槛），预览 / 手动重试是检视与复活死信的入口。

## 验证

`cargo test --lib` 中的顺序断言（渲染串比对，不连库）+ round2 报告里的三组一次性实例探针：
死信复活、SET 顺序、引用计数与孤儿。

## 仍然开着的两条（Low）

- **B6 派生根按目录库 dbnum 记账**。`record_id` 由 `dbnum + action + target_refno` 组成
  （`model_update_pending.rs:62`），而 `CascadeExpand` 派生出的 `RegenRoot` 继承的是
  **触发级联的那个目录库**的 dbnum（`:414`），根本身通常属于某个设计库。于是同一个生成根可能有两行：
  一行来自 CATA 级联，一行来自 DESI 直接变更。后果有限——批量路径按 `target_refno` 去重，同轮不重复生成，
  两行都会被清掉；残留的是**复活口径**：CATA 级联派生的死信只能靠新的 **CATA** 会话复活，
  设计库那侧再怎么改都碰不到它。
- **`status` 显示口径**。`status = 'pending'` 是无条件写的，所以一条 `attempts = 5` 的死信在表里
  显示为「排队中」，手动预览里看起来正常，实际自动路径永远不会执行它。

## 规律

**凡是「顺序 / 位置」承载了语义的地方，都要有一条测试把它焊死。** 这类约束的共同特征是：
违反它不会编译失败、不会运行报错，只会让某个远处的行为静默消失。识别方法很朴素——
如果你在 review 时想说「这两行不能换位置」，那就应该有一条断言替你说。

第二条：**测试要覆盖「不该发生」的那一侧。** 这里真正危险的不是「复活失效」被测到，
而是复活失效之后**队列看起来是干净的**（status=pending、无错误日志）。可观测性缺失会把
一个 Low 级缺陷变成永远查不出来的缺陷。

## 关联

- [`../../docs/2026-07-26_increment-update-chain-audit-round2.md`](../../docs/2026-07-26_increment-update-chain-audit-round2.md) B5 / B6
- 案例 [05 共享 SPCO 反向传播](case-05-shared-spco-reverse-cascade.md)（种子从哪来）
- 案例 [12 drain 失败隔离](case-12-drain-failure-isolation.md)（同一个 drain 循环里的另一处失败处理）
