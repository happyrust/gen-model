# 增量更新链路工作树审核（2026-08-04 第五轮）

- 审核对象：`gen-model` 增量更新链路的**当前工作树**（含未提交改动），
  以及 2026-08-04 两份同日文档（`2026-08-04_room-model-incremental-fix-report.md`
  修复清单、`docs/evidence/2026-08-04-incremental-update-live-revalidation.md`
  实机复验）之间的收口状态。
- 方法：静态源码审核 + git 历史核对。未起服务、未跑测试；工作树 15:27 的
  release 构建日志（`_zm1_build.log`）显示编译干净。
- 前几轮：07-26 链路内部（round1/2）、07-27 队列层（round3）与接口面、
  07-30 使用面（round4）。本轮聚焦**三件事**：① 8-04 修复清单是否真的落进代码；
  ② 8-04 实机复验发现的三个问题的代码现状；③ 工作树未提交改动（监控目录解析）
  本身的质量。

---

## 0. 结论摘要

「收口」列于 2026-08-05 回写；本文正文各节保留审核当时的描述，不改写，
以便对照修复前后的口径。

| # | 严重度 | 一句话 | 收口（2026-08-05） |
|---|---|---|---|
| V1 | — | **8-04 修复清单六条全部核实落地**（B2 / A1 / C2 / A2 / C1 / dbnum=0 覆盖），守护测试在位 | — |
| N1 | 高 | 实机复验问题一（SYS meta 基线恒差 1）**未修**：`count != parsed_count` 守卫原样，且 `PE=1` 纯根库连空基线分支都进不去 | ✅ **L0 已收口**（`111186b2`）：守卫改为 `baseline_parse_matches(pe, root, parsed)` 即 `pe - root == parsed`，根行显式数出来而非写死 +1；`PE=1` 纯根库走 `baseline_parse_confirmed_empty` 的空基线出口。**L2 实库复跑待做** |
| N2 | 高 | 实机复验问题二（失败基线次轮隐形为 up_to_date）**未修**：读路径 `dbnum_info_table` 回退仍在；提交 `5fbcb695` 只覆盖「整表缺失的就地升级」这一别的场景 | ✅ **L0 已收口**（`111186b2`）：`resolve_read_applied` 分两支——专用水位行存在时**不再**回退 info 表，只有整行缺失才回退；守护测试 `an_existing_failed_state_never_inherits_the_info_table_watermark`。**SC-002 逐库对拍（BL-04）待跑** |
| N3 | 高 | 实机复验问题三（空闲轮饿死后入队批次）**未修**：worker 主循环与 `drain_where` 无上限、无让位，2967 条积压仍会以一个合批挡住所有新批次 | ✅ **L0 已收口**（`4f46ebcc`）：空闲轮自分类 Settled/MoreWork/Failed（失败不再自唤醒成热循环）、阻断范围从全局收窄到按 dbnum、房间轮加 10 分钟地板；三件各配一条回退即红的测试。**长积压实库演练（QW-01）待跑** |
| N4 | 高 | **Gate 0 回归**：`Cargo.toml` 现钉 `aios_core = rs-core.git rev f9a66adb`，该版本 `File::with_name("DbOption")` 写死按 cwd 解析、**无 `DB_OPTION_FILE` 入口**（缓存里其他 4 个检出版本都有）。58 个 `#[ignore]` live 测试重新退回「只能换 cwd 变通」的状态 | ✅ **已收口**（`bba03d7a`）：升到 rs-core `7994051`（`f9a66adb` 的直接子提交，唯一改动就是加 `DB_OPTION_FILE`，缺省仍是 `DbOption`）。见 §3 补记 |
| W1 | 中 | 未提交的 `project_paths.rs`（11.8KB 纯逻辑新模块）**零单测**，违反本仓「每条修复配一条回退即红的测试」纪律（spec 001 FR-011） | ✅ **已收口**（`c35e4ece`）：模块随 14 条单测一并提交，覆盖 WD-01…06 全部断言 |
| W2 | 低 | `plan_watch_dirs` 的回退分支（`included_projects` 为空、拿 `project_dirs` 当名单）下，**相对路径条目永远解析失败**，且错误文案误导（报「对不上」，实际是回退分支不支持相对条目） | ✅ **已收口**（`c35e4ece`）：该分支改为 `base.join(...)` 直接解析相对条目；测试 `relative_entries_resolve_when_the_project_list_is_empty` |
| W3 | 低 | `collect_db_dirs_in` 的 `*000` 后缀判定会把 `ams1000` 这类目录也认作库目录（与旧实现同病，非回归） | ✅ **已收口**（`c35e4ece`）：测试 `a_directory_that_merely_ends_in_zeros_is_not_a_db_dir` 钉死反例 |

