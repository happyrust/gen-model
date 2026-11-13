// =========================================================================
// 小工具
// =========================================================================
const SIDEBAR_KEY = 'aios_dash_sidebar_collapsed';
const HIST_KEY_PREFIX = 'aios_dash_hist:';

function byId(id) { return document.getElementById(id); }
function esc(s) {
  return String(s == null ? '' : s).replace(/[&<>"']/g, ch => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[ch]));
}
function icon(name, cls) {
  return `<svg class="ico${cls ? ' ' + cls : ''}" aria-hidden="true"><use href="#i-${name}"/></svg>`;
}
function sleep(ms) { return new Promise(r => setTimeout(r, ms)); }
function pad2(n) { return String(n).padStart(2, '0'); }
function fmtClock(ms) { const d = new Date(ms); return `${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}`; }
function fmtNow() { return fmtClock(Date.now()); }
function parseTs(s) { if (!s) return NaN; const t = Date.parse(s); return isNaN(t) ? NaN : t; }
function fmtDateTime(s) {
  const t = parseTs(s);
  if (isNaN(t)) return s || '';
  const d = new Date(t);
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ${fmtClock(t)}`;
}
// 时长按决策需要的精度：2 小时 14 分
function fmtDur(ms) {
  if (!(ms >= 0)) return '—';
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s} 秒`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} 分钟`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h} 小时 ${m % 60} 分`;
  return `${Math.floor(h / 24)} 天 ${h % 24} 小时`;
}
// 正在走的计时：1 分 20 秒
function fmtSpan(ms) {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s} 秒`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} 分 ${s % 60} 秒`;
  return `${Math.floor(m / 60)} 小时 ${m % 60} 分`;
}
function fmtAgo(ms) {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s} 秒前`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} 分钟前`;
  return `${Math.floor(m / 60)} 小时前`;
}
function fmtGb(n) { if (typeof n !== 'number' || !isFinite(n)) return '—'; return n >= 100 ? String(Math.round(n)) : n.toFixed(1); }
// 内存只有这一个写法，卡片和表格用同一个，同一个采样值不会显示成两个数
function fmtMem(gb) { if (typeof gb !== 'number' || !isFinite(gb)) return '—'; return gb < 1 ? `${Math.round(gb * 1024)} MB` : `${gb.toFixed(1)} GB`; }
function fmtPct(p) { if (typeof p !== 'number' || !isFinite(p)) return '—'; return (p < 10 ? p.toFixed(1) : String(Math.round(p))) + '%'; }
function fmtFileSize(b) {
  if (typeof b !== 'number' || !isFinite(b)) return '—';
  if (b >= 1073741824) return (b / 1073741824).toFixed(1) + ' GB';
  if (b >= 1048576) return (b / 1048576).toFixed(1) + ' MB';
  return Math.max(1, Math.round(b / 1024)) + ' KB';
}
function hostOf(base) { return String(base || '').replace(/^https?:\/\//, ''); }
function timeoutSignal(ms) {
  if (typeof AbortSignal !== 'undefined' && AbortSignal.timeout) return AbortSignal.timeout(ms);
  const c = new AbortController();
  setTimeout(() => c.abort(), ms);
  return c.signal;
}

// 只在内容变了的时候写 DOM：每秒重画一遍也不会打断悬停、焦点和打开的菜单
function setText(id, text) { const e = byId(id); if (e && e.textContent !== text) e.textContent = text; }
function setHtml(id, html) { const e = byId(id); if (e && e._h !== html) { e.innerHTML = html; e._h = html; } }
function setTip(e, tip) {
  if (!e) return;
  if (tip) { if (e.getAttribute('data-tip') !== tip) e.setAttribute('data-tip', tip); }
  else if (e.hasAttribute('data-tip')) e.removeAttribute('data-tip');
}
function setHidden(id, hide) { const e = byId(id); if (e) e.classList.toggle('hidden', !!hide); }
// 停用用 aria-disabled 而不是 disabled：停用的按钮也要能停上去看到「为什么现在不能点」
function setOff(id, off, tip) {
  const e = byId(id);
  if (!e) return;
  if (off) { if (e.getAttribute('aria-disabled') !== 'true') e.setAttribute('aria-disabled', 'true'); }
  else if (e.hasAttribute('aria-disabled')) e.removeAttribute('aria-disabled');
  if (tip !== undefined) setTip(e, tip);
}
document.addEventListener('click', e => {
  const t = e.target && e.target.closest ? e.target.closest('[aria-disabled="true"]') : null;
  if (t) { e.preventDefault(); e.stopPropagation(); }
}, true);

// =========================================================================
// 提示框：全页只有这一套（data-tip），鼠标停留和键盘焦点都会出
// =========================================================================
const Tip = (() => {
  let cur = null, timer = 0;
  const box = () => byId('tip');
  function place(t) {
    const b = box(), r = t.getBoundingClientRect();
    const w = b.offsetWidth, h = b.offsetHeight;
    let top = r.top - h - 8;
    if (top < 8) top = Math.min(window.innerHeight - h - 8, r.bottom + 8);
    let left = r.left + r.width / 2 - w / 2;
    left = Math.max(8, Math.min(left, window.innerWidth - w - 8));
    b.style.left = Math.round(left) + 'px';
    b.style.top = Math.round(Math.max(8, top)) + 'px';
  }
  function show(t) {
    const text = t.getAttribute('data-tip');
    if (!text) return;
    const b = box();
    b.textContent = text;
    b.classList.remove('hidden');
    place(t);
  }
  function hide() {
    clearTimeout(timer); timer = 0; cur = null;
    const b = box();
    if (b) b.classList.add('hidden');
  }
  function arm(t, delay) {
    hide();
    if (!t) return;
    cur = t;
    timer = setTimeout(() => { if (cur === t && document.contains(t)) show(t); }, delay);
  }
  document.addEventListener('mouseover', e => {
    const t = e.target && e.target.closest ? e.target.closest('[data-tip]') : null;
    if (t !== cur) arm(t, 350);
  });
  document.documentElement.addEventListener('mouseleave', hide);
  document.addEventListener('focusin', e => {
    const t = e.target && e.target.closest ? e.target.closest('[data-tip]') : null;
    let kb = false;
    try { kb = e.target.matches(':focus-visible'); } catch (_) {}
    if (t && kb) arm(t, 0);
  });
  document.addEventListener('focusout', hide);
  document.addEventListener('mousedown', hide, true);
  document.addEventListener('keydown', e => { if (e.key === 'Escape') hide(); }, true);
  window.addEventListener('scroll', e => {
    if (cur && e.target && (e.target === document || (e.target.contains && e.target.contains(cur)))) hide();
  }, true);
  return {
    hide,
    // 重画之后：锚点没了就收起，文字变了就跟着变
    sync() {
      if (!cur) return;
      if (!document.contains(cur)) { hide(); return; }
      const b = box();
      if (b.classList.contains('hidden')) return;
      const text = cur.getAttribute('data-tip');
      if (!text) hide();
      else if (b.textContent !== text) { b.textContent = text; place(cur); }
    }
  };
})();

// =========================================================================
// 弹出层：行菜单、问题列表、失败列表都用它
// =========================================================================
const Pop = (() => {
  let anchor = null;
  const box = () => byId('pop');
  function shut() {
    const b = box();
    b.classList.add('hidden');
    b.innerHTML = '';
  }
  function close(refocus) {
    if (!anchor) return;
    const a = anchor;
    anchor = null;
    shut();
    a.setAttribute('aria-expanded', 'false');
    if (refocus && document.contains(a)) a.focus();
  }
  function open(a, html, opts) {
    opts = opts || {};
    if (anchor === a) { close(false); return; }
    close(false);
    Tip.hide();
    const b = box();
    b.innerHTML = html;
    if (opts.role) b.setAttribute('role', opts.role); else b.removeAttribute('role');
    b.classList.remove('hidden');
    anchor = a;
    a.setAttribute('aria-expanded', 'true');
    const r = a.getBoundingClientRect(), w = b.offsetWidth, h = b.offsetHeight;
    let left = opts.align === 'end' ? r.right - w : r.left;
    left = Math.max(8, Math.min(left, window.innerWidth - w - 8));
    let top = r.bottom + 4;
    if (top + h > window.innerHeight - 8) top = Math.max(8, r.top - h - 4);
    b.style.left = Math.round(left) + 'px';
    b.style.top = Math.round(top) + 'px';
    const first = b.querySelector('button, a[href]');
    if (first) first.focus();
  }
  document.addEventListener('mousedown', e => {
    if (anchor && !box().contains(e.target) && !anchor.contains(e.target)) close(false);
  }, true);
  document.addEventListener('keydown', e => {
    if (!anchor) return;
    if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); close(true); return; }
    if (e.key === 'Tab') { e.preventDefault(); close(true); return; }
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      const items = [...box().querySelectorAll('button, a[href]')];
      if (!items.length) return;
      e.preventDefault();
      const i = items.indexOf(document.activeElement);
      const n = e.key === 'ArrowDown' ? (i + 1) % items.length : (i - 1 + items.length) % items.length;
      items[n].focus();
    }
  }, true);
  window.addEventListener('scroll', e => {
    if (anchor && e.target && (e.target === document || (e.target.contains && e.target.contains(anchor)))) close(false);
  }, true);
  window.addEventListener('resize', () => close(false));
  document.addEventListener('click', e => {
    if (anchor && box().contains(e.target) && e.target.closest('.pop-item, [data-pop-close]')) close(false);
  });
  return {
    open, close,
    isOpen: () => !!anchor,
    sync() { if (anchor && !document.contains(anchor)) { anchor = null; shut(); } }
  };
})();

