# ADR-053：direct 模式——生成期数据读取直走 pdms-io 解析（不查数据库）

状态：**已接受**（2026-08-29 grill 确认：Q1–Q6 全采推荐项）；**Q3 已被 ADR-054 取代**（2026-09-02：未指定时点一律取文件最新会话，`applied_sesno` 不再是生成时点的默认来源）
日期：2026-08-29
关联：ADR-002（core.dll 权威范围）、ADR-004（按需解析 CATA）、ADR-005（refno 索引定位）、`docs/plans/scope-a-full-crate-swap-feasibility.md`（fork 整库替换评估，未采）、`teach/learning-records/0002/0004/0009`（core.dll RE 实证）；产出计划 `docs/plans/direct-mode-model-generation.md`。

## 背景

当前模型生成是「两段式」：pdms_io 解析 db 文件 → 写 SurrealDB（`pe` / `ATT_{noun}` / `pe_owner` 图）→ 生成期经 `aios_core` 查询取数（`get_named_attmap` 等，进程内 cached）。生成一个 refno 前，其依赖数据必须先整体落库；查询冷路径有 WS 往返，SurrealDB 是生成链的容量与稳定性瓶颈之一（历史上多次因 Surreal 容量/连接问题阻塞生成）。

core.dll 的做法（已 RE 实证）与此不同：**打开 db 文件，靠会话 B-tree 索引 O(log n) 单点定位记录，2KB 分页 + LRU 页缓存兜底 I/O，记录内 owner/children/引用属性惰性跳转（DGOTO 同构），attlib.dat 字典给 schema 与分类——从不「先建全库再用」**。

pdms-io 解析栈已把这套机制全部复刻：

| core.dll 机制 | pdms-io 对应物 | 状态 |
|---|---|---|
| 会话 B-tree 索引单点定位 | `PdmsIO::search_latest_refno(refno, Option<sesno>)` / `parse_pdms_db::find_refno_entry` | ✅（ADR-005 落地，支持 sesno pin） |
| 2KB 分页 + 页缓存 | `paged.rs` 分页读 + `PdmsIO` 会话页缓存 | ✅ |
| 记录→元素（含属性、UDA） | `PdmsIO::auto_get_element(refno) -> EleData` | ✅ |
| owner/children/引用边惰性导航 | `EleData.owner/children` + `cata_closure::outbound_refs_of` BFS | ✅ |
| attlib.dat 字典（ATNLOG 两级取值+继承+默认） | `parse_pdms_db::dict::NounClassifier`（1931 noun 交叉验证） | ✅ |
| 反向 back-ref（BREF/SPBREF） | ADR-002/003 B 工作流（Surreal 侧反向索引） | 部分 |

**direct 模式** = 生成期的数据读取绕开 SurrealDB，直接用上表机制从 db 文件现场取数。与现有「数据库模式」并存、可切换、可对拍。

## 生成期查询面（要替换的收口清单，fast_model 实测）

`get_named_attmap`(16 处) / `get_world_transform`(9) / `query_single_by_paths`(5) / `query_multi_deep_versioned_children_filter_inst`(5) / `query_group_by_cata_hash`(4) / `get_cat_refno`(4) / `get_children_named_attmaps`(4) / `get_type_name`(3) / `get_children_pes`(3) / `query_filter_children`(3) / `query_filter_deep_children_atts`(2) / `get_or_create_cata_context`(2) / 表达式求值(3)。

关键语义事实：
- `get_named_attmap` = `select * from pe:X.refno`——ATT 行整行即 NamedAttrMap，**内容全部源自解析期写入**，故文件侧可等价供给；
- `get_cat_refno` = 存量引用 1–3 跳走查（`CATR`/`SPRE`/`PRTREF` 链收口 SCOM/SPRF/SFIT/JOIN），无选型计算，attmap 跳转即可等价；
- 深层 children 查询 = children 树 + noun 过滤，`build_index_map`/children_map 走查等价；
- `get_world_transform` = owner 祖先链 POS/ORI 折叠，attmap 上溯可算。

## 决策（grill Q1–Q6）

| # | 决策点 | 选项 | 结论 |
|---|---|---|---|
| Q1 | 一期范围 | A｜只把「生成期读数据」改 direct，产物仍写 RocksDB（inst/geo），房间/空间后处理与增量摄入管线不动，SurrealDB 仍是数据权威；B｜连数据管线也 direct（Surreal 退役）；C｜只做离线对拍 CLI | **A** |
| Q2 | 接入边界 | A｜仿 `active_staging_reads()` 先例，在 aios_core 加 **direct 读上下文**（task-local read-context，查询函数入口路由）；B｜gen-model 侧包一层 Provider，逐调用点替换 | **A**（改动集中、调用点零散改造归一） |
| Q3 | 读取时点 | A｜按 dbnum pin `applied_sesno`（`search_latest_refno(refno, Some(sesno))`），与 DB 模式同一时点、可对拍；B｜文件最新会话（更新但与 DB 态可能分叉） | **A**（B 留作后续「免摄入直生」形态） |
| Q4 | EleData→NamedAttrMap 转换 | A｜抽出写库侧映射（名字/qualifier/UDA 语义）为共享函数，direct 与写库同源；B｜direct 独立实现 + 对拍兜底 | **A**（语义等价 by construction） |
| Q5 | 正确性证据 | A｜双跑对拍：同批生成根 db/direct 各跑一遍，inst/geo hash 逐元素一致（`cata_smoke` 同款）+ P0 attmap 逐字段 diff 探针；B｜仅性能基准 | **A**（性能基准另做，不作正确性依据） |
| Q6 | ida-bridge 补课策略 | A｜按需定向：先跑对拍，出现语义分歧再开 ida-bridge 会话定向 RE（存量 `.ida_scratch` 优先）；B｜先全面补 RE 再动工 | **A**（解析链 RE 已基本闭环，见上表） |

