// 单个服务的启停。做完时的提示和版本刷新由 onJobFinished 统一处理
function svcLabel(svc) {
  const s = ((lastStatus && lastStatus.services) || []).find(x => x.name === svc);
  return { s, label: (s && s.title) || svc };
}
async function startSingleService(svc) {
  await runJob('start', { service: svc }, false);
}
async function stopSingleService(svc) {
  const { label } = svcLabel(svc);
  const ok = await askConfirm({
    title: `停止 ${label}？`,
    body: '停着的时候它的端口打不开，依赖它的上层服务也会一起停。',
    okLabel: '停止',
    danger: true
  });
  if (!ok) return;
  await runJob('stop', { service: svc, force: false }, true);
}
// 好好跑着的服务重启前问一句；已经坏了或停着的直接重启，不问
async function restartSingleService(svc) {
  const { s, label } = svcLabel(svc);
  const healthy = !!s && (s.state === 'ok' || s.state === 'waiting');
  if (healthy) {
    const ok = await askConfirm({ title: `重启 ${label}？`, body: '重启期间这个服务会短暂中断。', okLabel: '重启' });
    if (!ok) return;
  }
  await runJob('restart', { service: svc }, true);
}