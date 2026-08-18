# 2026-08-18 core 元素 diff 单一事实源（T14）

状态：**通过**。

## 目标与边界

core.dll 的窗口变化链分两层：索引双根差分给出 Added/Deleted/Modified 候选，随后
只对 Modified 候选解析两端元素并做属性/成员桶差分。本次收口第二层的工程漂移：

- `old-pdms-io::get_refno_operation_status`（legacy 回放）原有一份内联 diff；
- `gen-model::net_window` 为净窗口另有一份 `diff_ele_data` 复刻。

现在权威实现为 `../vendor/old-pdms-io/src/io.rs::diff_ele_data`。它纯比较两个已解析
`EleData`，覆盖普通属性、显式属性、UDA（按 `hash_val`）和有序 children；记录寻址、
Added/Deleted 分类和 last-touch 仍由各收集器负责。`net_window` re-export 该符号，公开
形状不变。

## 验证

| 检查 | 结果 |
|---|---|
| vendor `cargo test --lib element_diff_tests -- --nocapture` | 2 passed，exit 0 |
| gen-model `cargo test --locked --lib data_interface::net_window ...` | 13 passed / 2 ignored，exit 0 |
| gen-model `cargo test --locked --lib increment_pipeline ...` | 48 passed / 6 ignored，exit 0 |
| `db8000_session_pairs`（含性质 i 的 Modified 逐桶对拍） | 20 passed，exit 0 |
| `live_ams8000_net_diff_agrees_with_point_lookups_and_covers_replay` | 1 passed，16.08s，exit 0 |
| `live_ams8000_net_window_payloads_match_replay_on_single_touch_refnos` | 1 passed，18.72s，exit 0 |
| issue-019 固定窗口全链签名 + T11b | 2 passed in 32.55s，exit 0 |

issue-019 生产链仍得到 `changed=3`、`sessions=[25,26]`、精确墓碑
`24384_24778` / `24384_24779`、水位 26。隔离 db8000 在测试后恢复原 SHA256
`2eae30556380eb79daf903cb15428e22df075e871e69acbcbed09a7edd337137`。

## 基线限制

vendor 全量 lib 档本机结果为 35 passed / 5 failed / 1 ignored；5 项均为既有环境夹具
问题（Windows 路径分隔、缺测试数据库、未初始化连接或旧配置数据形状），新增
`element_diff_tests` 单独运行全绿。首次构建还因陈旧 `PROTOC_INCLUDE` 指向不存在目录
而停止；改为仓内 `protoc/include` 与 `protoc/bin/protoc.exe` 后通过。
