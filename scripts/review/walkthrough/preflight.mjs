// 第一步：不依赖任何 npm 包，只用 Node 自带的 fetch 探活。
//
// 这一步存在的唯一理由是「不许挂死」：后端没起时，走查必须在几秒内给出一句
// 能照着做的失败，而不是让 Playwright 打开浏览器、在某个 waitFor 上耗到总超时。
// 因此 run.mjs 在 preflight 通过之前不会 import playwright——没装依赖也跑得动。

import { CONTRACT_ROUTES } from './contract.mjs';

/** @typedef {{ name: string, ok: boolean, detail: string, fatal: boolean }} Check */

async function probe(url, { timeoutMs, method = 'GET' }) {
  const ac = new AbortController();
  const timer = setTimeout(() => ac.abort(), timeoutMs);
  const startedAt = Date.now();
  try {
    const res = await fetch(url, { method, signal: ac.signal, redirect: 'manual' });
    return { reached: true, status: res.status, elapsedMs: Date.now() - startedAt, res };
  } catch (err) {
    return {
      reached: false,
      status: null,
      elapsedMs: Date.now() - startedAt,
      error: ac.signal.aborted ? `超时 ${timeoutMs}ms` : String(err?.cause?.code || err?.message || err),
    };
  } finally {
    clearTimeout(timer);
  }
}

/**
 * @param {object} cfg loadConfig() 的结果
 * @returns {Promise<{ ok: boolean, checks: Check[] }>}
 */
