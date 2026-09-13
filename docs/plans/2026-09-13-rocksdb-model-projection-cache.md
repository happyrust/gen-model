# RocksDB 持久基线 + kv-mem 增量 Overlay

## 决策

- `external` 与 `spawned-rocksdb` 的 SurrealDB/RocksDB 是重启后的持久真相源和冷数据查询层。
- `ModelMemoryStore` 不从 RocksDB 水化；它只保存本进程新生成的完整 root 增量快照。
- 读 root 时，若 kv-mem 有该 root 的 `RootReceipt`，内存快照覆盖 RocksDB 旧根；否则直接查询 RocksDB，结果不写入 kv-mem。
- 生成提交内存后立即可读，并把 root 交给进程内后台 writer；前台不等待 RocksDB 事务。
- 后台失败时保留内存快照并指数退避重试。进程异常退出后，源文件仍可重新生成 RocksDB 尚未确认的部分。

## 配置

```toml
model_cache_mode = "incremental-overlay"
model_cache_max_bytes = 2147483648
```

`memory-first` 仅作为旧配置的兼容别名；`database-first` 被拒绝，因为任何 RocksDB → kv-mem 回填都违反 overlay 边界。

## 双水位

正确性按 root 判断，库级最大会话号只可用于统计：

- memory watermark：`RootReceipt` 的 `source_sesno + manifest_hash + geometry_count + model_target`。
- durable watermark：RocksDB 根事务成功后的 `RootPublicationAck`，携带同一组内容身份。
- 两者完全相同才是 clean；否则是 pending/dirty，不能淘汰。
- 同 sesno 不同 manifest 或 target 仍是新内容，旧后台任务完成后不得误推进 ACK。

## 生命周期

1. 生成器把完整 root 快照提交到 `ModelMemoryStore`，旧 ACK 同步失效。
2. 刷新任务只等待 bounded queue 接收 root，不等待数据库 I/O。
3. 后台 writer 从当前完整内存快照渲染幂等替换事务并提交 RocksDB。
4. 事务成功后，只有 ACK 与当前 `RootReceipt` 完全一致才推进 durable 水位。
5. 水位追平后，该 root 才可按内存上限参与 LRU 淘汰。

## 可观测性与验收

- `/health.model_cache` 暴露 `pending_roots`、`durable_roots`、驻留字节与淘汰统计。
- 冷 root 查询不得增加 `resident_roots`。
- 内存 root 必须覆盖 RocksDB 中同 root 的旧版本。
- RocksDB 不可用时 pending root 保留且继续可读；后台重试不得阻塞生成请求。
- 同 sesno 不同内容的旧 ACK 不得使 root clean 或可淘汰。
