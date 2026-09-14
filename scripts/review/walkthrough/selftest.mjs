#!/usr/bin/env node
// 离线自检：不碰后端、不开浏览器、不写产物，只验两件走查结论所依赖的纯逻辑——
//   ① 契约匹配器把实际请求勾回 §1.2 表时不会勾错（具体路径要压过占位符路径）；
//   ② 控制台分类器能把 fallback 告警和普通日志分开（A4 的判据就建在这上面）。
// 这两处错了，走查报告会「看起来全绿」，属宪法 III 说的静默失效。
//
// 跑：node selftest.mjs   （退出码 0 全过 / 1 有失败）

import { CONTRACT_ROUTES, isReviewDomainPath, matchContractRoute, expectedRoutesFor } from './contract.mjs';
import { classifyConsoleText, isFallbackClass, redact, Capture } from './capture.mjs';
import { loadConfig } from './config.mjs';

let passed = 0;
const failures = [];

function check(name, actual, expected) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a === e) passed += 1;
  else failures.push(`${name}\n    期望 ${e}\n    实际 ${a}`);
}

function throws(name, fn, patternn) {
  try {
    fn();
    failures.push(`${name}\n    期望抛错，实际没抛`);
  } catch (err) {
    if (patternn.test(String(err.message))) passed += 1;
    else failures.push(`${name}\n    抛的错对不上 ${patternn}：${err.message}`);
  }
}

// ── ① 契约匹配 ────────────────────────────────────────────────────────
// 具体路径压过占位符：`/tasks/returned` 不能被 `/tasks/{id}` 抢走，否则
// 报告会把「列表接口通了」错记成「详情接口通了」。
check('GET /tasks/returned 不被 {id} 抢走',
  matchContractRoute('GET', '/api/review/tasks/returned')?.path,
  '/api/review/tasks/returned');
check('GET /tasks/abc123 落到 {id}',
  matchContractRoute('GET', '/api/review/tasks/abc123')?.path,
  '/api/review/tasks/{id}');
check('POST /tasks/batch-reactivate 不被 {id} 抢走',
  matchContractRoute('POST', '/api/review/tasks/batch-reactivate')?.path,
  '/api/review/tasks/batch-reactivate');
check('POST /tasks/xyz/approve 落到 {id}/approve',
  matchContractRoute('POST', '/api/review/tasks/xyz/approve')?.path,
  '/api/review/tasks/{id}/approve');
check('方法要分：GET /api/review/tasks 与 POST 同路径是两条',
  [matchContractRoute('GET', '/api/review/tasks')?.method, matchContractRoute('POST', '/api/review/tasks')?.method],
  ['GET', 'POST']);
check('同一路由挂两个方法：PATCH /comments/item/{id} 在册',
  matchContractRoute('PATCH', '/api/review/comments/item/c-1')?.path,
  '/api/review/comments/item/{comment_id}');
check('契约外的路径不匹配',
  matchContractRoute('GET', '/api/review/not-a-real-endpoint'),
  null);
check('PMS 六条在册',
  ['/api/review/embed-url', '/api/review/workflow/sync', '/api/review/workflow/verify',
    '/api/review/annotations/check', '/api/review/cache/preload', '/api/review/delete']
    .map((p) => matchContractRoute('POST', p)?.group),
  ['platform', 'platform', 'platform', 'platform', 'platform', 'platform']);
check('静态附件在册',
  matchContractRoute('GET', '/files/review_attachments/abc-123.png')?.group,
  'files');
// `/api/review/annotations/check` 是 PMS 那条，不能被 `/annotations/{id}` 吃掉。
check('annotations/check 不被 annotations/{id} 吃掉',
  matchContractRoute('POST', '/api/review/annotations/check')?.path,
  '/api/review/annotations/check');
check('PATCH /annotations/{id}/severity 落对',
  matchContractRoute('PATCH', '/api/review/annotations/a-1/severity')?.path,
  '/api/review/annotations/{annotation_id}/severity');

