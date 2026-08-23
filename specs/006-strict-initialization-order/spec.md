# Feature Specification：严格分阶段数据初始化

## User Stories

### US1：确定性初始化

服务初始化或重建时，SYS/DICT 必须全部收口后才处理 CATA，CATA 全部收口后才处理 DESI，
全部数据就绪后才生成模型。

### US2：失败可见且不越级

任一阶段的文件身份、扫描、解析或提交失败必须出现在回执与健康状态中，并阻止后续阶段，
不得因队列暂时为空而误判完成。

### US3：崩溃后可恢复

进程在任一阶段退出后，重启必须从文件、水位和持久工作单重建 manifest，在确认数据就绪
前不得消费模型工作。

## Functional Requirements

- **FR-001**：阶段固定为 Meta、Catalogue、Design、Model，房间后处理位于 Model 之后。
- **FR-002**：完整候选扫描必须先于 observation、入队和阶段派发；手动与自动路径共用。
- **FR-003**：派发器只冻结当前阶段；仅同阶段稳态 Design 窗口允许并发。
- **FR-004**：阶段转换前必须重扫；旧 epoch 完成不得满足新 manifest。
- **FR-005**：Meta 包含主项目 SYST/GLB/GLOB 及 included_projects DICT；Catalogue
  包含所有经优先级裁决的 CATA；Design 为当前 MDB 的 DESI。
- **FR-006**：跨项目 DICT/CATA 同 dbnum 按项目顺序选主——`catalogue_project_priority`
  点名的在前，其余按 `included_projects` 书写顺序；被遮蔽候选不得写观察、水位或队列。
- **FR-007**：同项目重复、显式名单含未知/重复项目、目录/头部不可读及 Ref0 冲突必须阻断
  阶段并公开原因；名单里没点到某个 `included_projects` 项目不算阻断成因。
- **FR-008**：`startup_autorun=false` 时 manifest 整体等待；一次真实触发释放所有前置阶段。
- **FR-009**：数据未就绪时模型 pending 不消费、不增加 attempts；需要生成的按需请求返回
  `initialization_not_ready`。
- **FR-010**：全量同步使用全局 Meta→Catalogue→Design 三段 await，不再混合 DESI/CATA。
- **FR-011**：模型 drain 必须在每个生成根前后检查数据阶段；新 epoch 或新数据批次到达时，
  当前单根结束后让位，未执行工作保持原状态与 attempts。
- **FR-012**：模型消费必须以独立 `model_drain` 任务公开来源 dbnum、来源会话、根、revision、
  认领 epoch 和让位/失败结果；数据任务不承担模型完成证明。

## Success Criteria

- 任意文件遍历顺序、多 worker 数和同轮事件排列下，首个模型写发生在最后一个 Design
  水位提交之后。
- 阶段失败时后续阶段无数据写、无水位推进、无模型生成。
- CATA 初始化结束时每个选中库都有数据支撑或合法空基线凭据。
- 重启后阶段可从持久事实恢复，遗留模型工作不会提前执行。
- 1608 条模型积压存在时，新数据最多等待当前单根生成；阶段让位不产生假死信。

## Assumptions

- 已有数据支撑的追平文件算阶段满足；上游单独变化不强制重建所有追平下游文件。
- 跨项目同号选一份，不迁移裸 dbnum 身份。
- 阶段状态为可重建进程状态，不新增 SurrealDB 表。
