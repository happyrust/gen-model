# stash pop 之后的 e3d-direct 工作树 · AMS 验证 · 历史模型（按 sesno / time 生成）· 2026-09-14 01:04

**目的**：`gen-model-refactor` 并入 `e3d-direct`（`793176510`）并在真库上复验（`1fcb32934`）之后，把 stash 里的
主工作树在途改动回灌回来（裁决见 `verification/stash-pop-20260913-e3d-direct-wip/README.md`）。这一轮是在
**回灌后的工作树**（`1fcb32934` + 8 个文件的未暂存 WIP 改动）上再跑一遍同一组 live 用例，确认保留下来的
WIP 没有改变历史模型这条路的行为。

**结论**：10 条用例逐条与合并提交那一轮（`verification/historical-model-live-20260913-merged-e3d-direct`）
以及 refactor 侧（`gen-model-refactor/verification/historical-model-live-20260913-2233`）**回执一致**；
快照查询 / 删除的生命周期也一致。

## 现场

| 项 | 值 |
|---|---|
| 二进制 | debug `aios_database.exe`，`0.1.23+g1fcb32934b4a.1789318917.dirty`（`build-identity.json`；`git_commit=1fcb32934b4a`、`git_dirty=true` = 本工作树 + WIP） |
| 怎么拿到的 | `D:\Rust\target` 被多个 worktree 共用，`cargo build` 一开始把 refactor worktree 00:52 刚编出的 `g86f4a0b9417f` 当成「fresh」；`touch src/lib.rs src/main.rs` 后重编（1 m 23 s），复制到 `%TEMP%` 运行目录，`version --json` 核过身份再起 |
| 配置 | `DbOption.toml`（本目录副本，与合并那一轮同源，只换 `meshes_path`）：AvevaMarineSample / MDB `ALL`，`read-through` + `embedded-mem`，`http_api_addr = 127.0.0.1:8047`，`e3d_version = "e3d31"`；运行目录在 `%TEMP%`，用完即删 |
| 源文件 | `D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001`，文件最新会话 574 |
| 启动 | 2 s 后 `/health`：`initialization.status = model_ready`、`features.history = available`、`degraded_sections = [dbnum_consistency]`（与前两轮相同） |
| Schema | `[E3D schema] 加载成功: mode=manual source=DbOption e3d_version=e3d31 … schema=e3d31`（`aios-database.stdout.log`） |

## 用例（`m-*.json`）

| # | 请求 | 结果 | 与前两轮对照 |
|---|---|---|---|
| 1 | BRAN 24384/26229 · latest | ✅ `24384_26229@574`，visited 13 / generated 16 / failed 0，291 ms | 一致 |
| 2 | 同根 · sesno=250 | ✅ `@250`（2026-08-20），16 件全部 mesh_reused | 一致 |
| 3 | 同根 · sesno=100 | ❌ `REFNO_NOT_FOUND_AT_SESSION: refno 24384/26229 is not in the element index of dbnum 8000 at session 100` | 一致 |
| 4 | sesno=99999 | ❌ `SESSION_NOT_FOUND … no session 99999; newest is 574` | 一致 |
| 5b | time=2026-09-01T00:00+08 | ✅ 解到 264（2026-08-28 17:21） | 一致 |
| 6 | dbnum 7998 + refno 24384/26229 | ❌ `INVALID_SELECTOR: refno 24384/26229 belongs to dbnum 8000, not requested dbnum 7998` | 一致 |
| 7 | sesno 与 time 同给 | HTTP 400（请求期拒绝，`m-case7-both-selectors.error.json`） | 一致 |
| 8 | EQUI 24384/24776 · latest | ✅ `@574` generated 1 | 一致 |
| 9 | 同根 · sesno=556 | ✅ `@556` generated **2**（多出 BOX 24384_26467） | 一致 |
| 10 | 同根 · sesno=573 | ✅ `@573` generated 1 | 一致 |
| — | `query instances @556` / `@574` | 2 行 / 1 行，差集 = `24384_26467` 仅在 @556 | 一致 |
| — | `query geometry @250` / `tubes @250` / `snapshot @556` | 10 行 / 200 / 200 | 一致 |
| — | `GET /model/history/24384_24776@556` / 不存在的 `@999` | 200 / 404 | 一致 |
| — | `DELETE @573` → `GET @573` / `instances @573` / 邻居 `instances @574` | deleted → 404 / 0 行 / 1 行 | 一致 |
| — | 独立 mesh 目录落盘 | 11 个文件 | 一致 |

## 回灌保留下来、与这条路有关的 WIP

- `e3d_model_service.rs` 的 `classify_required_root_error` 插在 Required 策略的失败链上：用例 3 仍是
  `REFNO_NOT_FOUND_AT_SESSION`（索引门在生成之前就拦下了，走不到那一层），用例 1/2/8/9/10 生成计数不变。
- 其余保留项（`evict_missing_root` / `current_cached_roots` / mesh 读入校验单测 / `model_coverage` SQL / `surreal_retry` 测试隔离 /
  `New-LocalRelease.ps1` 的 `Get-Sha256Compat`）不在 `/model/history/*` 的调用链上。

## 收尾

服务已停（PID 10252），`%TEMP%` 运行目录已删，8047 释放。本目录是这一轮新增的唯一产物，未提交。
