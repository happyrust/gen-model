import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { redact } from './walkthrough/capture.mjs';

const base = (process.env.BASE_URL || 'http://localhost:8022').replace(/\/$/, '');
const fixture = name => JSON.parse(readFileSync(new URL(`./platform_api_json/${name}.json`, import.meta.url), 'utf8'));
let count = 0;
function check(value, message) { assert.ok(value, message); count++; }
async function post(path, body, code = 200, bearer) {
  const response = await fetch(`${base}${path}`, { method: 'POST',
    headers: { 'Content-Type': 'application/json', ...(bearer ? { Authorization: `Bearer ${bearer}` } : {}) },
    body: JSON.stringify(body), signal: AbortSignal.timeout(20000) });
  const json = await response.json();
  check(response.status === 200, `${path}: HTTP ${response.status} ${redact(JSON.stringify(json))}`);
  check(code === 'success' ? json.success === true : json.code === code, `${path}: ${redact(JSON.stringify(json))}`);
  console.log(`PASS ${path} HTTP 200 envelope=${code}`);
  return json;
}
try {
  const identity = fixture('auth_token');
  const auth = await post('/api/auth/token', identity, 0);
  const token = auth.data.token;
  const formId = auth.data.form_id;
  check(typeof token === 'string' && !!formId, 'missing token/form_id');
  check(!Object.hasOwn(JSON.parse(Buffer.from(token.split('.')[1], 'base64url')), 'form_id'), 'JWT must not contain form_id');
  const verified = await post('/api/auth/verify', { token }, 0);
  check(verified.data.valid === true, 'JWT verify invalid');
  if (process.env.EXPECT_WORKFLOW_MODE) {
    check((verified.data.claims.workflow_mode || verified.data.claims.workflowMode) === process.env.EXPECT_WORKFLOW_MODE, 'workflow mode mismatch');
  }
  const task = await post('/api/review/tasks', { title: 'JSON fixture acceptance', modelName: identity.project_id,
    formId, checkerId: 'JH', reviewerId: 'JH', approverId: 'SH' }, 'success', token);
  const embed = await post('/api/review/embed-url', { project_id: identity.project_id, user_id: identity.user_id,
    workflow_role: identity.role, form_id: formId, token });
  check(embed.data.lineage.task_id === task.task.id && embed.data.query.form_id === formId, 'embed task lineage mismatch');
  const url = new URL(embed.url);
  check(!!url.searchParams.get('user_token'), 'embed token missing');
  // PMS/plant3d-web assembles form_id from data.query; raw URL keeps legacy fields.
  url.searchParams.set('form_id', embed.data.query.form_id);
  check(url.searchParams.get('form_id') === formId, 'form_id composition failed');
  const query = await post('/api/review/workflow/sync', { ...fixture('workflow_sync_query'), token, form_id: formId });
  check(query.data.task_created === true, 'query missing real task');
  const active = await post('/api/review/workflow/sync', { ...fixture('workflow_sync_active'), token, form_id: formId });
  check(active.data.current_node === 'jd', 'active did not advance to jd');
  const checker = await post('/api/auth/token', { ...identity, user_id: 'JH', user_name: '校核', role: 'jd', form_id: formId }, 0);
  const verify = await post('/api/review/workflow/verify', { form_id: formId, token: checker.data.token, action: 'agree' });
  check(verify.data.passed === true && verify.data.current_node === 'jd', 'workflow verify failed');
  const annotations = await post('/api/review/annotations/check', { form_id: formId, task_id: task.task.id, token }, 'success');
  check(annotations.data.passed === true, 'empty task annotation gate failed');
  await post('/api/review/cache/preload', { ...fixture('cache_preload'), token }, 0);
  // Only the fresh form minted by this run can be deleted; no prompt or external id.
  const deleted = await post('/api/review/delete', { ...fixture('review_delete'), token, form_ids: [formId] });
  check(deleted.results.length === 1 && deleted.results[0].success, 'delete failed');
  const after = await post('/api/review/workflow/sync', { ...fixture('workflow_sync_query'), token, form_id: formId });
  check(after.data.task_created === false && after.data.form_status === 'deleted', 'soft delete not visible');
  console.log(`PASS ${count} assertions; form=${formId}`);
} catch (error) {
  console.error(redact(error.stack));
  process.exitCode = 1;
}
