# 2026-09-04 开发方案：旧解析栈退役收口（B8 / B9）——基线全量解析换 e3d-io 底座，`parse_pdms_db` 离开工作区

- 承接：`2026-09-03-old-pdms-io-retirement-inventory.md`（第一张清单，old-pdms-io，**已完**）、
  `2026-09-04-parse-pdms-db-retirement-inventory.md`（第二张清单，B1–B7 **已完**，B8 / B9 **未动**）。
- 决策依据：ADR-058 D3（DESI 保留基线）/ D4（启动 = 退化窗口）；共识 d-6（对拍以 E3D TTY 为准，不拿第二套解析器当参照臂）；
  宪法 II（一条规则只有一份实现）、III（静默失效是最高级别缺陷）、VI（不变量由可执行的守护看住）。
- 用户 2026-09-04 原话：「继续退役 old-pdms-io 和 old parse，全面改成对接 e3d-io」。
- 状态（2026-09-04 晚间复核，见 §0.1 / §0.2）：**B8.0–B8.4 已落进工作树**；B8.5 已完成单测、7997 真库基线、
  SYS meta 实跑与一组 BRAN 的 **E3D TTY 85/85 对照**；**B9 六条全部落地**（依赖 / 探针 / 护栏 / CI / 脚本 / `Cargo.lock`
  重解 / 文档 / changelog），只剩 B8.5 的真库外部裁决与整棵工作树的提交拆分。Q2 / Q3 / Q4 按建议落地，
  Q6 落在「乙的兜底」。Q5 已明确为“e3d-io 物理读取对 E3D TTY”，旧 parser 不参与对比。
  §1 的 F 表与 §4 的行号是**方案落笔时**的，实施后的现状看 §0.1 与 §1.1，别拿旧行号去找代码。

## 0. 一句话

old-pdms-io 已经退干净（`Check-LegacyPdmsIo.ps1` 四判全绿，本轮实跑）。`parse_pdms_db` 在本仓只剩**一条路**还在用它——
库首次纳入时的基线全量解析（`versioned_db/database.rs::sync_total_async_threaded` + `member_prune.rs`）。本方案把这条路
收敛成 e3d-io 的「退化窗口」（`S = 无基会话, T = file_latest`，与稳态增量**同一个收集器、同一个终态渲染器**），然后把
`parse_pdms_db` 连同 `[patch]`、8 个探针 bin、2 个门后测试一起请出工作区，护栏改成与 old-pdms-io 同款的「manifest 与源码一处都不许有」。

## 0.1 落地状态（2026-09-04 复核；工作树快照 08:50，HEAD `9c849a86` + 未提交改动）

复核方式：逐条拿方案里的断言对工作树（`rg` / 读码），再实跑 B8.5 第 1 / 2 条。**工作树仍在变动**——另一会话正在同一棵树上收护栏
（`data_interface/mod.rs::legacy_parse_pdms_db_guard` 与 `scripts/Check-LegacyParsePdmsDb.ps1` 在本轮复核的 20 分钟内更新了两次），
下表以 08:50 快照为准，行号同。

