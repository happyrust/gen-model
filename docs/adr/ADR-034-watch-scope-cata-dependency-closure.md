# ADR-034：监听限定域内以 CATA 引用闭包取代完整 Catalogue 阶段

日期：2026-08-19

关联：ADR-004（按需解析元件库）、ADR-011（统一数据批次队列）、ADR-017（暂存窗口提交）、ADR-025（严格初始化阶段）。

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
