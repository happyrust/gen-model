# 2026-08-04 兼容水位播种：审核、加固、演练与统计缺口排查全记录

一句话：审核 `5fbcb695`（兼容场景下 applied 水位以当前数据库 pe 最大 sesno 为基线）确认
需求实现正确，随后分三轮把审核发现全部加固落地（`240fc2ac`、`6a50b1e5`、`6c49e4b3`），
用一次真实升级演练闭环验证，并把演练告警牵出的 dbnum_info_table 统计缺口排查到底
（结论：历史一次性事件，现行链路健康）。另附对同日另一会话两批未提交改动的审核与修补。

## 成果索引

| 提交 | 内容 |
|------|------|
| `240fc2ac` | fix(incr): 空水位表与缺表同判，播种走 pe 源并补过程日志 |
| `6a50b1e5` | fix(incr): queue_control 播种完成标记（可恢复播种）+ count 完整性比对告警 |
| `6c49e4b3` | fix(incr): 事件定义收敛 + 读回自证 + rebuild_dbnum_stats 修复入口 |
| 工作区修补 | 监控目录批次 2 个中危点、RVM 批次 3 个低危点（属另一会话的未提交工作，未代提交） |

演练与排查产物：`output/watermark-drill/`（before/after/restored 快照、restore.surql、
Make-RestoreSql.ps1、演练用 DbOption.toml、运行日志）。

## 1. 审核对象与结论

`5fbcb695 fix(incr): seed compatibility watermark from stored data`
（`src/data_interface/dbnum_state.rs` + ADR-001）：

- `ensure_increment_state_storage()` 先 `INFO FOR DB` 判断 `dbnum_watermark` 是否存在，
  **之后**才幂等建表——判据不被本次建表污染。
- 整表缺失 = 在已有业务数据上兼容启用增量：按 `pe` 每个 dbnum 的 `math::max(sesno)`
  建立 `applied_sesno`，以当前数据库实际内容为基线，而不是拿现场文件最新会话冒充。
- 回填一律 `applied_sesno = applied_sesno ?? {sesno}` 填空，绝不覆盖已建立水位；
  优先级 applied > 旧 sesno 字段 > info/pe，与 ADR-001 文档同步。
- 与正常初始化解析路径（`initialize_dbnum_baseline` → `finalize_baseline` 事务收口，
  水位取解析时文件 latest sesno）口径一致、互不干扰。
- 播种的最小行（只有 dbnum+水位、无文件身份）与扫描体系兼容：preview 的 Missing
  判定有 `!file_path.is_empty()` 守卫，`classify_scan` 过滤空 db_type/path，
  身份由首次 record_scan 回填（演练中实证）。

审核发现（均已在后续提交中处置）：

1. 【中】播种非原子：判定 → 建表 → pe 全表聚合（大库慢）→ 分块 UPSERT，中途死掉留下
   空表或半途表，重启后源切到 `dbnum_info_table`；info 缺失/陈旧的老库以 0 水位被
   `needs_initial_load` 判成首次导入，整库重解析。
2. 【低】pe max 语义边界：尾部纯硬删除会话会低估（重放幂等、仅浪费）；老系统全量解析
   中断留下的洞会高估（增量永远补不回，需完整性比对暴露）。
3. 【低】老 pe 行可能整体没有 sesno 字段（predate per-element session tracking）——
   过滤后不播种、走首次导入，是 ADR「不得猜测范围」的正确兜底。
4. 【低·运维】pe 全表聚合期间无过程日志，慢启动会被当成卡死。
5. 【提示】每次启动重跑 info 源回填（旧行为）：删水位行不能当「强制重新初始化」用，
   重置水位的正确姿势是把 applied_sesno 改小。

## 2. 三轮加固

### `240fc2ac`：空表与缺表同判 + 过程日志

- `should_seed_from_current_database`：表缺失**或行数为 0** 都走 pe 源；行数在建表与
  任何写入之前取（`count_watermark_rows`）。空表没有已建立水位需要保护，同判无损。
