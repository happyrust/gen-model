# 2026-09-28 三维校审流程缺陷修复计划：编校审单二次保存丢改动 / `project_id` 错位 / verify ≠ sync 干跑

> 状态：**已批准**（2026-09-28 Plannotator `--gate` `{"decision":"approved"}`，无批注；Q1–Q5 全按推荐项定稿，见 §9）。作者：fable-5-1-3（本轮审核人）。**T1–T4 已落地并提交主线 `ci/linux-build-refactor-20260928`**（2026-09-29，见 §9；Q9=A）；**T5 已提交 `plant3d-web` `main@76e142f0`，e2e 待实跑**；第 2 段审核追加 D7–D9 / D5b 与 Q6–Q8，**等拍板**。
> 审核基线：`gen-model` 分支 `refactor@858c94fb0`（工作树 `src/web_service/review/*` 的 14 个 `M` 经 `git diff -w` 核对只有 rustfmt 重排 + CRLF→LF，无语义改动）；契约方 `plant3d-web` `main@7ca2c6e6`。
> 上游：`docs/plans/2026-09-12-review-backend-port-plan.md`（移植口径「逐字一致」）、`docs/plans/2026-09-14-review-closure-landing-plan.md`（已落主线）、`docs/adr/ADR-075-review-data-persistence.md`、`docs/specs/review-api.md`。
> 约束：宪法 III（静默失效是最高级别缺陷）、宪法 VI（不变量由可执行守护看住）、d-536（提交信息只引 ADR / plan 路径）、工作树多会话共用（只 `git add <明确路径>`）、禁止 `cargo clean`。
> 本文是 PRD 形态：定「修什么、修到什么算好、先修哪个」；具体写法由执行工程在各条任务里定，改口径处回写 §9 并按需另开 ADR。

## 0. 一句话

移植把旧端 `plant-model-gen` 校审域的**行为**一字未动地搬进了 `gen-model`，连带搬进了旧端的几处逻辑缺陷，
其中两处会让设计人员的改动**静默丢失**、让 `review_forms.project_id` 装进「数据包名称」。本计划把这六处按严重级修掉，
并把「PMS 侧 external 模式下平台不做 owner 校验」「verify 就是 sync 的干跑」两条口径写进契约文档，让测试不再把缺陷反向钉成预期。

## 1. 缺陷清单（2026-09-28 全部核过源码，出处在括号里）

