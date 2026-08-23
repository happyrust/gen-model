# ADR-039：`dbnum_info_table.count` 的语义冲突与统一口径

## 状态

提议（2026-08-20）。本 ADR 只钉定义与验收口径，代码改动未落地。
关联 ADR-001（`applied_sesno` 是水位唯一权威）、ADR-021（水位必须有数据支撑）、
ADR-023（启动重扫修复无数据支撑的应用水位）、ADR-037（dabacon 快照完整性）。

## 背景

`dbnum_info_table.count` 现在有两个写入者，它们对这个数的定义不同。

事件侧：`pe` 表上的 `update_dbnum_event`（`versioned_db::database::dbnum_event_sql`）
按事件增量维护。`$event = CREATE` 加一；`$event = DELETE OR $is_delete` 减一，其中
`$is_delete = $value.deleted AND $event = UPDATE`。也就是**软删也减**——pe 行仍留在表里，
count 已经少一。它维护的是「活着的元素数」。

重算侧：`rebuild_dbnum_info_from_pe` 走 `pe_stat_groups_sql`，语句是
`SELECT … count() … FROM pe WHERE dbnum = N GROUP BY ref0`，**不过滤 `deleted`**。
它写回的是「pe 总行数（含墓碑）」。

三处对账都拿重算侧的口径去校验事件侧维护出来的值：

- `versioned_db::database::classify_stats_settlement`：`pe_count == info_count`
  才只补身份字段，否则全量重算；
- `manual_update::baseline_stats_need_rebuild`；
- `dbnum_state::seed_suspect_dbnums`（一次性水位播种时判「这个库不给播水位」）。

事件本身还有两个不对称，使漂移单调累积：

- 减法不幂等。已经 `deleted = true` 的行再被 UPDATE（后续窗口重写属性、重新标删），
  `$is_delete` 再次成立，count 再减一。
- 撤销软删（`deleted` 由 true 改回 false）落到只更新 sesno 的第三个分支，count 不加回来。

现场实测（2026-08-20 03:52，命名空间 `1516` / 库 `AvevaMarineSample`，dbnum 8000）：
`ref_0 = 24384` 有 6603 条 pe 行、其中墓碑 14 条，而 `dbnum_info_table:24384.count = 6585`。
`6603 − 14 = 6589`，仍差 4；同一个 dbnum 的另外两个 Ref0（16192、32576）分毫不差。
该差额在约半分钟一次、连采七次（03:49:35 – 03:52:46）里稳定不变，与在飞窗口无关。
至少一次往返可追溯：邻仓 plant-ui 的
`../plant-ui/artifacts/invalid-tubi-dashes-20260819/scenario-invalid-tubi.ps1`
（2026-08-19 无效 TUBI 虚线验收）把 `pe:24384_26236` 置 `deleted = true` 跑验收、
再 `-Revert` 置回 `false`。

## 决策（目标不变量，实现待排）

1. `dbnum_info_table.count` 的唯一定义是 **pe 行数（含软删墓碑）**，与
   `SELECT count() FROM pe WHERE dbnum = N` 同口径。理由：三处对账、播种迁移与基线
   完整性校验全部按行数写成，而「活元素数」在本库里没有第二个消费者；统一到行数
   只需要改事件一处。
2. 事件的减法只在**硬删**（`$event = DELETE`）时触发。软删是 UPDATE，只更新 sesno，
   不动 count；`$is_delete` 随之退役。
3. 若第 1 条因为出现了「活元素数」的真实消费者而被否，退路是反向统一：
   `pe_stat_groups_sql` 与三处对账全部加 `deleted != true`，同时事件的减法改成只在
   `$before.deleted != true AND $after.deleted = true` 时触发一次，撤销软删必须加回来。
   两条路二选一，不允许并存。
4. 在口径统一落地之前，**禁止把 `pe_count != info_count` 接成自动修复触发器**。
   `rebuild_dbnum_info_from_pe` 是服务端逐行 `string::split`，ams7351（3,345,853 行）
   实测 861 秒；在语义冲突下这个条件对任何有过软删的库恒真，接上去就是周期性的
   百秒级 CPU 燃烧且永不收敛。
5. 同理，口径统一前 `pe_count != info_count` 不得作为「这个库有数据洞」对外播报，
   包括 `seed_integrity_warnings` 的措辞。
6. 验收：口径统一后，对一个做过「软删 → 撤销 → 再软删」的 dbnum，事件维护值与
   `pe_stat_groups_sql` 重算值必须逐 Ref0 相等。该断言落成单测，不靠现场对账。

## 后果

- 在修好之前，`classify_stats_settlement` 对任何有墓碑的库都判 Rebuild，
  `settle_dbnum_info_after_total_sync` 的「跳过全量重算」优化事实上失效。
  这是已知代价，不是新缺陷。
- `seed_suspect_dbnums` 当前的「可疑」名单里混着只是有墓碑的正常库。水位播种已在
  2026-08-09 完成（`queue_control:watermark_seed`，354 个库，跳过 1030/1054/1103/1112/7997/7999），
  因此这条对存量库不再生效；只有清掉播种标记或接入新库时才会再暴露。
- 漂移本身不影响增量正确性：按 ADR-001，`applied_sesno` 是水位唯一权威，
  `dbnum_info_table` 的 max sesno 只在**没有专用水位行**时才被读侧迁移取用，而 sesno
  的维护（单调取 max）没有这个缺陷。真正的损失是**这条对账信号本该在
  `dbnum_watermark:N` 掉行时兜住「库里有洞却被判已追平」，现在因为对有墓碑的库恒真、
  恒报红而失去分辨力**。
- 口径选定之前不写 changelog、不动代码；本 ADR 是唯一的记账面。
