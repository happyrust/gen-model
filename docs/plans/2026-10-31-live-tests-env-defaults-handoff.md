# 任务：让 gen-model 的 live 测试不用手配环境变量就能跑

## 一、背景

仓库 `D:\work\plant-code\old\gen-model`，分支 `refactor`，当前 HEAD `2badaf2e0`，已推 origin（github.com/happyrust/gen-model）。

这套仓库有 117 条 `#[ignore]` 测试，其中一批是「真库 live 测试」：要连本机 8009 的 SurrealDB，还要读 E3D 语料。历史上它们跑不起来，原因不是代码有 bug，而是测试基础设施有三层缺陷叠在一起。前两层已经修完，第三层是本次要做的。

用户的诉求原话：「我现在希望这些环境变量变成默认，不要我去修改」——即不想每次跑测试都手动 `$env:AIOS_XXX=...`，希望这些值从配置或合理默认里推导出来。

## 二、环境事实（必须先掌握）

**进程与端口**
- 8009 SurrealDB：`surreal start --bind 127.0.0.1:8009 rocksdb:.surreal/ams-8009`，数据 1523.7 MB，PID 65064。health 探针 `http://127.0.0.1:8009/health` 返回 200。
- 8022 aios-database：当前已停止。
- 9099 rs-server：已停止。
- HTTP 查询端点 `http://127.0.0.1:8009/sql`，头需 `surreal-ns: 1516`、`surreal-db: AvevaMarineSample`、`Authorization: Basic base64(root:root)`、`Content-Type: text/plain; charset=utf-8`。**带 `$` 的 SurrealQL 一律写进 .py 脚本文件再跑，不要 `py -c` 内联**。

**编译**
- `CARGO_TARGET_DIR` = `D:\Rust\target`（所有 worktree 共享）。
- 另一条工作线的 `build_info.rs` 会阻断编译，临时绕过：
  ```powershell
  $env:E3D_MODEL_VERSION="0.0.0-audit"; $env:E3D_MODEL_BUILD_ID="0.0.0-audit@g0000000"
  ```

**跑测试的正确方式**
不要用 `cargo test --ignored` 全量跑，会挂住（`e3d_model_service`、`window_root_plan`、`reverse_reference`、`fork_surreal_compat` 这几个模块会超时挂死）。正确做法是两步：

```powershell
# 1. 先拿到测试二进制
cargo test --lib --no-run
# 输出里找：Executable unittests src\lib.rs (D:\Rust\target\debug\build\aios-database\<hash>\out\aios_database-<hash>.exe)

# 2. 直接调二进制，按模块分批、单线程跑
& $bin --ignored --test-threads=1 "data_interface::manual_update::"
```

**配置**
- `DbOption.toml` 第 50 行 `project_name = "AvevaMarineSample"`，51 行 `project_code = 1516`，52 行 `surreal_ns = 1516`。

## 三、已经修好的三层（不要重做）

### 第 1 层：`Already connected` —— 已修，提交 `56e5c57dc`

**症状**：一个测试二进制里跑第二条真库测试就报 `Failed to connect to database: Already connected`。

**根因**：`aios_core::init_test_surreal()` 每调一次就对进程级 `SUL_DB`（`static SUL_DB: Lazy<Surreal<Any>>`，定义在 `vendor/old-aios-core/src/rs_surreal/mod.rs:68`）再做一次 `connect`。SDK 对同一实例的第二次 `connect` 必返 `Already connected`。`--test-threads=1` 救不了，问题不在并发而在「单例只能连一次」。

**修法**：新增 `src/test_support.rs` 的 `init_test_db()`，用 `tokio::sync::OnceCell` 缓存首次结果，失败不写缓存（下一个测试还能重试并报自己的错）。7 个模块 14 处调用全部改掉。

### 第 2 层：`sending into a closed channel` —— 已修，提交 `cc9afa443`

**症状**：第一条测试绿，后面每条都撞 `sending into a closed channel`。

**根因**：`connect` 会在**当前** runtime 上 spawn 连接层的 router 任务。`#[tokio::test]` 默认一条测试一个 current_thread runtime，那条测试跑完 runtime 一落，router 任务跟着没，`SUL_DB` 就成了通道已关闭的空壳。同一机制在 `src/web_service/review/platform/tests.rs:49` 有完整注释记录。

**修法**：`test_support` 建一个永不 drop 的进程级 `ENGINE_RUNTIME`，另起一条驻留线程一直 `block_on(std::future::pending::<()>())`，driver 一直在跑，router 不会被回收；连接通过 `engine_runtime().handle().spawn(...)` 建在那个 runtime 上。

### 第 3 层（部分）：`AIOS_MANUAL_UPDATE_PROJECT` 缺默认值 —— 已修，提交 `2badaf2e0`