**一句话现状**：N1/N2/N3 的**纯函数半边**与 W1/W2/W3 已全部转绿，N4 已解开；
剩下的都是**实库半边**（BL-04 的 SC-002 对拍、QW-01 的长积压演练、各 live 系列），
它们此前全被 N4 挡着，现在可以按 `DB_OPTION_FILE` 定靶推进。

---

## 1. 已核实收口的（V1）

对照 `2026-08-04_room-model-incremental-fix-report.md` 逐条查证：

| 修复 | 代码证据 | 守护测试 |
|---|---|---|
| B2 房间泳道收敛后旧数字 | `TaskRegistry::set_detail` + `room_round` drain 后重新 `count_room_targets()` 写回 | `the_room_round_overwrites_its_detail_after_draining` |
| A1 静态资源缺失杀服务 | `web_service::serve` 降级告警 + `/health.static_assets` | `a_missing_asset_dir_degrades_instead_of_killing_the_service` |
| C2 收口失败给成功根记失败 | `drain_where` 收口失败 arm 不再 `record_failure` / `mark_failed`，只 `failures.push`（`model_update_pending.rs:925` 起） | `batch_settlement_failure_never_marks_generated_roots_failed` |
| A2 死信复活端点 | 路由 `POST /api/v1/update/pending-units/retry`（`web_service/mod.rs:243`）+ `handlers::pending_units_retry`（`handlers.rs:265`），202/404、原子 UPDATE、`BatchScheduler::wake()` | `a_manual_retry_revives_in_one_atomic_statement` |
| C1 ensure 生成前先落 durable pending | `ensure_regen_pending` 复用 `render_upsert`，替换只读 `current_regen_revision` | `a_durable_pending_row_is_written_before_generation_runs` |
| dbnum=0 覆盖真实库号 | `render_upsert` 非房间分支 `dbnum = dbnum?:0` | `an_enqueue_that_claims_no_dbnum_keeps_the_stored_one` |

另核实 spec 001 五个故事（副本冻库 / 类型替换阻断 / 级联丢引用者 / 派生根复活 /
CATA 标注）已全部提交（`bd816105`…`eb59bfd5` 一带），8 条回归测试在位。

正面结论保持有效：revision 收口仍无绕过路径；合批锁序两条批量路径仍一致。

## 2. 实机复验三问题的代码现状（N1 / N2 / N3）

### N1 · SYS meta 基线完整性恒差 1（审核当时：未修 → **已收口 `111186b2`**）

`manual_update.rs:2647-2656` 的守卫原样：

```rust
if let Some(parsed_count) = parsed_count
    && count != parsed_count
{
    anyhow::bail!("dbnum={} 基线不完整: PE={} 本次成功解析={}; ...");
}
```

`count` 数的是 `pe` 表行数（**含根/world 元素那一行**），`parsed_count` 是
`sync_total_async_threaded` 的返回（**不含根**）。四个 SYS meta 库全部恰差 1
（5100: 225/224；8191: 1229/1228）。并且 `PE=1` 的纯根库（5101）也失败：
空基线分支要求 `count == 0`（`manual_update.rs:2623`），根行让它永远不为 0。

**后果**：库里已有 PE 数据的 SYS meta 永远重建不出基线，每轮手动执行都多出
失败批次，这条路没有出口。修法两选一：守卫改为 `count != parsed_count + 根行数`
（把根行显式数出来，不要写死 +1），或让解析返回把根元素计入。修完必须配
「回退即红」测试。

