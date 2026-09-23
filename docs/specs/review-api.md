# 三维校审域接口规范（`/api/review/*`、`/api/users/*`、`/api/auth/*`、`/files/review_attachments/*`）

- 状态：**现状契约**（2026-09-13 起随移植落地维护；本文只记「线上是什么」，不定「应该是什么」）
- 出处：`docs/plans/2026-09-12-review-backend-port-plan.md`（§1.2 契约表、§1.4 目标侧约束、Q10「新建本文 + `web-service-api.md` §4 末尾一行索引」）
- 契约方：`plant3d-web` `src/api/reviewApi.ts`（路径、方法、请求/响应形状全部由前端写死）；PMS 平台（§8 六条 S2S 入站）；外部模型中心（§9 `aux-data`）
- 源：`plant-model-gen` `src/web_api/{jwt_auth.rs,review_api.rs,review_annotation_state.rs,platform_api/*,review_integration.rs}`（`:3100`）
- 实现：`src/web_service/review/`（`mod.rs` 骨架 + 子模块；挂进主 `Router` 的那一句在 `web_service/mod.rs::serve`，`.merge(review::router())` 位于 `.layer(...)` 之前）
- 相关规格：`docs/specs/web-service-api.md`（gen-model 自己的 `/api/v1` 面；§4 末尾索引到本文）

## 0. 总则：这一族接口为什么不长得像 `/api/v1`

1. **契约路径是前端写死的，不在 `/api/v1` 下。** 先例是 `GET /api/mbd/v2/pipe/{refno}`
   （`web_service/mod.rs`：「路径是 plant3d-web `mbdV2Api.ts` 写死的契约（不在 /api/v1 下）」，决策 d-438）。
   校审域同理：前端只改一个后端地址（`VITE_GEN_MODEL_API_BASE_URL` / `?backendPort=8022`）即可从 `:3100` 切到 `:8022`，
   路径一字不改（计划 G1、N4）。
2. **响应信封沿用旧端，不套 gen-model 的 `ApiError {code,message,detail}`**（`web-service-api.md` §3）。本域有两套信封：
   - 浏览器侧（§1–§7、§10）：`{success: bool, …, error_message?: string}`——前端逐处判 `response.success`；
     业务失败时 HTTP 状态码照常是 4xx/5xx，但正文仍是这套信封（例：401 `{success:false,error_message:"…"}`）。
   - PMS / 外部侧（§1.2 `auth`、§8、§9 `aux-data`）：`{code: number, message: string, data?: …}`，
     `code` 是业务码（200 / 400 / 401 / 404 / -1 …），HTTP 状态码与之同步或恒为 200，逐条见下文。
   这是**有意的例外**：契约以现状为准，不「顺手改好」（计划 N4）。改信封 = 改前端，属另一次计划。
3. **命名与时间格式跟旧端走，不统一。** 任务 / 记录 / 评论 / 批注状态一族是 `camelCase`（serde `rename_all`），
   PMS 六条与 `aux-data` 是 `snake_case`，`CollisionItem` 是 PascalCase 且带历史拼写（§9）。
   时间：`ReviewTask.createdAt/updatedAt/dueDate`、`WorkflowStep.timestamp`、`AnnotationComment.createdAt`、
   任务 history 的 `timestamp` 序列化为**北京时间字符串** `YYYY-MM-DD HH:MM:SS`；`ReviewAttachment.uploadedAt`、
   `ConfirmedRecordWithMeta.confirmedAt`、`AnnotationStateView.updatedAt` 是毫秒时间戳整数。
4. **持久化。** 本域数据落 SurrealDB（宪法 I），连接与介质、内存介质时的响亮拒绝由 `docs/adr/ADR-075-review-data-persistence.md`
   定（计划 Q6、A9：落点 = `SUL_DB` 同一 ns·db；介质不 durable 时四个前缀一律 503 并打多行横幅）；本文不重复。