// =========================================================================
// 消息条：代替 alert。kind 不给就按文字判断是不是出错
// =========================================================================
function toast(msg, kind, opts) {
  const text = String(msg == null ? '' : msg);
  const k = kind || (/失败|错误|出错|超时|异常|没有|没做成|无法/.test(text) ? 'bad' : 'info');
  const box = byId('toasts');
  if (!box) return null;
  const t = document.createElement('div');
  t.className = 'toast ' + k;
  t.setAttribute('role', k === 'bad' ? 'alert' : 'status');
  t.innerHTML = icon(k === 'bad' ? 'alert' : k === 'ok' ? 'check' : 'info') + `<span class="toast-text">${esc(text)}</span>`;
  if (opts && opts.actionLabel) {
    const a = document.createElement('button');
    a.className = 'btn-sm';
    a.textContent = opts.actionLabel;
    a.onclick = () => { t.remove(); if (opts.onAction) opts.onAction(); };
    t.appendChild(a);
  }
  const x = document.createElement('button');
  x.className = 'icon-btn sm';
  x.setAttribute('aria-label', '关闭这条消息');
  x.innerHTML = icon('x', 'sm');
  x.onclick = () => t.remove();
  t.appendChild(x);
  box.appendChild(t);
  while (box.children.length > 4) box.firstElementChild.remove();
  const ttl = opts && opts.sticky ? 0 : (k === 'bad' ? 10000 : 4500);
  if (ttl) setTimeout(() => t.remove(), ttl);
  return t;
}

