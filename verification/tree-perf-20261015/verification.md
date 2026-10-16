# 模型树展开性能：缓存 + spawn_blocking 实测记录（2026-10-15）

改动：tree 三端点进 `spawn_blocking`；`node()` 走 `tree_node_parts`（parse-only，跳过描述符抽取）；
`DirectTreeService` 加 `children_memo` 整层记忆（与服务同寿命 = 冻结会话语义）。

## 环境

- 新构建：`D:\Rust\target\release\aios-database.exe`（0.1.40+g14dd7e163711.dirty），`DB_OPTION_FILE` 指向
  `verification/tree-perf-20261015/runtime/DbOption.toml`（`http_api_addr=127.0.0.1:18222`），
  `AIOS_STORE_MODE=mem`（read-through）、`AIOS_ROOM_MEMBERSHIP=0`、`AIOS_STARTUP_AUTORUN=0`。
- 对照：既有常驻实例 `127.0.0.1:8022`（0.1.39 release，rocksdb，已跑约 6h，元素缓存 ~120k 全热）。
  同机同数据（AMS /ALL，71 库）。**未重启对照实例**，其「冷」数字仍沾了常驻缓存与 OS 页缓存的便宜。

## 结果（release vs release，curl 计时）

| 场景 | 旧 :8022 | 新 :18222 |
|---|---|---|
| `tree/roots` 冷 → 热 | —（热 ~2ms） | 322.9ms → ~2ms |
| `tree/children` ZONE 24383_95486（146 成员）**双冷** | 9.2ms | 3.6ms |
| 同上，热 ×10 | p50 3.2 / max 4.0 | **p50 2.2 / max 2.8（memo）** |
| `tree/children` ZONE 24383_86098（769 成员）冷 | 22.3ms | —（已热） |
| 同上，热 ×10 | p50 9.3 / max 11.9 | **p50 5.0 / max 6.2** |
| `tree/children` SITE 23717_2（531 成员）热 ×30 | p50 8.9 / max 45.2 | p50 4.2 / max 18.4 |

- **memo 生效的直接证据**：`/health` 的 `core.cached_elements` 在 memo 命中时不变（629→629）；
  冷展开 24381_535 时 +5（父已在 roots 时缓存，5 个成员各解析一次入库）。
- **响应一致性**：24383_86098 的 769 个节点两实例逐字节相同（refno/noun/name/owner/order/children_count/dbnum）。
- **并发**：12 个不同 refno 并发 children + 期间连打 roots：新实例 ping 全程 1.6–2.3ms（空闲基线 1.8–2.1）。
  旧实例本轮也没抖（并发请求全是热缓存小活）；spawn_blocking 的收益是结构性的——同步文件 I/O 不再占
  tokio worker，冷解析长尾（如 roots 323ms、大成员表冷读）不会再堵住同 worker 上的其他请求。

## 复现

```powershell
# 起验证实例（仓根 CWD，独立配置）
$env:DB_OPTION_FILE='D:\work\plant-code\old\gen-model\verification\tree-perf-20261015\runtime\DbOption.toml'
$env:AIOS_STORE_MODE='mem'; $env:AIOS_ROOM_MEMBERSHIP='0'; $env:AIOS_STARTUP_AUTORUN='0'
D:\Rust\target\release\aios-database.exe serve

# 计时
curl.exe -s -o NUL -w '%{time_total}' 'http://127.0.0.1:18222/api/v1/tree/children?refno=24383_86098'
```