check('校审域路径识别：/api/users', isReviewDomainPath('/api/users'), true);
check('校审域路径识别：/api/review/health', isReviewDomainPath('/api/review/health'), true);
check('校审域路径识别：/files/review_attachments/x.png', isReviewDomainPath('/files/review_attachments/x.png'), true);
check('非校审域：/api/v1/tree/roots', isReviewDomainPath('/api/v1/tree/roots'), false);
check('非校审域：/api/projects', isReviewDomainPath('/api/projects'), false);

const dupes = CONTRACT_ROUTES.map((r) => `${r.method} ${r.path}`)
  .filter((k, i, all) => all.indexOf(k) !== i);
check('契约表无重复条目', dupes, []);

// ── ② 控制台分类 ──────────────────────────────────────────────────────
// 文本逐字取自 plant3d-web 源码；前端改了措辞这里就该红，而不是悄悄漏判。
check('mock 真的用上了 → mock-used（useUserStore.ts:691）',
  classifyConsoleText('[useUserStore] Failed to load users, using development mock data: TypeError'),
  'mock-used');
check('reviewers 那条同样是 mock-used（:720）',
  classifyConsoleText('[useUserStore] Failed to load reviewers, using development mock data: Error'),
  'mock-used');
check('兜底关着但后端没答上 → mock-disabled（:694）',
  classifyConsoleText('[useUserStore] Failed to load users; production mock fallback is disabled: TypeError'),
  'mock-disabled');
check('本地模拟模式被拒 → mock-disabled（:1542）',
  classifyConsoleText('[useUserStore] Local review mock mode is disabled in production'),
  'mock-disabled');
check('影子比对 → shadow，不判 A4（reviewSnapshotService.ts:96）',
  classifyConsoleText('[review/M2 SHADOW] task_records build threw, fallback to legacy Error'),
  'shadow');
check('普通日志 → plain',
  classifyConsoleText('[vite] hmr update /src/App.vue'),
  'plain');
check('带 fallback 字样的无关日志不误判',
  classifyConsoleText('[show_refno] visible-insts 查询失败，回退直接加载'),
  'plain');
check('只有两类参与 A4 判定',
  ['mock-used', 'mock-disabled', 'shadow', 'plain'].map(isFallbackClass),
  [true, true, false, false]);

// ── ③ 配置校验 ────────────────────────────────────────────────────────
throws('单步超时不小于总超时要报错', () => {
  process.env.REVIEW_STEP_TIMEOUT_MS = '600000';
  process.env.REVIEW_TOTAL_TIMEOUT_MS = '600000';
  try {
    loadConfig([]);
  } finally {
    delete process.env.REVIEW_STEP_TIMEOUT_MS;
    delete process.env.REVIEW_TOTAL_TIMEOUT_MS;
  }
}, /总超时就没有意义/);

throws('未知步骤要报错', () => loadConfig(['--steps=u1,u9']), /未知步骤/);
throws('非法 wiring 要报错', () => {
  process.env.REVIEW_BACKEND_WIRING = 'proxy';
  try {
    loadConfig([]);
  } finally {
    delete process.env.REVIEW_BACKEND_WIRING;
  }
}, /只能是 query\|env/);

// ── 汇总 ──────────────────────────────────────────────────────────────
check('approve 场景不要求 return', expectedRoutesFor('u2', 'approve').some(r => r.path.endsWith('/return')), false);
check('return 场景不要求 approve', expectedRoutesFor('u2', 'return').some(r => r.path.endsWith('/approve')), false);
check('脱敏 URL token', redact('http://example/?user_token=private&form_id=f'), 'http://example/?user_token=<REDACTED>&form_id=f');
check('脱敏 JSON token', redact('{"token":"private"}'), '{"token":"<REDACTED>"}');
const c = new Capture({ backendBaseUrl: 'http://localhost:8122', webBaseUrl: 'http://localhost:3102' });
let listeners = 0;
const fakePage = { on() { listeners++; } };
c.attach(fakePage, 'main'); c.attach(fakePage, 'duplicate');
check('同一页面只挂一套监听', listeners, 4);
console.log(`selftest: ${passed} passed, ${failures.length} failed`);
if (failures.length) {
  for (const f of failures) console.error(`  FAIL ${f}`);
  process.exit(1);
}
process.exit(0);
