# room_resident_model 完善开发计划

> 关联：`docs/plans/2026-09-07-resident-room-model-plan.md`（常驻模型本体 P1–P4 的落地史与 P4.5 搬家约束——锁粒度、写点 ③' / ②'、扣根口径、键 = T 仍有效）。**2026-09-08 夜用户裁决「几何数据驻留、发生修改时就地替换」**——那份计划第 6 轮 Q1 (a)「几何不驻留」翻案，本计划是几何方向的基准。

## 目标与当前结论

目标是让服务在启动阶段、对外承诺房间能力可用之前，完成指定 MDB 下房间计算所需数据的内存驻留：

- 全部 DESI 库的房间拓扑；
- 全部在册面板的世界 AABB、实例变换与几何身份；
- 当前代所需的三角网缓存；
- 可逐库核验的来源会话、覆盖率与失败状态。

启动后，房间增量只对受影响房间/面板换代和重算，不在热路径重新遍历整个 MDB 或首次读取 `.mesh`。

已完成并验证的基础：

- `room_resident_model` 缺省值已从 `false` 翻为 `true`，显式配置或环境变量仍可关闭；
- 初始化顺序已是“空间树 -> 房间驻留初始化 -> watcher -> worker”；
- 定向测试通过：`room_resident` 28 项、`room_startup` 16 项、启动顺序契约 16 项；
- 隔离 ingest 服务启动实测：215 间房、230 块在册面板已进入 resident model，耗时约 2.95 秒；
- 当前稀疏投影只有 1/230 面板具备几何，229 块来自未监听 db7997，因此尚不能宣称“全 MDB 房间模型已驻留”。

Oracle 审查结论：默认翻转可保留，但完成发布前必须补齐双启动路径、全 MDB 覆盖判定、几何预热、显式健康状态和请求就绪门。

## 完成定义

`room_resident_model=ready` 必须同时满足：

1. 选定 MDB 的 DESI 库集合与 resident source coverage 完全相等，每个 dbnum 恰好一份当前来源令牌；
2. 每个在册面板都有可用几何描述，或被记录为带 dbnum、refno、原因的明确阻断项；
3. 当前代每个唯一 `(geo_hash, world_matrix)` 已完成 `.mesh` 读取和 `TriMesh` 构建；
4. resident model 的拓扑键、几何键和来源会话属于同一冻结快照；
5. 初始化未完成时，房间重算与会写房间边的入口等待或返回 `updating`，不能使用半份模型；
6. `/health` 明确区分 `disabled / loading / ready / degraded / blocked / unsupported`，不能用 `models != []` 或 `blocked == null` 代替完成判定。

缺几何、清单缺库、文件不可读或版本混合时不写房间边，不以空边结算成功。

## 第一阶段：统一启动编排与就绪门

### 1.1 抽出共享启动函数

在 `src/lib.rs` 抽出一条共享的房间预备流程，供 ingest 的 `run_cli` 与 read-through 的 `run_read_through_core` 调用：

1. 初始化 MDB 骨架并确定全部 DESI source；
2. 装载并验证空间树；
3. 将 resident 状态置为 `loading`；
4. 构建并预热 resident room generation；
5. 原子安装成功代并置为 `ready`，或保留上一成功代并置为 `degraded/blocked`；
6. 再开放 watcher、worker 和普通 HTTP 业务入口。

保留启动健康探针，但普通房间写入口在 resident `ready` 前返回结构化 `UPDATING`。read-through 不再绕过房间初始化。（`updating` 门只管对外入口与双启动路径；**dbnum Task 的房间收尾不等待**——顺序保证初始化先行，撞 `Blocked` 按 07 计划 Q13 (a) 扣本 Task 触到的根；07 计划第 8 轮 Q22。）

### 1.2 显式状态机

在 `src/fast_model/room_resident.rs` 增加进程级状态：