5. **护栏（宪法 VI）。** 路由清单以源码文本护栏钉住：`review/common.rs::REVIEW_CONTRACT_ROUTES`（§1–§7、§10 共 **37 条**方法·路径；计划 §6 A3 写的「42」是计数笔误，见计划 §11 R3 条）、
   `review/mod.rs` 的 merge 顺序护栏、`review/integration.rs::integration_routes_are_registered_verbatim`（§9 两条）、
   PMS 六条由 `review/platform` 自己立。少一条前端静默 404、多一条契约外扩面，都红。
6. **非目标。** `/ws/review*`（旧端从未实现，前端未配 `VITE_REVIEW_WS_BASE_URL` 时自动关闭，计划 N1 / Q9）、
   `:3100` 上的非校审接口（dashboard / logs / deployment-sites / model-version / model-history / incremental /
   pipeline annotation，计划 N2）、`review-internal-workflow` 内部流转 feature（计划 N6 / Q7）——本文不收。

## 1. 鉴权（`review/auth.rs`）

### 1.1 三套鉴权面

| 面 | 用在哪 | 凭据 | 配置 |
| --- | --- | --- | --- |
| `review_auth`（浏览器侧） | §2–§7、§10 全部路由（各子模块 `routes()` 自己压 `review_auth_middleware`） | `Authorization: Bearer <JWT>`；前端存 `localStorage['review_auth_token']` | `[review_auth] enabled / debug_project_id / debug_user_id / debug_role`；`enabled=false` 时**注入调试身份**放行，不校验 |
| `platform_auth`（PMS S2S） | §8 六条 | 请求体字段 `token`（不是 header） | `[platform_auth] enabled / debug_token`；`enabled=true` 校验 JWT；`enabled=false` 要求与 `debug_token` **精确相等**，`debug_token` 为空则一律拒绝 |
| `aux-data` 口令 | §9 `POST /api/review/aux-data` | 请求头 `UCode` / `UKey` | 环境变量 `AUX_DATA_UCODE`（缺省 `ZY`）/ `AUX_DATA_UKEY`（缺省空串） |

JWT：HS256。密钥与有效期由 `review/auth.rs::JwtConfig::from_config_file` 取：**配置文件能载入时**（`DB_OPTION_FILE` → `db_options/DbOption` → `DbOption`，服务器上恒成立）只读 `[model_center].token_secret` / `token_expiration_hours`（缺省 24 小时），此时环境变量 `JWT_SECRET` **不生效**；找不到任何配置文件才退到 `JwtConfig::default()` 读 `JWT_SECRET`。两路都没给 → 缺省 `default-jwt-secret-key`（启动告警；知道这个串的人不连服务器就能签出任意 `project_id` / `user_id` / `role` 的 token）。
现状：生产 `:8022` 在 2026-09-21 00:31 之前一直没配 `[model_center]`、实际跑在默认串上；此后补成旧 `:3100`（plant-model-gen `[model_center].token_secret`）同一把。注意 PMS **不持有**密钥：它的 S2S `token` 是先调 `POST /api/auth/token` 取的（`plant3d-web/scripts/pms-contract-sequence.ts` 第 0 步就是它），所以密钥只需本机签 / 验一致、跨重启与重部署稳定，值与旧端是否相同并不影响 PMS；换密钥 = 已发 token 全部失效，PMS 重取、前端重登即可。也因此 §8 六条的 JWT 校验挡住的只是「没先调 `/api/auth/token`」——真正的信任边界是**谁能打到 `/api/auth/token`**（它不要密码）。仓库 `DbOption.toml` 末尾有本段的注释模板；`github/deploy/backend-20260918` 分支的 `render_dboption.py` 尚未追加本段、`install_backend.sh` 写进 `service.env` 的 `JWT_SECRET` 按上面的口径不会被读到——重部署前要补（`changelog.md` 2026-09-21）。
claims：`{project_id, user_id, user_name, role?, workflow_mode?, exp, iat}`；**`form_id` 不进 JWT payload**（单据维度只走 URL / query，
`verify_platform_api_json.sh` 第 1b 步钉住）。`workflow_mode` 只在旧端 `review-internal-workflow` feature 构建里出现，本仓不移植：
`/api/auth/token` 收到该字段**只告警忽略**，claims 不携带。