### N2 · 失败基线次轮变 up_to_date（审核当时：未修 → 读路径**已收口 `111186b2`**；SC-002 待重测）

链条：失败的基线解析已写下 `dbnum_info_table` 行（如 `5100 → sesno=35`）→
下一轮 `resolve_migrated_applied_sesno(None, None, info_max)`（`dbnum_state.rs:431`）
回退出 `applied = 35 = file_latest` → 执行器判 up_to_date 不再入队；同一时刻
`GET /dbnums` 仍报 `initialized=false`。面板说没初始化、执行器说已最新。

提交 `5fbcb695`（seed compatibility watermark from stored data）**不是**这个问题的
修复：它处理的是「`dbnum_watermark` 整表不存在时的就地升级」，用 `pe.sesno`
最大值做一次性固化。而 N2 的场景是**表在、行在（扫描观察写的）、applied 为空**，
读路径照旧回退到 `dbnum_info_table`。

**根因**：`dbnum_info_table` 被两种语义共用——「旧版全量解析完成的水位」（迁移
可信）与「本次失败解析的中间产物」（迁移不可信）。修法方向：基线解析失败时
回滚/标记本次写入的 info 行，或迁移回退只认「有 `pe` 数据佐证」的 info 行
（与 `5fbcb695` 的 pe 口径统一）。修完 SC-002（`dbnum_statuses` 与 execute
逐库一致、差异为 0）才能变绿。

### N3 · 空闲轮饿死后入队批次（审核当时：未修 → **已收口 `4f46ebcc`**；长积压实机演练待跑）

`batch_worker.rs:128-142` 主循环仍是
`drain_queue_until_empty` → `idle_round`（无上限）→ `wait_for_work`。
`drain_where`（`model_update_pending.rs:880`）的 SELECT 无 LIMIT，attempts=0 的
积压全部并成**一个** `generate_roots` 合批。2967 条积压 = 一次跑数小时的空闲轮，
期间新入队的数据批次一步不动，面板只看到 `queued` 与 `worker_idle_secs` 上涨。

**修法方向**（三选一，或组合）：空闲轮积压消化分片（每片 N 个根，片间回头查
`freeze_next`）；`drain_where` 加 LIMIT + 多轮；空闲轮开始前 / 每片之间检查队列
非空即让位。注意保住两条既有不变量：合批同成同败 + 逐根回退定位坏根；
房间轮的先后次序（ADR-011 §8）。

## 3. Gate 0 可执行性回归（N4）

07-27 测试计划把「live 测试可用 `DB_OPTION_FILE` 定靶」列为 Gate 0（半天，
最高优先级）。当前 `Cargo.toml:72`：

```toml
aios_core = { git = "https://github.com/happyrust/rs-core.git", rev = "f9a66adb...", default-features = false }
```

该检出（`~/.cargo/git/checkouts/rs-core-.../f9a66ad/src/lib.rs`）7 处
`File::with_name("DbOption")` 全部写死按 cwd 解析，**没有** `DB_OPTION_FILE`；
同缓存里其余 4 个检出版本（06c9994 / 2916e6c / 7ce3120 / 9da12ef）都有。
即：换公开依赖（`0f5aabd5`）时钉住了一个**没有**环境变量入口的版本。

**后果**：全部 58 个 `#[ignore]` live 测试退回「换 cwd」变通；8-04 实机复验
也确实是靠改仓库根 `DbOption.toml` 的 `manual_db_nums` 跑的（测完要人工恢复，
证据文档末尾自己记了这笔账）。**处置**：把 rev 升到带 `DB_OPTION_FILE` 的版本，
或在 fork 上补那 3 行。这是新测试计划阶段一的第一个门。

### 补记（2026-08-05）：已解开

rs-core `7994051` 是 `f9a66adb` 的**直接子提交**，两者之间只有这一个提交
（`feat(config): DB_OPTION_FILE 环境变量定靶配置文件`）：

