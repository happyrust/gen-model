# 2026-09-12 三维校审后台移植计划：plant-model-gen → gen-model

> 状态：**已批准**（Plannotator 2026-09-12 `decision=approved`，无附注；开放问题 Q1–Q10 全按推荐项拍板；共识记录 d-594）。作者：校审组 PM（fable-5-1-27）。
> 执行 worktree：`D:\work\plant-code\old\gen-model-review-port`（分支 `feature/review-backend-port`，自 `e3d-direct@cf168862a` 切出）。
> 本文是 PRD 形态的移植计划：定「要什么、怎样算成功、先做什么」，**不定技术实现**——
> 怎么写由工程在各条任务里决定，凡改变本文口径处须回写 §11「落地记录」并另开 ADR。

## 0. 一句话

把 `plant3d-web` 三维校审前端今天打到旧后端 `:3100`（`plant-model-gen`）的**校审域接口**
（`/api/review/*`、`/api/users/*`、`/api/auth/*`、`/files/review_attachments/*`）
**逐字兼容**地搬进 `gen-model`（`:8022`）——前端只改一个后端地址即可切换，PMS 平台入站接口
原样可用，校审数据跨重启不丢。

## 1. 现状（2026-09-12 全部核过，出处在括号里）

### 1.1 三方角色

| 角色 | 路径 | 现状 |
| --- | --- | --- |
| 契约方（前端） | `D:\work\plant-code\old\plant3d-web` | Vue + Vite。校审接口经 `src/api/reviewApi.ts` 打到 `getBackendApiBaseUrl({fallbackUrl:'http://localhost:3100'})`（`src/utils/apiBase.ts:103`，env `VITE_GEN_MODEL_API_BASE_URL`，URL 参数 `?backend=` / `?backendPort=` 最高优先）；模型数据已走 gen-model `/api/v1`（`VITE_GEN_MODEL_V1_BASE_URL`，默认 `:8022`）。两套地址互不覆盖。 |
| 源（旧后端） | `D:\work\plant-code\plant-model-gen` | Rust，**非 git 仓库**（同级有 `plant-model-gen.bundle`）。axum 0.8.6（`ws`+`multipart`）+ SurrealDB（github happyrust/surrealdb `dev-3.1` fork）+ `aios_core`（happyrust/rs-core `dev-3.1`，patch 到 `../rs-core`）。Cargo 默认 feature `review` = 「三维校审最小必要集」（`Cargo.toml:186-212`）。监听 `[web_server] port = 3100`（`db_options/DbOption.toml:253-255`）。 |
| 目标（新后端） | `D:\work\plant-code\old\gen-model`（分支 `e3d-direct`，HEAD `cf168862a`，工作树有 34 个未提交改动） | Rust edition 2024，axum 0.8.9（**仅 `ws`，无 `multipart`**）+ tower-http + SurrealDB（gitee happydpc/surrealdb fork）+ `aios_core` = `old-aios-core`（`../vendor/old-aios-core`）。Web 服务在 `src/web_service/`（单 `Router` + `AppState`，`mod.rs:324-409`），监听 `DbOption.toml` `http_api_addr = "0.0.0.0:8022"`。接口规范 `docs/specs/web-service-api.md`。 |

旁支 `D:\work\plant-code\plant-web-server`：一次把 web_server 从 plant-model-gen 抽成独立进程的迁移（`README.md`、`MIGRATION.md`），其 review 走「HTTP SurrealQL CRUD 适配器」且只有 smoke 脚本级验证（`docs/api-implementation-parity.md:29-30`），git 上零提交。**本计划不迁它的代码**，只把它的 `scripts/smoke_review_*.ps1` 当验收脚本参考（见 Q1）。

### 1.2 前端实际消费的校审域契约（`src/api/reviewApi.ts`，行号为调用处）

