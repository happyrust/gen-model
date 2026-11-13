let loopsStarted = false;
function startLoops() {
  restartStatusPoll();
  if (!loopsStarted) {
    loopsStarted = true;
    pollLogs();
    setInterval(tick, 1000);
    setInterval(probeAllSites, 10000);
  }
  loadServiceVersions(true);
}

// 列表里的按钮统一用事件委托：行会被重画，监听挂在不动的容器上
function bindUi() {
  byId('fleet-rows').addEventListener('click', e => {
    const del = e.target.closest('[data-del]');
    if (del) { removeSavedServer(del.dataset.del); return; }
    const main = e.target.closest('.site-main');
    if (main && (main.dataset.base || '') !== (activeBase || '')) switchTargetServer(main.dataset.base || '');
  });
  byId('services-tbody').addEventListener('click', e => {
    const up = e.target.closest('[data-up]');
    if (up) { openUpgradeModal(up.dataset.up); return; }
    const act = e.target.closest('[data-act]');
    if (act) {
      const svc = act.dataset.svc, what = act.dataset.act;
      if (what === 'start') startSingleService(svc);
      else if (what === 'restart') restartSingleService(svc);
      else if (what === 'menu') openSvcMenu(svc, act);
      return;
    }
    const log = e.target.closest('[data-log]');
    if (log) { focusLog(log.dataset.log); return; }
    // 点行的空白处：下面的日志切到这个服务
    const tr = e.target.closest('tr.svc-row');
    if (tr && !e.target.closest('button, a, input') && !String(window.getSelection && window.getSelection()).trim()) setLogSource(tr.dataset.svc);
  });
  const dbBody = byId('dbnums-body');
  dbBody.addEventListener('click', e => {
    const tr = e.target.closest('tr.pick');
    if (tr && !e.target.closest('input')) toggleDb(tr.dataset.db);
  });
  dbBody.addEventListener('change', e => {
    const tr = e.target.closest('tr.pick');
    if (tr && e.target.matches('input[type="checkbox"]')) toggleDb(tr.dataset.db, e.target.checked);
  });
  byId('jobs-body').addEventListener('click', e => {
    const el = e.target.closest('[data-job]');
    if (el && !String(window.getSelection && window.getSelection()).trim()) viewJobOut(el.dataset.job);
  });
  byId('log-view').addEventListener('scroll', onLogScroll, { passive: true });
}

applySidebarState(localStorage.getItem(SIDEBAR_KEY) === '1');
syncThemeButton();
histLoad();
bindUi();
renderServerSelect();
renderAll();
renderLogs();
probeAllSites();
startLoops();