角色：`admin | sj | jd | sh | pz`（编制 / 校对 / 审核 / 批准），节点顺序 `sj → jd → sh → pz`。

### 1.2 端点

| 方法 · 路径 | 鉴权 | 请求 | 响应 |
| --- | --- | --- | --- |
| `POST /api/auth/token` | 无 | `{project \| project_id, username \| user_id, name? \| display_name? \| user_name?, form_id?, role?, workflow_mode?}` | 200 `{code:200, message, data:{token, expires_at(Unix 秒), form_id}}`（`form_id` 不传则服务端生成 UUID）；缺 `project`/`user` 或 `role` 非法 → HTTP 400 `{code:-1, message:"Missing required field: project_id or project" \| "Missing required field: user_id or username" \| "Invalid role: '…'. Valid values are: […]", data:null}` |
| `POST /api/auth/verify` | 无 | `{token, form_id?}`（`form_id` 是已废弃的提示字段，服务端忽略并告警） | 200 `{code, message, data:{valid, claims?, error?}}`；token 无效时 `valid:false, error:"…"` |

### 1.3 401 / 403 文本（计划 A5，逐字对照旧端 `jwt_auth.rs:513,647`）

`review_auth.enabled=true` 时，任何受保护路由：

| 情形 | HTTP | 正文 |
| --- | --- | --- |
| 无 `Authorization` 头，或有头但不是 `Bearer …` 形态 | 401 | `{success:false, error_message:"Missing or invalid Authorization header"}` |
| 有 Bearer 但 JWT 校验失败 | 401 | `{success:false, error_message:"Invalid token: <jsonwebtoken 错误原文>"}` |
| 角色闸门（`require_roles`）不满足 | 403 | `{success:false, error_message:"Access denied. Required roles: [\"…\"], your role: <role>"}` |
| 角色闸门处根本没有 claims | 401 | `{success:false, error_message:"No authentication token found"}` |

## 2. 用户（`review/users.rs`）

全部压 `review_auth_middleware`。用户源是旧端内置的评审人员名单（`build_mock_review_users`），移植不改。

| 方法 · 路径 | 请求 | 响应 |
| --- | --- | --- |
| `GET /api/users` | query `role?`、`status?` | `{success, users:[User], error_message?}` |
| `GET /api/users/me` | — | `{success, user?:User, error_message?}`；`user` 由 claims 推出（`user_id` / `user_name` / `role`） |
| `GET /api/users/reviewers` | — | `{success, users:[User]}`，只含可作校核 / 审核 / 批准的角色 |

`User = {id, username, name, email, role, department?, avatar?}`（snake_case 原样，无 rename）。

## 3. 提资单 / 任务与工作流（`review/tasks.rs`）

全部压 `review_auth_middleware`。对象 `ReviewTask`（`camelCase`）：

```
{ id, formId, title, description, modelName, status, priority,
  requesterId, requesterName, checkerId, checkerName, approverId, approverName,
  reviewerId, reviewerName,            // reviewer* 是 checker* 的兼容别名，两者同时回
  components: [{id, name, refNo, type}],
  attachments: [{id, name, url, size?, mimeType?, uploadedAt?}] | null,
  reviewComment: string|null,
  createdAt, updatedAt: "YYYY-MM-DD HH:MM:SS", dueDate: "YYYY-MM-DD HH:MM:SS"|null,
  currentNode: "sj"|"jd"|"sh"|"pz",
  workflowHistory: [{node, action, operatorId, operatorName, comment?, timestamp:"YYYY-MM-DD HH:MM:SS"}],
  returnReason: string|null }
```