| 域 | 方法 · 路径 | 出处 |
| --- | --- | --- |
| 鉴权 | `POST /api/auth/token`、`POST /api/auth/verify`；Bearer JWT 存 `localStorage['review_auth_token']` | `reviewApi.ts:98-119`；旧端 `jwt_auth.rs:480-483` |
| 用户 | `GET /api/users`、`GET /api/users/me`、`GET /api/users/reviewers` | `:1786-1803` |
| 提资单/任务 | `POST/GET /api/review/tasks`，`GET/PATCH/DELETE /api/review/tasks/{id}`，`POST …/{id}/start-review|approve|reject|cancel|submit|return`，`GET …/{id}/history`，`GET …/{id}/workflow`，`GET /api/review/tasks/returned` | `:1013-1208`、`:1472`；旧端 `review_api.rs:1673-1687,1733` |
| 批注记录 | `POST /api/review/records`，`GET …/records/by-task/{task_id}`，`DELETE …/records/item/{record_id}`，`DELETE …/records/clear-task/{task_id}` | `:1415-1459` |
| 评论/批注状态 | `POST /api/review/comments`，`GET …/comments/by-annotation/{annotation_id}`，`DELETE/PATCH …/comments/item/{comment_id}`，`PATCH /api/review/annotations/{annotation_id}`，`PATCH …/{annotation_id}/severity`，`POST /api/review/annotation-states/apply`，`GET /api/review/annotation-states` | `:1503-1637`、`:1752-1768`；旧端 `review_annotation_state.rs:148-152` |
| 附件 | `POST /api/review/attachments`（multipart，50MB 上限；前端另有 JSON+base64 兜底分支 `:1829-1850`），`DELETE /api/review/attachments/{attachment_id}`，静态 `GET /files/review_attachments/{stored_name}` | `:2087`；旧端 `review_api.rs:773,5792-6146`，`web_server/mod.rs:1434-1436`（`ServeDir("assets/review_attachments")`） |
| 同步 | `POST /api/review/sync/export`、`POST /api/review/sync/import` | `:2502-2515` |
| PMS 平台入站 | `POST /api/review/embed-url`、`/api/review/workflow/sync`、`/api/review/workflow/verify`、`/api/review/annotations/check`、`/api/review/cache/preload`、`/api/review/delete` | `:1232-1354`；旧端 `platform_api/mod.rs:26-49` |
| 模型侧集成 | `POST /api/review/aux-data`、`GET /api/review/collision-data` | `:2644-2699`；旧端 `review_integration.rs:112-115` |
| 健康 | `GET /api/review/health` | 旧端 `review_api.rs:1752` |
| 实时 | `/ws/review`、`/ws/review/user/{userId}`（`:2096-2114`）——**旧后端从未实现**（`web_server/mod.rs` 只有 `/ws/progress`、`/ws/tasks`），前端在未配 `VITE_REVIEW_WS_BASE_URL` 时自动关闭 | 见 §3 非目标 |

同在 `:3100` 但**不属于校审域**的接口（`/api/dashboard/activities`、`/api/logs*`、`/api/deployment-sites*`、`/api/site/identity`、`/api/model-version/*`、`/api/model-history/*`、`/api/model/units/*`、`/api/model/incremental/*`、`/api/pipeline/annotation/*`）见 §3 与 Q3。

### 1.3 源码盘点（plant-model-gen）

