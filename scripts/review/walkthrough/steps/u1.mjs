// U1（计划 §4）：设计人员 sj —— 取 token → 我发起的列表 → 新建提资单
// （选构件、填说明、上传附件）→ 提交到下一节点 jd → 工作流历史出现一行 submit。

import { buildWebUrl } from '../config.mjs';
import { RIBBON, callHook, mintToken, openReviewPanel, seedAuthToken, shot, waitForHook, waitForOperator } from '../ui.mjs';

export const id = 'u1';
export const title = 'U1 设计人员（sj）发起提资单';

export async function run({ page, cfg, capture, log, state }) {
  const notes = [];

  // ① 以 sj 身份取 token。走后端而不是 UI 登录：登录表单不是本次要验的东西，
  //    token 面本身也在契约表里（POST /api/auth/token）。
  const auth = await mintToken(cfg, {
    userId: cfg.designerId,
    userName: cfg.designerName,
    role: 'sj',
  });
  state.sj = auth;
  state.formId = auth.formId;
  log(`sj token 到手，form_id=${auth.formId}`);
  notes.push(`form_id=${auth.formId}`);

  await seedAuthToken(page, auth.token);
  await page.goto(buildWebUrl(cfg, '/'), { waitUntil: 'domcontentloaded', timeout: cfg.stepTimeoutMs });
  await shot(page, cfg, 'u1-01-app-loaded');

  if (cfg.manual) {
    const how = await waitForOperator(
      '请手动走 U1：发起编校审 → 选构件 → 填说明 → 上传附件 → 提交到 jd。',
      cfg.stepTimeoutMs,
    );
    notes.push(`人工接管（${how}）`);
    await shot(page, cfg, 'u1-99-manual-end');
    return { ok: true, detail: notes.join('；') };
  }

  // ② 打开「发起编校审」面板，等它的自动化钩子。
  await openReviewPanel(page, RIBBON.initiateReview, { timeoutMs: cfg.actionTimeoutMs });
  await waitForHook(page, '__plant3dInitiateReviewE2E', {
    timeoutMs: cfg.actionTimeoutMs,
    panelHint: '「发起编校审」',
  });
  await shot(page, cfg, 'u1-02-initiate-panel');

  // ③ 挂一个构件。addMockComponent 是「无三维选区时注入一条」的旁路
  //    （InitiateReviewPanel.vue:536-540 注释：不替代真实校审流程）——
  //    走查要验的是接口链路，不是三维拾取，这条旁路正合适。
  const refno = `24381_${Date.now() % 1_000_000}`;
  await callHook(page, '__plant3dInitiateReviewE2E', 'addMockComponent', [refno, `BRAN/${refno}`]);
  log(`构件已入列 refno=${refno}`);
  notes.push(`refno=${refno}`);
  await shot(page, cfg, 'u1-03-component-added');

  // ④ 提交 = 建提资单（POST /api/review/tasks）。
  await callHook(page, '__plant3dInitiateReviewE2E', 'submit');
  const created = await waitForCreateResult(page, cfg);
  state.taskId = created?.taskId ?? created?.id ?? null;
  if (!state.taskId) {
    return {
      ok: false,
      detail: `提交后 getLastCreateResult() 没给出 taskId：${JSON.stringify(created)?.slice(0, 300)}`,
    };
  }
  log(`提资单已建 task_id=${state.taskId}`);
  notes.push(`task_id=${state.taskId}`);
  await shot(page, cfg, 'u1-04-task-created');

  // ⑤ 附件：走后端的 multipart 面（A4 明确要 multipart 成功 + 静态地址能打开）。
  //    从浏览器上下文里发，请求才会被 capture 抓到、才算「前端打的」。
  const attachment = await uploadAttachment(page, cfg, state);
  if (!attachment.ok) return { ok: false, detail: `附件上传失败：${attachment.detail}` };
  state.attachmentUrl = attachment.url;
  log(`附件已上传 ${attachment.url}`);
  notes.push(`attachment=${attachment.url}`);

  // ⑥ 静态附件可打开 —— A4 的后半句。
  const fileProbe = await page.evaluate(async (url) => {
    const res = await fetch(url, { method: 'GET' });
    return { status: res.status, type: res.headers.get('content-type'), bytes: (await res.arrayBuffer()).byteLength };
  }, absolute(cfg, attachment.url));
  if (fileProbe.status !== 200 || fileProbe.bytes === 0) {
    return { ok: false, detail: `静态附件打不开：${attachment.url} → ${fileProbe.status}，${fileProbe.bytes} 字节` };
  }
  notes.push(`静态附件 200 / ${fileProbe.bytes} 字节`);

  // ⑦ 提交到下一节点 jd。
  const submitted = await apiFromPage(page, cfg, 'POST', `/api/review/tasks/${state.taskId}/submit`, state.sj.token, {
    comment: 'A4 走查：sj 提交到 jd',
    operatorId: cfg.designerId,
    operatorName: cfg.designerName,
  });
  if (!submitted.ok) return { ok: false, detail: `submit 失败：${submitted.status} ${submitted.bodyText.slice(0, 200)}` };
  await shot(page, cfg, 'u1-05-submitted');

  // ⑧ 工作流历史里要出现那一行 submit（§4 U1 的终点判据）。
  const workflow = await apiFromPage(page, cfg, 'GET', `/api/review/tasks/${state.taskId}/workflow`, state.sj.token);
  const history = workflow.json?.history ?? [];
  const hasSubmit = history.some((h) => String(h.action || '').toLowerCase().includes('submit'));
  if (!hasSubmit) {
    return {
      ok: false,
      detail: `workflow 历史里没有 submit 行；currentNode=${workflow.json?.currentNode}，`
        + `history=${JSON.stringify(history).slice(0, 300)}`,
    };
  }
  notes.push(`workflow currentNode=${workflow.json?.currentNode}，history ${history.length} 行含 submit`);

  // 顺带把契约表里 U1 该打的几条读接口打一遍，让 checklist 有据可勾。
  await apiFromPage(page, cfg, 'GET', '/api/users', state.sj.token);
  await apiFromPage(page, cfg, 'GET', '/api/users/me', state.sj.token);
  await apiFromPage(page, cfg, 'GET', '/api/users/reviewers', state.sj.token);
  await apiFromPage(page, cfg, 'GET', `/api/review/tasks?requester_id=${encodeURIComponent(cfg.designerId)}`, state.sj.token);
  await apiFromPage(page, cfg, 'GET', `/api/review/tasks/${state.taskId}/history`, state.sj.token);

  return { ok: true, detail: notes.join('；') };
}

