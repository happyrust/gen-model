// U4（补充覆盖，非 §4 用户路径）：批注编辑/删除 API 探针。
// 盖住 U1/U2/U3 三条用户路径不触发的 4 条契约接口：
//   PATCH  /api/review/comments/item/{comment_id}     改评论
//   DELETE /api/review/comments/item/{comment_id}     删评论（软删，作者==调用者）
//   DELETE /api/review/records/item/{record_id}        删单条确认记录
//   DELETE /api/review/records/clear-task/{task_id}    清空任务确认记录
//
// 依赖 u1 建好并已提交到 jd 的任务（用 `--steps=u1,u4` 跑）：提交后当前节点是 jd，
// 而确认记录的 operator 必须等于节点负责人（records.rs::create_record 的 owner-match），
// 所以本步一律以 jd 身份建/删记录与评论（delete_comment 也要求 author==调用者）。
// 纯 API，但都经浏览器上下文（apiFromPage）发出，请求才会计入采集与契约勾选。

import { buildWebUrl } from '../config.mjs';
import { mintToken, seedAuthToken, shot } from '../ui.mjs';
import { apiFromPage } from './u1.mjs';

export const id = 'u4';
export const title = 'U4 批注编辑/删除 API 探针（comment PATCH/DELETE、records item/clear-task）';

