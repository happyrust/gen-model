# 2026-08-19 Oracle 二次审核正确性收口证据

工作区：`D:\work\plant-code\old\gen-model`

## 依赖发布

| 仓库 | revision | 推送分支 | 结果 |
|---|---|---|---|
| old-aios-core | `29c91f48ce230814a26466d2150d51385417fab8` | `codex/room-panel-wire-repair` | push exit 0 |
| old-parse-pdms-db | `f3537dae21f64feb3880e5340320ee1db3bd9176` | `codex/room-panel-wire-repair-deps` | push exit 0 |
| old-pdms-io | `99094048fe309ea272048b1c7d28cee2b3e383cb` | `codex/room-panel-wire-repair-deps` | push exit 0 |
| manifold-csg | `2233c5a1162eba981e3f43efcd39a6bc25581335` | upstream fixed revision | resolved |

## 依赖仓库门禁

- `old-aios-core`: `cargo check --lib --no-default-features --features gen_model` → `Finished dev profile`, exit 0。
- `old-parse-pdms-db`: `cargo check --lib --no-default-features` → `Finished dev profile`, exit 0。
- `old-pdms-io`: `cargo test --lib --no-default-features -- --nocapture` → `72 passed; 5 failed; 2 ignored`，exit 101。失败均为仓库既有环境型测试：Windows 路径分隔符、未初始化 Surreal、现场 refno/配置缺失。
- `old-pdms-io`: `cargo test --lib net_window::tests --no-default-features -- --nocapture` → 全部通过，exit 0。
- `old-pdms-io`: `cargo test --lib snapshot::tests --no-default-features -- --nocapture` → `2 passed; 0 failed`, exit 0。
- `old-pdms-io`: `cargo check --lib --no-default-features` → `Finished dev profile`, exit 0。

## 主仓单元与集成门禁

- `cargo check --no-default-features --features ws,gen_model,manifold,project_hd,http_api` → `Finished dev profile`, exit 0。
- `cargo test --locked --lib cata_closure --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture` → `25 passed; 0 failed`, exit 0。
- `cargo test --locked --lib watch_scope --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture` → `9 passed; 0 failed`, exit 0。
- commit receipt → `1 passed; 0 failed`, exit 0。
- epoch activation barrier → `1 passed; 0 failed`, exit 0。
- geometry failure policy → `1 passed; 0 failed`, exit 0。
- hard replay planner → `1 passed; 0 failed`, exit 0。
- `db8000_two_delete_fixture`（加目标声明的 `legacy_session_replay`）→ `6 passed; 0 failed`, exit 0。
- `db_session_fixture_selfcheck` → `15 passed; 0 failed`, exit 0。
- `db8000_session_pairs` → `21 passed; 0 failed`, exit 0。
- `pdms_record_boundary` → `3 passed; 0 failed`, exit 0。
- `cargo check -p aios-py --no-default-features` → `Finished dev profile`, exit 0。
- `cargo metadata --locked --format-version 1 --no-deps` → exit 0；`Cargo.lock` 中旧三依赖 revision 命中数为 `0`。

## 已知未执行项

E3D 保存、tail 延迟注入和 staged NotManifold 属于交互式 live 验收。本轮先完成代码、离线回归、固定依赖与 Release；live 结论不由离线测试替代。
- `cargo build --release --locked --bin aios-database --no-default-features --features ws,gen_model,manifold,occ,project_hd,http_api` → `Finished release profile [optimized] target(s) in 5m 15s`, exit 0。
- `sigmap verify-plan specs/015-oracle-review-correctness-closure/plan.md` → plan checks out，exit 0。
- `sigmap verify-ai-output .context/ai-output.md` → no hallucinations detected，exit 0。
- `sigmap review-pr --staged` → 完成 48 文件审计；21 条均为“同文件内联测试未被路径启发式识别”或跨层 scope 提示，无 secret/P1；工具按 finding 语义返回 exit 1。
- 三个 E3D PowerShell 启动辅助脚本经 `System.Management.Automation.Language.Parser::ParseFile` 检查 → syntax PASS，exit 0。

## Oracle 会话复核与整改

Oracle 会话 `gen-model-increment-review-20260819-3` 报告 6 项，其中 Cargo.lock 缺失为附件裁剪造成的误报；实际锁文件已更新。其余问题已逐项收口：

- 显式 `DependencyCacheContext` 在窗口外同样权威，源 dbnum 不匹配直接报错。
- 恢复批次在开窗前读取并注入持久 `commit_token`；旧记录无 token 时在首次写回前固化。
- tail 禁止内嵌事务，receipt 与水位由同一个外层事务保护；tail/pre-tail 同样执行 64 KiB/行数硬限制。
- tail 超时写入 `outcome_unknown`，重试复用同 token；串行锁按单次尝试释放，使其他 dbnum 可继续。
- Required 几何策略覆盖缺失正负实体、空正实体集、零三角面和空差集。
- 生命周期回归曾发现离线 `create_window` 被全局恢复查询绑定；恢复读取已移至生产批次入口，显式 token 传入窗口。

## Oracle 后最终复验

- `cargo fmt -- --check` → exit 0。
- `cargo check --locked --no-default-features --features ws,gen_model,manifold,project_hd` → `Finished dev profile`, exit 0。
- `cata_closure` → `27 passed; 0 failed`, exit 0。
- `staging::executor` → `13 passed; 0 failed`, exit 0。
- `staging::lifecycle` → `9 passed; 0 failed`, exit 0。
- Required/BestEffort manifold 定向测试 → 各 `1 passed; 0 failed`, exit 0。
- 四项 CI 集成测试最终复验 → `6/15/21/3 passed`, exit 0。
- Release 最终复验 → `Finished release profile [optimized] target(s) in 1m 53s`, exit 0。

- `batch_worker` 源序与并发回归 → `48 passed; 0 failed`，exit 0。
- `watch_scope` 最终复验 → `9 passed; 0 failed`，exit 0。
- `cargo metadata --locked --format-version 1 --no-deps` 最终复验 → exit 0。
- 无兄弟依赖目录干净克隆 `C:\Users\dpc\AppData\Local\Temp\gen-model-clean-ef0c7004526f43b990503ee29ff69fd2`：`cargo metadata --locked` exit 0；Release 构建 `Finished release profile [optimized] target(s) in 1m 56s`，exit 0。