| # | 级别 | 现象 | 出处 |
| --- | --- | --- | --- |
| D1 | **P1** | 设计端对同一 `form_id` **二次保存**（改构件 / 改描述 / 改包名）→ 后端返回 `success:true` 与**旧任务**，新字段全部丢弃；前端提示「编校审单保存成功」并清空表单 | 后端 `src/web_service/review/tasks.rs:505-629`（`create_task` 的 form_id 去重分支：只写一条 `action='resubmit', target_node='jd'` 历史，不 UPDATE 任何字段、不刷 `review_form_model`）；前端 `plant3d-web/src/components/review/InitiateReviewPanel.vue:812-823`（external 模式**永远**走 `createReviewTask({formId})`）、`src/composables/useUserStore.ts:994`（只调 `reviewTaskCreate`，`reviewTaskUpdate` 仅附件补传用）、`src/components/review/embedContextRestore.ts:113-132`（落点时用旧任务 `restoredTaskDraft` 预填表单——所以「改一改再保存」是被引导出来的正常操作） |
| D1b | P2 | 去重分支写 `resubmit→jd` 历史不看当前节点：任务在 `jd/sh/pz/approved` 时再点保存也记一条假「再次发起」 | `tasks.rs:553-586` |
| D2 | **P1** | `review_forms.project_id` 被前端 `modelName`（= 用户填的**数据包名称**）覆盖；embed-url 建存根时写入的真 PMS `project_id` 随即丢失 | 前端 `InitiateReviewPanel.vue:815`（`modelName: formData.packageName`）；后端 `tasks.rs:741-748`（`sync_review_form_with_task_status(form_id, Some(model_name) /* 当 project_id */, …)`）、`tasks.rs:1992` / `:2228`（内部 `submit` / `return` 亦 `project_id: task_row.model_name`）；`platform/review_form.rs:203-227`（UPDATE 只在传值非空时覆盖，所以空串即「保留」） |
| D3 | P2 | `POST /api/review/workflow/verify action=active` 对「主单据存在但未建 task」**和**「主单据不存在」一律 `passed:true`（后者因 404 分支也填了 `task_status=blank`）；同一状态下 `workflow/sync action=active` 却 409 / 404。verify 不再是 sync 的干跑，PMS PreValidate 通过后送审失败、单据停在原节点看不到原因 | `platform/workflow_sync.rs:657-697`（放行分支）、`:1450-1459`（404 也填 `blank`）；`platform/tests.rs:743-790`、`:829-860` 把这两半钉成了预期，文件头 §表 承认旧测试原本断言 404 |
| D4 | P2 | external 模式（生产默认：claims 无 `workflow_mode`）下 `ensure_owner_matches` 整体短路——任何持有效 S2S token 的调用方、不论 actor 是谁都能推进任意 form；verify 通过响应里 `owner_id / owner_source` 恒空。设计上信任 PMS 可以接受，但**未成文**，`docs/specs/review-api.md` §8 没有这句 | `platform/workflow_sync.rs:1571-1573`、`:903-927`（`build_verify_pass_data` 不填 owner） |
| D5 | P3 | 去重查「最老」（`ORDER BY created_at ASC`），流转查「最新」（`find_task_by_form_id` `ORDER BY updated_at DESC`）；`form_id` 重复已被承认会发生（`query_workflow_data` 专门做了 `dominant_records_task_id` 对齐并 warn），两路选中不同任务时，设计端保存与 PMS 流转作用在两条记录上 | `tasks.rs:529` vs `platform/review_form.rs:487-495`；`platform/workflow_sync.rs:2445-2465` |
| D6 | P3 | `PATCH /api/review/tasks/{id}` 改 `components` 不同步 `review_form_model`；`workflow/sync` 回给 PMS 的 `data.models` 读的正是这张表，二者漂移 | `tasks.rs:1013-1167`；`platform/workflow_sync.rs:2201-2222`；全仓只有 `create_task`（`:715-737`）与 `sync.rs:360-375` 写这张表 |
| D7 | P2 | **OBB（箱体）批注不进批注闸门**：前端把 obb 当正式批注类型（`useToolStore.ts:812` `ANNOTATION_TYPE_TO_V7_FIELD` 含 `obb`），确认记录写 `obb_annotations`，处理状态接口也认 `obb`（`comments.rs:565` `is_valid_annotation_type`、`:684` 按类型 patch 记录）；但闸门 `SUPPORTED_ANNOTATION_TYPES = ["text","cloud","rect"]`，`AnnotationCheckRecordRow` 不选 `obb_annotations`，`included_types` 传 `obb` 直接 400。后果：审核人只对某构件画了 OBB 批注并标 open / rejected 时，`agree` 照样放行、`return` 反被拒（「无未处理或被驳回的批注」）——闸门在这一类批注上整体失效 | `platform/annotation_check.rs:29`、`:142-148`、`:452-458`、`:776-780`；对照 `platform/review_form.rs:401-407`、`workflow_sync.rs:2471`（删除 / 汇总两条路都认 obb） |
| D8 | **P1** | **S2S 与嵌入 token 同源同验签、`actor` 由请求体自报、external 不校 owner**：`verify_s2s_token_with_claims` 就是 `verify_token`（同一密钥、同一 claims），embed-url 签给浏览器的 token 天然是合法 S2S token；`fill_actor_from_claims` 在请求体带 `actor` 时**不与 claims 的 `user_id / role` 比对**；加上 D4 的 owner 短路。后果：任一拿到嵌入链接的用户，在浏览器里对**任意** `form_id` 发 `workflow/sync {action:"agree"/"return"/"active", actor:{id:"JH",roles:"jd"}, next_step:…}` 就能冒任意 actor 推进 / 驳回 / 批准单据，历史记成 JH。前端自己也在用这条路（`useReviewStore.applyExternalWorkflowChange` 收到 `pms.workflow_changed` 后用 `embedParams.userToken` 调 `reviewWorkflowSyncMutation`），说明「只有 PMS 后端持 S2S token」的前提不成立。D4 只成文不够 | `platform/auth.rs:18-48`、`platform/workflow_sync.rs:508-519`、`:1571-1573`；`plant3d-web/src/composables/useReviewStore.ts:960-1009`、`:855-882`（`resolveExternalWorkflowVerifyContext` 直接拿 `embedParams.userToken` 当 S2S token） |
| D9 | P3 | 嵌入页 postMessage 桥不校验来源：`attachEmbedPostMessageBridge` 支持 `trustedOrigins`，但 `DockLayout` 挂桥时没传，`pms.workflow_pre_action` / `pms.workflow_changed` 来自任何窗口都受理；`pre_action` 会**自动落库**未保存的确认记录（`flushPendingConfirmForExternalAction` → `POST /api/review/records`）再调 verify，`workflow_changed` 直接调 `workflow/sync` 变更。有 `form_id_mismatch` 兜底（只对当前打开的单据生效），但同页任意 iframe 都能替用户「确认当前数据」并推进当前单据 | `plant3d-web/src/components/DockLayout.vue:377-389`、`src/components/review/embedPostMessageBridge.ts:22-30`、`src/composables/useReviewStore.ts:715-724`、`:884-898` |
| D5b | P3 | 闸门读记录按 `task_id`（`load_effective_annotations` `WHERE task_id = $task_id`），而 `workflow/sync query` 读记录先按 `form_id`、再按 task 回退，并承认 form 下会有重复 task（`dominant_records_task_id`）。重复出现时闸门评估的是 active task 名下的记录（可能为空 → 「无批注，直接放行」），PMS 看到的 `records` 却挂在另一条 task 上——闸门被绕过。T1 之后 sj 节点不再新建重复 task，存量重复数据仍会撞到 | `platform/annotation_check.rs:731-751`；`workflow_sync.rs:2435-2465` |