- pe 源播种前打开始日志（区分缺失/为空，提示大库耗时），结束统一打源名、dbnum 数、耗时。

### `6a50b1e5`：完成标记 + 完整性比对

- `queue_control:watermark_seed` 标记只在全部播种批次成功后落下；缺失就（重）跑 pe 源。
  覆盖「分块 UPSERT 部分完成后死掉」（行数 > 0，空表判定失效）与「首次升级到带标记
  版本」两种无法从表内容区分的状态。重跑 fill-only 幂等，每库至多多一次 pe 聚合，
  之后回到 info 快路径。标记写失败只提示不阻断（后果仅是下次再播一遍）。
- 播种前按 dbnum 比对 `count(pe)` 与 `sum(dbnum_info_table.count)`（与基线路径
  `baseline_stats_need_rebuild` 同口径），对不上打告警不阻断；统计表整体为空只给一条
  整体提示；比对自身失败也不阻断。
- live 验证：首跑正确识别「标记缺失」、告警首秀当场抓到 dbnum=8000 pe 14178 != 统计
  14176；二次启动 0.0 秒走 info 快路径；已有水位零覆盖。

### `6c49e4b3`：事件收敛 + 读回自证 + 修复入口

- 删除 `define_dbnum_event_array_id` 死代码（对 string 形态 pe id 解析恒 NONE）。
- `define_dbnum_event` 定义后 `INFO FOR TABLE pe` 读回，校验事件体含 `string::split`
  指纹（`verify_dbnum_event_definition`），不含则打启动告警——多服务混跑同一库、
  谁最后启动谁 OVERWRITE，这是启动日志里唯一能看见「事件被换成坏版」的地方。
- `rebuild_dbnum_info_from_pe` 提升 pub，新增 `rebuild_dbnum_stats` bin：
  完整性告警报哪个库就修哪个（身份从水位表优先取、退而继承统计行，重建前后打对比）。
  对 8000 实跑：`统计 14176->14178 条，与 pe 一致`，历史缺口消除。

## 3. 升级演练（site-8000-incrtest，ns=1516 / AvevaMarineSample）

步骤与结果：

1. 表级备份（服务运行中冷拷 RocksDB 目录不安全）：24 行全字段 JSON + 可回放
   `restore.surql`（datetime 统一还原 RFC3339）。
2. `REMOVE TABLE dbnum_watermark;`
3. 用含修复的新构建二进制在独立 cwd（`output/watermark-drill/`，配置副本仅关
   gen_spatial_tree 与 web 监听）启动——与真实服务同一启动路径。
4. 播种日志按预期两行；**6/6 dbnum 的 applied_sesno == max(pe.sesno)**
   （5052→0、5100→35、5101→7、8000→34、8191→169、251047→6），身份字段为空（预期）。
5. 恢复：24 条 UPSERT 回放 + 删除播种新增的 5052 行，与演练前全表 8 字段比对**零差异**。

演练顺带实锤两件事：

- **info 表确实会陈旧**：8000 pe max=34，info 只到 32（滞后当日增量 2 个会话）。
  按旧 info 源播种会低 2 个会话导致重放；pe 源取到正确的 34。
- **REMOVE TABLE 在生产是有损操作**：7998 原有 applied=18 但该库 pe 无数据，播种后
  水位与全部文件身份登记一并丢失，只能靠备份恢复。生产重建水位表前必须照本演练
  导出 restore.surql。

并发插曲（如实记录）：16:18 起 plant-ui 后端（15:27 用本仓代码构建）与演练同库共存；
17:15:41 它应用了 8000 的 (34,36] 窗口把水位推到 36——演练恢复的 34 水位没有阻碍其
正确收敛（窗口重放幂等语义的一次意外实战验证）。

## 4. dbnum_info_table 统计缺口排查

现象：8000 的统计 count 少 2、sesno 停 32，而 pe 已到 34；缺的正是当日上午 10:03
增量窗口 (31,34] 写入的 `pe:24384_22439`（BEND）与 `pe:24384_22404`（BRAN）。