```rust
pub(crate) fn get_config_file_name() -> String {
    std::env::var("DB_OPTION_FILE").unwrap_or_else(|_| "DbOption".to_string())
}
```

缺省仍是 `DbOption`，现有服务与脚本零影响。

**升级不是改一行**：`parse_pdms_db` 与 `pdms_io` 各自也钉 `aios_core`，而
`parse_pdms_db` 在 `parse.rs` / `dict.rs` / `parse_explict_tools.rs` 共 41 处
把 `aios_core` 类型摆在 API 面上。Cargo 把同一 URL 的不同 rev 当**不同源**，
只升本仓会让依赖图里同时存在两份 `aios_core`，跨边界的类型对不上。
（`[patch]` 这条路也不通：Cargo 不允许用同一个源 patch 自己。）

因此三个仓一起动，顺序由依赖链决定（`pdms-io` 既直接依赖 `aios_core`，
又经 `parse_pdms_db` 间接依赖它）：

| 仓 | 提交 | 内容 |
|---|---|---|
| aios-parse-pdms | `65caaef` | `aios_core` → `7994051` |
| pdms-io | `1dd7fd4` | `aios_core` → `7994051`，`parse_pdms_db` → `65caaef` |
| gen-model | `bba03d7a` | 三个 rev 一起升 |

验证：依赖图收敛到**单一** `aios_core`；`cargo check --all-targets` 干净；
`cargo test --lib` **336 passed / 0 failed / 60 ignored**
（日志见 `output/logs/2026-08-05_gate0-*.log`）。

尚欠一步：Gate 0 的退出条件是「任取一个 `live_*` 用 `$env:DB_OPTION_FILE`
定靶跑通」，该运行时验证**未完成**——去跑时撞上同工作树另一处进行中的
`room_model` 重构（`PanelTreeCoverage` 尚未落地）导致编译不过，与本次升级无关。

## 4. 未提交改动审核（W1 / W2 / W3）

工作树 8 个文件、+95/-23 行，主题一个：**监控目录 / 项目根解析的唯一口径**
（新模块 `data_interface/project_paths.rs`），替换 `collect_db_dirs` 的
「一个项目失败拖垮整批 + `.unwrap_or_default()` 吞错」。

做对的部分（值得保留）：

- 逐项目容错，失败原因逐项目带出（`WatchDirPlan::problems/describe`）；
- `project_dirs` 支持绝对路径 / UNC 混排，`resolve_project_root` 统一口径，
  六个调用点（db_model / manual_update ×4 / cata_closure / database / probe）换齐；
- Windows 大小写 + 分隔符归一的 `path_starts_with`（修 `dirs_under` 用
  `Path::starts_with` 逐段区分大小写导致手动侧「一个候选都没有」的分家）；
- 监控目录为空时两处显式报警（重扫入口 + watcher 挂载），并区分
  「配置没解析出目录」与「目录都挂载失败」两种病，逐目录失败原因入错误信息；
- 根目录本身是 `*000` 时直接认（共享盘单独共享库目录的常见形态）；
- 目录去重走 canonicalize（防 8.3 短名 / 大小写别名触发同 dbnum 重复阻断）。

问题：

- **W1（必须补）**：全模块零测试。`normalize_path_input` / `is_absolute_input` /
  `resolve_project_root` / `join_project_entry` / `path_key` / `path_starts_with` /
  `plan_watch_dirs`（用临时目录）全是可以不连库测的纯函数。表驱动正反例见
  新测试计划 WD 系列。
- **W2**：`plan_watch_dirs` 在 `included_projects` 为空时拿 `project_dirs` 当名单，
  但 `resolve_project_root` 对相对条目要求「名字在 `included_projects` 里」——
  该分支下相对条目**恒返回 None**，报的却是「project_dirs 与 included_projects
  对不上」。要么让该分支支持 `base.join(entry)`，要么把文案改成真实原因。
- **W3**：`ends_with("000")` 会把 `ams1000` 认作库目录。旧实现同病、非本轮回归，
  但既然口径新立了名字，值得在谓词里排除（`len == 前缀+3` 或正则收紧）并加反例。

