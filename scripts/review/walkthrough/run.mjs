#!/usr/bin/env node
// A4 前端走查编排器（计划 §6 A4 / §4 U1–U3）。
//
// 顺序是刻意的：preflight 全部通过之前不 import playwright，也不开浏览器。
// 这样「后端没起」这种最常见的情形在几秒内就带着可照做的提示退出（exit 2），
// 而不是开着浏览器在某个 waitFor 上耗到总超时——那正是任务要求里说的「不许挂死」。
//
// 退出码：0 通过 / 1 走查判失败 / 2 preflight 失败 / 3 总超时 / 4 参数或环境错

import { spawn } from 'node:child_process';
import { mkdirSync } from 'node:fs';
import net from 'node:net';
import path from 'node:path';

import { loadConfig, webServerEnv } from './config.mjs';
import { Capture } from './capture.mjs';
import { formatChecks, preflight } from './preflight.mjs';
import { writeArtifacts, buildChecklist } from './report.mjs';

const EXIT = { ok: 0, failed: 1, preflight: 2, timeout: 3, badEnv: 4 };

const argv = process.argv.slice(2);
const startedAt = new Date().toISOString();

let cfg;
try {
  cfg = loadConfig(argv);
} catch (err) {
  console.error(`[config] ${err.message}`);
  process.exit(EXIT.badEnv);
}

if (argv.includes('--help') || argv.includes('-h')) {
  console.log(usage());
  process.exit(EXIT.ok);
}

main().then(
  (code) => process.exit(code),
  (err) => {
    console.error('[run] 未捕获的失败：', err?.stack || err);
    process.exit(EXIT.failed);
  },
);

