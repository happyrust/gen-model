# 按需解析元件库（CATA）移植实施计划

依据：

- 参考项目 `../../plant-model-gen`：`specs/002-on-demand-cata-closure/{spec,plan}.md`、`src/data_interface/cata_closure.rs`、`src/data_interface/db_index.rs`、`src/fast_model/gen_model/resolve.rs`
- 本仓 `docs/adr/ADR-004-on-demand-cata-parsing-port.md`（本次 grill 决策）
- 本仓 `src/fast_model/resolve.rs`、`src/fast_model/cata_model.rs`、`src/versioned_db/pe.rs`、`src/versioned_db/database.rs`、`src/data_interface/increment_pipeline.rs`、`vendor/aios-parse-pdms/src/parse.rs`

## 决策速查表

| # | 决策点 | 结论 |
|---|------|------|
| Q1 | 范围 | 运行期 / refno 级按需（不做项目级整库 sync 过滤 / manifest 前置 pass） |
| Q2 | 定位器 | 内存 `CataDbLocator` + 轻缓存（不引 sqlite） |
| Q3 | 存储 | INSERT IGNORE 落 SurrealDB `pe`/`ATT_*` |
| Q4 | 触发 | 主动预解析（生成根闭包）+ 惰性兜底（resolve cache-miss） |
| Q5a | 开关 | ~~默认 Off + env `AIOS_CATA_CLOSURE_MODE` opt-in~~ → **落地为默认 On**，env 双向覆盖（见「实施状态」） |
| Q5b | 校验 | `cache_miss_report` + 单根几何 diff 冒烟（完整校验模式留 Phase 2） |

## 实施原则

- ~~默认 Off、零回归~~（**已变更**：落地为默认 On，见「实施状态」）；关掉开关时行为与现状逐字节一致。
- `CataDbLocator` trait 抽象定位器，内存实现 + 轻缓存；将来可无痛升级到持久 sqlite（trait 不变）。
- 复用现成 `pe`/`ATT_*` 写原语（`save_pes` + `gen_sur_json` / `gen_sur_json_uda`）与 `SUL_DB`；不引入 rusqlite / 新 feature。
- 落库 `INSERT IGNORE` 幂等；并发 miss 全局互斥串行化。
- 改动 Rust 文件跑 `cargo fmt` + `cargo check`；遵循仓库既有 test 约定。

## 边模型（承自 spec 002，已 IDA/core.dll 交叉验证）

- **横向边**：元素属性里全部 `RefU64Type` / `RefU64Array` 出向引用（不走白名单，自动覆盖 `GMRE/GSTR/NGMR/PTRE` 及 `XGMREF/UDGEOM/TGEOM/PSPREF/GEOM`）。
- **纵向边**：到达节点纳入 owner 祖先链到库根；对容器名词（`GMSE/NGMS/PTSE/PSTR/SPRO/DTSE`；项目级另含 `SELE/SPCO`）展开子树。
- **收口 / 去重 / 终止**：`db_type` 收口到 CATA；`visited: HashSet<RefU64>` 去重防环；frontier 空即止。`cata_hash` 不参与解析去重。
- **精确模式**（`CataClosureConfig::precise`）：容器子树仅展开几何/点集容器，避免经 owner 链到 SPEC/SELE 后子树发散。

## 阶段

### 阶段 0：地基确认
状态：待办
- 确认解析器「按偏移解析单元素」入口：本仓有 `refno_table_map`（refno→`pos`），需确认/薄封装一个等价于参考 `parse_ele_data_with_info_sync(&bytes[pos-4..], &db_info)` 的入口。
- 确认 `get_named_attmap` 读 `SUL_DB`（已确认）。
- 确认 `pe`/`ATT_*` 写原语可复用（已确认：`versioned_db/database.rs` 的 `INSERT IGNORE INTO pe` + `gen_sur_json*`、`versioned_db/pe.rs::save_pes`）。

