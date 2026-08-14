# 2026-08-14 模型实例保存有界合批验证

## 对象与版本

- 仓库：`D:\work\plant-code\old\gen-model`
- 分支：`codex/shape-save-coalescing`
- test workspace：`D:\work\plant-code\old\test-worklspace`
- 旧二进制 SHA-256：`1370318038a7c5626b9559eb161e03a507a9dd3d5c839db0271edc59b85dc4a3`
- 首轮候选二进制 SHA-256：`a70389efcf06efc85b3c7bff47968296ac51fee2f4c1a01e6478dcd254d56044`
- 最终部署二进制 SHA-256：`fc0ed215744e55ad88519ff9b9a3ad61f85dd20cdc2e9d9209971e6390e0e2a7`
- A/B 原始日志与快照：
  `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\shape-save-coalescing-20260814-123712`

## 自动化验证

```text
cargo test --locked --lib fast_model::shape_save::tests:: \
  --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture
=> 6 passed

cargo test --locked --lib fast_model::pdms_inst::tests:: \
  --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture
=> 15 passed, 2 ignored

cargo check --locked --no-default-features --features ws,gen_model,manifold,project_hd
=> 0 errors

cargo build --release --locked --bin aios-database --no-default-features \
  --features ws,gen_model,manifold,occ,project_hd,http_api
=> 0 errors
```

覆盖：软/硬阈值、下一批放不下、channel close、idle timer、超大源批、typed conflict、
normal/tubi 重叠、NaN、neg 顺序、共享圆柱参数、100 次输入排列、SQL 行/字节上限、
staged 串行顺序、两次 journal 重放终态一致，以及失败后不执行后续 packet。

固定 16 根纯性能夹具把 16 个 1 行源批从 16 次 save 收敛为 1 次 flush；非删除 SQL
packet 从 48 降至 3，二者均下降 93.75%，超过 70% 门槛。

## test-workspace A/B

根集合：

```text
24384/22402, 24384/22404, 24384/22441, 24384/22476,
24384/22478, 24384/22515, 24384/22520, 24384/22522,
24384/22528, 24384/22550, 24384/22552, 24384/22554,
24384/22556, 24384/22558, 24384/22560, 24384/22566
```

- 旧版两轮模型生成耗时：40,827 ms、42,054 ms。
- 候选版两轮模型生成耗时：41,036 ms、41,722 ms，均落在旧版波动区间内。
- 旧版与候选版均生成 150 个实例行；`inst_relate` canonical JSON 均为
  `c775a8dc5daa201e5ec219911740a39370f1a86f07e9a4e9597e5c59442c4d37`（150 行）。
- 候选版结构化统计：`source_batches=4 flushes=4 instances=150 geos=428
  metadata_queries=4 sql_packets=24 sql_bytes=819115 scoped_deletes=150 conflicts=0`；四次
  flush 均为 `MaxWait`，单批 26～55 行、77～125 个几何 occurrence。
- 这组真实 BRAN 在四个 CATA 分段相隔超过 8 ms 后各自产出一个 25～48 行尾批，因此
  没有命中“16～32 个紧邻 1～3 行源批”的 70% 合批门；固定性能夹具负责该门，现场 A/B
  负责终态和端到端不回退。
- 空间树快照在旧版定向直写后检测到 epoch 漂移；候选版重启从库指针重建为 73,573 条并
  重新落盘；最终快照 SHA-256 为
  `88fb9bd93a6355f35183ad31dd9fc2d9dd1199a10d71ca6bb8337a09da89b3ee`，验证记录见
  `candidate-final.stdout.log`。这同时覆盖了进程重启恢复路径。
- staged mem 用例两次执行同一计划后 journal 为四条有序记录，终态值仍为 2；direct
  test-workspace 路径按设计不产生 staging journal。

## 结论

确定性终态、失败传播、staged 顺序、固定性能夹具 70% 门和 test-workspace 端到端耗时门
均通过。真实 16 BRAN smoke 未形成紧邻小尾批，故其统计保留为 workload 观测，不替代固定
性能夹具的合批断言。

## 可复核记录与回滚

| 角色 | 命令/路径 | 字面结果 |
| --- | --- | --- |
| 基线运行 | `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\shape-save-coalescing-20260814-123712\aios-database.baseline.exe`，输入为上述 16 根 | `模型生成 40827 ms / 42054 ms`，进程退出码 `0` |
| 修改后运行 | `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\shape-save-coalescing-20260814-123712\aios-database.candidate-final.exe`，输入为同一 16 根 | `模型生成 41036 ms / 41722 ms`，进程退出码 `0` |
| 修改后产物 | `D:\work\plant-code\old\test-worklspace\bin\aios-database.exe` | SHA-256 `fc0ed215744e55ad88519ff9b9a3ad61f85dd20cdc2e9d9209971e6390e0e2a7` |
| 补丁 | `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\shape-save-coalescing-20260814-123712\shape-save-coalescing.patch` | `git apply --reverse --check` 退出码 `0`；同目录 `shape-save-coalescing.patch.sha256` 保存最终摘要 |
| 验证清单 | `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\shape-save-coalescing-20260814-123712\verification-manifest.json` | `config_restored=true`、`service_running=false` |
| 可运行回滚 | `D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\shape-save-coalescing-20260814-123712\rollback-runtime.ps1` | 隔离目录实际执行退出码 `0`：`ROLLBACK_BINARY_MATCH=True`、`ROLLBACK_CONFIG_MATCH=True` |

最终质量命令的退出状态均为 `0`：`cargo fmt --all -- --check`、两个定向库测试、
`cargo check`、release 构建以及 `git diff --cached --check`。配置在验证结束后已恢复，服务保持停止，
避免测试配置继续影响后续人工启动。
