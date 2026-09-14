# 2026-09-14 校审闭环落主线计划：`codex/review-backend-closure` → `gen-model-refactor`

状态：**全部完成**（2026-09-14）。经 Plannotator `--gate` 批准（11:10，`{"decision":"approved"}`，无批注；fable-5-1-65 起草）。Q1–Q6 已于 11:12 拍板（§7 / §10）。**P0 / P1 于 13:07 落地**（合并提交 `afa0ecb69`，fable-5-1-6 执行）；**P2 / P3 / P4 于 14:49–15:10 落地**（fable-5-1-11 执行，见 §10）。V1–V8 全绿。
上游：`docs/plans/2026-09-12-review-backend-port-plan.md`（T1–T5 已完成，A1–A11 见其 §6）、`docs/evidence/2026-09-14-review-closure.md`（隔离分支实跑记录）。
约束：d-368（ADR-074 两档）、d-398（默认 `mem` 下校审域 503）、d-412（主线唯一是 `gen-model-refactor`，`e3d-direct` 已删、远端也删；取代 d-400）。

## 0. 一句话

把已在隔离分支验收过的校审闭环——`codex/review-backend-closure@8537a9c3f`（= `feature/review-backend-port@4400d8bab` × `e3d-direct@02d445ba4` × `gen-model-refactor@54a7c8b9d`）——补齐 6 处收尾后 `--no-ff` 合进 `gen-model-refactor`，前端把校审面的默认地址翻到 `:8022`、联调文档写清「`mem` 档校审域 503」，发 0.1.24，然后把过渡分支、worktree、遗留进程清掉。

## 1. 现状（2026-09-14 10:55 全部核过）

### 1.1 分支拓扑（d-412 之后）

| 引用 | 位置 | 说明 |
| --- | --- | --- |
| `gen-model-refactor` | `54a7c8b9d`（09-14 02:10） | **唯一主线**。工作树 `D:\work\plant-code\old\gen-model-refactor` 有 **28 个在途已改文件 + 一批未跟踪**（清单在 `verification/review-closure-20260914/refactor-preexisting-status.txt`），不属于校审。 |
| `codex/review-backend-closure` | `8537a9c3f`（10:21） | 待合入的隔离集成分支；worktree `gen-model-review-integration`；codex 会话仍在其上写证据文档（3 份 unstaged）。 |
| `feature/review-backend-port` | `4400d8bab`（09:54） | 校审开发分支，已全部进 closure；worktree `gen-model-review-port`。 |
| `e3d-direct` | **已删** | 兜底标签 `archive/e3d-direct-20260914 -> 02d445ba4`；其 8 个提交全部可从 closure 到达。远端 `origin/e3d-direct` / `github/e3d-direct`（`cf168862a`）未删。 |
| 合并预演 | 标签 `archive/gm-merge-rehearsal-20260914 -> 88fe28496` | worktree 与 9.3 GB target 已删；`88fe28496` 里有「夹具显式开关」那版闸测试写法，Q2 备选。 |

`8537a9c3f` 已包含 `54a7c8b9d`：只要主线不再前进，合入就是一次无冲突 merge（`--no-ff` 留合并点）。

### 1.2 验证状态（closure HEAD `8537a9c3f`）

| 项 | 结果 | 出处 |
| --- | --- | --- |
| 全库 `cargo test --lib --features http_api` | 2035 passed / 0 failed / 107 ignored | `refactor-full-tests.log`（10:20:49） |
| 校审域单测 | 150 / 150 | `refactor-review-tests-2.log` |
| A3 路由 / A6 双重启 / A9 内存拒绝 | PASS / PASS / PASS | `integrated-acceptance.json`（10:23:41） |
| A2 / A5 / A10 两端对拍 | PARTIAL | 旧 `plant-model-gen` 编不起来（path patch `pdms-io-fork/crates/parse_pdms_db` 缺 package）；`:3100` 上跑的是 `plant-web-server`，不是旧端 |
| 走查 U1 / U2 / U2 退回 / U3 | PASS | `walkthrough/out/20260914-integrated-*` |
| 工作流 CLI / PMS JSON 六条 | 53 / 53、36 断言 | `integrated-workflow-cli.log`、`integrated-json-cli.log` |
| 真模型在前端能开 | PASS | `integrated-real-model.json`（AMS 8000、`24384_22402`，mesh 200，页面零错） |

