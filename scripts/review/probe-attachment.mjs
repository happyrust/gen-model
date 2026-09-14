// Static registration is proven with a real uploaded file, never a missing-file 404.
import assert from 'node:assert/strict';
const base = process.argv[2];
const headers = { Authorization: `Bearer ${process.env.REVIEW_TOKEN}` };
let taskId;
try {
  const res = await fetch(`${base}/api/review/tasks`, {
    method: 'POST', headers: { ...headers, 'Content-Type': 'application/json' },
    body: JSON.stringify({ title: 'Static route probe', modelName: 'route-acceptance' }), signal: AbortSignal.timeout(20000),
  });
  assert.equal(res.status, 200);
  taskId = (await res.json()).task.id;
  const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+iS9cAAAAASUVORK5CYII=', 'base64');
  const form = new FormData();
  form.set('taskId', taskId); form.set('fileType', 'markup');
  form.set('file', new Blob([png], { type: 'image/png' }), 'route.png');
  const upload = await fetch(`${base}/api/review/attachments`, { method: 'POST', headers, body: form, signal: AbortSignal.timeout(20000) });
  assert.equal(upload.status, 200);
  const attachment = (await upload.json()).attachment;
  const download = await fetch(new URL(attachment.url, base), { signal: AbortSignal.timeout(20000) });
  assert.equal(download.status, 200);
  assert.deepEqual(Buffer.from(await download.arrayBuffer()), png);
  const removed = await fetch(`${base}/api/review/attachments/${attachment.id}`, { method: 'DELETE', headers, signal: AbortSignal.timeout(20000) });
  assert.equal(removed.status, 200);
  console.log('PASS: uploaded static file HTTP 200 and bytes identical');
} catch (e) {
  console.error(e.message); process.exitCode = 1;
} finally {
  if (taskId) await fetch(`${base}/api/review/tasks/${taskId}`, { method: 'DELETE', headers, signal: AbortSignal.timeout(20000) });
}
