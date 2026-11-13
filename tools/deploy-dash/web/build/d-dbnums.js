let dbnumsErr = '';
async function loadDbnums() {
  const base = activeBase;
  if (!dbnumsList.length) byId('dbnums-body').innerHTML = '<tr><td colspan="6" class="empty-cell">正在读取…</td></tr>';
  try {
    const res = await api('/v1/dbnums');
    if (base !== activeBase) return;
    dbnumsList = res.dbnums || [];
    dbnumsErr = '';
    // 列表里已经没有的库，从已选里去掉
    const have = new Set(dbnumsList.map(d => d.dbnum));
    for (const n of [...selectedDbnums]) if (!have.has(n)) selectedDbnums.delete(n);
  } catch (e) {
    if (base !== activeBase) return;
    if (e && e.status === 401) return;
    dbnumsErr = errWhy(e);
  }
  renderDbnums();
}
function renderDbnums() {
  const body = byId('dbnums-body');
  if (dbnumsErr && !dbnumsList.length) {
    body.innerHTML = `<tr><td colspan="6" class="empty-cell bad">读不到设计库列表：${esc(dbnumsErr)}</td></tr>`;
    syncDbnumSel();
    return;
  }
  const q = (byId('dbnum-filter').value || '').trim().toLowerCase();
  const rows = dbnumsList.filter(d => !q || String(d.dbnum).includes(q) || String(d.name || '').toLowerCase().includes(q));
  body.innerHTML = rows.map(d => {
    const on = selectedDbnums.has(d.dbnum);
    const model = d.initialized ? `已生成 · ${esc(d.pieces || 0)} 构件` : '<span class="cell-muted">未生成</span>';
    return `<tr class="pick${on ? ' selected' : ''}" data-db="${esc(d.dbnum)}">`
      + `<td class="col-check"><input type="checkbox"${on ? ' checked' : ''} aria-label="选中库 ${esc(d.dbnum)}"></td>`
      + `<td class="cell-mono">${esc(d.dbnum)}</td><td>${esc(d.db_type || d.kind || '—')}</td><td>${esc(d.name || '—')}</td>`
      + `<td class="num">${d.file_size != null ? esc(fmtFileSize(d.file_size)) : '—'}</td><td>${model}</td></tr>`;
  }).join('') || `<tr><td colspan="6" class="empty-cell">${dbnumsList.length ? '没有符合的库' : '这个工程里没有设计库'}</td></tr>`;
  syncDbnumSel();
}
function dbnumRows() { return [...byId('dbnums-body').querySelectorAll('tr.pick')]; }
function markDbRow(tr, on) {
  tr.classList.toggle('selected', on);
  const cb = tr.querySelector('input');
  if (cb) cb.checked = on;
}
// 勾选只改这一行，不重画整张表
function toggleDb(key, on) {
  const d = dbnumsList.find(x => String(x.dbnum) === String(key));
  if (!d) return;
  if (on === undefined) on = !selectedDbnums.has(d.dbnum);
  if (on) selectedDbnums.add(d.dbnum); else selectedDbnums.delete(d.dbnum);
  const tr = dbnumRows().find(r => r.dataset.db === String(key));
  if (tr) markDbRow(tr, on);
  syncDbnumSel();
}
// 表头的全选：只作用于当前筛出来的这些行
function toggleAllDbnums(on) {
  for (const tr of dbnumRows()) {
    const d = dbnumsList.find(x => String(x.dbnum) === tr.dataset.db);
    if (!d) continue;
    if (on) selectedDbnums.add(d.dbnum); else selectedDbnums.delete(d.dbnum);
    markDbRow(tr, on);
  }
  syncDbnumSel();
}
function syncDbnumSel() {
  const n = selectedDbnums.size;
  setText('dbnum-sel-count', n ? `已选 ${n} 个库` : '');
  setOff('btn-dbnums-run', n === 0, n === 0 ? '先勾选至少一个库' : '');
  const rows = dbnumRows();
  const sel = rows.filter(r => r.classList.contains('selected')).length;
  const all = byId('dbnum-all');
  all.checked = rows.length > 0 && sel === rows.length;
  all.indeterminate = sel > 0 && sel < rows.length;
  all.disabled = rows.length === 0;
}
async function triggerDbnums() {
  const arr = [...selectedDbnums];
  if (!arr.length) return;
  const shown = arr.slice().sort((a, b) => a - b);
  const ok = await askConfirm({
    title: `生成 ${arr.length} 个库的模型？`,
    body: '库号：' + shown.slice(0, 40).join('、') + (shown.length > 40 ? ` 等 ${shown.length} 个` : ''),
    okLabel: '开始生成'
  });
  if (!ok) return;
  const res = await runJob('init', { dbnums: arr }, false);
  if (res) switchView('overview');
}