### 1.1 已核对无问题（不进本计划）

- 事务里 `current_node / status` 比对：写侧统一小写常量，读侧 `normalize_*` 小写，不会因大小写误判 `WORKFLOW_STATE_CHANGED`。
- S2S 鉴权：`platform_auth.enabled=false` 仍要求 `debug_token` 精确匹配、空则拒绝，无「关掉即放行」档（`platform/auth.rs`）。
- 附件目录 / URL 单一出处（ADR-075 D6）、拒绝路由 503 不吞 `/api/v1`（`db.rs` 两条测试）。
- **第 2 段（2026-09-29 审，reviewer 嵌入链路）**：`DockLayout.bootstrapEmbedSession` 以 `authVerifyToken` 返回的 claims 覆盖 URL 里的 `user_id / project_id / workflow_role`（URL 不再是可信身份源）；token 校验失败或缺可信身份时清空落点持久化、只留 viewer；`restoreEmbedFormSnapshotContext` 用 `workflow/sync query` 的 `models / records / annotation_comments / attachments` 回放场景，`task.components` 以 `review_form_model` 为准（设计端草稿 `draftComponents` 仍取自 `review_tasks.components`——两处来源，D6 修后一致）；`ReviewPanel` 确认记录只写「未保存的增量」（`buildUnsavedReviewConfirmPayload`），闸门按 annotation id 合并多条记录、`review_annotation_states` 独立真源覆盖记录内嵌 `reviewState`，两边分类口径一致（`classify_annotation_state` ≡ `classify_from_independent_state`）；闸门四节点口径：`active` 仅 sj、`agree` / `return` 仅 jd/sh/pz，与 PMS 三维编校审流程（编制 → 校对 → 审核 → 批准）一致；`applyExternalWorkflowChange` 有 `form_id_mismatch` 兜底、`agree(非 pz)` / `active` / `return` 缺 `next_step` 直接拒。

## 2. 目标

- G1 设计人员在 `sj` 节点对同一 `form_id` 的每一次保存都落库并可从 `GET /api/review/tasks/{id}`、`workflow/sync action=query` 两条路读回同一份构件 / 描述 / 附件；不在 `sj` 节点时保存被**响亮拒绝**（HTTP 409 + 可读 `error_message`），前端原文展示。
- G2 `review_forms.project_id` 自 embed-url 建存根起不再被任何校审接口改成别的东西；`model_name` 只是 `review_tasks` 的展示字段。
- G3 `workflow/verify` 对任一 `(form_id, action, actor)` 的 `passed` 与随后 `workflow/sync` 同参数的 2xx/非 2xx **一致**（差别只在 verify 不写库）。
- G4 契约文档写明：external 模式的信任边界 = S2S token；verify ≡ sync 干跑；二次保存语义。测试与文档同向。
- G5 `review_form_model` 与 `review_tasks.components` 在 create / dedup-resave / PATCH 三条写路径后一致。

## 3. 非目标

- N1 不改任何接口路径、方法与响应**键集**（前端零契约改动；只新增一种 409 错误文本）。
- N2 不移植 `review-internal-workflow`（Q7 已定）、不改 internal/manual 模式的 owner 校验。
- N3 不做旧端 `:3100` 数据迁移；D2 历史数据是否回填见 Q3。
- N4 不动 `annotation_check` 闸门口径（本轮未发现问题，另审）。
- N5 不碰 `refactor` 工作树里其他会话在途的 28+ 个未提交文件。

