# 范围 A 可行性评估：gen-model 整库改用 pdms-io-fork 的 parse_pdms_db

日期：2026-07-23
背景：本次已完成范围 B/B1（把 fork 的会话 B-tree 索引定位移植进当前 vendor 库，见 `ADR-005`）。范围 A = 更彻底：gen-model 直接依赖 fork 的 `parse_pdms_db` crate，与 fork 完全共用一套解析源码。

## 结论（TL;DR）

**不建议作为「解析库替换」来做——它实质是一次 `aios_core` 0.2.0 → 0.3.2 的平台级大迁移，波及 gen-model 全仓 + pdms_io，成本高、风险大；而解析的核心收益（索引定位）已由 B1 以零依赖成本拿到。** 若确需，应立项为「gen-model + pdms_io 整体切到 pdms-io-fork 的 rs-core v0.3.2 世界」的平台升级，而非外科式换 crate。

## 关键事实

| 维度 | 现状（gen-model 世界） | fork 世界 | 影响 |
|---|---|---|---|
| aios_core | git rev `5667b70` = **v0.2.0**（gen-model / vendor parse_pdms_db / old/pdms-io **一致** pin） | rs-core 路径 **v0.3.2**（分支 `codex/fix-refno-enum-cata-hash`@d51a5bb8，**不含** 5667b70） | **crux** |
| aios_core 代码差异 | — | — | `git diff --no-index --stat`：**378 文件 / +66293 / −17001**；`lib.rs` 公共面 diff 495 行 |
| nom | 7 | 8 | **非阻塞**：gen-model 未调用 parse_pdms_db 的任何 nom 类型 API（边界不共享 nom 类型），nom7/8 可并存 |
| 其它新依赖 | — | +rkyv0.8 / glam0.30 / tempfile / config0.15 / cached0.56 | 加法式，低风险 |
| API 名称 | — | 同名函数基本齐（`parse_file_db_basic_data` 等） | 名称兼容，但**返回的 aios_core 类型属不同版本**才是问题 |

## 为什么 aios_core 是死结

parse_pdms_db 的公共类型（`RefU64` / `EleData` / `NamedAttrMap` / `DbBasicData` …）全部来自 aios_core，并在 **gen-model ↔ parse_pdms_db ↔ pdms_io 三方边界共享**。fork 的 parse_pdms_db 必须对 rs-core v0.3.2 编译；若 gen-model 仍用 v0.2.0，则两个 aios_core 版本同时进依赖树 → 边界处「v0.3.2 的 `EleData` ≠ v0.2.0 的 `EleData`」类型不兼容、**无法编译**。故范围 A 强制 gen-model 整仓迁到 v0.3.2。

## 迁移波及面

1. **gen-model 自身**：全仓 aios_core API 用点（`RefU64` / `EleData` / `NamedAttrMap` / `SUL_DB` / `db_tool` / …）需按 v0.3.2 适配；0.x 的 0.2→0.3 属破坏性档，378 文件级差异意味着大量改名 / 签名 / 行为变化（如 `RefnoEnum` catalog hashing 已改）。
2. **old/pdms-io**（gen-model 的 `pdms_io` 依赖，同 pin v0.2.0）：必须一并迁到 v0.3.2，或直接改用 **pdms-io-fork 的 pdms-io**。
3. **潜在**：任何共享 aios_core 类型的其它 dep。

## 选项

- **A0（推荐 · 维持现状）**：保留 B1 成果（索引定位已进 vendor 库、零回归、更正确）。范围 A 不做。
- **A1（若确需彻底对齐）**：立平台升级项目——gen-model 与 pdms_io 一并切到 pdms-io-fork 的 rs-core v0.3.2：把 `aios_core` / `pdms_io` / `parse_pdms_db` 三者都指向 fork 世界，再逐个修 v0.2→v0.3 编译错误 + 回归。预估工作量大（aios_core 378 文件级 API 迁移 + 全仓适配 + 全量回归），需独立排期与充分测试环境（SurrealDB + 真库几何回归）。
- **A2（折中，已在 B 覆盖）**：只要「fork 的解析方式」而非「同一套源码」——即 B/B1，已完成。

## 建议

维持 **A0**；若业务上必须与 fork 共库演进，按 **A1** 立项，先做 aios_core 0.2→0.3 的独立迁移 spike（评估真实编译错误规模）再决定。