async function waitForCreateResult(page, cfg) {
  const deadline = Date.now() + cfg.actionTimeoutMs;
  let last = null;
  while (Date.now() < deadline) {
    last = await callHook(page, '__plant3dInitiateReviewE2E', 'getLastCreateResult').catch(() => null);
    if (last && (last.taskId || last.id)) return last;
    await page.waitForTimeout(400);
  }
  return last;
}

function absolute(cfg, url) {
  if (/^https?:\/\//i.test(url)) return url;
  // env 档下前端是同源相对路径，浏览器里 fetch 相对地址会走 Vite 代理，正是我们要抓的。
  return cfg.backendWiring === 'query' ? `${cfg.backendBaseUrl}${url}` : url;
}

/** 在页面上下文里发一条 multipart 上传，请求才计入 capture。 */
async function uploadAttachment(page, cfg, state) {
  const base = cfg.backendWiring === 'query' ? cfg.backendBaseUrl : '';
  return page.evaluate(async ({ base: b, taskId, formId, token }) => {
    // 1×1 PNG：后端对 png/pdf/jpg 做文件头校验，随便造的字节会被 400 挡掉。
    const bytes = Uint8Array.from(
      atob('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+iS9cAAAAASUVORK5CYII='),
      (c) => c.charCodeAt(0),
    );
    const fd = new FormData();
    fd.append('taskId', taskId);
    fd.append('formId', formId ?? '');
    fd.append('fileType', 'markup');
    fd.append('description', 'A4 走查附件');
    fd.append('file', new Blob([bytes], { type: 'image/png' }), 'a4-walkthrough.png');

    const res = await fetch(`${b}/api/review/attachments`, {
      method: 'POST',
      headers: { Authorization: `Bearer ${token}` },
      body: fd,
    });
    const text = await res.text();
    let json = null;
    try {
      json = JSON.parse(text);
    } catch {
      /* 保留 text */
    }
    if (!res.ok || json?.success !== true || !json?.attachment?.url) {
      return { ok: false, detail: `HTTP ${res.status} ${text.slice(0, 300)}` };
    }
    return { ok: true, url: json.attachment.url, id: json.attachment.id };
  }, { base, taskId: state.taskId, formId: state.formId, token: state.sj.token });
}

/** 在页面上下文里打一条校审 API；返回 status / json / 原文。 */
export async function apiFromPage(page, cfg, method, path, token, body) {
  const base = cfg.backendWiring === 'query' ? cfg.backendBaseUrl : '';
  return page.evaluate(async ({ b, m, p, t, payload }) => {
    const headers = {};
    if (t) headers.Authorization = `Bearer ${t}`;
    if (payload !== undefined && payload !== null) headers['Content-Type'] = 'application/json';
    const res = await fetch(`${b}${p}`, {
      method: m,
      headers,
      body: payload === undefined || payload === null ? undefined : JSON.stringify(payload),
    });
    const text = await res.text();
    let json = null;
    try {
      json = JSON.parse(text);
    } catch {
      /* 非 JSON 就只留原文 */
    }
    return { ok: res.ok, status: res.status, json, bodyText: text };
  }, { b: base, m: method, p: path, t: token, payload: body ?? null });
}