### 1.3 closure HEAD 里的已知瑕疵（都小，但不该带进主线）

1. **16 处校审语义的 `ADR-072`** 没跟着改成 075：`rs_surreal/review/review_tables.surql` ×3、`src/options.rs:3623`、`src/web_service/mod.rs:415 / :567`（**:477 是主线自己的 ADR-072 D5，不动**）、`docs/evidence/2026-09-13-review-schema-parity.md` ×7、`docs/evidence/2026-09-13-review-port-acceptance.md` ×3。前 4 个文件的 13 处已有现成补丁 `%TEMP%\gm-integration-fixups.patch`（`git apply -p2 --check` 干净）。
2. `src/web_service/review/db.rs` 提交进去的是**混合换行**（`git ls-files --eol` = `i/mixed`）。
3. `changelog.md` 顶部顺序 `2026-09-11 → 2026-09-13 → 2026-09-10`，不是倒序。
4. `fresh_review_db()` 的持久化闸被 `#[cfg(not(test))]` 整段编掉（`db.rs:346`），26 条 `platform::tests` 因此转绿；代价是「Refused → `fresh_review_db` 回 Err」这条路在单测里永远走不到（闸自身的 5 条单测只直调 `review_persistence_for`）。
5. `DbOption.toml` 的 `e3d_version` 注释取了 e3d-direct 0.1.22 时代的 4 行版，主线是 1 行（纯注释）。
6. `docs/evidence/2026-09-14-review-closure.md` 末段仍写「不将隔离集成分支合入 `e3d-direct`」——目标分支已由 d-412 改为 `gen-model-refactor`。

另：`cargo fmt --check` 在 closure 上报 293 个文件有差（`refactor-fmt-check.log`），是主线存量漂移，**不在本计划范围**。

### 1.4 前端与文档

- `plant3d-web/.env.development`：`VITE_GEN_MODEL_API_BASE_URL=http://localhost:3100`（校审面仍指旧端），`VITE_GEN_MODEL_V1_BASE_URL=http://localhost:8022`（模型面已切）。
- `plant3d-web/docs/guides/gen-model-v1-local-dev.md` 没有一句提到「`mem` 档下校审域 503、前端表现为 `mock-disabled`」（d-398 要求写进去）。
- `gen-model-refactor/CONTEXT.md` 里「校审」零命中：校审域 / 持久化闸 / 逃生门三个词没有词条。
- 主线最新发布说明是 `docs/release-notes/0.1.23.md`，`Cargo.toml` 版本 0.1.23。

## 2. 目标（Goals）

- G1. `codex/review-backend-closure` 以 `--no-ff` 合进 `gen-model-refactor`；合并后主线 `cargo check --all-targets --features http_api` 干净、`cargo test --lib --features http_api` 0 failed。
- G2. 主线里 `ADR-072` 只剩「publish 阶段退役」这一个含义；校审持久化 ADR 在全仓只叫 ADR-075。
- G3. `plant3d-web` 开发默认配置下校审请求打到 gen-model `:8022`，零 mock 兜底；本地联调文档写清 `mem` 档 503 的现象与两种解法。
- G4. 0.1.24 本地发布包三腿冒烟通过；发布说明含校审域四前缀、ADR-075、`mem` 档口径。
- G5. 过渡分支 / worktree / 遗留进程清理完；决策库（d-368 / d-398 / d-408 / d-410 / d-412）与 ADR、文档口径一致。

## 3. 非目标（Non-goals）