## 关键取舍（Considered Options）

- **Q1 范围 A vs B**：B（数据管线也 direct、Surreal 退役）动的是整个增量摄入/暂存/房间体系（ADR-017/025/037…），与「生成不查库」的目标不成比例。A 把 direct 收在生成读侧：增量水位、durable pending、房间管线全部不动，风险面最小。Surreal 退役可作远期独立 ADR。
- **Q2 接入 A vs B**：查询调用点分散在 `resolve.rs`/`cata_model.rs`/`prim_model.rs`/`loop_model.rs`/`cal_model` 十几处；逐点换 Provider（B）侵入面大且漏点难查。aios_core 已有 staging 读上下文的成熟先例（task-local + 查询函数入口 if let 路由），direct 作为第二种读上下文语义干净，且天然覆盖全部收口函数。代价是要动 aios_core（本地 vendor patch → 升 rev 流程）。
- **Q3 时点 A vs B**：direct 若读文件最新态，遇到「摄入尚未追平的新会话」会与 DB 模式分叉，对拍失去意义；pin applied_sesno 则两模式读到同一逻辑时点。B-tree 查询本身带 sesno 参数，零额外成本。
- **Q4 转换 A vs B**：NamedAttrMap 的名字映射/qualifier 布局/UDA 合并语义如果 direct 独立实现，将与写库侧永久双维护、漂移即错。同源抽取一次成本，换 by-construction 等价。
- **并发形态**：`PdmsIO` 是 `&mut self`（文件句柄 + 会话页缓存）。direct 上下文按 dbnum 建会话池（`DashMap<dbnum, Mutex<PdmsIO>>` 起步），生成并发高时再评估只读 mmap/分片。这是实现细节不进决策，但风险表挂账。

## 后果（Consequences）

- 新增 direct 读上下文（aios_core）+ `DirectStore`（gen-model：dbnum→PdmsIO 会话池 + ref0→dbnum 定位复用 `CataDbLocator` + attlib 字典单例）。
- 生成入口（按需 API / durable pending 消费）可按配置进 direct 上下文跑生成；DB 模式行为逐字节不变（不进上下文即旧路径）。
- `cata_closure` 的 BFS 闭包引擎复用为 direct 的依赖预热器（可选），落库动作在 direct 下改为填内存 store。
- 新增探针：`direct_attmap_probe`（attmap 逐字段对拍）、`direct_gen_smoke`（单根/批量 inst/geo hash 对拍）。
- 配置：`DbOption.toml` 新增 `model_gen_mode = "db" | "direct"`（默认 `db`，零回归）。
- **语义红线**：direct 只改「数据从哪读」，不改生成算法、`cata_hash` 复用、产物写入与房间管线；同根集合 db/direct 产物必须逐元素一致（Q5 证据把关）。
- aios_core 改动走 `../vendor/old-aios-core` 本地 patch 开发 → 提交上游 → 升 rev；不得带 patch 推 main（pre-push 守卫已有）。

## 风险

- **R1 转换语义漂移**（qualifier/UDA/表达式属性）→ Q4 同源转换 + P0 逐字段探针闭环。
- **R2 世界坐标**需祖先链完整 → owner 链上溯逐级 attmap（浅且带页缓存，成本可控）；跨库 owner（DESI→SITE 库）需 ref0→dbnum 定位器兜底。
- **R3 查询面盘点遗漏**（espec/管件特化查询等长尾）→ P1 以「收口函数清单 + 编译期 deny 直连 SUL_DB」双保险；漏网走 fallback：direct 上下文内未覆盖查询显式报错（fail loud），不静默回落 DB。
- **R4 PdmsIO 并发争用** → 会话池分 dbnum 锁；对拍阶段量化锁等待，超阈值再优化。
- **R5 文件与水位竞态**：direct 读文件时 watcher 可能正写入新会话 → pin sesno 天然免疫「读到未应用会话」；文件替换（reinit）场景沿用现有文件身份守卫。
- **R6 与 staging 读上下文叠加**：direct 与 staging 互斥（生成期不在暂存窗口内），入口断言二者不同时在场。
