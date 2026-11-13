
// =========================================================================
// 连接状态：新鲜 / 过期 / 拿不到（页级横幅全屏只此一处）
// =========================================================================
const STATUS_TIMEOUT = 40000;   // 长轮询最长挂 25 秒，再宽限 15 秒
const STALE_MS = 40000;
function newConn() { return { since: Date.now(), lastOkAt: 0, fails: 0, lastErr: '' }; }
let conn = newConn();
function connState() {
  if (needsToken) return 'token';
  const now = Date.now();
  if (!conn.lastOkAt) return (conn.fails >= 2 || now - conn.since > 30000) ? 'error' : 'loading';
  if (conn.fails >= 2 || (conn.fails >= 1 && now - conn.lastOkAt > 30000)) return 'error';
  if (now - conn.lastOkAt > STALE_MS) return 'stale';
  return 'ok';
}
function setConn(kind, text) {
  if (kind === 'bad' && /令牌/.test(text || '')) needsToken = true;
  renderConn();
}
function connErrText(e, timedOut) {
  if (timedOut) return `${Math.round(STATUS_TIMEOUT / 1000)} 秒没有回应`;
  if (e && e.status) return e.message && !/^HTTP \d+$/.test(e.message) ? e.message : `服务器返回 HTTP ${e.status}`;
  if (e && e.unknownShape) return e.message;
  return '连不上 ' + (activeBase ? hostOf(activeBase) : location.host);
}
function renderConn() {
  const cs = connState(), now = Date.now();
  const stamp = byId('live-stamp'), dot = byId('conn-dot');
  let text = '连接中…', cls = 'live-stamp', dotCls = 'dot', tip = '';
  if (cs === 'ok') {
    text = '更新于 ' + fmtClock(conn.lastOkAt); dotCls = 'dot ok';
    tip = '状态有变化会立刻推过来；没变化时每 25 秒核对一次。';
    if (lastStatus && typeof lastStatus.elapsed_ms === 'number') tip += `\n代理这次采集用了 ${lastStatus.elapsed_ms} 毫秒。`;
  } else if (cs === 'stale') {
    text = fmtAgo(now - conn.lastOkAt); cls += ' stale'; dotCls = 'dot warn';
    tip = `${fmtClock(conn.lastOkAt)} 之后没再收到回应，下面的数可能是旧的。`;
  } else if (cs === 'error') {
    text = '拿不到状态'; cls += ' error'; dotCls = 'dot bad'; tip = conn.lastErr || '';
  } else if (cs === 'token') {
    text = '需要令牌'; cls += ' error'; dotCls = 'dot bad';
  }
  if (stamp.className !== cls) stamp.className = cls;
  if (dot.className !== dotCls) dot.className = dotCls;
  setText('conn-text', text);
  setTip(stamp, tip);
  const wrap = byId('wrap');
  wrap.classList.toggle('is-stale', cs === 'stale');
  wrap.classList.toggle('is-error', (cs === 'error' || cs === 'token') && !!lastStatus);
  const alertOn = cs === 'error' || cs === 'token';
  setHidden('page-alert', !alertOn);
  if (alertOn) {
    const why = cs === 'token' ? '这台服务器要访问令牌' : (conn.lastErr || '连不上');
    const tail = lastStatus && conn.lastOkAt ? `下面是 ${fmtClock(conn.lastOkAt)} 的数据。` : '';
    setText('page-alert-text', `拿不到状态：${why}。${tail}`);
    setText('page-alert-btn', cs === 'token' ? '输入令牌…' : '重试');
  }
  return cs;
}
function retryNow() {
  if (needsToken) { openTokenModal(''); return; }
  restartStatusPoll();
}

// =========================================================================
// 近 15 分钟的历史：页面自己每 5 秒记一个点（代理没有历史接口）。
// 只记真采到的：连不上、页面没开着的时段留空，不补数。按服务器分开存在 sessionStorage，刷新页面不丢。
// =========================================================================
const HIST_WIN = 15 * 60 * 1000, HIST_STEP = 5000, TICKS = 30, TICK_MS = 30000;
const SEV = { ok: 1, off: 2, busy: 3, warn: 4, bad: 5 };
let hist = [];          // [{t, s:{name:code}, c:{name:cpu%}, m:{name:GB}, d:运行盘可用GB, cf:可提交GB, q:在跑+排队}]
let histWorst = {};     // 两次采样之间见过的最差状态，短暂的抖动不丢
let histLastAt = 0, histSavedAt = 0;
// 起停任务正在处理的服务记成 busy（起停中），不记成异常：一次正常的启动不该在刻度上留一格红
function tickCode(s, job) {
  if (s.enabled === false) return '';
  if (svcActing(s, job)) return 'busy';
  return s.state === 'ok' ? 'ok' : s.state === 'bad' ? 'bad' : s.state === 'waiting' ? 'warn' : s.state === 'stopped' ? 'off' : '';
}
function histKey() { return HIST_KEY_PREFIX + (activeBase || 'local'); }
function histLoad() {
  hist = []; histWorst = {}; histLastAt = 0;
  try {
    const o = JSON.parse(sessionStorage.getItem(histKey()) || 'null');
    const now = Date.now();
    if (o && Array.isArray(o.samples)) hist = o.samples.filter(p => p && typeof p.t === 'number' && p.t >= now - HIST_WIN && p.t <= now + 1000 && p.s && p.c && p.m);
  } catch (_) { hist = []; }
  if (hist.length) histLastAt = hist[hist.length - 1].t;
}
function histSave() {
  try { sessionStorage.setItem(histKey(), JSON.stringify({ v: 1, samples: hist })); } catch (_) {}
  histSavedAt = Date.now();
}
function histNote(st) {
  const job = activeJob(st);
  for (const s of st.services || []) {
    const c = tickCode(s, job);
    if (c && (!histWorst[s.name] || SEV[c] > SEV[histWorst[s.name]])) histWorst[s.name] = c;
  }
}
function histSample() {
  const st = lastStatus, now = Date.now();
  if (!st || connState() !== 'ok' || now - histLastAt < HIST_STEP - 250) return;
  const p = { t: now, s: {}, c: {}, m: {} };
  const job = activeJob(st);
  for (const s of st.services || []) {
    const cur = tickCode(s, job), w = histWorst[s.name];
    const c = w && (!cur || SEV[w] > SEV[cur]) ? w : cur;
    if (c) p.s[s.name] = c;
    if (s.pid > 0 && typeof s.cpu_pct === 'number') p.c[s.name] = Math.round(s.cpu_pct * 10) / 10;
    if (s.pid > 0 && typeof s.mem_gb === 'number') p.m[s.name] = Math.round(s.mem_gb * 1000) / 1000;
  }
  const root = rootDisk(st);
  if (root && typeof root.free_gb === 'number') p.d = root.free_gb;
  if (st.host && typeof st.host.commit_free_gb === 'number') p.cf = Math.round(st.host.commit_free_gb * 100) / 100;
  if (st.jobs_summary) p.q = (st.jobs_summary.running || 0) + (st.jobs_summary.queued || 0);
  hist.push(p);
  histWorst = {};
  histLastAt = now;
  while (hist.length && hist[0].t < now - HIST_WIN) hist.shift();
  if (now - histSavedAt > 30000) histSave();
}
window.addEventListener('pagehide', histSave);

// 30 格，每格 30 秒，从现在往回数（最右一格就是最近 30 秒）；格内取最差状态；没有样本的格留空
function tickCells(pick) {
  const now = Date.now();
  const cells = new Array(TICKS).fill('');
  for (const p of hist) {
    const i = TICKS - 1 - Math.floor(Math.max(0, now - p.t) / TICK_MS);
    if (i < 0 || i >= TICKS) continue;
    const c = pick(p);
    if (c && (!cells[i] || SEV[c] > SEV[cells[i]])) cells[i] = c;
  }
  return cells;
}
function ticksHtml(cells) { return cells.map(c => `<i class="tick${c ? ' ' + c : ''}"></i>`).join(''); }
const TICK_TIP = '每格 30 秒，取这一格里最差的状态。\n绿：正常　琥珀：在等　红：异常　灰：停着　蓝：正在起停\n空格：那段时间这个页面没开着，没有数据。';

