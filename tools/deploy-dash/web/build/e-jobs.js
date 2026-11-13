let selectedJobId = null;
function jobStateHtml(j) {
  const pill = (cls, ic, word, why, tip) => `<span class="st"${tip ? ` data-tip="${esc(tip)}"` : ''}><span class="st-pill ${cls}">${ic ? icon(ic) : ''}${word}</span>`
    + (why ? `<span class="st-why">${esc(why)}</span>` : '') + '</span>';
  if (j.state === 'running') return '<span class="st"><span class="spin"></span>在跑</span>';
  if (j.state === 'queued') return `<span class="st">${icon('clock')}排队</span>`;
  if (j.state === 'done' && !j.exit_code) return '<span class="st"><span class="dot ok"></span>完成</span>';
  if (j.state === 'done') return pill('bad', 'x', '失败', '退出码 ' + j.exit_code);
  if (j.state === 'rejected') return pill('warn', 'alert', '没收下', '', '发起的时候被拒绝了，没有执行');
  if (j.state === 'cancelled') return pill('off', '', '已取消');
  if (j.state === 'lost') return pill('bad', 'x', '没跑完', '', '没有留下结束记录，多半是面板程序中途重启了');
  return esc(j.state || '—');
}
async function loadJobs() {
  const body = byId('jobs-body'), base = activeBase;
  try {
    const res = await api('/v1/jobs');
    if (base !== activeBase) return;
    const jobs = (res.jobs || []).slice().sort((a, b) => Number(b.id) - Number(a.id));
    body.innerHTML = jobs.map(j => {
      const t0 = parseTs(j.started_at), t1 = parseTs(j.finished_at);
      const dur = isNaN(t0) ? '—' : !isNaN(t1) ? fmtSpan(t1 - t0) : j.state === 'running' ? fmtSpan(srvNow() - t0) : '—';
      const when = j.started_at || j.queued_at;
      const id = esc(j.id);
      return `<tr class="pick${String(j.id) === selectedJobId ? ' selected' : ''}" data-job="${id}">`
        + `<td class="cell-mono">${id}</td>`
        + `<td><button type="button" class="link-btn" data-job="${id}" data-tip="看这个任务的结果和输出">${esc(jobLabel(j))}</button></td>`
        + `<td>${jobStateHtml(j)}</td>`
        + `<td class="cell-mono"${when ? ` data-tip="${esc(fmtDateTime(when))}"` : ''}>${esc(fmtShortTime(when) || '—')}</td>`
        + `<td class="num">${esc(dur)}</td>`
        + `<td class="cell-wrap">${esc(cjkSpace(j.message || (j.result && j.result.message) || ''))}</td></tr>`;
    }).join('') || '<tr><td colspan="6" class="empty-cell">还没有任务记录</td></tr>';
  } catch (e) {
    if (base !== activeBase || (e && e.status === 401)) return;
    body.innerHTML = `<tr><td colspan="6" class="empty-cell bad">读不到任务记录：${esc(errWhy(e))}</td></tr>`;
  }
}
function doctorHtml(d) {
  const probs = d.problems || [], notes = d.notes || [];
  const head = d.can_start === false
    ? `<div class="doc-head bad">${icon('x')}现在不能启动：${probs.length} 项要处理</div>`
    : probs.length
      ? `<div class="doc-head warn">${icon('alert')}可以启动；有 ${probs.length} 项要留意</div>`
      : `<div class="doc-head ok">${icon('check')}自检通过，可以启动</div>`;
  const items = probs.map(p => {
    const kv = [['查的是', p.checked], ['结果', p.result], ['下一步', p.next], ['配置项', p.field]].filter(x => x[1])
      .map(x => `<span class="k">${x[0]}</span><span>${esc(x[1])}</span>`).join('');
    return `<div class="doc-item"><div class="doc-what">${esc(p.what || '')}</div>${kv ? `<div class="doc-kv">${kv}</div>` : ''}</div>`;
  }).join('');
  const noteHtml = notes.length ? `<ul class="doc-notes">${notes.map(n => `<li>${esc(n)}</li>`).join('')}</ul>` : '';
  return `<div class="doc">${head}${items}${noteHtml}</div>`;
}
// 一个任务的结果和输出。输出要单独分页取：/v1/jobs/<id> 本身不带输出
async function viewJobOut(id) {
  id = String(id);
  selectedJobId = id;
  document.querySelectorAll('#jobs-body tr.pick').forEach(tr => tr.classList.toggle('selected', tr.dataset.job === id));
  const view = byId('job-out-view'), base = activeBase;
  const stale = () => base !== activeBase || selectedJobId !== id;
  setText('job-out-title', `任务 ${id}`);
  view.innerHTML = '<div class="log-empty">正在读取…</div>';
  try {
    const job = await api('/v1/jobs/' + encodeURIComponent(id));
    if (stale()) return;
    const lines = [];
    let after = 0, more = true, guard = 0;
    while (more && guard++ < 40) {
      const page = await api(`/v1/jobs/${encodeURIComponent(id)}/output?after=${after}&max=500`);
      if (stale()) return;
      for (const ln of page.lines || []) {
        lines.push(typeof ln === 'string' ? ln : (ln && ln.text != null ? (ln.t ? ln.t + ' ' : '') + ln.text : JSON.stringify(ln)));
      }
      const next = typeof page.next === 'number' ? page.next : after;
      more = !!page.more && next > after;
      after = next;
    }
    setText('job-out-title', `任务 ${id} · ${jobLabel(job)}`);
    const t0 = parseTs(job.started_at), t1 = parseTs(job.finished_at);
    const bits = [];
    if (!isNaN(t0)) bits.push(fmtDateTime(job.started_at) + ' 开始');
    if (!isNaN(t0) && !isNaN(t1)) bits.push('用时 ' + fmtSpan(t1 - t0));
    const who = jobWho(job);
    if (who) bits.push('来自 ' + who);
    if (job.output_file) bits.push('输出文件 ' + job.output_file);
    const r = job.result || {};
    const msg = job.message || r.message;
    const reasons = (job.busy_reasons || []).map(x => typeof x === 'string' ? x : JSON.stringify(x));
    let html = `<div class="job-sum">${jobStateHtml(job)}<span>${esc(bits.join(' · '))}</span></div>`;
    if (msg) html += `<div class="job-msg">${esc(cjkSpace(msg))}</div>`;
    if (reasons.length) html += `<div class="job-msg">${esc('没收下的原因：' + reasons.join('；'))}</div>`;
    if (r.doctor) html += doctorHtml(r.doctor);
    if (r.preview) html += `<div class="job-lines">${esc(JSON.stringify(r.preview, null, 2))}</div>`;
    if (lines.length) html += `<div class="job-lines">${esc(lines.join('\n'))}</div>`;
    else if (!r.doctor && !r.preview) html += `<div class="job-none">${job.state === 'running' || job.state === 'queued' ? '还没有输出。' : '这个任务没有输出。'}</div>`;
    view.innerHTML = html;
    view.scrollTop = 0;
  } catch (e) {
    if (stale() || (e && e.status === 401)) return;
    view.innerHTML = `<div class="log-empty bad">读不到任务 ${esc(id)}：${esc(errWhy(e))}</div>`;
  }
}
// 从别处（消息条、自检标记）跳过来看某个任务
function showJob(id) {
  selectedJobId = String(id);
  switchView('jobs');
  viewJobOut(id);
}