# 案例 14 · 同窗口重放必须收敛：pe_owner 幂等 + SurrealQL 转义

<sub>族 D 水位与重放 · Medium · 已修 · 证据层 B（单测）+ C（实库重放）</sub>

## 一句话

落库是 500 条一块分块提交的，失败后按同一窗口重放——只要有一条写不是幂等的，
这个 dbnum 就会**反复失败、水位永远不动**。

## 现象

**F4（非幂等）**：`Add` 路径对 `pe_owner` 用裸 `INSERT RELATION`（`../pdms-io/src/io.rs` 的 `to_surql`），
写之前不删；而 `Modified` 路径是有 `DELETE pe:{id}<-pe_owner` 的。落库按 `TX_CHUNK = 500` 分块提交
（整窗口非单事务），于是：

```text
早块提交成功  →  后块失败  →  按 ADR-001 同窗口重放
                                   ↓
                   重放早块时复合 id [pe:{id}, i] 已存在
                                   ↓
                   INSERT 撞重复 → 该 dbnum 反复失败，水位卡死
```

**F5（未转义）**：`to_modify_surql` 的 `name = '{}'` 与 `update_datacenter_version` 的插值未转义
（而 `dbnum_state::escape_surql_str` 早就有了）。名字里含 `'` 或 `\`（中文录入 / Windows 路径）
会破坏 SQL，甚至构成注入面。

## 证据

- [`../../docs/specs/incr-gen-fixes/spec.md`](../../docs/specs/incr-gen-fixes/spec.md) **F4 / F5**（Medium）。
- 分块提交的事实在 `increment_pipeline.rs:716`：`const TX_CHUNK: usize = 500;`，
  注释说明了动机——绕开 SurrealDB ws 通道上限（amssys 冷启动窗口会撑爆）。
- 收敛性要求由 [`ADR-001`](../../docs/adr/ADR-001-dbnum-update-state.md) 推导而来：
  「失败批次不推进水位、按同一窗口重试」只有在**重试总能成功**时才是安全的；
  非幂等写把这条前提直接否定了。

## 根因

两条写路径（`Add` / `Modified`）由不同时期加入，只有后者考虑了重复执行。
在整窗口单事务的**想象**中这不构成问题——回滚会抹掉早块。但实现是分块提交，
早块提交后就落地了，重放必然会二次执行它。

**这正是注释漂移的实际代价**：审核 A2 发现 `persist_latest_main_data` 顶部与 `wrap_in_transaction`
的文档都还在宣称「整窗口单事务、要么整体回滚，绝不留下半写状态」。谁读了那段注释，
谁就会认为 `Add` 不需要幂等。

## 修法

**F4**（T401–T403）：`to_surql` 的 `Add` 分支在 `INSERT RELATION INTO pe_owner` 前拼上
`DELETE pe:{id}<-pe_owner;`，与 `Modified` 对齐。等价的 create-or-replace 也可以，
关键是**先删后插**这个顺序必须体现在渲染串里。

**F5**（T501–T503）：`../pdms-io/src/io.rs` 新增 `escape_surql_str`，`to_modify_surql` 的
NAME 两处已转义；`gen-model` 侧复核 `update_datacenter_version` 仅插值枚举 / 数字，
**无外部字符串注入面**，无需改。要点是「有一处可复用的转义工具」，避免各写各的。

## 验证

- 语句级单测 `add_relate_idempotency_tests`（3 个用例）：含 children 的 `Add` 渲染出的 SQL 里
  `DELETE pe:{id}<-pe_owner` 必须**早于** `INSERT RELATION`；同参数重复渲染字节一致；
  无 children 时完全不触碰 `pe_owner`。
- 实库 `live_add_pe_owner_replay_is_idempotent`：取**真实 `to_surql` 输出**里的 `pe_owner` 两句
  连跑两遍（模拟同窗口重放），断言第二遍不报错且关系数**恰为 children 数、不重复累积**。
  只回放关系语句、不回放 pe / noun 主记录，使断言不依赖属性载荷完整度。
- 注意跑法：`cargo test -p pdms_io --lib add_relate_idempotency` **必须在 gen-model 工作区跑**——
  pdms-io 单独构建会因 `parse_pdms_db` 的 gitee revision 失效而失败，gen-model 的 `[patch]`
  把它指向 `vendor/aios-parse-pdms`。

## 一条相关的语义实测

批量化改造前，用 `bin/surreal.exe` 起内存实例（2.1.4，与生产同版）把几条前提坐实了，**不是推断**：

- 同一条 query 里重复 `LET $pe = …` 合法，后续语句读到的是新值 → 逐元素 SQL 可以**原样拼接**；
- 一条语句报错**不阻断后续语句**（非事务）→ 批量化保留了原来「尽力而为」的行为；
- `UPDATE` 到不存在的记录 / `NONE` id 是**静默 no-op** → 把错误从吞掉改成上报不会刷屏。

第三条同时解释了为什么 `datacenter_version` 的 `UPDATE` 是幂等的（见案例 11）。

## 规律

**幂等性不是写法偏好，而是重试策略的前提条件。** 一旦选择了「失败不推水位、按同窗口重放」，
这条链上的每一次写都必须能被安全地执行两次。检查方法很机械：把落库语句逐类过一遍，
问「连跑两遍会怎样」——`INSERT` 撞重复、计数器累加、追加型写入都是危险信号。

**「我们是单事务」这句话必须与实现同步。** 分块提交与单事务对幂等性的要求完全相反，
而这个差别只体现在一个常量和几行循环里。任何声称原子性的注释，都应该指向那段代码本身。

## 关联

- [`spec.md F4 / F5`](../../docs/specs/incr-gen-fixes/spec.md) · [`ADR-001`](../../docs/adr/ADR-001-dbnum-update-state.md)
- 案例 [11 水位三段式](case-11-watermark-three-phase.md)（分块提交与 finalize 单事务的分界）
- 案例 [19 窗口折叠](case-19-window-folding.md)（折叠的安全性同样依赖「没有语句读另一条记录」）