| 阶段 | 状态 | 证据（工作树） |
|---|---|---|
| B8.0 核实 | ✅ 第 1–4 条；第 5 条（语料规模）⏳ 真库机器 | 1 `pe_owner` 权威 = `in = child, out = owner`，渲染器是那个写反的（B8.1 已修）；**CONTEXT.md「pe_owner」词条仍未新增**。2 `pe.sesno == 0` 两仓 grep 无消费者：gen-model 命中全是 `applied_sesno / file_latest_sesno / source_end_sesno`；plant-ui 只有 rs-core `refno.rs:563` 的 refno 自身字段与 `task_queue.rs:539` 的 `applied_sesno`。3 `ATT_UDA` 消费形状 = `{ id, TYPE, refno: <TYPE>:<id>, udas: [{ u: UDA:<def>, v }] }`（plant-ui `fn_query_room_code_hh.surql:4` 读 `.udas where u.NAME`；`common.surql:148`）——方案 B8.1 第 1 条写的 `uda: [{u, v}]` 是笔误，实现按 `gen_sur_json_uda` 走对了。4 模板：**F11「本机没有模板目录」不成立**——`AIOS_E3D_TEMPLATE_DIR` 未设时走 `direct_store.rs:44` 的默认值 `E:\reverse\e3d\shadow_e3d31_aps_all`，本机存在且齐 `attlib / sysvir / dicvir / glbvir / catvir / desvir`；**无 `glovir.dat`**，只在某个文件反哈希出的库类型是 `GLOB`（`db_type_name` 按前三字母派生模板名）时才会撞，撞了是响亮失败（`DirectSchema::template_file_for`），符合 §6 那行的要求 |
| B8.1 渲染器补齐 | ✅ | `changelog.md` 2026-09-04「修复」第一条；`ele_ops.rs:1439 pe_owner_edge`、`:1446 render_uda_row_content`、`:1421 UdaRowPolicy`、`:1488 ElementRows`；新模块 `data_interface/uda_keys.rs::UdaKeyMap`；`increment_pipeline.rs:1486-1490` Add 前置清理多一条 `DELETE ATT_UDA:{pe}`；护栏 `ele_ops::terminal_render_guard_tests` 四条（:805-1008）+ `uda_keys::tests` 三条 |
| B8.2 `stream_baseline` | ✅ | `e3d_collector.rs:517-581`；与 `collect_net_window` 共用 `open_frozen_target(:439) / enumerate_target_index(:461) / read_element / allowed_ignored_final`，`None` 分支 :639-647 改调同一枚举；源码钉 `the_baseline_and_the_net_window_share_one_enumeration`（:957）；chunk 上限的夹具断言在 `tests/baseline_stream_8000.rs`（`CHUNK_SIZE = 64`，issue-019 db8000 归档） |
| B8.3 基线写者 | ✅（写路按「保留 `INSERT IGNORE` 批」落，与方案第 2 条不同，见 §4 B8.3 落地记） | 新模块 `versioned_db/baseline.rs::render_baseline_batches`（:25-72）；`database.rs:1194-1354` 解析段整段换成 `FrozenSource::capture → UdaKeyMap::load → spawn_blocking(stream_baseline → render_baseline_batches → sender.send)`；`member_prune.rs`、`pe.rs` 两个文件删除；`read_full_basic_data / legacy_session_ranges / preserve_unparsed_pe_metadata` 删（:247-249 留一句）；源码钉 `mod.rs::the_baseline_orchestration_has_no_legacy_parser_intermediates`（`DbBasicData / member_prune` 不许回到 `database.rs`） |
| B8.4 接线 | ✅ | 签名不变（`database.rs:997`）；六个调用点 `database.rs:729 / 768 / 817 / 830`、`manual_update.rs:3385 / 3426` 一个未动；`manual_update.rs:6414` 源码钉仍成立 |
| B8.5 门 | 第 1 / 2 条 ✅；第 3 条已有 BRAN 样本 ✅；第 4–6 条及扩大样本 ⏳ | 本轮实跑：`cargo check --workspace --all-targets --locked` 过（既有 2 条 warning）；`cargo test --lib -- ele_ops e3d_collector versioned_db legacy_parse_pdms_db_guard uda_keys baseline` **76 passed / 0 failed / 6 ignored**；`cargo test --locked --test baseline_stream_8000` **6 passed**；`Check-LegacyParsePdmsDb.ps1` 五判、`Check-LegacyPdmsIo.ps1` 四判全绿；BRAN `24383/73930` 以 e3d-io 读取 `ams7999_0001`，引用库 `ams5053_0001`，对 E3D TTY 为 **85 equivalent / 0 mismatch / 0 omitted** |
| B9 依赖收口 | ✅ 六条全落（2026-09-04 收口，会话 opus-5-35 补完 `Cargo.lock` 与文档半边） | 1 `Cargo.toml` 依赖 / `[patch]` / feature / 8 bin / 2 test 全删；`ele_data.rs` 的 `From<legacy>` 删；`Cargo.lock` 重解后少 5 个包（`parse_pdms_db`、`pdmsdb_engine_v2`、`dynfmt`、`hash32-derive`、`erased-serde 0.3.31`），**只少不多**（唯一 `+` 行是 `erased-serde` 去重名消歧）。2 §4 发现 5 的死代码 `frozen_source::read_frozen_bytes` 一并删。3 护栏翻面 + `Check-LegacyParsePdmsDb.ps1` 五判进 CI（`windows-tests.yml`）。4 `Toggle-LocalDeps.ps1` 只剩 aios_core、`Assert-NoLocalPatch.ps1` 白名单清空；**另补** `Check-LegacyPdmsIo.ps1` 第 1 条同时认连字符 `pdms-io`——它原先只扫 `pdms_io`，`pdmsdb_engine_v2 v0.1.0 (…/pdms-io.git?rev=…)` 两个字符串都不匹配，那条尾巴就是这么在「四判全绿」下藏住的。5 `CONTEXT.md`「分块解析基线」重写 + 新增「pe_owner 边」词条；`changelog.md` B8.2–B9 一条已落。6 共识已记（`record_decision` → `d-31`）。验收：`cargo tree -i parse_pdms_db` / `-i pdms_io` 均 `did not match any packages` |

**复核发现（方案没写到、与方案不同、或仍欠着的）**：

1. **写路保留了 `SenderJsonsData` 三种消息与 `INSERT IGNORE` 批**。方案 B8.3 第 2 条设想「消息变成 SurrealQL 语句批、逐行 `UPSERT`，
   超今天 1.5× 再加批量写者」；落地时反过来直接保留 `INSERT IGNORE`，`baseline.rs` 头注释给的理由不是吞吐而是**语义**：父层补缺
   （`manual_update.rs:3426`，`replace_dbs = false`）把 extract 母库整库再同步进同一个 dbnum，靠「已有行赢」保住叶子 extract 正在改的
   版本，`UPSERT` 会覆盖它。与增量共享的是 `ElementRows` 行形状，不是语句动词——方案「共享的是行形状」那句成立，
   B8.5 第 4 条的「超 1.5× 触发批量写者」从此没有触发对象，改成单纯量耗时与峰值内存。
2. **WORL 写全三样行**（`pe` / `ATT_WORL` / `pe_owner`），不是方案 B8.2 第 5 条写的「`pe` + `pe_owner`」；`BaselineStats::elements` 不计它，
   `baseline_parse_matches`（`manual_update.rs:2993`）一个字未改。这是**数据面形状变化**（legacy 不写 WORL 的 `pe` 行；`BaselineStats::worlds`
   的注释点名 plant-ui 根层查询为此绕过一跳），B8.5 第 3 条形状 diff 的「预期差」要加这一条。
3. **`BaselineChunk` / `BaselineStats` 形状**：`elements: Vec<(u32, EleData)>`（会话号随元素走，没有单独的 `sesno_of`）、`ignored: Vec<IgnoredFinalRecord>`；
   `BaselineStats` 多 `target_sesno / indexed / worlds`。`open_target` 落名 `open_frozen_target`。
