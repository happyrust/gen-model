
// =========================================================================
// 日志：只取尾部，跟着追；往上翻就停止跟随，右下角给「回到最新」
// =========================================================================
const LOG_KEEP = 800, LOG_TAIL = 300;
const LOG_SRC_NAME = { database: '数据库', backend: '后端', rs_server: 'rs-server', frontend: '前端', job: '任务', agent: '面板' };
let logEpoch = 0, logLoaded = false, logErr = '';
let logFollow = true, logUnseen = 0, logUid = 0, logAbort = null, logFile = '', logCleared = false;
let logDom = { key: '', upto: 0, rows: false };

function logLv(ln) {
  const l = String(ln.level || '').toLowerCase();
  return /^(err|fatal|panic|crit)/.test(l) ? 'err' : /^warn/.test(l) ? 'warn' : 'info';
}
function logPush(entry) { entry.i = ++logUid; logLines.push(entry); }
function hlText(text, q) {
  text = String(text == null ? '' : text);
  if (!q) return esc(text);
  const low = text.toLowerCase();
  if (low.length !== text.length) return esc(text);
  let out = '', i = 0;
  for (;;) {
    const k = low.indexOf(q, i);
    if (k < 0) break;
    out += esc(text.slice(i, k)) + '<mark>' + esc(text.slice(k, k + q.length)) + '</mark>';
    i = k + q.length;
  }
  return out + esc(text.slice(i));
}
// 正文自己以级别词开头（「WARN …」「ERROR …」）而且和这一行的级别是同一个词时，正文里那个词不再重复显示；不一样就原样留着
const LOG_LV_WORD = { err: 'ERROR', warn: 'WARN', info: 'INFO' };
const LOG_LV_SAME = { err: /^(?:ERROR|ERR)$/, warn: /^(?:WARN|WARNING)$/, info: /^INFO$/ };
function logText(ln, lv) {
  const text = String(ln.text == null ? '' : ln.text);
  const m = /^\[?([A-Za-z]{3,7})\]?:?[ \t]+/.exec(text);
  return m && LOG_LV_SAME[lv].test(m[1].toUpperCase()) ? text.slice(m[0].length) : text;
}
// 时间和级别后面各带一个空格：复制出去的文本才不会粘在一起
function logRowHtml(ln, q) {
  if (ln.gap) return `<div class="log-gap" data-i="${ln.i}">… 中间有 ${ln.gap} 行没取 …</div>`;
  const lv = logLv(ln);
  return `<div class="log-line${lv === 'info' ? '' : ' ' + lv}" data-i="${ln.i}"><span class="t">${esc(ln.t || '')} </span>`
    + `<span class="lv">${LOG_LV_WORD[lv]} </span><span class="x">${hlText(logText(ln, lv), q)}</span></div>`;
}
function renderLogJump() {
  const show = !logFollow && logDom.rows;
  setHidden('log-jump', !show);
  if (show) setHtml('log-jump', icon('down', 'sm') + (logUnseen > 0 ? `回到最新 · 新 ${logUnseen} 行` : '回到最新'));
}
function renderLogs(incremental) {
  const view = byId('log-view');
  if (!view) return;
  const q = (byId('log-search').value || '').trim().toLowerCase();
  const key = `${logEpoch}|${logLevel}|${q}`;
  const total = logLines.reduce((n, ln) => n + (ln.gap ? 0 : 1), 0);
  const show = ln => {
    if (ln.gap) return !q && logLevel === 'all';
    const lv = logLv(ln);
    if (logLevel === 'warn' && lv === 'info') return false;
    if (logLevel === 'err' && lv !== 'err') return false;
    return !q || String(ln.text || '').toLowerCase().includes(q);
  };
  let appended = 0, done = false;
  if (incremental === true && logDom.rows && logDom.key === key && logLines.length) {
    // 只追加新行、从顶上删掉超出的旧行：不重排整个列表，选中的文字和滚动位置都不受影响
    const fresh = logLines.filter(ln => ln.i > logDom.upto && show(ln));
    if (fresh.length) view.insertAdjacentHTML('beforeend', fresh.map(ln => logRowHtml(ln, q)).join(''));
    const minI = logLines[0].i;
    while (view.firstElementChild && Number(view.firstElementChild.dataset.i) < minI) view.removeChild(view.firstElementChild);
    appended = fresh.reduce((n, ln) => n + (ln.gap ? 0 : 1), 0);
    logDom.upto = logUid;
    done = !!view.firstElementChild;
  }
  if (!done) {
    const rows = logLines.filter(show);
    if (!rows.length) {
      const name = LOG_SRC_NAME[logSource] || logSource;
      let msg, bad = false;
      if (logLines.length) msg = `没有符合条件的行（这里一共 ${total} 行）`;
      else if (logErr) { msg = `读不到${name}日志：${logErr}`; bad = true; }
      else if (!logLoaded) msg = `正在读取${name}日志…`;
      else if (logCleared) msg = '已清空。有新日志会接着出现。';
      else msg = logSource === 'job' ? '还没有任务输出。' : `${name}还没有日志。`;
      view.innerHTML = `<div class="log-empty${bad ? ' bad' : ''}">${esc(msg)}</div>`;
      logDom = { key, upto: logUid, rows: false };
    } else {
      view.innerHTML = rows.map(ln => logRowHtml(ln, q)).join('');
      logDom = { key, upto: logUid, rows: true };
      logFollow = true;
    }
  }
  const shown = logDom.rows ? view.querySelectorAll('.log-line').length : 0;
  setText('log-count', !total ? '' : shown === total ? `${total} 行` : `${shown} / ${total} 行`);
  setTip(byId('log-count'), total ? `页面里只留最近 ${LOG_KEEP} 行。` + (logFile ? `\n完整日志在服务器上：${logFile}` : '') : '');
  if (logDom.rows) {
    if (logFollow) { view.scrollTop = view.scrollHeight; logUnseen = 0; }
    else logUnseen += appended;
  }
  renderLogJump();
}
function onLogScroll() {
  const v = byId('log-view');
  if (!v.clientHeight) return;
  const atBottom = v.scrollHeight - v.scrollTop - v.clientHeight < 24;
  if (atBottom === logFollow) return;
  logFollow = atBottom;
  if (atBottom) logUnseen = 0;
  renderLogJump();
}
function jumpLogBottom() {
  const v = byId('log-view');
  logFollow = true; logUnseen = 0;
  v.scrollTop = v.scrollHeight;
  renderLogJump();
  v.focus({ preventScroll: true });
}