`status` 取值：`draft | submitted | in_review | approved | rejected | cancelled`（旧端口径）；`priority` 缺省 `medium`。
信封：`TaskResponse = {success, task?, error_message?}`、`TaskListResponse = {success, tasks, total, error_message?}`、
`ActionResponse = {success, message?, error_message?}`。

| 方法 · 路径 | 请求 | 响应 / 语义 |
| --- | --- | --- |
| `POST /api/review/tasks` | `CreateTaskRequest`（`camelCase`）：`title, description?, modelName, checkerId?, checkerName?, approverId?, approverName?, reviewerId?(=checkerId 别名), reviewerName?, formId?, priority?, components?, dueDate?, attachments?` | 201/200 `TaskResponse`；`requesterId/Name` 取自 claims；外部驱动模式下 `checkerId/approverId` 可空 |
| `GET /api/review/tasks` | query `status? priority? requester_id? checker_id? approver_id? reviewer_id? form_id?(formId) limit? offset?` | `TaskListResponse`，按 `created_at DESC` 分页 |
| `GET /api/review/tasks/{id}` | — | `TaskResponse`；不存在 / 已软删 → 404 `{success:false,…}` |
| `PATCH /api/review/tasks/{id}` | `UpdateTaskRequest`：`title? description? priority? components? due_date?(毫秒) attachments?` | `TaskResponse`；写授权见下 |
| `DELETE /api/review/tasks/{id}` | — | `ActionResponse`；软删（`deleted=true`），列表 / 详情随即不可见 |
| `POST …/{id}/start-review` | — | `ActionResponse`；`sj/draft → jd/submitted` |
| `POST …/{id}/approve` `…/reject` `…/cancel` | `ReviewActionRequest = {comment?, reason?}` | `ActionResponse` |
| `GET …/{id}/history` | — | `{success, history:[{id, task_id, action, user_id, user_name, comment?, timestamp:"YYYY-MM-DD HH:MM:SS"}], error_message?}`（此处 **snake_case**，旧端如此） |
| `POST …/{id}/submit` | `SubmitToNextRequest`（`camelCase`）：`comment? operatorId? operatorName?` | `ActionResponse`；推进到下一节点 |
| `POST …/{id}/return` | `ReturnRequest`（`camelCase`）：`targetNode("sj"\|"jd"\|"sh"), reason, operatorId?, operatorName?` | `ActionResponse`；驳回到目标节点并写 `returnReason` |
| `GET …/{id}/workflow` | — | `WorkflowHistoryResponse`（`camelCase`）：`{success, currentNode, currentNodeName, history:[WorkflowStep], error_message?}` |
| `GET /api/review/tasks/returned` | — | `{success, tasks:[…精简行], total}`：被驳回待重流转的任务（前端当前未调用，旧端注册着，移植不减项） |
| `POST /api/review/tasks/batch-reactivate` | `{task_ids:[…], target_checker_id?, target_checker_name?}` | `{success, message?, results:[…]}`；`task_ids` 空 → 400 `{success:false, message:"task_ids 不能为空", results:[]}` |

**任务写授权**（`common.rs::authorize_task_write`，旧端同款）：任务配置了任一节点负责人时，`claims.user_id` 须为**当前节点负责人**或**发起人**，
否则 403 `{success:false, error_message:"权限不足：用户 … 既不是任务发起人（…）也不是当前节点「…」负责人（…）…"}`；
requester / checker / approver / reviewer **全部为空**（PMS 外部驱动、owner 由每次 `workflow/sync` 显式声明）时放行；任务不存在 → 404。
`submit` / `return` 的操作者校验同理，错误文本见 `common.rs::format_owner_mismatch_error`（指引改用 §8 `workflow/sync`）。

## 4. 批注确认记录（`review/records.rs`）

全部压 `review_auth_middleware`。以 `(form_id, current_node, operator_id)` 槽位**幂等落盘**确认快照；写入时同步刷新 §5.3 批注状态表。