| 文件 | 行数 | 职责 | 对外耦合（移植时要解的） | 难度 |
| --- | --- | --- | --- | --- |
| `src/web_api/review_api.rs` | 7126（含 27 个单测） | 任务/记录/评论/批注/附件/同步/用户 全部 handler + 路由 | 仅依赖同簇 `jwt_auth` / `platform_api` / `review_annotation_state` / `review_db`；`surrealdb::types::SurrealValue`（3.x fork API）；`sha2` | 中（体量大、耦合低） |
| `src/web_api/review_db.rs` | 295 | review 专用 Surreal 连接（独立 WS 连接 + 超时 + 索引/schema 预热） | `aios_core::{get_db_option, use_ns_db_compat, options::DbConnMode}`——**old-aios-core 只有 `get_db_option`**，其余不存在 | **难**（要按 gen-model 的 `SUL_DB: Surreal<Any>`/`ensure_store_connected` 重写，见 Q6） |
| `src/web_api/jwt_auth.rs` | 1085（13 单测） | JWT 签发/校验、角色 `sj/jd/sh/pz`、`review_auth_middleware`、`REVIEW_AUTH_CONFIG` / `PLATFORM_AUTH_CONFIG` | 配置来自 DbOption `[review_auth]` / `[platform_auth]`（`db_options/DbOption.toml:239-251`）+ 环境变量 `JWT_SECRET`；gen-model 的 `DbOptionExt`（`src/options.rs:11`）无这两段 | 中 |
| `src/web_api/review_annotation_state.rs` | 771 | 批注状态（`review_annotation_states` 表） | 同簇 | 易 |
| `src/web_api/platform_api/*`（`workflow_sync.rs` 2430、`annotation_check.rs` 1148、`embed_url.rs` 571、`review_form.rs` 556、`types.rs` 442、`auth.rs`、`cache_preload.rs`、`delete_handler.rs`、`config.rs`、`tests.rs` 29 单测） | ≈5800 | PMS ↔ 校审平台入站接口 | `embed_url.rs:467` → `crate::web_server::managed_project_sites::get_site`（站点注册表，gen-model 无对应物，见 Q5）；其余同簇 | 中 |
| `src/web_api/review_integration.rs` | 377 | `aux-data` / `collision-data` | `aios_core::project_primary_db` 直读旧模型库表 + `collision_events` 表——gen-model 的模型数据面已改为 e3d-direct/RocksDB，**无同形表** | **难**（见 Q4） |
| `src/web_server/dashboard_handlers.rs` | 244 | `/api/dashboard/activities`（读 `review_workflow_history`） | `review_db` | 易（是否纳入见 Q3） |
| `rs_surreal/review/review_tables.surql`、`resource/surreal/review_workflow.surql` | — | 表定义（两份有重叠且不一致：一份 SCHEMAFULL、一份 SCHEMALESS） | 运行时以 `review_db.rs:186-216,241-277` 里的 DDL 为准 | 需先定「以哪份为真」 |
| `shells/test_review_api.sh`、`test_workflow_e2e.sh`、`verify_platform_api_json.sh`、`run_pms_workflow_mock_page.sh`、`scripts/pms-browser-flow.ts` | — | e2e / mock 页 | 写死 `:3100` | 易（改端口复用） |

涉及表：`review_tasks`（SCHEMALESS，多级审批字段 `current_node` / `workflow_history` / `return_reason`）、`review_records`、`review_comments`、`review_annotation_severity`、`review_annotation_states`、`review_attachment`、`review_forms`、`review_form_model`、`review_opinion`、`review_workflow_history`（SCHEMAFULL，RUS-244 形态）、`review_history`、`collision_events`（只读）。

### 1.4 目标侧约束（gen-model）

1. 单 `Router` + `AppState`（`src/web_service/mod.rs:86-104`），`ApiError` 统一为 `{code,message,detail}`（spec §3）。**校审域接口的响应形状以前端为准**（`{success, …}` 风格，`reviewApi.ts` 逐处判 `response.success`），不套 gen-model 的错误信封——先例：`/api/mbd/v2/pipe/{refno}` 「路径是 plant3d-web 写死的契约（不在 /api/v1 下）」（`mod.rs:395-397`）。
2. 持久层唯一是 SurrealDB（`Cargo.toml:35`「宪法 I」；`.specify/memory/constitution.md` 附加约束）；连接的唯一入口是 `ensure_store_connected` → `SUL_DB: Surreal<Any>`，介质有 `EmbeddedMem / SpawnedMem / SpawnedRocks / External` 四档（`src/lib.rs:1879-1913`）。**内存两档重启即空**——校审数据落哪儿是本计划最大的架构问题（Q6）。
3. axum 需加 `multipart` feature；上传路由需单独放宽 body 上限（旧端 50MB + 报文余量）。
4. 静态附件目录：gen-model 已有 `asset_root`（`PLANT_ASSET_ROOT`，`mod.rs:433-455`）与 `/assets` ServeDir；`/files/review_attachments/*` 需新增挂载，路径逐字不变。
5. 宪法 III「静默失效是最高级别缺陷」、VI「不变量由可执行的守护看住」：路由清单必须有 include_str!/源码文本护栏测试（gen-model 惯例，`mod.rs:536-612`；旧端亦有 `stateless_web_api_route_paths()` 第二道护栏，`web_api/mod.rs:86-181`）。
6. 开发机 Windows / PowerShell，**禁止 `cargo clean`**（宪法附加约束）。
7. 提交信息只引 ADR / plan 路径，不写任务编号（决策 d-536 第 7 条）。

## 2. 目标（Goals）