## 4. 任务拆分（文件归属互不重叠；T1/T2 可并行，T3 等 Q1 拍板，T5 等 T1 定稿错误文本）

### T1 · 二次保存改成「更新」，去重与查找同源（D1 / D1b / D5 / D6）
- 文件：`src/web_service/review/tasks.rs`（`create_task` 去重分支、`update_task`、新增私有 `replace_form_models(form_id, refnos)`）、`src/web_service/review/platform/review_form.rs`（`find_task_by_form_id` 若需暴露「不水合附件」的轻量版）。
- objective：
  1. 去重查询改为复用 `find_task_by_form_id` 的排序口径（最新 `updated_at`），不再自带一条 `created_at ASC`。
  2. 命中活动任务且 `current_node == "sj"`：`UPDATE` 设计端可编辑字段（`title / description / model_name / components / attachments / priority / due_date`；**不碰** `checker_* / approver_* / reviewer_*`——它们由 PMS `next_step` 指定）、重刷 `review_form_model`（删旧插新，按 refno 去重）、写一条历史。历史 `action` 见 Q4。返回 UPDATE 后的任务。
  3. 命中活动任务但 `current_node != "sj"`：**409** `{success:false, task:null, error_message:"单据已送审（当前节点 X · 中文名），不可修改；如需修改请由 PMS 驳回到编制节点"}`，不写库。
  4. `update_task` 带 `components` 时同样调用 `replace_form_models`。
- deliverable：以上 + `mem://` 实跑单测：(a) 二次保存改 components/描述 → `GET tasks/{id}` 与 `workflow/sync query` 的 `models` 都是新值；(b) 任务在 `jd` 时二次保存 → 409、任务与 history 不变；(c) PATCH components 后 `review_form_model` 与 `components` 集合相等；(d) 去重与 `find_task_by_form_id` 对同一组重复行选中同一 id。
- acceptance：§5 V1–V3。

### T2 · `project_id` 只来自可信来源（D2）
- 文件：`src/web_service/review/tasks.rs`（三处传参）、`src/web_service/review/platform/tests.rs` 或 `tasks.rs` 单测。
- objective：`create_task` 传 `claims.project_id`（空则传空串，`review_form.rs` 的 `IF len>0` 语义即「保留」）；内部 `submit_to_next_node` / `return_to_node` 的 `project_id: task_row.model_name` 改为空串（保留 `review_forms` 现值）。`model_name` 不再流向 `review_forms`。
- deliverable：护栏测试——embed-url 建存根 `project_id=P` → `create_task(modelName="某包名")` → `review_forms.project_id` 仍为 `P`；同一测试再走内部 `submit` 一遍仍为 `P`。
- acceptance：§5 V4。历史数据处置见 Q3。

### T3 · verify 与 sync 同判（D3）
- 文件：`src/web_service/review/platform/workflow_sync.rs`（`verify_workflow_handler` 放行分支、`load_task_for_workflow` 404 分支的 `task_status`）、`src/web_service/review/platform/tests.rs`（两条断言反向）、`docs/specs/review-api.md` §8。
- objective（按 Q1 A 案）：verify `active` 对「主单据存在、未建 task」回 `200 passed:false, block_code:"TASK_NOT_CREATED", task_status:"blank", recommended_action:"block", reason:"表单尚未保存编校审数据，请设计人员先在三维页保存"`；对「主单据不存在」与其他 action 一致回 404。`load_task_for_workflow` 的 404 分支不再填 `blank`。
- deliverable：两条测试改成新口径 + 新增一条「verify 与 sync 对 blank form 的 active 判定一致」的对拍测试（同一夹具跑两条 handler，断言 `verify.passed == sync.status.is_success()`）。
- acceptance：§5 V5。

### T4 · 契约成文 + 诊断补位（D4）
- 文件：`docs/specs/review-api.md` §8（新增「信任边界与 owner 校验」「verify ≡ sync 干跑」「二次保存语义」三小节）、`src/web_service/review/platform/workflow_sync.rs::build_verify_pass_data`（external 下也回填 `owner_id / owner_source`，仅诊断、不改判定）。
- deliverable：文档 + 一条单测（external verify 通过响应带 owner 诊断）。
- acceptance：§5 V6。