### 阶段 1：内存定位器（Q2）
状态：待办
- 新增 `CataDbLocator` trait（`dbnum_of_ref0` / `db_type_of` / `file_of`）+ 内存实现：用 `PdmsWatcher` 文件清单 + `parse_db_basic_info`（dbnum/db_type/world）+ 各库 index 的 `ref0` 集构建 `ref0→dbnum`、`dbnum→(type,file)`。
- 轻量 json/bincode 缓存（按 db 目录指纹失效），避免每次进程重扫。

### 阶段 2：部分解析 + 闭包引擎（Q1）
状态：待办
- 新增 `src/data_interface/cata_closure.rs`：`parse_db_refnos`（会话缓存复用页）+ `CataClosureResolver`（BFS，`precise()` 配置）+ `collect_design_subtree_outbound` + `run_cata_closure_pass_for_refnos`。

### 阶段 3：存储 + 惰性兜底（Q3 + Q4 惰性）
状态：待办
- `ensure_cata_refnos_parsed(seeds)`：小闭包（保留属性表）→ `INSERT IGNORE` 落 `pe`/`ATT_{noun}`/`ATT_UDA`（用 `SUL_DB`）；全局互斥锁串行化。
- 接 `src/fast_model/resolve.rs`：`get_or_create_scom_info` 的 cache-miss（`get_named_attmap` 失败）+ `resolve_desi_comp` 的 `get_cat_refno` miss → 兜底后重试一次；miss 记 `cache_miss_report`。

### 阶段 4：主动预解析（Q4 主动）
状态：待办
- 在生成根即将重生成处（增量 `model_refresh` / 生成驱动，`generation_root` 归一之后）调 `run_cata_closure_pass_for_refnos(gen_root)` 批量解析闭包落库。
- 与阶段 3 惰性兜底并存：主动保效率、惰性收漏边。

### 阶段 5：开关 / 观测 / 校验（Q5）
状态：待办
- env `AIOS_CATA_CLOSURE_MODE`（规划时定为默认 Off，**落地为默认 On**）门控主动 + 惰性两条路径。
- `cache_miss_report`（必做）；单根一次性几何 diff 冒烟脚本（开/关按需，逐元素比 inst / geo hash）。

## 文件清单

- 新增：`src/data_interface/cata_closure.rs`（trait + 内存 locator + 闭包引擎 + 部分解析 + 兜底）。
- 改：`src/fast_model/resolve.rs`（cache-miss 惰性兜底 + miss 记录）。
- 改：生成根重生成入口（`src/data_interface/model_refresh.rs` 或生成驱动，精确行阶段 0/4 定位）。
- 可能改：`vendor/aios-parse-pdms/src/parse.rs`（补按偏移单元素解析薄封装）。
- 复用：`src/versioned_db/pe.rs`（`save_pes`）、`src/versioned_db/database.rs`（写原语参考）。
- 缓存落盘：`output/<project>/scene_tree/`（或本仓等价目录）下的 locator 缓存文件。

## 验证

- `cargo check`；改动 Rust 文件 `cargo fmt`。
- 单根冒烟：选一个 BRAN/EQUI，分别用 `AIOS_CATA_CLOSURE_MODE=off`（整库基线）与 `=on`（按需）各跑一遍，几何 hash / inst_relate 逐元素一致；`cache_miss_report` 为空或仅落已知 R2（按名引用）。基线一侧不能靠「不设环境变量」，默认值是 On。
- 相对整库解析：解析 CATA 元素数 `O(引用闭包)` ≪ `O(全部元件)`。

## 风险

- **R1** 解析器单元素入口：本仓有 `refno_table_map`+`pos`，无同名 `parse_ele_data_with_info_sync` → 薄封装（低）。
- **R2** 表达式按名引用（`DTAB`/`CATREF`）非 `RefU64` 边 → 惰性兜底覆盖 + `cache_miss_report` 观测。
- **R3** 内存定位器首次扫描成本（index-only，缓存后一次性；长驻服务无痛，单发 CLI 需缓存）。
- **R4** 主动预解析精确接入行需实际打开 `generation_root.rs` / `model_refresh.rs` 确认（部分文件以非 UTF-8 显示，需按字节读取）。
- **R5** `get_or_create_cata_context` / 版本一致性（`read_at`）在本仓的语义与参考可能不同，接入时核对。