**症状**：7 处 `std::env::var("AIOS_MANUAL_UPDATE_PROJECT").expect(...)`，不配就 panic。

**关键观察**：同一个测试里 `AIOS_MANUAL_UPDATE_DBNUM` 早就有默认值（`unwrap_or(7998)`）——同一组参数一半有默认一半没有，是历史遗留不是设计。

**修法**：`test_support::manual_update_project()`，显式环境变量仍优先，没配就取 `DbOption.project_name`（不写死字符串）。替换 7 处：`manual_update.rs` 3 处、`increment_manager.rs` 4 处。

## 四、当前状态

七个模块 `--ignored --test-threads=1` 跑下来：**24 passed / 14 failed**（三层修复前是 9 / 29）。

| 模块 | 修复前 | 现在 |
|---|---|---|
| increment_pipeline | 0 / 4 | **4 / 0 全绿** |
| increment_manager | 0 / 8 | 6 / 2 |
| model_update_plan | 0 / 5 | 3 / 2 |
| helper | 1 / 5 | 5 / 1 |
| update_scope | 0 / 2 | 2 / 1 |
| pdms_inst | 0 / 5 | 2 / 3 |
| manual_update | 0 / 7 | 2 / 5 |

默认回归 `cargo test --lib`：**2693 passed / 0 failed / 117 ignored**（改动前后一致，未受影响）。

## 五、要你做的事

把剩余 14 条失败里「因环境变量缺失」的部分也变成默认，让用户不用手配。**先分类，再动手，不要一律给默认值。**

### 剩余 14 条的实测分类

**A. 环境变量类（本次目标）**

1. `live_startup_sweep_repairs_a_caught_up_ghost_watermark`（`increment_manager.rs:1423`）
   报 `assertion left == right failed: testbed 明确关闭启动自动执行，left: "queued"`。
   需要 `AIOS_STARTUP_AUTORUN=1`。该变量在 `src/options.rs:1683` 有正式常量 `STARTUP_AUTORUN_ENV`，且 `src/bin/l3_suite.rs:928` 已有 `.env("AIOS_STARTUP_AUTORUN", "1")` 的惯例。

2. `probe_live_sql`（`helper.rs:732`）
   报 `set AIOS_PROBE_SQL: NotPresent`。这是临时探针测试，语义上**不该给默认值**——没有 SQL 就没有要跑的东西。建议改成「未配就跳过并打印 skip」，而不是给个假默认值。

**B. 大查询被服务端断连（2 条，不是环境变量）**

`live_rebuild_ref_rev_covers_shared_spco_consumers`（`manual_update.rs:10034`）、
`live_shared_spco_expands_to_generation_roots`（`manual_update.rs:10098`）
报 `load reverse-index attribute chunk failed: Internal error: receiving from an empty and closed channel`。
单独跑也复现，跑 20.42 秒后断。8009 本身健康（占用 2.4 GB，health 200）。是 `rebuild_reverse_index()` 这类重活触发服务端保护。
**这类不该靠环境变量解决**，要么调 chunk 大小，要么接受它只能单独跑。

**C. 业务断言与实现不符（3 条，是测试或实现的问题，不是环境）**

- `live_manual_update_project`（`manual_update.rs:9731`）：断言把 `watch_dbnums` 限定模式的 warning 当失败。实际输出是「本进程处于监听限定模式：只处理 watch_dbnums 7997,7998,8000 里的库」。
- `live_rollback_and_ghost_watermark_reinit_end_to_end`（`manual_update.rs:9833`）：断言要「回退重建：已整库清空」，实现实际输出「检测到回退（文件回退或被替换），已阻断：已整库清空 4 行 PE」。
- `live_rollback_wipe_clears_the_dbnum_for_reinit`（`manual_update.rs:7362`）：报 `resolve Ref0 ranges for dbnum 999999021`，夹具自身问题。

**D. 前提被破坏（1 条）**

`staged_transform_routes_pointer_updates_through_the_journal`（`increment_manager.rs:5811`）：
这条测试**刻意不连 `SUL_DB`**，源码注释写明「SUL_DB 未连接，直写即错」。但现在全局 `init_test_db()` 已经把 `SUL_DB` 连上了，前提被破坏，报 `Connection uninitialised`。
**这条和第 2 层修复直接冲突**，需要专门设计（比如给它独立的连接句柄，或让它不共享 `SUL_DB`）。

**E. 语料缺失（`room_resident` 的 9 条，不在上面七个模块里）**

`room_resident.rs` 的 9 条 `#[ignore]` 要的是 AMS 源文件语料（`AIOS_AMS_PROJECT_DIR` 等），与连接无关。参考已有的 `corpus_ready()` 模式（提交 `abe387b65` 里 `cable_endpoint` / `structural_connection` 的做法：文件不在就 `eprintln!("skip ...")` 后 `return`）。