export async function run({ page, cfg, log, state }) {
  const notes = [];
  if (!state.taskId || !state.formId) {
    return {
      ok: false,
      detail: 'U4 需要 u1 建好并提交到 jd 的任务；请用 --steps=u1,u4 跑（缺 task_id/form_id）。',
    };
  }

  const jd = state.jd || await mintToken(cfg, {
    userId: cfg.checkerId, userName: cfg.checkerName, role: 'jd', formId: state.formId,
  });
  const token = jd.token;
  await seedAuthToken(page, token);
  // 只需要一个 app 源的页面上下文来发 fetch（query 档直连 :8022，靠 CORS）；不点面板、不等 app ready。
  await page.goto(buildWebUrl(cfg, '/'), { waitUntil: 'domcontentloaded', timeout: cfg.stepTimeoutMs });
  await shot(page, cfg, 'u4-01-app-loaded');

  const ctxQ = `form_id=${encodeURIComponent(state.formId)}&task_id=${encodeURIComponent(state.taskId)}`;

  // ── A. 评论 CRUD：POST 建 → PATCH 改 → DELETE 删 → 复读确认 ─────────────
  const annotationId = `e2e-crud-${Date.now()}`;
  const created = await apiFromPage(page, cfg, 'POST', '/api/review/comments', token, {
    annotationId, annotationType: 'text',
    authorId: cfg.checkerId, authorName: cfg.checkerName, authorRole: 'jd',
    content: 'A4 探针评论（待改待删）', formId: state.formId, taskId: state.taskId,
  });
  const commentId = created.json?.comment?.id;
  if (!created.ok || created.json?.success !== true || !commentId) {
    return { ok: false, detail: `建评论失败：${created.status} ${created.bodyText.slice(0, 200)}` };
  }
  log(`探针评论已建 comment_id=${commentId}`);
  notes.push(`comment ${commentId} 已建`);

  const edited = await apiFromPage(
    page, cfg, 'PATCH', `/api/review/comments/item/${encodeURIComponent(commentId)}`, token,
    { content: 'A4 探针评论（已改）', formId: state.formId, taskId: state.taskId },
  );
  if (!edited.ok || edited.json?.success !== true) {
    return { ok: false, detail: `PATCH 评论失败：${edited.status} ${edited.bodyText.slice(0, 200)}` };
  }
  const afterEdit = await apiFromPage(
    page, cfg, 'GET',
    `/api/review/comments/by-annotation/${encodeURIComponent(annotationId)}?type=text&${ctxQ}`, token,
  );
  const editedSeen = (afterEdit.json?.comments ?? []).some((c) => String(c.content || '').includes('已改'));
  notes.push(`PATCH 200${editedSeen ? '（复读见新内容）' : '（复读未见新内容）'}`);

  const delComment = await apiFromPage(
    page, cfg, 'DELETE', `/api/review/comments/item/${encodeURIComponent(commentId)}?${ctxQ}`, token,
  );
  if (!delComment.ok || delComment.json?.success !== true) {
    return { ok: false, detail: `DELETE 评论失败：${delComment.status} ${delComment.bodyText.slice(0, 200)}` };
  }
  const afterDel = await apiFromPage(
    page, cfg, 'GET',
    `/api/review/comments/by-annotation/${encodeURIComponent(annotationId)}?type=text&${ctxQ}`, token,
  );
  const commentGone = (afterDel.json?.comments ?? []).every((c) => c.id !== commentId);
  notes.push(`DELETE 200${commentGone ? '（复读已无该条）' : '（软删，复读仍列出）'}`);

  // ── B. 确认记录：建一条 → DELETE 单条 → clear-task 清空 → 复读为 0 ────────
  //   create_record 强校验 operator==当前节点负责人（records.rs::current_node_owner_for_task_context）：
  //   sj→requester、jd→checker、sh/pz→approver。所以先读当前节点，用该节点负责人的身份建记录——
  //   单跑 u1,u4 时任务停在 jd；接在 u1,u2(return) 之后时任务已回到 sj。
  const taskDetail = await apiFromPage(page, cfg, 'GET', `/api/review/tasks/${encodeURIComponent(state.taskId)}`, token);
  const node = String(taskDetail.json?.task?.currentNode ?? 'jd');
  const task = taskDetail.json?.task ?? {};
  let recordToken = token;
  let recordActor = 'jd';
  if (node === 'sj') {
    const sj = state.sj || await mintToken(cfg, { userId: cfg.designerId, userName: cfg.designerName, role: 'sj', formId: state.formId });
    recordToken = sj.token; recordActor = `sj(${cfg.designerId})`;
  } else if (node === 'sh' || node === 'pz') {
    const approverId = String(task.approverId || '').trim();
    if (approverId) {
      const sh = await mintToken(cfg, { userId: approverId, userName: approverId, role: node, formId: state.formId });
      recordToken = sh.token; recordActor = `${node}(${approverId})`;
    }
  }
  notes.push(`当前节点 ${node}，以 ${recordActor} 建记录`);
  const rec = await apiFromPage(page, cfg, 'POST', '/api/review/records', recordToken, {
    taskId: state.taskId, formId: state.formId, type: 'batch',
    annotations: [], cloudAnnotations: [], rectAnnotations: [], measurements: [],
    note: 'A4 探针确认记录（待删）',
  });
  if (!rec.ok || rec.json?.success !== true) {
    return { ok: false, detail: `建确认记录失败：${rec.status} ${rec.bodyText.slice(0, 200)}` };
  }
  const before = await apiFromPage(page, cfg, 'GET', `/api/review/records/by-task/${encodeURIComponent(state.taskId)}`, token);
  const beforeCount = (before.json?.records ?? []).length;
  const recordId = rec.json?.record?.id ?? (before.json?.records ?? [])[0]?.id;
  if (!recordId) return { ok: false, detail: `建了确认记录但复读不到 id（by-task ${beforeCount} 条）` };
  log(`探针确认记录已建 record_id=${recordId}（by-task ${beforeCount} 条）`);
  notes.push(`record ${recordId} 已建`);

  const delItem = await apiFromPage(page, cfg, 'DELETE', `/api/review/records/item/${encodeURIComponent(recordId)}`, token);
  if (!delItem.ok || delItem.json?.success !== true) {
    return { ok: false, detail: `DELETE 记录单条失败：${delItem.status} ${delItem.bodyText.slice(0, 200)}` };
  }
  const mid = await apiFromPage(page, cfg, 'GET', `/api/review/records/by-task/${encodeURIComponent(state.taskId)}`, token);
  notes.push(`DELETE item 200，by-task ${(mid.json?.records ?? []).length} 条`);

  const clear = await apiFromPage(page, cfg, 'DELETE', `/api/review/records/clear-task/${encodeURIComponent(state.taskId)}`, token);
  if (!clear.ok || clear.json?.success !== true) {
    return { ok: false, detail: `clear-task 失败：${clear.status} ${clear.bodyText.slice(0, 200)}` };
  }
  const after = await apiFromPage(page, cfg, 'GET', `/api/review/records/by-task/${encodeURIComponent(state.taskId)}`, token);
  const afterCount = (after.json?.records ?? []).length;
  if (afterCount !== 0) return { ok: false, detail: `clear-task 后 by-task 仍有 ${afterCount} 条` };
  notes.push('clear-task 200，by-task 0 条');
  await shot(page, cfg, 'u4-02-crud-done');

  return { ok: true, detail: notes.join('；') };
}
