# 028 CATA 常驻解析缓存实施计划

## Constitution Check

- **严格正确性**：authority 与读域显式；查询错误和目录缺陷不折叠。
- **增量与水位**：只在权威提交及水位均成功后发布失效；失败不推进 epoch。
- **单一消费路径**：沿用现有 RocksDB、staging 和模型生成器，不增加 Legacy 消费者。
- **可观测性**：低基数 cache/flight/dependency/eviction/invalidation 指标与最终快照。
- **可恢复性**：容量 0 回到 RocksDB + 页内缓存；数据库格式无需回退。
- **运行环境**：nightly、fork SurrealDB 2.1、本地 path 依赖；禁止 `cargo clean`。

未发现宪法例外。

## 阶段

1. 建立 authority、读域、错误、loaded value 与 invalidation 类型。
2. 实现有界 `Arc<ScomInfo>` single-flight、epoch 围栏、依赖索引和统计。
3. 迁移页内预取与解析消费路径，修复 axis 错误吞没和 context 强制解包。
4. 在 staging commit 成功边界发布 selective/full invalidation，drop/abort 保持旧代。
5. 删除旧 SCOM 全局 map 和 redb 接线，更新配置及漂移护栏。
6. 运行单元/并发、nightly 构建、Python offline、SigMap 和 AMS 8000 对照基准。

## 决策引用

ADR-001、ADR-011、ADR-017、ADR-021、ADR-041、ADR-045。
