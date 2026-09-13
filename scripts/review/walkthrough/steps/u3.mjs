// U3（计划 §4）：PMS 平台 S2S —— embed-url 换带 user_token 的嵌入地址 →
// 浏览器以 embed 模式打开 → workflow/sync 推节点 → workflow/verify 核对 →
// annotations/check 查未闭环批注 → 结束后 delete 软删。
//
// 驱动用 plant3d-web 自带的仿 PMS 页 `pms-review-simulator.html`（仓库根，
// Vite dev 直接当静态页发）：它已经把「新增 → 调 embed-url → 把返回的地址塞进
// #review-iframe」这条链走通了，plant3d-web 的 e2e/pms-review-simulator-*.spec.ts
// 就是靠它验的。选择器 #tool-create / #review-iframe / #project-id-status 出自那两个 spec。
//
// 剩下的 sync / verify / annotations/check / delete 是 S2S 面（凭据在请求体 token 里，
// 不是 header），照 PMS 的做法从页面上下文直接打。

import { shot, waitForOperator } from '../ui.mjs';

export const id = 'u3';
export const title = 'U3 PMS 平台（S2S）嵌入与流转';

const SIMULATOR_PATH = '/pms-review-simulator.html';

export async function run({ page, cfg, capture, log, state }) {
  const notes = [];
  const formId = state.formId || `FORM-A4-${Date.now().toString(36).toUpperCase()}`;
  state.u3FormId = formId;

  await page.goto(`${cfg.webBaseUrl}${SIMULATOR_PATH}`, {
    waitUntil: 'domcontentloaded',
    timeout: cfg.stepTimeoutMs,
  });
  await shot(page, cfg, 'u3-01-simulator');

  if (cfg.manual) {
    const how = await waitForOperator(
      '请手动走 U3：在仿 PMS 页点「新增」，确认嵌入 iframe 打开且不落到「选择项目」。',
      cfg.stepTimeoutMs,
    );
    notes.push(`人工接管（${how}）`);
    await shot(page, cfg, 'u3-99-manual-end');
    return { ok: true, detail: notes.join('；') };
  }

  // ① 仿 PMS 页自己要先把项目列表读出来，否则「新增」拼不出 project_id。
  const statusLocator = page.locator('#project-id-status');
  try {
    await statusLocator.waitFor({ state: 'visible', timeout: cfg.actionTimeoutMs });
    await page.waitForFunction(
      () => /项目列表.*项/.test(document.querySelector('#project-id-status')?.textContent || ''),
      undefined,
      { timeout: cfg.stepTimeoutMs },
    );
  } catch {
    const shown = await statusLocator.textContent().catch(() => null);
    return {
      ok: false,
      detail: `仿 PMS 页没能把项目列表读出来（#project-id-status = ${JSON.stringify(shown)}）。`
        + '它读的是 /api/projects——那条不在校审域契约表里，由前端代理到的后端提供；'
        + `确认 ${cfg.backendBaseUrl} 或 Vite 代理上游能答 /api/projects。`,
    };
  }
  notes.push('仿 PMS 页项目列表就绪');

  // ② 点「新增」→ 页面调 POST /api/review/embed-url → 把返回地址塞进 iframe。
  await page.locator('#tool-create').click({ timeout: cfg.actionTimeoutMs });
  const iframe = page.locator('#review-iframe');
  try {
    await iframe.waitFor({ state: 'visible', timeout: cfg.actionTimeoutMs });
    await page.waitForFunction(
      () => /user_token=/.test(document.querySelector('#review-iframe')?.getAttribute('src') || ''),
      undefined,
      { timeout: cfg.stepTimeoutMs },
    );
  } catch {
    const src = await iframe.getAttribute('src').catch(() => null);
    return {
      ok: false,
      detail: `embed-url 没能拼出带 user_token 的嵌入地址（iframe src = ${JSON.stringify(src)}）。`
        + '查 requests.jsonl 里 POST /api/review/embed-url 的状态码；'
        + '若是 500/404，多半是 [platform_auth].review_web_base_url 没配（计划 Q5）。',
    };
  }
  const iframeSrc = await iframe.getAttribute('src');
  state.embedUrl = iframeSrc;
  log(`嵌入地址：${iframeSrc}`);
  notes.push('embed-url 拼链成功（含 user_token）');
  await shot(page, cfg, 'u3-02-embed-iframe');

  // 嵌入页里的控制台也要采 —— fallback 告警可能只在 iframe 里出现。
  const embedFrame = page.frameLocator('#review-iframe');
  try {
    await embedFrame.locator('[data-testid="embed-bootstrap-loading"]').waitFor({ state: 'hidden', timeout: cfg.stepTimeoutMs });
    notes.push('嵌入页引导完成');
  } catch {
    notes.push('嵌入页引导未在超时内完成（embed-bootstrap-loading 仍可见）');
  }
  const landedOnProjectPicker = await embedFrame.getByRole('heading', { name: '选择项目' }).count().catch(() => 0);
  if (landedOnProjectPicker > 0) {
    return {
      ok: false,
      detail: '嵌入页落到了「选择项目」——embed-url 的 project 预选没生效，'
        + 'PMS 用户点进来会看到选项目而不是校审工作台。',
    };
  }
  await shot(page, cfg, 'u3-03-embed-loaded');

  // ③ 取一个 S2S token：这六条的凭据在请求体的 token 字段里，不走 review_auth。
  const s2s = await platformCall(page, cfg, '/api/auth/token', {
    project_id: cfg.projectId,
    user_id: cfg.designerId,
    user_name: cfg.designerName,
    role: 'sj',
    form_id: formId,
  });
  const token = s2s.json?.data?.token;
  if (!token) return { ok: false, detail: `取 S2S token 失败：${s2s.status} ${s2s.text.slice(0, 200)}` };

  // ④ workflow/sync 推节点。actor 由 PMS 每次显式声明（common.rs:1016-1019）。
  const sync = await platformCall(page, cfg, '/api/review/workflow/sync', {
    form_id: formId,
    token,
    action: 'query',
    actor: { id: cfg.designerId, name: cfg.designerName, roles: 'sj' },
  });
  if (sync.json?.code !== 200) {
    return { ok: false, detail: `workflow/sync 失败：HTTP ${sync.status} body ${sync.text.slice(0, 300)}` };
  }
  notes.push(`workflow/sync code=${sync.json.code}`);

  // ⑤ workflow/verify 核对。
  const verify = await platformCall(page, cfg, '/api/review/workflow/verify', {
    form_id: formId,
    token,
    action: 'active',
    actor: { id: cfg.designerId, name: cfg.designerName, roles: 'sj' },
  });
  notes.push(`workflow/verify code=${verify.json?.code ?? `HTTP ${verify.status}`}`);

  // ⑥ annotations/check 查未闭环批注。
  const check = await platformCall(page, cfg, '/api/review/annotations/check', {
    form_id: formId,
    token,
  });
  notes.push(`annotations/check code=${check.json?.code ?? `HTTP ${check.status}`}`);

  // ⑦ 收尾软删。默认开，REVIEW_CLEANUP=0 可留着数据人工看。
  if (cfg.cleanup) {
    const del = await platformCall(page, cfg, '/api/review/delete', {
      form_ids: [formId],
      operator_id: cfg.designerId,
      token,
    });
    notes.push(`delete code=${del.json?.code ?? `HTTP ${del.status}`}`);
  } else {
    notes.push('delete 跳过（REVIEW_CLEANUP=0）');
  }

  return { ok: true, detail: notes.join('；') };
}

/** PMS 六条 + auth/token 都是 {code,message,data} 信封，且从页面上下文打才计入 capture。 */
async function platformCall(page, cfg, path, body) {
  const base = cfg.backendWiring === 'query' ? cfg.backendBaseUrl : '';
  return page.evaluate(async ({ b, p, payload }) => {
    const res = await fetch(`${b}${p}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });
    const text = await res.text();
    let json = null;
    try {
      json = JSON.parse(text);
    } catch {
      /* 非 JSON 留原文 */
    }
    return { status: res.status, json, text };
  }, { b: base, p: path, payload: body });
}
