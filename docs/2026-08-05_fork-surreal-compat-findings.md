# fork-surreal-compat-suite 首轮结论（T0.1）

- 套件：`src/test/fork_surreal_compat.rs`（`cargo test --lib --features http_api test::fork_surreal_compat::`）。
- 双跑基建：嵌入式 mem 引擎（`mem://`，`kv-mem` feature）↔ 自起一次性 fork 服务器
  （`bin/surreal.exe` = `2.1.4+20250317.45013fc9`，与 Cargo.lock 锁定 rev 对账；rocksdb
  后端落临时目录；连接与生产同款 `Config::default().ast_payload()` + root 登录）。
- 运行开关：二进制缺失时双跑用例软跳过（打 `[compat] skip`）；`AIOS_COMPAT_REQUIRE=1`
  把缺基建变成硬失败（门槛式运行用）；`AIOS_COMPAT_WS` 指向外部服务器时不自起进程。
- 本轮结果（2026-08-05，本机实跑）：mem-only 3 条 + 双跑 5 条全绿。

## 等价性结论（暂存方案赖以成立的部分）

| 行为点 | 结论 |
|---|---|
| 启动 DEFINE 全套重放（`define_common_functions` 目录序 + project_hd 重放 `fn::room_code` + `define_dbnum_event` + 全部索引） | mem 与 fork 重放后 `INFO FOR DB` **一字不差**；`fn::room_num_of` 执行结果一致 |
| fn:: 覆盖顺序 | 目录序加载后 hh 版覆盖 hd 版、再由启动重放矫正回 hd 版——两引擎行为一致；hd 胜出由 `$uda_room` 标记断言钉住 |
| `INSERT RELATION` 撞 id（ADR-010 D13） | 两引擎都**静默保留旧行**；普通表 `INSERT` 撞 id 行为也一致 |
| 事务语义 | 块内语句失败整段回滚、`CANCEL`、`THROW` 中止、成功提交——逐语句结果一致 |
| record id 形制 | `⟨⟩` 转义 id、数组 id、`type::thing` / `record::id` / `<string>` 投射，经 ast_payload 连接与嵌入式引擎序列化一致 |
| schemaless 裸对象 | 嵌套对象 / 数组 / 点路径更新（`inst_geo.pts` 同款形态）一致 |

## 发现（读路由与 journal 设计的输入）

- **F1（生产缺陷，独立事项）**：`init_inst_relate_indices` 的
  `DEFINE INDEX idx_inst_relate_zone_refno ... TYPE BTREE` 在 2.1.4 语法上不合法
  （`Unexpected token TYPE`），生产代码 `let _ = SUL_DB.query(...)` 把解析错误吞掉——
  **该索引在生产库从来没建成过**，`zone_refno` 回填扫描一直在裸奔。修复语法或删语句
  是独立事项，本套件按生产现状 1:1 复刻（吞错）。
- **F2（T0.5 验收标准）**：`define_common_functions` 不做 `check()`，逐语句错误被静默
  丢弃（全新库上 `REMOVE FUNCTION` 不存在的函数报错、后续 DEFINE 照常生效——暂存库
  初始化恰好受益于此）。**StagedExecutor / journal validator 不得继承这个行为**：写回
  重放必须逐语句 `check()`，语句错误 = 整块失败。
- **F3（对拍口径）**：`surrealdb::Value` 的 `Display` 是给人看的简化渲染（字符串不带
  引号、record id 不带 `⟨⟩` 转义），字符串 `"pe:x"` 与记录 id `pe:x` 会渲染成同一串。
  一切机器对拍（本套件与 T5.2 终态对拍）必须走 serde 结构化序列化，不得用 Display。
- **F4（生产雷点，需业主核实，阻断项见下）**：`update_dbnum_event`（rs-core
  `define_dbnum_event`，`run_cli` 每次启动 OVERWRITE 定义）的事件体假定 pe 的
  record id 是数组（`array::at(record::id($value.id), 0)`）。实测（mem 与 fork
  服务器行为一致，双跑用例 `dual_update_dbnum_event_rejects_string_pe_ids_identically`
  钉住）：
  - fork 把 `pe:24381_100677` 解析为**字符串** id（不是数组、也不是数字）；
  - 事件在场时，字符串 id 的 pe 行任何 `UPSERT`/`UPDATE` 都因 `array::at` 类型
    错误**整条语句失败**，包在事务里则**整个事务失败**；
  - 数组 id 的历史行形制（`pe:['24381_100677', 5]`）正常触发、正常记账。
  而增量路径的 Add 正是 `UPSERT pe:{id} CONTENT ...`（pdms_io `io.rs:862`）——
  两者不能共存。基线/闭包路径用 `INSERT IGNORE INTO pe`（INSERT 不触发事件）
  所以无感。**需要业主在生产库上核实事件的实际在场状态与 dbnum_info_table 的
  新鲜度**（`dbnum_state` 本就把它当可缺失的遗留水位源）。在对齐之前：
  暂存库初始化（`init_staging_schema`）**不安装**该事件，生产 `run_cli` 维持原样。

## 后续接入点

- T0.3 暂存库建库初始化 = `replay_startup_defines` 这一套（本套件已在全新 mem 库上
  排练通过，`REMOVE FUNCTION` 的静默错误不影响最终函数集）。
- 新增全局扫描 / 修补语句时，把行为疑点加进本套件双跑（`assert_dual_same` 一步一拍）。

## 2026-08-07 增补（层级查询优化 P0，用例 `dual_inst_relate_anc_u64_contains_index_agrees`）

层级查询优化方案（`docs/plans/2026-08-07-inst-relate-anc-u64-hierarchy-query-plan.md`）
的 Go/No-Go 验证，`AIOS_COMPAT_REQUIRE=1` 下全套 11 条用例全绿：

| 行为点 | 结论 |
|---|---|
| `DEFINE INDEX ... COLUMNS anc`（数组列、普通语法） | mem 与 fork 均合法建成 |
| `WHERE anc CONTAINS <u64>` 的 EXPLAIN | 两引擎都走 `idx_ir_anc`（绝对断言，非仅对拍）——**主路线 Go** |
| `WHERE dbnum = n` 的 EXPLAIN | 两引擎都走 `idx_ir_dbnum`（备选路线地板同时成立） |
| `CONTAINSANY [a, b]`（多根合并查） | 两引擎结果一致（未做走索引断言，热路径是单根 CONTAINS） |
| `anc` 存 `i64::MAX`（RefU64 打包值天花板） | 写读往返保真，CONTAINS 命中一致 |

- **F1 已修**（同日）：索引语法去掉 2.1.4 不认的 `TYPE BTREE`，改
  `DEFINE INDEX IF NOT EXISTS ... COLUMNS zone_refno`，吞错改显式上抛；
  生产（`pdms_inst::init_inst_relate_indices`）与暂存
  （`init_staging_schema`）共用常量 `INST_RELATE_ZONE_INDEX_SQL` 一条语句。
  `dual_startup_define_replay_info_parity` 在修复后保持两引擎一字不差。
