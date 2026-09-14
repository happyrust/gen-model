// 计划 §1.2 契约表的可执行副本：走查时按它逐条勾。
//
// 真源是后端的源码文本护栏 `src/web_service/review/common.rs::REVIEW_CONTRACT_ROUTES`
// （37 条方法·路径）与 `review/platform/mod.rs::route_guard`（PMS 六条）、
// `review/integration.rs`（模型侧两条）。本文件把那三处逐字抄成 JS，
// 并由 `contract.test.mjs` 反向核对，改了后端忘了改这里会红。
//
// 每条的 `walkthrough` 字段标「哪一步预期会打到它」：
//   u1 / u2 / u3 = 计划 §4 的三条用户路径；
//   optional     = 契约在册但本次走查不必然触发（前端未调用或需要额外操作）。

/** @typedef {{ method: string, path: string, group: string, walkthrough: string[], note?: string }} ContractRoute */

/** @type {ContractRoute[]} */
export const CONTRACT_ROUTES = [
  // ── 鉴权（reviewApi.ts:2356-2408；common.rs auth.rs 两条）────────────────
  { method: 'POST', path: '/api/auth/token', group: 'auth', walkthrough: ['u1', 'u2'] },
  { method: 'POST', path: '/api/auth/verify', group: 'auth', walkthrough: ['u3'] },

  // ── 用户（reviewApi.ts:1775-1801）──────────────────────────────────────
  { method: 'GET', path: '/api/users', group: 'users', walkthrough: ['u1'] },
  { method: 'GET', path: '/api/users/me', group: 'users', walkthrough: ['u1', 'u2'] },
  { method: 'GET', path: '/api/users/reviewers', group: 'users', walkthrough: ['u1'] },

  // ── 提资单 CRUD（reviewApi.ts:1008-1091）──────────────────────────────
  { method: 'POST', path: '/api/review/tasks', group: 'tasks', walkthrough: ['u1'] },
  { method: 'GET', path: '/api/review/tasks', group: 'tasks', walkthrough: ['u1', 'u2'] },
  { method: 'GET', path: '/api/review/tasks/{id}', group: 'tasks', walkthrough: ['u2'] },
  { method: 'PATCH', path: '/api/review/tasks/{id}', group: 'tasks', walkthrough: ['u1'] },
  { method: 'DELETE', path: '/api/review/tasks/{id}', group: 'tasks', walkthrough: ['optional'] },

  // ── 审核操作（reviewApi.ts:1103-1468）─────────────────────────────────
  { method: 'POST', path: '/api/review/tasks/{id}/start-review', group: 'tasks', walkthrough: ['optional'] },
  { method: 'POST', path: '/api/review/tasks/{id}/approve', group: 'tasks', walkthrough: ['u2'] },
  { method: 'POST', path: '/api/review/tasks/{id}/reject', group: 'tasks', walkthrough: ['optional'] },
  { method: 'POST', path: '/api/review/tasks/{id}/cancel', group: 'tasks', walkthrough: ['optional'] },
  { method: 'GET', path: '/api/review/tasks/{id}/history', group: 'tasks', walkthrough: ['u1', 'u2'] },

  // ── 多级审批流转（reviewApi.ts:1167-1202）─────────────────────────────
  { method: 'POST', path: '/api/review/tasks/{id}/submit', group: 'tasks', walkthrough: ['u1'] },
  { method: 'POST', path: '/api/review/tasks/{id}/return', group: 'tasks', walkthrough: ['u2'], note: 'approve 与 return 二选一，见 REVIEW_U2_DECISION' },
  { method: 'GET', path: '/api/review/tasks/{id}/workflow', group: 'tasks', walkthrough: ['u1', 'u2'] },

  // ── 驳回重流转（旧端 review_api.rs:1733，前端当前未调用）──────────────
  { method: 'GET', path: '/api/review/tasks/returned', group: 'tasks', walkthrough: ['optional'], note: '旧端注册着，前端未调用；移植不减项' },
  { method: 'POST', path: '/api/review/tasks/batch-reactivate', group: 'tasks', walkthrough: ['optional'], note: '同上' },

  // ── 批注记录（reviewApi.ts:1410-1455）─────────────────────────────────
  { method: 'POST', path: '/api/review/records', group: 'records', walkthrough: ['u2'] },
  { method: 'GET', path: '/api/review/records/by-task/{task_id}', group: 'records', walkthrough: ['u2'] },
  { method: 'DELETE', path: '/api/review/records/item/{record_id}', group: 'records', walkthrough: ['u4'] },
  { method: 'DELETE', path: '/api/review/records/clear-task/{task_id}', group: 'records', walkthrough: ['u4'] },

  // ── 评论（reviewApi.ts:1493-1556）─────────────────────────────────────
  { method: 'POST', path: '/api/review/comments', group: 'comments', walkthrough: ['u2'] },
  { method: 'GET', path: '/api/review/comments/by-annotation/{annotation_id}', group: 'comments', walkthrough: ['u2'] },
  { method: 'DELETE', path: '/api/review/comments/item/{comment_id}', group: 'comments', walkthrough: ['u4'] },
  { method: 'PATCH', path: '/api/review/comments/item/{comment_id}', group: 'comments', walkthrough: ['u4'] },

  // ── 批注轻量字段与严重度（reviewApi.ts:1585-1637）─────────────────────
  { method: 'PATCH', path: '/api/review/annotations/{annotation_id}', group: 'annotations', walkthrough: ['u2'] },
  { method: 'PATCH', path: '/api/review/annotations/{annotation_id}/severity', group: 'annotations', walkthrough: ['u2'] },

  // ── 批注处理状态（reviewApi.ts:1747-1760）─────────────────────────────
  { method: 'POST', path: '/api/review/annotation-states/apply', group: 'annotation-states', walkthrough: ['u2'] },
  { method: 'GET', path: '/api/review/annotation-states', group: 'annotation-states', walkthrough: ['u2'] },

  // ── 附件（reviewApi.ts:1927-2084）─────────────────────────────────────
  { method: 'POST', path: '/api/review/attachments', group: 'attachments', walkthrough: ['u1', 'u2'], note: 'multipart，50MB 上限' },
  { method: 'DELETE', path: '/api/review/attachments/{attachment_id}', group: 'attachments', walkthrough: ['optional'] },

  // ── 同步（reviewApi.ts:2497-2510）─────────────────────────────────────
  { method: 'POST', path: '/api/review/sync/export', group: 'sync', walkthrough: ['optional'] },
  { method: 'POST', path: '/api/review/sync/import', group: 'sync', walkthrough: ['optional'] },

  // ── 健康（旧端 review_api.rs:1752）────────────────────────────────────
  { method: 'GET', path: '/api/review/health', group: 'health', walkthrough: ['u1', 'u2', 'u3'] },

  // ── 静态附件（web_service 总入口 ServeDir，不在 REVIEW_CONTRACT_ROUTES 里）──
  { method: 'GET', path: '/files/review_attachments/{stored_name}', group: 'files', walkthrough: ['u1', 'u2'], note: 'A4 要求「上传后能打开」' },

  // ── PMS 平台入站六条（platform/mod.rs::route_guard，S2S）───────────────
  { method: 'POST', path: '/api/review/embed-url', group: 'platform', walkthrough: ['u3'] },
  { method: 'POST', path: '/api/review/workflow/sync', group: 'platform', walkthrough: ['u3'] },
  { method: 'POST', path: '/api/review/workflow/verify', group: 'platform', walkthrough: ['u3'] },
  { method: 'POST', path: '/api/review/annotations/check', group: 'platform', walkthrough: ['u3'] },
  { method: 'POST', path: '/api/review/cache/preload', group: 'platform', walkthrough: ['optional'] },
  { method: 'POST', path: '/api/review/delete', group: 'platform', walkthrough: ['u3'], note: '走查收尾软删' },

  // ── 模型侧集成（integration.rs，计划 §1.2「模型侧集成」行）─────────────
  { method: 'POST', path: '/api/review/aux-data', group: 'integration', walkthrough: ['optional'], note: 'T4；UCode/UKey 口令面，不走 review_auth' },
  { method: 'GET', path: '/api/review/collision-data', group: 'integration', walkthrough: ['optional'], note: 'T4；无数据源时返回 reason' },
];