// =========================================================================
// 弹窗：打开时把焦点放进去，Tab 圈在里面，Esc 关最上面那个，关掉后焦点回到原处
// =========================================================================
const modalStack = [];
function focusables(root) {
  return [...root.querySelectorAll('button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])')]
    .filter(e => !e.disabled && e.type !== 'hidden' && e.offsetParent !== null);
}
function showModal(id, focusSel) {
  const m = byId(id);
  if (!m) return;
  Tip.hide();
  Pop.close(false);
  if (m.classList.contains('hidden')) {
    m._back = document.activeElement;
    m.classList.remove('hidden');
    modalStack.push(id);
  }
  const target = (focusSel && m.querySelector(focusSel)) || focusables(m.querySelector('.modal-body') || m)[0] || focusables(m)[0];
  if (target) target.focus();
}
function hideModal(id) {
  const m = byId(id);
  if (!m || m.classList.contains('hidden')) return;
  m.classList.add('hidden');
  const i = modalStack.lastIndexOf(id);
  if (i >= 0) modalStack.splice(i, 1);
  const back = m._back;
  m._back = null;
  if (back && document.contains(back) && typeof back.focus === 'function') back.focus();
}
document.addEventListener('keydown', e => {
  if (!modalStack.length) return;
  const m = byId(modalStack[modalStack.length - 1]);
  if (!m) return;
  if (e.key === 'Escape') {
    if (Pop.isOpen()) return;
    e.preventDefault();
    const fn = m.dataset.close;
    if (fn && typeof window[fn] === 'function') window[fn](); else hideModal(m.id);
  } else if (e.key === 'Tab') {
    const f = focusables(m);
    if (!f.length) return;
    const first = f[0], last = f[f.length - 1], a = document.activeElement;
    if (!m.contains(a)) { e.preventDefault(); first.focus(); }
    else if (e.shiftKey && a === first) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && a === last) { e.preventDefault(); first.focus(); }
  }
});

