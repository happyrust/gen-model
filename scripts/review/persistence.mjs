// A6: seed through real HTTP APIs, then compare all data after a double restart.
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import { createHash, randomUUID } from 'node:crypto';
import { redact } from './walkthrough/capture.mjs';

const [mode, base, file] = process.argv.slice(2);
assert.ok(['seed', 'verify'].includes(mode) && base && file, 'usage: node persistence.mjs seed|verify BASE STATE_FILE');
async function call(path, body, token, method = 'POST') {
  const res = await fetch(`${base}${path}`, {
    method, headers: { ...(body ? { 'Content-Type': 'application/json' } : {}), ...(token ? { Authorization: `Bearer ${token}` } : {}) },
    body: body ? JSON.stringify(body) : undefined, signal: AbortSignal.timeout(20000),
  });
  const json = await res.json();
  assert.equal(res.status, 200, `${method} ${path}: ${redact(JSON.stringify(json))}`);
  assert.notEqual(json.success, false, `${path}: ${redact(JSON.stringify(json))}`);
  return json;
}
async function token(user, role, form) {
  return (await call('/api/auth/token', { project_id: 'acceptance', user_id: user, user_name: user, role, form_id: form })).data.token;
}
async function snapshot(state, sj) {
  const { taskId, formId, annotationId, attachmentUrl } = state;
  const reads = {
    task: `/api/review/tasks/${taskId}`,
    records: `/api/review/records/by-task/${taskId}`,
    comments: `/api/review/comments/by-annotation/${annotationId}?type=text&task_id=${taskId}`,
    states: `/api/review/annotation-states?form_id=${formId}&task_id=${taskId}`,
    history: `/api/review/tasks/${taskId}/history`,
    workflow: `/api/review/tasks/${taskId}/workflow`,
  };
  const data = {};
  for (const [name, path] of Object.entries(reads)) data[name] = await call(path, undefined, sj, 'GET');
  assert.ok(data.task.task.attachments.length > 0, 'attachment metadata missing');
  assert.ok(data.records.records.length > 0, 'records missing');
  assert.ok(data.comments.comments.length > 0, 'comments missing');
  assert.ok(data.states.states.some(s => s.annotationId === annotationId && s.decisionStatus === 'agreed'), 'state missing');
  assert.ok(data.history.history.length > 0 && data.workflow.history.length > 0, 'workflow history missing');
  const res = await fetch(`${base}${attachmentUrl}`, { signal: AbortSignal.timeout(20000) });
  assert.equal(res.status, 200, 'attachment download failed');
  const bytes = Buffer.from(await res.arrayBuffer());
  assert.ok(bytes.length > 0);
  data.attachment = { bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') };
  return data;
}
try {
  if (mode === 'seed') {
    const formId = `PERSIST-${randomUUID()}`;
    const sj = await token('SJ', 'sj', formId);
    const jd = await token('JH', 'jd', formId);
    const task = await call('/api/review/tasks', { formId, title: 'A6 complete persistence', modelName: 'acceptance', checkerId: 'JH', reviewerId: 'JH', approverId: 'SH' }, sj);
    const taskId = task.task.id;
    const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+iS9cAAAAASUVORK5CYII=', 'base64');
    const form = new FormData();
    form.set('taskId', taskId); form.set('fileType', 'markup'); form.set('file', new Blob([png], { type: 'image/png' }), 'persist.png');
    const upload = await fetch(`${base}/api/review/attachments`, { method: 'POST', headers: { Authorization: `Bearer ${sj}` }, body: form, signal: AbortSignal.timeout(20000) });
    assert.equal(upload.status, 200);
    const attachment = (await upload.json()).attachment;
    assert.ok(attachment.url);
    const active = await call('/api/review/workflow/sync', { form_id: formId, token: sj, action: 'active', next_step: { assignee_id: 'JH', roles: 'jd', name: '校核' }, comments: 'Persistence submit' });
    assert.equal(active.data.current_node, 'jd');
    const annotationId = `persist-${randomUUID()}`;
    await call('/api/review/records', { taskId, formId, type: 'review', annotations: [{ id: annotationId, title: 'Persistence annotation', refnos: ['24381_145018'], authorId: 'JH', authorRole: 'jd' }], note: 'A6 nonempty record' }, jd);
    await call('/api/review/comments', { taskId, formId, annotationId, annotationType: 'text', authorId: 'JH', authorName: '校核', authorRole: 'jd', content: 'A6 persistent comment' }, jd);
    const body = { taskId, formId, annotationId, annotationType: 'text' };
    await call('/api/review/annotation-states/apply', { ...body, action: 'fixed' }, sj);
    await call('/api/review/annotation-states/apply', { ...body, action: 'agree' }, jd);
    const state = { taskId, formId, annotationId, attachmentUrl: attachment.url };
    state.before = await snapshot(state, sj);
    writeFileSync(file, JSON.stringify(state, null, 2));
    console.log(`A6 seed PASS: ${taskId}, records/comments/states/history/workflow nonempty, attachment SHA256=${state.before.attachment.sha256}`);
  } else {
    const state = JSON.parse(readFileSync(file, 'utf8'));
    const after = await snapshot(state, await token('SJ', 'sj', state.formId));
    assert.deepEqual(after, state.before, 'persisted HTTP snapshots differ after restart');
    console.log(`A6 verify PASS: ${state.taskId}, all 7 snapshots identical after restart`);
  }
} catch (error) {
  console.error(redact(error.stack));
  process.exitCode = 1;
}
