// 走查配置：全部来自环境变量，全部有默认值，解析结果会原样写进 report.json。
//
// 两条「前端指向 :8022」的路子行为不同，README §2 有详解，这里只记判据：
//   query 档 `?backendPort=8022` —— apiBase.ts:88-90 的 URL 参数覆盖，优先级最高，
//     浏览器直连 :8022（跨域，靠 gen-model 的 CORS）；抓到的请求 origin 是 :8022。
//   env 档 `VITE_GEN_MODEL_API_BASE_URL=http://localhost:8022` —— dev 下 apiBase.ts:68-70
//     会把 loopback 折成同源空串，请求走 Vite 的 `/api` 代理；抓到的请求 origin 是 :3101。
// U3 只能用 env 档：嵌入地址由后端 embed-url 现拼，带不上 `?backendPort=`。

import { existsSync } from 'node:fs';
import path from 'node:path';

function str(name, fallback) {
  const v = process.env[name];
  return v === undefined || v.trim() === '' ? fallback : v.trim();
}

function int(name, fallback) {
  const raw = process.env[name];
  if (raw === undefined || raw.trim() === '') return fallback;
  const n = Number.parseInt(raw, 10);
  if (!Number.isFinite(n) || n <= 0) {
    throw new Error(`${name} 要正整数，收到 ${JSON.stringify(raw)}`);
  }
  return n;
}

function bool(name, fallback) {
  const raw = process.env[name];
  if (raw === undefined || raw.trim() === '') return fallback;
  const v = raw.trim().toLowerCase();
  if (['1', 'true', 'yes', 'on'].includes(v)) return true;
  if (['0', 'false', 'no', 'off'].includes(v)) return false;
  throw new Error(`${name} 要布尔值（1/0/true/false），收到 ${JSON.stringify(raw)}`);
}

function stripSlash(url) {
  return url.replace(/\/+$/, '');
}