async function main() {
  banner();

  // ── 1. preflight（不依赖 npm 包）──────────────────────────────────────
  const pre = await preflight(cfg);
  console.log('\n── preflight ──');
  console.log(formatChecks(pre.checks));
  if (!pre.ok) {
    console.error('\n[preflight] 未通过，走查不启动。上面每条 FAIL 都写了该怎么办。');
    persist({ verdict: 'PREFLIGHT_FAILED', preflightChecks: pre.checks, capture: new Capture(cfg), stepResults: [] });
    return EXIT.preflight;
  }
  if (cfg.dryRun) {
    console.log('\n[dry-run] preflight 通过；--dry-run 到此为止，不开浏览器、不改任何数据。');
    persist({ verdict: 'DRY_RUN_OK', preflightChecks: pre.checks, capture: new Capture(cfg), stepResults: [] });
    return EXIT.ok;
  }

  // ── 2. 依赖到位了才 import playwright ────────────────────────────────
  let chromium;
  try {
    ({ chromium } = await import('playwright'));
  } catch (err) {
    console.error(
      '\n[deps] 没装 playwright。在 scripts/review/walkthrough/ 下跑：\n'
      + '    npm ci\n'
      + '    npx playwright install chromium\n'
      + `  （原始错误：${err?.message}）`,
    );
    return EXIT.badEnv;
  }

  // ── 3. 总超时闸门 ────────────────────────────────────────────────────
  let timedOut = false;
  const totalTimer = setTimeout(() => {
    timedOut = true;
    console.error(`\n[timeout] 总超时 ${cfg.totalTimeoutMs}ms 到了，强制收尾。`);
  }, cfg.totalTimeoutMs);
  totalTimer.unref?.();

  const capture = new Capture(cfg);
  /** @type {Array<object>} */
  const stepResults = [];
  const state = { formId: process.env.REVIEW_FORM_ID || null, taskId: process.env.REVIEW_TASK_ID || null };
  let webServer = null;
  let browser = null;

  try {
    // ── 4. dev server：需要才起，起了就负责 kill（纪律③ 不留长驻进程）──
    webServer = await ensureWebServer(cfg);

    browser = await chromium.launch({ headless: cfg.headless });
    const context = await browser.newContext({
      viewport: { width: 1600, height: 1000 },
      ignoreHTTPSErrors: true,
    });
    context.setDefaultTimeout(cfg.actionTimeoutMs);
    // 嵌入 iframe 会开新页；新页也要挂采集，否则 U3 的控制台是空的。
    context.on('page', (p) => capture.attach(p, `page#${capture.console.length}`));

    const page = await context.newPage();
    capture.attach(page, 'main');

    for (const stepId of cfg.steps) {
      if (timedOut) {
        stepResults.push({ step: stepId, ok: false, elapsedMs: 0, detail: '总超时，未执行' });
        continue;
      }
      const mod = await import(`./steps/${stepId}.mjs`);
      capture.setStep(stepId);
      const t0 = Date.now();
      console.log(`\n── ${mod.title} ──`);
      let result;
      try {
        result = await withTimeout(
          mod.run({ page, cfg, capture, state, log: (m) => console.log(`   ${m}`) }),
          cfg.stepTimeoutMs,
          `${stepId} 单步超时 ${cfg.stepTimeoutMs}ms`,
        );
      } catch (err) {
        result = { ok: false, detail: String(err?.message || err) };
      }
      const elapsedMs = Date.now() - t0;
      stepResults.push({ step: stepId, title: mod.title, elapsedMs, ...result });
      console.log(`   → ${result.ok ? 'PASS' : 'FAIL'}（${Math.round(elapsedMs / 1000)}s）${result.detail ? `：${result.detail}` : ''}`);
      if (!result.ok) break; // 后面的步骤依赖前面的产物，前面塌了继续跑只是制造噪声
    }
  } finally {
    clearTimeout(totalTimer);
    if (browser) await browser.close().catch(() => {});
    if (webServer) await stopWebServer(webServer);
  }

  // ── 5. 判定 ──────────────────────────────────────────────────────────
  for (const step of cfg.steps) {
    if (!stepResults.some(s => s.step === step)) stepResults.push({ step, ok: false, elapsedMs: 0, detail: '前置步骤失败，未执行' });
  }
  const stepsOk = stepResults.length === cfg.steps.length && stepResults.every((s) => s.ok);
  const missing = buildChecklist(capture, cfg.steps, cfg.u2Decision).missing;
  const fallbackHits = capture.fallbackHits();
  const badRequests = capture.badRequests();
  const unexpected = capture.unexpectedOrigins();
  const verdict = timedOut
    ? 'TIMEOUT'
    : stepsOk && missing.length === 0 && fallbackHits.length === 0 && badRequests.length === 0 && unexpected.length === 0 && capture.pageErrors.length === 0
      ? 'PASS'
      : 'FAIL';

  const { report } = persist({ verdict, preflightChecks: pre.checks, capture, stepResults });
  summary(report, capture);

  if (timedOut) return EXIT.timeout;
  return verdict === 'PASS' ? EXIT.ok : EXIT.failed;
}

function persist({ verdict, preflightChecks, capture, stepResults }) {
  mkdirSync(cfg.outDir, { recursive: true });
  const out = writeArtifacts({
    cfg,
    capture,
    steps: cfg.steps,
    stepResults,
    preflightChecks,
    verdict,
    startedAt,
    finishedAt: new Date().toISOString(),
  });
  console.log(`\n[artifacts] ${cfg.outDir}`);
  return out;
}

function summary(report, capture) {
  console.log('\n── A4 判据 ──');
  const rows = [
    ['控制台 fallback 告警', report.a4.fallbackWarnings],
    ['校审请求非 2xx/3xx', report.a4.failedReviewRequests],
    ['请求打到预期外 origin', report.a4.unexpectedOrigins],
    ['本次应打却没打到', report.a4.contractMissing],
  ];
  for (const [label, n] of rows) console.log(`  ${n === 0 ? 'PASS' : 'FAIL'}  ${label}：${n}`);
  for (const f of capture.fallbackHits()) console.log(`        ${f.kind} [${f.step}] ${f.text}`);
  console.log(`\n结论：${report.verdict}`);
}

async function withTimeout(promise, ms, message) {
  let timer;
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error(message)), ms);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

