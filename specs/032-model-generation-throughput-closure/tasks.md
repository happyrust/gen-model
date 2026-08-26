# 032 初始化模型生成吞吐闭环任务

## P0：基线与单一集成入口

- [ ] T001 `docs/evidence/2026-08-25-model-throughput-closure/baseline/`：保存当前7997最终收敛
      日志、health、程序/config/源码哈希、根数、水位、PE、语义hash命令与退出码。依赖：无。
- [ ] T002 `docs/evidence/2026-08-25-model-throughput-closure/integration-manifest.md`：逐文件/符号列出
      `D:\work\plant-code\old\gen-model-cata-throughput` 中需要汇总的解析缓存改动，明确排除
      OCC/RVM及其他脏变更。依赖：T001。
- [ ] T003 `src/fast_model/cata_cache.rs`、`resolve.rs`、`cata_model.rs`、staging失效hook、配置和测试：
      把T002的focused资产汇总到当前分支；容量0/cache-off先过旧路径等价。依赖：T002。
- [ ] T004 [P] `src/fast_model/gen_model.rs`、`cata_model.rs`、`shape_save.rs`、`occ_generate.rs`：
      增加group wall、产品key、缓存、packet、AABB查找归因字段；不改行为。依赖：T001。
- [ ] T005 `docs/evidence/2026-08-25-model-throughput-closure/attribution/`：连续采集至少20组，计算
      产品key跨组重复率、CATA build p50/p95、SavePhase packet和stale lookup占比，执行20%/30%
      两个停止门。依赖：T003、T004。

## P1：CATA产品与批量读取

- [ ] T006 `src/fast_model/cata_model.rs`：定义只含目录局部语义的`CataProduct`/
      `CataProductKey`，拆分load/build/instantiate；cache-off也走同一实现。依赖：T005。
- [ ] T007 [P] `src/fast_model/cata_model.rs`测试：世界变换、SJUS、ARRI/LEAV、owner、NGMR owner
      和session不进入产品，两个共享CATA但实例上下文不同的元素输出正确。依赖：T006。
- [ ] T008 `src/fast_model/cata_model.rs`、`query.rs`：按execution group批量预载属性、CATR、
      GMRE/GSTR、NGMR、变换、session、generic type和正负体关系；数据库失败与missing分型。
      依赖：T006。
- [ ] T009 `src/fast_model/gen_model.rs`：接request-local产品复用，保持单Shape sender/receiver；
      同key single build，失败仍逐根定位。依赖：T007、T008。
- [ ] T010 `docs/evidence/2026-08-25-model-throughput-closure/request-cache/`：cache-off/request逐表、
      mesh和错误语义hash；CATA p50/p95至少下降30%。依赖：T009。

## P2：有界常驻产品缓存

- [ ] T011 `src/data_interface/cata_closure.rs`、`src/fast_model/cata_cache.rs`：生成包含完整排序依赖
      闭包的digest，并绑定authority、目录来源、effective_end_sesno和algorithm epoch。依赖：T010。
- [ ] T012 `src/fast_model/cata_product_cache.rs`：实现256项/64MiB双上限、single-flight、epoch、
      reverse-dependency、staging bypass、错误不负缓存和容量0回滚。依赖：T011。
- [ ] T013 [P] `src/fast_model/cata_product_cache.rs`测试：并发同key单leader、失败后可重试、旧epoch
      flight不覆盖新flight、LRU/bytes约束、Required失败不被旧缓存遮蔽。依赖：T012。
- [ ] T014 `src/fast_model/gen_model.rs`：接shadow模式，新旧产品逐字段比较且生产仍使用旧结果；
      mismatch进入任务/health。依赖：T012、T013。
- [ ] T015 `docs/evidence/2026-08-25-model-throughput-closure/product-cache/`：完整7997 shadow零差异
      后切on；构建次数-70%、CATA p50/p95-50%、RSS门通过。依赖：T014。

## P3：Shape与AABB

- [ ] T016 [P] `src/fast_model/pdms_inst.rs`、`shape_save.rs`：在同SavePhase内实现300/1MiB、
      600/2MiB、900/3MiB候选，不改writer数、等待时间和删除/冲突顺序。依赖：T010。
- [ ] T017 [P] Shape测试：packet边界可变但逻辑SavePlan一致；注入第N包失败后重放收敛；
      TargetedReplace scoped delete顺序不变。依赖：T016。
- [ ] T018 `docs/evidence/2026-08-25-model-throughput-closure/shape-packets/`：选择packet至少-35%、
      write p95≤1.15倍且无新增重试的最小候选。依赖：T017。
- [ ] T019 `specs/026-spatial-tree-refno-lookup`：完成T03占比门；不足30%则记录停止决定并跳到T023。
      依赖：T005。
- [ ] T020 `../vendor/old-aios-core/.../acceleration_tree.rs`及`src/fast_model/occ_generate.rs`：
      完成refno索引接口与直写/staged shadow接线，保留重复多重性和现有锁/发布顺序。依赖：T019。
- [ ] T021 [P] `src/data_interface/helper.rs`、`spatial_state.rs`和树测试：覆盖删除、reconcile、启动加载、
      pending replay、全树替换；漏hook时索引失效并回退扫描。依赖：T020。
- [ ] T022 `docs/evidence/2026-08-25-model-throughput-closure/aabb-index/`：完整7997 shadow mismatch=0，
      查找-80%、房间pending/spatial epoch/树hash一致后切on。依赖：T021。

## P4：默认值、正式A/B与交付

- [ ] T023 `scripts/`：建立group 8/16/32 × K 1/2/3/4候选runner；选择通过全部资源/正确性门且
      距最佳吞吐5%内的最小配置。依赖：T015、T018、T022或T019停止记录。
- [ ] T024 `docs/evidence/2026-08-25-model-throughput-closure/ab/`：独立SurrealDB 2.1.x相同7997
      快照，随机三轮legacy/三轮优化版，以`model_ready=true`最终收敛为停止边界。依赖：T023。
- [ ] T025 [P] 更新`changelog.md`、ADR-025实施说明、`docs/2026-08-12_live-test-ledger.md`和
      本规格任务状态。依赖：T024。
- [ ] T026 运行fmt、专项单测、model pending、on-demand、check、CI Release、Python客户端测试；
      所有命令/字面输出/退出码入验证记录。依赖：T024。
- [ ] T027 执行`sigmap verify-plan`、`sigmap verify-ai-output`、`sigmap review-pr`，并核对没有
      第二条模型消费路径、第二个Shape writer或并行AABB发布。依赖：T025、T026。
- [ ] T028 生成并复核修改程序、focused patch、验证记录、回滚脚本；在干净HEAD归档执行patch
      check，执行回滚dry-run。依赖：T027。

## 依赖顺序与并行说明

- 主链：T001→T002→T003→T005→T006→T008→T009→T010→T011→T012→T014→T015→T023→T024。
- Shape链T016～T018可在T010后与产品缓存并行；AABB链T019～T022可在T005后并行。
- `[P]`仅表示文件所有权无交叉时可并行；共享`cata_model.rs`、`gen_model.rs`或当前运行配置时仍串行。
- 当前`test-worklspace`运行保持到收敛；T024正式A/B不复用已经多次重启的探索性运行。

## 完成定义

只有T001～T028全部完成、性能硬门通过、语义hash一致、范围外零变化、四类交付物可重放且
回滚dry-run通过，规格才可标记完成。任何单项缓存命中率、HTTP 200或局部测试通过都不构成
完成证据。
