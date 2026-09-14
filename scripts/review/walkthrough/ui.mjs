// 三个走查步骤共用的页面助手。
//
// 驱动方式为什么不是 DOM 选择器：plant3d-web 的校审面板没有 data-testid
// （2026-09-13 全仓 grep src/review 零命中），但它自带三个 E2E 钩子，
// 在 `?automation_review=1` 或 localStorage `plant3d_automation_review=1` 时挂到 window 上：
//
//   __plant3dInitiateReviewE2E  src/components/review/InitiateReviewPanel.vue:521-546
//       addMockComponent(refNo?, name?) / submit() / getLastCreateResult()
//   __plant3dReviewerE2E        src/components/review/ReviewPanel.vue:1277-1410
//       addMockAnnotation / addMockMeasurement / confirmData(note?) /
//       refreshAnnotationCommentThread(type?, id?) / getAnnotationCount() /
//       getCloudScreenshot(id) / getConfirmedRecordCount() / …
//   __plant3dDtxE2E             src/composables/useDtxTools.ts:3165-3208
//       addCloudAnnotationWithScreenshot() → { annotationId, attachmentId, url, … }
//
// 钩子挂载的前提是对应面板已经 mount，所以每一步都是「先把面板点开，再等钩子」。
// 点面板这一下用 ribbon 的按钮文案（src/ribbon/ribbonConfig.ts:156-189），
// 点不开就抛一句写明「该点哪个按钮」的错，不静默等到超时。

import { setTimeout as delay } from 'node:timers/promises';

/** ribbon「校审」标签页下的面板按钮，文案取自 ribbonConfig.ts:171-189。 */
export const RIBBON = {
  tab: '校审',
  initiateReview: '发起编校审',
  reviewPanel: '校审面板',
  reviewerTasks: '待审核',
  resubmissionTasks: '批注处理',
  myTasks: '我的编校审',
  startReview: '开始校审',
  confirmData: '确认数据',
};

/** 点 ribbon 上某个按钮；找不到返回 false，由调用方决定怎么报。 */
export async function clickRibbon(page, label, { timeoutMs }) {
  const candidates = [
    page.getByRole('button', { name: label, exact: true }),
    page.getByRole('tab', { name: label, exact: true }),
    page.getByTitle(label, { exact: true }),
    page.getByText(label, { exact: true }),
  ];
  for (const loc of candidates) {
    const target = loc.first();
    try {
      await target.waitFor({ state: 'visible', timeout: Math.min(3_000, timeoutMs) });
      await target.click({ timeout: timeoutMs });
      return true;
    } catch {
      // 换下一种定位法
    }
  }
  return false;
}

/** 先切到「校审」标签页再点面板按钮。标签页点不到也继续——有的布局是常驻的。 */
export async function openReviewPanel(page, label, { timeoutMs }) {
  await clickRibbon(page, RIBBON.tab, { timeoutMs });
  const clicked = await clickRibbon(page, label, { timeoutMs });
  if (!clicked) {
    throw new Error(
      `点不开「${label}」面板。ribbon 文案取自 src/ribbon/ribbonConfig.ts:171-189；`
      + '前端改了文案就同步改 walkthrough/ui.mjs 的 RIBBON 常量。'
      + '想人工接管这一步：REVIEW_MANUAL=1 重跑，脚本只录不点。',
    );
  }
}

/**
 * 等某个 window 上的自动化钩子出现。
 * 超时消息要写清「钩子叫什么、谁挂的、为什么可能没挂上」——这是最容易卡住的一步。
 */
export async function waitForHook(page, hookName, { timeoutMs, panelHint }) {
  try {
    await page.waitForFunction(
      (name) => typeof window[name] === 'object' && window[name] !== null,
      hookName,
      { timeout: timeoutMs },
    );
  } catch {
    throw new Error(
      `等不到自动化钩子 window.${hookName}（${timeoutMs}ms）。三种可能：\n`
      + `  1. ${panelHint} 面板没打开——钩子是面板 mount 时才挂的；\n`
      + '  2. 页面 URL 少了 ?automation_review=1（或 localStorage plant3d_automation_review=1）；\n'
      + '  3. 前端版本不含该钩子——本走查按 2026-09-13 的 plant3d-web 写，'
      + '出处见 walkthrough/ui.mjs 顶部注释。',
    );
  }
}