- 复活旧 `plant-model-gen` 做 A2 / A5 / A10 两端 diff（见 Q1；替代判据是 `docs/evidence/2026-09-13-review-shape-parity.md` 的 37/37 路由、21 个响应结构体逐字、状态码集合逐条对照 + 单边实跑）。
- T4 碰撞数据源（P2，无生产者）、`/ws/review*`（N1）、`review-internal-workflow`（N6）。
- `cargo fmt` 存量漂移的整治。
- nginx 正式代理（`scripts/review/nginx-review-split.conf`）的部署与 `nginx -t`——部署方的活，本计划只保证配置文件随主线进包。
- 任何校审业务代码或契约改动（N4：前端零改动、契约以旧端为准）。

## 4. 分阶段与任务

### P0 · 合入前收尾（在 closure 分支上；codex 会话执行，本会话只读复核）

| # | 任务 | 交付 / 验收 |
| --- | --- | --- |
| P0.1 | 16 处 `ADR-072` → `ADR-075`（§1.3-1 清单；`mod.rs:477` 不动） | `git grep -n 'ADR-072' -- src/web_service/review rs_surreal/review scripts/review docs/specs/review-api.md docs/evidence/2026-09-13-review-* src/options.rs src/web_service/mod.rs` 只剩 `mod.rs:477` 一行 |
| P0.2 | `db.rs` 换行统一（建议 LF，与 `src/options.rs`、`src/web_service/mod.rs` 一致），**单独一个提交**，不夹别的改动 | `git ls-files --eol src/web_service/review/db.rs` 单一换行 |
| P0.3 | `changelog.md` 顶部改成倒序 09-13 → 09-11 → 09-10 | 目测 |
| P0.4 | 闸的测试口径按 Q2 定：(a) 保留 `#[cfg(not(test))]`；(b) 换成夹具显式开关（`archive/gm-merge-rehearsal-20260914` 里 `db.rs` 的 `TEST_ALLOW_EPHEMERAL_STORE` + `platform/tests.rs` 四行） | 校审域 150 条照绿；(b) 时补一条「Refused → `fresh_review_db` 回 Err」的单测 |
| P0.5 | `docs/evidence/2026-09-14-review-closure.md` 末段改口：合入目标 `gen-model-refactor`（d-412）；A2 / A5 / A10 的处置按 Q1 结论（d-408，PASS-静态）改写 | 文档 |
| P0.6 | `DbOption.toml` 的 `e3d_version` 注释取主线 1 行版（可选） | 目测 |
| P0.7 | 收尾后在 closure 上复跑：`cargo check --all-targets --features http_api`、`cargo test --lib --features http_api`、`scripts/review/acceptance.ps1`（A3 / A6 / A9） | 三条尾行 + HEAD 贴进 §10 |

### P1 · 合入主线

| # | 任务 | 交付 / 验收 |
| --- | --- | --- |
| P1.1 | **先解主线工作树的 28 个在途改动**（Q3）：由其主人提交或按 `wip/mainwt-stash-20260913` 的做法存成 `wip/mainwt-stash-20260914` 分支。git 不允许同一分支在两棵 worktree 上 checkout，合并只能在 `D:\work\plant-code\old\gen-model-refactor` 这棵树里做；而 `src/web_service/mod.rs`、`changelog.md`、`docs/specs/web-service-api.md` 两边都改了，脏树会被 merge 拒。 | `git -C gen-model-refactor status --short` 为空 |
| P1.2 | `git merge --no-ff codex/review-backend-closure -m "merge(review): 三维校审后台移植合入主线（ADR-075；plan 2026-09-12 T1–T5）"` | 预期零冲突（closure 已含 `54a7c8b9d`）；主线若已前进则先在 closure 上 `git merge gen-model-refactor` 复验再来 |
| P1.3 | 主线上复跑 P0.7 三条 + `git grep -n 'ADR-072' -- src rs_surreal scripts docs/specs docs/evidence/2026-09-13-review-*`（V3） | 尾行进 §10 |
| P1.4 | `CONTEXT.md` 加三个词条：**校审域**（四前缀）、**持久化闸**（`is_durable` 唯一判据、503 + 横幅）、**逃生门**（`PLANT_REVIEW_ALLOW_EPHEMERAL_STORE=1`，只给演示） | 词条互相引用 ADR-075 / d-398 |
| P1.5 | `changelog.md` 加 2026-09-14 条目：校审闭环合入、ADR-075、`e3d-direct` 退场（d-412） | 文档 |

