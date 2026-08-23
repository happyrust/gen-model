# Plan 016：BRAN 直管关系权威替换

**Date**: 2026-08-20  
**Spec**: `specs/016-authoritative-bran-tubing/spec.md`

## Summary

把 `gen_cata_geos` 产生的直管关系从“全局追加 INSERT”改成“每个成功 BRAN 一份完整替换脚本”：同一事务删除该 BRAN 的旧 `tubi_relate` 出边，补齐内容寻址的 `trans` / `aabb` 记录，再写入本轮完整关系集合；空集合也提交删除。删除子树入口同步清除 BRAN 的直管出边。

## Technical Context

- **Language/Version**: Rust edition 2024，nightly-2026-08-02
- **Primary Dependencies**: fork SurrealDB 2.1.4、aios-core、glam、parry3d
- **Storage**: SurrealDB；稳态增量可路由到 ADR-017 暂存窗口
- **Testing**: Rust 纯函数测试 + `mem://` 数据库测试 + 指定 AMS live 复测
- **Target Platform**: Windows / PowerShell，生产兼容 Linux release
- **Constraints**: ReplaySafe；不得 `cargo clean`；不得覆盖当前工作树内既有改动

## Constitution Check

- **I 水位承诺**：本改动不推进水位；模型写失败继续上抛，由现有窗口/待重试语义阻断收口。
- **II 单一规则**：直接路径与暂存路径继续共用 `execute_model_write`，不增加第二写路径。
- **III 静默失效**：成功替换以完整产物为准；渲染或写入错误显式返回。方向不合法的业务跳过由后续连接不变量单独可观察化。
- **IV 队列收口**：不新增队列 action；沿用既有生成根与 DeleteCleanup 消费路径。
- **V 标识真值**：BRAN 身份使用已解析 refno，dbnum/anc 继续使用 `resolve_inst_meta` 真值。
- **VI 可执行守护**：纯渲染测试钉住事务形状，`mem://` 回归钉住“多变少、内容可解、幂等重放”，live 测试与 evidence 钉现场链路。

结论：通过，无需 Complexity Tracking 例外。

## Referenced Decisions

- `docs/adr/ADR-010-room-membership-incremental-update.md`：隐含直管段世界变换与包围盒。
- `docs/adr/ADR-014-branch-atomic-model-replacement.md`：分支原子替换及空产物语义。
- `docs/adr/ADR-017-staged-increment-window-commit.md`：暂存路由与 ReplaySafe journal。
- `docs/adr/ADR-024-shape-save-coalescing.md`：直管关系不得借 `ShapeInstancesData::inst_tubi_map` 覆盖管件实例。

## Project Structure

```text
specs/016-authoritative-bran-tubing/
├── specs/016-authoritative-bran-tubing/spec.md
├── specs/016-authoritative-bran-tubing/plan.md
└── specs/016-authoritative-bran-tubing/tasks.md

src/fast_model/cata_model.rs
src/data_interface/helper.rs
docs/evidence/
docs/2026-08-12_live-test-ledger.md
changelog.md
```

## Implementation

1. 提取可测试的“单 BRAN 直管替换”渲染器，确定性排序和去重共享内容记录。
2. 三个直管产生点只向当前 BRAN 产物集追加；完成该 BRAN 后立即经 `execute_model_write` 提交一份完整替换，空集合照常提交。
3. 删除清理事务增加当前 refno 的 `tubi_relate` 出边删除，保证 BRAN 删除无幽灵直管。
4. 先落失败回归，再实现；执行格式化、定向单测、`cargo check`、live 复测和 SigMap 三门。
