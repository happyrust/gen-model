# 开发方案：稳态增量窗口 kv-mem 暂存与整窗口写回（ADR-017）

> 决策见 `docs/adr/ADR-017-staged-increment-window-commit.md`；词汇见 `CONTEXT.md`「暂存与写回」章节。本文只写「改什么、按什么顺序、怎么验收」。方案经三轮拷问会话敲定（2026-08-05），关键裁定：**整批才落盘（含全部生成根成功）**、**死信 = 纯阻断（无降级提交）**、**房间尽力而为**、**基线豁免**、**CATA 产物随窗口提交**。另经外部对抗式审核修订一轮（GPT-5.5 Pro，采纳/驳回记录见 `docs/2026-08-05_kvmem-staged-increment-oracle-review.md`）：新增 T0.5 ReplaySafe 门槛、T0.6 黄金等价 harness、T2.0 副作用前置、资源状态机与吸收重置 attempts。

## 1. 目标与不变量

- **I1 零落盘**：窗口计算期间，持久层（rocksdb 后端 SurrealDB 服务器）的数据表零写入。白名单只有控制面：`dbnum_watermark`（仅扫描观察字段）、`increment_update_attempt`（attempts / 阻断记录）、`queue_control`、`model_update_pending`（房间 / 派生 / 按需）。
- **I2 水位门控**：`applied_sesno` 只在写回尾事务推进；写回任何一块失败不推、不回退。
- **I3 重试经济**：staging database 跨重试保留，重试只重跑失败根；进程崩溃 = 整窗口重算并收敛（幂等）。
- **I4 终态等价**：同一窗口「暂存 + 写回」的持久层终态 == 现行直写路径终态，逐表对拍相等。
- **I5 失败语义**：生成根死信 = 窗口阻断（一级告警、持久层零痕迹）；房间失败不阻断、留 pending。
- **I6 豁免不变**：基线 / 冷启动 / `gen_all_geos_data` 全量路径行为一字不变。

## 2. 事实基线（2026-08-05 审计）

- 连接形态：默认 feature `ws`，`SUL_DB`（`Surreal<Any>`）经 WebSocket 连 fork SurrealDB v2.1.4（rocksdb 存储）；viewer / 材料表 / plant-ui 连同一服务器，且**只有 gen-model 写**（业主确认）。`kv-mem` feature 当前未启用，本仓无 `mem://` 用例。
- 窗口非原子：`persist_latest_main_data` 按 `TX_CHUNK=500` 分块；「整窗口单事务撑爆 ws 通道」是已记录事故（`increment_pipeline.rs` 注释，amssys 冷启动 4000+ 元素）。唯一真原子点是 `finalize_attempt`（datacenter + durable pending + 水位 + attempt 删除，`model_update_pending.rs`）。
- 生成轮「读自己写的」SQL 链（kv-mem 必要性的实证）：`save_instance_data` 写 `inst_relate/geo_relate/inst_geo/world_trans` → `process_meshes_update_db_deep` 里 `query_deep_visible_inst_refnos` 反读刚写的 `inst_relate` → `update_inst_relate_aabbs_by_refnos` 再读刚写的 `geo_relate/inst_geo.pts`。
- 全局扫描语句存在于写路径内部：如 `pdms_inst.rs` 的 `SELECT * FROM inst_relate WHERE zone_refno = NONE` 回填（commit-time-only 的第一个客户）。
- 解析窗口是纯写（语句由内存解析结果渲染）；房间先清后写是渲染好的 DELETE+RELATE，同轮吸收判定靠进程内集合；非 regen 动作读的是批前状态。
- drain 三阶段硬约束（非 regen → regen → 房间）与单 worker（ADR-011）不变；`execute_item` / `drain_where` / `room_round` 是接入点。

## 3. 分期任务

### P0 基础设施（可独立合入，全部有单测；T0.5 / T0.6 是进入 P1 的门槛）