const HERE = path.dirname(new URL(import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1'));

export function loadConfig(argv = []) {
  const steps = parseSteps(argv);
  const backendBaseUrl = stripSlash(str('REVIEW_BACKEND_URL', 'http://localhost:8022'));
  const backendPort = new URL(backendBaseUrl).port || '80';
  const webBaseUrl = stripSlash(str('REVIEW_WEB_URL', 'http://localhost:3101'));
  const webRepo = str('REVIEW_WEB_REPO', 'D:\\work\\plant-code\\old\\plant3d-web');

  const cfg = {
    steps,
    dryRun: argv.includes('--dry-run') || bool('REVIEW_DRY_RUN', false),

    backendBaseUrl,
    backendPort,
    webBaseUrl,
    webRepo,
    webRepoExists: existsSync(webRepo),

    // `auto` = 端口没在听才起；`never` = 只用现成的；`always` = 一律自己起（端口被占直接失败）。
    startWeb: str('REVIEW_START_WEB', 'auto'),

    // 前端指向后端的档位：query | env。U3 强制 env，见上方注释。
    backendWiring: str('REVIEW_BACKEND_WIRING', 'query'),

    projectId: str('REVIEW_PROJECT_ID', 'AvevaMarineSample'),
    designerId: str('REVIEW_SJ_USER', 'SJ'),
    designerName: str('REVIEW_SJ_NAME', '设计'),
    checkerId: str('REVIEW_JD_USER', 'JH'),
    checkerName: str('REVIEW_JD_NAME', '校核'),
    // u2 最后一下：approve（通过）还是 return（驳回带原因）。两条都在契约表里。
    u2Decision: str('REVIEW_U2_DECISION', 'approve'),

    headless: bool('REVIEW_HEADLESS', true),
    // 人工接管：浏览器打开后脚本不点任何东西，只录制控制台与请求，等你手动走完再回车。
    manual: bool('REVIEW_MANUAL', false),

    totalTimeoutMs: int('REVIEW_TOTAL_TIMEOUT_MS', 600_000),
    stepTimeoutMs: int('REVIEW_STEP_TIMEOUT_MS', 120_000),
    actionTimeoutMs: int('REVIEW_ACTION_TIMEOUT_MS', 20_000),
    // preflight 探活单次超时：要短，探不通就立刻失败，不许挂死。
    probeTimeoutMs: int('REVIEW_PROBE_TIMEOUT_MS', 5_000),
    webStartTimeoutMs: int('REVIEW_WEB_START_TIMEOUT_MS', 180_000),

    outRoot: str('REVIEW_OUT_DIR', path.join(HERE, 'out')),
    runId: str('REVIEW_RUN_ID', new Date().toISOString().replace(/[:.]/g, '-')),

    // 走查结束后是否调 PMS 的 /api/review/delete 软删本次数据。
    cleanup: bool('REVIEW_CLEANUP', true),
  };

  if (!['auto', 'never', 'always'].includes(cfg.startWeb)) {
    throw new Error(`REVIEW_START_WEB 只能是 auto|never|always，收到 ${JSON.stringify(cfg.startWeb)}`);
  }
  if (!['query', 'env'].includes(cfg.backendWiring)) {
    throw new Error(`REVIEW_BACKEND_WIRING 只能是 query|env，收到 ${JSON.stringify(cfg.backendWiring)}`);
  }
  if (!['approve', 'return'].includes(cfg.u2Decision)) {
    throw new Error(`REVIEW_U2_DECISION 只能是 approve|return，收到 ${JSON.stringify(cfg.u2Decision)}`);
  }
  if (cfg.stepTimeoutMs >= cfg.totalTimeoutMs) {
    throw new Error(
      `单步超时 ${cfg.stepTimeoutMs}ms 不小于总超时 ${cfg.totalTimeoutMs}ms，总超时就没有意义了`,
    );
  }

  cfg.outDir = path.join(cfg.outRoot, cfg.runId);
  cfg.screenshotDir = path.join(cfg.outDir, 'screenshots');
  return cfg;
}

function parseSteps(argv) {
  const flag = argv.find((a) => a.startsWith('--steps='));
  const raw = flag ? flag.slice('--steps='.length) : process.env.REVIEW_STEPS || 'u1,u2,u3';
  const steps = raw.split(',').map((s) => s.trim().toLowerCase()).filter(Boolean);
  const unknown = steps.filter((s) => !['u1', 'u2', 'u3'].includes(s));
  if (unknown.length) throw new Error(`未知步骤 ${unknown.join(',')}；只有 u1 / u2 / u3`);
  if (!steps.length) throw new Error('--steps 至少要一个步骤');
  return steps;
}

/**
 * 前端页面地址。`automation_review=1` 打开 plant3d-web 的三个 E2E 钩子
 * （InitiateReviewPanel.vue:521-535、ReviewPanel.vue:1277-1280、useDtxTools.ts:3165），
 * 没有它 U1/U2 只能靠 DOM 选择器，一改版就碎。
 */
export function buildWebUrl(cfg, routePath, extraParams = {}) {
  const url = new URL(routePath, `${cfg.webBaseUrl}/`);
  url.searchParams.set('automation_review', '1');
  if (cfg.backendWiring === 'query') url.searchParams.set('backendPort', cfg.backendPort);
  for (const [k, v] of Object.entries(extraParams)) {
    if (v !== undefined && v !== null) url.searchParams.set(k, String(v));
  }
  return url.toString();
}

/** 起 Vite dev 时注入的环境变量。显式钉死 VITE_BACKEND_PORT，免得代理上游靠推断。 */
export function webServerEnv(cfg) {
  return {
    ...process.env,
    VITE_GEN_MODEL_API_BASE_URL: cfg.backendBaseUrl,
    VITE_BACKEND_PORT: cfg.backendPort,
    VITE_BACKEND_URL: cfg.backendBaseUrl,
    // A4 的硬条件：关掉 mock 兜底，后端不通就必须报错，不许拿假数据糊过去。
    VITE_REVIEW_ALLOW_MOCK_FALLBACK: 'false',
    VITE_INCREMENTAL_ALLOW_DEMO_FALLBACK: 'false',
  };
}