// 确认框：代替 confirm。返回 Promise<boolean>；也接受一句话的字符串
let confirmResolve = null;
function askConfirm(opts) {
  if (typeof opts === 'string') opts = { body: opts };
  opts = opts || {};
  if (confirmResolve) { const r = confirmResolve; confirmResolve = null; r(false); }
  return new Promise(resolve => {
    setText('confirm-title', opts.title || '请确认');
    const body = byId('confirm-body');
    body.textContent = opts.body || '';
    body.classList.toggle('hidden', !opts.body);
    const ok = byId('confirm-ok');
    ok.textContent = opts.okLabel || '确定';
    ok.className = opts.danger ? 'danger-solid' : 'primary';
    byId('confirm-cancel').textContent = opts.cancelLabel || '取消';
    confirmResolve = resolve;
    showModal('confirm-modal', opts.danger ? '#confirm-cancel' : '#confirm-ok');
  });
}
function settleConfirm(v) {
  const r = confirmResolve;
  confirmResolve = null;
  hideModal('confirm-modal');
  if (r) r(!!v);
}
function cancelConfirm() { settleConfirm(false); }

// =========================================================================
// 页面状态
// =========================================================================
let token = localStorage.getItem(TOKEN_KEY) || '';   // 只是「当前面板」这一台的令牌；别的服务器的令牌在已存列表里
let lastSeq = 0;
let lastStatus = null;
let logSource = 'backend';
let logLevel = 'all';
let logAfter = -1;          // -1：还没定位到日志尾部
let logLines = [];
let logPaused = false;
let dbnumsList = [];
let selectedDbnums = new Set();
let svcVersions = {};       // name -> ServiceVersionEntry
let localComparisons = {};  // name -> { artifact, diff, kind, file, files }
let upgradeModalSvc = 'backend';
let upgradeBusy = false;
let fleetCache = {};        // base -> { name, health, word, full }
let curView = 'overview';
let needsToken = false;
let clockOffset = 0;        // 服务器钟和浏览器钟差得太多时才用（毫秒）
function srvNow() { return Date.now() - clockOffset; }

// =========================================================================
// 深浅色（缺省浅色，选过的记在浏览器里）
// =========================================================================
function syncThemeButton() {
  const dark = document.documentElement.classList.contains('dark');
  const u = byId('theme-ico');
  if (u) u.setAttribute('href', dark ? '#i-sun' : '#i-moon');
  setTip(byId('btn-theme'), dark ? '切到浅色' : '切到深色');
}
function toggleTheme() {
  const isDark = document.documentElement.classList.toggle('dark');
  localStorage.setItem(THEME_KEY, isDark ? 'dark' : 'light');
  syncThemeButton();
}

// =========================================================================
// 访问令牌
// =========================================================================
function openTokenModal(msg) {
  const m = byId('token-modal');
  byId('token-err').textContent = msg || '';
  // 已经开着就不再动输入框：后台请求接连 401 时不能把用户正在输入的内容冲掉
  if (!m.classList.contains('hidden')) return;
  const input = byId('token-input');
  input.value = getActiveToken();
  showModal('token-modal', '#token-input');
  input.select();
}
function closeTokenModal() { hideModal('token-modal'); }
function saveToken() {
  const val = byId('token-input').value.trim();
  if (activeBase) saveServerEntry(activeBase, val);
  else { token = val; localStorage.setItem(TOKEN_KEY, val); }
  closeTokenModal();
  lastSeq = 0;
  resetLogs();
  needsToken = false;
  conn = newConn();
  renderServerSelect();
  probeAllSites();
  startLoops();
}

// =========================================================================
// 接口
// =========================================================================
async function api(path, opts = {}, overrideBase = null, overrideToken = null) {
  const base = overrideBase !== null ? normalizeServerBase(overrideBase) : activeBase;
  const tok = overrideToken !== null ? overrideToken : (base ? ((loadSavedServers().find(x => x.base === base) || {}).token || '') : token);
  const headers = Object.assign({ 'Content-Type': 'application/json' }, opts.headers || {});
  if (tok) headers['Authorization'] = 'Bearer ' + tok;
  const resp = await fetch((base || '') + path, Object.assign({}, opts, { headers }));
  if (resp.status === 401) {
    // 只有「当前正在管的这一台」要令牌才弹窗；请求发出去之后又切了服务器的，不算
    if (overrideBase === null && base === activeBase) {
      needsToken = true;
      renderConn();
      openTokenModal('令牌无效或已过期，请重新输入');
    }
    const err = new Error('需要目标服务器访问令牌 (401)');
    err.status = 401;
    throw err;
  }
  if (resp.status === 304) return { notModified: true };
  const text = await resp.text();
  let data = {};
  try { data = text ? JSON.parse(text) : {}; } catch (_) { data = { raw: text }; }
  if (!resp.ok) {
    const err = new Error(data.message || data.error || ('HTTP ' + resp.status));
    err.status = resp.status;
    err.code = data.error || '';
    throw err;
  }
  return data;
}

