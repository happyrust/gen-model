# ADR-043：`insts_flat` 失效协议——缓存维护的成本必须与变更量成正比

## 状态

提议（2026-08-23）。修订 `docs/plans/2026-08-07-inst-relate-anc-u64-hierarchy-query-plan.md`
§P4（写时物化）与 `specs/019-booled-flat-backfill-closure/`；引用 ADR-011、ADR-025、
ADR-038、ADR-041。落地规格 `specs/025-insts-flat-invalidation/`。

## 背景

`inst_relate` 上有三个派生列，都是为读侧（plant-ui 加载模型）准备的：`aabb_d`、
`world_trans_d`、`insts_flat`。前两个与各自的指针**同语句原子写**，永不变旧。第三个
不行——它缓存的是一次图遍历子查询：

```sql
SELECT trans.d AS transform, record::id(out) AS geo_hash
FROM out->geo_relate
WHERE visible && out.meshed && trans.d != none && geo_type = 'Pos'
```

写路径是 journal（ADR-038），journal 里只能是纯字面量，这个子查询必须现场对图求值，
写不成字面量。P4 因此把它放到「持久层非 journal 路径」上事后收口：
`pdms_inst::sweep_inst_relate_flat`，挂在启动序列与 batch worker 空闲轮两处，
空闲轮那处由一个进程级 `AtomicBool` 脏位门控。

### 收益是真的

P2/P4 的逐句探针（AMS `inst_relate` 53,582 行）：平表扫描 0.7s 是地板，
`aabb.d`/`world_trans.d` 解引用 +2.0s，`insts` 子查询 **+8.7s**。物化加上另外三项
客户端优化把 plant-ui 全场加载从 253.3s 压到 2.73s。**缓存本身不容置疑。**

### 成本失控了

2026-08-23 从一份 27.5 小时的服务日志（watch 限定 dbnum 8000，12.6 MB stdout）逐行统计：

| 项 | 次数 | 总计 | 均值 |
| --- | --- | --- | --- |
| `inst_relate` 平表清扫 | 1,021 | **4.33 h** | 15.26s（中位 6.87s，max 73.8s）|
| 模型结点更新（真几何）| 1,199 | **1.50 h** | 4.51s |
| 元件库几何 `gen_cata_geos` | 671 | 0.56 h | 3.00s |

**一个性能缓存的维护开销是它服务的那件事的 2.9 倍。** 清扫每轮跑三段，三段的谓词
（`insts_flat = NONE`、`booled_id` 三重判定、`booled_id = '' OR lowercase = 'none'`）
在 `inst_relate` 上都不可索引——该表只有 `anc` 与 `dbnum` 两个索引——所以每轮是三次
全表扫。第三段甚至连 `LIMIT` 都没有，只为打一行警告。清扫的中位战果是「补 16 行」，
恰好等于 `DRAIN_PAGE_SIZE`。

更要命的是形状：清扫谓词里**没有 dbnum**，扫的是全库。一次首次导入基线（7997 约
2,890 个生成根 = 约 181 个空闲轮）每跑一轮就往 `inst_relate` 添一批行，下一轮的三次
全表扫就更慢一点。**总开销对生成根数是平方级的。**

### 而当前的清扫并不能保证正确性

外部二审（GPT-5.6 Sol，2026-08-23，留证
`docs/evidence/2026-08-23-insts-flat-invalidation/oracle-review.md`）指出一条本仓
自己的源码就能坐实的反例：

`src/fast_model/pdms_inst.rs` 的 `render_inst_geo_upsert` 带一个「重置 bad」开关，
为真时发 `UPDATE inst_geo:⟨hash⟩ SET bad = false;`——注释写明理由是「旧 `bad=true` 不清，
`gen_inst_meshes` 会永远跳过」。于是共享 geo `G` 存在这条路径：

1. A、B 两行都引用 `G`；首轮 `G` 是 `bad=true, meshed=false`，两行的 `insts_flat`
   都不含 `G`（被 `out.meshed` 过滤）。
2. 只对 B 做定向重生成：`G` 的 bad 位被清 → `G.meshed = true`。
3. A 没被重建。A 的 `insts_flat` 是**旧值且非 NONE**，读侧三分法不落兜底，
   直接采信——这是 RM13 的同类形态。

