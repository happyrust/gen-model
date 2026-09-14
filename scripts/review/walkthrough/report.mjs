// 产物落盘：report.json（机器读）、report.md（人读 / 贴进验收证据）、
// contract-checklist.md（计划 §1.2 逐条勾）、console.jsonl、requests.jsonl。

import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';

import { CONTRACT_ROUTES, expectedRoutesFor } from './contract.mjs';

function jsonl(rows) {
  return rows.map((r) => JSON.stringify(r)).join('\n') + (rows.length ? '\n' : '');
}

/**
 * 把实际抓到的请求勾回契约表。
 * @returns {{ rows: Array<object>, missing: Array<object> }}
 */
export function buildChecklist(capture, steps, decision = 'approve') {
  const expected = new Set();
  for (const step of steps) for (const r of expectedRoutesFor(step, decision)) expected.add(`${r.method} ${r.path}`);

  const rows = CONTRACT_ROUTES.map((route) => {
    const key = `${route.method} ${route.path}`;
    const hits = capture.requests.filter((r) => r.contractPath === route.path && r.method === route.method);
    const statuses = [...new Set(hits.map((h) => (h.status === null ? `FAILED(${h.failure || '?'})` : h.status)))];
    const ok = hits.length > 0 && hits.every((h) => h.status !== null && h.status < 400);
    return {
      method: route.method,
      path: route.path,
      group: route.group,
      expectedThisRun: expected.has(key),
      hitCount: hits.length,
      statuses,
      verdict: hits.length === 0 ? (expected.has(key) ? 'MISS' : 'NOT EXERCISED') : ok ? 'PASS' : 'FAIL',
      note: route.note ?? '',
    };
  });

  return { rows, missing: rows.filter((r) => r.verdict === 'MISS' || r.verdict === 'FAIL') };
}

export function writeArtifacts({ cfg, capture, steps, stepResults, preflightChecks, verdict, startedAt, finishedAt }) {
  mkdirSync(cfg.outDir, { recursive: true });
  mkdirSync(cfg.screenshotDir, { recursive: true });

  const { rows, missing } = buildChecklist(capture, steps, cfg.u2Decision);
  const fallbackHits = capture.fallbackHits();
  const badRequests = capture.badRequests();
  const unexpectedOrigins = capture.unexpectedOrigins();

  const report = {
    runId: cfg.runId,
    verdict,
    startedAt,
    finishedAt,
    elapsedMs: new Date(finishedAt) - new Date(startedAt),
    config: redactConfig(cfg),
    preflight: preflightChecks,
    steps: stepResults,
    counts: capture.counts(),
    a4: {
      // 计划 §6 A4：走查 U1、U2 全程无一次 mock 兜底，附件 multipart 成功且静态附件可打开。
      fallbackWarnings: fallbackHits.length,
      fallbackDetail: fallbackHits.map((c) => ({ step: c.step, kind: c.kind, level: c.level, text: c.text })),
      failedReviewRequests: badRequests.length,
      unexpectedOrigins: unexpectedOrigins.length,
      contractMissing: missing.length,
    },
    contractChecklist: rows,
  };

  writeFileSync(path.join(cfg.outDir, 'report.json'), JSON.stringify(report, null, 2), 'utf8');
  writeFileSync(path.join(cfg.outDir, 'console.jsonl'), jsonl(capture.console), 'utf8');
  writeFileSync(path.join(cfg.outDir, 'requests.jsonl'), jsonl(capture.requests), 'utf8');
  writeFileSync(path.join(cfg.outDir, 'contract-checklist.md'), renderChecklist(rows, cfg), 'utf8');
  writeFileSync(path.join(cfg.outDir, 'report.md'), renderReport(report, capture), 'utf8');

  return { report, checklistRows: rows };
}

function redactConfig(cfg) {
  const { ...rest } = cfg;
  return rest;
}

function renderChecklist(rows, cfg) {
  const lines = [
    `# 计划 §1.2 契约逐条勾（run ${cfg.runId}）`,
    '',
    '`expectedThisRun` = 本次跑的步骤按 contract.mjs 应当打到它；`NOT EXERCISED` = 在册但本次没走到，不算失败。',
    '',
    '| 方法 | 路径 | 域 | 本次应打 | 命中 | 状态码 | 判定 | 备注 |',
    '| --- | --- | --- | --- | --- | --- | --- | --- |',
  ];
  for (const r of rows) {
    lines.push(
      `| ${r.method} | \`${r.path}\` | ${r.group} | ${r.expectedThisRun ? '是' : '—'} | ${r.hitCount} | ${r.statuses.join(' ') || '—'} | ${r.verdict} | ${r.note} |`,
    );
  }
  const miss = rows.filter((r) => r.verdict === 'MISS');
  const fail = rows.filter((r) => r.verdict === 'FAIL');
  lines.push('', `合计 ${rows.length} 条：PASS ${rows.filter((r) => r.verdict === 'PASS').length}，`
    + `MISS ${miss.length}，FAIL ${fail.length}，NOT EXERCISED ${rows.filter((r) => r.verdict === 'NOT EXERCISED').length}。`);
  return lines.join('\n') + '\n';
}