- **T0.1 嵌入式 mem 引擎打通 + 双跑一致性套件**：fork SDK 开 `kv-mem` feature；建立 `fork-surreal-compat-suite`——同一批 SQL 在 mem 引擎与 fork 服务器（rocksdb）双跑对比，覆盖：`init_model_tables` 全部 DEFINE、`define_common_functions`（fn::，含 `fn::room_code` / `fn::room_num_of` 覆盖顺序）、`ast_payload` 兼容、`INSERT RELATION` 撞 id 行为（ADR-010 D13：fork 服务器静默保留旧行）、事务隔离语义、record id（含 `⟨⟩` 形制）序列化、schemaless 表接受裸对象的行为。差异逐条记录在案，作为读路由与 journal 设计的输入。
- **T0.2 StagedExecutor**：gen-model 侧新增执行器封装——`execute(sql, ExecMode)`，`ExecMode ∈ {Both(暂存执行+进日志), StagingOnly(只暂存不进日志), CommitOnly(不暂存只进日志，或对工作集行暂存执行)}`；`journal()` 有序语句表；`commit()` = TX_CHUNK 分块重放（CommitOnly 语句按原始位置参与）+ 调用方提供的尾事务；`abandon()` = DROP staging database + 丢日志。rs-core 侧新增 `STAGE_DB` 静态句柄与「批次执行上下文」路由点（task-local 当前库或显式句柄传递），`SurrealQueryExt` / `get_pe` / `get_named_attmap` / `get_world_transform` 等读入口接路由。**fail-closed 纪律：暂存上下文中路由不到的读一律报错，不静默回落持久层。**
- **T0.3 staging database 生命周期与资源状态机**：命名 `staging_{dbnum}_{window_id}`（window_id 进程内单调分配，逻辑会话区间入元数据，吸收扩窗不改名）；建库时初始化表定义与 fn::；窗口提交 / 废弃后 DROP，进程内登记表 + 窗口终态清扫兜底残留。资源治理三级状态机（行数 + 字节 + journal 字节计入同一配额）：告警阈值告警、更高阈值拒绝吸收扩窗、极限阈值废弃暂存并转入资源阻断告警——不允许走到 OOM；指标接 `/api/v1/health`。
- **T0.4 attempts 控制面**：`increment_update_attempt` 扩展 per-root attempts（键 = `(dbnum, root_refno)`）；窗口成功提交的尾事务清除该 dbnum 全部 attempts（正当性：A 语义下窗口能提交当且仅当全部根成功）；**冻结吸收扩窗时重置受影响根的 attempts**——这是窗口阻断的解除机制（修复重存 → 吸收 → 归零 → 重算），必须有测试钉住；`MAX_ATTEMPTS` 到达 → 窗口阻断状态记录（阻断原因、坏根、首次/最近失败时刻）。
- **T0.5 ReplaySafe 语句规范 + journal validator（门槛）**：成文规范——record id 显式固定、不依赖随机值、不以执行时刻的全库查询结果选择写入目标、`time::now()` 只允许出现在信息性字段；validator 在 `execute()` 入口拒绝不合规语句；配「随机中断写回再重放」的收敛测试。
- **T0.6 mini-window parity harness（门槛，黄金等价测试）**：不依赖生成管线的小型窗口——insert / update / delete / relation / fn:: 调用 / commit-time-only 各一条，走「暂存 + 写回」与「直写」两条路径，逐表 hash 对拍相等；后续每个接入阶段都在此 harness 上先行验证。

### P1 解析窗口接入

- **T1.1** `persist_latest_main_data` 改走 StagedExecutor（`Both`）：语句既在暂存库生效（供后续生成读窗口新态）也进日志。
- **T1.2** `ref_rev` 反向索引语句改 `Both`；重审 ADR-003 的 durable 恢复语义——窗口失败 = 整窗重放，`enqueue_ref_rev` 恢复记录只保留给跨窗口修复通道。
- **T1.3** `finalize_attempt` 重构为「尾事务渲染器」：datacenter 语句（本就是 commit-time 语义）+ 水位推进 + attempts 清除 + 房间/派生 pending 的 revision 条件收口，全部在持久层一个事务判真执行。
- **T1.4** 冻结吸收与暂存扩展：窗口执行起点重扫抬高上界时，吸收区间的增量解析补充进**同一个** staging database（window_id 命名不变，元数据记录吸收后的逻辑区间）；吸收同时重置受影响根的 attempts（T0.4）；资源状态机处于「拒绝吸收」档位时不吸收，后继排队行保持独立。

