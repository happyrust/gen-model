# ADR-045：CATA 常驻解析缓存以 committed authority 为边界

## 状态

已接受（2026-08-24）。引用 ADR-001、ADR-011、ADR-017、ADR-021、ADR-041。

## 背景

ADR-041 已消除同一分页内重复解析 SCOM，但完整初始化和后续分页仍会重复读取、遍历并解析
相同的静态目录定义。ADR-017 的 `kv-mem` 是尚未提交窗口的暂存数据库，不具备长期权威性，
也不能作为跨提交的 CATA 缓存。

## 决策

1. RocksDB/SurrealDB 是唯一权威源。只有 committed 读视图加载出的 `Arc<ScomInfo>` 可以
   进入进程内常驻缓存；staging SCOM、`CataContext`、设计属性和世界变换只在当前页存活。
2. 缓存键是 `(AuthorityId, SCOM RefnoEnum)`。数据库成功初始化或重新初始化时产生新的
   authority；publication epoch 只作发布围栏，不进入键。
3. 同一键、同一 epoch 只允许一个独立 spawned loader。发起者取消不取消共享加载；数据库
   错误、目录缺陷、取消和被新 epoch 取代的结果都不准入缓存。
4. loader 记录实际读取的目录依赖并维护双向索引。权威提交只在持久数据、尾部状态和水位
   全部成功后，原子执行 selective/full invalidation 并推进 epoch；依赖覆盖不完整时必须
   full-authority，提交失败与丢弃 staging 都不得推进 epoch。
5. 缓存按估算字节数和条目数双重有界，以 access tick 近似 LRU，超限回收到 90%。单项
   超限可供当前调用使用但不准入；`cata_cache_max_bytes=0` 关闭常驻准入。
6. 删除旧 `SCOM_INFO_MAP`、SCOM redb/`BytesTrait` 接线及隐式缓存入口。通用属性、变换
   `CacheMgr` 保留，SCOM 失效从 `clear_all_caches_batch` 拆出。

## 后果

- HTTP、Python、数据库 schema、mesh 与持久身份不变；消费侧改为共享 `Arc<ScomInfo>`。
- 多项目、重连和重建由 authority 隔离；staging 绝不会污染 committed cache。
- 回滚只需把常驻字节容量设为 0，仍保留显式读视图、错误分类和页内批量读取。
