// PMS lifecycle on a real task; each endpoint keeps its own wire envelope.
import assert from 'node:assert/strict';
import { mintToken, shot } from '../ui.mjs';
import { apiFromPage } from './u1.mjs';
import { redact } from '../capture.mjs';

export const id = 'u3';
export const title = 'U3 PMS 嵌入、真实流转、批注门禁与软删';

export async function run({ page, cfg, log }) {
  const sj = await mintToken(cfg, { userId: cfg.designerId, userName: cfg.designerName, role: 'sj' });
  const formId = sj.formId;
  const jd = await mintToken(cfg, { userId: cfg.checkerId, userName: cfg.checkerName, role: 'jd', formId });
  await page.goto(`${cfg.webBaseUrl}/pms-review-simulator.html`, { waitUntil: 'domcontentloaded' });
  const call = async (path, body, token = sj.token, method = 'POST') => {
    const res = await apiFromPage(page, cfg, method, path, token, body);
    assert.equal(res.status, 200, `${method} ${path}: ${res.status} ${redact(res.bodyText)}`);
    return res.json;
  };
  const platform = async (path, body, token = sj.token, code = 200) => {
    const data = await call(path, { ...body, token }, token);
    assert.equal(data.code, code, `${path}: ${redact(JSON.stringify(data))}`);
    return data;
  };
  // Negative probes stay separate from normal browser traffic and assert exact outcomes.
  const negative = async (path, body, status, predicate) => {
    const res = await fetch(`${cfg.backendBaseUrl}${path}`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body), signal: AbortSignal.timeout(cfg.probeTimeoutMs),
    });
    const json = await res.json();
    assert.equal(res.status, status, `${path} negative: ${redact(JSON.stringify(json))}`);
    assert.ok(predicate(json), `${path} negative envelope: ${redact(JSON.stringify(json))}`);
  };
  const created = await call('/api/review/tasks', {
    title: `PMS ${cfg.runId}`, modelName: cfg.projectId, formId,
    checkerId: cfg.checkerId, reviewerId: cfg.checkerId, approverId: 'SH',
    components: [{ id: 'pms-bran', refNo: '24381_145018', name: '验收 BRAN', type: 'BRAN' }],
  });
  assert.equal(created.success, true);
  const taskId = created.task.id;
  assert.equal(created.task.formId, formId);
  const embed = await platform('/api/review/embed-url', {
    project_id: cfg.projectId, user_id: cfg.designerId, workflow_role: 'sj', form_id: formId,
  });
  assert.equal(embed.data.lineage.task_id, taskId);
  const embedUrl = new URL(embed.url);
  assert.ok(embedUrl.searchParams.get('user_token'));
  // Same contract as plant3d-web reviewApi.buildTokenPrimaryEmbedUrl and PMS:
  // JWT intentionally has no form_id; compose it from data.query.form_id.
  assert.equal(embed.data.query.form_id, formId);
  embedUrl.searchParams.set('form_id', embed.data.query.form_id);
  log(`PMS task=${taskId}, form=${formId}`);
  const restored = page.waitForResponse(r => r.url().includes(`/api/review/records/by-task/${taskId}`) && r.status() === 200,
    { timeout: cfg.actionTimeoutMs }).then(() => true, () => false);
  await page.goto(embedUrl.toString(), { waitUntil: 'domcontentloaded' });
  if (!await restored) {
    await shot(page, cfg, 'u3-embed-failed');
    return { ok: false, detail: `嵌入未恢复任务；${(await page.locator('body').innerText()).slice(-1500)}` };
  }
  assert.equal(await page.getByRole('heading', { name: '选择项目', exact: true }).count(), 0);
  await shot(page, cfg, 'u3-01-embed-restored');

  await platform('/api/review/cache/preload', { project_id: cfg.projectId, initiator: cfg.designerId }, sj.token, 0);
  const active = await platform('/api/review/workflow/sync', {
    form_id: formId, action: 'active',
    actor: { id: cfg.designerId, roles: 'sj', name: cfg.designerName },
    next_step: { assignee_id: cfg.checkerId, roles: 'jd', name: cfg.checkerName }, comments: 'PMS 送校核',
  });
  assert.equal(active.data.current_node, 'jd');
  const annotationId = `pms-annotation-${cfg.runId}`;
  const record = await call('/api/review/records', {
    taskId, formId, type: 'review', annotations: [{ id: annotationId, title: '待处理问题', refnos: ['24381_145018'],
      authorId: cfg.checkerId, authorRole: 'jd', reviewState: { resolutionStatus: 'open', decisionStatus: 'pending' } }],
  }, jd.token);
  assert.equal(record.success, true);
  const gate = await call('/api/review/annotations/check', { form_id: formId, task_id: taskId, token: jd.token, intent: 'agree_advance' }, jd.token);
  assert.equal(gate.success, true);
  assert.equal(gate.data.passed, false);
  assert.ok(gate.data.summary.open > 0);
  await negative('/api/review/workflow/verify', { form_id: formId, token: jd.token, action: 'agree' }, 200,
    j => j.data?.passed === false || j.annotation_check?.passed === false);

  const state = { formId, taskId, annotationId, annotationType: 'text' };
  assert.equal((await call('/api/review/annotation-states/apply', { ...state, action: 'fixed' })).success, true);
  assert.equal((await call('/api/review/annotation-states/apply', { ...state, action: 'agree' }, jd.token)).success, true);
  const passed = await call('/api/review/annotations/check', { form_id: formId, task_id: taskId, token: jd.token, intent: 'agree_advance' }, jd.token);
  assert.equal(passed.success, true);
  assert.equal(passed.data.passed, true);
  const verify = await platform('/api/review/workflow/verify', { form_id: formId, action: 'agree' }, jd.token);
  assert.equal(verify.data.passed, true);
  assert.equal(verify.data.current_node, 'jd');
  const agreed = await platform('/api/review/workflow/sync', {
    form_id: formId, action: 'agree',
    next_step: { assignee_id: 'SH', roles: 'sh', name: '审核' }, comments: 'PMS 校核通过',
  }, jd.token);
  assert.equal(agreed.data.current_node, 'sh');
  const task = await call(`/api/review/tasks/${taskId}`, undefined, sj.token, 'GET');
  assert.equal(task.task.currentNode, 'sh');
  assert.equal(task.task.approverId, 'SH');
  const history = await call(`/api/review/tasks/${taskId}/history`, undefined, sj.token, 'GET');
  assert.ok(history.history.length >= 2);

  await negative('/api/review/workflow/sync', { form_id: formId, token: 'invalid-token', action: 'query' }, 401, j => j.code === 401);
  await negative('/api/review/annotations/check', { form_id: `${formId}-missing`, token: sj.token }, 404, j => j.success === false);
  await shot(page, cfg, 'u3-02-workflow-complete');
  if (cfg.cleanup) {
    const deleted = await platform('/api/review/delete', { form_ids: [formId], operator_id: cfg.designerId });
    assert.ok(deleted.results.every(r => r.success));
    const list = await call(`/api/review/tasks?formId=${encodeURIComponent(formId)}`, undefined, sj.token, 'GET');
    assert.equal(list.tasks.length, 0);
  }
  return { ok: true, detail: `task=${taskId}；六接口已验；嵌入恢复；sj→jd→sh；门禁 open 阻止、处理后通过；错误 token 401、缺任务 404 独立负例；软删=${cfg.cleanup}` };
}
