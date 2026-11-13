function syncUpgradeTabs() {
  document.querySelectorAll('#up-svc-tabs .seg-btn').forEach(b => b.setAttribute('aria-pressed', b.dataset.svc === upgradeModalSvc ? 'true' : 'false'));
}
function openUpgradeModal(svc) {
  upgradeModalSvc = svc || 'backend';
  showModal('svc-upgrade-modal');
  renderUpgradeModal();
  syncUpgradeTabs();
  loadServiceVersions(true);
}
async function closeUpgradeModal() {
  if (upgradeBusy) {
    const ok = await askConfirm({
      title: '还在传输或升级，要关掉这个窗口吗？',
      body: '已经发给服务器的升级请求会在后台继续做完。',
      okLabel: '关掉窗口',
      cancelLabel: '留在这里'
    });
    if (!ok) return;
  }
  hideModal('svc-upgrade-modal');
}
function switchUpgradeSvc(svc) {
  if (svc === upgradeModalSvc) return;
  if (upgradeBusy) { toast('正在传输或升级，做完再换服务', 'info'); return; }
  upgradeModalSvc = svc;
  setHidden('up-progress-wrap', true);
  renderUpgradeModal();
  syncUpgradeTabs();
}