- `Disabled { source }`
- `Loading { phase, started_at }`
- `Ready { generation, completed_at }`
- `Degraded { generation, problems }`
- `Blocked { problems }`
- `Unsupported { reason }`

`initialize_resident_rooms_at_startup` 返回 typed disposition，不再无条件打印“初始化完成”。没有 `project_hd/project_hh`、`spatial_compute=false`、显式关闭和构建失败必须分别落入不同状态。

## 第二阶段：严格的 MDB 覆盖与版本快可用性

### 2.1 冻结来源清单

新增 `ResidentSourceSnapshot`：

- `project / mdb / hierarchy`；
- 按 dbnum 排序的全部 DESI 文件；
- 每库冻结 `sesno`、文件身份、token；
- 期望库集合摘要；
- 拓扑摘要与几何摘要。

启动只探文件最新一次；后续 Task 使用本库冻结目标 `T`，不得在运行中追逐更新的文件右端。

### 2.2 禁止部分清单成功

修改 `initialize_published_at_startup`：

- resident 输入行的 dbnum 集合必须与 MDB source 集合完全一致；
- 任一 `Added/Advanced` 需要执行但在提交前失败时返回 `Err`，不安装旧代；
- 明确区分提交前失败与提交后快照头写失败；
- 提交后仅辅助快照失败可保留新代，但状态为 `degraded`；
- 未监听库缺失或陈旧不得只写进 summary 后继续报 ready。

结合 ADR-065 的最终迁移方向，将 `Published/Latest` 双形态逐步收敛为基于“各库冻结会话”的统一 generation；迁移完成前，两条 loader 必须实现同一覆盖和失败契约。

## 第三阶段：完整几何驻留与内存预算

### 3.1 定义驻留内容

当前 `ResidentRoomModel::assemble` 只驻留几何描述，`MeshCache` 在首次计算时才读 `.mesh`。改为启动期 eager warm：

1. 枚举所有在册面板的实例；
2. 对唯一 `(geo_hash, world_matrix)` 去重；
3. 并发读取 `.mesh` 并构建变换后的 `TriMesh`；
4. 汇总成功、缺文件、损坏、无 AABB、无实例；
5. 全部必需项成功后才安装为 ready generation。

不得只用共享缓存总长度证明当前代完整。增加当前代指标：

- `mesh_keys_expected`
- `mesh_keys_warmed`
- `mesh_warm_failed`
- `mesh_bytes`
- `panels_ready / panels_total`

### 3.2 资源约束

- 使用有界并发，默认复用现有几何并发额度；
- 按 `geo_hash` 共享不可变网格，世界变换缓存按实际重复率决定是否保留；
- 安装新代前先构建旁路 generation，安装时短持锁原子换代；
- 只保留当前代和上一代，换代后回收无引用缓存；
- `/health` 与启动日志记录耗时、峰值 RSS、缓存字节和失败样本。

## 第四阶段：Task 内增量换代

### 4.1 每库更新

在 dbnum Task 的房间收尾中接入 resident generation 更新：

1. Task 数据提交后，以冻结 `T` 捕获本库房间拓扑分片；
2. 模型发布 ACK 后，对本库全部在册面板整段重查几何（自愈，不按产物名单挑缓存条目——07 计划第 8 轮 Q21 (b)；产物名单只驱动重算目标）；
3. 在 `lock_spatial_serial` 外准备本库新分片和网格；
4. 锁内核验 generation 基线未变，合成全 MDB 新代并原子交换；
5. 只对 topology diff 与几何影响集涉及的面板/元素重算；
6. 房间边写成功后推进 resident reference，再允许 Task 收尾推进 `model_sesno`。

跨库共享房间按稳定 dbnum/refno 顺序加锁，锁内读取最新 generation；源库 Task 不持锁等待目标库。

### 4.2 失败语义

- 失败记录包含 generation、dbnum、source token、panel/root、阶段与原因；
- 新代构建失败时保留上一成功代，但本 Task 不得用旧代为新会话写边；
- 失败 Task 结束并释放槽位，相关库水位保持滞后，其他库继续；
- 不自动整批重算，不把缺几何折成 skipped，不以空关系结算成功。