### P2 生成轮接入（核心）

- **T2.0 副作用延迟挂钩（本阶段前置，原 P4 内容提前）**：空间树应用与 `AABB_TREE_DIRTY`、MQTT 模型变更通告、`accel_tree.bin` 落盘全部改挂到写回成功回调；缓存失效以提交 / 废弃为边界。生成轮接入之前必须先落地，否则暂存计算会把未提交状态泄漏进空间树与通告。
- **T2.1 计划层读切换**：`build_model_update_plan` 的 owner 链 / 类型 / 名称读走暂存库；按需把变化元素的祖先链部分解析进暂存（索引优先定位 + 迭代上溯）。
- **T2.2 生成根闭包预解析**：对每个生成根，把「根子树 + CATA 引用闭包」部分解析进暂存库（复用既有 部分解析 / 生成根闭包 / 索引优先建表 机制，把落库目标从持久层改为暂存库）。
- **T2.3 既有产物预载**：根的 `inst_relate/inst_info/geo_relate/inst_geo/world_trans/aabb` 与隐含直管段（tubi）旧行，从持久层点查拷入暂存库——保证部分失败时旧行可见、删除集推导与今天一致。
- **T2.4 生成执行链指向暂存**：`save_instance_data`、`update_inst_relate_aabbs_by_refnos`、`query_deep_visible_inst_refnos`、`update_world_transforms`、级联删除等在批次上下文一律走暂存句柄；**惰性兜底跨源版**——设计 / 目录 miss → 文件定点解析入暂存；产物 miss → 持久层点查拷入（两类 miss 都必须打日志）。
- **T2.5 commit-time-only 审计**：逐语句过一遍生成写路径，全局扫描 / 修补语句（`zone_refno` 回填等）按读写集分类标注——纯终态扫描直接 `CommitOnly`；写集与暂存读集相交的必须同时以工作集范围在暂存执行；依赖唯一性补洞或删除后状态的必须显式定义执行顺序；CommitOnly 语句写回时按 journal 原始位置执行（中间态语义 = 今日直写）。产出审计清单入库（表格：语句、位置、读集、写集、判定、理由）。StagedExecutor 对未标注的跨表扫描写语句默认拒绝执行（防新增遗漏）。
- **T2.6 窗口状态机与生成根锁**：全部根成功 → 进入写回；存在失败根 → 按 attempts 重试（复用暂存成果）；穷尽 → 窗口阻断（告警 + 面板可见），staging 保留至阻断解除或进程退出。窗口对其生成根在「生成开始 → 写回完成」全程持有生成根锁；on-demand 命中被暂存根时等待或拒绝（回执注明窗口进行中）。

### P3 房间轮接入

- **T3.1** `AabbChange` 由暂存侧包围盒刷新产出；本窗口房间任务在窗口内执行（drain 第三阶段挪进窗口）；PanelIndex 的面板行预载进暂存（百余行，逐轮一次）。
- **T3.2** 房间先清后写语句进日志（`Both`）；任务成功 → settle 语句随尾事务；任务失败 → durable pending 照旧入表（控制面直写，允许）；空闲轮房间轮保留，只消化积压，其执行同样以小提交单元走 StagedExecutor。
- **T3.3** 同轮吸收 / 封闭性检查在暂存语义下复验（吸收判定读的旧边来自持久层批前状态 + 本轮进程内写入集，语义与今天一致，需测试钉住）。

### P4 写回收尾与可观测性

- **T4.1** 写回执行与恢复状态机：分块重放 + 尾事务；写回失败且进程存活 → 保留暂存与内存 journal、指数退避重试 N 次，仍失败进入「写回滞留」告警（区别于窗口阻断——数据是好的，只是持久层不可用）；进程崩溃 → journal 随进程消失，唯一路径是整窗口重算，重算的 regen 删除集覆盖先前半提交行、幂等收敛。两条恢复路径由构造互斥（journal 只活在内存），实现与测试都要钉住这一点。
- **T4.2** 副作用回归验证：T2.0 已把挂钩前置，此处补齐废弃 / 重算路径的缓存失效（`clear_all_caches_batch` 纪律）与全链路回归。
- **T4.3** 可观测性：任务面板与 `/health` 增加窗口阻断状态（坏根、attempts、首次失败时刻）、staging 内存与资源状态机档位、写回时长与重试次数。