G1. `plant3d-web` 以 `?backendPort=8022`（或 `VITE_GEN_MODEL_API_BASE_URL=http://localhost:8022`）打开时，§1.2 表中除「实时」「模型侧集成」两行外的**全部**接口由 gen-model 提供，路径、方法、请求/响应形状与旧端一致，前端零代码改动、零 mock 兜底。
G2. PMS 平台入站六条接口（`embed-url / workflow/sync / workflow/verify / annotations/check / cache/preload / delete`）在 gen-model 上可用，`[platform_auth]` S2S 鉴权语义不变。
G3. 校审数据（任务、记录、评论、批注状态、附件文件与元数据、工作流历史）**跨 gen-model 重启保留**。
G4. JWT 鉴权（`[review_auth]`、`JWT_SECRET`、角色 `sj/jd/sh/pz`、debug 直通）行为不变，包括 401/403 的错误文本。
G5. 旧端已有的 82 个单测（review_api 27 / jwt 13 / workflow_sync 15 / annotation_check 18 / platform tests 29）移植后全绿；e2e 脚本改端口后全绿。
G6. `docs/specs/` 有校审接口规范；持久化落点有 ADR；本文 §11 有落地记录。

## 3. 非目标（Non-goals）

N1. `/ws/review*` 实时通道——旧端未实现，前端默认关闭；本次不建（Q9 确认）。
N2. §1.2 末段列出的 `:3100` 非校审域接口（dashboard / logs / deployment-sites / model-version / model-history / incremental / pipeline annotation）——不在本次范围；其中 `/api/dashboard/activities` 的校审活动流列为 **P2 可选**（Q3）。
N3. `plant-web-server` 独立进程形态、其 JSON 本地持久化模式（`PLANT_WEB_REVIEW_BACKEND=local`）——不迁。
N4. 前端重构 / 前端接口形状「顺手改好」——契约以现状为准，前端不动。
N5. 旧后端 `:3100` 下线——保留一个发布周期作对照（与 `VITE_MODEL_SOURCE=legacy` 同口径，`.env.example:28`）。
N6. `review-internal-workflow`（内部按钮流转，默认关闭 feature）——默认**不移植**（Q7）。
N7. 模型数据面接口（tree / attributes / ptset / meshes）——已由 gen-model `/api/v1` 提供，不重复。

## 4. 用户路径（验收走查用）

U1 **设计人员（sj）**：以 `debug_role=sj` 取 token → 查看「我发起的」列表 → 新建提资单（选模型构件、填说明、上传附件）→ 提交到下一节点（jd）→ 看到工作流历史一行 `submit`。
U2 **校对/审核/批准（jd/sh/pz）**：取 token → 「待我审」列表 → 打开任务 → 在三维视图加批注记录（`records`）+ 评论（`comments`）+ 严重度（`severity`）→ 上传云线附件 → `approve` 或 `return`（带原因）→ 历史与 `workflow` 端点可回放。
U3 **PMS 平台（S2S）**：`embed-url` 换取带 `user_token` 的嵌入地址 → 浏览器以 embed 模式打开 → PMS 调 `workflow/sync` 推节点 → `workflow/verify` 核对 → `annotations/check` 查未闭环批注 → 结束后 `delete` 软删。
U4 **运维**：重启 gen-model → 以上数据全部还在；`GET /api/review/health` 200；`review_auth.enabled=true` 时匿名请求 401。

## 5. 分阶段与任务拆分（按此刻在线 3 位工程拆；文件归属互不重叠）

依赖图：`T1 → {T2, T3}`（T2/T3 可先对着 T1 定下的持久层接口开工，集成在 T1 交付后）；`T4` 依赖 T1–T3；`T5` 最后。