4. **`pe.rs` 整个文件删了**（不只是 `save_pe_relates`），`SPdmsElement` 现从 `aios_core::pe` 来；F6 拿 `pe.rs:174` 当方向见证的那条证据从此只在 git 历史里。
5. ~~**`frozen_source.rs:412-417 read_frozen_bytes` 成了死代码**：唯一调用方 `read_full_basic_data` 已删。B9 顺手删。~~ **已删（B9 收口）。**
6. ~~**CONTEXT.md「分块解析基线」词条（:259-261）与实现相反**：它写「按遍历分批装入 `mem://` 实例 … _Avoid_: 流式解析」——`mem://` 读半边在
   HEAD 之前就已不在 `database.rs`，而新基线**就是**流式（`stream_baseline` 按 chunk 交给有界队列）。~~ **已按新底座重写（B9 收口），
   并从「已退役词条」小节下面挪回现行词条区，同处新增「pe_owner 边」词条。ADR-042 加了 2026-09-04 实施注记
   （§2 的 `mem://` 载体与 §10 的 journal 重放一样都没用上，基线与 `staging/` 再无关系）。** 仍欠：ADR-042 的状态行
   还是「已接受 2026-08-22」——改 Superseded 还是改写成「目标达成、载体换成流式块」是一次决策，留给 ADR 评审。
7. **`uda_keys` 是方案外新增的一层**：`ATT_UDA` 的 `u` 链接要把 UKEY 换成 `UDA:<refno>`，基线每文件 `UdaKeyMap::load()` 一次（`database.rs:1250`），
   查不到的键记账进日志 / 回执（`:1335`）而不是静默丢。这也把「DICT 相必须先于 DESI 相」从习惯变成了依赖——顺序 `database.rs:729 → 768 → 817 → 830` 本来就如此，别动。
8. **Q6 的实际落点是「乙的兜底」**：S3 未落地，CATA 相（`database.rs:817-821`）今天走同一个 `stream_baseline`（`catvir.dat`），没有另写代码；
   S3 落地时把 :817 那段连同 `full_sync_catalogue_files(.., "CATA")`（:725）一起删。
9. ~~**两处过期注释**：`Cargo.toml:46-53`（「还剩 versioned_db/{database,member_prune}.rs 的基线全量解析…这道门今天是空的」）、
   `manual_update.rs:104`（「基线走 `parse_pdms_db`」）。B9 一起改。~~ **两处都已改写（B9 收口）**，`manual_update.rs:6179`
   那条「重放走 legacy `pdms_io`」的测试文档注释同批改成「当时如此、现已随退役消失，判据不分叉这一条照旧」。
10. ~~**changelog 只记了 B8.1**：B8.2–B8.4 这一大块还没有 changelog 条目。~~ **已落（B9 收口）**：`changelog.md` 2026-09-04「移除」第二条
    即方案要求的「B8.2–B9 一条」，含发现 1 / 2 两处与方案不同的地方及其理由。

## 0.2 晚间真库进展与判据修订（2026-09-04）

- `ams7997_0001` 由 e3d-io 读取并完成基线：`170048` 个 `pe` / `dbnum_info` 行，文件最新会话与应用水位均为 `106`；
  基线收口排入 `18675` 个生成根。模型工作项改为每批最多 `500` 条先行幂等 UPSERT，全部成功后才以小事务推进水位，修复
  SurrealDB 大事务关闭通道的问题。
- `sysvir.dat` 对 `DTWL (0x000BFA34)` 给出的物理事实是 `authoritative=true, attribute_count=0`。e3d-io 已允许这种合法的
  零属性 noun 返回空属性提取；真实 `amssys 24575/13` 得到 `implicit_word_count=11, layout=wide, attributes=[]`，随后
  SYST 8191 基线写入 `1205` 个 `pe` 行，SYS/DICT meta 阶段完成。
- 验收数据源固定为两层：**e3d-io 读取的数据库记录、`attlib.dat` 与对应 `*vir.dat` 是候选事实；E3D TTY 导出是外部裁决**。
  旧 parser 的输出、旧库形状或旧 parser 生成的快照不再作为参照臂。BRAN `24383/73930` 已完成首组外部裁决：TTY 85 行均为
  `equivalent`，`mismatch=0`、`omitted=0`；其中 `Hstube / Pspec / Ispec` 通过 ref0 `13245` 唯一定位到 `ams5053_0001` 后解析为
  `/VMB1/CPP00102:P,50`、`/VMB1`、`/I100-HL`。该样本关闭普通属性和引用名解析，不代表 UDA 或五类库扩大抽样已经完成。

## 1. 现状核实（方案落笔时，2026-09-04 上午实测，工作树 `e3d-direct` @ `9c849a86` + 未提交改动；**实施前快照**，现状见 §1.1）

