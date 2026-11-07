# ADR-063：`ref_rev` 从「正确性依赖」降为「UI 便利」；反向引用查询的权威源是文件里的引用表

状态：**已接受**（2026-09-07 用户拍板「接着做 ADR-063（ref_rev 降级，级联计划 P5）」；方案
`docs/plans/2026-09-06-cascade-query-via-dab-ref-tables-plan.md` §2 原则 2 与 P5）；**部分实施**
（D1 / D2 / D3 已落地，D4 列的两处老规划器读点等 P4 的替换源接上再动，见「未做」一节）。
日期：2026-09-07
关联：
- **修订** ADR-003「B1-emit：维护 `ref_rev` 反向引用索引」——索引照旧可以存在，但它不再是任何选根
  判断的依据；`increment_pipeline` 那段「缺一条边就是某个设计实例静默不重生成」的注释同日改写。
- **承接** ADR-058 D2（CATA 触发源换成文件差分）与 09-05 kv-mem 计划 G5（读透跳过摄入链）——正是 G5
  抽掉了 `ref_rev` 的维护者，逼出本决策。
- **不含** `cata_ref_rev`：那是目录**行**的补偿队列、与 `ref_rev` 反向引用**边**不是一个东西，
  2026-09-07 已随 ADR-064 Q2 退役（查证：它从来只落 `pe` / noun / UDA / `pe_owner`，一条边都没写过）。

## 背景

`ref_rev` 是 SurrealDB 里的一张反向引用边表，回答「谁引用了元素 X」——改一个元素时靠它找出哪些
模型根要重算。维护点在增量窗口：`apply_one` 每过一个元素就 `DELETE <el>->ref_rev;` + `INSERT
RELATION`，失败进 `enqueue_ref_rev` 补偿队列、再由 `repair_reverse_index_for` 修。

三件事同时成立，使它站不住了：

1. **默认路径已经不读它。** direct + DESI（2026-09-04 起的默认）走
   `window_root_plan::build_model_update_plan_from_window` → e3d-model `plan_update`：owner 链闭包 +
   变换子树 + CTRAY 整条，**一次反向引用查询都没有**。读 `ref_rev` 的是老规划器（非 direct 或非 DESI
   窗口）与 CATA 反向级联执行器。
2. **它正在失去维护者。** `ref_rev` 只在摄入链上维护，而读透（默认 `data_face`）的 G5 定了跳过摄入链。
   一张没人维护、却仍被当作正确性依赖的索引，比没有索引更危险——空集与「真的没有引用者」长得一样。
3. **core 里根本没有这个问题。** E3D 自己的引用表由 dabacon 在 `db_put_reference` **写时同步维护**
   （删「旧目标, 自己」、插「新目标, 自己」，表不在就当场建），表根随会话页版本化；读侧
   `DB_MDB::findElements` 逐库游标点查，**零维护**。「联动查询的索引怎么更新」这件事被文件格式吞掉了。

覆盖面已经验过（计划 P0 差表，783 库实测）：文件里有 **75 张**反向引用表（DESI 侧 40 / CATA 侧 19），
e3d-model 几何真读的 15 个引用属性里 12 个有表，三个没有的都有理由（`VXREF` 在单元内、`HSRO` / `LSRO`
只是 stub 备选且 E3D 自己也不索引）。**F \ R 在 DESI / CATA 两侧都为空**——文件表盖得住库里那张能回答的。

## 决策

**不维护第二份事实。** 反向查询的权威源是文件里的引用表（e3d-io `ReferenceTable` /
`Engine::referrers_of`，按库点查、跨库并集，与 `DB_MDB::findElements` 同构）。`ref_rev` 降为 UI 便利。

| | 口径 |
|---|---|
| **D1 选根不读** | 任何决定「哪些根要重算」的代码都不许读 `ref_rev`。默认窗口规划器今天已经如此（`window_root_plan.rs` 全文只有一句说明性注释），用源码钉钉住 |
| **D2 维护降级** | `maintain_reverse_index` / `enqueue_ref_rev` / `repair_reverse_index_for` / `rebuild_reverse_index` 归入**摄入形态的 UI 便利**（属性面板那类展示）：摄入形态照旧维护、失败照旧进补偿队列（别让 UI 看到半张表）；**读透形态整个不跑** |
| **D3 不许把「没维护」读成「没有引用者」** | 读透形态下 `fetch_ref_rev_edges` **直接报错**，不返回空集。调用方走既有的退化路：`resolve_unit_rollup` 置 `cascade_deferred` + 告警，`expand_live_reverse_cascade` 让那条工作项失败。宁可看得见地退化，不可静默漏算 |
| **D4 护栏不对称** | 反查失败 / 表不可读 / 变动件数超阈 → 退到「该库全部根过期」并点名 `catalogue_cascade_degraded`。判不了就宁多算不漏算（沿用计划 §2 原则 3） |

`fast_delete` 的清库表单里 `ref_rev` **照旧保留**：它是 `pe` 的边，清库时必须跟着走，与本决策无关。

## 实施（2026-09-07）