## Non-Goals（本轮不做）

- 项目级整库 sync 部分解析过滤（`load_sync_filter` / `apply_sync_filter`）。
- 独立前置闭包 pass + `cata_closure.json` manifest 作为解析源（`run_cata_closure_pass_from_config`）。
- 完整常驻离线校验模式（`verify_cata_closure`），留 Phase 2。
- 改动几何复用（`cata_hash`）机制。


## 实施状态（2026-07-23 落地）

已实现并 `cargo check --lib` 通过（EXIT=0）+ rustfmt：

- **Phase 0 地基确认**：`parse_ele_data_with_info`（async 单元素解析）、`RefU64::get_0()`、`dbnum_watermark`(dbnum→type/file)、`dbnum_info_table`(ref0→dbnum)、`pe`/`ATT_*` 写原语——全部现成，无需改解析器。
- **Phase 1** `src/data_interface/cata_closure.rs`：`CataDbLocator` trait + `InMemoryCataLocator::build_for_project`（读 `dbnum_watermark` + `parse_file_db_basic_data` 扫 `ref0` + temp 目录 json 指纹缓存）。
- **Phase 2** 同文件：`outbound_refs_of` / `parse_db_refnos` / `CataClosureConfig(::precise)` / `CataClosureManifest` / `CataClosureResolver`(BFS：全出向+owner链+容器子树, db_type 收口 CATA) / `collect_design_subtree_outbound` / `run_cata_closure_pass_for_refnos`。
- **Phase 3** 同文件：`ensure_cata_refnos_parsed`（小闭包→`INSERT IGNORE` `pe`/`ATT_{noun}`/`ATT_UDA` + `INSERT RELATION pe_owner`，全局 `TokioMutex` 串行化）+ `cata_closure_enabled`(env 开关) + `try_lazy_cata_fallback`；接入 `src/fast_model/resolve.rs::get_or_create_scom_info` 的 cache-miss。
- **Phase 4** `ensure_cata_parsed_for_roots` + 接入 `src/data_interface/model_refresh.rs::run_owner_regen`（生成根重生成前主动预取；**默认 On**）。
- **Phase 5** env 开关 `AIOS_CATA_CLOSURE_MODE`(=`manifest`/`on`/`1`) ✅；`missing` 计数入日志。**待活环境**：单根几何 diff 冒烟、更完整 `cache_miss_report`。

单测：locator（ref0→dbnum / type / file / 计数）、`is_valid_ref0`、`precise` 配置，共 5 个（仓库规则不编译 test 目标，单测随源码留存）。

关键适配（与参考项目差异，编译期发现并修正）：`DbBasicData` = `aios_core::db::DbBasicData`；`EleData.children/owner` = `RefU64`；`NamedAttrValue::RefU64Array` 内层是 `Vec<RefnoEnum>`（需 `.refno()`）；写库走 `SUL_DB` 而非 `project_primary_db()`；`pe_owner` 关系格式对齐 `save_pe_relates`。

未接主动预取的 gen 入口（`owner_regen` 回退路径 / `compensate_owners` 侧效补偿）：正确性由 Phase 3 惰性兜底保证；如需其也主动预取，同样调 `ensure_cata_parsed_for_roots` 即可。

启用方式：**不设环境变量即为开启**——`cata_closure_enabled()` 在 `AIOS_CATA_CLOSURE_MODE` 未设置或取值无法识别时都返回 `true`。要退回整库解析行为必须**显式关闭**：`AIOS_CATA_CLOSURE_MODE=off`（或 `false`/`no`/`0`）。

> 规划阶段（本文上半部分 Q5a）定的是「默认 Off + opt-in」，但按需解析随 `d3caa290` 进仓时就已经是默认 On，那个 opt-in 形态从未落地。变更缘由与显式关值的保留用途见
> `docs/adr/ADR-004-on-demand-cata-parsing-port.md` 的「修订记录」。同一份修订记录里还列了 6 处仍写着「默认 Off」的**代码内注释**——它们尚未修正，读代码时以
> `cata_closure.rs::cata_closure_enabled()` 为准。

