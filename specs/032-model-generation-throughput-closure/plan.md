# 032 初始化模型生成吞吐闭环实施计划

## Constitution Check

- **水位是承诺**：所有缓存、批量读取和 packet 调整发生在模型派生阶段，不改
  `applied_sesno`、尾事务或完成凭证条件。
- **单一权威路径**：`run_regen_group → generate_roots_report` 继续接收 durable pending
  选出的根；缓存、预载和索引不能创建、删除或重排根。
- **静默失效零容忍**：批量查询失败、缓存依赖指纹缺失、索引失效均有显式状态；Required
  数据库失败上浮，索引失效回退现有扫描并计数。
- **队列三条出路不变**：优化结果不参与 attempts/revision/死信裁决，失败仍由现有逐根
  收口路径处理。
- **并发纪律不变**：复用全局 `geometry_gate`，保持单 Shape writer、单 AABB 发布和既有锁序。
- **运行环境**：Windows/nightly，不执行 `cargo clean`，live 一律使用独立 SurrealDB 2.1.x
  数据目录。

本规格不引入新的持久化真相或崩溃恢复语义，不新增 ADR。若实现需要持久化 CATA 产品或
并行发布 AABB，必须停止并另起 ADR。

## 当前证据与停止门

探索性 7997 运行只用于定位，不作为正式 A/B：CATA 约占 execution group wall time 的
70%～80%；Shape 生产者累计阻塞约 1.6 ms；Surreal 写 p95 约 661 ms；AABB 持锁/等待
随 K 增长，adaptive 已能在 1～4 间回退。

在修改代码前冻结至少 20 个连续组的原始日志和 health 快照。若 CATA 产品语义键跨组重复率
低于 20%，不做跨组产品缓存，保留批量读取和 request-local 复用；若 stale lookup 占 AABB
不足 30%，按 `specs/026` 停止索引切换，先做 Shape packet。

## 实施阶段

### 阶段 0：单一集成入口与基线

1. 当前 7997 进程继续跑到最终收敛；不再把中途重启数据当性能基线。
2. 建 `docs/evidence/2026-08-25-model-throughput-closure/`，记录二进制/源码/config 哈希、
   数据快照、权威根数、PE/水位、分段日志和语义 hash 命令。
3. 为 `gen-model-cata-throughput` 建 focused integration manifest，只列其现有解析缓存模块、
   resolver/CATA 调用点、配置、测试和依赖失效 hook；不导入
   OCC/RVM/其他未完成改动。
4. 在当前分支建立唯一集成点，先让已有解析缓存的 cache-off/on 测试重新通过，再开始产品优化。

### 阶段 1：归因与 CATA 产品边界

1. `src/fast_model/gen_model.rs` 增加 execution-group wall clock 和 CATA/Shape/根后半程独立计时。
2. `src/fast_model/cata_model.rs` 把 `gen_cata_single_geoms` 拆成同一实现共享的
   `load_cata_product_inputs`、`build_cata_product`、`instantiate_cata_product`。
3. 删除调用前后重复的 `get_named_attmap`；把世界变换、SJUS、owner、session、generic type
   留在实例化阶段。
4. 先使用 request-local `HashMap<CataProductKey, Arc<CataProduct>>`，不跨组。

阶段门：cache-off/request 语义 hash 相同，同组相同 key 单构建，CATA p50/p95 至少下降 30%。

### 阶段 2：批量预载与有界常驻产品缓存

1. 按组收集唯一设计 refno、CATR/CATA refno、GMRE/GSTR、NGMR 与正负体关系，使用已有
   批量 API 或新增单一批量 API；标量回退次数进入 telemetry。
2. 从 dependency closure 生成规范化 `dependency_digest`。第一版额外绑定当前 authority、
   `effective_end_sesno` 和算法 epoch，宁可少命中，不跨快照复用。
3. 复用已验证解析缓存的 single-flight/epoch/reverse-dependency/capacity 机制，新增独立的
   `CataProductCache`；初始 256 项/64 MiB，错误不缓存，取消不污染 flight。
4. `shadow` 模式同时构建新旧结果，生产采用旧结果；完整运行零差异后切 `on`。

阶段门：构建次数降低至少 70%，CATA p50/p95 至少下降 50%，RSS 门通过，Required 失败纪律不变。

### 阶段 3：单 writer 的 Shape packet 减量

1. 保留 `run_shape_save_receiver`、`FrozenShapeBatch` 和 writer 数量。
2. 在 `pdms_inst::build_save_plan/execute_save_plan` 边界按 SavePhase 合并。
3. A/B 候选为 300 行/1 MiB、600 行/2 MiB、900 行/3 MiB；不先调长通道等待时间。
4. 注入第 N 个 packet 失败并重跑全批，证明 ReplaySafe 与 scoped-delete 顺序不变。

阶段门：packet/group 至少下降 35%，写 p95 不高于基线 1.15 倍，重试不增加。

### 阶段 4：AABB refno 派生索引

1. 先完成 `specs/026` T03 的占比门；满足后复用 `AccelerationTree` 的 refno index 接口，值保留
   同 refno 多条记录。
2. 直写与 staged 两条分支都在现有树锁/空间串行锁内查询索引；事务成功且 tree 同步后更新索引。
3. 启动加载、全树替换、删除、reconcile、pending replay 全部接 hook；漏接即 `valid=false`。
4. `shadow` 完整跑一轮，逐 refno 比较索引与全树扫描的条数和值；生产仍使用扫描结果。

阶段门：shadow mismatch=0，查旧条目至少快 80%，房间 pending 和 spatial epoch 集合一致。

### 阶段 5：重新选择默认并发和正式 A/B

优化开关固定后再测 execution group 8/16/32 与 K 1/2/3/4。选择所有硬门通过、吞吐距离最佳值
不超过 5% 的最小配置。正式 A/B 使用同一独立 7997 快照，随机顺序运行至少三轮 legacy 和
三轮优化版，以最后一个根完成、AABB/空间写回收敛、`model_ready=true` 为停止边界。

## 配置与即时回滚

```toml
model_cata_read_preload = false
model_cata_product_cache = "off" # off | request | shadow | on
model_cata_product_cache_max_entries = 256
model_cata_product_cache_max_bytes = 67108864
model_shape_packet_rows = 300
model_shape_packet_bytes = 1048576
model_aabb_ref_index = "off" # off | shadow | on
```

正常回滚顺序：AABB 索引 off → Shape 300/1 MiB → 产品缓存 off → 批量预载 off。纯
`CataProduct` 重构只有在 cache-off golden 等价后才保留。语义几何不一致先关产品缓存；空间
不一致先关 AABB 索引。全部改动是进程内派生或 packetization，不做 schema 回滚。

## 质量门与交付物

- `cargo fmt --check`
- `cargo test --locked --lib model_update_pending --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture`
- `cargo test --locked --lib on_demand_model --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture`
- CATA cache/product、Shape replay、AABB index 专项测试。
- `cargo check`
- CI Release 构建命令。
- `sigmap verify-plan`、`sigmap verify-ai-output`、`sigmap review-pr`。
- 修改程序、focused patch、含命令/输入/字面输出/退出码的验证记录和可运行回滚脚本。

## Oracle 复核

Oracle 会话 `review-and-optimize-the-current` 建议顺序为 CATA 产品构建 → 批量读取 → Shape
packet → AABB refno index → 最后再调 K；同时明确要求保留单 writer 和串行空间发布。本计划
按当前源码、既有解析缓存实测和 7997 现场指标校正后采用该顺序。
