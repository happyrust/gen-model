# 交付单元删除 / 改位号的中台删除记录（`datacenter_delete`）· 开发计划

> 来源：plant-ui issue [#1「增加删除标记」](https://github.com/MUDS7/plant-ui/issues/1)（2026-09-18）：最小交付单元被删除、或其 `NAME`（位号）被修改时，
> 把**修改前的那一份**记进库，提资时一并交给中台去删；改后的新数据照常提资。
> 上位：本仓「数据窗口直写」随窗语句批（ADR-056 实施约束 1、`increment_tail::FinalizeRender`）、`datacenter_version` 状态维护
> （`increment_pipeline::resolve_datacenter_statements_with`）；rs-server `datacenter/`（提资端，`get_datacenter_delete_data`）。
> 基线：本仓 `main@bb3274e63`（2026-09-22）；rs-server `7a6cb6f`；plant-ui `137f34560`（rs-core `data_center.rs`）。
> 状态：**T1–T5 已落地**（T1 / T2 2026-09-22 会话 fable-5-1-43，用户「计划通过，从 T1 开工」；T3 验证 + T4 / T5 2026-09-23 会话 fable-5-1-7）。
> T1 / T2：`PeChainView` 换掉 `(noun, owner)` 二元组、`load_pe_chain_view_from_persistent` 多选 `name`、`resolve_datacenter_statements_with`
> 两个分支 + `nearest_zone`、`render_datacenter_delete_insert_ignore`、调用点多传 `dbnum` / `end_sesno_time`。T3：白名单两条「应接受」、
> `INST_RELATE_INDEX_SQL` 建表建索引、fork 双跑 `dual_datacenter_delete_refno_ids_insert_ignore`。T4：`fast_delete` 源码钉
> `the_wipe_never_touches_the_delivery_ledgers`（`datacenter_version` 一并钉住）、预览回执 `DbnumPreview.datacenter_delete_rows`
> （`IncrementPipeline::count_datacenter_delete_records` 走执行体同一条渲染路，只计数）。T5：`CONTEXT.md` 词条「交付删除记录」、changelog。
> 验证（2026-09-23，e3d-io `TextList` 编译期阻塞已随 `direct_attmap.rs` / `element_attributes.rs` 补分支解除）：`cargo check --lib` 绿；
> `cargo test --lib -- datacenter_delete datacenter_tests accepts_the_persist_path_statement_shapes the_wipe_never_touches the_preview_count`
> **21 绿**——fork 双跑首跑红：`SELECT VALUE count() … GROUP ALL` 在 2.1.4 上回的是 `{ count: n }` 对象、不是裸整数，探针改按 id 第二位
> （老位号）数行后绿；全量 `cargo test --lib` **2433 绿 / 0 红 / 128 ignored**；`rustfmt --check` 改到的每一段干净（同文件 HEAD 上就有的
> 5 处旧 diff 不动）。**跨仓 §6（rs-server 消费端）未开工**；Q1 `HANG` 待确认。
> 追记（2026-09-23，同会话）：对外**只读**口 `GET /api/v1/datacenter/deletes`（spec §4.14，`data_interface::datacenter_delete_reader`）——用户拍板
> 放 gen-model、只读、不带 `instance_code`、鉴权同 `/api/v1`；每行附 `published`（join `datacenter_version`）。D7 不变：删行仍归 rs-server。
> 追记（2026-09-24，审查）：§6「改回原名」判据更正为比**当前 `NAME`** 推出的 `instance_code`（原写法比 `datacenter_version` 行上的，会吞掉普通改名），
> 并补「查无此实例按成功」；读口 `since_sesno` 须配 `dbnum`（缺则 400）、行序补 `id` 成全序、spec §4.14 写明边删边拉始终 `start=0`。
> 追记（2026-09-24）：**Q1 已决**——用户拍板 `HANG` 算提资交付单元、按配置全收（D8 / §7 已改；现行代码即如此，不动）。
> 追记（2026-09-24）：**Q2 已决**——消费端取数分三路：改名跟 refno、删除按 ZONE、孤儿按 dbnum 兜底进提资预览要确认（§6 rs-server / plant-ui 两条与 §7 已改；本仓不动）。
> 追记（2026-09-24）：读口加 `from` / `to` 按 `sesno_time` 查时间范围（半开区间、按真实时刻比、纯日期按北京时间零点，spec §4.14）；只用于查询，增量消费仍用 `dbnum + since_sesno`。
> 追记（2026-09-28，用户重新拍板）：`datacenter_delete` 是“发生过”事件表，ID **只用 refno 值**，不含 `pe` / `name`；写入改为
> `INSERT IGNORE`，同一 refno 首次事件赢；按最新 issue，写前还须确认同 ID 的 `datacenter_version` 已存在。剩余消费与处理交给第三方。
> 此前 D2 的数组主键、UPSERT 和 D6 无条件写方案作废；`rs-server` 不动。
> 追记（2026-09-24）：§4 真机验收用 E3D TTY 夹具套件跑了（`output/e3d-fixture/20260924-123508`，10/10）：改名、改回两窗各一行 `renamed`，删整台 EQUI 一行 `deleted`，
> 其余 8 案一行不落，全表恰好 3 行、`published = false`；kill 重放那一条仍未跑。实现上 `sesno` 取变化所在会话、`sesno_time` 取窗口末端会话的时刻，两者可能不是同一版，待定。
> 追记（2026-09-24）：上一条的「待定」已决（用户拍板）——`sesno_time` 改取变化所在会话、与 `sesno` 同一版的写入时刻（D3 表已改），夹具套件改为逐行对 E3D 会话页核时刻。
> 验证（2026-09-28）：最新契约的 E3D TTY 夹具 10/10 通过（`output/e3d-fixture/20260928-datacenter-delete-refno-event`）。预置两个同 refno
> `datacenter_version` 后，改名 apply 写 1 行、恢复改名写 0 行且首次字段不变，删除交付单元写 1 行，其余 8 案均 0 行；两条事件均 `published = true`。

## 0. 一句话

在 gen-model 增量的**随窗语句批**里（与 `datacenter_version` 的 Modify / Delete 同一批、水位之前、整窗重放收敛），对**交付单元自身**的两种变化各写一条
显式 ID 的 `INSERT IGNORE`：被删 → 记删除前的位号；改位号 → 记改前的位号。表叫 **`datacenter_delete`**，主键只用 **refno 值**。
gen-model 不读事件表消费状态，但写前批量点查 `datacenter_version`，未发布过的 refno 不记录；事件表不清。
本轮不改 rs-server，消费、提交与后续清理由第三方负责。

## 1. 基线盘点（行号对 `main@bb3274e63`）

| 积木 | 位置 | 与本需求的关系 |
| --- | --- | --- |
| 随窗语句批：`window_statements`（`datacenter_version` 状态 + OWNER 搬迁 `anc` 修复），固定目标 UPDATE 纯字串，按 `PERSIST_TX_CHUNK` 包事务，**先于**尾事务执行；任一块失败水位不动、整窗重放 | `src/data_interface/increment_tail.rs` L103–140、L199–210；`window_landing/mod.rs` L122–130；`window_landing/artifact.rs` L57 | 新语句就并进这一批：天然拿到「不得在水位之后丢失」+「幂等重放」两条纪律，不另开写点 |
| `resolve_datacenter_statements_with(data, delivery_unit_types, load_pe)`：`Deleted` → `update datacenter_version:{refno} set status='Delete', belong_zone=…`（BRAN 取 owner.owner，其余取 owner）；`Modified` → 上溯到最近单元层（含 ZONE）→ 归组 `set status='Modify'`；`effective_view` = overlay（本窗净态）优先、持久层窗口前态兜底，走链上限 64 | `src/data_interface/increment_pipeline.rs` L1515–1660 | 两个新分支加在这里；复用 `effective_view` 走链求最近 ZONE |
| `window_net_states(data)` → `HashMap<RefU64, (Option<noun>, Option<owner>)>`；`load_pe_noun_owner_from_persistent` 只查 `id, noun, owner` | `increment_pipeline.rs` L1462–1497、L1664–1697 | 两处都要多带 `name`（删除分支的老位号从窗口前态读） |
| `datacenter_statements` 只对 `db_type == "DESI"` 渲染；`delivery_unit_types = generation_root::configured_delivery_unit_types()`（默认 `BRAN / HANG / SUPPO / EQUI`，项目可配） | `increment_pipeline.rs` L1718–1734 | 同门；「哪些 noun 算交付单元」吃配置，不硬编码 issue 里的 `BRAN / SUPP / EQUI` |
| 窗口折叠：同 refno 多次操作折成一条净变化，`modified_explicit_attrs["NAME"] = (old, new)`（`added` → `modified` 的搬迁也折对） | `increment_pipeline.rs` L240–270；测试 L2676–2712 | 改位号的 (old, new) 现成；房间改名判定已在用同一份（`model_update_plan.rs` L686–695） |
| `normalize_attribute_name`：`att.NAME` / `NAME` 归一 | `src/data_interface/model_impact.rs` L268–276 | 判「是不是 NAME」用它 |
| 重放安全白名单 `replay_safe::validate_statement`：带显式 `id` 的 `INSERT IGNORE` 可重放；拒随机发号、`time::now()`、无界 UPDATE | `src/data_interface/staging/replay_safe.rs` L9、L70–175、测试 L375–461 | 新语句形状 `INSERT IGNORE INTO datacenter_delete { id: datacenter_delete:⟨refno⟩, … }` 要过这道门；**不能**写 `recorded_at = time::now()` |
| 整库清空 `RANGE_TABLES`（`pe / inst_relate / tubi_relate / room_relate / room_panel_relate / ref_rev …`）**不含 `datacenter_version`**——提资状态是 rs-server 的账，回退 / 重初始化不清 | `src/data_interface/fast_delete.rs` L13–30、L594–652 | `datacenter_delete` 同一条纪律：**不进** `RANGE_TABLES`，清库不碰 |
| 墓碑：被删元素的 `pe` 行 `deleted = true`、noun / owner / name 照 S 侧保留；随窗语句在元素批**之后**、但 `datacenter_statements` 的持久层点查在**落库之前**（artifact 先渲后落） | `ele_ops.rs` L1608–1612；`increment_pipeline.rs` L1057–1060 → `land_increment_window` | 删除分支读到的是窗口前态那一行，名字在 |
| rs-server：发布成功 `save_datacenter_record` → `upsert datacenter_version:{refno} set instance_code, status='Insert'`；`instance_code = change_name_into_illegal(NAME)`（去首 `/`、非法字符换 `_`） | rs-server `src/datacenter/query.rs` L564–596 | 本仓只依赖该同 refno 版本行作为“已发布”存在性门；本轮不改 rs-server，事件消费交给第三方 |

**现状结论**：删除那一半今天已经能走（老位号还留在 `datacenter_version` 行上、状态 Delete）；**改位号是真空白**——只标 Modify → 下次按新位号提交 → 中台
旧位号永久残留，且 `save_datacenter_record` 的 UPSERT 会把 `instance_code` 覆盖成新值，旧位号在库里也没了。

## 2. 设计（D1–D9）

### D1　表名：`datacenter_delete`

issue 写的 `datacent_delete` 视作笔误：本仓与 rs-core 的常量都叫 `datacenter_version`（`DATACENTER_VERSION`），新表同前缀。与 `datacenter_version` 同库同命名空间，
SCHEMALESS（`datacenter_version` 今天就是被 rs-server 的 UPSERT 隐式建出来的，不在任何 `.surql` 里）。只加一条索引给消费端的按 ZONE 过滤：

```surql
DEFINE TABLE IF NOT EXISTS datacenter_delete SCHEMALESS;
DEFINE INDEX IF NOT EXISTS idx_datacenter_delete_zone ON TABLE datacenter_delete COLUMNS zone_refno;
```

放在 `pdms_inst.rs` 那组 `DEFINE INDEX IF NOT EXISTS` 旁（启动灌 schema 的同一处），不新开 `.surql`。

### D2　主键：`datacenter_delete:<refno 值>`；首次事件赢

```text
datacenter_delete:⟨7997_1⟩
```

- ID 只取 `RefU64` 的 `{dbnum}_{ref1}` 值，与 `datacenter_version` 的 ID 口径一致；不把 `pe` 记录链接或 `name` 放进 ID。
- 写入使用 `INSERT IGNORE`。同一 refno 重放、再次改名或随后删除都不覆盖、不新增，只保留首次“曾发生过”的事实。
- `refno` 记录链接和首次事件的老位号仍作为普通字段返回，第三方按自身规则处理；本仓不维护消费状态。

### D3　字段

| 字段 | 值 | 备注 |
| --- | --- | --- |
| `refno` | `pe:⟨dbnum_ref1⟩` | 冗余一份链接，消费端不用拆 id 数组 |
| `name` | 老位号（`NAME` 原文） | issue 的 `name` |
| `type` | 交付单元 noun（`BRAN` / `EQUI` / `SUPPO` / `HANG` …） | issue 的 `type` |
| `zone_refno` | `pe:⟨…⟩` 或 `NONE` | issue 的 `zone_refno`；求法见 D5 |
| `dbnum` | u32 | 库号，方便按库排查 |
| `sesno` | 变化所在的会话（op 的 last-touch，不是窗口末端） | 发生在哪一版；不做键 |
| `sesno_time` | **同一版**会话页上的写入时刻（字串；只读真落记录的会话，一个会话读一次；2026-09-24 起，此前误取窗口末端 `end_sesno_time`） | 不用 `time::now()`（重放不确定、白名单拒） |
| `reason` | `'deleted'` \| `'renamed'` | 两个分支各写各的 |

不写 `instance_code` / `object_model_code`：前者由 `name` 用 rs-server 同一个 `change_name_into_illegal` 推，后者改名不变、消费端从 `datacenter_version:{refno}` 取；
gen-model 不复制 rs-server 的编码规则。

### D4　写入判据（两个分支，都只看**交付单元自身**）

```text
for op in 本窗净变化（DESI 窗口）:
  unit_types = configured_delivery_unit_types()          # 不含 ZONE；ZONE 只是 Modify 上溯的终点
  match op:
    Deleted:
      (noun, owner, name) = S 侧（窗口前态 / 墓碑）
      if noun ∈ unit_types and name 非空:
        INSERT IGNORE INTO datacenter_delete { id: datacenter_delete:<refno值>, reason: 'deleted', … }
    Modified where noun ∈ unit_types:
      (old, new) = modified_attrs ∪ modified_explicit_attrs 里 normalize == "NAME" 的那一条
      if old 非空 and old != new:
        INSERT IGNORE INTO datacenter_delete { id: datacenter_delete:<refno值>, reason: 'renamed', … }
    其余（下属管件的增删改名、Add）:
      不写                                              # issue 原文「不包含其下属管件」；管件变化照旧只让单元记 Modify
```

- `NAME` 从 `added_explicit_attrs` 出现（老值不存在 → 以前没名字）不记：中台没有按空位号发布过的东西。
- `Deleted` 的 noun / name 必须来自 **S 侧**：overlay 对 Deleted 没有数据，`effective_view` 会落到持久层点查——那一行此时还是窗口前态（artifact 先渲后落），名字在。
- 改位号同时又换属主 / 搬迁：照记（老位号照删），`Modify` 那条照旧。

### D5　`zone_refno` 的求法：沿 owner 链上溯到最近 `ZONE`

不沿用 `belong_zone` 那套「BRAN 隔一层、其余取 owner」的写法——支吊架（`SUPPO` / `HANG`）上头可能隔着 `STRU` / `FRMW`。用现成的 `effective_view` 走链（上限 64），
第一个 noun == `ZONE` 的祖先即是；到顶 / 链断 → `NONE`（issue 表允许为空，消费端按 ZONE 过滤时自然漏掉，日志点名）。
删除分支从 S 侧链上溯（被删单元的 owner 边在墓碑上还留着），改名分支按 overlay 优先（与 Modify 上溯同口径）。

### D6　写前检查 `datacenter_version`；只记录已发布 refno

收集本窗口变更 refno 后，按 `datacenter_version:⟨ref0_ref1⟩` 分块显式点查 `record::exists(id)`；只有命中的 refno 才渲染
`INSERT IGNORE`。检查发生在生成固定随窗 SQL 之前，写语句本身仍不含条件查询，重放安全不变。未发布过的单元被删 / 改名不进事件表。

### D7　生命周期：gen-model 只写不删；清库不碰

- 与 `datacenter_version` 同一条纪律：**不进** `fast_delete::RANGE_TABLES`，`Reinitialize` / 回退 / 删库都不清——待删的老位号仍待删，与库重不重建无关。
- 删行只有一个写者：rs-server 在中台确认 `remove-apply` 成功之后（§6）。gen-model 任何路径不 `DELETE datacenter_delete`。
- 基线首载（退化窗口）不产生记录：没有 S 侧，谈不上「改前」。

### D8　交付单元类型来自配置

判据用 `configured_delivery_unit_types()`（默认 `BRAN / HANG / SUPPO / EQUI`），不按 issue 硬编码三种。issue 写的 `SUPP` 在本仓对应 `SUPPO`；`HANG` 也算
提资的最小交付单元（**2026-09-24 用户拍板，Q1 已决**）：按配置全收，现行代码就是这样，不用改。

### D9　与 `datacenter_version` 的 Delete 分支并存

被删单元今天已经 `set status = 'Delete', belong_zone = …`；本计划**不改**那条（rs-server 现有消费路仍靠它），只是多落一行 `datacenter_delete`。两条路径在消费端
合并、按 `instance_code` 去重（§6）。等 rs-server 切到只读新表之后，再议退掉 `belong_zone` 那半句——不在本计划内。

## 3. 实施步骤（本仓，T1–T5，每步全量 `cargo test --lib` 不降）

- **T1 数据形状**（✅ 已落）：`window_net_states` 与 `load_pe_noun_owner_from_persistent` 的 `(Option<String>, Option<RefU64>)` 换成一个小结构
  `PeChainView { noun, owner, name }`（`load_pe_chain_view_from_persistent`：`SELECT id, noun, owner, name FROM […] WHERE record::exists(id)`）；
  `effective_view` 同步。既有 `resolve_datacenter_statements_with` 的渲染测试只换夹具形状（`row()` / `PeChainView`），断言不改、仍绿。
- **T2 渲染**（✅ 已落）：`resolve_datacenter_statements_with` 加 D4 两个分支 + D5 的 `nearest_zone(start)`；渲染器
  `render_datacenter_delete_insert_ignore(&DatacenterDeleteRecord) -> String`，ID 只取 refno 值，位号进普通字段并对 `\` / `'` 转义。
  语句加进现有 `statements`（Delete 逐条那一路），去重沿用 `dedup_statements_keep_last`。调用点 `datacenter_statements(data, db_type, dbnum, end_sesno_time)`
  ——`end_sesno_time` 的读取从尾事务前挪到随窗渲染前（同一页会话页，读一次）。§3.1 的 10 条用例落成 6 个测试函数，每条新语句都过
  `replay_safe::validate_statement`。
- **T3 白名单与 schema**（✅ 代码已落，**未验证**——落笔时主线 lib 被并行会话的 vendor 改动挡在编译期，见状态行；主线合上后先跑
  `cargo test --lib -- datacenter_delete datacenter_tests accepts_the_persist_path_statement_shapes planned_through`）：
  `replay_safe::accepts_the_persist_path_statement_shapes` 加两条「应接受」（改名 / 删除各一，含转义位号）；`pdms_inst::INST_RELATE_INDEX_SQL` 末尾加
  `DEFINE TABLE IF NOT EXISTS datacenter_delete SCHEMALESS` + `DEFINE INDEX IF NOT EXISTS idx_datacenter_delete_zone … COLUMNS zone_refno`（生产启动、
  暂存建库、四处 EXPLAIN 钉测试共用这一份）；`fork_surreal_compat::dual_datacenter_delete_refno_ids_insert_ignore` 用生产渲染器
  `render_datacenter_delete_insert_ignore` 在 mem + fork 双跑：DEFINE 建得进、`INSERT IGNORE` 写得进、同一 refno 的重放与后续事件均不覆盖不增行、
  `zone_refno` 等值 / `IN [...]` / `= NONE` 查得回、refno ID 往返、`'` / `\` 转义原样落库、
  fork 上等值 EXPLAIN 走 `idx_datacenter_delete_zone`。
- **T4 护栏**：`fast_delete` 加源码钉「`RANGE_TABLES` 不含 `datacenter_delete`」（与 `datacenter_version` 并排一句）；`manual_update` 预览回执加一格
  `datacenter_delete_rows: usize`（只计数，不改判定）。
- **T5 文档**：`CONTEXT.md` 新词条「**交付删除记录 (Delivery Delete Record)**」（`datacenter_delete` 的一行：某个交付单元曾以某位号发布、如今该位号要从中台删掉；
  写者只有增量随窗语句、删者只有 rs-server；_Avoid_：删除标记、`datacent_delete`）；`changelog.md` 一条；本文件状态改「已落地」。

### 3.1 单测清单（`increment_pipeline.rs` 现有 `render_with` 夹具）

| 用例 | 期望 |
| --- | --- |
| BRAN 自身 `NAME` `/A → /B` | 一条 `INSERT IGNORE INTO datacenter_delete { id: datacenter_delete:⟨7997_3⟩, reason: 'renamed', … }` + 原有 `Modify` |
| BRAN 下 ELBO 改 `NAME` | **不**出 `datacenter_delete`，只出 BRAN 的 `Modify` |
| BRAN `NAME` 从 `added_explicit_attrs` 出现（老值无） | 不出 |
| EQUI 被删（S 侧 name `/E-101`，owner 链 EQUI → ZONE） | `reason = 'deleted'`、`zone_refno` = 那个 ZONE；原有 `status = 'Delete', belong_zone = …` 照旧 |
| SUPPO 被删，owner 链 SUPPO → STRU → ZONE | `zone_refno` = 隔两层的 ZONE（D5 与 `belong_zone` 的差别就在这条） |
| 链断（owner 行不在） | `zone_refno = NONE`，语句仍出 |
| 同一 refno 重放或后续再次改名 | 仍只有一行，首次事件字段不变 |
| 同一批语句渲两遍 | 逐字相同（重放收敛） |
| 位号含 `"` / `\` | 转义后 `validate_statement` 接受 |
| 非 DESI 窗口 | 零语句（同现有门） |

## 4. 验收

1. `cargo test --lib` 全绿、`cargo clippy` 触及文件 0 告警。
2. TTY 真跑前为目标 BRAN / EQUI 预置同 refno 的 `datacenter_version` 行；改位号 + 删除各产生一条事件，字段齐；同窗重放及同 refno 后续事件不改变首次记录。
3. 从未提资过的单元不出行（D6）；`Reinitialize` 之后已有事件行仍在（D7）。

## 5. 不做

- 不改 `datacenter_version` 的 Delete / Modify 语义（D9）；不写 `instance_code` / `object_model_code`（D3）；
  不为下属管件记删除记录（D4）；不清表（D7）；不给这张表加水位或回执。

## 6. 跨仓后续（本仓不实现）

- `gen-model` 只维护事件表：确认同 refno 的 `datacenter_version` 已存在后，以 `INSERT IGNORE` 记录首次删除或改名事件。
- `rs-server` 本轮不改；消费、提交、确认和清理策略全部交给第三方实现。
- `plant-ui` issue 只作为需求来源；本仓不替第三方规定事件的消费顺序或删除时机。

## 7. 开放问题

- **Q1**（**已决**，2026-09-24 用户拍板）`HANG` 是否算提资交付单元（D8）——算，按配置全收（默认 `BRAN / HANG / SUPPO / EQUI`），代码不动。
- **Q2**（**已决**，2026-09-24 用户拍板）消费端是否需要按 `dbnum` 而不是 ZONE 过滤（跨 ZONE 搬迁后老 ZONE 可能不在所选范围里）——分记录类型：
  `renamed` 跟 refno（随单元新数据同批）、`deleted` 按存的 `zone_refno`、孤儿（`NONE` / ZONE 已不存在）按 dbnum 兜底、提资预览里确认后才发。细则见 §6；本仓两个字段都已给，不用改。
- **Q3** 老库里已经发生过、但本计划落地前的改位号，中台残留怎么补——一次性脚本对比 `datacenter_version.instance_code` 与当前 `NAME`，不在本计划内。
