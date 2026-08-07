# E3D PML 改数据 → 增量更新端到端实测（2026-08-06）

一次真实的「在 E3D 里用 PML 改一个设计参数 → SAVEWORK 落文件 → 增量服务扫描入队 →
kv-mem 暂存窗口执行 → 写回推水位」的完整链路，全程在真库上跑通。

## 环境

| 部件 | 取值 |
|---|---|
| 服务二进制 | `D:\Rust\target\debug\aios-database.exe` 的副本 `aios-database-bigstack.exe`（`editbin /STACK:64MB`）；执行侧改用 `manual_exec_probe-bigstack.exe` |
| 关键环境变量 | `RUST_MIN_STACK=67108864`（放大 tokio 工作线程栈）、`AIOS_SKIP_STARTUP_ROOM_BUILD=1` |
| SurrealDB | `bin/surreal.exe`（2.1.4）`ws://127.0.0.1:8009`，rocksdb 后端 `.surreal/ams-7997-e3d-test-20260805`，ns `1516` / db `AvevaMarineSample` |
| E3D | 影子安装 `E:\reverse\e3d\shadow_e3d31_aps_all\des.exe`，MDB `/ALL1`，经 `run_incremental_macro.ps1` + `GenModelIncrementalTest` addin 自动跑宏 |
| 被测目标 | DAMP `=24381/100819`（`/1CUP001VAR_CODEX`，dbnum 7997），其属主 BRAN `24381/100817`（`/-CUP-S-3-M-1201`） |

### 踩到的两个坑（记下来给后来人）

1. **debug 构建启动即栈溢出**：直接跑 `aios-database.exe` 报 `main thread has overflowed
   its stack` 闪退；`editbin /STACK` 只放大主线程栈，tokio 工作线程默认 2MB 仍溢出。
   **两者都要**：`editbin /STACK:64MB`（主线程）+ `RUST_MIN_STACK=67108864`（工作线程）。
2. **专用 E3D 会话的 addin 没加载**：`run_incremental_macro.ps1` 让启动器部署的是
   `DesignAddins_no_multicad_with_viewer3d.xml`（不含 GenModelIncrementalTest）。要手动给
   启动器传 `-AddinsXmlPath ...\DesignAddins_no_multicad_with_viewer3d_incrtest.xml`（含
   `GenModelRvmExport` + `GenModelIncrementalTest` 两个 addin）宏才会自动跑。
3. **隔离副本库走不通**：`surreal export` 出来的 surql 带 fork 特有的函数体（`fn::code`
   那行 `?: ,` 形态），`import` 到新实例解析报错。本轮因此在真库上做。

## 链路与证据

### 1. E3D 侧：PML 改设计参数 + SAVEWORK

宏 `scripts/e3d/projams_damp_desp_apply.mac`：定位 `=24381/100819` → `DESPAR NUM2 1400`
（原 1000）→ `SAVEWORK`。

- addin 日志 `output/fable1-e2e/e3d_incr_test.log`：
  `$M "...projams_damp_desp_apply.mac" -> True`
- 宏日志 `scripts/e3d/projams_damp_desp_apply.log`：`Desparam 534460 1400 800 1000 800 ...`
  （第 2 个设计参数已是 1400）
- 文件 `D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7997_0001`：mtime
  `07:51:39 → 16:37:49`，size `58167296 → 58185728`（SAVEWORK 写入了 session 93）

### 2. 服务侧：扫描 + 入队恰好是 92→93 增量

`manual_exec_probe-bigstack.exe AvevaMarineSample ALL1`（范围 `/ALL1` 的 DESI 只含
7997，其余 4 个 `up_to_date`），入队回执：

```json
{"dbnum":7997,"db_type":"DESI","start_sesno":93,"end_sesno":93}
```

即服务扫描文件读到 E3D 刚写的 session 93（此前 applied=92），只入队 92→93 这一段增量。

### 3. 执行侧：kv-mem 暂存窗口 → 重生成 → 写回推水位（ADR-017 路径）

批次终态 `succeeded`（task `db-20260806-165316-000000`，`output/fable1-e2e/manual_exec_7997.log`）：

- `数据批次 dbnum=7997 db_type=DESI 使用 kv-mem 暂存窗口 staging_7997_1（sesno 93..=93）`
- `批量重生成 1 个根成功（耗时 183828ms）：24381/100817`（BRAN 冷生成，含 CATA 目录闭包按需解析）
- `开始写回 ... journal=169 条 / 1213818 字节，暂存语句=187 条 / 9537893 字节，预计写入行=12129`
- `写回完成 dbnum=7997 水位推进至 sesno=93，失效缓存=3 项，尝试=1 次`
- `写回后空间树与文件已收敛 dbnum=7997`
- 结果 JSON：`changed_elements=2`；unit `{root:"24381/100817", noun:"BRAN", status:"generated", attempts:0}`

### 4. 对比验证（`output/fable1-e2e/compare-*.json`）

| 断言对象 | 基线（session 92） | 增量后（session 93） | 判定 |
|---|---|---|---|
| `dbnum_watermark:7997` applied/file | 92 / 92 | **93 / 93** | 水位推进 ✓ |
| DAMP `pe:24381_100819` sesno | 92 | **93** | 数据被 session 93 更新 ✓ |
| BRAN `pe:24381_100817` sesno | 92 | **93** | 同上 ✓ |
| BRAN 单元生成 | — | `generated` / attempts 0 | 模型重生成 ✓ |
| `model_update_pending WHERE regen_root='24381/100817'` | — | **0** | 成功根已收口、无重复生成（ADR-017 缺陷 5 语义）✓ |
| `inst_relate WHERE in.dbnum=7997` | （结构库未生成，≈0） | 45637（含历史 issue7 生成 + 本轮分支替换） | 模型实例存在 ✓ |

DAMP 的设计参数（DESP=1400）不作为 `pe` 顶层字段存储（pe 字段仅 cata_hash/dbnum/
name/noun/owner/refno/sesno 等），它在生成时从会话数据解析进几何；`cata_hash` 是目录
组件（SCOM）引用哈希，同一组件改设计参数不变，符合预期。

## 观察到的一个问题（值得跟）

暂存房间轮初始化失败并把全部房间目标保留 pending（**fail-closed，未污染**，符合 ADR-017
缺陷 1 的设计）：

```
暂存房间轮初始化失败，全部房间目标保留 pending:
查询 496 块在册面板的实例失败: Serialization error: failed to deserialize;
expected an object-like struct named Transform, found None
```

即 7997 结构库的 496 块在册面板没有几何实例（结构库从未整体生成），读它们的
`Transform` 反序列化到 `None`。数据与模型照常提交，房间归属这一轮尽力而为地推迟——
这正是「房间是可事后重建的派生数据、不阻断窗口」的既定行为，但这条反序列化路径值得
按 fail-soft 复查（`None` 应被跳过而非报错）。

## 复原

改动落在测试库 `.surreal/ams-7997-e3d-test-20260805`（非生产）。如需回滚 DAMP 参数：
E3D 里跑 `scripts/e3d/projams_damp_desp_restore.mac`（NUM2 1400→1000 + SAVEWORK）再执行
一次增量即可。

## 关键产物

- `output/fable1-e2e/execute-receipt.json` / `manual_exec_7997.log`：入队回执与批次全程日志
- `output/fable1-e2e/compare-0-baseline.json` / `compare-1-after.json` / `compare-2-model.json`：前后快照
- `output/fable1-e2e/e3d_incr_test.log` / `scripts/e3d/projams_damp_desp_apply.log`：E3D addin 与宏日志