### 单根几何冒烟入口（已写，待活环境跑）

`src/bin/cata_smoke.rs` + `cata_closure::geo_smoke_digest`：对给定设计参考号逐个 `resolve_desi_comp` 算确定性摘要。跑两遍比对：

```text
# 基线（整库 / CATA 已解析）：默认已是 On，必须显式 off 才真正走整库对照
$env:AIOS_CATA_CLOSURE_MODE = "off"; cargo run --bin cata_smoke -- --refnos 24383_66456,24383_66457
# 按需（命中未解析走惰性兜底补齐）：默认即开，显式 on 只是更醒目
$env:AIOS_CATA_CLOSURE_MODE = "on";  cargo run --bin cata_smoke -- --refnos 24383_66456,24383_66457
```

> 基线那一条**必须显式 `off`**。默认值改成 On 之后，「不设环境变量」跑出来的是按需结果而不是整库结果，两遍比对会拿按需比按需——`combined_digest` 必然一致，冒烟假绿、证明不了任何事。

两次 `combined_digest` 一致即「按需 == 整库」；不一致时 `per_refno` 直接定位发散元件。

### 真库端到端验证（AvevaMarineSample，无需 SurrealDB）

**`cata_parse_probe`（by-refno 部分解析）** 在 `ams000\ams8000_0001`：
- `db_no=8000` / `DESI`；索引 `refno_table_map=30692`、`children_map=11603`、bytes≈12.9MB；`ref0` distinct=**2**（印证每库 ref0 极少）。
- 部分解析采样 **5/5 成功**（JLDATU / POINSP / STRU / CYLI / VERT），`outbound` / `children` 抽取正确。

**`cata_closure_probe`（跨库闭包）** 根 `24384_18447`(STRU)：
- 目录扫描定位器：**301 dbnum / 581 ref0**。
- 闭包：`seeds=12` → `visited=60` / `rounds=5` → 精确锁定 `dbnum=5052(CATA)` 的 **60 个元素**；`missing=1`（非 `RefU64` 边，生成期惰性兜底覆盖）。
- 结论：单设计根仅需 1 个 CATA 库、60 个元件，`O(引用闭包) ≪ O(全部元件)` 实证成立。

验证工具（随源码留存）：`src/bin/cata_parse_probe.rs`、`src/bin/cata_closure_probe.rs`、`src/bin/cata_smoke.rs`（后者走 `resolve_desi_comp` 出几何 diff，需活 SurrealDB）。

### Phase 6：dbnum→CATA 依赖缓存（bincode，生成期提前预加载）

- **缓存结构** `CataDepCache`：`源 dbnum → { source_sesno, cata_refnos:[u64], updated_at }`，bincode 落 `output/<project>/cata_dep_cache.bin`（env `AIOS_CATA_DEP_CACHE_PATH` 可覆盖），原子写(tmp+rename)。
- **失效口径**：源库 `applied_sesno`（来自 `dbnum_watermark`）变 → 该源条目失效重算；CATA 定义变**不**改「依赖哪些 id」(as-written 边)，不触发失效。
- **产出+消费入口** `preload_cata_for_roots(project, roots)`（挂 `model_refresh::run_owner_regen`，取代原 `ensure_cata_parsed_for_roots`）：按源 dbnum 分组 → 缓存命中(sesno 相符)取 ids → 未命中/过期则 `run_cata_closure_pass_for_refnos` 现算 + `put`/`save` → 汇总 ids 批量 `ensure_cata_refnos_parsed` 预加载落库；惰性兜底仍兜 `missing`。
- **收益**：命中缓存时**零闭包计算**、一次批量预取；跨次 / 跨进程复用。单测 `dep_cache_roundtrip_and_sesno_invalidation`（bincode 往返 + sesno 失效）。
- 启用同 `AIOS_CATA_CLOSURE_MODE`（**默认 On**，显式 `off` 才关）。
