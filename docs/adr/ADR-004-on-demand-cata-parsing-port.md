# ADR-004：从 plant-model-gen 移植「按需解析元件库（CATA）」

状态：已接受；Q5a 已修订（见文末「修订记录」——默认值落地时由 Off 改为 On）
日期：2026-07-23（修订 2026-07-27）
关联：ADR-002（core.dll 权威范围）、ADR-003（反向级联索引）；参考项目 `../../plant-model-gen` 的 spec `002-on-demand-cata-closure`、`src/data_interface/cata_closure.rs`、`src/fast_model/gen_model/resolve.rs`；本仓 `src/fast_model/resolve.rs`、`src/fast_model/cata_model.rs`、`src/versioned_db/pe.rs`、`src/versioned_db/database.rs`、`src/data_interface/increment_pipeline.rs`、`vendor/aios-parse-pdms/src/parse.rs`

## 背景

增量重生成一个生成根时，本仓依赖元件库（CATA：SCOM / 几何集 / 点集）已**整库解析**进 SurrealDB。但一次生成实际只用到被引用的一小撮元件，整库解析 CATA 浪费大量 I/O / 内存 / 时间。

参考项目 plant-model-gen 已实现 **refno 级引用闭包**式「按需解析 CATA」：部分解析原语 + BFS 闭包（跟随全部出向 `RefU64` + owner 链 + 容器子树，`db_type` 收口到 CATA）+ 运行期惰性兜底，与 core.dll 的 `DGOTO` 惰性导航同构（从不整库解析）。

本仓已具备关键地基：解析器 `parse_file_db_basic_data` / `refno_table_map`（refno→文件偏移）/ `parse_db_basic_info`（部分解析可行）；SurrealDB 写原语现成（`INSERT IGNORE INTO pe` + `gen_sur_json` / `gen_sur_json_uda`，见 `versioned_db/database.rs`、`versioned_db/pe.rs`）；`get_named_attmap` 读 `SUL_DB`。缺口：**无 ref0→dbnum 定位器**（无 sqlite `db_index`）、**无 `cata_closure`**、`resolve` cache-miss **无兜底**。

## 决策（grill Q1–Q5）

| # | 决策 | 结论 |
|---|------|------|
| Q1 | 移植范围 | **运行期 / refno 级按需**：给定生成根解析其 CATA 闭包 + resolve 惰性兜底；**不**移植项目级整库 sync 过滤 / manifest 前置 pass |
| Q2 | ref0→dbnum 定位器 | **分阶段权威**：元素尚未解析时使用就地内存 `CataDbLocator` + 轻量 json/bincode 缓存（按 db 目录指纹失效）；元素落库后以 `pe.dbnum` 为权威；不另建持久映射表，不引 `sqlite-index`。同一 Ref0 若映射到不同 dbnum，记录冲突并只阻断该 Ref0，不做后写覆盖或全项目失败 |
| Q3 | 解析结果存储 | **INSERT IGNORE 落 SurrealDB `pe`/`ATT_*`**（复用 `save_pes` + `gen_sur_json*` + `SUL_DB`） |
| Q4 | 触发/接入点 | **主动预解析（生成根闭包）+ 惰性兜底（resolve cache-miss）** 并用 |
| Q5a | 开关/默认 | 默认 **Off**（既有整库行为，零回归），env `AIOS_CATA_CLOSURE_MODE` opt-in ⚠️ **已修订为默认 On，见文末** |
| Q5b | 正确性证据 | **`cache_miss_report` + 单根几何 diff 冒烟**；完整离线校验模式留 Phase 2 |

## 关键取舍（Considered Options）

- **范围**：运行期按需子集（选）而非完整 spec-002（预置 pass + manifest + `load/apply_sync_filter`）——后者改动面大，且项目级整库 sync 裁剪不是增量引擎的核心痛点。
- **定位器**：解析前使用内存 + 轻缓存、解析后读取既有 `pe.dbnum`（选），而非移植 `db_index.sqlite` 或维护第二份持久映射——避免新增 rusqlite / feature、预扫步骤和双写一致性；`ref0` 每库个位数、内存量级小。`CataDbLocator` 只解决“PE 尚不存在时属于哪个库”，不取代落库后的元素归属；构建时收集 Ref0 归属冲突，而不是按文件顺序覆盖或让整个 locator 构建失败。冲突 Ref0 查询 fail closed，其他映射继续可用。
- **存储**：落 SurrealDB `pe`/`ATT_*`（选）而非仅内存缓存——重试 `get_named_attmap` 透明命中、持久、与预解析数据不可区分，且写原语现成。
- **触发**：主动 + 惰性（选）而非单一——主动保效率（每根批量一次），惰性保正确（按名引用非 `RefU64` 边、闭包必漏）。
- **开关**：默认 Off + opt-in（选）——安全灰度、零回归。⚠️ 灰度期已过，默认改为 On，见文末修订记录。

## 后果（Consequences）