// 迷你折线：固定 15 分钟窗口，纵轴从 0 起，参考线（红线、开工线）画在同一个刻度里
function sparkSvg(points, opts) {
  opts = opts || {};
  const W = 100, H = 24;
  const end = Math.floor(Date.now() / HIST_STEP) * HIST_STEP + HIST_STEP, start = end - HIST_WIN;
  const pts = points.filter(p => typeof p.v === 'number' && p.t >= start);
  const refs = (opts.refs || []).filter(r => typeof r.v === 'number' && r.v > 0);
  let max = Math.max(opts.floor || 0, ...pts.map(p => p.v), ...refs.map(r => r.v));
  if (!(max > 0)) max = 1;
  max *= 1.12;
  const x = t => ((t - start) / HIST_WIN) * W;
  const y = v => H - 1 - (Math.max(0, v) / max) * (H - 2);
  let d = '', prevT = 0;
  for (const p of pts) {
    const gap = prevT && p.t - prevT > HIST_STEP * 3.5;   // 中间断过就断开，不连成一条线
    d += (d && !gap ? 'L' : 'M') + x(p.t).toFixed(1) + ' ' + y(p.v).toFixed(1);
    prevT = p.t;
  }
  const refLines = refs.map(r => `<path class="ref ${r.cls}" d="M0 ${y(r.v).toFixed(1)}H${W}"/>`).join('');
  const line = pts.length >= 2 ? `<path class="line" d="${d}"/>` : '';
  return `<svg class="spark" viewBox="0 0 ${W} ${H}" preserveAspectRatio="none" aria-hidden="true">${refLines}${line}</svg>`;
}

// =========================================================================
// 判定：问题清单和整体状态（纯函数，按 DASHBOARD-DESIGN.md 第 3、10 节）
// =========================================================================
const SVC_ORDER = ['database', 'backend', 'rs_server', 'frontend'];
function rootDisk(st) {
  const ds = (st.host && st.host.disks) || [];
  return ds.find(d => String(d.role || '').includes('root')) || ds[0] || null;
}
function systemDisk(st) {
  const ds = (st.host && st.host.disks) || [], r = rootDisk(st);
  return ds.find(d => d !== r && String(d.role || '').includes('system')) || null;
}
function driveLetter(d) { return String((d && d.drive) || '').replace(/[:\\/]+$/, ''); }
function enabledSvcs(st) { return (st.services || []).filter(s => s.enabled !== false && s.state !== 'disabled'); }
function hasProc(s) { return s.pid > 0 || s.state === 'ok' || s.state === 'waiting'; }
function activeJob(st) {
  const j = st && st.job;
  return j && (j.state === 'running' || j.state === 'queued') ? j : null;
}
// 起停任务正在处理的服务：进程刚起来、检查还没过，代理这时报的就是 bad。这段时间它算「启动中」，不算异常、也不算等太久
// （服务表那一行、问题列表、指标卡、刻度都用这一条，和桌面版 row_kind 一致）。任务一结束，还没好的照常报异常。
function svcActing(s, job) { return !!svcBusyWord(s, job); }
function reasonPhrase(s) {
  switch (s.reason_code) {
    case 'not_running': return '进程不在';
    case 'health_failed': return '检查没通过';
    case 'port_taken': return '端口被占';
    case 'unreadable': return '核对不了';
    case 'waiting_storage_version': return '等库版本';
  }
  if (s.state === 'bad') return s.pid > 0 ? '检查没通过' : '进程不在';
  return '';
}
function stateWord(state) {
  return ({ ok: '正常', stopped: '已停止', waiting: '等待', bad: '异常', disabled: '未启用', unknown: '未知' })[state] || state || '—';
}

// 每个服务「这个状态从什么时候开始」：用来算等了多久
let stateSince = {};
function noteStateSince(st) {
  const now = Date.now();
  for (const s of st.services || []) {
    const prev = stateSince[s.name];
    if (prev && prev.state === s.state) continue;
    let at = now;
    if (!prev && s.state === 'waiting') {   // 第一眼看到就在等：从进程起来的时刻算
      const t = parseTs(s.started_at) + clockOffset;
      if (!isNaN(t) && t <= now) at = t;
    }
    stateSince[s.name] = { state: s.state, at };
  }
}
function waitingMs(s) {
  const x = stateSince[s.name];
  return x && x.state === 'waiting' ? Date.now() - x.at : 0;
}

// 运行盘可用空间近 15 分钟的走向，只看采到的点。rate：GB/分钟，只在一路下降且降了超过 1 GB 时才算
function diskTrend() {
  const pts = hist.filter(p => typeof p.d === 'number');
  if (pts.length < 2) return null;
  const a = pts[0], b = pts[pts.length - 1];
  const spanMin = (b.t - a.t) / 60000;
  if (spanMin < 1) return null;
  let mono = true;
  for (let i = 1; i < pts.length; i++) if (pts[i].d > pts[i - 1].d + 0.005) { mono = false; break; }
  const delta = b.d - a.d;
  return { spanMin, delta, rate: mono && delta < -1 ? -delta / spanMin : 0 };
}

let doctorFinding = null;   // 最近一次自检没过时记下：{ jobId, n, canStart }
function deriveProblems(st, trend, live) {
  const out = [], g = st.guard || {};
  const svcTitle = s => s.title || s.name;
  const job = activeJob(st);
  // P1 服务异常（起停任务正在处理的不算，见 svcActing）
  for (const s of enabledSvcs(st)) {
    if (s.state !== 'bad' || svcActing(s, job)) continue;
    out.push({ rank: 1, sev: 'bad', key: 'svc:' + s.name, svc: s.name, text: `${svcTitle(s)} 异常 · ${reasonPhrase(s)}`, sub: s.next || s.reason || '',
      act: { label: `重启 ${svcTitle(s)}`, run: () => restartSingleService(s.name) } });
  }
  // P6 磁盘不够
  const root = rootDisk(st);
  if (root && typeof root.free_gb === 'number') {
    const red = g.min_disk_free_gb > 0 ? g.min_disk_free_gb : 2;
    const eta = trend && trend.rate > 0 ? (root.free_gb - red) / trend.rate : Infinity;
    const dn = driveLetter(root);
    if (root.free_gb < red) out.push({ rank: 2, sev: 'bad', key: 'disk', text: `${dn} 盘只剩 ${fmtGb(root.free_gb)} GB`, sub: `低于红线 ${fmtGb(red)} GB` });
    else if (root.free_gb < 10) out.push({ rank: 2, sev: 'warn', key: 'disk', text: `${dn} 盘只剩 ${fmtGb(root.free_gb)} GB`, sub: `红线 ${fmtGb(red)} GB` });
    else if (eta <= 60) out.push({ rank: 2, sev: 'warn', key: 'disk', text: `${dn} 盘约 ${Math.round(eta)} 分钟到红线`, sub: `现在 ${fmtGb(root.free_gb)} GB，红线 ${fmtGb(red)} GB` });
  }
  const sys = systemDisk(st);
  if (sys && typeof sys.free_gb === 'number') {
    if (sys.free_gb < 1.5) out.push({ rank: 2, sev: 'bad', key: 'sysdisk', text: `系统盘 ${driveLetter(sys)} 只剩 ${fmtGb(sys.free_gb)} GB`, sub: '低于红线 1.5 GB' });
    else if (sys.free_gb < 5) out.push({ rank: 2, sev: 'warn', key: 'sysdisk', text: `系统盘 ${driveLetter(sys)} 只剩 ${fmtGb(sys.free_gb)} GB`, sub: '' });
  }
  // P7 内存吃紧
  const cf = st.host && st.host.commit_free_gb;
  if (typeof cf === 'number') {
    const abortL = g.abort_commit_free_gb > 0 ? g.abort_commit_free_gb : 0, minL = g.min_commit_free_gb > 0 ? g.min_commit_free_gb : 0;
    if (abortL && cf < abortL) out.push({ rank: 3, sev: 'bad', key: 'mem', text: `可提交内存只剩 ${fmtGb(cf)} GB`, sub: `低于收摊线 ${fmtGb(abortL)} GB` });
    else if (minL && cf < minL) out.push({ rank: 3, sev: 'warn', key: 'mem', text: `可提交内存只剩 ${fmtGb(cf)} GB`, sub: `低于开工线 ${fmtGb(minL)} GB` });
    else if (!abortL && !minL && cf < 4) out.push({ rank: 3, sev: 'warn', key: 'mem', text: `可提交内存只剩 ${fmtGb(cf)} GB`, sub: '低于 4 GB（这台没配内存护栏线，这是面板缺省的提醒线）' });
  }
  const be = (st.services || []).find(s => s.name === 'backend');
  if (be && be.pid > 0 && typeof be.mem_gb === 'number' && g.max_backend_gb > 0 && be.mem_gb > g.max_backend_gb) {
    out.push({ rank: 3, sev: 'bad', key: 'bemem', svc: 'backend', text: `后端占用 ${fmtMem(be.mem_gb)}`, sub: `超过上限 ${fmtGb(g.max_backend_gb)} GB` });
  }
  // P3 后端降级
  const bh = st.backend;
  if (be && be.state === 'ok' && bh) {
    const secs = bh.degraded_sections || [];
    if ((bh.status && bh.status !== 'ok') || secs.length) {
      const closureBad = bh.mdb_closure_status && bh.mdb_closure_status !== 'ok';
      const why = closureBad ? '找不到工程目录' : (secs[0] || bh.status);
      out.push({ rank: 5, sev: 'warn', key: 'degraded', svc: 'backend', why, text: `后端降级 · ${why}`, sub: secs.length ? '降级项：' + secs.join('、') : '',
        act: st.urls && st.urls.backend ? { label: '打开后端设置页', href: st.urls.backend } : null });
    }
  }
  // P8 后端任务失败
  const fails = (st.jobs_summary && st.jobs_summary.failures_24h) || [];
  if (fails.length) {
    out.push({ rank: 6, sev: 'bad', key: 'fails', text: `后端任务近 24 小时失败 ${fails.length} 次`, sub: fails[fails.length - 1].message || '',
      act: st.urls && st.urls.ops ? { label: '打开运维面板', href: st.urls.ops } : null });
  }
  if (live) {
    // P2 服务等太久
    for (const s of enabledSvcs(st)) {
      const ms = s.state === 'waiting' && !svcActing(s, job) ? waitingMs(s) : 0;
      if (ms > 180000) out.push({ rank: 7, sev: 'warn', key: 'wait:' + s.name, svc: s.name, text: `${svcTitle(s)} 等了 ${fmtDur(ms)}${reasonPhrase(s) ? ' · ' + reasonPhrase(s) : ''}`, sub: s.next || '' });
    }
    // P9 自检不过
    if (doctorFinding) {
      const id = doctorFinding.jobId;
      out.push({ rank: 8, sev: 'warn', key: 'doctor', text: `自检有 ${doctorFinding.n} 项没过`, sub: '按自检的结论，现在不能启动', act: { label: '看自检结果', run: () => showJob(id) } });
    }
  }
  // P10 两边配置不一致
  const drift = st.drift || [];
  if (drift.length) {
    out.push({ rank: 9, sev: 'warn', key: 'drift', text: `两边配置不一致 ${drift.length} 项`, sub: drift.map(d => d.key).join('、'), act: { label: '看工程配置', run: () => switchView('config') } });
  }
  out.sort((a, b) => (a.sev === b.sev ? a.rank - b.rank : (a.sev === 'bad' ? -1 : 1)));
  return out;
}

