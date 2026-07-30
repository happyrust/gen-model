# Implementation Plan: 增量更新链路的静默失效修复

**Branch**: `001-incr-update-integrity-fixes` | **Date**: 2026-07-31 |
**Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/001-incr-update-integrity-fixes/spec.md`

## Summary

五处静默失效，全部属于同一个族：**某条判定路径上存在一个「什么都不说就跳过」的
分支**。修复方向统一为把判定收敛成单一权威谓词，并用测试把「两条路径必须给同一个
答案」钉住。

技术路线：

1. **共享谓词化**（US1/US2）——把「这是不是候选库文件」「这个异常阻不阻断」
   各收成一个函数，自动与手动路径都只调它；用源码顺序/调用点断言测试守住。
2. **标识真值化**（US3）——反向级联改用真实 `ref0 → dbnum` 反查；反查不可得时
   走保守分支并告警。
3. **复活规则按语义分派**（US4）——把「无条件复活」的判据从 action 类型
   放宽为「本次入队不认领会话号」。
4. **口径归一**（US5）——先做决策，再让代码与文档说同一句话。

每一处修复都先写一条**会红的**回归测试（RED），再改实现（GREEN）。

## Technical Context

**Language/Version**: Rust（edition 2024，见 `Cargo.toml`）

**Primary Dependencies**: `surrealdb`（主库）、`aios_core` / `pdms_io` /
`parse_pdms_db`（vendored 工作区依赖）、`notify`（PollWatcher）、`walkdir`、
`indexmap`、`tokio`、`anyhow`；可选 feature：`sql`（sqlx/MySQL）、`mqtt`、`http_api`

**Storage**: SurrealDB。相关表：`pe` / `pe_owner` / `ref_rev` /
`dbnum_watermark` / `dbnum_info_table` / `model_update_pending` /
`increment_update_attempt` / `incr_side_effect_pending` / `datacenter_version`

**Testing**: `cargo test`。分两层——纯函数单测（默认跑）与
`#[ignore]` 的 live 测试（需本地 SurrealDB + 解析过的 E3D 工程）。
本特性新增的回归测试**全部落在纯函数层**（见 Constitution VI）。

**Target Platform**: Windows 服务进程（开发机 PowerShell）；库文件常驻 SMB 共享盘

**Project Type**: 单 crate 后台服务 + CLI/HTTP 入口

**Performance Goals**: 修复不得增加每轮文件事件的磁盘 IO；白名单过滤应当
**减少**需要读文件头的候选数（SC-005）

**Constraints**: 单 worker 消费模型（ADR-011）不变；水位单调不降（ADR-001）；
所有拼进 SurrealQL 字面量的字符串过 `escape_surql_str`；**不使用 `cargo clean`**

**Scale/Scope**: 8 个 E3D 项目 × 每项目最多约 1000 个库文件；
单库窗口可达 170 个会话 / 4600 次元素操作（amssys 冷启动实测）

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| 原则 | 本特性如何满足 |
|---|---|
| I. 水位是承诺 | 本特性不改动水位推进时机与事务边界。US2 强化的是「该阻断时阻断」，方向与原则一致 |
| II. 一条规则只有一份实现 | 这是本特性的主线：US1/US2 就是把已经分叉的两份实现合回一份 |
| III. 静默失效是最高级别缺陷 | 五个故事全部在消除 `_ =>` / 静默 `continue` / 只有 `println!` 的失败 |
| IV. 队列任务可消费/可收口/可复活 | US4 直接补上「可复活」的缺口 |
| V. 标识只用真值 | US3 直接消除 Ref0 冒充 dbnum；反查不可得时留保守分支而非猜测 |
| VI. 不变量由守护看住 | FR-011 要求每条修复附一条「回退即红」的回归测试 |

**结论：无违规，无需 Complexity Tracking。**

复查点（Phase 1 设计完成后重新过一遍）：

- US3 若最终采用「从 `pe` 读 `dbnum` 字段」而非 `CataDbLocator`，
  需重新确认它是否仍满足原则 V（该字段是否是权威真值）。
- US1 抽共享谓词时若为了复用而放宽了手动路径现有的任一道门，视为违反原则 II，必须回退。

## Project Structure

### Documentation (this feature)

```text
specs/001-incr-update-integrity-fixes/
├── spec.md              # 需求与验收（已完成）
├── research.md          # Phase 0：审核证据与候选修法（已完成）
├── plan.md              # 本文件
└── tasks.md             # Phase 2 输出（/speckit-tasks）
```

本特性不需要 `data-model.md`（不新增实体）与 `contracts/`（不改对外接口）。
`quickstart.md` 由 tasks 的验证章节替代。