| # | 事实 | 证据 |
|---|---|---|
| F1 | **old-pdms-io 无残留。** 工作区依赖图无 `pdms_io`、无包依赖它、两份 manifest 无、三处源码无 | `scripts/Check-LegacyPdmsIo.ps1` 四条 `WORKSPACE_GRAPH_HAS_NO_PDMS_IO / NO_PACKAGE_DEPENDS_ON_PDMS_IO / MANIFESTS_HAVE_NO_PDMS_IO / SOURCE_HAS_NO_PDMS_IO`。源码里剩下的 `pdms_io` 字样全是注释、护栏本身与 `Assert-NoLocalPatch.ps1:9` 的 patch 白名单 |
| F2 | **`parse_pdms_db` 的 lib 侧代码行只剩基线路两个文件。** | `Check-LegacyParsePdmsDb.ps1` 四条全绿。`versioned_db/database.rs`：`use parse_pdms_db::parse::*`（:17）、`read_full_basic_data → parse_db_basic_data`（:1139-1146）、`parse_file_with_chunk`（:1477）、两条测试（:440/:443）；`versioned_db/member_prune.rs`：`parse_ele_membs`（:27/:142）、两条测试（:339/:383）。`ele_data.rs:82` 的 `From<legacy EleData>` 在 `legacy_parse_pdms_db` 门后 |
| F3 | **`sync_total_async_threaded` 六个调用点。** | `database.rs::sync_total`：SYST/GLB/GLOB（:869）、DICT（:908）、CATA（:957）、DESI（:970）；`manual_update.rs`：单库基线（:3381）、父层补缺（:3422）；另有源码钉 :6409 |
| F4 | **退化窗口的「收集半边」已经存在。** `collect_net_window` 在 `base_sesno == None`（窗口从文件第一个会话起）时把目标索引整棵当 `Inserted` 枚举 | `e3d_collector.rs:493-514`（`DbView::open_at` + `IndexCursor::at_root(..).walk()`）。缺的是**分块 / 流式**（今天整窗攒进一个 `BTreeMap`）与基线编排 |
| F5 | **终态渲染器不写 `ATT_UDA`。** 基线写；e3d-io 收集器把 `:UDA` 装进 `uda_atts`，`WholeAttMap::merge()` 不并它 → **增量路今天就丢 UDA** | 基线：`database.rs:1589-1616`（`gen_sur_json_uda` → `INSERT IGNORE INTO ATT_UDA`）；渲染器：`ele_ops.rs:1196-1223` `render_terminal_element_state` 只出 `pe` / `ATT_<noun>` / `pe_owner`；收集器：`e3d_collector.rs:236-237, 339-389`；`old-aios-core/src/types/whole_attmap.rs:123-131`；`gen_sur_json_uda` 只认 `UDA:` 前缀键（`named_attmap.rs:838`） |
| F6 | **`pe_owner` 边方向在仓里有两种写法，终态渲染器是少数派。** | `in = child, out = owner`：基线 `pe.rs:174`、`cata_closure.rs:1541`（读侧 :1439 `{owner}<-pe_owner` 取 `in` 当 children）、legacy 兼容 `ele_ops.rs:1270`。`in = owner, out = child`：**生产终态渲染** `ele_ops.rs:1216`（`persist_latest_main_data → render_terminal_persist_statements → PersistElementChange::to_surql`）。疑似反向，B8.0 第一件事核实 |
| F7 | **`pe.sesno` 两种口径。** 基线：元素所在页落进哪个会话页区间（链头会话的元素记 **0**，逐字复刻旧表）；收集器：终稿记录所在页反查的 last-touch 会话 | `db_file_header.rs:265-282` `legacy_session_ranges`；`e3d_collector.rs:538` `last_touch` |
| F8 | **失败容忍不同。** 基线：属性解析失败的元素只留 `pe` 拓扑（`preserve_unparsed_pe_metadata`）；收集器：只容忍 attlib 反不出名字的 noun 与 `MNUM`，其它**整窗作废** | `database.rs:256, 1493-1519`；`e3d_collector.rs:24-27, 425-434` |
| F9 | **写路形状不同。** 基线：有界队列（100）/ 单写者 / 批量 `INSERT IGNORE INTO pe [..]`；增量：`persist_transaction_batches(PERSIST_TX_CHUNK)` 逐元素 `UPSERT … CONTENT` | `database.rs:50-56, 1199-1296`；`increment_pipeline.rs:1445-1483` |
| F10 | **基线编排里不属于解析的部分**要原样留下：`check_and_clear_db`（replace）、`dbnum_info_table` 结算、失败清库、`parse_error::flush`、返回 `HashMap<dbnum, count>` 给 `baseline_parse_matches` | `database.rs:1365, 1683-1718`；`manual_update.rs:2953-2999` |
| F11 | **本机没有真库、没有模板目录。** `AIOS_E3D_TEMPLATE_DIR` 未设，`*vir.dat` 不在仓里 | live 门要在有 `attlib.dat + desvir.dat / sysvir.dat / dicvir.dat / …` 与 448 库语料的机器上跑 |
| F12 | **依赖与脚本。** `parse_pdms_db` git `005936a6` + `[patch]` → `../vendor/old-parse-pdms-db`；传递 `aios_core`（同 patch，**本仓也直接依赖，不在退役范围**）、`pdmsdb_engine_v2`（`efd2e19`）。`Toggle-LocalDeps.ps1:33` / `Assert-NoLocalPatch.ps1:9` 点名它。CI 只跑 `Check-LegacyPdmsIo.ps1`（`windows-tests.yml:117`），**没跑** `Check-LegacyParsePdmsDb.ps1` | `Cargo.toml:54, 211, 325-330` |

## 2. 目标形态

```
库首次纳入（applied_sesno == 0 / pe 零行 / 幽灵水位重建 / 父层补缺）
  └─ FrozenSource::capture(path)                              e3d-io：dbnum / 库类型 / T=file_latest / 冻结长度
      └─ e3d_collector::stream_baseline(frozen, db_type, chunk)   ★ 新：DbView@T 整棵索引 → 按 chunk 出 Vec<EleData>
            │  与 collect_net_window 共用 open_pinned_set / read_element / assemble_ele_data / allowed_ignored_final
            └─ 每 chunk：terminal_persist_changes(全 Add) → render_terminal_element_state（pe / ATT_<noun> / ATT_UDA / pe_owner）
                  └─ versioned_db::baseline 写者（有界队列 + 单写者，语句形状与增量同一份）
      └─ 编排原样：dbnum_info 结算 / 失败清库 / parse_error::flush / 返回 dbnum→count

稳态增量 S→T（今天已是 e3d-io）
  └─ collect_net_window → terminal_persist_changes → render_terminal_element_state → persist_latest_main_data
```