### T1 · 落点、持久层与配置（基础设施）
- objective：在 gen-model 建立校审模块的骨架、持久层访问方式、配置段与静态附件挂载，让 T2/T3 的 handler 有地方落、有 DB 可用。
- 文件：`Cargo.toml`（axum `multipart`、`sha2`/`jsonwebtoken` 等新依赖）、`src/options.rs`（`DbOptionExt` 增 `[review_auth]` `[platform_auth]` `[review_attachments]`）、`src/web_service/review/{mod.rs,db.rs,schema.rs}`（新）、`rs_surreal/review/*.surql`（以 `review_db.rs` 运行时 DDL 为准合并两份 surql）、`src/web_service/mod.rs`（挂载点 + `/files/review_attachments` ServeDir + body limit）、`docs/adr/ADR-0xx-review-data-persistence.md`（新）。
- deliverable：可编译的空模块（`/api/review/health` 已通）、持久层访问 API（供 T2/T3 调用的函数签名与超时/慢查询日志口径）、schema 首启自动建表建索引、ADR 写明校审数据落哪一档介质及理由。
- acceptance：见 §6 A1、A2、A6、A9。
- constraints：不得违反宪法 I（只用 SurrealDB）；`SUL_DB` 为内存介质时启动必须**响亮拒绝**启用校审模块或按 ADR 走独立持久连接（宪法 III，不许静默降级）。

### T2 · 鉴权 + 用户 + 任务/工作流/记录/评论/批注/附件/同步（`review_api.rs` 主体）
- objective：把 `jwt_auth.rs`、`review_api.rs`、`review_annotation_state.rs` 移入 `src/web_service/review/{auth.rs,tasks.rs,records.rs,comments.rs,attachments.rs,sync.rs,users.rs,annotation_state.rs}`（拆法由工程定，但**文件归属只归 T2**），路径与响应形状逐字不变。
- deliverable：§1.2「鉴权/用户/任务/记录/评论·批注状态/附件/同步/健康」八行全部端点 + 迁入的 27+13 个单测 + 路由清单护栏测试。
- acceptance：§6 A3、A4、A5、A7。
- constraints：响应信封沿用旧端 `{success,…}`；附件落盘目录与 URL 前缀不变；`Multipart` 上限与 413 语义不变（`review_api.rs:5792-5841`）。

### T3 · PMS 平台入站（`platform_api/*`）
- objective：迁 `platform_api` 六接口 + `review_form.rs` 到 `src/web_service/review/platform/`。
- deliverable：六端点 + 迁入的 15+18+29 个单测；`embed_url` 的站点注册表依赖按 Q5 结论替换；`shells/verify_platform_api_json.sh`、`test_workflow_e2e.sh` 改端口后可跑。
- acceptance：§6 A8。
- constraints：`[platform_auth]` 与 `[review_auth]` 分离的语义不变；`workflow_sync` 写 `review_workflow_history` 的 `source = pms-sync` 口径不变。

### T4 · 模型侧集成（`aux-data` / `collision-data`）—— P2，Q4 拍板后开
- objective：用 gen-model 的数据面（`QueryService` / direct store）重写 `review_integration.rs` 两端点，形状不变。
- deliverable：两端点 + 「数据源不存在时的显式空结果 + 原因」；`collision_events` 在 gen-model 没有来源时的处置写进 ADR。
- acceptance：§6 A10。

### T5 · 端到端验收与文档
- objective：把 §6 全部判据跑一遍并留痕。
- deliverable：`docs/evidence/2026-09-xx-review-port-acceptance.md`（命令、输出、截图）、`docs/specs/review-api.md`（或 web-service-api.md §4.13，Q10）、本文 §11 落地记录。
- acceptance：§6 全部条目有 PASS/FAIL 记录，FAIL 有归因。

## 6. 验收标准（每条第三方可独立判真假）

