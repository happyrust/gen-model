# Plan 017：不可成管直段的诊断线型

**Date**: 2026-08-20  
**Spec**: `specs/017-invalid-tubing-diagnostic-line/spec.md`

## Summary

把 `gen_cata_geos` 里三处「方向判定失败就 `continue`」的分支改成产出带标记的直段：
`TubiRelationSpec` 增加 `invalid` 与 `invalid_reason`，`render_tubi_branch_replace` 把标记写进
关系行；查询层的诊断标记从「端点已删除」扩成「记录自带标记 or 端点已删除」。渲染侧不动。

## Technical Context

- **Language/Version**: Rust edition 2024，nightly-2026-08-02
- **Primary Dependencies**: fork SurrealDB 2.1.4、aios-core、glam、parry3d
- **Storage**: SurrealDB 无模式关系表；稳态增量可路由到 ADR-017 暂存窗口
- **Testing**: Rust 纯函数测试 + `mem://` 数据库测试 + 指定 AMS live 复测
- **Target Platform**: Windows / PowerShell，生产兼容 Linux release
- **Constraints**: ReplaySafe；不得 `cargo clean`；不得覆盖当前工作树内既有改动

## Constitution Check

- **I 水位承诺**：不推进水位；标记只描述本轮渲染事实，写失败继续上抛。
- **II 单一规则**：方向判定仍只有 `PdmsTubing::is_dir_ok` 一处实现，三个产生点共用同一个
  「构造 spec」的辅助函数，不出现第二套判定。
- **III 静默失效**：本特性正是为消灭一处静默跳过而设——判定失败从 `#[cfg(debug_model)] println!`
  升级为落库的显式标记，查看端与后续健康统计都能看见。
- **IV 队列收口**：不新增队列 action。
- **V 标识真值**：标记来自实际几何判定，不猜、不填近似值；口径未知单列一种原因而不是伪装成方向问题。
- **VI 可执行守护**：纯函数测试钉住「方向失败仍产出且带标记」「变换沿实际连线」，
  `mem://` 测试钉住标记随重生成翻转，live 测试钉现场那支 BRAN 的 4+2 分布。

结论：通过，无需 Complexity Tracking 例外。

## Referenced Decisions

- `docs/adr/ADR-010-room-membership-incremental-update.md`：直段世界变换与包围盒的用途，据此排除不可成管段。
- `docs/adr/ADR-014-branch-atomic-model-replacement.md`：分支原子替换及空产物语义。
- `docs/adr/ADR-017-staged-increment-window-commit.md`：暂存路由与 ReplaySafe journal。
- `specs/016-authoritative-bran-tubing/spec.md`：本特性写入的完整集合语义由它保证。

## Project Structure

```text
specs/017-invalid-tubing-diagnostic-line/
├── specs/017-invalid-tubing-diagnostic-line/spec.md
├── specs/017-invalid-tubing-diagnostic-line/plan.md
└── specs/017-invalid-tubing-diagnostic-line/tasks.md

src/fast_model/cata_model.rs
../vendor/old-aios-core/src/rs_surreal/inst.rs
../plant-ui/vendor/rs-core/src/rs_surreal/inst.rs
docs/evidence/
docs/2026-08-12_live-test-ledger.md
changelog.md
```

## Implementation

1. `TubiRelationSpec` 增加 `invalid: Option<TubiInvalidReason>`——用 `Option` 而不是
   `bool + reason` 两个字段，是为了让「标记为真却说不出原因」在类型上不可表达；
   `render_tubi_branch_replace` 无条件写出 `invalid`，仅在为真时写 `invalid_reason`。
2. 抽一个「由 `PdmsTubing` 构造 spec」的辅助函数，三个产生点共用；方向判定失败时
   走同一个构造函数、只是标记不同，变换沿实际连线取向。
3. 查询层诊断标记改成并集，`TubiInstQuery.invalid` 保持 `#[serde(default)]`。
4. 复核空间归属与料表口径是否需要过滤带标记直段。
5. 先落失败回归，再实现；执行格式化、定向单测、`cargo check`、live 复测。
