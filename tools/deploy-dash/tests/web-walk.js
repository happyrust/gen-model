// 部署面板网页走查：node web-walk.js <调试端口> <场景.json> [--run <沙盒目录>] [--shots <截图目录>]
// 连一个开着远程调试端口的（无头）Chrome，按场景一步步做；除了常规操作，还有 typefile、setfiles、clicktext、clickif、expect、noexceptions。
// 场景里的 ${RUN} 换成沙盒目录（缺省 <本目录>/../target/test-run）；截图写进 --shots（缺省 <沙盒>/shots）。
// 单步出错只记下、不中断；typefile 读到的内容只送进页面，不打印。
const fs = require('fs');
const path = require('path');
const [,, portArg, scenarioPath, ...rest] = process.argv;
const opt = name => { const i = rest.indexOf(name); return i >= 0 ? rest[i + 1] : undefined; };
const runDir = path.resolve(opt('--run') || path.join(__dirname, '..', 'target', 'test-run'));
const shotsDir = path.resolve(opt('--shots') || path.join(runDir, 'shots'));
const base = 'http://127.0.0.1:' + portArg;
const sleep = ms => new Promise(r => setTimeout(r, ms));
const results = [];

async function main() {
  const steps = JSON.parse(fs.readFileSync(scenarioPath, 'utf8').split('${RUN}').join(JSON.stringify(runDir).slice(1, -1)));
  const targets = await (await fetch(base + '/json/list')).json();
  let page = targets.find(t => t.type === 'page');
  if (!page) page = await (await fetch(base + '/json/new?about:blank', { method: 'PUT' })).json();
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((res, rej) => { ws.onopen = res; ws.onerror = () => rej(new Error('ws connect failed')); });
  let id = 0;
  const pending = new Map();
  const logs = [];
  let loadFired = null;
  const send = (method, params = {}) => new Promise((res, rej) => {
    const i = ++id;
    pending.set(i, { res, rej });
    ws.send(JSON.stringify({ id: i, method, params }));
  });
  ws.onmessage = ev => {
    const m = JSON.parse(ev.data);
    if (m.id && pending.has(m.id)) {
      const p = pending.get(m.id);
      pending.delete(m.id);
      if (m.error) p.rej(new Error(m.error.message)); else p.res(m.result);
      return;
    }
    if (m.method === 'Runtime.consoleAPICalled') {
      logs.push('[console.' + m.params.type + '] ' + m.params.args.map(a => a.value ?? a.description ?? '').join(' '));
    } else if (m.method === 'Runtime.exceptionThrown') {
      const d = m.params.exceptionDetails;
      logs.push('[exception] ' + ((d.exception && d.exception.description) || d.text) + ' @' + (d.lineNumber + 1) + ':' + d.columnNumber);
    } else if (m.method === 'Log.entryAdded') {
      logs.push('[log.' + m.params.entry.level + '] ' + m.params.entry.text + ' ' + (m.params.entry.url || ''));
    } else if (m.method === 'Page.loadEventFired' && loadFired) {
      loadFired();
      loadFired = null;
    } else if (m.method === 'Page.javascriptDialogOpening') {
      logs.push('[dialog.' + m.params.type + '] ' + m.params.message);
      send('Page.handleJavaScriptDialog', { accept: true }).catch(() => {});
    }
  };
  await send('Page.enable');
  await send('Runtime.enable');
  await send('Log.enable');
  await send('DOM.enable');
  const evalJs = async (js) => {
    const r = await send('Runtime.evaluate', { expression: js, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error('eval: ' + ((r.exceptionDetails.exception && r.exceptionDetails.exception.description) || r.exceptionDetails.text));
    return r.result.value;
  };
  const locate = (sel, text) => `(() => {
    const all = [...document.querySelectorAll(${JSON.stringify(sel)})];
    const e = all.find(x => x.getClientRects().length > 0 && ${text ? `(x.textContent || '').includes(${JSON.stringify(text)})` : 'true'});
    if (!e) return null;
    e.scrollIntoView({ block: 'center', inline: 'center' });
    const r = e.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2, disabled: !!e.disabled };
  })()`;
  const clickAt = async (p) => {
    await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: p.x, y: p.y });
    await send('Input.dispatchMouseEvent', { type: 'mousePressed', x: p.x, y: p.y, button: 'left', clickCount: 1 });
    await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: p.x, y: p.y, button: 'left', clickCount: 1 });
  };
  const navWait = async (fn, settle) => {
    const done = new Promise(r => { loadFired = r; });
    await fn();
    await Promise.race([done, sleep(8000)]);
    await sleep(settle ?? 800);
  };
  const pressKey = async (k, shift) => {
    const vk = { Tab: 9, Enter: 13, Escape: 27, ' ': 32, ArrowDown: 40, ArrowUp: 38, ArrowLeft: 37, ArrowRight: 39, F5: 116 }[k] || 0;
    const b = { key: k, code: k === ' ' ? 'Space' : k, windowsVirtualKeyCode: vk, nativeVirtualKeyCode: vk, modifiers: shift ? 8 : 0 };
    const text = k === 'Enter' ? '\r' : (k === ' ' ? ' ' : undefined);
    await send('Input.dispatchKeyEvent', Object.assign({ type: text ? 'keyDown' : 'rawKeyDown' }, b, text ? { text } : {}));
    await send('Input.dispatchKeyEvent', Object.assign({ type: 'keyUp' }, b));
  };
  const show = v => (typeof v === 'string' ? v : JSON.stringify(v));
  for (const s of steps) {
    try {
      switch (s.op) {
        case 'size':
          await send('Emulation.setDeviceMetricsOverride', { width: s.w, height: s.h, deviceScaleFactor: s.dpr || 1, mobile: !!s.mobile });
          await sleep(s.settle ?? 300);
          break;
        case 'goto':
          await navWait(() => send('Page.navigate', { url: s.url }), s.settle);
          break;
        case 'reload':
          await navWait(() => send('Page.reload', { ignoreCache: true }), s.settle);
          break;
        case 'wait':
          await sleep(s.ms);
          break;
        case 'eval': {
          const v = await evalJs(s.js);
          console.log('eval> ' + (s.label ? s.label + ': ' : '') + show(v));
          break;
        }
        case 'expect': {
          const v = await evalJs(s.js);
          const ok = !!(v && typeof v === 'object' && 'ok' in v ? v.ok : v);
          const detail = v && typeof v === 'object' && 'detail' in v ? v.detail : '';
          results.push({ label: s.label, ok });
          console.log((ok ? 'OK   ' : 'FAIL ') + s.label + (detail !== '' && detail !== undefined ? '  (' + show(detail) + ')' : ''));
          break;
        }
        case 'click':
        case 'clicktext':
        case 'clickif': {
          const p = await evalJs(locate(s.sel, s.text));
          if (!p) {
            if (s.op === 'clickif') { console.log('skip> ' + s.sel + (s.text ? ' 「' + s.text + '」' : '') + ' 不在'); break; }
            throw new Error('no element: ' + s.sel + (s.text ? ' 「' + s.text + '」' : ''));
          }
          if (p.disabled) console.log('note> ' + s.sel + ' 是禁用的');
          await clickAt(p);
          await sleep(s.settle ?? 300);
          break;
        }
        case 'key':
          await pressKey(s.key, s.shift);
          await sleep(s.settle ?? 150);
          break;
        case 'type':
          await send('Input.insertText', { text: s.text });
          await sleep(150);
          break;
        case 'typefile':
          await send('Input.insertText', { text: fs.readFileSync(s.file, 'utf8').trim() });
          await sleep(150);
          break;
        case 'setfiles': {
          const doc = await send('DOM.getDocument', { depth: 0 });
          const q = await send('DOM.querySelector', { nodeId: doc.root.nodeId, selector: s.sel });
          if (!q.nodeId) throw new Error('no element: ' + s.sel);
          await send('DOM.setFileInputFiles', { nodeId: q.nodeId, files: s.files });
          await sleep(s.settle ?? 800);
          break;
        }
        case 'media':
          await send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: s.scheme }] });
          break;
        case 'shot': {
          const params = { format: 'png' };
          if (s.full) {
            const m = await send('Page.getLayoutMetrics');
            const cs = m.cssContentSize || m.contentSize;
            params.clip = { x: 0, y: 0, width: cs.width, height: cs.height, scale: 1 };
            params.captureBeyondViewport = true;
          }
          const r = await send('Page.captureScreenshot', params);
          const out = path.join(shotsDir, s.path);
          fs.mkdirSync(path.dirname(out), { recursive: true });
          fs.writeFileSync(out, Buffer.from(r.data, 'base64'));
          console.log('shot> ' + s.path);
          break;
        }
        case 'logs':
          console.log(logs.length ? logs.join('\n') : '(no console output)');
          break;
        case 'noexceptions': {
          const ex = logs.filter(l => l.startsWith('[exception]'));
          results.push({ label: s.label, ok: ex.length === 0 });
          console.log((ex.length ? 'FAIL ' : 'OK   ') + s.label + '  (' + ex.length + ' 个未捕获异常)');
          break;
        }
        case 'clearlogs':
          logs.length = 0;
          break;
        default:
          throw new Error('unknown op ' + s.op);
      }
    } catch (e) {
      const what = s.label || s.sel || '';
      results.push({ label: 'step ' + s.op + ' ' + what, ok: false });
      console.log('STEP-FAIL ' + s.op + ' ' + what + ': ' + e.message);
    }
  }
  const failed = results.filter(r => !r.ok).length;
  console.log(`== ${results.length - failed}/${results.length} 通过` + (failed ? `，${failed} 项不符` : ''));
  ws.close();
}

main().then(() => process.exit(0), e => { console.error('FAILED: ' + e.message); process.exit(1); });