function deriveOverall(st, problems) {
  const en = enabledSvcs(st);
  const okN = en.filter(s => s.state === 'ok').length;
  const count = `${okN}/${en.length}`;
  if (st.stack && st.stack.config_valid === false) return { key: 'badconfig', word: '配置不合法', icon: 'x', tone: 'bad', count: '' };
  if (st.stack && st.stack.first_run) return { key: 'unconfigured', word: '未配置', icon: 'dashed', tone: '', count: '' };
  const job = activeJob(st);
  const lockCmd = !job && st.lock && st.lock.held ? String(st.lock.command || '') : '';
  if ((job && ['up', 'start', 'restart'].includes(job.kind)) || /(^|\W)up(\W|$)/.test(lockCmd)) return { key: 'starting', word: '启动中', icon: '', tone: 'busy', count };
  if ((job && ['down', 'stop'].includes(job.kind)) || /(^|\W)down(\W|$)/.test(lockCmd)) return { key: 'stopping', word: '停止中', icon: '', tone: 'busy', count };
  if (!en.some(hasProc)) return { key: 'stopped', word: '已停止', icon: 'square', tone: 'off', count };
  const red = problems.some(p => p.sev === 'bad');
  const degraded = problems.some(p => p.key === 'degraded');
  if (en.length && okN === en.length && !degraded && !red) return { key: 'ok', word: '全部正常', icon: 'check', tone: 'ok', count };
  return { key: 'partial', word: '部分异常', icon: red ? 'x' : 'alert', tone: red ? 'bad' : 'warn', count };
}
function siteInfo(o, st) {
  const health = o.key === 'ok' ? 'ok' : o.key === 'partial' ? (o.tone === 'bad' ? 'bad' : 'warn')
    : o.key === 'badconfig' ? 'bad' : o.key === 'stopped' ? 'stopped' : (o.key === 'starting' || o.key === 'stopping') ? 'busy' : 'idle';
  const word = (o.key === 'ok' || o.key === 'partial') ? o.count : o.word;
  const name = (st.stack && st.stack.name) || (st.project && st.project.name) || '';
  return { health, word, full: o.word + (o.count ? ' ' + o.count : ''), name };
}

// =========================================================================
// 画
// =========================================================================
let curProblems = [];
let activeSiteInfo = null;   // 当前这一台在侧栏里的那一行，和结论栏用同一个判定结果