// =========================================================================
// 服务器下拉、站点侧栏
// =========================================================================
function renderServerSelect() {
  const sel = byId('target-server-select');
  if (sel) {
    const bases = loadSavedServers().map(x => x.base).filter(b => b && b !== location.origin);
    if (activeBase && !bases.includes(activeBase)) bases.push(activeBase);
    let html = `<option value="">当前面板 (${esc(location.host)})</option>`;
    for (const b of bases) html += `<option value="${esc(b)}">${esc(hostOf(b))}</option>`;
    html += '<option value="__add__">+ 指定服务器 IP…</option>';
    sel.innerHTML = html;
    sel.value = activeBase || '';
    setTip(sel.parentElement, '现在管的是 ' + (activeBase ? hostOf(activeBase) : '当前面板 ' + location.host));
  }
  const lbl = byId('cfg-target-label');
  if (lbl) lbl.textContent = activeBase ? hostOf(activeBase) : '当前面板 (' + location.host + ')';
  renderFleetSidebar();
}

function applySidebarState(collapsed) {
  const sb = byId('fleet-sidebar');
  if (!sb) return;
  sb.classList.toggle('collapsed', collapsed);
  const b = byId('btn-sidebar-toggle');
  b.setAttribute('aria-expanded', collapsed ? 'false' : 'true');
  setTip(b, (collapsed ? '展开站点列表' : '收起站点列表') + '（Ctrl+B）');
}
function toggleSidebar() {
  const sb = byId('fleet-sidebar');
  if (!sb) return;
  const collapsed = !sb.classList.contains('collapsed');
  applySidebarState(collapsed);
  localStorage.setItem(SIDEBAR_KEY, collapsed ? '1' : '0');
}
document.addEventListener('keydown', e => {
  if ((e.ctrlKey || e.metaKey) && String(e.key || '').toLowerCase() === 'b' && !/INPUT|TEXTAREA|SELECT/.test((document.activeElement || {}).tagName)) {
    e.preventDefault();
    toggleSidebar();
  }
});

