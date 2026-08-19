# Spec 013 实施计划

## Constitution Check

- 水位只在完整数据落库后建立；失败不推进。
- watcher/manual 共用同一收集入口，不引入第二消费路径。
- 异常使用类型化错误和持久错误账本，不落进 silent default。
- `parse_pdms_db` 作为 decoder，`pdms_io` 作为 reader authority，保持模块边界单一。

## 阶段

1. `old-parse-pdms-db`：最小元素身份解析。
2. `old-pdms-io`：快照身份、严格净窗口、目标会话存在性查询。
3. 主仓：接入完整性门、基线失败清理、CATA 缓存重试。
4. 三仓单测、主仓 CI feature 构建、固定夹具与 live 证据。
