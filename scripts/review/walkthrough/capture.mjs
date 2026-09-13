// 采集：控制台分类 + 每条校审请求的 URL/方法/状态码。
//
// A4 的判据是「浏览器控制台无 fallback 告警」。plant3d-web 里 fallback 有两种，
// 后果完全不同，混在一起报会误导验收人，所以这里按源码逐字分类（出处见下表）：
//
//   mock-used      useUserStore.ts:691,720  "using development mock data"
//                  —— mock 兜底真的用上了：页面上的数据是假的。A4 直接判 FAIL。
//   mock-disabled  useUserStore.ts:694,723  "production mock fallback is disabled"
//                  useUserStore.ts:1542     "Local review mock mode is disabled in production"
//                  —— 兜底按 A4 的要求关着，但后端这一次调用失败了。数据不是假的，
//                     可 A4 要的是「全程无一次失败」，同样判 FAIL，只是归因不同：
//                     前者是前端配错（兜底没关），后者是后端没答上。
//   shadow         reviewSnapshotService.ts:96,119,153  "[review/M2 SHADOW]"
//                  —— 影子比对的告警，与 mock 兜底无关，不判 A4。
//
// 剩下的按 error / warning / info / log 原样记，不做判断——「普通日志」那一类。

import { isReviewDomainPath, matchContractRoute } from './contract.mjs';

const MOCK_USED_PATTERNS = [/using development mock data/i];
const MOCK_DISABLED_PATTERNS = [
  /production mock fallback is disabled/i,
  /Local review mock mode is disabled in production/i,
];
const SHADOW_PATTERNS = [/\[review\/M2 SHADOW\]/i];

/** @returns {'mock-used'|'mock-disabled'|'shadow'|'plain'} */
export function classifyConsoleText(text) {
  if (MOCK_USED_PATTERNS.some((re) => re.test(text))) return 'mock-used';
  if (MOCK_DISABLED_PATTERNS.some((re) => re.test(text))) return 'mock-disabled';
  if (SHADOW_PATTERNS.some((re) => re.test(text))) return 'shadow';
  return 'plain';
}

/** 判 A4 的只有这两类；shadow 与 plain 不参与。 */
export function isFallbackClass(kind) {
  return kind === 'mock-used' || kind === 'mock-disabled';
}

export class Capture {
  constructor(cfg) {
    this.cfg = cfg;
    /** @type {Array<object>} */
    this.console = [];
    /** @type {Array<object>} */
    this.requests = [];
    /** @type {Array<object>} */
    this.pageErrors = [];
    this.step = 'init';
    this.backendOrigin = new URL(cfg.backendBaseUrl).origin;
    this.webOrigin = new URL(cfg.webBaseUrl).origin;
  }

  setStep(step) {
    this.step = step;
  }

  /** 给一个 page 或 iframe 所属的 page 装监听；嵌入页要单独调一次。 */
  attach(page, label) {
    page.on('console', (msg) => {
      let text;
      try {
        text = msg.text();
      } catch {
        text = '<console text 取不到>';
      }
      const kind = classifyConsoleText(text);
      this.console.push({
        at: new Date().toISOString(),
        step: this.step,
        page: label,
        level: msg.type(),
        kind,
        text,
        location: safeLocation(msg),
      });
    });

    page.on('pageerror', (err) => {
      this.pageErrors.push({
        at: new Date().toISOString(),
        step: this.step,
        page: label,
        message: String(err?.message || err),
        stack: String(err?.stack || ''),
      });
    });

    page.on('response', async (res) => {
      let url;
      try {
        url = new URL(res.url());
      } catch {
        return;
      }
      if (!isReviewDomainPath(url.pathname)) return;
      const req = res.request();
      const method = req.method();
      const contract = matchContractRoute(method, url.pathname);
      this.requests.push({
        at: new Date().toISOString(),
        step: this.step,
        page: label,
        method,
        url: res.url(),
        origin: url.origin,
        pathname: url.pathname,
        status: res.status(),
        contractPath: contract ? contract.path : null,
        contractGroup: contract ? contract.group : null,
        // env 档下请求打到 :3101 由 Vite 代理转发，query 档下直连 :8022；
        // 打到别的 origin（比如旧端 :3100）就是接线错了。
        origin_class: url.origin === this.backendOrigin
          ? 'backend-direct'
          : url.origin === this.webOrigin
            ? 'via-dev-proxy'
            : 'unexpected',
        resourceType: req.resourceType(),
      });
    });

    page.on('requestfailed', (req) => {
      let url;
      try {
        url = new URL(req.url());
      } catch {
        return;
      }
      if (!isReviewDomainPath(url.pathname)) return;
      this.requests.push({
        at: new Date().toISOString(),
        step: this.step,
        page: label,
        method: req.method(),
        url: req.url(),
        origin: url.origin,
        pathname: url.pathname,
        status: null,
        failure: req.failure()?.errorText || 'unknown',
        contractPath: matchContractRoute(req.method(), url.pathname)?.path ?? null,
        origin_class: 'failed',
        resourceType: req.resourceType(),
      });
    });
  }

  /** A4 判据：fallback 告警一条都不许有。 */
  fallbackHits() {
    return this.console.filter((c) => isFallbackClass(c.kind));
  }

  /** 非 2xx / 3xx 的校审请求。 */
  badRequests() {
    return this.requests.filter((r) => r.status === null || r.status >= 400);
  }

  unexpectedOrigins() {
    return this.requests.filter((r) => r.origin_class === 'unexpected');
  }

  counts() {
    const byKind = {};
    for (const c of this.console) byKind[c.kind] = (byKind[c.kind] || 0) + 1;
    return {
      console: this.console.length,
      consoleByKind: byKind,
      pageErrors: this.pageErrors.length,
      reviewRequests: this.requests.length,
      badRequests: this.badRequests().length,
    };
  }
}

function safeLocation(msg) {
  try {
    const loc = msg.location();
    return loc?.url ? `${loc.url}:${loc.lineNumber}:${loc.columnNumber}` : null;
  } catch {
    return null;
  }
}
