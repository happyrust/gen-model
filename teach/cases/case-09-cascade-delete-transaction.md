# 案例 09 · 级联清理的事务边界：中途失败后永久孤儿，还上报成功

<sub>族 C 删除清理 · Medium · 已修 · 证据层 B（SurrealDB 一次性实例实测）</sub>

## 一句话

清理条件读的是它自己刚删掉的那条边——删完边、条件块没跑完就失败，重试时条件恒为假，
清理被静默跳过，函数却返回 `Ok`。案例 08 消灭的孤儿几何，换了个入口回来了。

## 现象

删除清理对每个 refno 发三条语句：

```sql
let $old_inst = (select value out from inst_relate:X)[0];   -- 先记住 inst_info
delete from inst_relate:X;                                   -- 删边
if $old_inst != none and array::len($old_inst<-inst_relate) = 0 { ... };  -- 再决定要不要删 inst_info
```

第三条读的是第二条刚删掉的那条边的另一端。审核时这三条**不在事务里**（只是 `join(";")`），
而团队自己实测过「一条语句报错不阻断后续语句」。于是存在这个窗口：
`delete from` 已执行、`if` 块未执行（`if` 自身报错 / 连接中断 / 服务端重启）。

## 证据

出处 [`../../docs/2026-07-26_increment-update-chain-audit-round2.md`](../../docs/2026-07-26_increment-update-chain-audit-round2.md) 第三节，
用一次性内存实例（`127.0.0.1:8098`）实跑：

**先证伪一个假设**。Oracle 怀疑的是「同一批 SQL 里多个 refno 共享一个 `inst_info` 时会脏读」——不成立：

| 时点 | `inst_info:s` | `inst_geo:g` |
|---|---|---|
| 删完第一个 refno | 存在 | 存在 |
| 删完第二个 refno | 已删 | 已删 |

同一条 query 内语句**顺序执行且后一条读得到前一条的写**，`array::len($old_inst<-inst_relate) = 0`
不会读到中间态；跨 chunk 隔离更强。

**再证实真问题（B1）**。手工制造半执行状态后重跑完整的三条语句：

```text
删掉 inst_relate:c 之后，再跑完整的三条语句
→ {inst_info: true, inst_geo: true, geo_relate: true}   // 三件套全都还在
```

因为 `$old_inst` 这时读到的是 `NONE`，`if` 条件短路，整段清理被跳过——**而函数返回 `Ok`**，
任务被当作完成删除。`inst_info / geo_relate / inst_geo` 三件套永久残留，且没有任何告警。

**顺带查出 B2**。原删除集是 `select value [out, id, in] from $old_inst->geo_relate`——
`in` 才是 `inst_info` 本身。若某个 `inst_info` **没有任何 `geo_relate` 边**（几何生成半途失败留下的），
该集合为空，`inst_info` 就删不掉。实测确认：`{inst_info: true}`。而文档注释明写它在删除范围内。

## 根因

两个独立的问题叠在一处：

1. **事务边界画错了位置**。这三条语句之间存在一个「可观察的中间态」，而中间态一旦被观察到
   （下一次重试就是观察者），清理条件的语义就反转了：从「还有人引用吗」变成「边都没了，不用管」。
2. **回收方式依赖了一条可能不存在的边**。用 `geo_relate` 的 `in` 端顺带删除 `inst_info`，
   等于假设「每个 inst_info 都至少有一条 geo_relate」——半途失败的生成过程恰好会打破这个假设。

## 修法

[`../../src/data_interface/helper.rs:60`](../../src/data_interface/helper.rs) 的 `render_cascade_delete`：

```sql
BEGIN TRANSACTION;
let $old_inst = (select value out from inst_relate:X)[0];
delete from inst_relate:X;
if $old_inst != none and array::len($old_inst<-inst_relate) = 0 {
    delete array::flatten(select value [out, id] from $old_inst->geo_relate);
    delete $old_inst;                      -- ← B2：显式回收，不靠 geo_relate 顺带
};
COMMIT TRANSACTION;
```

两个决定值得记：

- **事务只包一个 refno，不包整批**。跨 refno 的原子性反而会让一个坏 refno 拖垮整批
  （与案例 12 的失败隔离同一取向）。事务在这里的作用不是「一起成功」，而是**消除中间态**——
  半执行整体回滚，重试从干净状态开始，可自愈。
- **`inst_info` 用显式 `delete $old_inst` 回收**，不再依赖 `geo_relate` 三元组的 `in` 端。

doc 注释把「为什么需要这个事务」完整写在函数上方，包括那句关键的
「重试时 `$old_inst` 只会读到 `NONE`，整段清理被静默跳过，而函数照样返回 `Ok`」。

## 验证

单测 `cascade_delete_keeps_the_edge_delete_and_refcount_gc_in_one_transaction`：
断言渲染串以 `BEGIN TRANSACTION;` 开头、`COMMIT TRANSACTION;` 结尾，且删边语句与 GC 条件之间
**没有提交边界**。

## 规律

**「读自己刚写的东西」的语句序列，必须是原子的。** 判据很好用：如果第 N 条语句的条件依赖第 N-1 条
的副作用，那么这段代码在「执行到一半」时的语义与「从头执行」不同——而重试永远是从头执行。
不包事务的话，重试不但救不回来，还会把错误状态**固化**成正确状态。

**失败要么可重试、要么可告警，最坏的是「返回成功」。** B1 真正的危害不是残留三件套，
而是任务被标记为完成、队列干净、日志无声。任何「条件短路 → 跳过 → 返回 Ok」的路径都值得停下来
问一句：短路的那个条件，会不会正好是失败留下的？

## 关联

- [`../../docs/2026-07-26_increment-update-chain-audit-round2.md`](../../docs/2026-07-26_increment-update-chain-audit-round2.md) B1 / B2
- 案例 [08 删除元素后旧几何残留](case-08-deleted-element-orphan-geometry.md)（同一类孤儿的第一个入口）
- 案例 [11 水位三段式](case-11-watermark-three-phase.md)（另一处「事务边界该画在哪」的判断）