// 一台站点好不好：和结论栏用同一个判定函数，侧栏和结论栏不会各说各的
function summarizeSiteHealth(st) {
  return siteInfo(deriveOverall(st, deriveProblems(st, null)), st);
}
function siteList() {
  return [{ base: '', isCurrent: true }].concat(
    loadSavedServers().filter(x => x.base && x.base !== location.origin).map(x => ({ base: x.base, isCurrent: false }))
  );
}
function renderFleetSidebar() {
  const box = byId('fleet-rows');
  if (!box) return;
  const html = siteList().map(item => {
    const isSel = (activeBase || '') === item.base;
    let info = fleetCache[item.base];
    if (isSel && activeSiteInfo) info = activeSiteInfo;   // 当前这一台：直接用结论栏刚算出来的那一份
    info = info || {};
    const h = info.health || 'connecting';
    const word = info.word || '连接中';
    const addr = item.isCurrent ? location.host : hostOf(item.base);
    const title = info.name || (item.isCurrent ? '当前站点' : addr);
    const mark = h === 'ok' ? '<span class="dot ok"></span>' : h === 'bad' ? icon('x', 'sm') : h === 'warn' ? icon('alert', 'sm')
      : h === 'stopped' ? icon('square', 'sm') : h === 'busy' ? '<span class="spin"></span>' : icon('dashed', 'sm');
    const tone = h === 'ok' ? ' ok' : h === 'bad' ? ' bad' : h === 'warn' ? ' warn' : '';
    const wcls = h === 'bad' ? ' w-bad' : h === 'warn' ? ' w-warn' : '';
    const b = esc(item.base);
    const tip = esc(`${title}\n${addr}\n${info.full || word}`);
    return `<div class="site-row${isSel ? ' active' : ''}">`
      + `<button type="button" class="site-main" data-base="${b}"${isSel ? ' aria-current="true"' : ''} data-tip="${tip}">`
      + `<span class="site-mark${tone}">${mark}</span>`
      + `<span class="site-text"><span class="site-line"><span class="site-name">${esc(title)}</span><span class="site-word${wcls}">${esc(word)}</span></span>`
      + `<span class="site-sub">${esc(addr)}</span></span></button>`
      + (item.isCurrent ? '' : `<button type="button" class="site-del" data-del="${b}" aria-label="从列表移除 ${esc(addr)}" data-tip="从列表移除">${icon('x', 'sm')}</button>`)
      + '</div>';
  }).join('');
  if (box._h !== html) { box.innerHTML = html; box._h = html; }
}
async function removeSavedServer(base) {
  const ok = await askConfirm({
    title: `从列表移除 ${hostOf(base)}？`,
    body: '只是不再在这里显示，那台服务器不受影响。保存在这个浏览器里的令牌会一起删掉。',
    okLabel: '移除'
  });
  if (!ok) return;
  const list = loadSavedServers().filter(x => x.base !== base);
  localStorage.setItem(SERVERS_KEY, JSON.stringify(list));
  delete fleetCache[base];
  if (activeBase === base) switchTargetServer('');
  else renderServerSelect();
}
async function probeAllSites() {
  const targets = [{ base: '', tok: localStorage.getItem(TOKEN_KEY) || '' }].concat(
    loadSavedServers().filter(x => x.base && x.base !== location.origin).map(x => ({ base: x.base, tok: x.token || '' }))
  );
  await Promise.all(targets.map(async t => {
    const keep = (fleetCache[t.base] || {}).name || '';
    try {
      const headers = {};
      if (t.tok) headers['Authorization'] = 'Bearer ' + t.tok;
      const resp = await fetch((t.base || '') + '/v1/status?since=0', { headers, signal: timeoutSignal(4000) });
      if (!resp.ok) {
        const word = resp.status === 401 ? '未授权' : 'HTTP ' + resp.status;
        fleetCache[t.base] = { name: keep, health: 'bad', word, full: word };
        return;
      }
      fleetCache[t.base] = summarizeSiteHealth(await resp.json());
    } catch (_) {
      fleetCache[t.base] = { name: keep, health: 'bad', word: '离线', full: '连不上' };
    }
  }));
  renderFleetSidebar();
}

// 换一台服务器：旧的那台在路上的请求全部作废，界面回到「正在读取」
function switchTargetServer(base, tok) {
  histSave();   // 先把旧的这一台的历史存好，再换目标
  activeBase = normalizeServerBase(base);
  if (activeBase && tok !== undefined) saveServerEntry(activeBase, tok);
  localStorage.setItem(ACTIVE_SERVER_KEY, activeBase);
  lastSeq = 0; lastStatus = null;
  resetLogs();
  svcVersions = {}; localComparisons = {};
  setText('pkg-cmp-summary', '');
  needsToken = false;
  conn = newConn();
  myJobs.clear(); jobWatchReady = false; lastJobId = null; doctorFinding = null; stateSince = {};
  selectedDbnums = new Set(); dbnumsList = [];
  histLoad();
  renderServerSelect();
  renderAll();
  renderLogs();
  restartStatusPoll();
  refreshNow();
  loadConfig();
  loadServiceVersions(true);
  if (curView === 'dbnums') loadDbnums();
  if (curView === 'jobs') loadJobs();
}
function onServerSelectChange(val) {
  if (val === '__add__') {
    renderServerSelect();
    promptAddServer();
    return;
  }
  switchTargetServer(val);
}

// 「指定服务器」弹窗（代替两次 prompt；令牌用密码框，不明文显示）
function promptAddServer() {
  byId('server-host').value = activeBase ? hostOf(activeBase) : '';
  byId('server-token').value = '';
  byId('server-err').textContent = '';
  showModal('server-modal', '#server-host');
}
function closeServerModal() { hideModal('server-modal'); }
function submitServerModal() {
  const raw = byId('server-host').value.trim();
  if (!raw) {
    byId('server-err').textContent = '先填地址';
    byId('server-host').focus();
    return;
  }
  const b = normalizeServerBase(raw);
  const tok = byId('server-token').value.trim();
  closeServerModal();
  if (!b) { switchTargetServer(''); return; }
  // 令牌留空：这台以前存过令牌就沿用，不拿空串覆盖
  const known = loadSavedServers().find(x => x.base === b);
  if (tok || !known) switchTargetServer(b, tok);
  else switchTargetServer(b);
}
