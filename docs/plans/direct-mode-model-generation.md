# direct 模式实施计划：生成期数据读取直走 pdms-io 解析

> 本计划由 grill-with-docs 会话产出。决策见 `docs/adr/ADR-053-direct-mode-generation-reads.md`（**已接受**，2026-08-29 全采推荐项）；术语见 `CONTEXT.md`。本文件只写「做什么、按什么顺序、怎么验收」，不含逐行实现。

依据：

- 本仓 `docs/adr/ADR-053`（本次 grill 决策）
- core.dll RE 实证：`teach/learning-records/0002`（模型更新逻辑）、`0004`（attlib 字典格式）、`0009`（批量生成管线）；`.ida_scratch/`（core31-retrace / core3d-retrace 反编译工作区）
- 解析栈：`../vendor/old-pdms-io`（`io.rs`：B-tree 定位 / 单点解析 / 会话页缓存）、`../vendor/old-parse-pdms-db`（`refno_index.rs` / `dict.rs` / `paged.rs`）、`../vendor/old-aios-core`（`rs_surreal/query.rs` 查询收口 + `staging` 读上下文先例）
- 本仓 `src/data_interface/cata_closure.rs`（BFS 闭包引擎）、`src/fast_model/{resolve,cata_model,prim_model,loop_model}.rs`（查询调用面）

## 决策速查表（2026-08-29 grill 已确认）

| # | 决策点 | 结论 |
|---|---|---|
| Q1 | 范围 | 一期只改「生成期读数据」；产物写 RocksDB、房间/增量摄入管线、SurrealDB 权威地位不动 |
| Q2 | 接入 | aios_core 新增 direct 读上下文（仿 `active_staging_reads()` task-local 路由） |
| Q3 | 时点 | 按 dbnum pin `applied_sesno`（B-tree 查询带 sesno 参数） |
| Q4 | 转换 | EleData→NamedAttrMap 与写库侧同源（抽共享映射函数） |
| Q5 | 证据 | attmap 逐字段探针 + 同根集合 db/direct 双跑 inst/geo hash 逐元素一致 |
| Q6 | RE 策略 | 按需定向 ida-bridge（存量 `.ida_scratch` 优先），不做前置全面 RE |

## 实施原则

- **零回归**：`model_gen_mode` 默认 `db`；不进 direct 上下文时所有查询路径逐字节不变。
- **fail loud**：direct 上下文内未覆盖的查询显式报错，不静默回落 DB（回落会让对拍假绿）。
- **同源优先**：凡 DB 模式已有的语义（名字映射、qualifier、UDA、children 序），direct 复用同一份代码取数，不做第二实现。
- **语义红线**：direct 只改「数据从哪读」，不改生成算法、`cata_hash` 复用、产物写入与房间管线。
- 改动 Rust 文件跑 `cargo fmt` + `cargo check`；aios_core 改动走 `scripts/Toggle-LocalDeps.ps1 -On` 本地 patch 开发 → 上游提交 → 升 rev（不得带 patch 推 main）。

## 阶段

