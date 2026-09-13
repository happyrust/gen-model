# RocksDB 持久模型 + 进程内投影缓存

## 决策

- `external` 与 `spawned-rocksdb` 的 SurrealDB/RocksDB 是模型真相源。
- `ModelMemoryStore` 在持久模式下是 2 GiB、按 root 淘汰的进程内缓存；未取得 durable ACK 的候选根不参与淘汰。
- 默认 `memory-first`；可用 `database-first` 强制每次按 root 刷新。
- 启动只水化控制面，不再按 dbnum 读取完整模型投影。
- cache miss 点查 `gen_root`，再通过 `idx_inst_relate_anc` 与 `idx_tubi_relate_anc` 读取该 root 的结构化投影。
- RocksDB 不可用时，已有缓存仍可作为只读投影；miss 和写入失败，不把空结果当成删除。

## 配置

```toml
model_cache_mode = "memory-first"       # memory-first | database-first
model_cache_max_bytes = 2147483648       # 默认 2 GiB
```

环境变量 `AIOS_MODEL_CACHE_MODE` 和 `AIOS_MODEL_CACHE_MAX_BYTES` 覆盖配置文件。

## 正确性边界

- `RootReceipt.geometry_count` 必须与索引查询返回的结构化记录数相同，否则拒绝缓存。
- 记录必须与 root 的 dbnum、source session 和 ancestor 范围一致。
- 同 root miss 由 singleflight 锁合并；不缓存不存在的 root。
- database-first 点查确认 root 不存在时，清除该 root 的旧缓存；嵌套 root 共享记录仍由其它 resident receipt 保留。
- `/health.model_cache` 暴露容量、驻留量、命中/加载/淘汰、inflight、存储连通性和只读降级状态。

## 验收

- `EXPLAIN FULL` 必须显示两张几何表分别使用 `idx_*_anc`。
- 按 root 加载不得带入同 dbnum 的其它 root。
- RocksDB ACK 前的候选根不得因容量压力被淘汰。
- 启动源码不得调用整库模型投影水化。
