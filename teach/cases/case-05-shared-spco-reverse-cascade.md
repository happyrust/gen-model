# 案例 05 · 改一个共享 SPCO，72 个消费者全部重生成

<sub>族 B 反向级联 · High · 已修（源端 UI 触发待授权）· 证据层 B（单测）+ C（实库）</sub>

## 一句话

正向问题（这个实例用的是哪个目录元件）读一次属性就有答案；反向问题（这个目录元件被谁用着）在
PDMS 数据里**根本没有出口**——不建索引，改共享目录件时所有使用者的几何都会陈旧。

## 现象

修改一个被多个设计实例引用的**共享目录 / 规格元件本身**（`SPCO` / `SPEC` / 几何集）时，
增量链路只把这个 CATA 元素归一到它自己的 CATA owner 重生成，**不反查引用它的设计实例**。
于是：

- 目录侧容器刷新了；
- 设计侧那 N 个管件、阀门、风阀的几何**全部停在旧形状**，而且没有任何告警。

`model_refresh.rs` 里长期挂着描述这个缺口的 TODO。

## 证据

**内核怎么做的（A 层）**：core.dll 用的是**存储型 back-ref 逆指针**——
`ATT_BREF` / `ATT_SPBREF` / `ATT_SCBREF` / `ATT_TABREF` / `ATT_DBREF` 等属性，由
`DB_ElementChangesPlugger::PostSetRefListAttribute`（`0x591E780`）在写引用列表属性时维护
（它本身只广播，真正写 back-ref 的是订阅者，订阅入口 `SubscribePostSetRefListAttribute` `0x581f7e0`）。
另有 `DB_Clone::getRelatedElements`。

**为什么不能直接读（A→B 的取舍）**：实测 `parse_raw_ele_data_with_info` 按 **schema 固定偏移**解码属性，
而 back-ref 不在 schema（`all_attr_info.json`）里——PDMS 的 back-ref 是**独立引用表 / 系统维护结构**，
不是元素隐含块里的固定偏移属性。**离线不可得**。正向引用属性（`SPRE`/`CATR`/`DESP`/`PARA`/`PRTREF`/`HREF`/`TREF`…，
`att_type == ELEMENT`）则可以正常解码。

**实库分布（C 层）**：`BRAN 24381/100817` 能查到三条反向关联：
`SPCO 23274/295421`（`/CADCHVACSPEC/HRTUBEA`）、`SPEC 23274/295406`、`SPEC 23274/295635`。
全量反向索引重建：扫描 **274,215** 个当前元素，写入 **46,244** 条去重边。
共享 `SPCO 23274/295504`（`/CADCHVACSPEC/RVCD`）的正向 DAMP 消费者为 **72**，`ref_rev` 反向边也是 **72**——
使用者集合完整，不存在遗漏。

## 根因

数据模型只有正向边。任何「谁在用我」的问题都需要**全表扫描**才能回答，而增量链路不可能为每次变更扫全库。

## 修法

[`ADR-003`](../../docs/adr/ADR-003-reverse-cascade-index.md)：自建「正向引用反转」持久索引，
落库时同步维护；[`ADR-008`](../../docs/adr/ADR-008-catalog-reverse-propagation.md)：把它接进模型工作计划。

**索引形态**（容易看反的地方）：`ref_rev` 是 SurrealDB 图边表，**存的是正向边**——
`in` = 引用方（设计实例），`out` = 被引用方（目录元件），边 id 是两端拼成的复合主键（重复写幂等）。
「反向」体现在**查询方向**上：

```text
维护时顺着走：  pe:ELBO_A      ->ref_rev   ⇒ 这个元素的全部出边（清理只需 DELETE pe:X->ref_rev）
查询时倒着走：  pe:CAT_ELBOW90 <-ref_rev   ⇒ 所有引用它的 in
```

这样存的理由是维护成本最低：清理一个元素的旧引用沿它自己的邻接表走，**不用扫表、也不用建二级索引**。

**触发链路**：

```mermaid
flowchart LR
    A["CATA 库窗口<br/>净变化 Modified/Deleted 的目录元素"] --> B["落 CascadeExpand 种子<br/>（持久化，随水位一起提交）"]
    B --> C["drain 时 expand_live_reverse_cascade<br/>沿 ref_rev 反查引用者"]
    C --> D["只对设计库引用者产根<br/>目录/规格中间层只上溯"]
    D --> E["派生 RegenRoot 批量重生成"]
```

三处刻意的设计：

1. **反查失败非致命**。索引维护失败只记 warning，不阻断数据批次与水位（沿用 ADR-003 的降级策略）；
   反查失败则持久化 `cascade_expand` 种子，成功重查后**先幂等写入派生根任务再删种子**。
2. **CATA 只落种子，不做 rollup**。`build_model_update_plan` 收紧为 DESI-only 时曾把 CATA 触发一并断开；
   现改为 CATA 专用轻量分支——只为净变化 Modified / Deleted 且影响模型的目录元素落种子，
   **无 rollup / Transform / DeleteCleanup**（T805）。
3. **只对设计库引用者产根**。目录 / 规格中间层只上溯，防止目录 owner 链被误当 Normal 根、产生永远失败的任务。
   净新增（`Add`）的 CATA 不落种子——新目录件还没有人引用，这是业务决策而非缺陷。

## 验证

- 单测：`referenced → referrer` 边去重、排除 self、**传递级联**、环安全。
- 实库（2026-07-26）`live_shared_spco_cascade_regenerates_every_consumer`：对共享 `SPCO 23274/295504`
  单次 drain 完成 **1 个 CascadeExpand + 67 个 BRAN 根**，队列清空，**72/72 个 DAMP 消费者均存在模型**，
  耗时 585.32 s。
- 仍缺：在 E3D 里实际修改该 SPCO 后的**源 session / UI 触发**证据，以及前后三维截图。
  当前证据链是「反查 → 展开 → 重生成」这一段，不含「E3D 编辑 → session 文件」那一段。

## 规律

**当权威机制不可得时，重建它的等价物，并把「不可得」的理由写进决策。** ADR-003 没有停在
「core.dll 有 back-ref，我们没有」，而是记清了三件事：内核用什么（存储型逆指针）、为什么读不到
（不在 schema 固定偏移、是系统维护结构）、替代方案的代价是什么（多维护一份索引，间接引用需自行覆盖）。
这三条让后来者能判断什么时候该推翻这个决策。

第二条：**降级要降到「可恢复」而不是「已丢失」。** 反查失败时如果只打一条 warning 就结束，
这次级联就永久丢了；落一颗持久化种子，则下一轮 drain 还能补上。

## 关联

- [`ADR-003`](../../docs/adr/ADR-003-reverse-cascade-index.md) · [`ADR-008`](../../docs/adr/ADR-008-catalog-reverse-propagation.md) · [`spec.md F8`](../../docs/specs/incr-gen-fixes/spec.md)
- 课程 [`../lessons/0002-ref-rev-reverse-reference-index.html`](../lessons/0002-ref-rev-reverse-reference-index.html)（10 张流程图讲透写 / 查 / 用 / 坏了怎么办）
- 案例 [06 建边资格解耦](case-06-ref-edge-eligibility-decoupled.md)、[07 CascadeExpand 与死信](case-07-cascade-expand-and-dead-letter.md)