### P5 验收与对拍（唯一硬标准：I4 终态等价 + I1 零落盘）

- **T5.1 隔离性探针**：live 用例在窗口执行中途对持久层做数据表快照 diff，必须为空（控制面白名单除外）。
- **T5.2 终态对拍**：room_fixture 式合成库 + 真实会话 live 用例（`live_projams_real_attribute_sessions_*` 系列），同一窗口分别走「暂存写回」与「现行直写」，逐表（pe / attrs / inst_* / trans / aabb / room_* / datacenter / 水位 / pending）比较相等；对拍维度另加：mesh 文件集合 hash、空间树 checksum、MQTT 通告条数与口径。
- **T5.3 故障注入**：坏根阻断（水位不动、持久层零痕迹、修复重存后收敛）；写回中途 kill -9（重启幂等重放收敛）；房间坏网格（窗口提交成功、pending 留存、空闲轮收敛）；吸收扩窗（同一 staging 扩展、终态等价）。
- **T5.4 全量回归**：`cargo test --lib --features http_api`（当前 346 条）全绿 + live 夹具逐个实跑。
- **T5.5 性能与内存基线**：典型窗口（单元素改动 / 整 BRAN 重排 / 百元素窗口）的端到端耗时与 staging 峰值内存，对比现行路径，回归阈值入 CI 报告。

## 4. 风险清单

- **R1 读路由覆盖面（最大风险）**：rs-core 读入口多（`get_pe` / attmap / 世界变换 / fn:: 调用），漏一处 = 生成读到持久层旧属性、静默错模型。缓解：批次上下文默认路由 + fail-closed（路由不到就报错）+ T5.2 对拍兜底。
- **R2 DESI 惰性兜底首次引入**：闭包漏边在设计侧没有历史经验。缓解：miss 必打日志、对拍用例覆盖祖先链 / 跨根引用。
- **R3 长窗口资源失控**：冻结吸收 + CATA 闭包 + 产物会放大暂存与 journal。缓解：T0.3 资源状态机（告警 / 拒绝吸收 / 废弃暂存），不允许走到 OOM；窗口切分预案与 A 语义相互作用，触发时单独立项再议，不预做。
- **R4 commit-time-only 语义漂移与遗漏**：暂存态与提交态对全局扫描语句可见的世界不同；新增语句绕过标注。缓解：读写集分类审计（T2.5）、按原始位置重放、StagedExecutor 默认拒绝未标注跨表写。
- **R5 mem 引擎与 fork 服务器行为差异**：`INSERT RELATION` 撞 id、schemaless 接受裸对象等 fork 特性在 mem 引擎上的一致性未知。缓解：T0.1 行为验证清单 + 差异记录。
- **R6 on-demand 与窗口并发**：窗口全程持有生成根锁（T2.6），on-demand 命中被暂存根时等待或拒绝；其余场景读持久层提交态，与今天一致。
- **R7 写回窗口的读者可见性**：phase-1 残余（秒级），消除依赖 phase-2 fork 暂存会话（BEGIN STAGING / overlay / 原子 COMMIT，方向已定不开工；立项前先压测 rocksdb 长事务 / 快照钉住 / overlay 内存）。
- **R8 阻断窗口的孤儿 mesh 累积**：反复重试且数据变化时内容寻址 mesh 会累积。缓解：列独立 GC 后续项（按引用与时龄回收），不阻塞本方案。

## 5. 明确不做（本期）

- 不做常驻全库镜像；不做任何形式的降级提交（自动或人工）；不改基线 / 冷启动 / 全量生成路径；不动 ADR-012 合批策略与 fresh/retry 判据；不动 ADR-015 pending 身份；phase-2 fork 暂存会话只立方向不设计细节。