| 方法 · 路径 | 请求 | 响应 |
| --- | --- | --- |
| `POST /api/review/records` | `ConfirmedRecordData`（`camelCase`）：`taskId, formId?, type, annotations[], cloudAnnotations[], rectAnnotations[], obbAnnotations[], measurements[], dimensionDocument?, dimensionDocumentBaseVersion, note` | `ConfirmedRecordResponse = {success, record?, records?, error_message?}`；`dimensionDocument` 版本 CAS 失败 → 409 `{success:false, error_message:…}` |
| `GET /api/review/records/by-task/{task_id}` | — | `{success, records:[ConfirmedRecordWithMeta]}` |
| `DELETE /api/review/records/item/{record_id}` | — | `{success, error_message?}` |
| `DELETE /api/review/records/clear-task/{task_id}` | — | `{success, error_message?}` |

`ConfirmedRecordWithMeta = {id, taskId, formId, type, annotations, cloudAnnotations, rectAnnotations, obbAnnotations, measurements,
dimensionDocument?, dimensionDocumentVersion, note, confirmedAt(毫秒)}`。

## 5. 评论与批注状态（`review/comments.rs`、`review/annotation_state.rs`）

全部压 `review_auth_middleware`。批注类型 `type ∈ {text, cloud, rect, obb}`。

### 5.1 评论

| 方法 · 路径 | 请求 | 响应 |
| --- | --- | --- |
| `POST /api/review/comments` | `CreateCommentRequest`（`camelCase`）：`annotationId, annotationType, authorId, authorName, authorRole, content, replyToId?, formId?, taskId?, workflowNode?, reviewRound?` | `CommentResponse = {success, comment?, comments?, error_message?}` |
| `GET /api/review/comments/by-annotation/{annotation_id}` | query `type? form_id? task_id?` | `{success, comments:[AnnotationComment]}` |
| `DELETE /api/review/comments/item/{comment_id}` | — | `{success, error_message?}`；软删，仅作者或 admin，否则 403 |
| `PATCH /api/review/comments/item/{comment_id}` | `{content}` | `{success, comment?}`；仅作者或 admin |

`AnnotationComment = {id, annotationId, annotationType, authorId, authorName, authorRole, content, replyToId, createdAt:"YYYY-MM-DD HH:MM:SS"}`。

### 5.2 批注轻量字段与严重度

| 方法 · 路径 | 请求 | 响应 |
| --- | --- | --- |
| `PATCH /api/review/annotations/{annotation_id}` | query `type`；body（`camelCase`）`{title?, description?, formId?, taskId?}` | `{success, title?, description?, updatedAt?(毫秒), error_message?}` |
| `PATCH /api/review/annotations/{annotation_id}/severity` | query `type`；body `{severity: "principle"\|"general"\|"drawing"\|null, formId?, taskId?}`（`null` 清空） | `{success, severity?, updatedAt?, error_message?}` |

### 5.3 批注处理状态（独立真源 `review_annotation_states`）

| 方法 · 路径 | 请求 | 响应 |
| --- | --- | --- |
| `POST /api/review/annotation-states/apply` | `{formId, taskId, annotationId, annotationType, action, note?}`（`camelCase`；`action` 是设计侧「已修改 / 不需解决」或校核侧「同意 / 驳回」） | `{success, state?:AnnotationStateView, error_message?}` |
| `GET /api/review/annotation-states` | query `form_id`(`formId`)、`task_id?`(`taskId`) | `{success, states?:[AnnotationStateView], error_message?}` |

`AnnotationStateView = {formId, taskId, annotationId, annotationType, workflowNode, reviewRound, resolutionStatus, decisionStatus, note?,
updatedById, updatedByName, updatedByRole, updatedAt(毫秒), history:[…]}`。

## 6. 附件（`review/attachments.rs` + 静态挂载）