## 第五阶段：健康、接口与可观测性

`/health.room_build.resident` 增加：

- `enabled`、`status`、`phase`、`generation`；
- `mdb_dbnums_expected / loaded / missing`；
- `rooms`、`registered_panels`、`panels_ready`；
- `mesh_keys_expected / warmed / failed / bytes`；
- `source_sessions`、`topology_digest`、`geometry_digest`；
- `last_swap`、`last_failure`、`startup_duration_ms`。

启动总览显示 resident 开关和最终状态。`/dbnums` 继续以数据/模型严格水位判决，不增加第三枚持久水位；resident generation 是内存状态和持久参照，不替代 `model_sesno`。

## 测试与验收

### 单元与 mem://

- 配置缺省开、配置关、环境变量双向覆盖、错误配置策略可观测；
- ingest/read-through 两条启动路径均满足空间树 -> resident -> readiness -> watcher/worker；
- MDB 三库清单完整、缺一库拒绝 ready、重复库拒绝；
- 提交前失败不安装，提交后辅助快照失败安装为 degraded；
- mesh eager warm 成功、缺失、损坏、重复 key 去重、跨代复用与回收；
- 房间改名、面板搬迁、房间号变化的 topology diff；
- 半份 generation 不能被房间计算入口取得。

### 故障注入

- 冻结拓扑后、mesh warm 中、generation swap 前后中断；
- 部分模型 ACK 后中断；
- 房间边写入前后中断；
- 重启后只安装完整 generation，边无漏删、漏写或混合版本；
- unchanged 重启零房间写，并命中缓存/分片快速路径。

### 隔离 live

使用 AMS 指定 MDB，至少覆盖 ingest 与 spawned-memory/read-through 各一次：

1. 记录 MDB DESI 库精确列表、resident 覆盖列表和差集；
2. 要求 `panels_ready == registered_panels`，否则按原因明确失败，不报 ready；
3. 记录 resident 初始化耗时、总启动耗时、峰值 RSS、mesh 缓存字节；
4. 服务 ready 后执行 FRMW 名称进/出关键字、PANE 跨房间搬迁、同面板集房间号变化；
5. 每案将规范化 `room_relate` 与 `room_panel_relate` 逐边对比 `AIOS_ROOM_FULL_REBUILD=1`；
6. 改房间号场景要求边 ID 保持不变；
7. Plant UI 验证房间选择、成员数量和模型显示随增量变化；
8. 背靠背 SAVEWORK 与冷启动在 120 秒内达到严格水位和 resident ready。

## 实施顺序与发布门

1. S1：状态机、双启动路径、业务 ready 门；
2. S2：MDB source snapshot 与严格覆盖；
3. S3：三角网 eager warm、内存预算与指标；
4. S4：dbnum Task 内按库换代与失败语义（**与 08 重构计划 S8/T4 同一笔**——删 drain 宿主与搬家一体；07 计划第 8 轮 Q23）；
5. S5：mem/fault/live/Plant UI 完整验收；
6. S6：验收通过后删除 Published 兼容形态残留与旧表清理（**pending/drain 宿主已随 S4 = S8 退**；07 计划第 8 轮 Q23）。

默认翻转是开发检查点，不单独作为生产发布依据。生产切换必须满足：双启动路径通过、全 MDB 覆盖无缺口、当前代 mesh 完整、房间双表逐边等于全量、故障恢复通过、正常 live 在 120 秒内收敛。

## 本阶段边界

- 不增加第三枚持久水位；
- 不把 `room_relate` 作为内存真值，内存驻留的是计算输入和 generation；
- 不扩大 `watch_dbnums` 的语义，resident 始终按选定 MDB；
- 不在本阶段清理旧表，先完成迁移和双态对拍；
- 不把当前稀疏 db8000 的 `1/230` 几何覆盖当作通过证据。
