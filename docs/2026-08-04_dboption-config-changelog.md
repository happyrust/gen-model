# DbOption.toml 配置变更清单（基线 2025-06-30 → 2026-08-06）

## 背景

`DbOption.toml` 最后一次入库改动是提交 `7bba5fd4`（2025-06-30，“实现P0级并行优化和完整文档体系”）。
其后 2026-07 下旬开始的增量更新 / 按需生成 / Web 服务等工作只改了工作区文件，一直未提交，
配置语义已与基线明显分叉。本文以 `7bba5fd4:DbOption.toml` 为基线，对照当前工作区版本，
逐项记录**新增键**与**取值变更**，并给出各变更的引入日期（依据同目录带日期的备份文件
`DbOption.toml.bak-7997gen-20260727` / `DbOption.toml.bak-empty127-20260727-143519` /
`DbOption.toml.bak-aabbidx-20260729` 与文件内注释交叉推定）。

文件内的对应位置均已就地加注 `【新增 日期】` / `【变更 日期】` 标签。

## 一、新增配置键（基线中不存在）

| 键 | 当前值 | 引入日期 | 说明 |
|---|---|---|---|
| `http_api_addr` | `"0.0.0.0:8022"` | 2026-07-27（初值 `0.0.0.0:8021`） | Web 服务（REST + WebSocket）监听地址；未配置则不启动，需编译 `http_api` feature。随手动更新 / 按需生成 Web 服务引入（`docs/specs/web-service-api.md`、ADR-011）。2026-07-29 因 8021 端口被占改为 8022。消费方：`src/web_service/mod.rs` |
| `http_api_cors` | `["*"]` | 2026-07-27 | Web 服务 CORS 允许来源列表，`["*"]` 全放行（局域网前端联调）。与 `http_api_addr` 同批引入 |
| `delivery_unit_types` | 未启用（注释） | 2026-07-25 | 最小交付单元类型——增量 / 手动 / 按需生成共用的“生成根”口径；配置后**完全取代**默认集合 `["BRAN","HANG","SUPPO","EQUI"]`，`[]` 表示不使用交付单元。trim + 大写 + 去重，拒绝 WORL/WORLD/SITE/ZONE 与 FTUB。随批量根重生成能力引入（ADR-012、`docs/specs/manual-model-update.md`）。消费方：`data_interface/generation_root.rs` |
| `append_delivery_unit_types` | 未启用（注释） | 2026-07-25 | 在默认交付单元集合之外**追加**类型，仅当未配置 `delivery_unit_types` 时生效。规则同上 |
| `room_key_word` | `["-RM"]` | 2026-07-29 | 房间名关键词**列表**，用于房间-面板关系匹配（`fast_model/room_model.rs::get_room_key_word`）。**替代旧键 `room_keyword`（单字符串，已废弃删除）**：一次支持多关键词，且 AMS 房间名（如 `/1RX-RM03-R301`）末段由 `project_hd` 规则校验 |

## 二、取值变更（键在基线中已存在）

| 键 | 基线值 | 当前值 | 变更日期 | 原因 |
|---|---|---|---|---|
| `sync_live` | `true` | `false` | 2026-07-27（07-29 曾临时回 `true`，08-04 复关） | 生产增量改为“启动重扫后持续监听”；Web 冒烟验证期间关闭启动期全量同步 |
| `replace_dbs` | `false` | `true` | 2026-07-27 | 增量重解析允许覆盖既有库文件记录 |
| `project_path` | `D:/AVEVA/Projects/E3D2.1` | `D:/AVEVA/Projects/E3D3.1` | 2026-07-27 | AMS 样例工程迁移到 E3D3.1 项目目录 |
| `included_projects` | 含 `"SCB"` | 移除 `"SCB"` | 2026-07-27 | E3D3.1 下未部署 SCB，避免扫描报错 |
| `gen_model` | `true` | `false` | 2026-07-27 | 非 CATA 增量监听模式：启动不做全量重算，增量/按需路径内部自行强制 `true` |
| `gen_mesh` | `true` | `false` | 2026-07-27 | 同上 |
| `gen_spatial_tree` | `false` | `true` | 2026-07-29 | 配合房间增量与空间索引（aabbidx）验证启用 |
| `gen_model_batch_size` | `16` | `4` | 2026-07-29 | ams7997 全量生成 16 并发在 `save_instance_data` 撞 RocksDB 事务写冲突（empty127 轮） |
| `debug_refno_types` | `["CATA","LOOP","PRIM"]` | `["LOOP","PRIM"]` | 2026-07-27 | CATA 改走闭包按需解析（`cata_closure`），不再全量调试解析 |
| `manual_db_nums` | 注释（未启用） | `[7997, 7998, 7999, 8000]` | 2026-07-27 起启用 | 各轮实测窗口：07-27 `[7997,7999,8000]` → 07-29 `[8000]` → 08-04 `[7998,8000]`（7998 为 18 会话最小设计库，8000 验 up_to_date 分支） → 08-06 `[7997,7998,7999,8000]`（issue #10 的 E3D 实测窗口，重新纳入 7997/7999；跑完可收窄回 `[7998,8000]`） |
| `room_keyword` | `"-R-"` | **键已删除** | 2026-07-29 | 被 `room_key_word`（列表）取代，见上表 |
| `debug_root_refnos` | `[]` | 注释掉 | — | 清理：缺省即为空，无语义变化 |
| `incr_sync` | `false`（多一个空格） | `false` | — | 仅格式整理，无语义变化 |