/**
 * 钩子已在就直接用，不在才去点 ribbon 把面板打开。
 * 嵌入落地（user_token 进来）会按角色自动打开面板，此时再点 ribbon 是 **toggle**，会把刚开的
 * 面板关掉（DockLayout「Panel review exists, closing it」，2026-09-13 实跑撞到）。
 */
export async function ensureHook(page, hookName, ribbonLabel, { timeoutMs, panelHint }) {
  try {
    await waitForHook(page, hookName, { timeoutMs: Math.min(3_000, timeoutMs), panelHint });
    return 'already-open';
  } catch {
    await openReviewPanel(page, ribbonLabel, { timeoutMs });
    await waitForHook(page, hookName, { timeoutMs, panelHint });
    return 'opened-via-ribbon';
  }
}

/** 在页面里调钩子上的一个方法，返回它的返回值。 */
export function callHook(page, hookName, method, args = []) {
  return page.evaluate(
    ({ name, fn, a }) => {
      const hook = window[name];
      if (!hook) throw new Error(`window.${name} 不在`);
      if (typeof hook[fn] !== 'function') {
        throw new Error(`window.${name}.${fn} 不是函数；现有的是 [${Object.keys(hook).join(', ')}]`);
      }
      return Promise.resolve(hook[fn](...a));
    },
    { name: hookName, fn: method, a: args },
  );
}

/** 把 JWT 塞进 localStorage：前端读 `review_auth_token`（reviewApi.ts:98-119）。 */
export async function seedAuthToken(page, token) {
  await page.addInitScript((t) => {
    try {
      localStorage.setItem('review_auth_token', t);
      localStorage.setItem('plant3d_automation_review', '1');
    } catch {
      /* 无痕/禁用存储时忽略，页面自己会报 */
    }
  }, token);
}

/**
 * 直接找后端要一个角色 token（`POST /api/auth/token`，无鉴权）。
 * 走查里 U1/U2 用它「以某角色登录」，比在 UI 上填表稳。
 */
export async function mintToken(cfg, { userId, userName, role, formId }) {
  const body = {
    project_id: cfg.projectId,
    user_id: userId,
    user_name: userName,
    role,
  };
  if (formId) body.form_id = formId;

  const res = await fetch(`${cfg.backendBaseUrl}/api/auth/token`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(cfg.probeTimeoutMs),
  });
  const text = await res.text();
  let json;
  try {
    json = JSON.parse(text);
  } catch {
    throw new Error(`POST /api/auth/token 返回的不是 JSON（${res.status}）：${text.slice(0, 300)}`);
  }
  // 这一族是 PMS 侧信封 {code, message, data}，不是浏览器侧的 {success,…}（规范 §0.2）。
  if (!res.ok || !json?.data?.token) {
    throw new Error(
      `取 ${role} token 失败：HTTP ${res.status} ${JSON.stringify(json).slice(0, 300)}。`
      + 'role 只能是 admin|sj|jd|sh|pz；project/user 缺一会回 400。',
    );
  }
  return { token: json.data.token, formId: json.data.form_id, expiresAt: json.data.expires_at };
}

/** 截图，文件名带步骤与序号，便于贴进验收证据。 */
export async function shot(page, cfg, name) {
  const file = `${name}.png`;
  await page.screenshot({ path: `${cfg.screenshotDir}/${file}`, fullPage: false }).catch(() => {});
  return file;
}

/** 人工接管：开着浏览器只录不点，等操作者在终端回车。 */
export async function waitForOperator(prompt, timeoutMs) {
  process.stdout.write(`\n[manual] ${prompt}\n[manual] 走完后回到这里按回车继续（最多等 ${Math.round(timeoutMs / 1000)}s）…\n`);
  return new Promise((resolve) => {
    let done = false;
    const finish = (how) => {
      if (done) return;
      done = true;
      process.stdin.pause();
      resolve(how);
    };
    process.stdin.resume();
    process.stdin.once('data', () => finish('enter'));
    delay(timeoutMs).then(() => finish('timeout'));
  });
}
