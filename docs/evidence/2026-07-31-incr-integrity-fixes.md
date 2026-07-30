# 增量链路静默失效修复 · 验证留痕

**日期**：2026-07-31 | **分支**：`codex/pre-hierarchy-refactor`
**特性**：`specs/001-incr-update-integrity-fixes/`（US1~US5）

## 环境

| 项 | 值 |
|---|---|
| 客户端 SDK | `surrealdb 2.1.4`（fork：`git+https://gitee.com/happydpc/surrealdb.git#45013fc9`） |
| 服务端 | `bin/surreal.exe` — `2.1.4+20250317.45013fc9`，与 SDK 同一个 rev（**仓库里本来就有**） |
| 启动方式 | `./scripts/Start-Surreal8009.ps1 -Memory`（**空的内存实例，用完即停**） |
| 连接 | `DbOption.toml`：`v_ip=localhost`、`v_port=8009`、`surreal_ns=1516`、`project_name=AvevaMarineSample` |

### 为什么用空的内存实例，而不是项目真实数据

本轮唯一需要 live 验证的是一条**不依赖任何工程数据**的 SurrealQL
（`DbnumState::record_blocked_observation`），它在一个自己创建、自己清理的
throwaway dbnum 上跑就够了。

顺带记录两个环境事实，下次别再踩：

1. **`.surreal/ams-8009` 这份 RocksDB 数据是存储版本 2**，而 `PATH` 上的
   `surreal.exe`（`D:\Rust\.cargo\bin`，`3.3.0-nightly`）要求版本 3，直接启动会报
   `The data stored on disk is out-of-date with this version (Expected: 3, Actual: 2)`。
   **不要用 3.x 去开它**——那需要不可逆的存储迁移。
2. 即便绕开数据目录，3.x 服务端与 2.1.4 客户端也**握不上手**：
   `WebSocket protocol error: SubProtocol error: Server sent no subprotocol`。

对的二进制一直就在仓库里（`bin/surreal.exe`，2.1.4，与 `Cargo.lock` 锁的 rev
同一个），只是 `PATH` 会先命中 3.x。这次之后加了
`scripts/Start-Surreal8009.ps1`：它按 `bin/surreal.exe` 找、启动前核对大版本与
git rev、遇到 3.x 直接拒绝并说清楚为什么。**以后一律走脚本，不要手敲
`surreal start`。**

## 静态门

```text
cargo check --lib          干净（0 error；本 crate 0 warning）
cargo test  --lib          285 passed / 0 failed / 57 ignored   （基线 277 passed）
```

`cargo check --workspace --all-targets` 失败，但这是**预先存在**、与本特性无关的：
`src/bin/cata_parse_probe.rs:74` 调 `parse_db_refnos` 少传一个 `&str`
（`cata_closure.rs:563` 的签名变过，这个 probe 没跟上）。

## Live 验证

```powershell
& "D:\work\plant-code\old\test-worklspace\surreal.exe" start `
    --user root --pass root --bind 127.0.0.1:8009 memory

cargo test --lib -- --ignored --exact `
  data_interface::dbnum_state::tests::live_blocked_observation_keeps_the_verdict_evidence_intact `
  --nocapture
```

```text
running 1 test
载入surreal common.surql
载入surreal fn_query_room_code.surql
载入surreal fn_query_room_code_hh.surql
载入surreal gen_root.surql
载入surreal get_room_nodes.surql
载入surreal gy_common.surql
载入surreal init_status.surql
载入surreal material_common.surql
test ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 342 filtered out
```

验到的四件事（dbnum `999_999_002`，跑完即清理）：

- `record_blocked_observation` 的 SurrealQL 语法有效、能落库；
- 阻断后**判据字段一个都没动**：`db_type` 仍是 `DESI`、`file_path` 仍是原路径、
  `file_name` 不变；
- 观察字段照实更新：`file_size` 8192、`file_latest_sesno` 70；
- `applied_sesno` 仍是 50 —— 扫描永不推进水位（ADR-001）；
- **第二轮拿库里现存的登记身份再判一次，仍然是 `TypeChanged`** —— 这才是重点，
  异常没有把自己抹掉。

这条测试的「回退即红」是**由直接数据断言保证**的（不是源码模式断言）：
把 `record_blocked_observation` 换回 `record_scan`，写进去的 `db_type` 就是 `CATA`，
`assert_eq!(after.db_type, "DESI")` 当场失败。这一条没有做实验验证，
另外两条做了（见下）。

## 回退即红（实验验证）

| 改回旧写法 | 失败的测试 |
|---|---|
| `duplicate_dbnums_across_watch_dirs` 用 `!should_exclude_file(...)` | `every_auto_path_gates_on_the_shared_candidate_predicate` |
| `revives_unconditionally` 只认 `is_room_recalc()` | `a_task_that_claims_no_session_revives_on_every_enqueue`（失败信息直接印出恒假的 `attempts = IF 0 > (source_end_sesno?:0)`） |

两次都在验证后立即恢复，全量测试转绿。

## 尚未验证的部分

以下四条修复只有纯函数与源码断言覆盖，**没有在真实工程数据上端到端跑过**：

- US1 副本过滤在真实库目录上的效果（需要一个带副本的监控目录）；
- US2 的完整链路 `scan_and_check_file` → 阻断 → 不入队；
- US3 `load_referrer_dbnums` 在真实 `pe` 表上的取数（SQL 形状与
  `fetch_base_graph_nodes` 同源，风险低，但没跑过）；
- US4 死信复活在真实队列表上的往返。

要补的话需要 `.surreal/ams-8009` 那份真实数据 + 2.1.4 服务端。

## 环境还原

临时内存实例已停止，8009 恢复为未监听。你原有的两个 surreal 进程
（8032 的 `acp7320-perf.db`、1516 的 `ams.db`）**全程未受影响**。
