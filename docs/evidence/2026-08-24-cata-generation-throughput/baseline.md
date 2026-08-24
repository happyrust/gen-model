# 8000 空 RocksDB 初始化基线

## 输入与执行面

- 主仓提交：`b4bc9a56767da9121ec9653d77b3ed7faf8f4090`
- 后端：SurrealDB `2.1.4+20250317.45013fc9`，RocksDB
- 地址：`127.0.0.1:8169`，namespace `1516`，database `AvevaMarineSample`
- 命令：`aios-database.exe serve --debug-dbnum 8000`
- 完成边界：日志出现 `初始化完成：项目 AvevaMarineSample`；没有等待轮询意义的
  `pending=0`。

| 输入/记录 | 字节 | SHA-256 |
|---|---:|---|
| `ams8000_0001` | 17127424 | `FEE497B524DED0040C755613CC0A485D09256C9C146C71AB66B70395B008EC58` |
| `DbOption.toml` | 13577 | `F8ECD37F867581E1C7F3AD38FE72B55A1D65A98A3495B311DFF3B5D1679FF58B` |
| `init.stdout.log` | 227831 | `19E121D866991029A730FA45E1F46DDD2286D2C1542C10338B021246E39BD9C8` |
| `init.stderr.log` | 15608 | `1BBA3A928AAC885B6A88D6653C55D933CC5E09BA1027E95E14AD288120CBB0BC` |

空库探针在启动前返回空 `accesses/analyzers/configs/functions/models/params/tables/users`。

## 字面完成输出

```text
初始化完成：项目 AvevaMarineSample，启动总耗时 31m46.0s
```

该标记之后服务端曾因内存分配失败退出；初始化完成标记、模型统计和日志哈希均在该故障
之前产生，因此基线有效，但该次运行不作为峰值内存验收样本。

## 基线统计

从 44 条 `处理元件库几何体: N 花费总时间: T ms` 和 48 条
`生成完所有模型时间: Tms` 解析：

| 指标 | 基线 |
|---|---:|
| 初始化 wall-clock | 1906 s |
| CATA 页数 / 唯一身份总数 | 44 / 1702 |
| CATA 累计耗时 | 1,564,968 ms |
| CATA 页 p50 / p95 | 36,478 / 65,836 ms |
| 每身份归一化 p50 / p95 | 821.37 / 1,977.95 ms |
| 模型页累计耗时 | 1,827,754 ms |
| 模型页 p50 / p95 | 41,095 / 72,955 ms |

这说明 CATA 阶段占模型页累计耗时约 85.6%，是本阶段首要瓶颈。
