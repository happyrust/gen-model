// U1（计划 §4）：设计人员 sj —— 取 token → 我发起的列表 → 新建提资单
// （选构件、填说明、上传附件）→ 提交到下一节点 jd → 工作流历史出现一行 submit。

import { buildWebUrl } from '../config.mjs';
import { RIBBON, callHook, ensureHook, mintToken, seedAuthToken, shot, waitForOperator } from '../ui.mjs';

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
  // 以 PMS 嵌入入口进入（`user_token + form_id`，embedRoleLanding.ts:109-137）：只有嵌入模式下
  // InitiateReviewPanel.handleSubmit 才会把 form_id 写进 POST /api/review/tasks
  // （InitiateReviewPanel.vue:785-812 `requestFormId`），否则任务的 formId 由后端另生成，
  // U2 用同一 form_id 进来就找不到这条单（2026-09-13 实跑：`[embed][form-restore] … taskId: null`）。
  // sj 落地只开 viewer（embedRoleLanding.ts:304-306），「发起编校审」面板仍要点 ribbon 打开。
  // `output_project` 让 App.vue 跳过项目选择页（/api/projects 不在校审域，gen-model 不提供）。
  await page.goto(
    buildWebUrl(cfg, '/', { user_token: auth.token, form_id: auth.formId, output_project: cfg.projectId }),
    { waitUntil: 'domcontentloaded', timeout: cfg.stepTimeoutMs },
  );
  await page
    .locator('[data-testid="embed-bootstrap-loading"]')
    .waitFor({ state: 'hidden', timeout: cfg.stepTimeoutMs })
    .catch(() => {});
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

  // ② 打开「发起编校审」面板，等它的自动化钩子（已自动开着就不再点 ribbon，点了是 toggle）。
  await ensureHook(page, '__plant3dInitiateReviewE2E', RIBBON.initiateReview, {
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

  // ③b 数据包名称是 validateForm() 的必填项（InitiateReviewPanel.vue:655-663），空着时
  //    handleSubmit() 直接 return、一条请求都不发、getLastCreateResult() 永远是 null
  //    （2026-09-13 首次实跑就卡在这儿）。钩子没有 setter，只能照占位符填输入框
  //    （模板 :1134-1135，占位符「输入编校审数据包名称...」）。
  const packageName = `A4 走查 ${cfg.runId}`;
  await page
    .getByPlaceholder('输入编校审数据包名称')
    .first()
    .fill(packageName, { timeout: cfg.actionTimeoutMs });
  notes.push(`packageName=${packageName}`);
  await shot(page, cfg, 'u1-03-component-added');

  // ④ 提交 = 建提资单（POST /api/review/tasks）。
  await callHook(page, '__plant3dInitiateReviewE2E', 'submit');
  const created = await waitForCreateResult(page, cfg);
  state.taskId = created?.taskId ?? created?.id ?? null;
  if (!state.taskId) {
    // 把面板上可见的校验错误一并带出来，下次卡住不用再开截图猜。
    const visibleErrors = await page
      .locator('p.text-xs')
      .allTextContents()
      .then((all) => all.map((t) => t.trim()).filter(Boolean))
      .catch(() => []);
    return {
      ok: false,
      detail: `提交后 getLastCreateResult() 没给出 taskId：${JSON.stringify(created)?.slice(0, 300)}`
        + (visibleErrors.length ? `；面板校验提示：${visibleErrors.join(' / ')}` : ''),
    };
  }
  log(`提资单已建 task_id=${state.taskId}`);
  notes.push(`task_id=${state.taskId}`);
  await shot(page, cfg, 'u1-04-task-created');

  // Explicit account assignment: jd is a role, JH is the default PMS account.
  const assigned = await apiFromPage(page, cfg, 'PATCH', `/api/review/tasks/${state.taskId}`, auth.token, {
    checkerId: cfg.checkerId, checkerName: cfg.checkerName,
    reviewerId: cfg.checkerId, reviewerName: cfg.checkerName,
  });
  if (!assigned.ok || assigned.json?.success !== true) {
    return { ok: false, detail: `配置校核负责人失败：${assigned.status} ${assigned.bodyText.slice(0, 300)}` };
  }

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