| # | 判据 | 怎么判 |
| --- | --- | --- |
| A1 | 目标分支 `cargo check --features http_api` 与 `cargo test`（纯函数层）全绿，且未改动任何既有 `/api/v1/*` 测试 | CI/本机命令输出 |
| A2 | gen-model 启动后 `GET http://localhost:8022/api/review/health` 返回 200，JSON 键集与旧端 `:3100` 相同 | 两端 curl 对比 |
| A3 | §1.2 表「鉴权/用户/任务/记录/评论·批注状态/附件/同步/健康」全部 **42 条**方法·路径在 gen-model 上均非 404，且有一条路由清单护栏测试（源码文本断言）钉住 | `cargo test` + 逐条 curl 脚本 |
| A4 | 走查 U1、U2 全程无一次 mock 兜底（`VITE_REVIEW_ALLOW_MOCK_FALLBACK=false`，浏览器控制台无 fallback 告警），附件上传 multipart 成功且 `/files/review_attachments/...` 可打开 | 实机走查 + 截图 |
| A5 | `review_auth.enabled=true` 时：无 Bearer → 401，错误文本与旧端一致；角色不足 → 403，文本一致（`jwt_auth.rs:513,647`） | curl 对比 |
| A6 | 重启 gen-model 后，U1/U2 创建的任务、记录、评论、附件、工作流历史全部可读 | 重启前后 `GET` 对比 |
| A7 | 旧端 82 个单测移植后全部通过（数量不少于 82） | `cargo test` 计数 |
| A8 | `test_workflow_e2e.sh` / `verify_platform_api_json.sh` 指向 `:8022` 全绿；`platform_auth.enabled=true` + `debug_token` 语义与旧端一致 | 脚本输出 |
| A9 | `SUL_DB` 处于内存介质且未按 ADR 配置独立持久连接时，启动日志明确拒绝启用校审模块（非 warning 一行了事） | 构造配置复现 |
| A10 | （P2）`aux-data` / `collision-data` 形状与旧端一致；无数据源时返回显式空集 + `reason` 字段 | curl 对比 |
| A11 | 存在 `docs/specs/review-api.md`（或 §4.13）、持久化 ADR、本文 §11 落地记录 | 文件存在且被引用 |

## 7. 风险

| 风险 | 影响 | 缓解 |
| --- | --- | --- |
| 两个 SurrealDB fork（github happyrust dev-3.1 vs gitee happydpc）SDK API 不同（`surrealdb::types::SurrealValue`、`IndexedResults`） | 7k 行 handler 中的取值代码大面积编译错误 | T1 先做一份「SDK 差异对照 + 最小样例编译」，再放 T2/T3 开工 |
| `review_db.rs` 依赖的 `use_ns_db_compat` / `effective_surrealdb` 在 old-aios-core 不存在 | 持久层整体重写 | T1 用 gen-model 现有 `ensure_store_connected` 路径重写，ADR 钉住 |
| 内存介质丢数据 | 校审数据重启即空（违反 G3） | A9 + Q6 |
| `review_api.rs` 单文件 7k 行，拆分时路径/形状走样 | 前端静默失效 | A3 护栏 + 前端 `reviewApi.test.ts`（45KB）作契约参照 |
| plant-model-gen 非 git，源基线漂移 | 移植到一半源被改 | 开工前对 `src/web_api/{review_*,jwt_auth.rs,platform_api}` 做一次 sha256 清单存 `docs/evidence/` |
| gen-model 工作树有 34 个未提交改动 | 混入无关变更 | 在新 worktree 上做（Q2） |

## 8. 开放问题（**2026-09-12 已拍板：Q1–Q10 全部按推荐项**；下文括号内「推荐」即为定稿）

- **Q1 源码基线**：以 `plant-model-gen` 当前目录为唯一源，`plant-web-server` 只借 smoke 脚本思路、不迁其代码？（推荐：是）
- **Q2 落地分支**：新建 worktree `D:\work\plant-code\old\gen-model-review-port`，分支 `feature/review-backend-port`，基于 `e3d-direct`？（推荐：是；沿用 `gen-model-ptset-api` 的做法）
- **Q3 范围边界**：`:3100` 上的非校审接口不纳入；`/api/dashboard/activities` 校审活动流作 P2 可选？（推荐：是）
- **Q4 `aux-data` / `collision-data`**：列为 P2（T4），先交 T1–T3；若 gen-model 没有碰撞数据源，则返回显式空集并在 ADR 记「碰撞数据源待接」？（推荐：是）
- **Q5 `embed-url` 的站点注册表依赖**：用 `DbOptionExt` 新配置项（校审 Web 基址）替代 `managed_project_sites::get_site`？（推荐：是）
- **Q6 校审数据持久化落点**：要求「跨重启保留」是硬约束；实现是（a）复用 `SUL_DB` 且要求 durable 介质、（b）独立持久 Surreal 连接/独立 ns·db，由工程在 T1 以 ADR 定？（推荐：硬约束确认 + 工程定 a/b）
- **Q7 `review-internal-workflow`**：不移植（推荐），还是作为默认关闭的 cargo feature 一并带过来？
- **Q8 前端切换**：只靠 `VITE_GEN_MODEL_API_BASE_URL` / `?backendPort=8022` 指向 gen-model，前端零改动；旧 `:3100` 保留一个发布周期？（推荐：是）
- **Q9 `/ws/review*`**：确认为非目标？（推荐：是）
- **Q10 规范位置**：新建 `docs/specs/review-api.md` 并在 `web-service-api.md` §4 末尾加一行索引（推荐），还是直接写成 §4.13？