async function pollLogs() {
  for (;;) {
    if (logPaused || needsToken) { await sleep(400); continue; }
    const epoch = logEpoch;
    const q = `/v1/logs?source=${encodeURIComponent(logSource)}`;
    const ctl = new AbortController();
    logAbort = ctl;
    const timer = setTimeout(() => ctl.abort(), 15000);
    let more = false;
    try {
      if (logAfter < 0) {
        // 先问一句最新编号，只从尾部往回取几百行（after=0 会从最旧的一行给起）
        const probe = await api(`${q}&after=0&max=1&wait=0`, { signal: ctl.signal });
        if (epoch !== logEpoch) continue;
        logAfter = Math.max(0, (probe.newest || 0) - LOG_TAIL);
      }
      const res = await api(`${q}&after=${logAfter}&max=200&wait=${logLoaded ? 2 : 0}`, { signal: ctl.signal });
      if (epoch !== logEpoch) continue;
      const newest = res.newest || 0;
      logFile = res.file || '';
      if (res.reset) {
        // 编号对不上了（面板程序重启过，或日志换了文件）：清掉，重新只取尾部
        logLines = [];
        logAfter = Math.max(0, newest - LOG_TAIL);
        logLoaded = false;
        renderLogs();
        continue;
      }
      let next = typeof res.next === 'number' ? res.next : logAfter;
      const lines = res.lines || [];
      const before = logUid;
      if (res.dropped > 0 && logLines.length) logPush({ gap: res.dropped });
      for (const ln of lines) logPush(ln);
      if (newest - next > LOG_KEEP) {   // 落后太多（比如暂停了很久）：不一页页追，直接跳到尾部
        logPush({ gap: newest - LOG_TAIL - next });
        next = newest - LOG_TAIL;
      }
      logAfter = next;
      more = !!res.more;
      const first = !logLoaded, hadErr = !!logErr;
      logLoaded = true; logErr = '';
      if (lines.length) logCleared = false;
      if (logLines.length > LOG_KEEP) logLines.splice(0, logLines.length - LOG_KEEP);
      if (first || hadErr) renderLogs();
      else if (logUid !== before) renderLogs(true);
    } catch (e) {
      if (epoch !== logEpoch) continue;
      if (!(e && e.status === 401)) {
        logErr = e && e.status ? (/^HTTP \d+$/.test(e.message) ? '服务器返回 ' + e.message : e.message) : '连不上';
        if (!logLines.length) renderLogs();
      }
      await sleep(3000);
      continue;
    } finally {
      clearTimeout(timer);
    }
    if (!more) await sleep(250);
  }
}
function setPillState(groupSel, attr, val) {
  document.querySelectorAll(groupSel + ' .pill-btn').forEach(b => {
    const on = b.dataset[attr] === val;
    b.classList.toggle('active', on);
    b.setAttribute('aria-pressed', on ? 'true' : 'false');
  });
}
// 换来源、换服务器、换令牌：手上的行作废，路上的请求作废，重新从尾部取
function resetLogs() {
  logAfter = -1; logLines = []; logEpoch++; logLoaded = false; logErr = ''; logFile = ''; logCleared = false;
  logFollow = true; logUnseen = 0;
  if (logAbort) { try { logAbort.abort(); } catch (_) {} }
}
function setLogSource(src) {
  if (!src || (src === logSource && (logLoaded || !logErr))) return;
  logSource = src;
  setPillState('#log-sources', 'src', src);
  resetLogs();
  if (logPaused) togglePauseLog();   // 暂停着换来源会一直空着，所以换来源就恢复
  renderLogs();
  if (lastStatus) renderStatus(lastStatus);   // 服务表里高亮的那一行跟着换
}
// 「看日志」：切到这个服务的日志，并把日志区滚到眼前
function focusLog(src) {
  if (curView !== 'overview') switchView('overview');
  setLogSource(src);
  const calm = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  byId('log-card').scrollIntoView({ block: 'nearest', behavior: calm ? 'auto' : 'smooth' });
}
function setLogLevel(lv) {
  logLevel = lv;
  setPillState('#log-levels', 'lv', lv);
  renderLogs();
}
function togglePauseLog() {
  logPaused = !logPaused;
  const b = byId('btn-log-pause');
  b.textContent = logPaused ? '继续' : '暂停';
  b.setAttribute('aria-pressed', logPaused ? 'true' : 'false');
  setTip(b, logPaused ? '已暂停：新日志先不取。点一下接着取。' : '');
}
function clearLogs() {
  logLines = []; logUnseen = 0; logCleared = true;
  renderLogs();
}