### P2 · 前端与联调文档（`plant3d-web`，P1 之后即可并行）

| # | 任务 | 交付 / 验收 |
| --- | --- | --- |
| P2.1 | `.env.development`：`VITE_GEN_MODEL_API_BASE_URL=http://localhost:8022`，注释保留「要回旧端改成 `:3100`（保留一个发布周期，N5）」 | 前端零代码改动 |
| P2.2 | `docs/guides/gen-model-v1-local-dev.md` 加「校审域要 `rocksdb` 档」一节：`mem` 起 gen-model 时四前缀 503、前端归到 `mock-disabled`；解法一先起 SurrealDB（`bin\surreal.exe` / `scripts/Start-Surreal8009.ps1`）并把 `store_mode = "rocksdb"`，解法二演示用 `PLANT_REVIEW_ALLOW_EPHEMERAL_STORE=1`；注明 U3（S2S）只能走 env 档（loopback 折成同源经 Vite 代理） | 文档 |
| P2.3 | 用主线二进制重跑 `scripts/review/walkthrough`（`?backendPort=8022` 与 env 两档） | 报告里 `mock-used = 0`、`mock-disabled = 0`、`origin_class=unexpected = 0` |

### P3 · 发布 0.1.24

| # | 任务 | 交付 / 验收 |
| --- | --- | --- |
| P3.1 | `docs/release-notes/0.1.24.md`：校审域四前缀与 ADR-075；`mem` 出厂默认下校审域 503 及启用步骤（d-398）；随 e3d-direct 进来的 `-ReuseWebFromZip`、release.json 的 `e3d_io_git_commit`、`e3d_version` 可选默认 e3d210 | 文档 |
| P3.2 | `scripts/New-LocalRelease.ps1` 三腿冒烟（B 腿 `mem`、C 腿包内 `surreal.exe` 起库 + `rocksdb`），顺带验 `-ReuseWebFromZip` | 冒烟日志尾行进 §10 |
| P3.3 | 实包复验 A9：B 腿包起来 `GET /api/review/health` 503 且 body `code = review.persistence.refused`；C 腿 200 | curl 贴进 §10 |

### P4 · 清理

| # | 任务 | 交付 / 验收 |
| --- | --- | --- |
| P4.1 | 合入后删 `feature/review-backend-port`、`codex/review-backend-closure`；`git worktree remove` `gen-model-review-port`、`gen-model-review-integration`、`gen-model-review-baseline` | `git worktree list` / `git branch` 无残留 |
| P4.2 | `verification/review-closure-20260914/`：按 Q6 决定留 json / md / log 证据、删 `runtime/`（rocksdb 数据、meshes、资源副本） | 目录体积 |
| P4.3 | 远端 `origin/e3d-direct`、`github/e3d-direct`（Q5） | `git branch -r` |
| P4.4 | 主工作树 `D:\work\plant-code\old\gen-model`（detached `02d445ba4`，20 项未跟踪）以后当什么用（Q4） | — |
| P4.5 | 杀掉 09-12 / 09-13 起的十余个 `surreal.exe start … memory` 遗留进程（`Get-Process surreal`），它们只是过去几天验收剩下的 | 进程表 |

## 5. 验收标准（第三方可独立判真假）

