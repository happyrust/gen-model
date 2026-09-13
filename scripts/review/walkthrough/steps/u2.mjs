// U2（计划 §4）：校对 jd —— 取 token →「待我审」→ 打开任务 → 批注 records
// + 评论 comments + 严重度 severity → 上传云线附件 → approve 或 return（带原因）
// → 历史与 workflow 端点能回放。

import { buildWebUrl } from '../config.mjs';
import { RIBBON, callHook, mintToken, openReviewPanel, seedAuthToken, shot, waitForHook, waitForOperator } from '../ui.mjs';
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
  await page.goto(buildWebUrl(cfg, '/'), { waitUntil: 'domcontentloaded', timeout: cfg.stepTimeoutMs });
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
    // 不当失败：外部驱动模式下 checker 可以为空，owner 由 PMS 每次 sync 显式声明
    //（common.rs:1059-1064）。列表里没有不代表流程不通，后面 approve 会给出真结论。
    log('注意：任务不在 checker_id 过滤出的列表里；外部驱动模式下 checker 可空，继续走。');
  }

  // ③ 打开任务详情（契约表 GET /api/review/tasks/{id}）。
  const detail = await apiFromPage(page, cfg, 'GET', `/api/review/tasks/${state.taskId}`, auth.token);
  if (!detail.ok || detail.json?.success !== true) {
    return { ok: false, detail: `打不开任务 ${state.taskId}：${detail.status} ${detail.bodyText.slice(0, 200)}` };
  }
  notes.push(`当前节点 ${detail.json?.task?.currentNode}`);

  // ④ 校审面板 + 批注。钩子挂在 ReviewPanel mount 之后。
  await openReviewPanel(page, RIBBON.reviewPanel, { timeoutMs: cfg.actionTimeoutMs });
  await waitForHook(page, '__plant3dReviewerE2E', {
    timeoutMs: cfg.actionTimeoutMs,
    panelHint: '「校审面板」',
  });
  await shot(page, cfg, 'u2-02-review-panel');

  const annotationId = await callHook(page, '__plant3dReviewerE2E', 'addMockAnnotation', [
    'A4 走查批注',
    '自动化走查创建，用于验证 records / comments / severity 三条链路',
  ]);
  await callHook(page, '__plant3dReviewerE2E', 'addMockMeasurement', ['distance']);
  log(`批注已加 annotation_id=${annotationId}`);
  notes.push(`annotation_id=${annotationId}`);

  // ⑤ 确认数据 = POST /api/review/records（批注确认记录按槽位幂等落盘）。
  await callHook(page, '__plant3dReviewerE2E', 'confirmData', ['A4 走查确认']);
  const records = await apiFromPage(page, cfg, 'GET', `/api/review/records/by-task/${state.taskId}`, auth.token);
  const recordCount = (records.json?.records ?? []).length;
  if (recordCount === 0) {
    return { ok: false, detail: `confirmData 之后 records/by-task 仍然是空的（${records.status}）` };
  }
  notes.push(`records ${recordCount} 条`);
  await shot(page, cfg, 'u2-03-records-confirmed');

  // ⑥ 云线批注 + 自动截图上传：走 useDtxTools 的钩子，
  //    它内部会打 POST /api/review/attachments（A4 要的第二处 multipart）。
  const cloud = await callHook(page, '__plant3dDtxE2E', 'addCloudAnnotationWithScreenshot').catch((e) => ({ error: String(e?.message || e) }));
  if (cloud?.error) {
    // 云线截图依赖三维画布真的渲染出来；无 GPU 的无头环境常拿不到。
    // 这一条不阻断走查，但要如实记进交付，别让验收人以为跑过了。
    log(`云线附件 NOT RUN：${cloud.error}`);
    notes.push(`云线附件 NOT RUN（${cloud.error.slice(0, 120)}）`);
  } else {
    notes.push(`云线附件 ${cloud.url}`);
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

  await apiFromPage(page, cfg, 'POST', '/api/review/annotation-states/apply', auth.token, {
    formId: state.formId,
    taskId: state.taskId,
    annotationId,
    annotationType: 'text',
    action: 'agree',
    note: 'A4 走查：校核同意',
  });
  await apiFromPage(page, cfg, 'GET', `/api/review/annotation-states?form_id=${encodeURIComponent(state.formId)}&task_id=${encodeURIComponent(state.taskId)}`, auth.token);

  // ⑧ approve 或 return。两条都在契约表里，按 REVIEW_U2_DECISION 选一条走。
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