// =========================================================================
// 操作
// =========================================================================
const JOB_NAME = { up: '一键启动', down: '全部停止', start: '启动', stop: '停止', restart: '重启', init: '生成模型', doctor: '自检',
  configure: '保存配置', preview_update: '更新预览', incremental_update: '增量更新' };
// 一次请求为什么没成：服务器给了说法就用它的，没给就报状态码，连不上就说连不上哪一台
function errWhy(e) {
  if (e && e.status) return /^HTTP \d+$/.test(e.message) ? '服务器返回 ' + e.message : e.message;
  return '连不上 ' + (activeBase ? hostOf(activeBase) : location.host);
}
function actErrText(what, e) { return cjkSpace(`${what}没做成`) + `：${errWhy(e)}`; }
// 发起一个任务。成功返回应答；没做成就提示一次并返回 null（不抛异常）。任务做完时的提示由 onJobFinished 统一给
async function runJob(kind, args, confirmReq) {
  try {
    const res = await api('/v1/jobs', { method: 'POST', body: JSON.stringify({ kind, args: args || {}, confirm: !!confirmReq, note: 'web-dashboard' }) });
    const id = res.job_id != null ? res.job_id : (res.job && res.job.id != null ? res.job.id : res.id);
    if (id != null) myJobs.add(String(id));
    return res;
  } catch (e) {
    if (!(e && e.status === 401)) {
      const svc = args && args.service ? ' ' + svcLabel(args.service).label : '';
      toast(actErrText((JOB_NAME[kind] || kind) + svc, e), 'bad');
    }
    return null;
  }
}
async function confirmDown() {
  const st = lastStatus;
  const running = st ? enabledSvcs(st).filter(hasProc).map(s => s.title || s.name) : [];
  const nJobs = st && st.jobs_summary ? (st.jobs_summary.running || 0) : 0;
  const ok = await askConfirm({
    title: '全部停止？',
    body: (running.length ? `会停掉 ${running.join('、')}。` : '会停掉所有服务。') + '停着的时候网页和接口都打不开。'
      + (nJobs > 0 ? `\n后端还有 ${nJobs} 个任务在跑，会被打断。` : ''),
    okLabel: '全部停止',
    danger: true
  });
  if (!ok) return;
  await runJob('down', { force: false }, true);
}
async function refreshNow() {
  try { await api('/v1/refresh', { method: 'POST', body: '{}' }); } catch (_) {}
}
async function toggleQueue() {
  const bh = lastStatus && lastStatus.backend;
  if (!bh) return;
  const pausing = !bh.queue_paused;
  try {
    const res = await api(pausing ? '/v1/queue/pause' : '/v1/queue/resume', { method: 'POST', body: '{}' });
    if (res && res.pending) toast(pausing ? '已经请求暂停增量同步，后端应答后生效' : '已经请求恢复增量同步，后端应答后生效', 'info');
    refreshNow();
  } catch (e) {
    if (!(e && e.status === 401)) toast(actErrText(pausing ? '暂停增量同步' : '恢复增量同步', e), 'bad');
  }
}
function openFrontend() {
  const st = lastStatus;
  let url = st && st.urls && st.urls.frontend;
  if (!url) {
    const fe = ((st && st.services) || []).find(s => s.name === 'frontend');
    let host = location.hostname;
    if (activeBase) { try { host = new URL(activeBase).hostname; } catch (_) {} }
    url = `${location.protocol}//${host}:${(fe && fe.port) || 3100}/`;
  }
  window.open(url, '_blank', 'noopener');
}

const VIEWS = ['overview', 'dbnums', 'config', 'jobs'];
function switchView(name) {
  if (!VIEWS.includes(name)) name = 'overview';
  curView = name;
  for (const v of VIEWS) setHidden('view-' + v, v !== name);
  document.querySelectorAll('#view-tabs .seg-btn').forEach(b => {
    const on = b.dataset.view === name;
    b.classList.toggle('active', on);
    if (on) b.setAttribute('aria-current', 'page'); else b.removeAttribute('aria-current');
  });
  if (name === 'dbnums') loadDbnums();
  if (name === 'config') loadConfig();
  if (name === 'jobs') loadJobs();
  if (name === 'overview' && logFollow) { const v = byId('log-view'); v.scrollTop = v.scrollHeight; }
}
function goDbnums() { switchView('dbnums'); }