- V1. `git branch --contains 8537a9c3f` 含 `gen-model-refactor`，且 `gen-model-refactor` 顶端是一个 merge commit（`--no-ff`）。
- V2. 主线：`cargo check --all-targets --features http_api` exit 0；`cargo test --lib --features http_api` 0 failed、passed ≥ 2035。
- V3. 主线 `git grep -n 'ADR-072' -- src rs_surreal scripts docs/specs docs/evidence/2026-09-13-review-*` 的每一行都是「publish 阶段退役」含义（`mod.rs` D5 那行及主线自己的 ADR-072 文件名引用），无一处指校审。
- V4. `git ls-files --eol src/web_service/review/db.rs` 显示单一换行。
- V5. 主线二进制跑 `scripts/review/acceptance.ps1`：A3 46/46、A6 PASS、A9 PASS。
- V6. `plant3d-web` 默认 `.env.development` 起 dev，走查报告 `mock-used = 0`、`mock-disabled = 0`、`origin_class=unexpected = 0`。
- V7. 0.1.24 B 腿包 `GET /api/review/health` → 503 + `review.persistence.refused`；C 腿 → 200。
- V8. `git worktree list` 无 `review-port` / `review-integration` / `review-baseline`；`git branch` 无 `feature/review-backend-port`、`codex/review-backend-closure`。

## 6. 风险

| 风险 | 处置 |
| --- | --- |
| 主线工作树 28 个在途改动一直不收，P1 卡住 | Q3 先拍归属；最晚用 `wip/mainwt-stash-20260914` 分支存走 |
| 合入窗口里主线又前进，closure 过时 | P0.7 与 P1.2 间隔压到同一个工作时段；主线前进就先 `git merge gen-model-refactor` 到 closure 复验 |
| `db.rs` 换行统一是整文件 diff，掩盖真实改动 | 单独提交（P0.2） |
| `#[cfg(not(test))]` 让闸的回归静默 | Q2；至少补一条直调 `fresh_review_db` 的负例（若选 (b)） |
| 前端翻到 `:8022` 后，还在用 `:3100` 校审的同事断链 | 注释写明回退一行；N5 旧端保留一个发布周期 |
| 出厂 `mem` 下校审域 503 被当成故障 | d-398 已定；P2.2 / P3.1 把现象与解法写在最先被看到的地方 |
| 旧端对拍永远补不齐，A2 / A5 / A10 永远 PARTIAL | Q1 拍板：用静态对照 + 单边实跑作替代判据，并在证据文档明说 |

## 7. 开放问题（**2026-09-14 11:12 已拍板：Q1–Q6 全部按推荐项**；括号内「推荐」即为定稿）

- **Q1 A2 / A5 / A10 两端对拍**：旧 `plant-model-gen` 编不起来。(a) 接受「静态逐字对照（37/37 路由、21 结构体、状态码集合）+ 单边实跑」为替代判据，改判 PASS-静态并在证据文档写明；(b) 找一份能编的旧端（补 path patch）再跑一次真对拍。推荐 (a)。→ **定 (a)**，决策 d-408。
- **Q2 闸的测试口径**：(a) 保留 `#[cfg(not(test))]`；(b) 换成夹具显式开关，让闸在单测里活着。推荐 (b)，代价一个小提交。→ **定 (b)**，决策 d-410；P0.4 照此做，并补一条直调 `fresh_review_db` 的 Refused 负例。
- **Q3 `gen-model-refactor` 工作树 28 个在途改动**归谁、何时提交或存分支。→ **P1.1 开工时若仍未被主人收走，按 `wip/mainwt-stash-20260913` 的做法存成 `wip/mainwt-stash-20260914` 分支**，不阻塞合入。
- **Q4 主工作树 `gen-model`** 现在 detached 在 `02d445ba4`：以后专做「主线之外的实验树」还是切到别的分支？→ **当实验树，不动。**
- **Q5 远端 `origin/e3d-direct` / `github/e3d-direct`** 删不删。→ **删**（P4.3 时执行：`git push origin --delete e3d-direct; git push github --delete e3d-direct`），d-400 已按此修订为 d-412（取代 d-400）。
- **Q6 `verification/review-closure-20260914/`**（含 rocksdb 数据与 meshes）：只留 json / md / log，其余删？→ **只留 json / md / log，删 `runtime/`**（P4.2）。