function renderReport(report, capture) {
  const L = [];
  L.push(`# A4 前端走查报告 · ${report.runId}`);
  L.push('');
  L.push(`结论：**${report.verdict}**（耗时 ${Math.round(report.elapsedMs / 1000)}s）`);
  L.push('');
  L.push('## A4 判据');
  L.push('');
  if (report.verdict === 'PREFLIGHT_FAILED' || report.verdict === 'DRY_RUN_OK') {
    L.push(`> **这张表此刻是空转的**：verdict=${report.verdict}，浏览器根本没开、一条请求都没发，`);
    L.push('> 「0 条 fallback 告警」只说明没采到东西，不代表 A4 通过。先让 preflight 全绿再跑完整走查。');
    L.push('');
  }
  L.push('| 判据 | 数 | 结论 |');
  L.push('| --- | --- | --- |');
  L.push(`| 控制台 fallback 告警（mock-used + mock-disabled） | ${report.a4.fallbackWarnings} | ${report.a4.fallbackWarnings === 0 ? 'PASS' : 'FAIL'} |`);
  L.push(`| 校审请求非 2xx/3xx | ${report.a4.failedReviewRequests} | ${report.a4.failedReviewRequests === 0 ? 'PASS' : 'FAIL'} |`);
  L.push(`| 请求打到预期之外的 origin | ${report.a4.unexpectedOrigins} | ${report.a4.unexpectedOrigins === 0 ? 'PASS' : 'FAIL'} |`);
  L.push(`| 本次应打却没打到的契约条目 | ${report.a4.contractMissing} | ${report.a4.contractMissing === 0 ? 'PASS' : 'FAIL'} |`);
  L.push('');

  if (report.a4.fallbackDetail.length) {
    L.push('### fallback 告警明细');
    L.push('');
    for (const f of report.a4.fallbackDetail) {
      L.push(`- \`${f.kind}\` [${f.step}] ${f.level}: ${f.text}`);
    }
    L.push('');
    L.push('`mock-used` = 兜底真的用上了（前端没关 VITE_REVIEW_ALLOW_MOCK_FALLBACK）；');
    L.push('`mock-disabled` = 兜底关着但后端这一次没答上（归因在后端）。');
    L.push('');
  }

  L.push('## 分步');
  L.push('');
  L.push('| 步骤 | 结论 | 耗时 | 说明 |');
  L.push('| --- | --- | --- | --- |');
  for (const s of report.steps) {
    L.push(`| ${s.step} | ${s.ok ? 'PASS' : 'FAIL'} | ${Math.round((s.elapsedMs || 0) / 1000)}s | ${(s.detail || '').replace(/\|/g, '\\|').replace(/\n/g, ' ')} |`);
  }
  L.push('');

  L.push('## 控制台分类计数');
  L.push('');
  for (const [kind, n] of Object.entries(report.counts.consoleByKind)) L.push(`- ${kind}: ${n}`);
  L.push(`- pageerror: ${report.counts.pageErrors}`);
  L.push('');

  const bad = capture.badRequests();
  if (bad.length) {
    L.push('## 失败的校审请求');
    L.push('');
    L.push('| 步骤 | 方法 | 路径 | 状态 |');
    L.push('| --- | --- | --- | --- |');
    for (const r of bad) L.push(`| ${r.step} | ${r.method} | \`${r.pathname}\` | ${r.status ?? `FAILED(${r.failure})`} |`);
    L.push('');
  }

  L.push('## 产物');
  L.push('');
  L.push('- `report.json` — 本报告的机器可读版');
  L.push('- `contract-checklist.md` — 计划 §1.2 逐条勾');
  L.push('- `console.jsonl` — 全部控制台消息（含分类）');
  L.push('- `requests.jsonl` — 全部校审域请求（URL / 方法 / 状态码 / origin 归类）');
  L.push('- `screenshots/` — 每步截图');
  return L.join('\n') + '\n';
}
