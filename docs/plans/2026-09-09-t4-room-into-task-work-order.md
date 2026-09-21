# T4（S8 房间内联）开工单：房间搬进 dbnum Task

- 日期：2026-09-09（会话 fable-5-37 接手 fable-5-30 审核线开出）
- 性质：**执行工单，不再设计**。设计已全部拍板：重构计划 §4 S8 行（Q1–Q23，07 计划 §8 第 6/7/8 轮）、
  ADR-065 D1/G2 (i)/D3/D4/D7/D8、ADR-067 D1–D9 + 实施约束、共识 d-21/d-44/d-73/d-87/d-89。
  本文只把那一段落拆成**每片全量测试收口**的切片序列，并把验收义务与协调前提钉在片上。
- 审核基准：`docs/2026-09-09-room-increment-audit-v2.md`（H1/M1/M2/L1/L2/W1/W2）。
- completion 计划 S4 与本单**同一笔**（07 计划第 8 轮 Q23）；本单即它的执行形。

## 0. 一句话

房间工作从 drain 宿主（`model_update_pending.rs::drain_rooms_selected`）搬进 dbnum Task 收尾
（新文件 `src/fast_model/room_task.rs`），恢复 ADR-065 D1 顺序（数据 → 模型 → **房间** → 凭证前移 →
`advance_model_watermark`），常驻房间模型（换代点 / 写点 / 护栏）随迁；发布视图三表与文件轮次屏障
随后退役。T5（pending 表退役）**必须排在本单全部落地之后**（d-73）。

## 1. 切片序列（每片：改什么 → 收口）

### T4.0　GroupCache 跨代命中即提升（Q16）——**已落（2026-09-09 本会话）**

- `room_topology.rs`：代内条目带上本文件冻结令牌；`cached_groups` 未中时从新到旧在别的代里找
  **同一份文件**（dbnum / sesno / 令牌逐字相同、层级同口径），命中就提进当前 composite 的那一代。
  写入收口到唯一入口 `insert_groups`（remember 与提升共用，淘汰上限只守一次）。
- 效果：邻库前进后没变的文件**连磁盘工件都不读**；连续两次换代命中不丢（ADR-067 实施约束 3）。
  同号换文件（令牌不同）与层级不同一律不借——与 d-89 磁盘键同判。
- 钉：`unchanged_files_are_promoted_across_generations_and_survive_two_swaps`（行为）+
  `the_group_cache_has_an_eviction_bound`（扩：写入路径唯一、淘汰在唯一入口）。
- 收口：定向 `room_topology::` 10 passed；全量见 changelog 当日条目。

### T4.1　模块搬家：`room_task.rs` 承接 Task 房间收尾（行为不变）

- 把 `batch_worker.rs` 里 Task 房间收尾一段（`finish_file_room_round` 及其直接帮手）原样搬进新文件
  `src/fast_model/room_task.rs`，`batch_worker.rs` 只留一行调用（S8 行明文）。**不改一处行为**：
  顺序、回执、日志逐字不动，现有顺序钉原样跟着搬。
- 前提：**§3 协调前提先清**——`batch_worker.rs` 的 Task 收尾重排是别的会话未提交改动，搬家等于
  挪它的在途代码，动手前须确认该会话已收笔（或由用户裁移交）。
- 收口：全量不降；`git diff` 逐段核对「只挪不改」。

### T4.2　D1 顺序恢复 + 房间失败扣根（d-73 义务在此结清）

- `room_task.rs`：房间阶段移到 `complete_model_task_on`（model_sesno 推进）**之前**；房间失败 =
  **牵涉的根**不安定——凭证不前移、`model_sesno` 不越过（G2 (i)）；结构触发面板重算失败**扣面板
  所属根**（Q6，元数据门「凭证前移不少收」的唯一例外）。
- **反向重钉**：现有顺序钉断言的恰是过渡顺序（publish < complete < rooms），必须改成断言
  rooms 先于 complete；补 mem:// 真测「房间失败挡住牵涉根凭证前移、水位不越过」（ADR-065 实施约束 2）。
- **顺带欠账（2026-09-09，会话 fable-5-47 代记）**：启动房间段重排后 `lib.rs` 三条启动钉
  （`the_startup_path_asks_the_gate_before_rebuilding_rooms`、
  `startup_reconciles_models_before_waiting_for_data_and_keeps_rooms_last`、
  `the_degraded_data_wait_skips_model_wait_and_rooms_and_is_named_on_the_banner`）的标记已从
  `run_cli` 撤走、在 HEAD 上是红的，现挂 `#[ignore]`（理由指回本条与 mem-107）——本片反向重钉时
  按新启动形状一并重钉或删除，别让 ignore 长住。
- 过渡兜底（durable 行 / `room_released` / 空闲轮恢复重放）**本片不动**——它们到 T5 才退。
- 收口：全量不降 + 上述 mem:// 真测绿。

### T4.3　消费搬家：就地重算与写边进 Task

- 本 Task 触到的面板 / 构件就地重算与写边，读**实时几何**；几何驻留由本 Task 就地替换
  （d-21：本库在册面板整段重查、锁内合代原子交换，其它库继承）——**顺带消灭 mem-48 点名的
  T4 前最尖正确性缺口**：几何变了而换代键不变（ModelOnly 补差 / 按需生成）会拿旧驻留几何写边（Q21）。
