function appendUpLog(msg) {
  const box = byId('up-logs');
  box.classList.remove('hidden');
  box.textContent = (box.textContent ? box.textContent + '\n' : '') + `[${fmtNow()}] ${msg}`;
  box.scrollTop = box.scrollHeight;
}

function setUpgradeBusy(busy, label = '') {
  upgradeBusy = busy;
  if (busy && label) {
    setHidden('up-progress-wrap', false);
    byId('up-progress-text').textContent = label;
  }
  renderUpgradeModal();
}

function setUploadProgress(pct, text) {
  setHidden('up-progress-wrap', false);
  byId('up-progress-pct').textContent = Math.round(pct) + '%';
  byId('up-progress-fill').style.width = Math.min(100, Math.max(0, pct)) + '%';
  if (text) byId('up-progress-text').textContent = text;
}