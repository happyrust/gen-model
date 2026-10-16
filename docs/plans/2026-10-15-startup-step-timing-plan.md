# 启动步骤打点统一：每步干什么 + 耗时，控制台与日志都可见

> **状态：已实施并实测通过（2026-10-16）**。mem 部署形态（`enable_log=true`）实跑：
> `log-init → store-connect → surql-embedded → db-manager → mdb → http →
> route-directory → spatial-room → session-baseline` 全段配对，dblog.txt 与控制台
> 双出口均在；`[root-scan]` 仅 2 次且都在 `stage=mdb` 区间内；remount 节拍越拍
> 无新增扫描；`/health` 200。
>
> 2026-10-15 · 关联 ADR-083/085（介质分臂）、lazy-startup 计划（启动分层）
> 动机：初始化排障/优化时，「哪一步在干什么、吃了多久」目前散在三种写法里——
> read-through 臂有 `[startup] stage=X event=begin/done elapsed_ms=N`，run_cli 臂
> 是零散 `println!("…耗时 {}")`（只上 stdout，不进 dblog.txt），还有一批步骤完全静默。
> 统一成一套机器可读打点，两行（begin/done）贯穿每条初始化步骤。

## 约定

```
[startup] stage=<kebab名> event=begin <这步干什么，一句话中文>
[startup] stage=<kebab名> event=done elapsed_ms=<毫秒> [detail]
[startup] stage=<kebab名> event=skip <原因>        # 条件步骤被跳过时
```

- `stage/event/elapsed_ms` 机器可读 token 永远在行首，desc/detail 收尾；
- `println!` 上控制台（部署里 stdout 已重定向进运行日志）+ `log::info!` 进
  `enable_log` 开的 `*_dblog.txt`（WriteLogger，Info 档；TermLogger 是 Warn 档，
  不会在终端二次回显）；
- 两条臂（`run_cli` / `run_read_through_core`）与 `run_app` 分臂前的公共段共用
  同一对 helper：同名步骤用同名 stage。

## 改动

### T1 helper（`src/lib.rs`，`fmt_elapsed` 旁）

- `startup_step_begin(stage, what) -> Instant`：打 begin 行，回表计时起点。
- `startup_step_done(stage, started, detail)`：打 done 行，`detail` 放本步关键
  数字（routes=N / migrated=N…），空串则只有 elapsed_ms。
- `startup_step_skip(stage, reason)`：条件步骤不跑时的单行出声。

### T2 `run_app` 公共段（分臂前）

| stage | 覆盖 |
|---|---|
| `config` | `get_db_option_ext` / `apply_asset_root` 装载 |
| `instance-lock` | `acquire_process_instance_lock` |
| `store-connect` | 持久臂：`SUL_DB.connect(rocksdb)` + `ensure_store_connected` |
| `data-face-claim` | `data_face_signature::validate_or_claim` |
| `spatial-tree` | `load_project_tree_verified` + revalidator spawn（或关闭出声） |

设置模式臂保持原样（它本来就是最小启动）。

### T3 `run_read_through_core`（mem 部署形态，现网在走的臂）

既有六段 println 收敛进 helper，并拆细：

| stage | 覆盖 |
|---|---|
| `store-connect` | `ensure_store_connected`（kv-mem 连接） |
| `surql-embedded` | `define_embedded_functions` |
| `db-manager` | `init_form_config` |
| `mdb` | `init_mdb`：搜索根发现 + SYS 声明 + 逐库绑定 |
| `http` | `start_configured`（含 bind 等待） |
| `route-directory` | 路由目录 + 头核验（banner 保留） |
| `spatial-room` | 空间树 + 常驻房间初始化 |
| `session-baseline` | 首轮会话基线（detail 带 `total_ms=`） |

### T4 `run_cli`（持久介质臂，之前只有零散耗时行）

逐段打点：`instance-lock`、`option-checks`（退役键点名/并发闸/延迟采样/范围出声）、
`log-init`、`surql-functions`、`surql-embedded`、`surql-selfcheck`、
`increment-state`、`dbnum-event-retire`、`inst-indices`、`inst-anc-backfill`、
`watermark-seed`、`write-marker-rollback`、`watermark-mirror`、`retired-tables`、
`inst-flat-sweep`、`flat-regression-probe`、`db-manager`、`route-directory`、
`pause-restore`、`tag-summary`、`http`、`spatial-room`、`watcher-init`、
`batch-worker`、`model-coverage`、`data-wait`、`model-converge`、`material`、
`ssc-tree`。

- 原 `println!("…完成，耗时 X")` 行由 done 行取代（计数折进 detail）；
  纯信息行（横幅、策略声明、等待期进展行）保留不动。
- `serve_if_configured` 换成 `start_configured`：**bind 完成才返回**，才能量出
  `stage=http` 的真实耗时；bind 失败从「spawn 任务悄悄死」变成启动就地失败
  （`start_configured` 的注释本就声明了这个契约，run_cli 是最后一个没遵守的臂）。
- `data-wait` / `model-converge` 不跑时打 `event=skip` 行，别让人对着空白猜。

### T5 测试

- `read_through_startup_reports_each_waiting_stage`：字面量钉改成钉 helper 调用形态
  `startup_step_begin("store-connect"`，stage 名单更新为 T3 的八段。
- 新增钉：helper 体内 `println!` 与 `log::info!` 并存（双出口不丢）；
  `run_cli` 体内关键 stage 都有 begin/done 配对。
- 既有钉 `every_startup_arm_names_a_stale_store_mode_key`、`read_through_startup_never_generates_the_whole_mdb` 不受影响（结构不动）。

## 验收

- `cargo check --lib` 0 error；`cargo test --lib` 全绿。
- mem 部署形态起服务，stdout 日志逐行可见：
  `store-connect → surql-embedded → db-manager → mdb → http → route-directory →
  spatial-room → session-baseline`，每段 begin/done 配对、带 elapsed_ms；
  `[root-scan]` 只出现在 `stage=mdb` 的 begin/done 之间（与 root-scan 闸改动自洽）。
- `enable_log=true` 时 dblog.txt 含同样的 `[startup]` 行。
