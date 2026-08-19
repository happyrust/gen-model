# Watch Scope 8000 / CATA 引用闭包离线证据（2026-08-19）

## 结论

- `watch_dbnums=[8000]` 的生产定位器只消费监听 DESI 与扫描裁决后的 CATA 清单。
- CATA 行采用确定性 `CONTENT` replacement；UDA/owner 先清旧集合再写当前集合。
- 缓存同时受源窗口右端与 CATA 文件指纹约束，暂存失败丢弃，水位提交后发布。
- Required 依赖连续 300 秒没有实质进展即失败；进展事件重置计时，定位更新不重置。
- reconcile 看见运行行时不替换活动 epoch；任务与健康回执包含依赖阶段和计数。

## 命令与结果

完整字面输出位于 `test-increment/runs/watch8000-dependency-20260819/verification/`。

| 命令 | 结果 |
|---|---|
| `rustfmt +nightly-2026-08-02 --check --edition 2024 <本次 Rust 文件>` | exit 0 |
| `cargo check --locked` | exit 0 |
| `cargo test --locked --lib cata_closure --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture` | 21 passed, exit 0 |
| `cargo test --locked --lib watch_scope --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture` | 9 passed, exit 0 |
| `cargo test --locked --lib dependency_ -- --nocapture` | 8 passed, exit 0 |
| 四个 CI 集成目标（按目标 feature 门追加 `legacy_session_replay`） | 44 passed, exit 0 |

## 交付物校验

`implementation.patch` 已在独立目录对 `original/` 执行 `git apply --check` 和实际应用，
与 `modified/` 逐文件换行归一化比较一致；随后执行 `rollback.ps1`，恢复结果与
`original/` 一致，新增 ADR/spec/plan 文件均已移除。字面记录见
`test-increment/runs/watch8000-dependency-20260819/verification/artifact-roundtrip.log`。

## test-worklspace live 验收

测试目录：`D:\work\plant-code\old\test-worklspace\bin`。这是既有数据店上的逻辑重放：
先把 8000 水位从 232 置回 33，已有数据行未清空，用于验证幂等重放、依赖闭包和水位
原子性，不冒充从干净 33 快照起步的物理基线。

- 首轮暴露启动 epoch 把模型阶段延后、从而绕过 Required 依赖的问题；修复后任务在
  commit 前明确进入 `dependency_index → dependency_closure → dependency_write`。
- Required 首次实跑因旧 `FOR $row ... UPSERT` 不满足 journal ReplaySafe 而失败；任务
  `failed`、水位保持 33、`staging_window` 为 0。replacement 改为显式记录目标后，内存库
  回归证明旧字段会删除、重复两次结果一致。
- 最终任务 `db-20260819-105933-000000`：`parsed=404`、`missing=0`，状态
  `succeeded`，窗口 `34..=232`，开始 `10:59:33`、完成 `11:37:59`。写回日志从修复前
  1284 条降至 404 条；写回耗时 2,301,380ms，总耗时 2,306,043ms。
- 终态字面查询：`applied_sesno=232`、`applied_sesno_time=2026-08-19T00:23:21Z`、
  `staging_rows=0`、8000 的 `pe_rows=6556`。任务回执保留依赖统计，health 为 `ok`，
  `active_dependency=null`。
- 提交期间多轮 reconcile 均打印“保留活动 epoch=1”，没有用新 manifest 覆盖运行任务。

部署、health/tasks JSON、SurrealQL 字面输出、原始/中间/最终二进制与 rollback 位于：
`D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\watch8000-cata-dependency-live-20260819-102646`。
最终部署 SHA-256：`3D60CB8C063CF62895420AC065812D5AFF7C92A55B180475822690E7E5BC7E0F`。

尚余专项：修改一个真实被引用 CATA 属性后的刷新对拍、300 秒静默故障注入，以及
`24384/23257` 的按需/全量 `combined_digest` 对拍。