| 方法 · 路径 | 鉴权 | 请求 | 响应 |
| --- | --- | --- | --- |
| `POST /api/review/attachments` | `review_auth` | `multipart/form-data`：文件字段 + `taskId` / `formId` 等表单字段；扩展名白名单 `pdf dwg dxf xls xlsx csv doc docx png jpg jpeg`；单文件 ≤ 50MB；pdf/png/jpg 做文件头校验 | `{success, attachment?:ReviewAttachment, error_message?}`；超限 413 `{success:false, error_message:"文件超过 50MB 上限…", operation, elapsedMs, formId?, taskId?}`；白名单 / 文件头不符 400 |
| `DELETE /api/review/attachments/{attachment_id}` | `review_auth` | — | `{success, error_message?}`，同时从任务 `attachments` 数组摘除 |
| `GET /files/review_attachments/{stored_name}` | 无 | — | 静态文件（`ServeDir`，目录由 `[review_attachments].dir` 相对 `asset_root` 解析）；URL 前缀**不是配置项**，前端写死 |

`ReviewAttachment = {id, name, url:"/files/review_attachments/<stored_name>", size?, mimeType?, uploadedAt?(毫秒)}`。
上传路由单独放宽 body 上限到 52MB（50MB 文件 + multipart 报文余量），其余路由沿用 axum 默认。
前端另有 JSON+base64 的兜底分支（`reviewApi.ts:1829-1850`），旧端不接受、本仓同样不接受。

## 7. 同步（`review/sync.rs`）

全部压 `review_auth_middleware`。

| 方法 · 路径 | 请求 | 响应 |
| --- | --- | --- |
| `POST /api/review/sync/export` | `{taskIds?, includeAttachments?, includeComments?, includeRecords?}`（`camelCase`） | `{success, tasks:[ReviewTask], comments:[AnnotationComment]\|null, records:[ConfirmedRecordWithMeta]\|null, error_message?}` |
| `POST /api/review/sync/import` | `{tasks:[ReviewTask], overwrite?}` | `{success, importedCount, skippedCount, error_message?}` |

## 8. PMS 平台入站六条（`review/platform/*`，S2S）

鉴权面是 `platform_auth`（§1.1）：凭据是**请求体 `token`**，校验失败 → HTTP 401 `{code:401, message:"invalid token: …" \| "invalid debug_token" \| "platform auth debug_token 未配置"}`。
不经 `review_auth_middleware`。信封一律 `{code, message, …}`，HTTP 状态码与 `code` 同步（200 / 400 / 401 / 404 / 500）。`snake_case`。

| 方法 · 路径 | 请求 | 响应 / 语义 |
| --- | --- | --- |
| `POST /api/review/embed-url` | `{project_id, user_id, workflow_role?(兼容顶层 role), workflow_mode?, form_id?, token?, extra_parameters?}` | `{code, message, data:{relative_path, token, query:{form_id, is_reviewer}, lineage:{form_id, task_id?, current_node?, status?}, form:{form_id, exists, status?, task_created?}, task?:ReviewTask}, url?}`；`url` 带 `user_token=` 与 `form_id=`，**不回流** `project_id` / `user_id` / `output_project`；校审 Web 基址来自 `DbOptionExt` 新配置项（计划 Q5，替代旧端 `managed_project_sites::get_site`）；`form_id` 不传则新建空白单（`form.status="blank"`） |
| `POST /api/review/workflow/sync` | `SyncWorkflowRequest`：`{form_id, token, action:"query"\|"active"\|"agree"\|"return"\|"stop", actor?:{id, name?, roles}, next_step?:{assignee_id, name, roles}, comments?, metadata?:{source?}}`；`actor` 不传时从 JWT claims 推，debug_token 模式必传否则 400；`active` / `agree(非 pz)` / `return` 必带 `next_step` | `{code, message, data?:{models:[refno], task_id?, records:[…], annotation_comments:[…], attachments:[…], form_exists, form_status?, task_created?, current_node?, task_status?, next_step?, annotation_states?}, error_code?, annotation_check?}`；非法 `action` → 400；写 `review_workflow_history` 时 `source = "pms-sync"`（或 `metadata.source`） |
| `POST /api/review/workflow/verify` | 同上（干跑；`next_step` / `comments` / `metadata` 静默忽略） | `{code, message, data?:{passed, action, block_code?, current_node?, task_status?, next_step?, actor_id?, owner_id?, owner_source?, expected_next_node?, requested_next_step?, reason, recommended_action}, error_code?, annotation_check?}`；不落库 |
| `POST /api/review/annotations/check` | `{task_id?(taskId), form_id?(formId), current_node?(currentNode), intent?, included_types?(includedTypes), token?}` | `{success, data?:{passed, recommended_action, current_node, summary:{total, open, pending_review, approved, rejected}, blockers:[{annotation_id, annotation_type, title?, description?, state_code, state_label, refnos, updated_at?, updated_by_name?, updated_by_role?, note?}], message}, error_code?("ANNOTATION_CHECK_FAILED"), error_message?}`（**这一条是 `success` 信封**，旧端如此） |
| `POST /api/review/cache/preload` | `{project_id, initiator, token}` | `{code, message, data?}` |
| `POST /api/review/delete` | `{form_ids:[…], operator_id, token}` | `{code, message, results:[{form_id, success, message}]}`；软删：之后 `workflow/sync query` 回 `form_exists:true, form_status:"deleted", task_created:false`，`models` / `records` / `attachments` 为空 |