### P0：可行性探针（无需改 aios_core，1–2 天）
状态：**已完成（2026-08-29，验收通过）**
- 已新增 `src/bin/direct_attmap_probe.rs`：按 dbnum 从 `dbnum_watermark` 取文件路径 + `applied_sesno`（pin 时点），`PdmsIO::search_latest_refno + parse_element` 取 EleData，`whole_attmap.merge()` 出 NamedAttrMap，与 `aios_core::get_named_attmap` 逐字段 diff。`--dbnum 0` 可列出全部水位行。
- **验收结果**：dbnum 8000（DESI，120 样本）与 7333（DESI，80 样本）均 **0 真值冲突、0 单侧缺元素**；direct 读出的属性集是 DB 读出的**严格超集**。全部残差分类如下，均为 DB 侧收窄、非 direct 侧错误：
  1. **词哈希归一**（CTYE/FLOW/GTYP 等）：direct 存词哈希整数，DB 读侧按 schema 反哈希成 `WordType` 字符串（0→空串）。→ P1 转换器必须内置同款反哈希（用 attlib dict），让消费方看到与 DB 相同的 `WordType`。
  2. **DB 读损耗键**（TYPEX≈90%、UNIPAR≈20% 元素）：direct 有 `IntArrayType`，DB 读侧 schema 认作字符串、转换失败落空串。生成代码不消费这些键的值（TYPEX 无人读；UNIPAR/BULG 仅作为 `model_impact.rs`/`dchc_change_classes.json` 里的**属性名**参与变更分类），无行为差异。
  3. **DB 行历史缺键**（SPAMAP≈80%、BULG、UNUSED、LJCB）：写库历史路径未落这些键，DB 读侧得 None；同样无人消费值。
  4. **SESNO**：写库簿记字段（行最后写入时的会话号）vs 文件内元素会话号，语义不同，按元数据跳过（同 REFNO/TYPE）。
- **性能首个数字**（debug 构建、含进程内首连开销）：direct ≈5.0–11.0 ms/元素，DB ≈13.0–15.9 ms/元素，direct 快约 1.4–2.6×。
- **开放项（转入 P1）**：本库无已摄入的 CATA 水位行（CATA 走 `cata_closure` 按需解析、不入 `dbnum_watermark`），CATA 侧对拍需在 P1 结合闭包解析路径补做。

### P1：DirectStore + 读上下文（核心地基）
状态：待办
- **转换器（Q4）**：从写库侧（`pdms_io` 的 surql 生成 / `versioned_db` 的 `gen_sur_json*`）抽出「EleData→命名属性集」共享映射，补 `EleData → NamedAttrMap` 直接转换；P0 探针转正为其回归测试。**P0 已定语义规格**：词属性按 attlib dict 反哈希成 `WordType`（对齐 DB 读侧视图）；TYPEX/UNIPAR/SPAMAP 等生成不消费的键保留 direct 原值即可（超集无害）；SESNO 按元数据处理。CATA 侧对拍随本阶段结合 `cata_closure` 补做。
- **DirectStore**（gen-model 新增 `src/data_interface/direct_store.rs`）：
  - dbnum→`PdmsIO` 会话池（`DashMap<dbnum, Mutex<PdmsIO>>`，pin 各库 `applied_sesno`）；
  - ref0→dbnum 定位复用 `cata_closure::CataDbLocator`；
  - attlib 字典单例（`dict::NounClassifier`）；
  - 缓存：refno→NamedAttrMap 进程缓存（对齐 DB 模式 `#[cached]` 语义）。
- **读上下文（Q2）**：aios_core 仿 staging 先例加 `active_direct_reads()`；收口函数（`get_named_attmap` / `get_cat_refno` / `get_children_pes` / `get_type_name` / `get_world_transform` / `query_*` 深层 children 族 / `get_children_named_attmaps` / `get_or_create_cata_context`）入口加路由，direct 分支由 gen-model 注入的 provider 回调实现（避免 aios_core 反向依赖 pdms_io——provider trait 定义在 aios_core，实现在 gen-model）。
- 与 staging 上下文互斥断言（R6）。

### P2：生成链接入 + 单根冒烟
状态：待办
- 按需生成入口（单根 API / `gen_targeted_geos_data_with_policy`）加 direct 上下文包裹（配置 `model_gen_mode = "direct"` 或请求级参数）。
- 新增 `src/bin/direct_gen_smoke.rs`（`cata_smoke` 同款）：同一批生成根 db/direct 各跑一遍，逐元素比 inst/geo hash；`per_refno` 定位发散元素。
- 覆盖用例：BRAN（管件+TUBI）、EQUI（图元）、GENSEC（扫掠+目录闭包）、含 UDA/表达式属性的根、跨库引用根。
- 长尾查询面在此阶段现形（fail loud），逐个补 provider 实现并回填 ADR-053 清单。