function svcIcon(name) {
  const paths = {
    database: '<ellipse cx="12" cy="5" rx="8" ry="3"/><path d="M4 5v14c0 1.66 3.58 3 8 3s8-1.34 8-3V5M4 12c0 1.66 3.58 3 8 3s8-1.34 8-3"/>',
    backend: '<rect x="3" y="3" width="18" height="8" rx="2"/><rect x="3" y="13" width="18" height="8" rx="2"/><path d="M7 7h.01M7 17h.01"/>',
    rs_server: '<rect x="9" y="2" width="6" height="6" rx="1"/><rect x="2" y="16" width="6" height="6" rx="1"/><rect x="16" y="16" width="6" height="6" rx="1"/><path d="M5 16v-3a1 1 0 0 1 1-1h12a1 1 0 0 1 1 1v3M12 12V8"/>',
    frontend: '<rect x="2" y="3" width="20" height="14" rx="2"/><path d="M8 21h8M12 17v4"/>'
  };
  return `<svg class="ico" viewBox="0 0 24 24" aria-hidden="true">${paths[name] || '<rect x="4" y="4" width="16" height="16" rx="2"/>'}</svg>`;
}
function flagHtml(text, tone) {
  return tone ? `<span class="flag ${tone}">${icon(tone === 'bad' ? 'x' : 'alert', 'sm')}${esc(text)}</span>` : esc(text);
}
function kpiTone(id, tone) {
  const c = byId(id), cls = 'kpi-card' + (tone ? ' ' + tone : '');
  if (c.className !== cls) c.className = cls;
}
function setLink(id, url) {
  const a = byId(id);
  if (url) {
    if (a.getAttribute('href') !== url) a.setAttribute('href', url);
    a.classList.remove('off');
    a.removeAttribute('aria-disabled');
    a.removeAttribute('tabindex');
    setTip(a, url);
  } else {
    a.setAttribute('href', '#');
    a.classList.add('off');
    a.setAttribute('aria-disabled', 'true');
    a.setAttribute('tabindex', '-1');
    setTip(a, '');
  }
}
function fmtShortTime(s) {
  const t = parseTs(s);
  if (isNaN(t)) return '';
  const d = new Date(t), hm = `${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
  return d.toDateString() === new Date().toDateString() ? hm : `${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ${hm}`;
}
function runAct(act) {
  if (!act) return;
  if (act.href) window.open(act.href, '_blank', 'noopener');
  else if (act.run) act.run();
}
function runProblemAct(i) { runAct((curProblems[i] || {}).act); }

function heroSet(o) {
  const ic = byId('hero-icon');
  const cls = 'hero-icon' + (o.tone ? ' ' + o.tone : '');
  const html = o.tone === 'busy' ? '<span class="spin"></span>' : icon(o.icon || 'dashed');
  if (ic.className !== cls) ic.className = cls;
  if (ic._h !== html) { ic.innerHTML = html; ic._h = html; }
  setText('hero-title', o.word);
  setText('hero-count', o.count || '');
}
function heroFix(act, off, tip) {
  const b = byId('hero-fix-btn');
  if (!act) { b.classList.add('hidden'); b.onclick = null; return; }
  b.classList.remove('hidden');
  const html = esc(act.label) + (act.href ? icon('ext', 'sm') : '');
  if (b._h !== html) { b.innerHTML = html; b._h = html; }
  b.onclick = () => runAct(act);
  setOff('hero-fix-btn', !!off, tip || '');
}

function renderAll() {
  renderConn();
  renderStatus(lastStatus);
}

function renderStatus(st) {
  const cs = connState();
  if (!st) { renderEmpty(cs); Tip.sync(); Pop.sync(); return; }
  const down = cs === 'error' || cs === 'token';
  const trend = diskTrend();
  const problems = deriveProblems(st, trend, true);
  curProblems = problems;
  const o = down ? { key: cs, word: cs === 'token' ? '需要访问令牌' : '拿不到状态', icon: cs === 'token' ? 'key' : 'x', tone: cs === 'token' ? 'warn' : 'bad', count: '' }
    : deriveOverall(st, problems);
  activeSiteInfo = down ? null : siteInfo(o, st);
  const job = activeJob(st);
  const extLock = !job && st.lock && st.lock.held ? st.lock : null;
  const busyTip = job ? `正在跑：${jobLabel(job)}`
    : extLock ? `另一个 ${extLock.command || '命令'} 在跑（进程号 ${extLock.pid || '—'}${extLock.since ? '，起于 ' + fmtDateTime(extLock.since) : ''}）` : '';
  const ctx = { cs, down, o, problems, trend, job, extLock, busyTip, busy: !!(job || extLock) };
  document.querySelectorAll('#kpi-grid .skel').forEach(e => e.classList.remove('skel'));
  renderTop(st);
  renderHero(st, ctx);
  renderSubRow(st, ctx);
  renderJobBanner(job);
  renderKpis(st, ctx);
  renderServices(st, ctx);
  renderFooter(st);
  renderFleetSidebar();
  Tip.sync();
  Pop.sync();
}

// 还没拿到过状态：加载中是骨架，拿不到时写明没有数据（不显示 0）
function renderEmpty(cs) {
  const loading = cs === 'loading';
  const o = cs === 'error' ? { word: '拿不到状态', icon: 'x', tone: 'bad' }
    : cs === 'token' ? { word: '需要访问令牌', icon: 'key', tone: 'warn' }
    : { word: '正在读取状态', icon: 'dashed', tone: '' };
  activeSiteInfo = null;
  curProblems = [];
  heroSet(o);
  setHidden('hero-problem', true);
  heroFix(null);
  setHidden('hero-more', true);
  for (const id of ['btn-up', 'btn-down', 'btn-doctor', 'btn-open-web']) setOff(id, true, loading ? '' : '现在拿不到状态');
  byId('btn-up').classList.remove('primary');
  setOff('queue-toggle', true, '');
  byId('queue-switch').classList.remove('on');
  byId('queue-toggle').setAttribute('aria-checked', 'false');
  setText('queue-desc', '—');
  setHtml('hero-chips', '');
  setLink('link-web', '');
  setLink('link-api', '');
  renderJobBanner(null);
  for (const k of ['svc', 'job', 'disk', 'mem']) {
    byId(`kpi-${k}-val`).classList.toggle('skel', loading);
    setText(`kpi-${k}-val`, loading ? '00.0' : '—');
    setText(`kpi-${k}-unit`, '');
    setHtml(`kpi-${k}-f1`, '');
    setHtml(`kpi-${k}-f2`, '');
    kpiTone('kpi-' + k, '');
  }
  for (const id of ['kpi-svc-ticks', 'kpi-job-ticks', 'kpi-disk-bar', 'kpi-mem-bar']) setHtml(id, '');
  for (const id of ['kpi-job-hint', 'kpi-disk-hint', 'kpi-mem-hint']) setText(id, '');
  setText('kpi-disk-label', '磁盘');
  const tbody = byId('services-tbody');
  const html = loading
    ? '<tr><td colspan="8"><span class="skel skel-bar"></span></td></tr>'.repeat(4)
    : '<tr><td colspan="8" class="empty-cell">还没有拿到过这台服务器的状态</td></tr>';
  if (tbody._h !== html) { tbody.innerHTML = html; tbody._h = html; }
  setText('top-host', activeBase ? hostOf(activeBase) : location.host);
  setHidden('top-proj', true);
  setText('foot-left', '运行目录 —');
  setText('foot-right', '');
  renderFleetSidebar();
}

function renderTop(st) {
  const name = (st.stack && st.stack.name) || '';
  const host = byId('top-host');
  setText('top-host', name || (activeBase ? hostOf(activeBase) : location.host));
  setTip(host, `站点 ${name || '—'}\n${activeBase || location.origin}`);
  const p = st.project, pill = byId('top-proj');
  if (p && p.name) {
    pill.classList.remove('hidden');
    setText('top-proj', p.name);
    setTip(pill, [`工程 ${p.name}`, p.code ? `代号 ${p.code}` : '', p.mdb ? `MDB ${p.mdb}` : '', p.e3d_version ? `E3D ${p.e3d_version}` : '', p.e3d_root || ''].filter(Boolean).join('\n'));
  } else {
    pill.classList.add('hidden');
  }
}

function renderHero(st, ctx) {
  const o = ctx.o;
  heroSet(o);
  // 最要紧的那一个问题点名，旁边就是修的按钮；其余的收进「另 N 项」
  const top = o.key === 'partial' ? ctx.problems[0] : null;
  const prob = byId('hero-problem');
  const ptext = top ? top.text : '';
  const pcls = 'hero-problem' + (top ? ' ' + top.sev : '') + (ptext ? '' : ' hidden');
  if (prob.className !== pcls) prob.className = pcls;
  if (prob.textContent !== ptext) prob.textContent = ptext;
  setTip(prob, top && top.sub ? top.sub : '');
  const moreN = o.key === 'partial' ? ctx.problems.length - 1 : 0;
  setHidden('hero-more', !(moreN > 0));
  if (moreN > 0) setText('hero-more', `另 ${moreN} 项`);

  let act = null, actOff = false, actTip = '';
  if (o.key === 'unconfigured') act = { label: '开始配置…', run: () => openSiteModal() };
  else if (o.key === 'badconfig') act = { label: '改配置…', run: () => switchView('config') };
  else if (top && top.act) {
    act = top.act;
    if (ctx.busy && !act.href) { actOff = true; actTip = ctx.busyTip; }
  }
  heroFix(act, actOff, actTip);

  const anyProc = enabledSvcs(st).some(hasProc);
  let upOff = false, upTip = '', downOff = false, downTip = '', docOff = false, docTip = '';
  if (ctx.down) { upOff = downOff = docOff = true; upTip = downTip = docTip = '现在拿不到状态'; }
  else if (ctx.busy) {
    upOff = downOff = docOff = true; upTip = downTip = docTip = ctx.busyTip;
    if (o.key === 'starting' && ctx.job) downTip = '启动任务不能中途取消';
  }
  else if (o.key === 'unconfigured') { upOff = downOff = true; upTip = downTip = '先完成配置'; }
  else if (o.key === 'badconfig') { upOff = true; upTip = '配置不合法，先改配置'; downOff = !anyProc; downTip = anyProc ? '' : '已经都停了'; }
  else if (o.key === 'stopped') { downOff = true; downTip = '已经都停了'; }
  else if (o.key === 'ok') { upOff = true; upTip = '都在运行'; }
  else { downOff = !anyProc; downTip = anyProc ? '' : '已经都停了'; }
  setOff('btn-up', upOff, upTip);
  setOff('btn-down', downOff, downTip);
  setOff('btn-doctor', docOff, docTip || '检查配置和运行环境，看现在能不能启动');
  byId('btn-up').classList.toggle('primary', o.key === 'stopped' && !upOff);
  const fe = (st.services || []).find(s => s.name === 'frontend');
  const feOk = !!(fe && fe.state === 'ok') && !ctx.down;
  setOff('btn-open-web', !feOk, feOk ? ((st.urls && st.urls.frontend) || '') : '前端没在跑');
}

function renderSubRow(st, ctx) {
  const bh = st.backend, tog = byId('queue-toggle'), sw = byId('queue-switch');
  if (!bh || ctx.down) {
    sw.classList.remove('on');
    tog.setAttribute('aria-checked', 'false');
    setOff('queue-toggle', true, ctx.down ? '现在拿不到状态' : '后端没在跑，现在改不了');
    setText('queue-desc', '—');
  } else {
    const on = !bh.queue_paused;
    sw.classList.toggle('on', on);
    tog.setAttribute('aria-checked', on ? 'true' : 'false');
    setOff('queue-toggle', false, on ? '开着：队列正常出队。点一下暂停。' : '已暂停：排队的任务先不跑。点一下恢复。');
    setText('queue-desc', on ? '开' : '已暂停');
  }
  const chips = [];
  const drift = st.drift || [];
  if (drift.length) chips.push(`<button type="button" class="chip warn" aria-haspopup="true" aria-expanded="false" onclick="openDriftList(this)">${icon('alert', 'sm')}配置不一致 ${drift.length}</button>`);
  if (doctorFinding) chips.push(`<button type="button" class="chip warn" onclick="showJob(${Number(doctorFinding.jobId)})">${icon('alert', 'sm')}自检 ${doctorFinding.n} 项</button>`);
  if (ctx.extLock) chips.push(`<span class="chip" tabindex="0" data-tip="${esc(ctx.busyTip + '\n等它结束后才能启停。')}">${icon('clock', 'sm')}另一个 ${esc(ctx.extLock.command || '命令')} 在跑</span>`);
  setHtml('hero-chips', chips.join(''));
  setLink('link-web', st.urls && st.urls.frontend);
  setLink('link-api', st.urls && st.urls.backend);
}
function openDriftList(anchor) {
  const drift = (lastStatus && lastStatus.drift) || [];
  const rows = drift.map(d => `<div class="pop-row warn">${icon('alert', 'sm')}<span class="pop-row-text">${esc(d.key)}<span class="pop-row-sub">应当是 ${esc(d.want)}，现在是 ${esc(d.have)}</span></span></div>`).join('');
  Pop.open(anchor, `<div class="pop-title">面板配置和后端配置有 ${drift.length} 项不一致</div>${rows}<div class="pop-sep"></div><button type="button" class="pop-item" onclick="switchView('config')">${icon('file')}去工程配置</button>`, {});
}
function openProblemList(anchor) {
  const rows = curProblems.map((p, i) => `<div class="pop-row ${p.sev}">${icon(p.sev === 'bad' ? 'x' : 'alert', 'sm')}`
    + `<span class="pop-row-text">${esc(p.text)}${p.sub ? `<span class="pop-row-sub">${esc(p.sub)}</span>` : ''}</span>`
    + (p.act ? `<button type="button" class="btn-sm" data-pop-close onclick="runProblemAct(${i})">${esc(p.act.label)}${p.act.href ? icon('ext', 'sm') : ''}</button>` : '')
    + '</div>').join('');
  Pop.open(anchor, `<div class="pop-title">现在有 ${curProblems.length} 个问题</div>${rows}`, {});
}

function renderJobBanner(job) {
  setHidden('job-banner', !job);
  if (!job) return;
  setText('job-title', jobLabel(job));
  const p = job.progress, hasBar = !!(p && p.total > 0);
  let detail = job.state === 'queued' ? '排队中' : cjkSpace(job.message || '');
  if (hasBar) detail = `${p.done}/${p.total}${p.step ? ' · ' + p.step : ''}`;
  setText('job-detail', detail);
  setHidden('job-bar', !hasBar);
  if (hasBar) byId('job-bar-fill').style.width = Math.round(Math.min(1, p.done / p.total) * 100) + '%';
  const t0 = parseTs(job.started_at);
  setText('job-elapsed', isNaN(t0) ? '' : '用时 ' + fmtSpan(srvNow() - t0));
  const who = jobWho(job);
  setTip(byId('job-banner'), who ? `发起：${who}` : '');
}
// 代理给的任务名和结果里，中文和英文、数字常贴在一起（「重启rs-server完成」）：显示时补一个空格，和页面其它地方的写法一致
function cjkSpace(s) {
  return String(s == null ? '' : s).replace(/([\u4e00-\u9fff])([A-Za-z0-9])/g, '$1 $2').replace(/([A-Za-z0-9])([\u4e00-\u9fff])/g, '$1 $2');
}
function jobLabel(j) { return cjkSpace((j && (j.label || JOB_NAME[j.kind] || j.kind)) || ''); }
// 谁发起的：代理没记到来源时会给一句括号里的说明，那不是名字，不显示
function jobWho(j) {
  const by = j.by && !/^[（(]/.test(j.by) ? j.by : '';
  return [by, j.from].filter(Boolean).join(' · ');
}

function renderKpis(st, ctx) {
  const o = ctx.o, g = st.guard || {};
  const unconf = o.key === 'unconfigured';
  const en = enabledSvcs(st);
  const okN = en.filter(s => s.state === 'ok').length;

  // —— 服务：正常数 / 启用数 ——
  setText('kpi-svc-val', unconf ? '—' : `${okN} / ${en.length}`);
  setText('kpi-svc-unit', unconf ? '' : '正常');
  const names = new Set(en.map(s => s.name));
  setHtml('kpi-svc-ticks', ticksHtml(tickCells(p => {
    let w = '';
    for (const n in p.s) if (names.has(n) && (!w || SEV[p.s[n]] > SEV[w])) w = p.s[n];
    return w;
  })));
  setTip(byId('kpi-svc-ticks'), TICK_TIP);
  // 起停任务正在处理的服务不算异常、不算在等（见 svcActing）：这一格只说它在起停
  const acting = en.filter(s => svcActing(s, ctx.job));
  const idle = s => !acting.includes(s);
  const badSvc = en.filter(s => s.state === 'bad' && idle(s)), waitSvc = en.filter(s => s.state === 'waiting' && idle(s));
  const ttl = list => list.map(s => s.title || s.name).join('、');
  let f1 = '', f1tone = '';
  if (unconf) f1 = '还没配置';
  else if (badSvc.length) { f1 = ttl(badSvc) + ' 异常'; f1tone = 'bad'; }
  else if (acting.length) {
    const cur = acting.filter(s => svcBusyWord(s, ctx.job) !== '排队');   // 排队的不列，只说正在处理的那个
    const word = cur.length ? svcBusyWord(cur[0], ctx.job) : '启动中';
    f1 = cur.length === 1 ? `${ttl(cur)} ${word}` : word;
  }
  else if (o.key === 'stopped') f1 = '都停着';
  else if (waitSvc.length) f1 = ttl(waitSvc) + ' 在等';
  else if (en.length && okN === en.length) f1 = '全部正常';
  else f1 = ttl(en.filter(s => s.state !== 'ok')) + ' 没在跑';
  setHtml('kpi-svc-f1', flagHtml(f1, f1tone));
  const ups = en.filter(s => s.state === 'ok').map(s => parseTs(s.started_at)).filter(t => !isNaN(t));
  setHtml('kpi-svc-f2', en.length && ups.length === en.length ? '已运行 ' + esc(fmtDur(srvNow() - Math.max(...ups))) : '');
  setTip(byId('kpi-svc-f2'), '整套一起运行了多久：从最晚起来的那个服务算起');
  kpiTone('kpi-svc', badSvc.length ? 'bad' : '');

  // —— 后端任务：在跑 / 排队 / 近 24 小时失败 ——
  const js = st.jobs_summary;
  if (!js || unconf) {
    setText('kpi-job-val', '—');
    setText('kpi-job-unit', '');
    setText('kpi-job-hint', '');
    setTip(byId('kpi-job-hint'), '');
    setHtml('kpi-job-ticks', '');
    setHtml('kpi-job-f1', unconf ? '还没配置' : '后端没在跑');
    setHtml('kpi-job-f2', '');
    kpiTone('kpi-job', '');
  } else {
    const fails = js.failures_24h || [];
    setText('kpi-job-val', String(js.running || 0));
    setText('kpi-job-unit', `在跑 · ${js.queued || 0} 排队`);
    const hint = [];
    if (js.paused) hint.push('已暂停出队');
    if (js.held > 0) hint.push(`挂起 ${js.held}`);
    setText('kpi-job-hint', hint.join(' · '));
    setTip(byId('kpi-job-hint'), js.held > 0 ? `${js.held} 条重扫积压挂在队里：等对应的库来一次增量才会跑，没人触发不会动。` : '');
    setHtml('kpi-job-ticks', sparkSvg(hist.map(p => ({ t: p.t, v: p.q })), { floor: 3 }));
    setTip(byId('kpi-job-ticks'), '近 15 分钟在跑加排队的任务数');
    const row = (js.rows || []).find(r => r.state === 'running');
    let jf1 = '空闲';
    if (js.upgrade && js.upgrade.total > 0) jf1 = `库升级 ${js.upgrade.done}/${js.upgrade.total}`;
    else if (row) jf1 = `库 ${row.dbnum} · ${row.intent || ''}${row.phase ? ' · ' + row.phase : ''}`;
    else if ((js.queued || 0) > 0) jf1 = js.paused ? '排着队，暂停中' : '排着队';
    setHtml('kpi-job-f1', esc(jf1));
    setHtml('kpi-job-f2', fails.length
      ? `<button type="button" class="flag bad" aria-haspopup="true" aria-expanded="false" onclick="openFailList(this)">${icon('x', 'sm')}失败 ${fails.length}</button>` : '');
    kpiTone('kpi-job', fails.length ? 'bad' : '');
  }

  // —— 磁盘：运行目录所在盘还剩多少，离红线多远 ——
  const root = rootDisk(st), sys = systemDisk(st);
  const diskP = ctx.problems.filter(p => p.key === 'disk' || p.key === 'sysdisk');
  if (!root || typeof root.free_gb !== 'number') {
    setText('kpi-disk-label', '磁盘');
    setText('kpi-disk-hint', '');
    setText('kpi-disk-val', '—');
    setText('kpi-disk-unit', '');
    setHtml('kpi-disk-bar', '');
    setHtml('kpi-disk-f1', '代理没报磁盘');
    setHtml('kpi-disk-f2', '');
    kpiTone('kpi-disk', '');
  } else {
    const red = g.min_disk_free_gb > 0 ? g.min_disk_free_gb : 2;
    setText('kpi-disk-label', `磁盘 ${root.drive}`);
    setTip(byId('kpi-disk-label'), '运行目录所在的盘' + (sys ? `\n系统盘 ${sys.drive} 还剩 ${fmtGb(sys.free_gb)} GB` : (String(root.role || '').includes('system') ? '，也是系统盘' : '')));
    setText('kpi-disk-hint', `红线 ${fmtGb(red)} GB`);
    setTip(byId('kpi-disk-hint'), g.min_disk_free_gb > 0 ? '这台服务器配的磁盘护栏线：可用空间低于它就是红色' : '这台没配磁盘护栏线，按缺省的 2 GB 算');
    setText('kpi-disk-val', fmtGb(root.free_gb));
    setText('kpi-disk-unit', 'GB 可用');
    setHtml('kpi-disk-bar', sparkSvg(hist.map(p => ({ t: p.t, v: p.d })), { refs: [{ v: red, cls: 'bad' }] }));
    setTip(byId('kpi-disk-bar'), '近 15 分钟的可用空间；红色虚线是红线');
    const tr = ctx.trend;
    let df1 = '', dtone = '';
    if (root.free_gb < red) { df1 = '低于红线'; dtone = 'bad'; }
    else if (tr && tr.rate > 0) {
      const eta = (root.free_gb - red) / tr.rate;
      if (eta <= 360) { df1 = eta <= 90 ? `约 ${Math.round(eta)} 分钟到红线` : `约 ${(eta / 60).toFixed(1)} 小时到红线`; if (eta <= 60) dtone = 'warn'; }
    }
    if (!df1 && tr) {
      const m = Math.max(1, Math.round(tr.spanMin));
      df1 = Math.abs(tr.delta) < 0.05 ? `近 ${m} 分钟没变` : `近 ${m} 分钟 ${tr.delta > 0 ? '+' : '−'}${Math.abs(tr.delta).toFixed(1)} GB`;
    }
    if (!dtone && root.free_gb >= red && root.free_gb < 10) { dtone = 'warn'; if (!df1) df1 = '不到 10 GB'; }
    setHtml('kpi-disk-f1', flagHtml(df1, dtone));
    const sysP = diskP.find(p => p.key === 'sysdisk');
    setHtml('kpi-disk-f2', sysP ? flagHtml(`系统盘 ${sys.drive} 剩 ${fmtGb(sys.free_gb)} GB`, sysP.sev)
      : (root.total_gb > 0 ? `已用 ${Math.round((1 - root.free_gb / root.total_gb) * 100)}%` : ''));
    kpiTone('kpi-disk', diskP.some(p => p.sev === 'bad') ? 'bad' : (diskP.length ? 'warn' : ''));
  }

  // —— 内存：可提交余量，离开工线 / 收摊线多远 ——
  const cf = st.host && st.host.commit_free_gb;
  const memP = ctx.problems.filter(p => p.key === 'mem' || p.key === 'bemem');
  const minL = g.min_commit_free_gb > 0 ? g.min_commit_free_gb : 0, abortL = g.abort_commit_free_gb > 0 ? g.abort_commit_free_gb : 0;
  if (typeof cf !== 'number') {
    setText('kpi-mem-val', '—');
    setText('kpi-mem-unit', '');
    setText('kpi-mem-hint', '');
    setHtml('kpi-mem-bar', '');
    setHtml('kpi-mem-f1', '代理没报内存');
    setHtml('kpi-mem-f2', '');
    kpiTone('kpi-mem', '');
  } else {
    setText('kpi-mem-val', fmtGb(cf));
    setText('kpi-mem-unit', 'GB 可提交');
    setTip(byId('kpi-mem-unit'), '可提交余量：系统还能再答应出去多少内存（物理内存加页面文件，减去已经答应的）。后端按它决定开不开工。');
    const hintEl = byId('kpi-mem-hint');
    if (minL || abortL) {
      setText('kpi-mem-hint', minL ? `开工线 ${fmtGb(minL)} GB` : `收摊线 ${fmtGb(abortL)} GB`);
      setTip(hintEl, [minL ? `开工线 ${fmtGb(minL)} GB：余量低于它，新任务先不开工。` : '', abortL ? `收摊线 ${fmtGb(abortL)} GB：余量低于它，在跑的任务会停下来。` : ''].filter(Boolean).join('\n'));
    } else {
      setText('kpi-mem-hint', '提醒线 4 GB');
      setTip(hintEl, '这台没配内存护栏线，这是面板缺省的提醒线。');
    }
    const refs = (minL || abortL) ? [{ v: minL, cls: 'warn' }, { v: abortL, cls: 'bad' }] : [{ v: 4, cls: 'warn' }];
    setHtml('kpi-mem-bar', sparkSvg(hist.map(p => ({ t: p.t, v: p.cf })), { refs }));
    setTip(byId('kpi-mem-bar'), '近 15 分钟的可提交余量；琥珀虚线是开工线，红色虚线是收摊线');
    const be = (st.services || []).find(s => s.name === 'backend');
    const beOver = memP.some(p => p.key === 'bemem');
    const memLow = memP.find(p => p.key === 'mem');
    let mf1;
    if (memLow) mf1 = flagHtml(memLow.sub.replace(/（.*$/, ''), memLow.sev);
    else if (be && be.pid > 0 && typeof be.mem_gb === 'number') mf1 = flagHtml('后端 ' + fmtMem(be.mem_gb) + (beOver ? `，超过上限 ${fmtGb(g.max_backend_gb)} GB` : ''), beOver ? 'bad' : '');
    else mf1 = '后端没在跑';
    setHtml('kpi-mem-f1', mf1);
    setHtml('kpi-mem-f2', st.host.mem_total_gb > 0 ? `全机 ${fmtGb(st.host.mem_total_gb)} GB` : '');
    kpiTone('kpi-mem', memP.some(p => p.sev === 'bad') ? 'bad' : (memP.length ? 'warn' : ''));
  }
}
function openFailList(anchor) {
  const st = lastStatus;
  const fails = (st && st.jobs_summary && st.jobs_summary.failures_24h) || [];
  const rows = fails.slice().reverse().map(f => `<div class="pop-row bad">${icon('x', 'sm')}<span class="pop-row-text">${esc(fmtShortTime(f.at))}　库 ${esc(f.dbnum)}<span class="pop-row-sub">${esc(f.message || '')}</span></span></div>`).join('');
  const ops = st && st.urls && st.urls.ops;
  Pop.open(anchor, `<div class="pop-title">近 24 小时失败 ${fails.length} 次</div>${rows}`
    + (ops ? `<div class="pop-sep"></div><a class="pop-item" href="${esc(ops)}" target="_blank" rel="noopener">${icon('ext')}打开运维面板</a>` : ''), { align: 'end' });
}

// —— 服务表 ——
// 起停期间这一行在干什么（协议里没有「启动中」这个状态，从任务推出来）
function svcBusyWord(s, job) {
  if (!job || s.enabled === false || s.state === 'disabled') return '';
  if (job.service) {
    if (job.service !== s.name) return '';
    if (job.kind === 'stop') return s.state === 'stopped' ? '' : '停止中';
    if (job.kind === 'restart') return '重启中';
    if (job.kind === 'start') return s.state === 'ok' ? '' : '启动中';
    return '';
  }
  if (job.kind === 'up') {
    if (s.state === 'ok') return '';
    const step = job.progress && job.progress.step ? String(job.progress.step) : '';
    if (step && !(step.includes(s.title || s.name) || step.includes(s.name))) return '排队';
    return '启动中';
  }
  if (job.kind === 'down') return s.state === 'stopped' ? '' : '停止中';
  return '';
}
function svcStateCell(s, ctx, busyWord) {
  const tipParts = [];
  if (s.checked) tipParts.push('查的是：' + s.checked);
  if (s.reason) tipParts.push('结果：' + s.reason);
  if (s.next) tipParts.push('下一步：' + s.next);
  let inner;
  const degr = s.name === 'backend' ? ctx.problems.find(p => p.key === 'degraded') : null;
  if (busyWord === '排队') inner = '<span class="st-why">排队</span>';
  else if (busyWord) inner = `<span class="spin"></span>${busyWord}`;
  else if (s.enabled === false || s.state === 'disabled') inner = '<span class="st-pill dis">未启用</span>';
  else if (s.state === 'ok' && degr) {
    inner = `<span class="st-pill warn">${icon('alert')}降级</span><span class="st-why">${esc(degr.why || '')}</span>`;
    if (degr.sub) tipParts.push(degr.sub);
  }
  else if (s.state === 'ok') inner = '<span class="dot ok"></span>正常';
  else if (s.state === 'bad') inner = `<span class="st-pill bad">${icon('x')}异常</span><span class="st-why">${esc(reasonPhrase(s))}</span>`;
  else if (s.state === 'waiting') {
    const ms = waitingMs(s);
    inner = ms > 180000
      ? `<span class="st-pill warn">${icon('alert')}等待</span><span class="st-why">${esc(reasonPhrase(s) || fmtDur(ms))}</span>`
      : `${icon('clock')}等待<span class="st-dur">${esc(fmtSpan(ms))}</span>`;   // 中间的「·」由样式加，见 .st-dur
  }
  else if (s.state === 'stopped') inner = `<span class="st-pill off">${icon('square')}已停止</span>`;
  else inner = `<span class="st-pill off">${esc(stateWord(s.state))}</span><span class="st-why">${esc(reasonPhrase(s))}</span>`;
  const tip = tipParts.join('\n');
  return `<span class="st"${tip ? ` tabindex="0" data-tip="${esc(tip)}"` : ''}>${inner}</span>`;
}
function svcRow(s, st, ctx) {
  const n = esc(s.name), title = s.title || s.name;
  const enabled = s.enabled !== false && s.state !== 'disabled';
  const proc = s.pid > 0;
  const g = st.guard || {};
  const busyWord = svcBusyWord(s, ctx.job);
  const dash = '<span class="dash">—</span>';

  // 进程号单独包一层：手机上地方不够，这一段不显示（样式里的 .svc-pid）
  const port = s.port ? '端口 ' + s.port : '';
  const pid = enabled ? (proc ? 'PID ' + s.pid : 'PID —') : '';
  const subHtml = esc(port) + (pid ? `<span class="svc-pid">${port ? ' · ' : ''}${esc(pid)}</span>` : '') || '—';
  const subTip = proc && s.ours === false ? '这个进程不是本套起的' : '';
  const c0 = `<div class="svc-name"><span class="svc-ico">${svcIcon(s.name)}</span><span class="svc-text">`
    + `<button type="button" class="svc-ttl" data-log="${n}" data-tip="看 ${esc(title)} 的日志">${esc(title)}</button>`
    + `<span class="svc-sub"${subTip ? ` data-tip="${subTip}"` : ''}>${subHtml}</span></span></div>`;

  const c1 = svcStateCell(s, ctx, busyWord);
  const c2 = enabled ? `<div class="ticks mini" data-tip="${esc(TICK_TIP)}">${ticksHtml(tickCells(p => p.s[s.name] || ''))}</div>` : dash;
  const c3 = proc && typeof s.cpu_pct === 'number'
    ? `<span class="metric">${sparkSvg(hist.map(p => ({ t: p.t, v: p.c[s.name] })), { floor: 10 })}<span class="metric-val">${fmtPct(s.cpu_pct)}</span></span>` : dash;
  const over = s.name === 'backend' && g.max_backend_gb > 0 && s.mem_gb > g.max_backend_gb;
  const c4 = proc && typeof s.mem_gb === 'number'
    ? `<span class="metric"${over ? ` data-tip="超过上限 ${fmtGb(g.max_backend_gb)} GB"` : ''}>${sparkSvg(hist.map(p => ({ t: p.t, v: p.m[s.name] })), { floor: 0.1 })}<span class="metric-val${over ? ' bad' : ''}">${fmtMem(s.mem_gb)}</span></span>` : dash;
  const t0 = parseTs(s.started_at);
  const c5 = proc && !isNaN(t0) ? `<span data-tip="起于 ${esc(fmtDateTime(s.started_at))}">${esc(fmtDur(srvNow() - t0))}</span>` : dash;

  const vEntry = svcVersions[s.name];
  const inst = vEntry && vEntry.installed ? vEntry.installed : null;
  const localCmp = localComparisons[s.name];
  const rawDiff = localCmp ? localCmp.diff : (vEntry ? vEntry.diff : null);
  let badge = '';
  if (rawDiff) {
    const dm = diffUiMeta(rawDiff);
    badge = `<button type="button" class="ver-badge ${dm.cssCls}" data-up="${n}" data-tip="${esc(dm.summary || dm.label)}">${esc((localCmp ? '本地包：' : '待推：') + dm.label)}</button>`;
  } else if (vEntry && vEntry.backup && vEntry.backup.exists) {
    badge = `<button type="button" class="ver-badge" data-up="${n}" data-tip="有一份升级前的备份，可以回滚">可回滚</button>`;
  }
  const verMain = s.version || (inst && inst.version) || '—';
  const subParts = [];
  if (inst && inst.build_id) subParts.push(inst.build_id);
  if (inst && inst.short_sha) subParts.push('#' + inst.short_sha);
  if (inst && inst.bytes) subParts.push((inst.bytes / 1048576).toFixed(1) + ' MB');
  const c6 = `<div class="ver"><div class="ver-line"><span class="ver-main">${esc(verMain)}</span>${badge}</div>`
    + (subParts.length ? `<div class="ver-sub">${esc(subParts.join(' · '))}</div>` : '') + '</div>';

  // 操作：一行一个最可能要点的按钮，其余收进「更多」。停止是破坏性的，不摊在每一行上
  const off = ctx.busy || ctx.down;
  const offAttr = off ? ` aria-disabled="true" data-tip="${esc(ctx.down ? '现在拿不到状态' : ctx.busyTip)}"` : '';
  let acts = '';
  if (enabled) {
    // 「日志」只在异常时出现，放最左：主按钮和「更多」在各行的位置对齐，不被它挤动
    if (s.state === 'bad' && !busyWord) acts += `<button type="button" class="btn-sm ghost" data-log="${n}">日志</button>`;
    acts += s.state === 'stopped'
      ? `<button type="button" class="btn-sm" data-act="start" data-svc="${n}"${offAttr}>${icon('play', 'sm')}启动</button>`
      : `<button type="button" class="btn-sm" data-act="restart" data-svc="${n}"${offAttr}>${icon('restart', 'sm')}重启</button>`;
  }
  acts += `<button type="button" class="btn-sm ghost icon-only" data-act="menu" data-svc="${n}" aria-haspopup="menu" aria-expanded="false" aria-label="${esc(title)} 的更多操作">${icon('more')}</button>`;
  const c7 = `<div class="row-acts">${acts}</div>`;

  return {
    key: s.name,
    cls: 'svc-row' + (s.name === logSource ? ' selected' : '') + (ctx.o.key === 'unconfigured' ? ' dim' : ''),
    cells: [
      { cls: 'col-svc', html: c0 }, { cls: 'col-state', html: c1 }, { cls: 'col-hist', html: c2 }, { cls: 'col-cpu num', html: c3 },
      { cls: 'col-mem num', html: c4 }, { cls: 'col-up', html: c5 }, { cls: 'col-ver', html: c6 }, { cls: 'col-act', html: c7 }
    ]
  };
}
// 按格子打补丁：只有内容变了的格子才重写，按钮上的焦点和悬停不会被每秒的刷新打断
function patchRows(tbody, rows) {
  const have = tbody.children;
  let same = have.length === rows.length && !!tbody._rows;
  if (same) for (let i = 0; i < rows.length; i++) if (have[i].dataset.svc !== rows[i].key || have[i].children.length !== rows[i].cells.length) { same = false; break; }
  if (!same) {
    tbody.innerHTML = rows.map(r => `<tr class="${r.cls}" data-svc="${esc(r.key)}">${r.cells.map(c => `<td class="${c.cls}">${c.html}</td>`).join('')}</tr>`).join('');
    tbody._rows = rows.map(r => r.cells.map(c => c.html));
    tbody._h = null;
    return;
  }
  rows.forEach((r, i) => {
    const tr = have[i], prev = tbody._rows[i];
    if (tr.className !== r.cls) tr.className = r.cls;
    r.cells.forEach((c, j) => { if (prev[j] !== c.html) { tr.children[j].innerHTML = c.html; prev[j] = c.html; } });
  });
}
function renderServices(st, ctx) {
  const idx = s => { const i = SVC_ORDER.indexOf(s.name); return i < 0 ? 99 : i; };
  const svcs = (st.services || []).slice().sort((a, b) => idx(a) - idx(b));
  const tbody = byId('services-tbody');
  if (!svcs.length) {
    const html = '<tr><td colspan="8" class="empty-cell">这套里还没有服务</td></tr>';
    if (tbody._h !== html) { tbody.innerHTML = html; tbody._h = html; tbody._rows = null; }
    return;
  }
  if (tbody._h) tbody._rows = null;
  patchRows(tbody, svcs.map(s => svcRow(s, st, ctx)));
}
function openSvcMenu(name, anchor) {
  const st = lastStatus;
  const s = ((st && st.services) || []).find(x => x.name === name);
  if (!s) return;
  const n = esc(name);
  const busy = !!(activeJob(st) || (st.lock && st.lock.held)) || connState() === 'error';
  const off = busy ? ' aria-disabled="true"' : '';
  const enabled = s.enabled !== false && s.state !== 'disabled';
  const items = [`<button type="button" class="pop-item" role="menuitem" onclick="focusLog('${n}')">${icon('file')}看日志</button>`];
  if (enabled) {
    items.push(s.state === 'stopped'
      ? `<button type="button" class="pop-item" role="menuitem"${off} onclick="startSingleService('${n}')">${icon('play')}启动</button>`
      : `<button type="button" class="pop-item" role="menuitem"${off} onclick="restartSingleService('${n}')">${icon('restart')}重启</button>`);
    if (hasProc(s)) items.push(`<button type="button" class="pop-item danger" role="menuitem"${off} onclick="stopSingleService('${n}')">${icon('square')}停止…</button>`);
  }
  items.push('<div class="pop-sep"></div>');
  items.push(`<button type="button" class="pop-item" role="menuitem" onclick="openUpgradeModal('${n}')">${icon('upload')}版本与升级…</button>`);
  Pop.open(anchor, items.join(''), { align: 'end', role: 'menu' });
}

function renderFooter(st) {
  setText('foot-left', '运行目录 ' + ((st.stack && st.stack.root) || '—'));
  const parts = [];
  if (st.stack && st.stack.version) parts.push('stack ' + st.stack.version);
  if (st.agent_version) parts.push('代理 ' + st.agent_version);
  if (st.backend_exe_note) parts.push(st.backend_exe_note);
  setText('foot-right', parts.join(' · '));
}

// =========================================================================
// 状态轮询：长轮询。换服务器、换令牌时旧的请求直接作废，不会把上一台的数据画到这一台上
// =========================================================================
let statusEpoch = 0, statusAbort = null;
function restartStatusPoll() {
  statusEpoch++;
  if (statusAbort) { try { statusAbort.abort(); } catch (_) {} }
  pollStatus(statusEpoch);
}
async function pollStatus(epoch) {
  while (epoch === statusEpoch) {
    if (needsToken) return;   // 等用户给了令牌，saveToken 会重新起
    const ctl = new AbortController();
    statusAbort = ctl;
    let timedOut = false;
    const timer = setTimeout(() => { timedOut = true; ctl.abort(); }, STATUS_TIMEOUT);
    try {
      const res = await api(`/v1/status?since=${lastSeq}`, { signal: ctl.signal });
      clearTimeout(timer);
      if (epoch !== statusEpoch) return;
      if (!res.notModified && !Array.isArray(res.services)) {
        const err = new Error('代理返回的内容不认识（可能连到了别的程序，或版本对不上）');
        err.unknownShape = true;
        throw err;
      }
      conn.lastOkAt = Date.now(); conn.fails = 0; conn.lastErr = '';
      if (res.notModified) { histSample(); renderAll(); }
      else { lastSeq = res.seq || 0; ingestStatus(res); }
    } catch (e) {
      clearTimeout(timer);
      if (epoch !== statusEpoch) return;
      if (e && e.status === 401) { renderAll(); return; }
      conn.fails++;
      conn.lastErr = connErrText(e, timedOut);
      renderAll();
      await sleep(Math.min(8000, 1500 + conn.fails * 1000));
      continue;
    }
    await sleep(400);
  }
}

// 一份新快照到了：记历史、看有没有任务刚做完、重画
const myJobs = new Set();   // 这个页面自己发起的任务号
let jobWatchReady = false, lastJobId = null;
function ingestStatus(st) {
  lastStatus = st;
  const g = parseTs(st.generated_at);
  if (!isNaN(g)) { const off = Date.now() - g; clockOffset = Math.abs(off) > 90000 ? off : 0; }
  noteStateSince(st);
  histNote(st);
  histSample();
  const lj = st.last_job, id = lj ? String(lj.id) : null;
  if (!jobWatchReady) { jobWatchReady = true; lastJobId = id; seedDoctor(); }
  else if (id && id !== lastJobId) { lastJobId = id; onJobFinished(lj); }
  renderAll();
}
// 刚连上一台服务器时：看最近一次自检是不是「不能启动」。那之后已经成功起过服务的，旧结论不再算数
async function seedDoctor() {
  const base = activeBase;
  try {
    const res = await api('/v1/jobs');
    if (base !== activeBase) return;
    const jobs = (res.jobs || []).slice().sort((a, b) => Number(b.id) - Number(a.id));
    for (const j of jobs) {
      if (j.state !== 'done') continue;
      if (['up', 'start', 'restart'].includes(j.kind) && !j.exit_code) return;
      if (j.kind !== 'doctor') continue;
      const d = j.result && j.result.doctor;
      if (d && d.can_start === false) {
        doctorFinding = { jobId: j.id, n: (d.problems || []).length };
        if (lastStatus) renderStatus(lastStatus);
      }
      return;
    }
  } catch (_) {}
}
function onJobFinished(j) {
  const mine = myJobs.delete(String(j.id));
  const label = jobLabel(j), msg = cjkSpace(j.message || '');
  if (curView === 'jobs') {
    loadJobs();
    if (selectedJobId === String(j.id)) viewJobOut(j.id);   // 正看着的那个任务刚做完：把它的结果补上
  }
  if (['up', 'down', 'start', 'stop', 'restart'].includes(j.kind)) setTimeout(() => loadServiceVersions(true), 1500);
  if (j.kind === 'doctor') { reportDoctor(j, mine); return; }
  if (j.state === 'done' && !j.exit_code) { if (mine) toast(msg || cjkSpace(`${label}完成`), 'ok'); }
  else if (j.state === 'cancelled') { if (mine) toast(cjkSpace(`${label}已取消`), 'info'); }
  else toast(cjkSpace(`${label}没做成`) + (msg ? '：' + msg : ''), 'bad', { actionLabel: '看输出', onAction: () => showJob(j.id) });
}
async function reportDoctor(j, mine) {
  const see = { actionLabel: '看结果', onAction: () => showJob(j.id) };
  try {
    const full = await api('/v1/jobs/' + j.id);
    const d = full && full.result && full.result.doctor;
    if (!d) { if (mine) toast(j.message || '自检做完了', j.exit_code ? 'bad' : 'ok', see); return; }
    const n = (d.problems || []).length;
    // 只有「不能启动」才算一个问题挂在页面上；能启动但有要留意的项，只在结果里说
    doctorFinding = d.can_start === false ? { jobId: j.id, n } : null;
    if (mine) {
      if (d.can_start === false) toast(`自检没过：${n} 项要处理，现在不能启动`, 'bad', Object.assign({ sticky: true }, see));
      else if (n > 0) toast(`自检通过，可以启动；另有 ${n} 项要留意`, 'ok', see);
      else toast('自检通过，可以启动', 'ok', see);
    }
    if (lastStatus) renderStatus(lastStatus);
  } catch (_) {
    if (mine) toast('自检做完了，结果没读回来', 'bad', see);
  }
}
function tick() {
  histSample();
  renderAll();
}
