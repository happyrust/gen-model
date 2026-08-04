# 增量更新实机复验（2026-08-04）

环境：`aios-database` 0.1.4，用工作树当前源码重编（含 `manual_update.rs` /
`increment_manager.rs` / `dbnum_state.rs` 在 8/3 20:30 的未提交改动，约 400 行，
对应 `specs/001-incr-update-integrity-fixes`）。SurrealDB 2.1.4（`bin/surreal.exe`）
监听 `127.0.0.1:8009`，数据后端是 `.surreal/site-8000` 的**一次性副本**
`.surreal/site-8000-incrtest`，原库未动。服务 `http://127.0.0.1:8022`，
`sync_live=false`（只跑手动路径），`manual_db_nums = [7998, 8000]`。
客户端 `plant-ui-app.exe`（0.1.4 发布包）经 `PLANT_MODEL_API_URL` 连同一个服务。

## 结果

| 范围 | 结果 | 证据 |
|---|---:|---|
| `preview` → `execute` → 入队 → 冻结 → 应用 → 推水位 | 通过 | `_incrtest_8000_preview.json` / `_incrtest_8000_execute.json` / `_incrtest_8000_task.json` |
| 8000 库 31–34 窗口重放与 2026-07-27 基线一致 | 通过 | 交付单元同为 BRAN `24384/22404`、`24384/22441` |
| 交付单元根解析（最近 MDU 祖先） | 通过 | 2 个 BRAN 根，`will_generate=true`，`no_generation=0` |
| ZONE 归并 | 通过 | 两个单元归到 ZONE `24384/22400`（`/1RX03-LCT`） |
| 模型生成落库 | 通过 | `inst_relate` 由 0 变为 51；`model_update_pending` 收口为 0 |
| 空库基线（7998） | 通过 | 0 元素、0 单元，水位仍推进到 18（合法空基线分支） |
| 失败批次不推水位 | 通过 | 四个 SYS meta 库 `applied_sesno` 保持为空 |
| plant-ui 对连 | 通过 | 模型树 4 个 SITE；日志「三维模型已就绪：49 个元素，55 个网格实例」；任务队列面板读到同一份队列共 6 条、失败行标红 |
| SYS meta 基线初始化 | **失败** | 见问题一 |
| `dbnum_statuses` 与 `execute` 口径一致（SC-002） | **不一致** | 见问题二 |

### 8000 库 31–34 窗口

预览解析出 4 个会话、净 3 处修改、0 新增 0 删除，`model_affecting=3`：

| sesno | added | modified | deleted |
|---:|---:|---:|---:|
| 31 | 0 | 2 | 1 |
| 32 | 0 | 3 | 0 |
| 33 | 0 | 1 | 0 |
| 34 | 0 | 1 | 0 |

交付单元 2 个，`old_owner` 与 `new_owner` 相同（无搬迁）：

| 生成根 | noun | modified | will_generate |
|---|---|---:|---|
| `24384/22404` | BRAN | 1 | 是 |
| `24384/22441` | BRAN | 2 | 是 |

批次终态 `succeeded`，`changed_elements=8`，`units_done=2/2`，两个单元
`status=generated`、`attempts=0`、无 warning，耗时 2 分 15 秒
（10:03:28 → 10:05:43）。结束后 `applied_sesno=file_latest_sesno=34`。

这四个会话正是 `2026-07-27-projams-incremental-update-validation.md` 里记录的
FTUB 跨 BRAN 移动与重排窗口，该文档预期生成 BRAN 22404 与 22441——本次重放结果
逐项吻合。

## 问题一：SYS meta 基线完整性校验恒差 1，导致永远初始化不了

四个 SYS meta 库全部以同一口径失败：

```
dbnum=5100  基线不完整: PE=225  本次成功解析=224 ; 不推进 applied_sesno
dbnum=8191  基线不完整: PE=1229 本次成功解析=1228; 不推进 applied_sesno
```

`5101`（PE=1）与 `251047`（PE=13）同样失败。每一例都**恰好差 1**，看起来是根 /
world 元素在 `pe` 里占一行、而在「本次成功解析」的计数里不占。判据在
`manual_update.rs` 里 `count != parsed_count` 那道守卫。

后果：只要库里已经有 PE 数据，SYS meta 就再也重建不出基线，每次手动执行都会
多出四条失败批次。`applied_sesno` 不推进这一点是对的，但这条路没有出口。

## 问题二：失败的基线会让该库在下一轮变成 `up_to_date`，失败自此隐形

第一轮执行时四个 SYS 库被判为需要初始化并入队（随后失败）；**第二轮执行它们被
计入 `up_to_date=5`，压根不再入队**。原因是失败的那次已经写下了
`dbnum_info_table` 行（如 `5100 → sesno=35 count=225`），而
`resolve_migrated_applied_sesno(existing_applied, legacy_sesno, info_table_max)`
在 `applied_sesno` 为空时会回退到 `dbnum_info_table` 的最大 sesno，于是解析出
`applied = 35 = file_latest_sesno`。

同一时刻 `GET /dbnums` 仍然报 `applied_sesno=0, initialized=false`。也就是说面板
说「没初始化」、执行器说「已经最新」，两边对同一份磁盘现状给出相反结论——正是
spec 001 SC-002 要求「逐库一致、差异数为 0」的那一条，当前差异为 4。

## 问题三：空闲轮会把后入队的数据批次饿死

第一次执行的 5 条批次在队列里全部 `queued` 卡了 4 分钟没动，`worker_idle_secs`
一路涨到 351。原因是 worker 启动时队列为空，直接进了空闲轮去消化 **2967 条模型
欠账**（8000 库 7/29 基线留下的，全部 `attempts=0`，因而被当成一个合批一次性
生成）；而 `batch_worker` 的主循环是 `drain_queue_until_empty` → `idle_round`
串行、且 `idle_round` 无上限——它不跑完，新入队的批次一步也动不了。

把那 2967 条导出存档后清空，队列在 3 秒内跑完。手动增量在积压面前没有优先级，
这一点面板上也看不出来（队列显示 5 条 `queued`，健康检查显示 `worker_alive=true`）。

## 环境备注（与本次结论无关，但会挡住后来人）

`.surreal/ams-8009`（1.57 GB）与 `D:\backup-dbs\ams-8009.db` 都已经**无法被
SurrealDB 2.1.4 打开**：

```
Corruption: Corrupt or unsupported format_version: 7 in .surreal\ams-8009/000150.sst
```

SST 是 format_version 7，只有更新的 RocksDB 才会写——即 readme 与
`Start-Surreal8009.ps1` 反复警告的「PATH 上的 3.x」确实打开过这个目录。最早可追到
`_ams_surreal.log` 2026-07-30 17:32 的同一条报错。本次因此改用 `site-8000`。

## 产物

- `_incrtest_8000_preview.json`：8000 库 31–34 的完整预览（会话、单元、ZONE）
- `_incrtest_8000_execute.json`：入队回执
- `_incrtest_8000_task.json`：批次终态与两个单元的生成结果
- `_incrtest_7998_preview.json` / `_incrtest_7998_execute.json`：首轮五库入队与空库基线
- `_incrtest_pending_backup_8000.json`：清空前导出的 2967 条模型欠账

本次未改任何产品代码。为跑通测试改动的两处配置需要在后续恢复：
`DbOption.toml` 的 `manual_db_nums`（原 `[8000]`），以及发布包
`pc/DbOption.toml` 的 `mdb_name`（原 `ALL1`）。
