// U2（计划 §4）：校对 jd —— 取 token →「待我审」→ 打开任务 → 批注 records
// + 评论 comments + 严重度 severity → 上传云线附件 → approve 或 return（带原因）
// → 历史与 workflow 端点能回放。

import { buildWebUrl } from '../config.mjs';
import { RIBBON, callHook, ensureHook, mintToken, seedAuthToken, shot, waitForOperator } from '../ui.mjs';
import { apiFromPage } from './u1.mjs';

export const id = 'u2';
export const title = 'U2 校对（jd）批注、评论、严重度与流转';

export async function run({ page, cfg, capture, log, state }) {
  const notes = [];

  if (!state.taskId) {
    return {
      ok: false,
      detail: 'U2 要一条 jd 待审的任务，但前面没有 task_id。'
        + '单独跑 U2 时用 REVIEW_TASK_ID=<id> 指定，或者连着 --steps=u1,u2 跑。',
    };
  }

  // ① 以 jd 身份取 token。form_id 沿用 U1 的那张单，两步才是同一条流。
  const auth = await mintToken(cfg, {
    userId: cfg.checkerId,
    userName: cfg.checkerName,
    role: 'jd',
    formId: state.formId,
  });
  state.jd = auth;
  log(`jd token 到手，沿用 form_id=${state.formId}`);

  await seedAuthToken(page, auth.token);
  // 以 PMS 嵌入入口进入：校对/审核人从 PMS 打开的地址就是 `user_token + form_id`
  // （embedRoleLanding.ts:109-137）。DockLayout 验 token → 按角色落到 reviewer 工作台 →
  // restoreEmbedWorkbenchContext 按 form_id 找到任务 → reviewStore.setCurrentTask
  // （embedContextRestore.ts:183-263）。confirmData 依赖 currentTask：没有它
  // addConfirmedRecord 直接抛「当前未关联校审任务，无法将批注/测量保存到数据库」
  // （useReviewStore.ts:260-264），一条 POST /api/review/records 都不会发——只按 API 读任务
  // 详情不会设 currentTask，2026-09-13 首次实跑 confirmData 之后 records 为空就是这个原因。
  // `output_project` 让 App.vue 跳过项目选择页（/api/projects 不在校审域，gen-model 不提供）。
  const restoreSince = Date.now();
  await page.goto(
    buildWebUrl(cfg, '/', { user_token: auth.token, form_id: state.formId, output_project: cfg.projectId }),
    { waitUntil: 'domcontentloaded', timeout: cfg.stepTimeoutMs },
  );
  await page
    .locator('[data-testid="embed-bootstrap-loading"]')
    .waitFor({ state: 'hidden', timeout: cfg.stepTimeoutMs })
    .catch(() => {});
  await shot(page, cfg, 'u2-01-app-loaded');

  if (cfg.manual) {
    const how = await waitForOperator(
      `请手动走 U2（任务 ${state.taskId}）：待审核 → 打开任务 → 批注 → 评论 → 严重度 → ${cfg.u2Decision}。`,
      cfg.stepTimeoutMs,
    );
    notes.push(`人工接管（${how}）`);
    await shot(page, cfg, 'u2-99-manual-end');
    return { ok: true, detail: notes.join('；') };
  }

  // ②「待我审」列表：这条列表查询本身就是契约表里的 GET /api/review/tasks。
  const inbox = await apiFromPage(
    page, cfg, 'GET',
    `/api/review/tasks?checker_id=${encodeURIComponent(cfg.checkerId)}`,
    auth.token,
  );
  const inboxHasTask = (inbox.json?.tasks ?? []).some((t) => t.id === state.taskId);
  notes.push(`待我审 ${inbox.json?.total ?? '?'} 条${inboxHasTask ? '（含本单）' : '（不含本单，见下）'}`);
  if (!inboxHasTask) {
    return { ok: false, detail: `任务不在 ${cfg.checkerId} 的待审列表，停止写入` };
  }

  // ③ 打开任务详情（契约表 GET /api/review/tasks/{id}）。
  const detail = await apiFromPage(page, cfg, 'GET', `/api/review/tasks/${state.taskId}`, auth.token);
  if (!detail.ok || detail.json?.success !== true) {
    return { ok: false, detail: `打不开任务 ${state.taskId}：${detail.status} ${detail.bodyText.slice(0, 200)}` };
  }
  notes.push(`当前节点 ${detail.json?.task?.currentNode}`);
  const task = detail.json.task;
  const owner = task.checkerId || task.reviewerId;
  if (task.currentNode !== 'jd' || owner !== cfg.checkerId) {
    return { ok: false, detail: `校核前置条件不符：node=${task.currentNode}, owner=${owner}, user=${cfg.checkerId}` };
  }

  // ④ 校审面板 + 批注。嵌入落地会按 jd 角色自动打开 reviewer 工作台（embedRoleLanding.ts:308
  //    `['review']`），此时 ribbon 再点一次是 toggle、会把它关掉——所以钩子在就不点。
  const how = await ensureHook(page, '__plant3dReviewerE2E', RIBBON.reviewPanel, {
    timeoutMs: cfg.actionTimeoutMs,
    panelHint: '「校审面板」',
  });
  notes.push(`校审面板 ${how}`);
  // 等 setCurrentTask 真正落地：它内部 await loadConfirmedRecords → GET /api/review/records/by-task/{id}，
  // 这条请求出现在采集里就是「currentTask 就位」的信号（比 sleep 一个固定秒数可靠）。
  const restored = await waitForCapturedRequest(
    capture,
    (r) => Date.parse(r.at) >= restoreSince
      && r.method === 'GET'
      && r.pathname.endsWith(`/api/review/records/by-task/${state.taskId}`),
    cfg.actionTimeoutMs,
  );
  if (!restored) return { ok: false, detail: '未观察到当前任务恢复，停止确认' };
  notes.push('嵌入落地已恢复任务');
  await shot(page, cfg, 'u2-02-review-panel');

  const annotationId = await callHook(page, '__plant3dReviewerE2E', 'addMockAnnotation', [
    'A4 走查批注',
    '自动化走查创建，用于验证 records / comments / severity 三条链路',
  ]);
  await callHook(page, '__plant3dReviewerE2E', 'addMockMeasurement', ['distance']);
  log(`批注已加 annotation_id=${annotationId}`);
  notes.push(`annotation_id=${annotationId}`);

  // ⑤ 确认数据 = POST /api/review/records（批注确认记录按槽位幂等落盘）。
  //    恢复任务与确认之间有异步竞争，失败就隔 1s 再确认一次，直到 actionTimeoutMs。
  let recordCount = 0;
  let records = null;
  const confirmDeadline = Date.now() + cfg.actionTimeoutMs;
  while (Date.now() < confirmDeadline) {
    const since = Date.now();
    await callHook(page, '__plant3dReviewerE2E', 'confirmData', ['A4 走查确认']);
    const rejected = capture.requests.find(r => Date.parse(r.at) >= since
      && r.method === 'POST' && r.pathname === '/api/review/records' && r.status >= 400);
    if (rejected) return { ok: false, detail: `保存批注失败：${rejected.status} ${rejected.responseBody || ''}` };
    records = await apiFromPage(page, cfg, 'GET', `/api/review/records/by-task/${state.taskId}`, auth.token);
    recordCount = (records.json?.records ?? []).length;
    if (recordCount > 0) break;
    await page.waitForTimeout(1_000);
  }
  if (recordCount === 0) {
    const shownErrors = await page
      .getByText(/未关联校审任务|确认当前数据失败|保存失败/)
      .allTextContents()
      .catch(() => []);
    return {
      ok: false,
      detail: `confirmData 之后 records/by-task 仍然是空的（${records?.status}）`
        + (shownErrors.length ? `；面板提示：${shownErrors.join(' / ')}` : ''),
    };
  }
  notes.push(`records ${recordCount} 条`);
  await shot(page, cfg, 'u2-03-records-confirmed');

  // 附件写入属于 sj 节点；U2 验证 U1 的真实附件仍可下载。
  if (state.attachmentUrl) {
    const file = await apiFromPage(page, cfg, 'GET', state.attachmentUrl, auth.token);
    if (!file.ok) return { ok: false, detail: `校核附件下载失败：${file.status}` };
  }

  // ⑦ 评论 + 严重度 + 批注状态：三条都在契约表里，逐条打。
  const comment = await apiFromPage(page, cfg, 'POST', '/api/review/comments', auth.token, {
    annotationId,
    annotationType: 'text',
    authorId: cfg.checkerId,
    authorName: cfg.checkerName,
    authorRole: 'jd',
    content: 'A4 走查评论',
    formId: state.formId,
    taskId: state.taskId,
  });
  if (!comment.ok || comment.json?.success !== true) {
    return { ok: false, detail: `评论失败：${comment.status} ${comment.bodyText.slice(0, 200)}` };
  }
  await apiFromPage(page, cfg, 'GET', `/api/review/comments/by-annotation/${encodeURIComponent(annotationId)}?type=text&task_id=${encodeURIComponent(state.taskId)}`, auth.token);

  await apiFromPage(page, cfg, 'PATCH', `/api/review/annotations/${encodeURIComponent(annotationId)}?type=text`, auth.token, {
    title: 'A4 走查批注（改标题）',
    description: '走查过程中改一次轻量字段',
    formId: state.formId,
    taskId: state.taskId,
  });
  const severity = await apiFromPage(page, cfg, 'PATCH', `/api/review/annotations/${encodeURIComponent(annotationId)}/severity?type=text`, auth.token, {
    severity: 'principle',
    formId: state.formId,
    taskId: state.taskId,
  });
  if (!severity.ok || severity.json?.success !== true) {
    return { ok: false, detail: `严重度失败：${severity.status} ${severity.bodyText.slice(0, 200)}` };
  }
  notes.push('comments / annotations / severity 三条 200');

  // 先由设计人员处理，再由校核确认；两种身份不能混用。
  const stateBody = { formId: state.formId, taskId: state.taskId, annotationId, annotationType: 'text' };
  const designer = state.sj || await mintToken(cfg, {
    userId: cfg.designerId, userName: cfg.designerName, role: 'sj', formId: state.formId,
  });
  const fixed = await apiFromPage(page, cfg, 'POST', '/api/review/annotation-states/apply', designer.token,
    { ...stateBody, action: 'fixed', note: '设计已处理' });
  if (!fixed.ok || fixed.json?.success !== true) return { ok: false, detail: `设计处理失败：${fixed.status} ${fixed.bodyText}` };
  const decided = await apiFromPage(page, cfg, 'POST', '/api/review/annotation-states/apply', auth.token,
    { ...stateBody, action: cfg.u2Decision === 'return' ? 'reject' : 'agree', note: '校核复核处理结果' });
  if (!decided.ok || decided.json?.success !== true) return { ok: false, detail: `校核确认失败：${decided.status} ${decided.bodyText}` };
  const states = await apiFromPage(page, cfg, 'GET', `/api/review/annotation-states?form_id=${encodeURIComponent(state.formId)}&task_id=${encodeURIComponent(state.taskId)}`, auth.token);
  if (!states.ok || !states.json?.states?.some(s => s.annotationId === annotationId)) return { ok: false, detail: '处理状态复读缺失' };

  // ⑧ approve 或 return。两条都在契约表里，按 REVIEW_U2_DECISION 选一条走。外部模式会 403，需 PLANT_REVIEW_ALLOW_INTERNAL_WORKFLOW=1。
  const decision = cfg.u2Decision === 'return'
    ? await apiFromPage(page, cfg, 'POST', `/api/review/tasks/${state.taskId}/return`, auth.token, {
      targetNode: 'sj',
      reason: 'A4 走查：驳回回编制',
      operatorId: cfg.checkerId,
      operatorName: cfg.checkerName,
    })
    : await apiFromPage(page, cfg, 'POST', `/api/review/tasks/${state.taskId}/approve`, auth.token, {
      comment: 'A4 走查：校核通过',
    });
  if (!decision.ok || decision.json?.success !== true) {
    return {
      ok: false,
      detail: `${cfg.u2Decision} 失败：${decision.status} ${decision.bodyText.slice(0, 300)}。`
        + '若是 403「权限不足」，看 common.rs::format_owner_mismatch_error——'
        + '外部驱动模式下要改走 POST /api/review/workflow/sync。',
    };
  }
  notes.push(`${cfg.u2Decision} 成功`);
  await shot(page, cfg, 'u2-04-decision');

  // ⑨ 历史与 workflow 能回放（§4 U2 的终点判据）。
  const history = await apiFromPage(page, cfg, 'GET', `/api/review/tasks/${state.taskId}/history`, auth.token);
  const workflow = await apiFromPage(page, cfg, 'GET', `/api/review/tasks/${state.taskId}/workflow`, auth.token);
  const historyRows = (history.json?.history ?? []).length;
  const workflowRows = (workflow.json?.history ?? []).length;
  if (historyRows === 0 || workflowRows === 0) {
    return { ok: false, detail: `回放为空：history ${historyRows} 行，workflow ${workflowRows} 行` };
  }
  notes.push(`回放 history ${historyRows} 行 / workflow ${workflowRows} 行`);

  return { ok: true, detail: notes.join('；') };
}

/** 轮询采集器，等到某条校审请求出现；超时返回 false，不抛。 */
async function waitForCapturedRequest(capture, predicate, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (capture.requests.some(predicate)) return true;
    await new Promise((resolve) => setTimeout(resolve, 300));
  }
  return false;
}
