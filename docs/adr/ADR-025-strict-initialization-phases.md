# ADR-025：数据初始化采用严格阶段屏障

状态：Accepted（2026-08-14）

关联：ADR-004（按需 CATA）、ADR-007（SYS 数据域）、ADR-011（单数据队列）、
ADR-017（暂存窗口提交）、ADR-021（水位数据支撑）；`specs/006-strict-initialization-order/`

## 背景

启动重扫过去按文件大小遍历并直接入队。全量同步虽然先等待 SYS，但把 DESI 与 CATA
放在同一轮；增量路径又只登记 CATA 文件身份、默认不执行整库基线。结果是已有库启动、
多 worker 或同轮多文件变化时，DESI 可能在新的 SYS/DICT 或 CATA 数据之前执行，启动
全量模型生成也可能越过尚未消费的数据批次。

## 决策

1. 所有启动初始化、回退重建、手动执行与同一观察轮的稳态更新共用阶段顺序：
   `Meta → Catalogue → Design → Model → Room`。
2. `Meta` 包含主项目 SYST/GLB/GLOB 及 included_projects 的 DICT；`Catalogue` 包含
   included_projects 的全部有效 CATA；`Design` 只包含当前 MDB 声明的 DESI。
3. 完整候选扫描先生成不可变 manifest，再原子安装到唯一的 `BatchScheduler`。派发器只
   冻结当前阶段，阶段内保留 FIFO；Meta、Catalogue 与基线/重建独占，只有稳态 Design
   暂存窗口可以并发。
4. 阶段转换前重扫。早期阶段出现新目标或 blocker 时回退阶段并关闭模型门；旧 epoch 的
   完成不得满足新 manifest。
5. 阶段状态不另建数据库表。重启从文件、水位及持久模型工作单重建；启动默认
   `Discovering`，在完整重扫确认前模型门关闭。
6. DICT/CATA 跨项目裸 dbnum 冲突按项目顺序选主：`catalogue_project_priority` 点名的排在
   最前，其余按 `included_projects` 的书写顺序接着排。显式名单是覆盖层不是全部顺序——
   没点到名是「没意见」，不是「排不出主」。同项目重复、显式名单含未知或重复项目、
   头部不可读、目录不可达或 Ref0 归属冲突均阻断所在阶段。
7. 数据批次只提交数据、水位与模型意图。模型 drain、显式全量生成工具及会产生新模型的
   按需请求仅在 `data_ready` 后执行；房间保持在模型之后。根据后续 ADR-051，服务启动
   不再仅因 `gen_model` / `gen_mesh` 开启而隐式执行全量生成。
8. 模型 drain 是阶段控制器的正式消费者而不是无条件空闲任务。每页可预取多个根，但必须
   逐根执行，并在认领前、每根前后重新检查当前 epoch、模型门及数据队列。新数据到达时，
   当前单根完成后立即让位；未执行的持久工作不记失败、不增加 attempts。
9. `model_update_pending` 记录一次消费尝试的 epoch、来源会话、根和结果。根据后续
   ADR-050，它只属于当前进程，重启后的工作真值由文件会话号与 `applied_sesno` 重新比对。

## 后果

- 全量 CATA 初始化取代“只登记、生成时才按需补齐”作为初始化主路径；按需 closure 继续
  作为漏边兜底。
- 队列全局 FIFO 改为“当前阶段内 FIFO”，这是 ADR-011 的明确例外；仍只有一个队列和一个
  派发器。
- 稳态数据与模型不再在每个批次内同步完成，模型工作以 durable pending 跨越阶段屏障；
  `applied_sesno` 仍只承诺数据已落库，不承诺模型已生成。
- 阶段状态可重建而非持久化，避免出现第二份与水位竞争的完成真值。
- 初始化门关闭属于调度让位而非模型失败；只有实际解析、生成或持久化错误才进入重试账。

## 否决方案

- 只按文件类型排序：运行中追加、失败重试及多 worker 会再次打乱顺序。
- 让 CATA 继续只做 locator 登记：无法兑现完整 CATA 数据先于 DESI。
- 将状态表改成 `(project, dbnum)`：改动水位、队列、PE 聚合和清库语义，本期用显式优先级
  解决跨项目同号。

## 2026-08-19 Oracle 审核修订

manifest/epoch 安装与任务冻结共用 activation gate；锁序固定为 activation gate → scheduler queue → coordinator，下一 epoch 不得越过旧 epoch 的冻结边界。

## 2026-08-25 实施说明：模型完整性与有界根级流水

- 模型阶段打开前先调用既有 `fn::sync_gen_roots` 物化当前生成根；成功凭证同时记录
  `source_end_sesno` 与 `source_end_sesno_time`，只有与当前数据水位快照一致才满足模型门。
- 数据库认领页保持 100，但根锁和不可抢占实例生成按 execution group 获取；组间重新检查
  epoch、数据队列和模型门。组内 Shape writer 单路，后半程根并发复用全局 geometry gate，
  AABB 仍由 `SPATIAL_STATE_SERIAL` 单路发布。
- 指定 dbnum 重建端点只强制重排权威根到同一 `model_update_pending` 消费器；不增加第二条
  模型消费路径，不删除旧显示，不改变数据水位，进程重启后不自动恢复人工重建任务。