/** 计划 §1.2 里「实时」一行：旧端从未实现，前端未配 WS 基址时自动关闭（N1 / Q9）。 */
export const NON_GOAL_PATHS = ['/ws/review', '/ws/review/user/{userId}'];

/** 契约路径的前缀集合：判一条请求「是不是校审域的」。 */
const CONTRACT_PREFIXES = ['/api/review/', '/api/users', '/api/auth/', '/files/review_attachments/'];

export function isReviewDomainPath(pathname) {
  if (pathname === '/api/users' || pathname === '/api/review/health') return true;
  return CONTRACT_PREFIXES.some((p) => pathname.startsWith(p));
}

/** `/api/review/tasks/{id}/approve` → 匹配 `/api/review/tasks/abc123/approve` 的正则。 */
function toRegExp(template) {
  const escaped = template
    .split('/')
    .map((seg) => (seg.startsWith('{') && seg.endsWith('}') ? '[^/]+' : seg.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')))
    .join('/');
  return new RegExp(`^${escaped}$`);
}

const COMPILED = CONTRACT_ROUTES.map((route) => ({ ...route, re: toRegExp(route.path) }));

/**
 * 把一条实际请求匹配回契约表。
 *
 * 具体路径优先于带占位符的：`/api/review/tasks/returned` 必须匹配到它自己那条，
 * 而不是 `/api/review/tasks/{id}`——两条都能匹配上时按「占位符更少」取胜。
 */
export function matchContractRoute(method, pathname) {
  const upper = String(method || '').toUpperCase();
  const hits = COMPILED.filter((r) => r.method === upper && r.re.test(pathname));
  if (hits.length === 0) return null;
  const placeholders = (p) => (p.match(/\{/g) || []).length;
  return hits.sort((a, b) => placeholders(a.path) - placeholders(b.path))[0];
}

/** 某一步（u1/u2/u3）预期会打到的契约条目。 */
export function expectedRoutesFor(step, decision = 'approve') {
  return CONTRACT_ROUTES.filter((r) => r.walkthrough.includes(step)
    && !(step === 'u2' && r.path.endsWith(decision === 'approve' ? '/return' : '/approve'))
    // Token minting and health preflight run in Node, outside browser capture.
    && r.path !== '/api/auth/token' && r.path !== '/api/review/health');
}