### P3：批量与性能基准
状态：待办
- dbnum 级批量：durable pending 消费路径可选 direct；并发争用量化（R4），必要时会话池分片/只读句柄。
- 基准矩阵：单根 / 百根 / 整 dbnum，db vs direct，冷/热两态；记录 `get_named_attmap` 等分项耗时（cata_model 已有分项计时器可直接复用）。
- 目标（验收线）：direct 批量生成端到端不慢于 DB 模式；单点冷读显著快于 DB 冷查询。

### P4：增量协同 + 收尾
状态：待办
- 增量批次收口后（数据+模型已入 RocksDB、水位推进——与既有不变量一致）direct 模式下的 pending 消费全链验证；房间阶段确认仍走 Surreal 不受影响（Q1 范围外）。
- `DbOption.toml` 文档化 `model_gen_mode`；`/health` 暴露当前模式与 direct 覆盖率（provider 命中/fallback 计数）。
- ADR-053 定稿（提议→已接受，回填实测数据）。

### RE 轨（并行，按需触发，Q6）
- 触发条件：P0/P2 对拍出现「无法用现有 RE 结论解释」的语义分歧。
- 手段：优先查 `.ida_scratch` 存量（`_allroutines.json` / `analysis/*.c`）；不够则开 ida-bridge 会话定向反编译（候选缺口：`ELMODL` 子树遍历细节、`GMDRAW` 5 个跳过 noun 码、`MODCMP` 建模方式选择——见 teach/0009 §六）。
- 产出一律回写 `teach/learning-records/`（新增编号），与本计划互链。

## 文件清单

- 新增：`src/bin/direct_attmap_probe.rs`（P0）、`src/data_interface/direct_store.rs`（P1）、`src/bin/direct_gen_smoke.rs`（P2）。
- 改：`../vendor/old-aios-core/src/rs_surreal/query.rs` 等收口函数 + 新增 direct 读上下文模块（P1，随后升 rev）。
- 改：`../vendor/old-pdms-io` / `old-parse-pdms-db`（仅当 P0 暴露解析缺口）。
- 改：`src/data_interface/cata_closure.rs`（闭包引擎输出内存 store 的可选出口）、生成入口（P2）、`DbOption.toml` + 配置结构（P2/P4）。
- 复用不改：`refno_index.rs`、`dict.rs`、`paged.rs`、RocksDB 产物写入、房间/空间管线。

## 验证

- P0 探针：抽样 attmap 逐字段一致（或差异全部可解释并修复）。
- P2 冒烟：同根集合 db/direct inst/geo hash 逐元素一致；fallback 计数=0。
- P3 基准：性能矩阵入 `docs/`；不达验收线要有归因。
- 全程：`cargo check` EXIT=0 + rustfmt；aios_core 改动在 patch-on 与 patch-off（升 rev 后）两态都编译。

## 风险（详见 ADR-053）

R1 转换语义漂移（同源+探针闭环）；R2 跨库 owner 链（定位器兜底）；R3 查询面漏点（fail loud+覆盖率计数）；R4 PdmsIO 并发（会话池，量化后再优化）；R5 文件竞态（sesno pin+文件身份守卫）；R6 staging 互斥（入口断言）。

## Non-Goals（本轮不做）

- SurrealDB 退役 / 数据摄入管线 direct 化（Q1 范围 B，远期独立 ADR）。
- 房间/空间后处理 direct 化。
- 读文件最新会话的「免摄入直生」形态（Q3 选项 B，direct 稳定后再议）。
- fork 世界（rs-core v0.3.2）整库替换（`scope-a-full-crate-swap-feasibility.md` 维持 A0 结论）。
- back-ref 反向索引直读 E3D（ADR-002/003 既有轨道，不并入本计划）。