export async function preflight(cfg) {
  /** @type {Check[]} */
  const checks = [];
  const healthUrl = `${cfg.backendBaseUrl}/api/review/health`;

  // ① 后端可达 —— 不通就没有任何后续动作值得做。
  const health = await probe(healthUrl, { timeoutMs: cfg.probeTimeoutMs });
  if (!health.reached) {
    checks.push({
      name: 'backend-reachable',
      ok: false,
      fatal: true,
      detail:
        `后端不可达：${healthUrl}（${health.error}，耗时 ${health.elapsedMs}ms）。\n`
        + `  先把 gen-model 起到 ${cfg.backendBaseUrl}：在 worktree 根目录\n`
        + '    . .\\scripts\\review\\env.ps1\n'
        + '    cargo run --features http_api\n'
        + `  确认 curl ${healthUrl} 返回 200 之后再跑本走查。\n`
        + '  （只想换个地址：REVIEW_BACKEND_URL=http://host:port）',
    });
    return { ok: false, checks };
  }
  if (health.status !== 200) {
    checks.push({
      name: 'backend-reachable',
      ok: false,
      fatal: true,
      detail:
        `后端在听但 ${healthUrl} 回 ${health.status}（期望 200）。\n`
        + '  计划 §6 A2 要求 health 200 且键集与旧端 :3100 相同；先修后端再走查。',
    });
    return { ok: false, checks };
  }

  let healthBody = null;
  try {
    healthBody = await health.res.json();
  } catch {
    healthBody = null;
  }
  checks.push({
    name: 'backend-reachable',
    ok: true,
    fatal: false,
    detail: `${healthUrl} → 200（${health.elapsedMs}ms）${healthBody ? ` keys=[${Object.keys(healthBody).join(',')}]` : ''}`,
  });

  // ② 校审域是真的挂上了没有 —— health 通不代表 review 路由都在。
  //    取契约表里两条无副作用的 GET 试打；404 = 路由没注册（前端会静默失效）。
  const canaries = [
    { method: 'GET', path: '/api/users/me' },
    { method: 'GET', path: '/api/review/tasks' },
  ];
  for (const c of canaries) {
    const r = await probe(`${cfg.backendBaseUrl}${c.path}`, { timeoutMs: cfg.probeTimeoutMs, method: c.method });
    const ok = r.reached && r.status !== 404 && r.status !== 405;
    checks.push({
      name: `route-registered ${c.method} ${c.path}`,
      ok,
      fatal: !ok,
      detail: ok
        ? `${r.status}（401/403 也算在册：鉴权在起作用，路由不是 404）`
        : `${c.method} ${c.path} → ${r.reached ? r.status : r.error}；路由没注册，前端会静默 404。`
          + '先跑 cargo test --features http_api --lib web_service::review 看护栏。',
    });
    if (!ok) return { ok: false, checks };
  }

  // ③ 前端仓在不在 —— 起 dev server 要用它。
  checks.push({
    name: 'web-repo',
    ok: cfg.webRepoExists,
    fatal: !cfg.webRepoExists && cfg.startWeb !== 'never',
    detail: cfg.webRepoExists
      ? `${cfg.webRepo}（只读，计划 N4：不改它任何 tracked 文件）`
      : `找不到前端仓 ${cfg.webRepo}；用 REVIEW_WEB_REPO 指对，或 REVIEW_START_WEB=never 自己先把 dev server 起好`,
  });

  // ④ dev server 在不在听 —— 决定要不要自己起。
  const web = await probe(cfg.webBaseUrl, { timeoutMs: cfg.probeTimeoutMs });
  const webUp = web.reached;
  let webFatal = false;
  let webDetail;
  if (webUp && cfg.startWeb === 'always') {
    webFatal = true;
    webDetail = `${cfg.webBaseUrl} 已被占用，而 REVIEW_START_WEB=always 要求自己起（vite strictPort=true 会直接失败）。`
      + '改用 REVIEW_START_WEB=auto，或先停掉占用进程。';
  } else if (!webUp && cfg.startWeb === 'never') {
    webFatal = true;
    webDetail = `${cfg.webBaseUrl} 没在听，而 REVIEW_START_WEB=never 不允许自己起。`
      + `先在 ${cfg.webRepo} 跑 npm run dev，或改 REVIEW_START_WEB=auto。`;
  } else {
    webDetail = webUp
      ? `${cfg.webBaseUrl} 已在听（${web.status}）——沿用现成的 dev server。`
        + '注意：它的 VITE_REVIEW_ALLOW_MOCK_FALLBACK 是否为 false 本脚本无法从外部确认，'
        + '控制台采集会兜住这一点（见 README §5）。'
      : `${cfg.webBaseUrl} 没在听，走查会自己起 dev server 并在结束时 kill。`;
  }
  checks.push({ name: 'web-dev-server', ok: !webFatal, fatal: webFatal, detail: webDetail });
  if (webFatal) return { ok: false, checks };

  // ⑤ U3 的接线约束 —— 嵌入地址由后端现拼，带不上 ?backendPort=。
  if (cfg.steps.includes('u3') && cfg.backendWiring === 'query') {
    checks.push({
      name: 'u3-wiring',
      ok: false,
      fatal: true,
      detail:
        'U3（PMS 嵌入）不能用 REVIEW_BACKEND_WIRING=query：嵌入地址是后端 embed-url 现拼的，'
        + '带不上 ?backendPort=，嵌入页会按 env 解析后端。\n'
        + '  改用 REVIEW_BACKEND_WIRING=env，并确认 DbOption.toml 的\n'
        + `    [platform_auth] review_web_base_url = "${cfg.webBaseUrl}"\n`
        + '  （计划 Q5：这一项顶替了旧端的站点注册表 managed_project_sites::get_site）',
    });
    return { ok: false, checks };
  }

  // ⑥ 契约表自检 —— 表本身有重复/写错，走查勾出来的结论就不可信。
  const seen = new Set();
  const dupes = [];
  for (const r of CONTRACT_ROUTES) {
    const key = `${r.method} ${r.path}`;
    if (seen.has(key)) dupes.push(key);
    seen.add(key);
  }
  checks.push({
    name: 'contract-table',
    ok: dupes.length === 0,
    fatal: dupes.length > 0,
    detail: dupes.length ? `contract.mjs 有重复条目：${dupes.join('、')}` : `${CONTRACT_ROUTES.length} 条，无重复`,
  });

  return { ok: checks.every((c) => !c.fatal), checks };
}

export function formatChecks(checks) {
  return checks
    .map((c) => `${c.ok ? 'PASS' : c.fatal ? 'FAIL' : 'WARN'}  ${c.name}\n      ${c.detail.split('\n').join('\n      ')}`)
    .join('\n');
}