- `increment_pipeline.rs`：`maintain_reverse_index` 与它的补偿入队在读透形态下整段跳过；那段
  「`ref_rev` 是关联模型也要更新的权威来源」的注释按新口径改写。
- `manual_update.rs`：`fetch_ref_rev_edges`（生产侧唯一入口）在读透形态下 `ensure!` 报错，
  错误文本点名本 ADR 与替代源。`fetch_ref_rev_edges_on` 保持纯函数形态，测试照旧直接用它。

## 未做（等 P4 把替换源接上）

老规划器与 CATA 级联执行器里的三个读点仍在读 `ref_rev`，本轮**没有**动：

- `resolve_unit_rollup` → `load_ref_reversal_closure`（交付单元上卷，`model_update_plan.rs:929`、
  `manual_update.rs:4761`）
- `direct_root_allowed` 用 `OwnershipSnapshot::ref_reversal`
- `expand_live_reverse_cascade`（`ModelWorkAction::CascadeExpand` 的执行器，`model_update_pending.rs:3768`）

理由：把它们换成文件表并集是计划 P4 那条通用一跳（kind-16 属性、MDB 逐库并集）的活，而那条**还没接**；
先删读点等于把能力从老路上拿掉而新路还没有。它们今天的可达性也已经很窄——direct 下 DESI 走窗口规划器，
CATA 没有数据批次所以 `build_cata_cascade_plan` 根本不会被调，`CascadeExpand` 工作项只可能是旧队列里的
遗留行。D3 那道错误保证了：即便走到，它们在读透形态下也是**看得见地退化**，不是静默返回空集。

**补记（2026-09-07 同日，P4 已接线之后）**：通用一跳落在
`data_interface/reverse_reference.rs::DesignReferenceCascade`（开关 `model_design_reference_cascade`
默认关，计划 §3 P4 落地段）。这三个读点**仍然没动**，理由换了一条、而且更硬：P4 的**行为**只有
`POSREF` 一个属性（P0 差表里设计→设计、e3d-model 几何真读的只有它），而这三个读点走的是 `ref_rev`
那 603 个名字的全量资格。拿一条换六百条是**收窄**，不是等价替换——要拆，得先逐条回答「老路那些边里
今天还有哪些真在影响几何」。那是 P5 收口的活。

`docs/2026-09-04_model-cascade-update-inventory.md` 的 #4 / #5 也留到那时一并改口径。

## 验收

- 已做：`cargo test --lib --features http_api` 全绿；源码钉盯住「默认窗口规划器不读 `ref_rev`」与
  「读透形态不维护、且读它要报错」。
- **未 live**：默认形态（`spawned-mem` + `read-through` + direct）下改一个 SCOM 参数的 CATA 窗口、
  只有引用它的根重算——这条验收属于 P4 把文件表反查接进规划器之后，本 ADR 不认领。
  （P4 同日已接线，但它接的是**设计→设计**那一跳；这条 CATA 窗口的 live 走的是 P3 的
  `cascade_stale_catalogue_windows`，仍未跑。）

## 追记 2026-10-31：`expand_live_reverse_cascade` 的空集必须出声（T-P-5）

> 依据：`docs/specs/incr-gen-audit/spec.md` R-P-5 与 `tasks.md` T-P-5（审计按 2026-10-27 工作树）。本节只追记，D1–D4 与「未做」原文不改。

- **补的是 D3 的一个漏洞，不是「未做」里的源替换。** D3 只挡了读透形态（`fetch_ref_rev_edges` 报错）；摄入形态下 `ref_rev` 读得到、却可能是
  **空表 / 没建索引 / 没解析过的库**，那时 `expand_live_reverse_cascade`（`manual_update.rs:2163-2228`）对一个目录件反查出空集、静默返回空根集——
  与「确实没人引用」长得一样（背景第 2 条说的那种危险）。原有的告警只在「查得到引用者、查不到库号」时响。
- **现行**：`referrers.is_empty()` 时 `log::warn!` + 控制台一句「反向级联：目录件 `<refno>` 在 ref_rev 里没有任何入边，本次不展开任何设计根（若这是空索引 /
  未解析的库，级联会漏算，请核对 ref_rev）」（`manual_update.rs:2215-2227`）；返回值不变（仍是空根集），工作项不失败——因为摄入形态下
  「真的没人引用」是常态，把它当失败会让每个孤立目录件都打断级联。这是「看得见地退化」的最轻一档：只说出来，不改判决。
- **没做的**：三个 `ref_rev` 读点仍在（「未做」一节原样）；没有把空集升成 D4 的「该库全部根过期」——那会让每个真的没人引用的目录件都把整库打过期。
  要分清「空表」与「没人引用」，得有「这库的 `ref_rev` 有没有建过」这个事实，它不在库上；P5 收口时随源替换一并解决。
- 顺带：同一函数里 `for referrer in referrers` 改为借用遍历（`:2184-2194`），纯机械改动。`cata_dependency.rs:1934-1950` 那条测试钉的放宽与本条无关
  （它钉的是 `catalogue_behind_out_of_scope` 插入后的字段顺序）。