一句话：**基线 = 每个元素都是 `Add` 的窗口**。收集器、`EleData`、渲染器三样与增量完全同一份；基线只多一个「按 chunk 流式 + 有界写」的外壳。
`parse_db_basic_data / parse_file_with_chunk / parse_ele_membs / member_prune / legacy_session_ranges` 整体下线。

## 3. 要拍板的（请在 Plannotator 上直接批注）

| # | 问题 | 选项 | 建议 |
|---|---|---|---|
| Q1 | `parse_pdms_db` 的退出方式 | **A** 整体退出，同 old-pdms-io：删依赖、删 `[patch]`、删 `legacy_parse_pdms_db` feature、删 8 个探针 bin（`attr_info_compat_probe / cata_closure_probe / cata_parse_probe / curd_parse_probe / noun_layout_parse_probe / refno_attr_probe / refno_index_probe / world_probe`）与 2 个门后测试（`full_attributes_real` / `pdms_record_boundary`），`ele_data.rs` 的 `From<legacy>` 一并删；护栏改「一处都不许有」<br>**B** 清单 §3.3 原案：`optional = true` + `legacy_parse_pdms_db = ["dep:parse_pdms_db"]`，探针留在门后 | **A**。d-6 已定「不拿第二套解析器当参照臂」，探针存在的唯一理由就是当参照臂；old-pdms-io 走的也是 A。`full_attributes_real` 的「对照 `q att` 清单」若还要，改成 e3d-io 读法对 TTY 清单（那才是 d-6 的判据） |
| Q2 | 基线换底座走哪条路 | **a** 退化窗口（§2；一份收集、一份渲染，`member_prune` 消失）<br>**b** 原地换解析器：`indexed_refnos` 造 `refno_table_map`、`scan_elements` 造 `children_map`、`DbSet` 逐元素 `to_named_attmap`，保留 `DbBasicData` 与两条写路 | **a**。b 换完仍是「两条路两个中间态」，宪法 II 不许；a 同时是 ADR-058 D3/D4 与 single-read-two-lanes 计划的实施项 |
| Q3 | 基线的失败容忍 | **甲** 与增量同一条闸：非白名单解码失败 → 该库基线失败、不推水位、回执点名 refno / noun / 原因<br>**乙** 保留 `preserve_unparsed_pe_metadata` 式降级（留 `pe` 拓扑、丢属性、计数） | **甲**。乙正是宪法 III 说的「它跳过的东西谁会发现」；而且 e3d-io 上「解不出属性」多半是 schema 缺形（`shape_conflicts` 一类），写进去的是错数不是空值。若 448 库里真有成批命中，那是要修 schema 的信号，不是要放行的信号 |
| Q4 | `pe_owner` 边方向以谁为准 | 不是拍脑袋能定的：B8.0 先在真库跑 `SELECT * FROM pe_owner LIMIT 3` 与两条读路（`cata_closure.rs:1439` 的 `<-pe_owner` 取 children；模型树上溯）定权威，再决定改渲染器还是改读路 | 倾向权威 = `in = child, out = owner`（基线 + cata_closure + `unique_pe_owner (in, out)` 索引都按它建）。若核实如此，`render_terminal_element_state` 是 bug，**先修它**（B8.1），基线换底座之前增量就已经在写反边 |
| Q5 | 对拍判据 | e3d-io 按数据库二进制、`attlib.dat` 与对应 `*vir.dat` 生成 `pe / ATT_* / ATT_UDA / pe_owner`；E3D TTY 导出 `Q ATT` / `Q MEMBERS` 作为唯一外部参照 | 逐元素比较值、字段存在性、UDA、owner 与成员序；每条分歧都回到物理描述符和 TTY 裁决。旧 parser 输出及其历史库形状不进入对比，也不得把 e3d-io 自洽写成 TTY 门通过 |
| Q6 | CATA 相的基线（`sync_total` :957） | **甲** 随 ADR-058 D1 / 启动重构 S3 删除，本方案不给 CATA 造新基线<br>**乙** 先换底座保持行为，S3 再删 | **甲**，前提是 S3 与本方案同期或更早落地；若 S3 排在后面，则 CATA 相临时走同一个 `stream_baseline`（`catvir.dat`），不另写代码——只是多跑一种 `db_type` |

## 4. 分阶段

### B8.0 核实（0.5 人日；F11 之下本机只能做前两条）