## 8. 顺序与协调

P0（codex，约半天）→ P1（主线树的主人 + codex，卡 Q3）→ P2（plant3d-web，P1 后并行）→ P3 → P4。
本会话（fable-5-1-65）：只读复核 P0 / P1 结果、写 P1.4 / P1.5 / P3.1 文档、把拍板记进决策库；**不碰 codex 与主线的工作树里别人在途的文件**。

## 9. 参考

- `docs/plans/2026-09-12-review-backend-port-plan.md`（母计划）
- `docs/evidence/2026-09-14-review-closure.md`、`docs/evidence/2026-09-13-review-shape-parity.md`、`docs/evidence/2026-09-13-review-port-acceptance.md`
- `docs/adr/ADR-074-the-process-never-spawns-a-database-store-mode-is-mem-or-rocksdb.md`、`docs/adr/ADR-075-review-data-persistence.md`
- 决策 d-368 / d-398 / d-408 / d-410 / d-412（取代 d-400）；标签 `archive/e3d-direct-20260914`、`archive/gm-merge-rehearsal-20260914`
- 补丁 `%TEMP%\gm-integration-fixups.patch`（P0.1 前 4 个文件 13 处）

## 10. 落地记录

- 2026-09-14 11:10 · 计划经 Plannotator `--gate` 批准（`{"decision":"approved"}`，无批注）。
- 2026-09-14 11:12 · Q1–Q6 全按推荐项拍板（fable-5-1-65）：Q1 (a) → d-408；Q2 (b) → d-410；Q3 未收走就存 `wip/mainwt-stash-20260914`；Q4 `gen-model` 当实验树不动；Q5 远端 e3d-direct 也删（P4.3 执行）→ d-412 取代 d-400；Q6 `verification/review-closure-20260914/` 只留 json / md / log。相关既有决策：d-368（ADR-074 两档）、d-398（默认 `mem` 下校审域 503）。
- 2026-09-14 12:50–13:07 · **P0 收尾在 closure 分支上完成**（fable-5-1-6 接手执行：codex 会话自 10:31 起无动作，其 3 份未提交证据 / 计划文档先原样入库 `dc40a34ee`，未改一字）。`e9eb9033c` P0.1 ADR 引用 072→075 改号 16 处（V3：`git grep ADR-072` 校审范围只剩 `mod.rs:477` 主线 D5 与 port-acceptance 首段的撞号说明）；`0d3c27799` P0.2 `db.rs` 行尾归一 LF（`git diff --ignore-cr-at-eol` 为空，V4 ✓）；`15584c412` P0.4 按 Q2 (b)：撤 `#[cfg(not(test))]`，`TEST_ALLOW_EPHEMERAL_STORE` 夹具开关 + `fresh_review_db_under()` 接缝 + Refused→Err 负例 / Ready 不拦正例，校审域 **152 / 152**；`3a26db34f` P0.3 changelog 倒序 + P0.6 `DbOption.toml` 注释对齐主线（对主线只剩 store_mode 注释三行差异，closure 措辞为准）；`1283e8856` P0.5 闭环记录改口（合入目标 `gen-model-refactor`、A2 / A5 / A10 → PASS-静态）。P0.7 于 `1283e8856`：`cargo check --all-targets --features http_api` exit 0（3 个存量 warning）；`cargo test --lib --features http_api` **2037 passed / 0 failed / 107 ignored**（≥ 2035，V2 ✓）。acceptance 见下条 P1.3。
- 2026-09-14 13:07 · **P1.1 / P1.2 合入主线**：主线工作树 28 个在途改动的主人此刻仍在活动（`rvm_compare` 扫描 12:54 起持续跑），故按 Q3 暂存但**不留空**——`git stash push`（只跟踪文件）→ `git merge --no-ff codex/review-backend-closure` → `git stash pop` 三步 2 秒内完成，事前 `git merge-tree` 预演 3-way 零冲突、事后 28 文件 / +4727 −3394 与合入前逐字节相同；暂存另存分支 `wip/mainwt-stash-20260914`（`76eccfa59`，父 `54a7c8b9d`），补丁备份 `%TEMP%\gm-mainwt-inflight-20260914.patch`。合并提交 **`afa0ecb69`**（父 `54a7c8b9d` + `1283e8856`），`git diff --stat HEAD codex/review-backend-closure` 为空（树逐字节等于 closure tip）。V1 ✓。
- 2026-09-14 13:10 · **P1.3**：主线 `afa0ecb69` 的树与 `1283e8856` 相同，P0.7 两条即主线结果；主线工作树因含 28 个非校审在途改动，不能作为「纯主线二进制」的构建来源，V5（acceptance）用 `gen-model-review-integration` 工作树（同一棵树）构建的二进制跑，结果另记。**P1.4 / P1.5**：`CONTEXT.md` 末尾新增「三维校审（Review）」一节三个词条（校审域 / 持久化闸 / 逃生门），`changelog.md` 顶部新增 `## 2026-09-14` 校审闭环合入条目；两文件都有别人的在途改动，本会话只把自己的块写进索引提交（`git hash-object` + `update-index`），工作树里别人的块原样保留为未暂存。顺手发现在途改动把 `## 2026-09-13` 标题误改成了 `## 2026-09-14`（ADR-074 条目因此挂错日期），已在工作树里把 `## 2026-09-13` 标题补回 `### ADR-074` 之前，未动其余。
- 2026-09-14 14:49 · **P3.1 版本收口**（fable-5-1-11，主线提交 **`80bee35e6`**，仅 3 文件）：`Cargo.toml` / `Cargo.lock` 版本 0.1.23 → 0.1.24；`docs/release-notes/0.1.24.md` 入库（校审域四前缀 + ADR-075；出厂 `mem` 下四前缀 503 及两条启用路；随 e3d-direct 进来的 `-ReuseWebFromZip`、`release.json` 的 `e3d_io_git_commit`、`e3d_version` 默认 e3d210）。主线工作树另有 30 个非本计划在途改动（他人 rvm 工作），本提交只带这 3 个路径，未碰其余。
- 2026-09-14 14:51–14:59 · **P3.2 发布包 + 三腿冒烟**：在 `gen-model-review-integration`（detach 到主线 `80bee35e6`，与 `gen-model-refactor` 逐字节同树）跑 `New-LocalRelease.ps1 -ReuseWebFromZip <0.1.23-full> -IncludeWorkspace -ExcludeMeshes -StrictSmoke`（release 构建 7m41s）。产物 `releases\ams-local-0.1.24_release.20260914065130.g80bee35e65f2-runtime-workspace.zip`（211.9 MB，sha256 `fe60bac69c0572f81e3dec395649d4d0c99013a3a8e69ee05845a8aaef32aec9`，backend build_id `0.1.24+release.20260914065130.g80bee35e65f2`，plant-ui 0.1.10）。**三腿全 passed**：A 设置模式（health mode=setup + /setup.html）；B `mem`（medium=mem、durable=False、服务未拉 surreal 子进程）；C `rocksdb`（包内 surreal.exe 起库 + 服务连上、medium=rocksdb、durable=true、数据目录落 7 文件）。日志 `verification/review-closure-20260914/mainline-release-0.1.24.log`。
- 2026-09-14 14:52 · **P1.3 / V5 主线 acceptance**（主线二进制，HEAD `80bee35e6`，48s，`mainline-acceptance.json`）：**A3 46/46 PASS**、**A6 PASS**（任务/记录/评论/批注/历史/工作流/附件哈希双重启后全同）、**A9 PASS**（`mem` 四前缀 503 + 横幅、模型 /health 200）；A2 / A5 / A10 **PARTIAL**（旧 `:3100` 是 plant-web-server、非旧校审端，按 d-408 以静态逐字对照 `review-shape-parity` 为替代判据判 PASS-静态）；A1 / A7 NOT RUN（cargo 计数另跑，P0.7 已附 `2037 passed / 0 failed`）。V2 复用 P0.7。
- 2026-09-14 15:00 · **P2 前端与联调文档**（`plant3d-web`）：P2.1 `.env.development` 的 `VITE_GEN_MODEL_API_BASE_URL` 由 `:3100` 改 `:8022`（该文件 `.gitignore` 内、本地改动，不入库；注释写明回退与 `mem` 档 503 现象）；P2.2 `docs/guides/gen-model-v1-local-dev.md` 新增 **§9 校审域** 一节（`mem` 出厂档四前缀 503 / `mock-disabled`、解法一 rocksdb 档、解法二逃生门 `PLANT_REVIEW_ALLOW_EPHEMERAL_STORE=1`、U3 只走 env 档），提交 **`ae1231b`**（plant3d-web；该库 measurement 会话在途，故只把本节的 index blob 提交、其余在途块原样保留未暂存）。
- 2026-09-14 14:57 · **P2.3 / V6 主线二进制走查**（主线二进制 + 自起 Vite :3102 + rocksdb 库；`mainline-walkthrough.log`、`walkthrough/out/20260914-mainline-{u1u2-query,u3-env}`）：u1+u2（query 档）与 u3（env 档）**全 PASS**；四判据 **mock-used=0、mock-disabled=0、预期外 origin=0、应打未打=0**；契约表 46 条无重复。
- 2026-09-14 15:03 · **P3.3 / V7 实包复验 A9**（解 0.1.24 zip 起包内 `aios-database.exe`；`package-0.1.24-review-a9.json`、`.log`）：**B 腿 `store_mode=mem` → `/api/review/health` 503** body `code=review.persistence.refused`、服务未拉 surreal 子进程；**C 腿包内 surreal 起 rocksdb + `store_mode=rocksdb` → 200**、medium=rocksdb、durable=true。结论 PASS。
- 2026-09-14 15:05–15:09 · **P4 清理**：P4.1 `git worktree remove gen-model-review-integration` + 删 `codex/review-backend-closure`（其 tip `1283e8856` 经合并点 `afa0ecb69` 仍在主线可达）；`gen-model-review-port` / `gen-model-review-baseline` 工作树与 `feature/review-backend-port` 分支此前（13:53，port 在途已存 `wip/review-port-inflight-20260914`）已移除。P4.2 `verification/review-closure-20260914/` 无 `runtime/`（仅 json/md/log 证据，Q6 满足）。**P4.3 远端 `e3d-direct` 已删**（`git push origin --delete` gitee + `git push github --delete` 均 `- [deleted]`，远端跟踪引用已 prune；兑底标签 `archive/e3d-direct-20260914 -> 02d445ba4` 保留）。P4.4 主工作树 `gen-model` 仍 detached 在 `02d445ba4`，不动（Q4）。P4.5 无遗留 `surreal.exe`。顺手删孤立构建缓存 `D:\Rust\target-review-integration`（16 GB，其工作树已移除），D: 空间回到 91.7 GB。
- **V1–V8 复核**：V1 主线顶端 `afa0ecb69` 是 `--no-ff` 合并点、`git branch --contains 8537a9c3f` 含主线 ✓；V2 `2037 passed / 0 failed`（P0.7）✓；V3 校审范围无 `ADR-072`（仅 `mod.rs:477` D5 与 port-acceptance 首段撞号说明）✓；V4 `db.rs` 单一 LF ✓；V5 A3/A6/A9 PASS ✓；V6 走查四判据全 0 ✓；V7 实包 B 腿 503 / C 腿 200 ✓；V8 无 `review-port`/`review-integration`/`review-baseline` 工作树、无 `feature/review-backend-port`/`codex/review-backend-closure` 分支 ✓。证据均在 `verification/review-closure-20260914/`。