## 9. 顺序与协调

1. 本文经 Plannotator 批准 → `set_team_goal` 落库 → 开放问题按批注定稿。
2. 派活：T1 → 工程 A；T2 → 工程 B；T3 → 工程 C（三人并行；T2/T3 前半段只做搬运与拆文件，不碰 T1 的文件）。
3. T1 交付并验收后，T2/T3 接入真实持久层；再派 T4（谁先空谁领）与 T5。
4. 每条任务 `complete_task` 时附：改动文件清单、`cargo test` 计数、对应 A 条判据的输出。

## 10. 参考

- 前端契约：`D:\work\plant-code\old\plant3d-web\src\api\reviewApi.ts`、`src/utils/apiBase.ts`、`.env.example`
- 旧端：`D:\work\plant-code\plant-model-gen\src\web_api\{mod.rs,review_api.rs,review_db.rs,jwt_auth.rs,review_annotation_state.rs,review_integration.rs,platform_api\}`、`src/web_server/{mod.rs,dashboard_handlers.rs}`、`rs_surreal/review/review_tables.surql`、`resource/surreal/review_workflow.surql`、`db_options/DbOption.toml:239-265`、`shells/test_*.sh`
- 目标：`D:\work\plant-code\old\gen-model\src\web_service\mod.rs`、`src/lib.rs:1879-1913`、`src/options.rs`、`docs/specs/web-service-api.md`、`.specify/memory/constitution.md`
- 既有决策：d-536（提交信息规范）、d-559（gen-model 新端点先例 `POST /api/v1/element/ptset`）、d-438（`/api/mbd/v2` 契约路径先例）

## 11. 落地记录

（随各任务交付追加：日期 · 任务 · 改动文件 · 验收条目 · 与本文口径的偏离及 ADR 编号）