### Source Code (repository root)

```text
src/data_interface/
├── increment_manager.rs      # US1 主战场：三处遍历的文件门控；US2：scan_and_check_file 裁决；D6：file_stem 处理
├── dbnum_state.rs            # US2 的判定权威（FileAnomaly / blocks / check_file_against_state / record_scan）
├── manual_update.rs          # US1 的参照实现（scan_project_candidates）；US3：expand_live_reverse_cascade
├── model_update_pending.rs   # US4：render_upsert 的复活判据；derived_regen_item
├── model_update_plan.rs      # US5：build_cata_cascade_plan 的启用状态标注
├── update_scope.rs           # US5：admits 的 CATA 口径
└── cata_closure.rs           # US3 的 ref0 → dbnum 反查来源（CataDbLocator）

docs/
├── adr/                      # 若 US5 决策为「纳入 CATA」，需新增或修订 ADR
└── evidence/                 # live 验证留痕（如果本轮跑了 live 测试）
```

**Structure Decision**：沿用现有单 crate 布局，测试与被测代码同文件
（`#[cfg(test)] mod tests`），与仓库既有做法一致。不新建模块目录——
本特性只收敛判定、不引入新概念。

## Implementation Phases

### Phase A：共享谓词（US1 + US2 + D6）

三处改动都在 `increment_manager.rs`，彼此耦合，一起做。

1. 新增 `AiosDBManager::is_candidate_db_file(&self, path) -> bool`
   （或等价的自由函数），内部 = `!should_exclude_file(path) && is_pdms_db_file_name(stem_or_name)`。
   注意**统一取名口径**：手动侧用 `file_name()`，自动侧用 `file_stem()`——
   谓词内部固定一种，避免第三种口径。
2. `sweep_watch_dirs` / `async_watch` / `duplicate_dbnums_across_watch_dirs`
   全部改调这个谓词。
3. `sweep_watch_dirs` 的文件名解析从 `?` 改为「跳过 + 告警」，与 `async_watch` 对齐；
   并把它移到 `is_dir()` 与门控之后（少做无用功）。
4. `scan_and_check_file` 的裁决改为 `anomaly.as_ref().is_some_and(FileAnomaly::blocks)`；
   阻断类异常不写入会污染判据的字段。为每种异常保留一条点名日志
   （不能出现「阻断了但没说是哪一类」）。

**风险**：`should_exclude_file` 的黑名单里有 `com` 扩展名，而 `amscom` 是合法库
（无扩展名，不受影响）；但需确认没有项目使用带点的库名。
→ 由 T003 的表驱动测试覆盖真实样本。

### Phase B：反向级联的标识真值（US3）

1. 先做一次 spike：确认 `expand_live_reverse_cascade` 调用点能否拿到
   `CataDbLocator`（它当前是 `pub(crate) async fn`，被
   `model_update_pending::execute_item` 的 `CascadeExpand` 分支调用）。
2. 优选方案：`load_base_graph` 已经为全部引用者加载了节点，扩展它返回 `dbnum`，
   用 `pe.dbnum` 与 `non_design_dbnums` 比较——一次查询解决，无需引入 locator。
3. 反查缺失（节点没有 `dbnum` 字段）时保留该引用者并累计一条告警，
   与 `load_non_design_dbnums` 现有的「排除集合」保守取向一致。

### Phase C：复活规则（US4）

`model_update_pending::render_upsert` 的分派条件从
`item.action.is_room_recalc()` 改为「不认领会话号」。房间任务的
`dbnum = math::max(...)` 那一支要保留（它与复活无关，是 dbnum 字段的合并策略）——
拆成两个独立判断，别把两件事绑在一个 if 上。

### Phase D：CATA 口径（US5）

**这一阶段以决策开始，不以编码开始。** 两个出口：

- **决策 A（暂不启用）**：在 `build_cata_cascade_plan` 与
  `IncrementPipeline` 的 CATA 分支上标注「当前不可达，启用条件 = `UpdateScope::admits`
  放行 CATA」，并在 `update_scope.rs` 的注释里反向指回来。相关测试加
  `#[ignore]` 说明或改名点出「规划器单测，非端到端」。
- **决策 B（纳入范围）**：改 `admits`，补 ADR，并补一条端到端 live 测试
  （CATA 会话 → 入队 → `CascadeExpand` → 设计根重生成）。

### Phase E：收尾

- `cargo check` + `cargo test`（纯函数层）。
- 若跑了 live 测试，在 `docs/evidence/2026-07-31-*.md` 留痕。
- 更新 `CONTEXT.md` / 相关 ADR 的「已知偏差」段落。

## Complexity Tracking

> 无 Constitution 违规，本节留空。