1. `pe_owner` 方向（Q4）：读 `live_add_pe_owner_replay_is_idempotent`（`increment_pipeline.rs:3534`）与 `cata_closure.rs:1439` 的期望；有真库时 `SELECT * FROM pe_owner LIMIT 3`。结论写进 `CONTEXT.md`「pe_owner」词条（今天没有这个词条）。
2. `pe.sesno == 0` 的消费者：`rg 'sesno == 0|sesno <= 0' src/` 命中的都是 `applied_sesno` / `source_end_sesno` / `file_latest_sesno`，**没有**读 `pe.sesno` 为 0 的（本轮已查）。F7 的链头元素从此写真会话号——确认 UI / 房间 / 抽取树没有把 `pe.sesno = 0` 当「基线元素」的暗约定（`plant-ui` 侧 grep 一次）。
3. `ATT_UDA` 的消费者形状：`uda_table.rs` 与 `plant-ui` 读 `ATT_UDA` 行的字段（`id / TYPE / uda: [{u, v}]`），B8.1 的渲染要逐字段对上。
4. 模板目录齐全性：Meta 相要 `sysvir.dat / dicvir.dat / glovir.dat`（`GLB` → `glbvir.dat` 是否存在，`template_file_for` 是按前三字母派生的，`db_element.rs:61-70`）。缺哪份，Meta 相就换不了底座——那是环境问题，先列出来。
5. 语料规模：448 库里最大的几个（元素数 / 文件大小），定 B8.2 的默认 chunk 与内存上限。

### B8.1 终态渲染器补齐（1–1.5 人日；独立于换底座，**先做**——修的是增量路今天就有的洞）

1. `render_terminal_element_state` 增写 `ATT_UDA`：`uda_atts` 非空 → `UPSERT ATT_UDA:{id} CONTENT { id, TYPE, uda: [{u, v}, …] }`（形状与 `gen_sur_json_uda` 逐字段同，`RefU64Type / ElementType` 值走 `to_pe_key`）；为空且是 `Modified` → `DELETE ATT_UDA:{id}`（旧行不许残留，与 `RetiredTypeRow` 同理）。`Deleted` 路径不动（`pe.deleted = true` 软删，`ATT_*` 行本来就留着）。
2. `pe_owner` 方向按 B8.0 结论统一；`render_terminal_persist_statements` 里的两条清理语句（`DELETE {pe}->pe_owner` / `DELETE pe_owner:[{pe}, NONE]..=[{pe}, ..]`）随方向一起复核——方向反了的话 `->` 清的是自己指向 owner 的边，不是自己的 children 边。
3. 护栏（宪法 VI）：`ele_ops` 单测三条——UDA 行形状与 `gen_sur_json_uda` 输出逐字节等、UDA 清空时出 `DELETE`、`pe_owner` 边 `in/out` 与 `cata_closure::render_pe_owner_statements` 同向（两处渲染同一份 children 得到同一组边，用 `include_str!` 或直接调用互比）。
4. `changelog.md` 一条：这两处是**增量路的既有缺陷**，与基线换底座无关，单独提交、单独可回滚。

### B8.2 流式基线收集器 `e3d_collector::stream_baseline`（2 人日）

```rust
pub struct BaselineChunk { pub elements: Vec<EleData>, pub sesno_of: Vec<u32>, pub ignored: Vec<IgnoredFinalRecord> }
pub fn stream_baseline(
    frozen: &FrozenSource, db_type: &str, chunk_size: usize,
    mut sink: impl FnMut(BaselineChunk) -> anyhow::Result<()>,
) -> anyhow::Result<BaselineStats>
```

1. `frozen.verify_path_identity()` → `DirectSchema::open_from_env()` → `open_pinned_set(schema, path, db_type, T)`（与 `collect_net_window` 同一段，抽成私有 `open_target(frozen, db_type)`）。
2. `DbView::open_at(path, Exact(T))` + `IndexCursor::at_root(..).walk()` 枚举整棵索引——**就是 `collect_net_window` 的 `None` 分支**（`e3d_collector.rs:493-514`），抽成 `enumerate_target_index(view) -> impl Iterator<Item = (RefNo, IndexPosition)>`，两处共用。
3. 每 `chunk_size` 个 refno：`read_element(&target_set, refno)` → `EleData`，`sesno = last_touch(position.page)`（F7 新口径，与增量同）；失败走 `allowed_ignored_final`（Q3 甲：白名单外直接 `Err`，错误文本带 refno / noun / 原因）。
4. `sink(chunk)?` 后**释放该 chunk**，不攒全窗；`BaselineStats{ elements, chunks, ignored, pages_read, elapsed_ms }` 回给调用方。
5. **WORL 的处理**：基线今天 `ignore_world_refno = true`、`all_refnos` 里也滤掉了 `world_refno`（`database.rs:1427-1432`），但 `save_pe_relates` 会写 WORL → SITE 的 `pe_owner` 边；`baseline_counts` 单独数 WORL 行（`manual_update.rs:3331`）且 `baseline_parse_matches` 期待 `pe_count − root_count == parsed_count`。新路里 WORL 作为一个普通 `Add` 写 `pe` + `pe_owner`，`stats.elements` 里**不计**它——`baseline_parse_matches` 一个字不改。
6. 护栏：`enumerate_target_index` 与 `collect_net_window` `None` 分支同一函数（源码钉：`collect_net_window` 全文不含第二个 `IndexCursor::at_root`）；`stream_baseline` 的 chunk 内存上限单测（用 8000 夹具，`chunk_size = 3`，断言 sink 被调 ⌈n/3⌉ 次且每次 ≤ 3）。

### B8.3 基线写者 `versioned_db::baseline`（新模块，2 人日）

