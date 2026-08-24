# 批量读取后的串行回滚档（geometry_permits=1）

## 执行

- 独立 RocksDB：`.scratch/cata-throughput/serial-3/surreal-rocksdb`
- 端口：8173
- 完成边界：`初始化完成：项目 AvevaMarineSample`
- wall-clock：1875.891 s（31m15.5s）
- 应用 CPU：1399.594 s
- 应用峰值工作集：182,886,400 bytes
- 输入 `ams8000_0001` SHA-256：
  `FEE497B524DED0040C755613CC0A485D09256C9C146C71AB66B70395B008EC58`

## 统计

| 指标 | 串行档 | 相对旧基线 |
|---|---:|---:|
| 初始化 wall-clock | 1875.891 s | -1.58% |
| CATA 页数 / 身份数 | 44 / 1702 | 相同 |
| CATA 累计耗时 | 1,541,469 ms | -1.50% |
| CATA p50 / p95 | 36,185 / 67,774 ms | -0.80% / +2.94% |
| 每身份 p50 / p95 | 853.20 / 2,052.13 ms | +3.88% / +3.75% |
| 模型累计耗时 | 1,794,529 ms | -1.82% |
| 模型 p50 / p95 | 41,820 / 73,494 ms | +1.76% / +0.74% |

额度 1 的职责是结果回滚与等价基准，不承担性能门。首个页面的结构化摘要显示约
38.3/41.3 秒消耗在唯一 CATA 的 `resolve_component`，证明下一步应并行这些独立身份，
而不是扩大分页或等待 `pending=0`。

## 最终计数

```json
{"aabb":4759,"geo_relate":9554,"geom_error":2593,"inst_geo":3606,"inst_info":1309,"inst_relate":2681,"pe":21950,"pending":0,"world_trans":0}
```

`pending` 只作为随最终快照记录的表计数，不是本次计时停止条件。

| 记录 | SHA-256 |
|---|---|
| `DbOption.toml` | `FAB8B85B332D2D35091AB808834A020237698E97B5C6BA80122644820138CC3C` |
| `init.stdout.log` | `184EF3C4DEAEB8B4972C7A6790EF7D3029CB04908F99B4DD34B4A8934BB55A9C` |
| `init.stderr.log` | `9B37382000FA3FE55C8EB60D282699F3E043D64E32EF8B790C9513FD3173A08B` |
| `final-counts.json` | `5A85A6F82A6C73F333DE18A21269D3BDC41785F4449F2C6EA892B0BC25808C8A` |
