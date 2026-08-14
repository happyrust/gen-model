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
6. DICT/CATA 跨项目裸 dbnum 冲突由 `catalogue_project_priority` 显式选主；同项目重复、
   无优先级裁决、头部不可读、目录不可达或 Ref0 归属冲突均阻断所在阶段。
7. 数据批次只提交数据、水位与持久模型意图。模型 drain、启动全量生成及会产生新模型的
   按需请求仅在 `data_ready` 后执行；房间保持在模型之后。

## 后果

- 全量 CATA 初始化取代“只登记、生成时才按需补齐”作为初始化主路径；按需 closure 继续
  作为漏边兜底。
- 队列全局 FIFO 改为“当前阶段内 FIFO”，这是 ADR-011 的明确例外；仍只有一个队列和一个
  派发器。
- 稳态数据与模型不再在每个批次内同步完成，模型工作以 durable pending 跨越阶段屏障；
  `applied_sesno` 仍只承诺数据已落库，不承诺模型已生成。
- 阶段状态可重建而非持久化，避免出现第二份与水位竞争的完成真值。

## 否决方案

- 只按文件类型排序：运行中追加、失败重试及多 worker 会再次打乱顺序。
- 让 CATA 继续只做 locator 登记：无法兑现完整 CATA 数据先于 DESI。
- 将状态表改成 `(project, dbnum)`：改动水位、队列、PE 聚合和清库语义，本期用显式优先级
  解决跨项目同号。

