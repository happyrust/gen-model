# S8 / T4 Task 回执房间段 · 消费者侧字段输入（plant-ui 终态明细）

- 日期：2026-09-09（plant-ui 会话 fable-5-29 送来，用户指示提交）
- 来源：plant-ui 仓 `docs/plans/2026-09-09-task-finished-detail-plan.md` §3（消费者清单原文）与
  §4 V-D3 / V-D4（房间段形态与列表上限裁决）；界面稿画板「段形态表」；数据段 V1 / V2 已落地并
  sim 目检过（plant-ui `docs/evidence/2026-09-09-task-finished-detail-sim-inspection.md`）。
- 性质：**输入，不是要求**。plant-ui 的纪律是段级缺席整段不画（两代并存、与其欠账段三态同款），
  这些字段不来界面不崩；本文供 S8 定形 Task 回执时当消费者对照表，字段名 / 形状 / 挂点由 S8 裁，
  裁完有差异请回写一句到本文，plant-ui 侧照裁定改 V3。

## 一、为什么现在送来

S8（T4 房间内联，U5 / N-D：房间随 dbnum Task 收尾）落地时，Task 终态 `result` 第一次有机会带
房间段。plant-ui 终态行内明细四段里唯独房间段无契约可画（其 V3 / V4 押在这里）；趁回执定形前把
消费点摆出来，免得定形后两边返工。挂点建议：**Task 终态 `result` 里与批次结果平级**（房间工作
已随 U5 归属本 Task，不再是独立流水）。

## 二、字段清单（plant-ui 计划 §3 照录，附界面消费点）

| 建议字段 | 语义 | 界面消费点 | 备注 |
|---|---|---|---|
| `rooms.panels_recalculated` / `elements_recalculated` | 本 Task 就地重算了几块面板、几个构件（`RoomRecalcPanel / Element` 口径） | 房间段段头「重算 2 块面板 + 27 个构件」 | 正是 S8 新家 `room_task.rs` 收尾时手里就有的数 |
| `rooms.touched[] = {room, moved_in, moved_out, recalculated}` | 受影响房间与迁入 / 迁出计数 | 房间行组；超 10 间收成「N 间 · 展开」（V-D4） | — |
| `rooms.moves[] = {refno, noun, name, from_room?, to_room?}` | 搬间明细；`from` 空 = 入屋、`to` 空 = 出屋 | 搬间明细行，列前 10 件 + 「展开全部 N 件」（V-D4） | `intent = reinitialize` 的整库重建建议只给计数不给明细（明细可能巨大），随定形一并裁 |
| `rooms.failed[] = {target, message, involved_roots}` | 失败留痕；`involved_roots` = 凭证被卡住的根 | 失败框：G2 (i) 后果句（牵涉根凭证不前移、模型水位卡住、库不判一致）+ 两个命令面按钮——「重算这几根」打 `POST /model/ensure`、「再执行一次这个库」= execute 单库 `dbnums[]`；N-C 明文不给队列重试 | `involved_roots` 正是 G2 (i) 判据的点名，界面拿它拼 `ensure` 的目标集 |
| `metadata_only_roots: u32` | 元数据门省下的根数 | 模型段「省力注」那半句 | 规划器侧元数据门已落（`0afe581be`）；plant-ui 按其 ADR-0019 先例**不解析 `warnings[]` 文本反拼数字**，只认结构化字段 |

## 三、缺席语义（界面侧已经兜好，S8 可以分批给）

- `rooms` 整段缺席 → 行上四段退三段、明细里房间段不出现——不猜、不冒充「无变更」。
- 段内单字段缺席 → 对应格不画。先例：欠账段三态（`pending_endpoint_retired`）、数据段分解缺席
  降级句（V1，sim 目检里 db7006 走的就是这条）。
- 所以回执可以增量定形：先给 `panels / elements` 计数，`touched / moves / failed` 后补，
  界面每一格独立降级、不互相牵连。

## 四、消费者侧现成的验收（S8 落地后即可对拍）

plant-ui 计划 §六.3：房间段四形态（有变更 / 无工作 / 失败 G2 / 字段缺席）各验一例——搬间明细与
`room_relate` 边的实际变化逐条一致；失败例中该库不判一致、行上水位段不亮、两个按钮各自打对
端点。plant-ui 侧渲染测试（四形态 + 上限）排在其 V3 / V4，契约一定形即可开工。