**现有的全表清扫也修不了它**：回填段只圈 `insts_flat = NONE`，修复段只圈
`booled_id` 不符。一个「少了一个正体」的数组两段都命中不了。所以这不是「改成按 refno
清扫会不会引入的问题」，而是**今天就存在的缺口**，只是被全表扫的开销掩盖着。

P4 设计文档当年取消 `idx_inst_relate_out` 的论证是「置 meshed 的生成批与建行同任务同
refno 锚点，任务成功 ⇒ 可达 geo 全 meshed|bad」。`reset_bad` 这条路径不在那个论证的
覆盖范围里，而且没有测试钉住它。

### 还有一处语义自相矛盾

回填段写的是 `IF booled_id != NONE THEN [{ geo_hash: booled_id }] ELSE (子查询) END`。
空串不是 `NONE`，所以 `booled_id = ''` 的行会被回填成 `insts_flat = [{ geo_hash: '' }]`；
而同一个函数下面的修复段与脏值段把空串和字面 `'none'` 明确定义为「没有布尔成品，
不应写进平表」。两段对同一个值的判定相反。

## 决策

### 1. 缓存留下，全表清扫退出热路径

`insts_flat` 是缓存不是权威，但它的收益已经用实测坐实（+8.7s/5.3 万行，且随表增长）。
删掉它等于把 plant-ui 打回十几秒。**不动缓存本身。**

退出的是「每个空闲轮三次全表扫」。全表扫只保留两个身份：启动序列的存量回填、
人工诊断入口。

### 2. 缓存维护的成本与变更量成正比，不与表容量成正比（NON-NEGOTIABLE）

任何写在增量热路径上的缓存维护动作，其代价必须只与本轮实际变更的行数有关。
落到实现上：按 record id 点名 `UPDATE`，不发无索引谓词。

这条同时是一条**通用禁令**：空闲轮里不得出现任何周期性的全表正确性探针。
`LIMIT 1` 也不豁免——谓词无索引时 `LIMIT 1` 仍可能扫很远。这类检查一律降为
低频审计或启动一次。

### 3. 失效集是「终态受影响集」，不是「本轮写过的行」

新概念 `flat_invalidated_refnos`，定义为「本次提交之后，哪些 `inst_relate` 行的
`insts_flat` **逻辑值**可能变了」。它**不等于** `SavePlan::written_refnos`：

- 现在的脏位由 `!plan.packets.is_empty()` 触发，而 packets 含 `inst_geo` / `geo_relate` /
  共享内容等，覆盖面严格大于 `written_refnos`（只收实际写出的 `inst_relate` 行）。
- 子查询依赖 `geo_relate` 的增删与 `visible` / `geo_type` / `trans`、`inst_geo.meshed`、
  以及 `booled_id`。这几项的写路径都必须能映射回受影响的 refno。

### 4. 失效集在**生成任务终态之后**消费，不在 save plan 写完之后

现在 `mark_insts_flat_dirty()` 就写在 `save_instance_data` 的 packets 执行完之后。
那一刻只能证明 save plan 完成，证明不了 mesh / OCC / manifold / AABB 已经跑完，
而 P4 的正确性论证恰恰依赖「任务成功 ⇒ 可达 geo 全 meshed|bad」。让代码结构表达这个
事实：收集 → 生成任务成功 → 才入刷新队列。

推论：**不得在建行时就把正体实例写进 `insts_flat`**。带负实体的构件在 OCC 跑完之前
写正体，读侧会因为「三副本齐活」而不落兜底，当场画出未做布尔的正体——把 RM13 从
「历史存量」变成「每次生成都有的窗口」。

### 5. 缓存变旧前必须先 durable 失效（NON-NEGOTIABLE）

> 凡是可能让一个**已物化**的缓存变旧的写，必须在同一事务里要么写入新的正确值，
> 要么把该行的缓存 durable 置为「无效」。

有了这条，刷新队列本身才允许是 best-effort：`有效缓存 → 依赖变化 → 缓存失效 →
异步刷新 → 有效缓存`，任何一步崩溃最多掉到读侧兜底（**慢，但不错**）。

没有这条，一个纯内存的失效集在崩溃时就是真正的正确性丢失——读者继续采信旧的非
NONE 缓存，而且没有任何路径会再碰它。

