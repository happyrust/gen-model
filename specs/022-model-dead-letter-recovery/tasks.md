# 022 模型死信恢复任务

- [x] T01 保存现状哈希与代码基线到 `docs/evidence/2026-08-21-zero-ncyl-dead-letter-recovery/baseline/`。
- [x] T02 [P] 在 `src/fast_model/prim_model.rs` 补充圆柱类定向基本体失败尺寸与回归测试。
- [x] T03 [P] 在 `src/data_interface/model_update_pending.rs` 实现单查询状态快照与 action/阶段边界测试。
- [x] T04 在 `src/data_interface/batch_worker.rs` 实现公告状态机及 Model→Room 顺序回归。
- [x] T05 [P] 在 `src/web_service/handlers.rs` 增加 health 字段、降级判定与预算测试。
- [x] T06 [P] 在 `scripts/e3d/` 增加只删除精确零尺寸 NCYL 的守卫宏。
- [x] T07 在 `scripts/` 增加 dry-run 修复编排和回滚脚本。
- [x] T08 [P] 更新 `docs/specs/web-service-api.md`、`changelog.md` 与 live 台账。
- [x] T09 运行 Rust 格式化、相关 lib/http_api 测试和 `cargo check`。
- [x] T10 运行 `sigmap verify-plan`、`sigmap verify-ai-output`、`sigmap review-pr`（`review-pr` 的全工作树扫描受既有 495 文件脏状态影响，结果留证）。
- [x] T10a 将基本体模型数据错误持久写入 `geom_error`，补 kind+目标销账、动态 health 汇总和 Surreal 内存回归。
- [ ] T11 建立并验证 E3D/Surreal 成对备份，执行源修复与 7997 重建。
- [ ] T12 单次显式 retry，等待模型与房间工作收敛，完成 10 分钟观察并写证据。

`[P]` 仅表示文件所有权互不重叠时可并行；本次实现仍按现场依赖顺序执行。
