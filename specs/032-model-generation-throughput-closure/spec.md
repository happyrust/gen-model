# 032 初始化模型生成吞吐闭环规格

## 背景

7997 当前水位为 106，权威生成根为 2720 个。`test-worklspace` 的探索性运行已经证明
有界根级流水可以正确补种、消费和收口，但主要耗时不在调度器：16 根 execution group
的 CATA 元件库几何构建通常约 10 秒，根后半程 wall time 平均约 2.39 秒；K=4 时累计
AABB 时间从 K=1 的约 1.48 秒上升到约 2.79 秒；Shape 生产者阻塞可以忽略，但单组仍有
约 94～149 个 SQL packet，Surreal 写入 p95 约 661 ms。

另一个工作树 `D:\work\plant-code\old\gen-model-cata-throughput` 已经完成并验证
authority-scoped `Arc<ScomInfo>` 常驻解析缓存。它把 1091 次解析加载降为 168 次，但
AMS 8000 端到端中位耗时没有改善。因此本规格不把“扩大解析缓存”当作性能方案，而是
复用其 authority、single-flight、epoch 失效和容量纪律，继续优化解析之后的 CATA
几何产品构建、Shape packet 和 AABB 查旧条目。

## 功能要求

1. 当前根选择、`watch_scope`、水位、pending revision、重试/死信、Regen→AABB 阶段屏障、
   单 Shape writer 和 `SPATIAL_STATE_SERIAL` 锁序保持不变。
2. CATA 生成必须拆成“批量加载输入 → 构建目录局部产品 → 按设计实例实例化”。目录局部
   产品不得包含世界变换、owner、sesno、SJUS、ARRI/LEAV 当前选择、generic type、NGMR
   实例 owner 或 BRAN/TUBI 实例结果。
3. 同一 execution group 内相同语义 CATA 产品只构建一次；跨组缓存启用前必须先证明
   request-local 复用语义等价。
4. 跨组 CATA 产品缓存必须按项目 authority、目录来源、CATA refno/hash、完整依赖指纹和
   算法 epoch 隔离；依赖指纹不完整时旁路，错误不负缓存，staging 读不进入常驻缓存。
5. 属性、CATR、GMRE/GSTR、NGMR、世界变换、session、generic type 和正负体关系应按组
   批量加载。记录缺失与数据库失败必须区分；Required 路径数据库失败仍使当前页失败。
6. Shape 保存继续由一个 receiver 执行。只允许同一 SavePhase 内合并 packet，不跨
   scoped-delete、冲突检查、ReplaySafe 或失败收口边界。
7. AABB 优化只替换“按 refno 查旧条目”；数据库事务、房间 pending、spatial epoch、
   `tree.sync_refnos` 和内存发布仍在原串行临界区内。派生索引必须保留重复条目多重性；
   任一写路径未同步时失效并显式回退全树扫描。
8. 新优化均有独立配置开关；关闭后回到当前已验证路径，不要求数据库 schema、水位或
   pending 修复。
9. 任务指标必须区分 execution-group wall time 与并发根累计时间，并公开 CATA 产品、
   Shape packet、AABB 查找/等待/持锁和缓存容量命中指标。
10. 只有语义、资源和吞吐硬门全部通过，优化配置才可作为默认值发布。

## 非目标

- 不增加 Shape writer，不并行发布 AABB，不取消 `SPATIAL_STATE_SERIAL`。
- 不新增模型消费路径，不让 API 直达未经治理的整库生成函数。
- 不改变生成根枚举、几何算法、mesh 容差、布尔算法或房间归属规则。
- 不把进程内缓存变成第二份持久化模型完成真相。
- 不在本规格中提高 `root_inflight_max` 上限；优化完成后只在 1～4 内重新选默认值。

## 成功标准

- CATA 阶段 p50、p95 相对 K=1 基线至少降低 50%。
- 7997 模型阶段中位耗时降低至少 35%，或当前根吞吐提升至少 50%。
- Shape SQL packet/group 至少减少 35%，Surreal 写入 p99 不高于基线 1.25 倍。
- AABB 查旧条目耗时至少降低 80%，且 K=4 的 AABB 斜率明显低于当前结果。
- 峰值 RSS 不超过基线 1.25 倍，模型写入字节数不超过基线 1.20 倍。
- `inst_info`、`inst_relate`、`inst_geo`、`geo_relate`、正负/布尔关系、规范化 mesh、
  AABB、空间关系、房间关系、`gen_root` 完成凭证与水位逐字段语义哈希完全一致。
- 最终当前根全部进入合法成功终态，Regen/AABB/空间写回为 0，`model_ready=true`，范围外
  dbnum 没有持久化变化。

## 决策引用

- ADR-011：单协调器与单模型消费路径。
- ADR-025：初始化阶段顺序与模型就绪门。
- ADR-045（spatial-tree-refno-lookup-on-read-path）：AABB 按 refno 派生索引与回退纪律。
- `specs/023-parallel-root-generation-pipeline`：统一几何闸和有界根级并发。
- `specs/026-spatial-tree-refno-lookup`：AABB 查旧条目实施前置与验证门。
- 外部集成资产：`gen-model-cata-throughput/specs/027-cata-generation-throughput` 与
  `specs/028-cata-resident-cache`；集成时只迁移与当前规格相关的文件/符号，不整体合并脏工作树。