今天 `inst_relate` 的替换写（`DELETE` + `INSERT`，新行不带 `insts_flat`）与 OCC 成功
路径（`booled_id` 与 `insts_flat` 同语句）天然满足这条；共享 geo 的 `bad → meshed`
不满足，是本轮要补的缺口。

### 6. 存量修复改为带标记的数据库级 migration

修复段修的是 RM13 那批历史行，不是新产生的形态。改成 `if 迁移标记不存在 { 修复 →
复核无残留 → 落标记 }`。判据是**库上的标记**，不是「本进程跑过一次」——旧备份恢复、
库拷贝、滚动部署期间短暂跑起旧 writer，都会重新产生老格式。

### 7. 读侧加一道行内自检与版本位

`booled_id` 有效而 `insts_flat[0].geo_hash` 与之不符，是**行内就能判定**的，不需要图
查询。读侧拿到这个形态时当作缓存未命中转 pass2。再加 `insts_flat_ver`，读者只信当前
版本，以后物化规则演进时旧缓存自动退化为兜底而不是错值。

这两项让 RM13 类回归的后果封顶在「变慢」。

## 长期方向（不在本 ADR 的落地范围内）

把 `insts_flat` 从「数据库图查询的缓存」变成「**生成任务的终态产物**」：Rust 在几何
终态（mesh 结果已知、布尔结果已知）时构造纯字面量写下去，与 `aabb_d` /
`world_trans_d` 同族。做成之后常态清扫可以完全退役，journal 纯数据纪律也不破——
journal 里仍然只有 `SET insts_flat = [{transform: …, geo_hash: …}]`。

代价说清楚：那个过滤谓词会变成 SurrealQL 一份、Rust 一份，两处必须永远一致，
而不一致的表现是静默渲染错误。**这条路线的前置不是性能，是先把共享 geo 的失效闭上环**
（见下）。

## 未决：共享 geo 的 `bad → meshed`

两条路线，本 ADR 不定，交给 spec 的前置实测：

- **路线 A（不可变）**：一个 `geo_hash` 一旦终态 `bad`，永不在同一 hash 上恢复
  `meshed`；要重试就把 mesh 算法版本纳入 hash。依赖变成真不可变，反向失效不需要。
- **路线 B（反向失效）**：共享 geo 从「不参与 flat」变成「参与 flat」时，找出所有引用
  它的 `inst_relate` 行，置无效并入队刷新。这类恢复是低频事件，反向遍历贵一点也远比
  每轮全表扫合理。

倾向 B，除非 A 的不变量能被现成机制保证。**无论选哪条，先有那条回归测试**：两个
refno 共用一个 `bad` geo，只重试其中一个，另一个的 `insts_flat` 不得变旧。

## 后果

- 常态开销从「3 次全表扫 / 轮」降到「约 16 个 record-id 点名 UPDATE / 轮」，首次导入
  基线的平方项消失。
- `written_refnos` 不再等同于失效集，新增写路径必须显式回答「它会不会改变 flat 的逻辑
  值」，答不出就是缺陷（宪法 III）。
- 第 5 条把一个当前隐含的假设升级成硬纪律；它同时是把 `insts_flat` 改成终态产物的
  前置条件。
- 修复段变成 migration 之后，老库首次启动仍会付一次全表价——这是设计意图，不是回归。

## 否决方案

- **删掉 `insts_flat`，读侧一律走 slim**：+8.7s/5.3 万行且随表线性增长，7997 之后更差。
  拿正确性问题当理由删一个已验证的性能设施，是把两件事混在一起。
- **给 `insts_flat = NONE` 建索引**：`NONE` 在 SurrealDB 里是「字段不存在」而不是普通
  空值，2.1.4 上「字段不存在」能否走普通索引查找没有依据。要走这条得先用双引擎
  `EXPLAIN` 验（照 `anc CONTAINS` 那次的模板），而且它要为 55 万干净行维护一个低基数
  索引。
- **把刷新工作塞进 `model_update_pending`**：那张表自己一个索引都没有，且已经承担几何
  重试与死信语义。把一个性能缓存的刷新塞进去会造出队列公平性与饥饿问题（ADR-011 的
  单队列前提说的是数据批次，不是「所有异步工作都往同一张表挤」）。
- **保持现状只调页大小**：页大小摊薄的是生成器启动开销，不是全表扫；清扫的代价与页
  大小无关，与表容量有关。