端到端判据：`scripts/review/test_workflow_e2e.sh`（embed-url → 建单 → sync active/agree/return/stop → delete，13 段断言）与
`scripts/review/verify_platform_api_json.sh`（夹具 `scripts/review/platform_api_json/*.json`）指向 `:8022`（计划 A8）。

## 9. 模型侧集成（`review/integration.rs`）

旧端 `review_integration.rs`。两条都**不经** `review_auth_middleware`（旧端注册在 JWT 之外）：`aux-data` 用 §1.1 的 `UCode` / `UKey` 口令，`collision-data` 匿名可读。

| 方法 · 路径 | 鉴权 | 请求 | 响应 |
| --- | --- | --- | --- |
| `POST /api/review/aux-data` | 头 `UCode` / `UKey`；缺头或不匹配 → **HTTP 401 空体** | `{project_id, model_refnos:[…], major, requester_id, page:int, page_size:int, form_id, new_search?:bool}`（`page`/`page_size` 收整数；字符串 → 422，旧端如此） | HTTP 200 `{code:200, message:"ok", page, page_size, total, data:{collision:[CollisionItem], quality:[], otverification:[], rules:[]}, reason?}`；`page` / `page_size` 原样回显 |
| `GET /api/review/collision-data` | 无 | query `project_id? refno? limit?(缺省 100) offset?(缺省 0)`；数值类型不合 → 400 | HTTP 200 `{success:true, data:[CollisionItem], total, error_message?, reason?}` |

```
CollisionItem = { ObjectOneLoc, ObjectOne, ObjectTowLoc, ObjectTow,   // "Tow" 是旧端拼写，前端按此读，不改
                  ErrorMsg, ObjectOneMajor, ObjectTwoMajor, CheckUsr, CheckDate, UpUsr, UpTime, ErrorStatus }
```

**碰撞数据源（计划 Q4，验收 A10）。** 旧端从 `aios_core::project_primary_db()` 直读 `collision_events` 表，查空 / 查错时**静默回落到两条 mock 碰撞**
——旧端线上这两条接口实际交付的一直是 mock（`collision_events` 在旧端也没有生产者，`collision_api.rs` 的 `/api/collision/detect` 是
`feature is not available` 空壳）。gen-model 的模型数据面是 e3d-direct / RocksDB（`QueryService` / direct store），没有 `collision_events`
同形表，也不造假的数据面。因此现状是：