### T5 · 前端配合（plant3d-web，零契约改动）
- 文件：`src/components/review/InitiateReviewPanel.vue`、`src/composables/useUserStore.ts`、`docs/verification/pms-3d-review-integration-e2e.md`。
- objective：
  1. external 且 `restoredTaskDraft.taskId` 存在时，面板标题/提示改为「正在修改已保存的编校审单（task …）」，保存按钮文案「保存修改」。
  2. `createReviewTask` 收到与本地列表同 id 的任务时按 id 替换而非 `push`（现在会在 `reviewTasks` 里堆重复项）。
  3. 后端 409 的 `error_message` 原文进 `notification.details`（现有 catch 已能显示，只需确认不被「保存失败」笼统文案盖掉）。
  4. e2e TC-1 加「保存后重开同一单据、改构件、再保存、JH 打开看到新构件」一步。
- acceptance：§5 V7。

### T6 · obb 批注进闸门（D7；拟增，等 Q7）
- 文件：`src/web_service/review/platform/annotation_check.rs`（`SUPPORTED_ANNOTATION_TYPES` 加 `obb`、`AnnotationCheckRecordRow` / `load_effective_annotations` 选并遍历 `obb_annotations`）、`platform/tests.rs`（一条：只有 obb open 批注时 `agree` 被挡、`return` 放行）。
- acceptance：V9 · 单测 + `pms-3d-review-integration-e2e.md` 补一步「JH 只画 OBB 批注 → 同意被挡」。

### T7 · 信任边界收口（D8 / D9 / D5b；拟增，等 Q6 / Q8）
- 后端：`platform/workflow_sync.rs`（`fill_actor_from_claims` 绑 claims；external 模式 `agree / return` 复用 `ensure_owner_matches`）、`platform/annotation_check.rs`（`load_effective_annotations` 与 `query_workflow_data` 同一取记录口径：先 `form_id` 再 `task_id`）、`docs/specs/review-api.md` §8。
- 前端：`DockLayout.vue` 挂桥传 `trustedOrigins`（来源见 Q8）。
- acceptance：V10 · 持 SJ 嵌入 token 冒 `actor JH` 调 `sync agree` → 403；同 form 两条 task、记录挂旧 task 时 `verify agree` 仍看到 open 批注。

## 5. 验收（每条第三方可独立判真假）

| # | 判据 | 怎么判 |
| --- | --- | --- |
| V1 | 同 `form_id` 在 `sj` 节点二次 `POST /api/review/tasks` 改 `components` → 200，`GET /api/review/tasks/{id}` 与 `workflow/sync action=query` 的 `models` 均为新集合 | `cargo test --lib web_service::review` 新测试 + 走查 U1 重开改构件 |
| V2 | 任务在 `jd` 时二次保存 → 409，`error_message` 含当前节点中文名；`review_tasks` / `review_workflow_history` 行数与内容不变 | 单测 |
| V3 | 构造两条同 `form_id` 活动任务，去重与 `find_task_by_form_id` 选中同一 id | 单测 |
| V4 | embed-url 建存根后 `create_task` + 内部 `submit`，`review_forms.project_id` 全程等于 embed-url 传入值 | 单测 |
| V5 | 对 blank form：verify active `passed:false` 且 sync active 非 2xx；对不存在 form：两者皆 404 | 单测（对拍） |
| V6 | `docs/specs/review-api.md` §8 有三小节；external verify 通过响应含 `owner_id` | 文件 + 单测 |
| V7 | 前端 `pnpm test`（`InitiateReviewPanel*.test.ts`、`useUserStore.createReviewTask.test.ts`）绿；e2e 新步骤 JH 端看到新构件 | 测试输出 + 截图 |
| V8 | 全库 `cargo test --lib --features http_api` 不比基线（2035 passed）少；校审域 ≥ 152 | 计数 |

## 6. 风险

| 风险 | 影响 | 缓解 |
| --- | --- | --- |
| D1 改成 UPDATE 后，PMS 侧若依赖「每次保存都新建 task」的旧行为（无证据，`review_form_model`/records 都按 `form_id` 挂） | PMS 端显示异常 | T1 只在 `sj` 节点更新；e2e TC-1 全程回归 |
| D3 改判后，若 PMS 真的在设计人员保存**之前**就调 verify active | PMS 送审入口被挡 | Q1 拍板前先在 `pms-3d-review-integration-e2e.md` 的实跑日志核时序；保留 B 案 |
| 409 新文本被前端笼统文案盖住 | 用户仍看不到原因 | T5 第 3 条 |
| `refactor` 工作树多会话在途 | 误把别人改动卷进提交 | 只 `git add` 本计划列出的文件；提交前 `git diff --cached --stat` 对照 |

## 7. 开放问题（拍板后按批注定稿，写进 §9）

