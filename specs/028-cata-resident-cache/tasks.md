# 028 CATA 常驻解析缓存任务

- [x] T01 新增 ADR-045/spec 028 并修订 ADR-041/spec 027。
- [x] T02 在 `src/fast_model/cata_cache.rs` 实现 authority、single-flight、容量和统计。
- [x] T03 将 `resolve.rs`/`cata_model.rs` 迁移为显式 scope 和 `Arc<ScomInfo>`。
- [x] T04 在 staging commit 边界接入依赖失效与 epoch 发布。
- [x] T05 删除 core `SCOM_INFO_MAP`、SCOM redb/`BytesTrait` 接线。
- [x] T06 更新配置、漂移检查、testbed、changelog、台账和 evidence。
- [x] T07 运行单元/并发、nightly 构建、Python offline 与 SigMap 门。
- [x] T08 用三组 cache-on/off 新进程和独立空 RocksDB 完成 AMS 8000 验收。