- 两端点在**没有碰撞数据源时返回显式空集 + `reason`**：`collision:[]` / `data:[]`、`total:0`、
  `reason:"collision_source_unavailable: …"`（以稳定 token `collision_source_unavailable` 开头，常量
  `integration::COLLISION_SOURCE_UNAVAILABLE_REASON`）。**不返回 mock。** 前端对空数组显示「无碰撞数据」，`reason` 供 curl / 诊断读。
- `reason` 是本仓相对旧端**唯一新增**的键；有数据源时不序列化，键集与旧端逐字相同（`integration.rs::legacy_shapes_are_preserved_when_a_source_exists`）。
- 数据源接缝是 `integration::CollisionSource`（今天只有 `Unavailable`）。接入时（候选：PMS `/HD/QueryAssistReview` 代理——前端注释
  「后端代理 → PMS」即此方向；或 gen-model 自带碰撞结果落 SurrealDB `collision_events`）只加一个变体并把 `AppState.queries` 穿进
  `collision_source()`，两条 handler 形状不动。「碰撞数据源待接」记在 `docs/adr/ADR-075-review-data-persistence.md` §未决。

## 10. 健康（`review/mod.rs`）

| 方法 · 路径 | 鉴权 | 响应 |
| --- | --- | --- |
| `GET /api/review/health` | 无（注册在所有子模块 `.merge()` 之后、JWT 之外，供部署探针 / PMS 预检） | 200 `{status:"ok", database:"healthy", elapsedMs}`；504 `{status:"error", database:"unreachable"\|"unhealthy", operation:"review.health.connect"\|"review.health.probe", elapsedMs, message}`（探针 5s 超时） |

与 `GET /api/v1/health` 的分工：那条回答站点存活，这条只回答「校审数据当前是否可查询」；校审探针不往 `/api/v1/health` 的断连账本里记。

## 11. 验收映射

| 计划 §6 | 本文 | 怎么判 |
| --- | --- | --- |
| A2 | §10 | 两端 `curl /api/review/health` 键集相同 |
| A3 | §1–§7、§10 | `cargo test review_contract_routes_are_registered_verbatim`（37 条 + 静态 1 条）+ `scripts/review/test_review_api.sh <base> routes`（38 个探针） |
| A5 | §1.3 | curl 对比 401 / 403 文本 |
| A8 | §8 | `scripts/review/test_workflow_e2e.sh`、`scripts/review/verify_platform_api_json.sh` 指向 `:8022` |
| A10 | §9 | 两端 curl 对比键集；无数据源时 `reason` 在场、`total:0`、无 mock；`cargo test web_service::review::integration` |
| A11 | 本文 | `web-service-api.md` §4 末尾索引到本文 |

## 12. 变更记录

- 2026-09-13 · 初版（T4 / T5 文档半边）。§1–§7、§10 按 T2 已落地代码；§8 按旧端 `platform_api/types.rs` 与 T3 在途实现，T3 交付后如有出入以代码为准并回写本节；§9 为 T4 落地形状（`reason` 新增，Q4）。

### 2026-09-14 闭环验收补充

- 角色 `jd` 与账号 ID 分离；夹具校核账号为 `JH`，任务 checkerId/reviewerId 必须与 JWT user_id 一致。未恢复任务、当前节点不符或缺少负责人时，走查直接失败，保留 403 正文并脱敏。
- `embed-url` 的 `data.url` 与 `data.query` 配合使用；按前端 `buildTokenPrimaryEmbedUrl` 将 `query.form_id` 加入 URL，JWT 不包含 form_id。
- `annotations/check` 使用 `success/data` 信封；未闭环批注使 `workflow/verify` 返回 HTTP 200 且 `data.passed=false`，不是 HTTP 409。
- A3 清单共 46 条：37 核心方法·路径、1 静态附件挂载、6 PMS、2 集成。资源 404 必须检查业务错误体，不能当作路由未注册，也不能仅凭非 404 宣称业务通过。
- 完整重启验收由 `scripts/review/persistence.mjs seed|verify` 比较 7 组非空快照，包含附件文件 SHA256；任务元数据复读不等同于完整持久化通过。