- **Q1 D3 verify 对 blank form 的 active**：A 改为 `passed:false, block_code=TASK_NOT_CREATED`、404 与其他 action 一致（**推荐**）；B 保持放行，但 spec §8 写明「active 放行不代表 sync 会成功」。
- **Q2 D1 非 `sj` 节点的二次保存**：A 409 拒绝（**推荐**，宪法 III）；B 允许只改 `description / attachments`、忽略 `components` 并在响应加 `warning`。
- **Q3 D2 历史数据**：A 只修代码不回填（**推荐**：`review_forms.project_id` 目前无消费方，embed-url 每次打开会用 PMS 传入值再 `UPDATE` 一遍，自然收敛）；B 出一条一次性 SurrealQL 回填脚本进 `scripts/review/`。
- **Q4 D1 二次保存的历史 `action`**：A 新增 `resave`（**推荐**：`resubmit` 语义是「退回后再次发起」，现有 `plant3d-web` 的 `WorkflowHistory` 组件对未知 action 显示原文，不会崩）；B 沿用 `resubmit` 但仅当任务 `return_reason` 非空时写。
- **Q5 T5 前端是否随本计划一起做**：A 一起做，同一轮 e2e 验收（**推荐**）；B 后端先落、前端另开计划。
- **Q6 D8 信任边界怎么收（第 2 段新增，未拍板）**：A `workflow/sync` / `verify` 在 token 带 claims 时强制 `actor.id == claims.user_id`（PMS 是按人经 `/api/auth/token` 取 token 再送审的，不受影响；浏览器持自己的 token 就冒不了别人）**并** external 模式对 `agree / return` 复用 `ensure_owner_matches`（owner 就是上一步 `next_step` 写进 `checker_id / approver_id` 的人，平台侧有据可查）（**推荐**）；B 只做前半（actor 绑 claims），owner 仍全信 PMS；C 给 PMS 单独一套 S2S 凭证（`aud:"pms"` claim 或独立密钥），嵌入 token 一律不进 §8 六条——改动最大，PMS 侧要配合换 token 获取方式。
- **Q7 D7 obb 进闸门**：A 直接把 `obb` 加进 `SUPPORTED_ANNOTATION_TYPES` 并选 `obb_annotations` 列（**推荐**，一处常量 + 一处行结构 + 一条测试）；B 保持排除但前端禁止对 obb 设 reviewState——与现有处理状态接口冲突，不建议。
- **Q8 D9 postMessage 来源**：A embed-url 响应里带 PMS 页面 origin、`DockLayout` 传给 `trustedOrigins`，缺省只信 `document.referrer` 的 origin（**推荐**）；B 不改，写进契约「嵌入页只能挂在 PMS 自己的页面里」。
- **Q9 T1–T4 落在哪条线（见 §9 2026-09-29 条）——已定 A（2026-09-29，用户拍板）**：审核基线 `858c94fb0` 是从 `gen-model-refactor` 分叉出去的 detached 线，真正的 refactor 主线（`gen-model-refactor` / `ci/linux-build-refactor-20260928`）已多 236+ 提交且校审域也有改动（`c8d5f6b92` active 走批注闸门、`d6a30743c` 事务写冲突重试等）。A 以 `ci/linux-build-refactor-20260928` 为落点提交（T1/T2 已在隔离 worktree `D:\work\plant-code\old\gen-model-review-t1t2` 上打好补丁并跑绿）（**推荐**）；B 落在 `858c94fb0` 这条线，再由主线合并。

## 8. 顺序与协调

1. 本文经 Plannotator `--gate` 批准 → Q1–Q5 按批注定稿 → 回写 §9。
2. T1 与 T2 并行（都在 `tasks.rs` 但改动区块不重叠：T1 在 `:505-629` / `:1013-1167`，T2 只改三处传参；同一执行人最省事）。
3. T3 在 Q1 定稿后开工；T4 与 T3 同人（都碰 `workflow_sync.rs` 与 spec §8）。
4. T5 在 T1 的 409 文本定稿后开工。
5. 每条任务交付附：改动文件清单、`cargo test --lib web_service::review` 计数、对应 V 条输出。提交信息按 d-536 引本文路径。

## 9. 落地记录

（随各任务交付追加：日期 · 任务 · 改动文件 · 验收条目 · 与本文口径的偏离及 ADR 编号）

