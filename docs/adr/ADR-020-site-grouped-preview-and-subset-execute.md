# ADR-020: SITE 分组预览、勾选子集执行与会话时间戳

> **状态：已实现（2026-08-07，画板验收通过当日）。** 由 plant-ui ADR-0017（向导预览重构为
> SITE 视图）驱动的三项契约新增。落点：`manual_update.rs`（`SiteSummary` +
> `build_site_rollup` 与 ZONE 同引擎、`session_time_rfc3339`、`subset_selects`）、
> `handlers.rs`（`ExecuteReq.dbnums`）、`batch_scheduler.rs`（回执 `unselected`）；
> `docs/specs/web-service-api.md` §4.2/§4.3 与 `docs/specs/manual-model-update.md`
> §预览结构/§确认与合并已同步。

## 背景

plant-ui 把手动更新向导的预览树重构成「库分组头 → SITE 子行 → 交付单元」，库行带勾选框，
让人选择本次执行哪些库（对着 SITE 说话，而不是文件名）。执行边界**不变**：数据应用的最小
单位仍是（dbnum, 会话号区间），水位仍是库级单调值。SITE 是选取入口与报告口径，
不是执行范围——这条铁律出自 plant-ui ADR-0015 的查证，本文所有设计都压在它上面。

## 决策

### 1. `DbnumPreview.sites: Vec<SiteSummary>`

按**最近 SITE 祖先**给净变化分桶，`ZoneSummary`（ADR 前身，界面已退役但契约保留）同款做法：
pre/post 两张所有权快照上各解一次，挪动的单元可以出现在两个桶里。

```text
SiteSummary {
    site_refno: String,   // 空串 = 「SITE 归属未知」桶（解析不出 SITE 祖先的变化）
    name: String,
    added / modified / deleted: u32,
    moved_in / moved_out: u32,
    model_affecting: u32,
    units: Vec<DeliveryUnitSummary>,   // 原形态照旧，只是挂在 SITE 桶下
}
```

- 所有权链不跨库，所以本库窗口内变更元素的 SITE 祖先必然在本库——分桶不会把别的库的
  SITE 引进来。**例外是级联单元**（`cascaded`：变更在别的库、反向引用逼着它重生成）：
  它仍挂在**触发批次**的 `DbnumPreview` 下，按它自己的 SITE 祖先入桶，消费方要知道
  「SITE 桶是报告口径，选择的是批次」。
- `#[serde(default)]`，旧消费者不受影响。

### 2. `DbnumPreview.applied_sesno_time` / `file_latest_sesno_time`

两个 `Option<String>`（RFC3339）。语义是**那个会话在 E3D 里被写入的时刻**（会话页自带的
年/月/时/秒，`SessionPageData::get_dt`），不是我们应用它的挂钟时刻——两个时间同一把尺子，
相减直接回答「文件里还有多大时间跨度的设计改动没被吸收」。

- `file_latest_sesno_time`：`DbPageBasicInfo.latest_ses_data` 现成就有，零额外 IO。
- `applied_sesno_time`：`get_ses_data(applied_sesno)` 读一页会话页。
- `applied_sesno == 0`（需初始化）或会话页读不到（文件被截断等）→ `None`，
  界面文案「从未应用」。
- 逐会话的 `SessionPreview.date` **本次不加**：预览扫描本来就逐页解析待应用窗口的会话页，
  日期几乎免费，但界面还没有「按会话展开」的画板——等有消费者再进契约（字段图 §8 惯例）。

### 3. `POST /api/v1/update/execute` 加可选 `dbnums: [u32]`

- 兑现 plant-ui ADR-0015「Consequences」第 1 条（该 ADR 其余部分不动）。
- 语义是**范围内的子集选择**：每个 dbnum 先过 `UpdateScope::admits`（in_scope），
  不在当前 MDB 声明名单里的直接拒——不给绕过 ADR-0013 统一范围门的第二条路。
- 缺省（不带字段）= 全范围，行为与今天完全一致。
- 过滤作用于**执行时的重扫循环**：未勾选的库不扫描、不入队、水位不动，预览之后新产生
  的会话也不会被偷偷并入。
- 202 入队回执、排队中合并、运行中冻结（ADR-011）全部照旧——过滤只决定谁入队。

### 4. 明确不做

- **worklist 优先根排序提示**（plant-ui ADR-0015 后果第 2 条）：依赖 G5/G6 claim/lease
  未验收地基，且与本次诉求无关，照旧欠着。
- **预览请求不变**：预览永远全范围扫描。勾选是 execute 的事——预览是看清全局的地方，
  不许让人对着收窄的视图以为看到了全部。

## Consequences

- `preview_dbnum` 的分桶逻辑在既有 pre/post 所有权快照上加一遍 SITE 解析，与
  `build_unit_rollup` / zones 分桶共享缓存，成本是每个变更 refno 多走几步 owner 链。
- 三个新字段全部 `serde(default)` / `Option`，HTTP 层向后兼容；`dbnums` 是请求体新可选
  字段，老客户端不带即旧行为。
- 消费方唯一是 plant-ui 向导（S2-G / S2-H 画板，验收基准
  `plant-ui/design/MODEL-UPDATE-FIELD-MAP.md` §2-G）。
- `ZoneSummary` 保留不动，`sites[]` 与 `zones[]` 并存——前者是界面在用的报告口径，
  后者是契约兼容负担，哪天真要删走自己的 ADR。
