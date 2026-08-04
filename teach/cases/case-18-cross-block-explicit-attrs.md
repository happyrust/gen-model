# 案例 18 · 跨块显式属性 CURD/DBLS 丢失，模型树整棵为空

<sub>族 E 解析与按需 · Critical · 已修 · 证据层 B（只读探针）+ C（真库重解析）</sub>

## 一句话

一个 `break` 让跨记录块存储的长引用列表在半路截断，`CURD`/`DBLS` 属性解析不出来——
最终表现是 `rs-plant3-d` 加载 AvevaMarineSample 时**模型树整棵为空**。

## 现象

- `rs-plant3-d` 加载模型树为空；
- 重解析 SYS 时日志反复出现「显式属性退出」；
- 真库实测：amssys 里 `24575/1309`（MDB `/MHULLFWD`）、`24575/1478`（MDB `/MHULLAFT`）、
  `24575/1494` 等元素的 `CURD`/`DBLS` 反复解析失败。

## 证据

出处 [`ADR-006-explicit-cross-block-collect-fix`](../../docs/adr/ADR-006-explicit-cross-block-collect-fix.md)。

SYS 元数据里的设计 MDB（noun = MDB）与 DB 元素带有 `CURD`（当前数据库列表）/ `DBLS` 这类
**很长的引用列表属性**，其数据**跨多个记录块（block）**存储，块之间可能夹有其它块头或
`0x00 00 00 07` 追加段（continuation segment）。

vendor 的 `collect_explict_data`（收集元素显式属性字节）在遇到

- 块头 `flag != 1`，或
- 块内 self-ref 与本元素 refno 不符，或
- 声明长度非法

时**直接 `break`**。于是跨块列表的后续块被丢弃，收集缓冲区正好停在 `CURD` 属性的 4 字节 hash 处 →
`parse_raw_explicit_attrs` 读不到 8 字节头 → 报「显式属性退出」→ 属性丢失。

## 根因

解析器把「遇到看不懂的块」当成「元素结束」。对单块元素这个假设成立，对跨块列表**恰好相反**：
块之间夹杂其它内容是正常布局，此时应该**继续找下一个匹配块**，而不是收工。

fork（`pdms-io-fork`）早已重构过这一段，本仓的 vendor 停在旧实现上——这是一次
**已修复缺陷在另一分支上重现**的典型情况。

## 修法

按「最小忠实移植」（对齐 ADR-005 的范式）把 fork 的 `collect_explict_data` 逻辑移植进 vendor，
用 vendor 现有 helper 适配：

1. 遇不匹配 / 非法块**不再 `break`，改 resync**——跳 4 字节按 word 对齐继续找下一个 `0x0001` 块，
   `MAX_RESYNC = 64` 防跑飞；
2. 新增 `collect_explicit_segmented_payload`：主段 payload 从 offset 12 起，收集其后的
   `0x07` 追加段（payload 从 offset 24 起）；
3. 主段 8 字节保留区**自适应裁剪**：全 0 → drain 8/12；否则用 `looks_like_attr_stream_start`
   判断 offset 12 vs 20，兼容两种块布局、对普通元素与旧实现字节等价；
4. 旧实现原样保留为 `collect_explict_data_legacy`（`#[allow(dead_code)]`）作对照 / 兜底。

## 验证

- `cargo check -p parse_pdms_db` / `--bin curd_parse_probe` EXIT = 0；
- 新增只读探针 `src/bin/curd_parse_probe.rs`：解析真实 amssys，确认 MDB `/MHULLAFT` / `/EQUIPMENTFWD`
  现在解析出**完整** `CURD`/`DBLS` 列表；覆盖统计 **MDB = 50 带 CURD = 48**、**DB = 110 带 STYP = 110/110**；
- 重解析 SYS 后日志「显式属性退出」= **0**（此前多次）。

## 与增量更新的关系

这个案例出现在链路的**最上游**——属性解析层。它值得放进增量案例集有三个理由：

1. **症状与增量缺陷难以区分**。「模型树为空」「某类元素没有几何」既可能是增量判定漏了，
   也可能是解析器根本没读到那条属性。排查顺序应该是**先确认数据解析对了**，再查增量逻辑。
2. **它是「模型树为空」的第一层根因**，第二层是 [`ADR-007`](../../docs/adr/ADR-007-sys-meta-parse-not-gated-by-included-files.md)
   （SYS 元数据解析不应被 included files 门控）。一个用户可见故障背后是两层独立缺陷，
   修完第一层还是空，很容易误判成「修错了」。
3. **它决定了增量能看到什么**。`CURD`/`DBLS` 这类列表属性描述的是库与库的关系，
   解析不出来则整条 owner / 引用链断裂，后续所有生成根归一、反向级联都无从谈起。

## 规律

**解析器遇到看不懂的字节时，「停下」和「跳过继续找」是两个截然不同的策略，选错的代价不对称。**
停下会静默丢数据（且丢的往往正是最长、最重要的那条属性）；跳过继续找最坏是多读一些垃圾，
再由后续校验挡掉。带上限的 resync（这里是 `MAX_RESYNC = 64`）是标准做法。

**上游解析的缺陷会伪装成下游业务缺陷。** 用户报的是「模型树为空」，看起来像模型生成的问题；
真实位置在字节级解析。凡是「整类东西都没有」的故障，先往上游查一层。

**跨仓 fork 里已修的东西要主动回流。** 这个缺陷在 fork 里早就修了，本仓因为 vendor 停在旧实现
又踩了一遍。ADR-005 / ADR-006 采用「最小忠实移植」范式正是为此。

## 关联

- [`ADR-006-explicit-cross-block-collect-fix`](../../docs/adr/ADR-006-explicit-cross-block-collect-fix.md)
- [`ADR-005 refno 索引解析对齐 fork`](../../docs/adr/ADR-005-refno-index-parse-from-fork.md) ·
  [`ADR-007`](../../docs/adr/ADR-007-sys-meta-parse-not-gated-by-included-files.md)（第二层根因）
- [`CONTEXT.md`](../../CONTEXT.md)：会话索引 / 索引优先建表 / 部分解析