- 2026-09-12 · PM · 计划经 Plannotator 批准；Q1–Q10 全按推荐项；`set_team_goal` 已落库；共识 d-594；worktree `gen-model-review-port`（`feature/review-backend-port` ← `e3d-direct@cf168862a`）已建；本文复制入该 worktree，由 T1 随首个提交入库。T1/T2/T3 已派给三位工程（T2/T3 前半段只搬运拆文件，不碰 T1 的文件）。
- 2026-09-12 · 基线提交 `033b61f70`（T1a 骨架 + T2 前半段搬运）· 改动：`Cargo.toml`/`Cargo.lock`（axum `multipart`；`jsonwebtoken`/`base64`/`uuid` 随 `http_api`；`tower` dev-dep）、`src/web_service/mod.rs`（`.merge(review::router())` + 护栏测试）、`src/web_service/review/{mod,common,auth,users,tasks,records,comments,annotation_state,attachments,sync}.rs`（持久层接缝暂走 `SUL_DB`）、`review/platform/mod.rs`（7 个契约符号的编译脚手架）、`review/integration.rs`（11 行空壳）、`scripts/review/test_review_api.sh`（旧 shells 脚本改指 `:8022`）、本文入库 · 验收：`cargo check --features http_api` 绿；A2 静态半边（health 键集测试）· 偏离：无（T1b 持久层 / T3 真实实现 / T4 各自另记）。
- 2026-09-13 · T4 模型侧集成 + T5 文档半边（fable-5-1-13，t-58）· 改动：`src/web_service/review/integration.rs`（`POST /api/review/aux-data`、`GET /api/review/collision-data` 两端点 + 8 条单测：401、键集、`reason`、路由清单护栏）；`docs/specs/review-api.md`（新，§0–§12，覆盖 §1.2 表全部契约与 §1.4 信封例外）；`docs/specs/web-service-api.md`（§4 末尾一行索引，Q10）；`scripts/review/test_workflow_e2e.sh`、`scripts/review/verify_platform_api_json.sh`（自旧端 `shells/` 搬入，默认地址 `:3100` → `:8022`）+ `scripts/review/platform_api_json/{auth_token,workflow_sync_query,workflow_sync_active,cache_preload,review_delete}.json`（verify 脚本要的夹具；旧端 `shells/platform_api_json/` 在源目录里不存在，按 `jwt_auth.rs::TokenRequest` / `platform_api/types.rs` 补齐）；本文 §11 · 验收：A10 静态半边 `cargo test --lib web_service::review::integration` 8/8 绿，响应键集比旧端只多 `reason`（无数据源时在场、`total:0`、无 mock）；A10 两端 curl 对比 **NOT RUN**（本机 `:8022` / `:3100` 均未在跑，`:8022` 侧响应体已由进程内 router 取得并记入交付，`:3100` 侧按源码 mock 路径对照，留 T5 端到端时补）；A11 文件存在且被索引；A8 两脚本 `bash -n` 通过、**NOT RUN**（待 T3 六条落地）· 偏离与说明：① Q4 落地形状——两端点新增 `reason: Option<String>`（无数据源时以 `collision_source_unavailable:` 开头；有数据源时不序列化），**不再回落 mock**（旧端静默造两条假碰撞，宪法 III）；数据源接缝 `integration::CollisionSource`，今天恒 `Unavailable`，「碰撞数据源待接」记在 `docs/adr/ADR-072-review-data-persistence.md` §未决（文件归 T1b，内容由 T4 提）；② `verify_platform_api_json.sh` 第 2 步原硬断言 `claims.workflow_mode == manual` 只在旧端 `review-internal-workflow` feature 构建下成立（Q7 / N6 不移植，`auth.rs` 按此不写 claims），改为 `EXPECT_WORKFLOW_MODE` 非空时才判等、默认只断言 `code==200` 并打印实际值；③ 两脚本以 LF 落库（仓内脚本主流；源为 CRLF）。
- 2026-09-13 · T2 收尾 / R3（opus-5-6，t-57，已验收）· 改动：`src/web_service/review/common.rs`（持久层接缝改为再导出 `super::db::{await_review_query, await_review_query_long, fresh_review_db}` 与 `super::schema::review_workflow_history_schema_ready`，删本地超时实现与写死的 `SUL_DB.clone()`；新增护栏测试 `review_contract_routes_are_registered_verbatim`（剥注释后按括号配对解析八个子模块 `routes()` + `mod.rs::router()`，与常量表 `REVIEW_CONTRACT_ROUTES` 双向集合比对，两次变异验证真跑出 FAILED）与 `contract_routes_are_reachable_without_path_conflicts`；修基线 `Thing` 结构体字面量在 2.1.4 fork `#[non_exhaustive]` 下的 2 个 E0639）、`src/web_service/review/auth.rs`（401/403 四句文本逐字护栏测试，对照旧端 `jwt_auth.rs:513,517,647,657`）、`scripts/review/test_review_api.sh`（修 `set -e` 下 `((PASSED++))` 提前退出；`probe_route` 把 404/405/000 都判失败；新增 `auth` 模式供 A5 两端 diff）· 验收：A7 `cargo test --lib web_service::review` 73 passed；A3 静态半边绿、curl 半边 NOT RUN；A4 NOT RUN；A5 文本断言绿、两端 curl NOT RUN（三项 NOT RUN 皆因起服务是长驻命令，归 T5）· **口径修正**：§6 A3 写的「42 条」是计数笔误，实为 **37 条**方法·路径（auth 2 + users 3 + tasks 15 + records 4 + comments 6 + annotation-states 2 + attachments 2 + sync 2 + health 1；tasks 含旧端注册但前端未调的 `GET /tasks/returned`、`POST /tasks/batch-reactivate`；`/comments/item/{id}` 一条 route 挂 delete+patch 计 2）**外加**静态 `GET /files/review_attachments/{stored_name}`（Web 服务总入口 ServeDir，归 T1b）；护栏表 37 条 ≡ 脚本 `routes` 模式 38 个探针。§6 表格原文不改，以本条为准 · 提交：待 T1b（`db.rs` / `schema.rs`）先入库后再提这三个文件。
- （待补）T1b 持久层与配置（opus-5-8 接手 opus-5-7 的在途工作，ADR-072）/ T3 PMS 六条（fable-5-1-9）：各任务 `complete_task` 时把「改动文件 · 验收条目 · 偏离及 ADR 编号」交给本节持有人统一回写，不各自改本文。