- 换代点拆分 `ensure_at(dbnum, T)` / `current()`（ADR-065 D3）：只在初始化探文件，Task 内不探（Q17，
  常驻模型建不起来时本 Task 触到的根全部扣住）；换代仍在 `lock_spatial_serial` 下，锁外备料锁内换代。
- 写点 ③ → ③'：进锁读-改-写、只前移**本库**令牌（Q18）；②' 范围 = MDB 全部 DESI（Q19）。
- 护栏 `observe_generation_swap` 搬进 Task 房间收尾，`panel_page_full` 臂删（审核 L2）；
  启动对比不再探文件最新。
- 收口：全量不降 + mem:// 一次真跑；warm 阻代口径不得回退（d-87 钉在树上）。

### T4.4　Task 回执房间段定形（消费者对照）

- 按 `docs/plans/2026-09-09-s8-task-receipt-room-fields-consumer-input.md`（plant-ui 送来的输入）定形：
  `rooms.panels_recalculated / elements_recalculated`、`touched[]`、`moves[]`（reinitialize 只给计数）、
  `failed[] = {target, message, involved_roots}`（G2 (i) 点名，界面拿它拼 `POST /model/ensure`）、
  `metadata_only_roots`。字段名 / 形状由本片裁，**裁完差异回写那份文档**，plant-ui 照裁定改 V3。
- 段与字段可增量给（界面侧缺席语义已兜好）；先计数后明细是被允许的落地次序。
- 收口：全量不降 + 回执样例进 evidence。

### T4.5　旧形态退役（仍在 T4 内，T5 之前）

- 退：`drain_rooms_selected` 宿主、`ResidentForm::Published`、`query_room_insts` 的 Published 臂、
  初始化摄入臂（`plan_manifest_advance` / `advance_orphaned_manifest_rows` / `RoundOwnership`）、
  `room_published_inst` / `published_file_input` / 文件轮次屏障、M2 (a) `enqueue_room_panel_recalcs`
  入队（进程内闸留）；`room_publication_effects`「产物 → 目标」改内存直交（D7）。
- **不退**：`room_topology_source` 分片（参照要用）；attempts / 死信随 **T5** 退（审核 M2）；
  L1 范围谓词随写点 ③' 按库前进自然消解，届时核一遍再删。
- 收口：全量不降；F-C 残留（`updating` 按 dbnum 收窄）与 F-H 口径在本片自然合一，回填审核表。

### T4.6　验收门（过了才准开 T5）

- live #2：八场景 + §9 逐边对拍（房间边逐边 == `build_room_relations`），`room_resident_model`
  开关两态各一遍；**直接按实时几何内联口径建门，F-F 的发布路径版不补**（审核 §5.3）。
- mem:// 故障注入按 completion 计划「故障注入」清单。
- 背靠背两窗 + 冷启动零写入两条（重构计划 §5 live #2 增补）。

## 2. 硬约束（改动前先对表）

| 共识 | 内容 | 挡什么 |
|---|---|---|
| d-73 | T5 绝不能先于 T4 落地；T4 携带反向重钉 + mem:// 真测 | 先删表把房间欠账变隐形 |
| d-87 | warm 阻代只收 unusable / mesh-warm，「从未生成」只留痕 | completion S2 把严格门改回「阻代拒算」 |
| d-89 | 磁盘工件按本文件令牌寻址；不另算摘要；T4 换 ensure_at 形态后仍按本条 | 工件键把 composite 编回去 |
| d-21 | 几何驻留、Task 就地替换，不做每次收尾重读 | 退回「每次重读实时行」 |
| d-44 | ADR-067 D1–D9 是 T4 搬家的执行依据 | 重开已拍板的设计题 |

## 3. 协调前提（共享树，动手前逐条清）

1. `batch_worker.rs` / `model_update_pending.rs` / `room_resident.rs` / `room_startup.rs` 都有**别的
   会话未提交改动**（Task 收尾重排、completion S1 生命周期等，src/ 约 1.9 万行在途）。T4.1 起
   直接重排这些区域——**须用户确认在途会话已收笔或明确移交**；拿不准就按 d-91 先例单独拉
   worktree 做，收尾再并（由用户裁）。
2. S5a（`model_update_pending.rs` 非 pending 部分的模块搬家）**未做**：不挡 T4.1–T4.4，但 T4.5
   退 drain 宿主时建议先做 S5a，否则「退宿主」与「搬水位」搅在同一笔里（S5a 本意就是消这个歧义）。
3. 每片收口跑全量前先看有没有别的会话在跑测试（LNK1104 锁 exe，共享 target）。

## 4. 每片通用收口

`cargo test --lib -- --skip damaged_existing_content_addressed_file_is_rejected` 不降；rustfmt 在本片
改动行上干净（别的会话在途区域的漂移不动）；changelog 一条；T4.2 / T4.3 另加 mem:// 真跑；
改回执的片把样例落 evidence。审核表（v2 §2）在 T4.5 后回填一轮。
