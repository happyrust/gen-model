# 合并后 e3d-direct 的 AMS 验证 · 历史模型（按 sesno / time 生成）· 2026-09-13 23:17

**目的**：`gen-model-refactor` 并入 `e3d-direct`（merge `793176510`）之后，用 AvevaMarineSample 真库复跑
历史模型那组 live 用例，确认合并后的主线与 refactor 侧回执一致；顺带核对 SchemaPack 手动选版这条
（合并时冲突最多的一处）在真库上照常工作。

**结论**：10 条用例逐条与 refactor 侧（`gen-model-refactor/verification/historical-model-live-20260913-2233`）
**回执一致**；`e3d_version = "e3d31"` 走 `mode=manual source=DbOption`，schema pack 加载成功。

## 现场

| 项 | 值 |
|---|---|
| 二进制 | debug `aios_database.exe`，`0.1.23+g793176510c4d.1789312596`（= merge 提交，工作树干净；`build-identity.json`） |
| 配置 | `DbOption.toml`（本目录副本）：AvevaMarineSample / MDB `ALL`，`read-through` + `embedded-mem`，`http_api_addr = 127.0.0.1:8047`，独立 `meshes_path`；运行目录在 `%TEMP%`，用完即删 |
| 源文件 | `D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001`，文件最新会话 574 |
| 启动 | 2 s 后 `/health`：`initialization.status = model_ready`、`features.history = available`、`degraded_sections = [dbnum_consistency]`（与 refactor 侧相同） |
| Schema | `[E3D schema] 加载成功: mode=manual source=DbOption e3d_version=e3d31 auto_detect=false schema=e3d31`（`aios-database.stdout.log`） |

## 用例（`m-*.json`）

| # | 请求 | 结果 | 与 refactor 侧对照 |
|---|---|---|---|
| 1 | BRAN 24384/26229 · latest | ✅ `24384_26229@574`，visited 13 / generated 16 | 一致 |
| 2 | 同根 · sesno=250 | ✅ `@250`，16 件全部 mesh_reused | 一致 |
| 3 | 同根 · sesno=100 | ❌ `REFNO_NOT_FOUND_AT_SESSION: refno 24384/26229 is not in the element index of dbnum 8000 at session 100` | 一致（86f4a0b94 的修复已在主线） |
| 4 | sesno=99999 | ❌ `SESSION_NOT_FOUND … newest is 574` | 一致 |
| 5b | time=2026-09-01T00:00+08 | ✅ 解到 264（2026-08-28 17:21） | 一致 |
| 6 | dbnum 7998 + refno 24384/26229 | ❌ `INVALID_SELECTOR` | 一致 |
| 8 | EQUI 24384/24776 · latest | ✅ `@574` generated 1 | 一致 |
| 9 | 同根 · sesno=556 | ✅ `@556` generated **2**（多出 BOX 24384_26467） | 一致 |
| 10 | 同根 · sesno=573 | ✅ `@573` generated 1 | 一致 |
| — | `query instances @556 / @574`、`query geometry @250`、`DELETE @573` → 之后 `GET` 404 | 2 行 / 1 行 / 10 行；deleted | 一致 |

## 合并本身的验证（见 merge 提交说明）

- `cargo check --lib` 通过；`cargo test --lib` 全量 **1901 passed / 0 failed / 110 ignored**。
- 7 个冲突文件的裁决与取舍（SchemaPack 缺省口径取 refactor 侧）写在 `793176510` 的提交说明里。