- 新增 `src/data_interface/cata_closure.rs`（移植版：`CataDbLocator` trait + 内存实现 + `CataClosureResolver` + `parse_db_refnos` + `ensure_cata_refnos_parsed`）；改 `src/fast_model/resolve.rs`（惰性兜底）+ 生成根重生成入口（主动预解析）。
- 解析器可能需薄封装「按偏移解析单元素」入口（参考用 `parse_ele_data_with_info_sync`，本仓有 `refno_table_map`+`pos` 但无同名函数）。
- 落库用 `INSERT IGNORE` 幂等；并发 miss 全局互斥串行化（对齐参考 `LAZY_CATA_FALLBACK_LOCK`）。
- 合法 Ref0 没有可用库归属时，不猜测、不静默跳过：当前模型单元失败并保留 durable pending，映射恢复后重试；已经成功的数据落库与 `applied_sesno` 不回滚。按需生成把它作为可重试服务错误返回，不映射成元素不存在或生成成功。
- 同一 Ref0 出现在多个 `dbnum` 时，仅该 Ref0 及命中它的引用闭包失败并保留 pending；
  对外返回 `409 ref0_affiliation_conflict`，项目其余生成继续工作，冲突计数进入健康信息。
- **语义红线**：按需解析只影响「数据是否已在库」，**不**改几何复用（`cata_hash`）与生成结果；开 / 关必须逐元素一致（由 Q5b 冒烟证据把关）。
- Off 默认下零行为变化；开启前提是 `get_named_attmap` 读 `SUL_DB`（已确认）。⚠️ 默认已改为 On，「零行为变化」不再是默认路径的描述，见文末修订记录。

## 修订记录

### 2026-07-27 · Q5a 默认值：Off → On

**现状**：权威实现 `src/data_interface/cata_closure.rs::cata_closure_enabled()` 的默认值是 **On**——env `AIOS_CATA_CLOSURE_MODE` 未设置、或取值无法识别时，一律按开启处理；只有显式给 `off` / `false` / `no` / `0` 才关闭。也就是说本 ADR 正文 Q5a 描述的「默认 Off + opt-in」**从未作为落地形态存在过**：按需解析随 `d3caa290` 一起进仓时就已经是默认开启的。

**为什么是 On**：仓库里找不到一个「把默认从 Off 翻成 On」的提交——`cata_closure.rs` 只有 `d3caa290` 一次提交，进仓即默认 On，所以当时没有留下决策记录。以下理由是**事后据实测证据补记**的，不是原始决策原文：按需解析已经不是灰度中的实验特性，而是生成路径的常规形态。结构专业的实测（`docs/2026-07-25_test-structure-gensec-on-demand-report.md`、`teach/cases/case-17-structural-on-demand-trio.md`）显示，GENSEC 的 BOX / BEAM 变体在首次生成时需要自动解析 236 / 233 个 CATA 依赖才能拿到几何；若默认 Off，这类「目录尚未加载」的失败会成为按需生成的常态失败形态。

**显式关值保留做什么**：`AIOS_CATA_CLOSURE_MODE=off` 仍然有用，两个用途——
1. **冒烟对照**：`src/bin/cata_smoke.rs` 需要开 / 关各跑一遍比 `combined_digest`，证明「按需解析 == 整库解析」不漏不改几何；
2. **临时回退**：怀疑按需解析引入偏差时，一个环境变量即可退回整库行为。

**语义红线不变**：默认值只影响「数据是否已在库」，不改几何复用（`cata_hash`）与生成结果。正文「后果」一节的这条红线依然成立，需要把关的证据（Q5b 冒烟）也依然待跑。

**尚未对齐的位置**：本次只对齐了文档。以下 6 处**代码内注释**仍写着「默认 Off」，与同文件的权威实现自相矛盾，留待下次改动这些文件时顺手修正——

| 文件 | 行 | 注释内容 |
|---|---|---|
| `src/data_interface/cata_closure.rs` | 1146 | `ensure_cata_parsed_for_roots`：受 env 开关门控（默认 Off） |
| `src/data_interface/cata_closure.rs` | 1165 | `try_lazy_cata_fallback`：默认 Off 即直接返回 false，零回归 |
| `src/data_interface/cata_closure.rs` | 1348 | `preload_cata_for_roots`：受 env 开关门控（默认 Off） |
| `src/data_interface/model_refresh.rs` | 58 | 主动预取的调用点：默认 Off；开关见 cata_closure_enabled |
| `src/fast_model/resolve.rs` | 15 | 惰性兜底调用点：默认 Off，零回归 |
| `src/fast_model/coverage_audit.rs` | 16 | 借 `AIOS_CATA_CLOSURE_MODE` 类比自己的开关：同款「默认 Off + opt-in」 |

最后一处的性质与前五处不同：`AIOS_GEOM_COVERAGE_AUDIT` 自己**确实**是默认 Off，错的只是拿 `AIOS_CATA_CLOSURE_MODE` 当同款范例这个类比。