四组定性实验（当前库，测后清理）：单条 `UPSERT pe CONTENT` 新建 ✓、事务内 Add 语句
组合 ✓、旧版形态 `INSERT INTO pe [...]` ✓、真实窗口 17:15 (34,36] ✓——**全部正确驱动
统计表**。时间线考古确认 10:03 的执行就发生在当前数据目录上（RocksDB IDENTITY 7/29
创建，09:38 与 13:51 各重启一次 surreal），但当时的进程/二进制/事件定义状态已不可回溯。

结论：**现行增量写入路径与事件机制没有 bug**；缺口是历史一次性事件（最可能是当时库里
的事件定义处于缺失或坏版本状态，后续启动的 OVERWRITE 已自动修复）。排查中坐实的三个
系统性隐患已在 `6c49e4b3` 处置其二（收敛 + 自证 + 修复入口），其三见下方遗留。

关键机制事实（排查副产品，供后人省力）：

- 统计表只有两个写入方：pe 上的 `update_dbnum_event` 事件（增量维护，含 updated_at）
  与 `rebuild_dbnum_info_from_pe`（全量重建，不含 updated_at）。
- 事件只做增量维护：sesno 缺口会被后续窗口抬平，**count 缺口永不自愈**，只有 rebuild
  能纠正。
- 统计表不是水位（ADR-001）：缺口对增量正确性零影响，影响面限于 info 回填的兼容路径
  与统计展示——这也是播种源从 info 换成 pe 的追认理由。

## 5. 对另一会话未提交改动的审核与修补

### 监控目录解析批次（project_paths.rs 等 8 文件）

总评：质量很高，方向正确。逐项目容错解析、UNC 混排、F6 判重键升级为（归属项目, dbnum）、
归属取自监控目录、MountState 失联/恢复生命周期、`watch_dirs()` 启动∪新发现——每处都有
实测事故注脚与测试。已代修 2 个中危点（编译通过、未代提交）：

- `warn_unattributed_once`：归属退化告警上 stderr、按目录去重——退化会让 F6 判重键
  回到「主项目 + dbnum」，跨项目同号 sys 库（8191）重新互相误伤，必须现场可见。
- 补扫子集化：`sweep_watch_dirs` 拆出 `sweep_dirs(dirs)`，重挂轮只补扫刚恢复的目录，
  事件 select 循环的阻塞窗口从「整面扫（网络盘分钟级）」缩到「新恢复目录」。
  源码钉子测试的两个断言复核仍成立。

未动的提示级：path_key 的 ASCII 大小写折叠、refresh_health 单次 is_dir 抖动降级、
headers 快照消费点确认。

### RVM 基线验证批次（rvm_baseline/ + rvm_verify）

总评：设计干净（feature 门控、快照契约、ATT 关键洞察），0 中高危。已代修 3 个低危点：

- `M_TO_MM` 注释补边界：几何坐标（rvm-rs 换算成米）要乘回，`group.translation`（CNTB）
  RVM 原生即毫米不乘——已用真实快照核对，防后人怀疑漏乘。
- `full_noun_to_short` 补 `SUPPORT => SUPPO`、`HANGER => HANG`（截 4 兜底对 5 字
  短名词失效）。
- 纯函数单测 7 个（identity 5 + att 2，按仓库规约只写不跑）。

留给 compare 实装时权衡：root_name 回退链对多根导出的语义、path 分段歧义。

## 6. 遗留事项

1. **rs-core 仓库**还导出着 array-id 版 `define_dbnum_event`（对 string 形态 id 静默
   失效），本仓库删不到；已靠读回自证兜底，建议在 rs-core 删除并更新依赖 rev。
2. 三个提交（`240fc2ac`、`6a50b1e5`、`6c49e4b3`）尚未推送远程。
3. 工作区的两批修补混在另一会话的未提交改动里，由该会话统一提交。
4. `output/watermark-drill/` 产物可作为生产重建水位表的标准操作模板，暂留。