function portOf(url) {
  const u = new URL(url);
  return Number(u.port || (u.protocol === 'https:' ? 443 : 80));
}

function isListening(url, timeoutMs) {
  return new Promise((resolve) => {
    const socket = net.connect({ host: new URL(url).hostname, port: portOf(url) });
    const done = (v) => {
      socket.destroy();
      resolve(v);
    };
    socket.setTimeout(timeoutMs, () => done(false));
    socket.once('connect', () => done(true));
    socket.once('error', () => done(false));
  });
}

async function ensureWebServer(cfg) {
  if (cfg.startWeb === 'never') return null;
  if (cfg.startWeb === 'auto' && (await isListening(cfg.webBaseUrl, cfg.probeTimeoutMs))) {
    console.log(`\n[web] 沿用已在听的 ${cfg.webBaseUrl}（不由本脚本管理，结束时也不会 kill）`);
    return null;
  }

  console.log(`\n[web] 起 Vite dev：${cfg.webRepo} → ${cfg.webBaseUrl}`);
  const child = spawn('npm', ['run', 'dev', '--', '--port', String(portOf(cfg.webBaseUrl)), '--strictPort'], {
    cwd: cfg.webRepo,
    env: webServerEnv(cfg),
    stdio: ['ignore', 'pipe', 'pipe'],
    shell: process.platform === 'win32', // Windows 上 npm 是 npm.cmd
    windowsHide: true,
  });
  child.stdout.on('data', (b) => process.env.REVIEW_WEB_LOG === '1' && process.stdout.write(`[vite] ${b}`));
  child.stderr.on('data', (b) => process.stderr.write(`[vite:err] ${b}`));

  const deadline = Date.now() + cfg.webStartTimeoutMs;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(`Vite dev 起不来，进程已退出（code ${child.exitCode}）。REVIEW_WEB_LOG=1 看它的输出。`);
    }
    if (await isListening(cfg.webBaseUrl, 1_000)) {
      console.log(`[web] 就绪（${Math.round((cfg.webStartTimeoutMs - (deadline - Date.now())) / 1000)}s）`);
      return child;
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  child.kill('SIGTERM');
  throw new Error(`Vite dev ${cfg.webStartTimeoutMs}ms 内没听上 ${cfg.webBaseUrl}。REVIEW_WEB_LOG=1 看它的输出。`);
}

async function stopWebServer(child) {
  if (!child || child.exitCode !== null) return;
  console.log('[web] 收掉本脚本起的 dev server');
  if (process.platform === 'win32') {
    // Windows 上 npm run dev 是 cmd → node 两层，SIGTERM 只杀得掉外层，留下 vite 占着 3101。
    await new Promise((resolve) => {
      spawn('taskkill', ['/pid', String(child.pid), '/t', '/f'], { stdio: 'ignore' }).on('close', resolve);
    });
  } else {
    child.kill('SIGTERM');
  }
}

function banner() {
  console.log(`A4 前端走查 · run ${cfg.runId}`);
  console.log(`  步骤        ${cfg.steps.join(' → ')}${cfg.manual ? '（人工接管，只录不点）' : ''}`);
  console.log(`  后端        ${cfg.backendBaseUrl}`);
  console.log(`  前端        ${cfg.webBaseUrl}（${cfg.backendWiring === 'query' ? '?backendPort= 直连' : 'env + Vite /api 代理'}）`);
  console.log(`  超时        总 ${cfg.totalTimeoutMs}ms / 单步 ${cfg.stepTimeoutMs}ms / 动作 ${cfg.actionTimeoutMs}ms`);
  console.log(`  产物        ${cfg.outDir}`);
}

function usage() {
  return `用法：node run.mjs [--steps=u1,u2,u3] [--dry-run]

  --dry-run   只跑 preflight，不开浏览器、不改任何数据
  --steps=    默认 u1,u2,u3；U3 需要 REVIEW_BACKEND_WIRING=env

环境变量见 ${path.join(path.dirname(new URL(import.meta.url).pathname), 'README.md')} §3。`;
}