1. 把 `sync_total_async_threaded` 里**解析段**（`database.rs:1345-1657`：`FrozenSource::capture` 之后到 `parsed_db_infos_for_parser.insert` 之前）替换为：`stream_baseline(frozen, db_type, chunk, |chunk| { let changes = chunk 全 Add → PersistElementChange; let statements = render_terminal_persist_statements(&changes, dbnum); sender.send(statements) })`。
2. 写者保留今天的形状（F9）：有界 `flume` 队列 + `BASELINE_WRITE_WORKERS = 1` 单写者 + `execute_surreal_checked` 冲突重试；**只是消息从「三种 JSON 批」变成「一段 SurrealQL 语句批」**。`INSERT IGNORE` 批量与 `UPSERT … CONTENT` 逐行的吞吐差先量（B8.5 第 4 条）；若逐行 UPSERT 在最大库上超今天基线 1.5×，加一个「行 JSON → `INSERT IGNORE INTO pe [..]`」的批量写者，**它消费的仍是 `render_terminal_element_state` 用的同一份 `pe_data.gen_sur_json / att_map.gen_sur_json / uda json`**——共享的是行形状，不是语句动词。
3. `member_prune::prune_resurrected_members` 整段删除：e3d-io 的 `member_refnos()` 就是元素自己的成员块，补链轮不存在，issue #10 的幽灵子树在源头就没有。`member_prune.rs` 整个文件删；它的三条测试里「`24383_2` 与 `24383_66456` 只留一棵」改成对 `stream_baseline` 输出断言（夹具在 `tests/fixtures/issues/`）。
4. `read_full_basic_data` 删；`legacy_session_ranges` 删（唯一调用方是基线）；`preserve_unparsed_pe_metadata` 删（Q3 甲）；`set_uda_attr` / `gen_pdms_element_insert_sql` 两段死注释顺手清。
5. `sesno == 0 → continue`（`database.rs:1377`）保留：空库不进基线是原义。
6. `#[cfg(feature = "sql")]` 的 MySQL 分支（`save_pes_mysql`）：不在退役范围，但它吃 `DbBasicData`——feature 默认关，编译不到；改成吃 `EleData` 或删由用户定，本方案**不动它**，只保证默认 features 编译过。
7. 护栏：`sync_total_async_threaded` 全文不含 `parse_pdms_db` / `DbBasicData` / `member_prune`（源码钉，进 `legacy_parse_pdms_db_guard`）；`manual_update.rs:6409` 那条既有源码钉复核仍成立。

### B8.4 接线与编排（0.5 人日）

- `sync_total_async_threaded` **签名不变**（`db_option / project / cur_dbno_set / db_types / proj_progress_chunk → HashMap<u32, usize>`），六个调用点一个不动。
- `manual_update.rs:3381` 单库基线与 :3422 父层补缺自然换底座；父层补缺的 `parent_gap_refno_count` 已是 e3d-io 活索引（B5），两边口径从此一致。
- Q6：CATA 相按拍板处理；DICT / SYST / GLB / GLOB 相在模板齐全的前提下同一条路。

### B8.5 门（真库机器，1–2 人日；B8.0 第 5 条定语料）

1. **单测**（本机）：`cargo test --lib -- ele_ops e3d_collector versioned_db legacy_parse_pdms_db_guard` 全绿；`cargo check --workspace --all-targets --locked` 默认档过。
2. **夹具**（本机）：`tests/db8000_*` 与 `db_session_fixture_selfcheck` 不受影响（它们已是 e3d-io 读法）；新增 `tests/baseline_stream_8000.rs`：对 8000 夹具跑 `stream_baseline`，断言元素集 = `snapshot_presence` 的 present 集、每个元素的 `EleData` 与 `read_element_at` 逐字段同。
3. **e3d-io → TTY 形状和值对拍**（沙箱，Q5）：挑 5 个库（含最大一个、含 UDA 的一个、含 refno 复用 issue #27 的 ams7999）。
   对同一批元素导出 TTY `Q ATT` / `Q MEMBERS`，再与 e3d-io 落下的 `pe / ATT_<noun> / ATT_UDA / pe_owner` 比字段名、值、
   owner、边方向和成员序。旧 parser 与旧库快照不进入数据集；分歧逐条回到 `attlib.dat` / `*vir.dat` 和 TTY 裁决，结论进
   `docs/evidence/2026-09-xx-baseline-e3d-io-cutover.md`。
4. **耗时**：448 库整库基线总耗时与峰值内存，对比今天（`_7997_gen*.log` 一类有现成基准）。超 1.5× 触发 B8.3 第 2 条的批量写者。
5. **TTY 抽样**：每个样本库随机 50 个元素 `Q ATT` 对 `ATT_<noun>` 行、10 个有 UDA 的对 `ATT_UDA` 行、10 个 owner `Q MEMBERS` 对 `pe_owner` 边序。全部一致才算过；不一致的写进 evidence，哪边错修哪边。
6. **回归**：`live_ams8000_*` 与 `live_real_*` 那批 `#[ignore]` live 测试挑与基线相关的复跑；`baseline_parse_matches / baseline_parse_confirmed_empty` 的空库路径用一个 0 元素库跑一次。

### B9 依赖收口（0.5 人日，B8.5 全绿之后一次提交）

按 Q1 = A：