## 三、时间线速览

- **≤ 2026-07-27**：切换 E3D3.1 / 移除 SCB；关闭启动期全量生成（`gen_model`/`gen_mesh`→false）与 `sync_live`；`replace_dbs`→true；`debug_refno_types` 去 CATA；启用 `manual_db_nums`；新增 Web 服务键 `http_api_addr`(8021)/`http_api_cors`；（07-25 前后）交付单元键随 ADR-012 引入。
- **2026-07-27 → 07-29**：`http_api_addr` 8021→8022；`gen_spatial_tree`→true；`gen_model_batch_size` 16→4；`room_keyword`→`room_key_word=["-RM"]`。
- **2026-08-04**：`sync_live` 复关（Web 冒烟验证）；`manual_db_nums` 调至 `[7998, 8000]` 增量实测窗口。
- **2026-08-06**：`manual_db_nums` 放宽至 `[7997, 7998, 7999, 8000]`，重新纳入 issue #10 取证涉及的 7997（基线库 `.surreal/ams-7997-e3d-test-20260805`，applied=92）与 7999（此前被排除，applied=3、file=41）。
- **2026-08-06（增量范围只认 MDB）**：`manual_db_nums` / `exclude_db_nums` / `only_sync_sys` **不再参与增量判定**——增量范围只由 `mdb_name` 那个 MDB 声明的 DESI 决定（`data_interface/update_scope.rs`），代码上 `should_process_database` 并入 `increment_manager::in_scope_with`。起因正是上一条：手写名单挡掉 7999 时，日志与「MDB 里没有这个库」一模一样，现场只看得到每 30 秒重复一次的「不在本期执行范围，跳过数据库: 类型=DESI, 编号=7999」。这三个键仍对全量模型生成与按需基线解析生效，因此上表里 `manual_db_nums` 的取值仍有意义，只是**不再限制增量跑哪些库**——本地站点从此按 `/ALL` 声明的 29 个 DESI 全量增量。
- **2026-08-06（`gen_spatial_tree` 开关治理）**：结论钉死——开关保留，但角色从「功能形态开关」转为**运维开关**（止血降级 / 演练隔离），所有配置的默认姿态为 `true`。备用配置 `DbOption-ams.toml` / `DbOption-zsy.toml` / `DbOption_text.toml` 残留的 `false` 一并翻正，消除「整份拷回把 false 带回来」的 issue #7 复发路径。`load_spatial_tree` / `save_spatial_tree_to_db` 确认为**死键**（rs-core 仅定义、本仓与 rs-core 均零读取），但它们是必填 bool 字段，从 toml 删键会让 config 反序列化报 missing field 起不来——因此只就地标注、不删，待 rs-core 删除字段后随之移除。同批记录默认值改进方向：rs-core `DbOption` 的 bool 缺省为 false，「缺键=关」与「默认需要空间/房间计算」相悖，下次动 rs-core 时应将 `gen_spatial_tree` 缺省翻为 true（或连带清理死字段）。
- **2026-08-07（`gen_spatial_tree` 使用移除）**：推翻 08-06「保留为运维开关」的结论——空间/房间计算恒开启，代码不再读这个键。拆掉的门：启动树加载与启动全量房间重建（`lib.rs` 两处 + `run_app` 一处）、AABB 刷新的 `maintain_spatial_tree` 快速路径（`update_inst_relate_aabbs_by_refnos` 收并为单函数）、房间入队口（`enqueue_room_recalc` 不再收 `DbOption`）、暂存窗口房间语义/merge 门与房间轮早退（`batch_worker`）、暂存房间报告早退（`model_update_pending`）、`/health` 的 `gen_spatial_tree` 字段。相应的四条「回退即红」源码钉子一并退役。键本身降级为**死键**（与上一条的两个键同款处理）：必填 bool、删键起不来，待 rs-core 删字段后随之移除；运维止血从此只能靠 `AIOS_SKIP_STARTUP_ROOM_BUILD`（只跳启动全量重建，不冻结增量）。

## 四、注意事项

- 本文件描述的是 **AMS 本地站点工作区**的配置状态，多数取值属于实测窗口调参（如
  `manual_db_nums`、`sync_live`），部署到其它站点时应按站点实际情况取值，不应照抄。
- `http_api_addr` / `delivery_unit_types` / `room_key_word` 三组新键是**功能性配置**，
  部署新站点时需要显式决策；其余多为运行窗口开关。
- `gen_spatial_tree` 自 2026-08-07 起为**死键**：代码零读取，空间/房间计算恒开启。
  键仍必须留在 toml（必填 bool，删键起不来），取值不再有任何效果；快速重启可用
  `AIOS_SKIP_STARTUP_ROOM_BUILD` 环境变量跳过启动全量房间重建（增量照常）。