### 建议的优先级

1. **A-1**（`AIOS_STARTUP_AUTORUN`）——最直接，有现成惯例，预计 +1 passed。
2. **A-2**（`probe_live_sql` 改跳过）——小而干净。
3. **E**（`room_resident` 用 `corpus_ready` 模式）——9 条，收益最大，但要看语料是否真在盘上。
4. **D**（`staged_transform` 冲突）——需要设计，可能要给 `test_support` 加「不连全局」的模式。
5. **C**（业务断言）——要判断是实现对还是断言对，别盲改。
6. **B**（大查询断连）——最后做，可能要调 chunk 或加超时。

## 六、硬约束（违反会出事）

**1. 文件归属：工作区有大量别人未提交的改动，绝不能整文件 `git add`。**

`git status` 里这些是别人的：`src/data_interface/` 下十几个文件、`build.rs`、`build_info.rs`、`changelog.md`、`CONTEXT.md`、`DbOption.toml`、`docs/adr/*` 多个 ADR。

尤其 `manual_update.rs`、`increment_manager.rs`、`increment_pipeline.rs` 三个文件里混有另一条工作线的改动（`expand_live_reverse_cascade` 的借用改写、T-O-2 的 `enqueue_discovered` 闸、`FileAnomaly::NameChanged` 等）。

**精确暂存的正确做法**（本次已用过两次，有效）：

```powershell
# 1. 从 HEAD 取版本到仓库外的临时目录
git show "HEAD:src/data_interface/manual_update.rs" | Out-File -Encoding utf8 D:\work\plant-code\old\_stage2\manual_update.rs

# 2. 在临时副本上只做自己的替换（用 .Replace，不要用 -replace 正则）
$c = Get-Content $path -Raw
$c = $c.Replace($old, $new)
Set-Content -Path $path -Value $c -NoNewline -Encoding utf8

# 3. 写 blob 进索引，工作区不动
$blob = git hash-object -w $path
git update-index --cacheinfo "100644,$blob,src/data_interface/manual_update.rs"

# 4. 核对只含自己的改动
git diff --cached -- src/data_interface/manual_update.rs | Select-String -Pattern "^[+-][^+-]"
```

**2. 探针脚本一律放仓库外。** 曾因为在仓库里建 `.probe_*.py` / `.verify_*.py` 被判成 out_of_scope。用 `D:\work\plant-code\old\probe-*`。

**3. PowerShell 陷阱。**
- 提交信息不要写在 `-m "..."` 里（反引号会被当转义符，中文注释里的 \` 会触发 `ParserError`）。写进文件用 `git commit -F msg.txt`。
- `-replace` 只接受两个参数，第三个会报 `InvalidOperation`，且**会把文件写空**。用 `.Replace()`。
- `grep` 被禁，过滤输出用 `rg` 或 `Select-String`。

**4. 委派纪律。** 外部 CLI 走 `swe2-bridge`（需声明 `owned_paths` 与 `checks`），子 agent 用 `worker` / `explorer`。**各委派文件归属不得重叠。** 委派自报不可采信，必须自己复验。

**5. 提交规范。** 提交信息中文，格式 `type(scope): 摘要`，正文写清「症状 / 根因 / 修法 / 实测数据」。含 `Co-authored-by: factory-droid[bot] <138933559+factory-droid[bot]@users.noreply.github.com>`。

**6. 推送。** 只推 `origin refactor`。远端有并行工作（`git log` 里能看到别人的取证提交），推前先 `git rev-list --count origin/refactor..HEAD` 确认。

## 七、不要做的事

- 不要跑 `cargo test --ignored` 全量（会挂死）。
- 不要整文件 `git add` 任何 `src/data_interface/` 下的文件。
- 不要为 `probe_live_sql` 编一个假默认 SQL——没有 SQL 就没有要跑的东西，应该跳过。
- 不要把 B 类（大查询断连）当成环境变量问题去「修」。
- 不要动 `vendor/` 下的仓库（`plant-mbd` 有 8 处未提交改动，`e3d-io` / `e3d-attlib` 各有未提交改动）。
- 不要动 `core`（`aios_core` 是 git 依赖，rev 钉在 `d5a6a88c2c3b2093621bf72ead42493fcab67515`）；如果要改它，先问用户。

## 八、验收标准

1. 剩余 14 条里，A 类（环境变量）全部解决，且不引入「假默认值」。
2. `cargo test --lib` 仍然 2693 passed / 0 failed / 117 ignored。
3. 七个模块 `--ignored --test-threads=1` 的 passed 数从 24 继续上升，且每条失败都能说清属于 B/C/D/E 哪一类。
4. 每个提交只含自己的改动，`git diff --cached` 核对过。