1. `Cargo.toml`：删 `parse_pdms_db` 依赖（:211）、`[patch."…/old-parse-pdms-db.git"]`（:325）、`legacy_parse_pdms_db` feature（:54）与它的整段注释（:42-53）、8 个 `[[bin]]` + 2 个 `[[test]]` 段；`LOCAL-DEPS` 注释里的 parse_pdms_db 句子；`pdmsdb_engine_v2` 随之离开 `Cargo.lock`（patch 关着 `cargo update -p parse_pdms_db --precise` 不适用，直接 `cargo generate-lockfile` 不行——用 `cargo update --workspace` 只动被摘掉的子树，diff 里核对只少不多）。
2. 删文件：8 个探针 `src/bin/*.rs`、`tests/full_attributes_real.rs`、`tests/pdms_record_boundary.rs`、`src/versioned_db/member_prune.rs`；`ele_data.rs:81-94` 的 `From<legacy>`。
3. 护栏翻面：`legacy_parse_pdms_db_guard` 三条 → 两条「manifest 一处都不许有 / 源码一处都不许有」（照 `legacy_pdms_io_guard` 抄）；`Check-LegacyParsePdmsDb.ps1` 第 1 条改 `did not match any packages`、2–4 条并成源码扫描；**进 CI**（`windows-tests.yml` 在 :117 旁加一步）。
4. 脚本：`Toggle-LocalDeps.ps1` 只剩 aios_core；`Assert-NoLocalPatch.ps1:9` 白名单删 `parse_pdms_db`、`pdms_io`。
5. 文档：`CONTEXT.md` 把「基线全量解析」词条改写（e3d-io 退化窗口、无 member_prune、sesno 口径）、新增「pe_owner」词条；两张退役清单各补一段「2026-09-xx 收口」；ADR-058 D3 加一句实施注记（基线走同一收集器）；`changelog.md` 分两条（B8.1 独立一条；B8.2–B9 一条）。
6. 记共识：`record_decision`「基线与增量共用 e3d-io 收集器与终态渲染器；parse_pdms_db 整体退出，不再有任何 legacy 解析器门」。

## 5. 验收总门

1. `cargo tree --workspace -e normal --locked -i parse_pdms_db` → `did not match any packages`；`-i pdmsdb_engine_v2` 同。
2. `rg 'parse_pdms_db|old-parse-pdms-db' Cargo.toml python/Cargo.toml src tests scripts` 只剩历史文档引用（`docs/`、`changelog.md`）。
3. `Check-LegacyPdmsIo.ps1` 四条 + 新 `Check-LegacyParsePdmsDb.ps1` 全绿，且两者都在 CI 里。
4. **一份收集、一份渲染**：`stream_baseline` 与 `collect_net_window` 共用 `enumerate_target_index / open_target / read_element`（源码钉）；`sync_total_async_threaded` 全文不含 `DbBasicData`。
5. B8.5 第 3 / 5 条：形状 diff 的每条分歧都有 TTY 裁决记录；TTY 抽样全部一致。
6. 增量路的 UDA 与 `pe_owner` 方向缺陷（F5 / F6）有独立的回归测试且已合入。
7. 448 库基线耗时 ≤ 今天 1.5×，峰值内存不随最大库线性增长（chunk 生效）。

## 6. 风险与对策

| 风险 | 对策 |
|---|---|
| F6 若真是渲染器写反了边，增量路已经写了一段时间的反向 `pe_owner` | B8.1 单独提交；修复后对已受影响的库走一次 `manual rebuild`（`POST /model/rebuild` 已有）而不是手改数据；evidence 记清受影响窗口 |
| 448 库里有成批 `shape_conflicts` / 白名单外解码失败，Q3 甲会让这些库基线失败 | 这是正确的（错数比空值更糟）；先在限定域跑，命中的 noun / 键收进 schema 修复清单（`2026-09-03-single-read-two-lanes-plan.md` §4 已有同一条） |
| 逐行 `UPSERT` 吞吐不够 | B8.3 第 2 条的批量写者，行形状仍共享；先量再做，不预先加 |
| Meta 相模板不齐（`glbvir.dat` 之类） | B8.0 第 4 条先查；缺的那种 `db_type` 在 B8.4 报「模板缺失」明确失败，不静默跳过；补模板是环境事项，与代码分开记 |
| `pe.sesno` 链头 0 → 真值，某个消费者暗依赖 0 | B8.0 第 2 条 grep 两仓；找到就先改消费者再切 |
| 沙箱形状 diff 被误用成判据（d-6 的红线） | evidence 模板里把「形状 diff」与「TTY 裁决」分两栏，前者只许写「发现 N 条分歧」，后者写结论 |
| 与启动重构 S3（CATA 退库）互相踩 `sync_total` | Q6 拍板定先后；两批不同时开 |

## 7. 不做 / 留档

- **不做**：`aios_core` 的 schema 权威搬家（`get_default_pdms_db_info` 仍是 `direct_attmap::to_named_attmap` 的定形依据，独立 ADR）；e3d-io watcher（已由 `db_file_registry` 收口）；模型车道 L1；CATA 退库 S3；python 绑定（已无 legacy 面）；MySQL 分支。
- **留档**：`legacy_session_ranges` 那段「链头会话记 0」的怪相只在 `changelog` 留一句，代码删干净；`full_attributes_real` 若要重生，是「e3d-io 读法 vs TTY `q att` 清单」的新测试，不是搬旧的。

## 8. 顺序与工作量

| 阶段 | 人日 | 前置 | 可并行 |
|---|---|---|---|
| B8.0 核实 | 0.5 | 无 | 与 B8.1 并行 |
| B8.1 渲染器补齐（UDA / 边方向） | 1–1.5 | B8.0 第 1 条 | 独立提交 |
| B8.2 `stream_baseline` | 2 | 无 | 与 B8.1 并行 |
| B8.3 基线写者 + 删 legacy 段 | 2 | B8.1、B8.2 | — |
| B8.4 接线 | 0.5 | B8.3 | — |
| B8.5 门 | 1–2 | B8.4；真库机器 | — |
| B9 依赖收口 | 0.5 | B8.5 全绿 | — |
| 合计 | ≈ 8–9 | | |

拍板 Q1–Q6 之后，B8.0 / B8.1 / B8.2 三条今天就能开；真正的长杆是 B8.5 要等真库机器。