另有一处顺带改动：`fast_model/resolve.rs` 新增 `#[ignore]` live 测试
`scom_geometry_resolves_from_stored_reference_attributes`（BEND 的 SCOM.GMRE
从存储属性解析、不依赖 legacy `->GMRE` 边）——方向对，但它同样吃 N4 的
cwd 定靶问题。

## 5. 其余仍开的账（继承 07-30 round4，未变化）

| # | 项 | 状态 |
|---|---|---|
| A3 | ensure 超时/忙碌语义三处各说各话（spec 202 vs 实现 504/409 vs 客户端文案说反） | 未动；B1 接上前只影响 sweep 脚本，**必须赶在 B1 之前定案** |
| A4 半 | `/health.ref0_affiliation_conflicts` 未实现（static_assets 半边已修） | 未动 |
| A5-A9 / A10 | spec 全文修订（identity_mismatch、pending_units 字段、TaskId 格式、http_api feature、PLANT_UI_WEB_ROOT 缺省） | 未动，建议一次做完摘掉修订注记 |
| D1 | 词表「冻结吸收」无实现（后继排队行不被吸收终止） | 未动；要么补实现要么改词表 |
| D2 | 词表「模型变更通告」无实现（WS 只有 Tasks 主题） | 未动；plant-ui 的「陈旧标记」依赖它，是跨仓契约空洞 |
| B1 | plant-ui / 宿主无任何 `/model/ensure` 调用点（D-12 卡口） | 未动，在 plant-ui 仓 |
| B3/B4 | 宿主 rs-plant3-d 不带 MDB、丢错误 code | 未动，在宿主仓 |
| B5 | 批次 panic 的 `{"error": …}` 到不了面板 | 未动 |
| B6 | `/dbnums` 客户端 15s 超时 vs 大项目重扫 | 未动 |

## 6. 建议处理顺序

1. ~~**N4 依赖钉版**（半天内）：升 rs-core rev 或补 fork——它挡着一切 live 验证。~~
   → 已完成（`bba03d7a` + 上游 `65caaef` / `1dd7fd4`），见 §3 补记。
2. ~~**N2 失败基线隐形**（数据正确性 + SC-002）：与 N1 同一片代码，一起修。~~
   → 代码已修（`111186b2`）；**SC-002 逐库对拍仍待跑**。
3. ~~**N1 基线差 1**：修守卫口径 + `PE=1` 空基线出口。~~ → 已完成（`111186b2`）。
4. ~~**N3 空闲轮饿死**：分片 + 让位……~~ → 代码已修（`4f46ebcc`，三件各配回退即红
   的测试）；**长积压实库演练仍待跑**。
5. ~~**W1/W2 project_paths 补测**~~ → 已完成（`c35e4ece`，14 条单测，W3 一并修）。
6. A3 定案 → B1 接线（跨仓，排进 plant-ui 侧计划）。**仍开**。

第 1–5 条都在 `docs/2026-08-04_data-model-queue-test-plan.md` 里有对应的
测试用例编号（BL / WD / QW 系列），修复与测试同批落地。

**2026-08-05 之后的剩余队列**（按依赖顺序）：

1. Gate 0 的运行时验证：用 `$env:DB_OPTION_FILE` 定靶跑通一个 `live_*`。
2. BL-04（SC-002 逐库对拍差异归零）与 BL-01…03 的 L2 实库复跑。
3. QW-01 长积压演练：M-B1 导出的 2967 条欠账正是天然夹具。
4. A3 ensure 语义定案 → plant-ui B1 接线 → D-12 解封。
   注意 plant-ui 侧有 `eye_dispatch_does_not_call_the_model_generation_api`
   把「眼睛图标不得触发生成」钉死了，接线时要改成「显式入口才触发」而不是删掉它。
5. plant-ui 补 `POST /update/pending-units/retry` 调用点（服务端 A2 已就绪），
   补齐 QW-02 / Q-13 的界面半边。
