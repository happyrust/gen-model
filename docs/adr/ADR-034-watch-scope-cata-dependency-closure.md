# ADR-034：监听限定域内以 CATA 引用闭包取代完整 Catalogue 阶段

日期：2026-08-19

关联：ADR-004（按需解析元件库）、ADR-011（统一数据批次队列）、ADR-017（暂存窗口提交，已被 ADR-056 取代）、ADR-025（严格初始化阶段）。

> **2026-09-03 对照（ADR-056 D8）**：「解析产物与 DESI 数据同属 ADR-017 提交单元」「依赖缺失 / 300 秒无进展 ⇒ 整个窗口
> 失败、DESI 水位不推进」「fail-closed 前置」三处失效。模型面经 `E3dDbResolver` 直接从文件读 CATA，不再以「CATA 行先落库」
> 为前置；`cata_closure` 解析进 SurrealDB 只服务 `ref_rev` 反向索引与 UI 目录属性，改为 `finalize_attempt` 之后的补偿任务
> `SideEffectCompensator::enqueue_cata_ref_rev`（spec 035 T126），`missing > 0` 记 warning，不拦模型、不拦水位；
> `prepare_required_dependencies` 看门狗已删（T121）。监听限定域的**阶段顺序**（`Meta → Design（内含 CATA 引用闭包）→ Model`）、
> 权威依赖清单、部分解析不推进 CATA 水位、缓存新鲜度与 epoch 纪律不变。
>
> **2026-09-07 对照（ADR-064 Q2）**：上面那条补偿任务 `enqueue_cata_ref_rev` 也退役了——CATA 依赖行改由
> `cata_dependency::persist_cata_dependencies` 排在 DESI 块与水位**之前**落（`pe.dependency = true`，ADR-064 D5），
> 水位之后不再有 CATA 落库；`preload_cata_for_roots` / 依赖缓存 `CataDepCache` 从此没有生产调用方，只剩诊断 bin。

## 决策

`watch_dbnums` 为空时继续执行 ADR-025 的 `Meta → Catalogue → Design → Model`。限定域非空时，
`watch_dbnums` 只选择主 DESI 批次；初始化改为 `Meta → Design（内含 CATA Dependency）→ Model`，
不为所有 CATA 建完整数据批次。完整候选扫描仍须先完成跨项目优先级、同号重复和文件身份裁决，
选中的 CATA 形成权威依赖清单，供限定 DESI 的引用闭包定位；被遮蔽候选不得进入清单。

引用闭包只解析生成根实际引用的 CATA 元素。部分解析不建立或推进 CATA 的 `applied_sesno`；
解析产物与 DESI 数据、模型产物及 DESI 水位同属 ADR-017 提交单元。任一依赖缺失、身份歧义、
属性解析失败或连续 300 秒没有实质进展，整个窗口失败并丢弃，DESI 水位不推进。

CATA 文件指纹参与闭包缓存新鲜度。文件变化时用 replacement semantics 刷新被引用元素，
不得以 `INSERT IGNORE` 保留旧属性。缓存是可丢弃优化，只能在提交成功后发布；提交后、缓存发布前
崩溃只导致重算。CATA-only 文件事件不会绕过限定域主动创建 DESI 批次，下一次被限定 DESI
触发时按最新指纹刷新。

周期对账不得用新 epoch 使正在运行的旧 epoch 批次失去阶段归属；新清单延后到活动批次收口后安装。

## 后果

- 限定调试不再为无关 Catalogue 付出完整解析成本，但 8000 的真实几何依赖仍是必需输入。
- 依赖解析由 best-effort 变成暂存 DESI 窗口的 fail-closed 前置；运行期独立按需生成保留惰性兜底。
- 无限定域的生产初始化行为不变。

## 2026-08-19 Oracle 审核修订

依赖缓存上下文必须显式携带源 dbnum 与实际窗口右端；Ref0 多库冲突记录全部候选，仅在窗口实际引用该 Ref0 时阻断。
