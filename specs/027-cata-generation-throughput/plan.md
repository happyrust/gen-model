# 027 CATA 模型生成提速实施计划

## Constitution Check

- **严格正确性**：缺失与查询错误分流；坏件落 `geom_error`，批量故障整页失败。
- **增量与水位**：只处理 RocksDB 落盘后的模型阶段，不改变 `applied_sesno`。
- **单一消费路径**：沿用现有分页、模型任务和 shape saver，不新增初始化协议或消费者。
- **可观测性**：输出预取、解析、变换、正负体、合并和发送的结构化计时及数量。
- **可恢复性**：额度 1 回到串行；空 RocksDB 基线和两次优化运行均保存输入与输出哈希。
- **运行环境**：nightly、fork SurrealDB 2.1、RocksDB；不执行 `cargo clean`。

未发现需要列入 Complexity Tracking 的宪法例外。

## 阶段

1. 封存当前空 RocksDB 8000 基线，并在独立工作树固定本地 path 依赖。
2. 在 `old-aios-core` 增加五个保持输入身份的批量只读 API。
3. 在 `gen_model` 批量读取 BRAN/HANG 子节点，在 `cata_model` 建立页内快照。
4. 按排序后的唯一 CATA 身份经全局几何闸并发解析，串行稳定合并和发送。
5. 让目录和设计 Manifold 使用同一闸，删除未接入的实验性能入口。
6. 补齐缺失/错误、并发额度、稳定顺序和源码护栏测试。
7. 用两个独立空 RocksDB 目录运行 8000，比较等价性、p50/p95、总耗时和资源。
8. 更新 changelog、live ledger、性能 evidence 并执行 SigMap 审查。

## 决策引用

ADR-001、ADR-011、ADR-017、ADR-021、ADR-025、ADR-041。
