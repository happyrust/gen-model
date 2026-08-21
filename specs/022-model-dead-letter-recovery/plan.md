# 022 模型死信恢复实施计划

## Constitution Check

- **严格正确性**：无效负体继续硬失败，删除源数据中的空元素，不构造近似几何。
- **增量与水位**：不手改 `applied_sesno`；7997 会话回退走 ADR-021 的清库重建。
- **单一消费路径**：不新增批次消费者或绕过模型门的房间执行路径。
- **可观测性**：死信状态由一次查询形成快照，health 与日志共享同一语义边界。
- **可恢复性**：源文件与已验证 Surreal 导出共同构成基线，修改与回滚均保留哈希和命令记录。
- **运行环境**：使用仓库锁定的 nightly 和 SurrealDB 2.1；不执行 `cargo clean`。

未发现需要列入 Complexity Tracking 的宪法例外。

## 阶段

1. 在 `prim_model` 保留失败并补齐定向圆柱类基本体尺寸诊断；将缺失/非法 BREP 和 NaN 变换严格写入 `geom_error`，成功后按 kind+参考号销账。
2. 在 `model_update_pending` 增加单次查询状态快照及纯函数聚合测试。
3. 在 `batch_worker` 增加 300 秒去重公告状态机，并钉住 Model→Room 顺序。
4. 在 health 中新增兼容字段、阻断条件与 degraded 判定，沿用 2 秒预算。
5. 增加 dry-run 优先的 PowerShell 编排、守卫式 E3D 宏和独立回滚入口。
6. 更新 Web API 规格、changelog、任务表和 live 台账。
7. 运行格式化、相关单测、`cargo check` 与 SigMap 创建验证。
8. 建立成对现场基线，删除空 NCYL，重建 7997，单次复活模型工作，等待房间积压清零并留存证据。

## 验证与回滚

- 基线和修改后命令、输入、文字输出、退出状态写入 `docs/evidence/2026-08-21-zero-ncyl-dead-letter-recovery/`。
- 回滚先停止所有消费者，再恢复源文件并从已验证导出恢复数据库，复核会话、水位、死信与 health 后再启动。

## 决策引用

ADR-011、ADR-018、ADR-021、ADR-025。本计划不新增 ADR。