- 2026-09-28 · 审核 + 计划（fable-5-1-3）· 审核基线 `refactor@858c94fb0` × `plant3d-web main@7ca2c6e6`；本文经 Plannotator `--gate` 批准（`{"decision":"approved"}`，无批注）。开放问题定稿：**Q1=A**（verify 对 blank form 的 active 改判 `passed:false, block_code=TASK_NOT_CREATED`；form 不存在一律 404）、**Q2=A**（非 `sj` 节点二次保存 409）、**Q3=A**（`project_id` 只修代码不回填历史）、**Q4=A**（二次保存历史 `action='resave'`）、**Q5=A**（T5 前端随本计划一起做、同轮 e2e 验收）。T1–T5 尚未派活。
- 2026-09-29 · **T1 + T2 落地**（fable-5-1-6）· 改动文件：`src/web_service/review/tasks.rs`（`create_task` 去重分支改走 `find_active_task_row_by_form_id` + 新 `resave_existing_task`：sj 节点一笔事务 UPDATE `title / description / model_name / priority / components`（`attachments / due_date` 只在请求带了时覆盖）+ `REPLACE_FORM_MODELS_SQL` 重刷 `review_form_model` + `action='resave'` 历史，`WHERE current_node='sj'` 乐观保护、`REVIEW_RESAVE_STATE_CHANGED` → 409；非 sj → 409 原文「单据已送审（当前节点 X · 中文名），不可修改；如需修改请由 PMS 驳回到编制节点」；`update_task` 带 `components` 时调 `replace_form_models`，失败进响应 `error_message`；`create_task` 的 `sync_review_form_with_task_status` 改传 `claims.project_id`；`submit_to_next_node` / `return_to_node` 的 `project_id` 传空串）、`src/web_service/review/platform/review_form.rs`（抽 `find_active_task_row_by_form_id`，`find_task_by_form_id` 复用——去重与流转同一排序口径）、`src/web_service/review/platform/tests.rs`（新增 5 条 `mem://` 实跑：V1 二次保存改字段 + `GET tasks/{id}` / `workflow/sync query` 两路一致 + 历史恰好一条 `resave` + 不带 attachments 保留旧附件、V2 jd 节点二次保存 409 且 tasks / history / form_model 零写入、G5 PATCH components 刷 `review_form_model`、V3 两条重复 task 去重与 `find_task_by_form_id` 同源、V4 `review_forms.project_id` 经首建（claims 无项目号 → 保留）/ 二次保存 / 内部 submit 全程不变）。**验证**：本工作树（`858c94fb0`）当前编不过——不是本改动，`src/data_interface/window_root_plan.rs` 的 `change_is_metadata_only -> bool` 对不上 `vendor/e3d-model` 09-16 起的 `plan_update_with(… -> Option<NoModelTier>)`（其他会话在途）；改在隔离 worktree `D:\work\plant-code\old\gen-model-review-t1t2`（`ci/linux-build-refactor-20260928@d89a5777d` + 同一份补丁，`git diff -w` 与本树只差主线自己的 `surreal_retry` 包装）上跑：`cargo test --lib --features http_api web_service::review` → **163 passed / 0 failed**（含 `every_static_surrealql_literal_in_review_handlers_parses_under_the_bundled_engine`，新 SQL 的 `FOR … IN` 在 2.1.4 fork 下解析通过）；全库 `cargo test --lib --features http_api` → **2443 passed / 0 failed / 128 ignored**（V8 基线 2035 ✓）。**与本文口径的偏离**：① `attachments / due_date` 在二次保存请求未携带时**保留**而不是清空（前端附件另有 `/api/review/attachments` 维护链，`undefined` 不等于「用户删光了」）；② `update_task` 刷 `review_form_model` 失败不回滚任务行、改为响亮进 `error_message`（与首建回填同口径；要严格原子需把 PATCH 也改成事务，另议）；③ 主线（ci tip）的流转事务已包 `surreal_retry`，`resave` 事务暂未包——落 A 案时顺手补。**基线告警**：`858c94fb0` 不是 `gen-model-refactor` 的祖先（见 Q9），D1–D6 在主线 tip 同样存在（`tasks.rs:516-632 / :752-754 / :2003 / :2239`）。
- 2026-09-29 · **Q9=A 定稿 + T1/T2 入库**（fable-5-1-6）· 用户拍板落主线：提交 `0af24df2d` 到 `ci/linux-build-refactor-20260928`（4 个文件：tasks.rs / review_form.rs / platform/tests.rs / 本文）。`old/gen-model@858c94fb0` 工作树里同一份改动**仍在、未提交**（那棵树编不过，且不是主线祖先；其 M 里混着别的会话的 rustfmt 重排，无法只回滚我那部分）——以主线为准，那份可随时丢弃。
- 2026-09-29 · **T3 + T4 落地**（fable-5-1-6，主线 `ci/linux-build-refactor-20260928`）· 改动文件：`src/web_service/review/platform/workflow_sync.rs`（`verify_workflow_handler` 删掉「active + blank 放行」分支；`load_task_for_workflow` 404 分支不再填 `task_status=blank`，「尚未创建活动 review task」分支加 `with_verify_block_code("TASK_NOT_CREATED")` 且 reason 附「表单尚未保存编校审数据，请设计人员先在三维页保存」；`build_verify_pass_data` 多收 `actor_id`，external 模式通过响应也回填 `actor_id / owner_id / owner_source`（空则 `null`，`expected_next_node` 通过时仍为 `null` 不变））、`src/web_service/review/platform/tests.rs`（两条反向：`…blank_form_without_task_soft_blocks_active`、`…missing_form_returns_404_for_every_action`；新增 `test_workflow_verify_matches_sync_outcome_for_active`（V5 对拍：blank 都挡 / 不存在都 404 / sj 草稿都过且 verify 不动库、sync 推到 jd）与 `test_workflow_verify_pass_carries_owner_diagnostics_in_external_mode`（V6）；文件头两行表格改口）、`docs/specs/review-api.md`（§8.1 信任边界与 owner 校验、§8.2 verify ≡ sync 干跑、§8.3 二次保存语义；§12 变更记录）。**验证**：`cargo test --lib --features http_api web_service::review` → **165 passed / 0 failed**。**与本文口径的偏离**：D3 的 `TASK_NOT_CREATED` 对空白主单据的**所有** action 生效（不只 active）——`agree / return` 对 blank 本来就是软阻断，只是补了 block_code，判定不变；§8.1 按 D8 现状如实写「边界事实上覆盖到任一持嵌入 token 的浏览器」，Q6 定稿后再改口。
- 2026-09-29 · **T5 落地**（fable-5-1-6，`plant3d-web` `main`，提交 `76e142f0`，用户拍板）· 改动：`src/components/review/InitiateReviewPanel.vue`（external 且 `restoredTaskDraft.taskId` 存在 → 标题「修改已保存的编校审单」、黄条提示「正在修改已保存的编校审单（task …）：保存会覆盖构件、描述与附件；单据已送审时后端会拒绝……」、按钮「保存修改」；`data-testid` `initiate-review-panel-title` / `initiate-review-editing-saved-task`）、`src/composables/useUserStore.ts`（`createReviewTask` 收到与本地列表同 id 的任务时按 id 替换而非 push）、`src/composables/useUserStore.createReviewTask.test.ts`（+2：同 formId 二次保存不堆重复项；409 原文抛错且不回退本地）、新增 `src/components/review/InitiateReviewPanel.resaveSavedTask.test.ts`（3 条：已保存单据文案 / 首建文案不变 / 409 原文进 `notification.details` 不被「编校审单保存失败」盖掉）、`docs/verification/pms-3d-review-integration-e2e.md`（TC-1 加 2b / 2c / 2d：重开改构件再保存 → 同一 task、JH 端看到新构件；送审后再保存 → 409；§9 变更记录）。T5-3 核对：后端 409 经 `fetchJson` → `ReviewApiHttpError(message = error_message)` → `createReviewTask` 非网络错误原样上抛 → 面板 `notification.details`，**零代码改动即成立**，只补了测试。**验证**：`vitest run` 三个文件 → **26 passed / 0 failed**（12 + 3 + 11）；`eslint` 对改动的 `.ts` 零错误（`InitiateReviewPanel.vue` 的 3 条 import-x/order 是 HEAD 上就有的）。**未做**：V7 的 e2e 实跑与截图——需要线上包部署 `0af24df2d` 之后的后端，2b / 2c / 2d 在 e2e 文档里标了「待实跑」。
- 2026-09-29 · **第 2 段审核**（fable-5-1-6）· reviewer 嵌入链路 `DockLayout` → `embedContextRestore` / `embedFormSnapshotRestore` → `ReviewPanel` / `useReviewStore` 确认记录 → `annotation_check` 闸门（`plant3d-web main@52b93c24`，较 `7ca2c6e6` 只多两笔与校审无关的提交）。新增 **D7 / D8 / D9 / D5b** 进 §1、**Q6–Q9** 进 §7、拟增 **T6 / T7** 进 §4；无问题项记在 §1.1。D8 把 D4 从「成文即可」抬成 P1：嵌入 token 就是 S2S token，前端自己也拿它调 `workflow/sync`。
