// 组装 dashboard.html：以备份的原文件为底，按原行号把重写的段换进去；保留段只做带核对的小改动。
// deploy-agent/src/dashboard.html 由本脚本生成：改网页就改这里的分段再生成，不要直接改它。
// 用法：node --expose-internals build.js            组装到 tools/deploy-dash/target/web/out.html 并做检查
//      node --expose-internals build.js --write    检查通过后写入 tools/deploy-dash/deploy-agent/src/dashboard.html
// （--expose-internals 只为了用 Node 自带的 acorn 做「有没有用到没定义的名字」这项检查；不加也能组装）
const fs = require('fs');
const path = require('path');
const cp = require('child_process');
const crypto = require('crypto');

const dir = __dirname;
const root = path.resolve(dir, '..');
const SRC = path.join(root, 'backup', 'dashboard.before.html');
const DEST = path.resolve(root, '..', 'deploy-agent', 'src', 'dashboard.html');
const OUT_DIR = path.resolve(root, '..', 'target', 'web');   // target 不进库
const OUT = path.join(OUT_DIR, 'out.html');
const OUT_JS = path.join(OUT_DIR, 'out-script.js');
fs.mkdirSync(OUT_DIR, { recursive: true });
const WRITE = process.argv.includes('--write');

const fail = [], warn = [];
const must = (cond, msg) => { if (!cond) fail.push(msg); };
const piece = name => fs.readFileSync(path.join(dir, name), 'utf8').replace(/^\uFEFF/, '').replace(/\r\n/g, '\n').replace(/^\n+|\n+$/g, '').split('\n');

const raw = fs.readFileSync(SRC, 'utf8');
must(crypto.createHash('sha256').update(fs.readFileSync(SRC)).digest('hex').startsWith('c2d5bb52'), '备份文件的校验值变了');
const eol = raw.includes('\r\n') ? '\r\n' : '\n';
const L = raw.replace(/\r?\n$/, '').split(/\r?\n/);   // L[n-1] = 原文件第 n 行
must(L.length === 2577, `原文件应为 2577 行，实际 ${L.length}`);

// —— 1. 段落边界核对（在原文上）——
const REPLACED = [[1, 385], [386, 810], [853, 1467], [1469, 1501], [1779, 1800], [1850, 1885], [2133, 2149], [2402, 2423], [2565, 2574]];
const anchors = [
  [385, l => l.trim() === '</head>'], [386, l => l.trim() === '<body>'], [811, l => l.trim() === '<script>'],
  [852, l => l === '}'], [853, l => l.startsWith('function renderServerSelect(')],
  [1456, l => l.startsWith('function switchView(')], [1467, l => l === '}'],
  [1469, l => l.startsWith('async function loadDbnums(')], [1501, l => l === '}'],
  [1779, l => l.startsWith('async function loadJobs(')], [1800, l => l === '}'],
  [1850, l => l.startsWith('async function startSingleService(')], [1885, l => l === '}'],
  [2133, l => l.startsWith('function openUpgradeModal(')], [2149, l => l === '}'],
  [2402, l => l.startsWith('function appendUpLog(')], [2423, l => l === '}'],
  [2565, l => l.startsWith('let loopsStarted')], [2574, l => l.trim() === 'startLoops();'], [2575, l => l.trim() === '</script>']
];
for (const [n, test] of anchors) must(test(L[n - 1] || ''), `第 ${n} 行不是预期的段落边界：${L[n - 1]}`);

// —— 2. 保留段里的单行修改（改之前核对原文）——
function edit(n, test, repl) {
  const cur = L[n - 1];
  if (!test(cur)) { fail.push(`第 ${n} 行和预期不符：${cur}`); return; }
  L[n - 1] = typeof repl === 'function' ? repl(cur) : repl;
}
edit(1681, l => l.trim() === "document.getElementById('site-modal').classList.remove('hidden');", "  showModal('site-modal');");
edit(1683, l => l.startsWith('function closeSiteModal()'), "function closeSiteModal() { hideModal('site-modal'); }");
edit(2153, l => l.includes("'#up-svc-tabs .tab'"), l => l.replace("'#up-svc-tabs .tab'", "'#up-svc-tabs .seg-btn'"));
edit(2159, l => l.trim() === "feBtn.style.display = '';", "    feBtn.classList.remove('hidden');");
edit(2162, l => l.trim() === "feBtn.style.display = 'none';", "    feBtn.classList.add('hidden');");
edit(2175, l => l.includes(": curRunSvc.state) : '已安装';"), l => l.replace(": curRunSvc.state) : '已安装';", ": stateWord(curRunSvc.state)) : '已安装';"));
edit(2209, l => l.trim().startsWith('rbBtn.title = hasBackup ?'), l => "  rbBtn.classList.toggle('hidden', !hasBackup);\n" + l.replace('rbBtn.title =', 'rbBtn.dataset.tip ='));
edit(2508, l => l.trim().startsWith('if (!confirm(`确定对服务'),
  "  if (!await askConfirm({ title: `推送并升级 ${svc}？`, body: '流程：推送新版本 → 停止服务 → 备份当前版本 → 原子替换 → 启动并健康检查。\\n启动失败会自动回滚。', okLabel: '推送并升级' })) return;");
edit(2541, l => l.trim().startsWith('if (!confirm(`确定将'),
  "  if (!await askConfirm({ title: `回滚 ${svc}？`, body: '会换回最近一次升级前的备份。', okLabel: '回滚', danger: true })) return;");

// —— 3. 保留段里的整体替换（数量要对得上）——
const inReplaced = n => REPLACED.some(r => n >= r[0] && n <= r[1]);
function replAll(from, to, expect) {
  let count = 0;
  for (let n = 1; n <= L.length; n++) {
    if (inReplaced(n)) continue;
    const parts = L[n - 1].split(from);
    if (parts.length > 1) { count += parts.length - 1; L[n - 1] = parts.join(to); }
  }
  must(count === expect, `「${from}」应替换 ${expect} 处，实际 ${count} 处`);
}
replAll('alert(', 'toast(', 11);
replAll("'var(--ok)'", "'var(--ok-text)'", 4);
replAll("'var(--bad)'", "'var(--bad-text)'", 10);

// —— 4. 换段（从后往前，行号不串）——
const head = piece('a-head.html');
let si = -1;
head.forEach((l, i) => { if (l.trim() === '</style>') si = i; });
must(si >= 0, 'a-head.html 里没找到 </style>');
must(head[head.length - 1].trim() === '</head>', 'a-head.html 应以 </head> 结尾');
if (si >= 0) head.splice(si, 0, ...piece('a2-extra.css'));
const bodyPiece = piece('b-body.html');
must(bodyPiece[0].trim() === '<body>', 'b-body.html 应以 <body> 开头');
must(!bodyPiece.some(l => /<\/?script/.test(l)), 'b-body.html 里不应有 script 标签');
const jsMain = [].concat(piece('c1-core.js'), [''], piece('c2-status.js'), [''], piece('c3-actions.js'));
const swaps = [
  [1, 385, head], [386, 810, bodyPiece.concat([''])], [853, 1467, jsMain],
  [1469, 1501, piece('d-dbnums.js')], [1779, 1800, piece('e-jobs.js')], [1850, 1885, piece('f-svc.js')],
  [2133, 2149, piece('g-upgrade-open.js')], [2402, 2423, piece('h-upgrade-progress.js')], [2565, 2574, piece('i-init.js')]
];
swaps.sort((a, b) => b[0] - a[0]);
for (const [a, b, lines] of swaps) L.splice(a - 1, b - a + 1, ...lines);
const out = L.join('\n') + '\n';

// —— 5. 检查 ——
const sStart = out.indexOf('\n<script>\n');
const sEnd = out.lastIndexOf('\n</script>');
must(sStart > 0 && sEnd > sStart, '没找到主脚本');
const js = out.slice(sStart + '\n<script>\n'.length, sEnd);
const html = out.slice(0, sStart) + out.slice(sEnd);
const jsLine0 = out.slice(0, sStart + '\n<script>\n'.length).split('\n').length - 1;   // 脚本第 1 行在整个文件里的行号偏移
fs.writeFileSync(OUT_JS, js);
const chk = cp.spawnSync(process.execPath, ['--check', OUT_JS], { encoding: 'utf8' });
must(chk.status === 0, 'node --check 没过：\n' + (chk.stderr || chk.stdout));

// 5a. 残留的原生弹窗 / 原生提示
for (const [re, what] of [[/(?<![\w.$])alert\(/g, 'alert('], [/(?<![\w.$])confirm\(/g, 'confirm('], [/(?<![\w.$])prompt\(/g, 'prompt('], [/\.title\s*=/g, '.title =']]) {
  const n = (js.match(re) || []).length;
  must(n === 0, `脚本里还有 ${n} 处 ${what}`);
}
must(!/\stitle="/.test(html.replace(/<title>[\s\S]*?<\/title>/, '')), '标记里还有原生 title= 提示');
if (/\stitle=\\?"/.test(js)) warn.push('脚本生成的标记里有 title=');

// 5b. 顶层函数重名
const fnDecl = [...js.matchAll(/^(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\(/gm)].map(m => m[1]);
const seen = new Set();
for (const n of fnDecl) { if (seen.has(n)) fail.push(`顶层函数重复定义：${n}`); seen.add(n); }

// 5c. 用到但没定义的名字（用 Node 自带的 acorn 做作用域分析）；内联事件处理也一起查
const GLOBALS = new Set(('window document localStorage sessionStorage location navigator console fetch setTimeout clearTimeout setInterval clearInterval '
  + 'Promise Math JSON Date Number String Object Array Set Map WeakMap WeakSet Error TypeError RangeError RegExp Symbol Boolean BigInt parseInt parseFloat isNaN isFinite '
  + 'encodeURIComponent decodeURIComponent encodeURI decodeURI AbortController AbortSignal URL URLSearchParams FormData Blob File FileReader '
  + 'Uint8Array Uint16Array Uint32Array Int8Array Int32Array Float64Array ArrayBuffer DataView TextEncoder TextDecoder crypto performance '
  + 'requestAnimationFrame cancelAnimationFrame undefined NaN Infinity globalThis matchMedia getComputedStyle Event CustomEvent Node Element HTMLElement '
  + 'XMLHttpRequest Headers Request Response atob btoa structuredClone queueMicrotask Intl Reflect Proxy arguments').split(/\s+/));
let acorn = null;
try { acorn = require('internal/deps/acorn/acorn/dist/acorn'); } catch (_) { warn.push('没加 --expose-internals，跳过「没定义的名字」检查'); }
function patNames(p, set) {
  if (!p) return;
  switch (p.type) {
    case 'Identifier': set.add(p.name); break;
    case 'ObjectPattern': for (const q of p.properties) patNames(q.type === 'RestElement' ? q.argument : q.value, set); break;
    case 'ArrayPattern': for (const q of p.elements) patNames(q, set); break;
    case 'RestElement': patNames(p.argument, set); break;
    case 'AssignmentPattern': patNames(p.left, set); break;
  }
}
const SKIP = new Set(['type', 'loc', 'start', 'end']);
function kids(node, fn) {
  for (const k in node) {
    if (SKIP.has(k)) continue;
    const v = node[k];
    if (Array.isArray(v)) { for (const c of v) if (c && typeof c.type === 'string') fn(c); }
    else if (v && typeof v.type === 'string') fn(v);
  }
}
// 一个函数（或整个脚本）里声明的名字；不进嵌套函数。let/const 按函数级算，稍宽松
function collect(node, set) {
  if (!node || typeof node.type !== 'string') return;
  switch (node.type) {
    case 'FunctionDeclaration': if (node.id) set.add(node.id.name); return;
    case 'FunctionExpression': case 'ArrowFunctionExpression': return;
    case 'ClassDeclaration': if (node.id) set.add(node.id.name); break;
    case 'VariableDeclaration': for (const d of node.declarations) patNames(d.id, set); break;
    case 'CatchClause': if (node.param) patNames(node.param, set); break;
  }
  kids(node, c => collect(c, set));
}
function analyze(ast, parentScope, onUnknown) {
  const ref = (id, scope) => {
    for (let s = scope; s; s = s.parent) if (s.names.has(id.name)) return;
    if (!GLOBALS.has(id.name)) onUnknown(id.name, id.loc.start.line);
  };
  const walkPat = (p, scope) => {
    if (!p) return;
    switch (p.type) {
      case 'Identifier': return;
      case 'ObjectPattern':
        for (const q of p.properties) {
          if (q.type === 'RestElement') walkPat(q.argument, scope);
          else { if (q.computed) walk(q.key, scope); walkPat(q.value, scope); }
        }
        return;
      case 'ArrayPattern': for (const q of p.elements) walkPat(q, scope); return;
      case 'RestElement': walkPat(p.argument, scope); return;
      case 'AssignmentPattern': walkPat(p.left, scope); walk(p.right, scope); return;
      default: walk(p, scope);
    }
  };
  const fnScope = (node, scope) => {
    const s = { names: new Set(['arguments']), parent: scope };
    if (node.type === 'FunctionExpression' && node.id) s.names.add(node.id.name);
    for (const p of node.params) patNames(p, s.names);
    collect(node.body, s.names);
    for (const p of node.params) walkPat(p, s);
    walk(node.body, s);
  };
  const walk = (node, scope) => {
    if (!node || typeof node.type !== 'string') return;
    switch (node.type) {
      case 'Identifier': ref(node, scope); return;
      case 'MemberExpression': walk(node.object, scope); if (node.computed) walk(node.property, scope); return;
      case 'Property': if (node.computed) walk(node.key, scope); walk(node.value, scope); return;
      case 'MethodDefinition': case 'PropertyDefinition': if (node.computed) walk(node.key, scope); walk(node.value, scope); return;
      case 'FunctionDeclaration': case 'FunctionExpression': case 'ArrowFunctionExpression': fnScope(node, scope); return;
      case 'VariableDeclarator': walkPat(node.id, scope); walk(node.init, scope); return;
      case 'CatchClause': walk(node.body, scope); return;
      case 'LabeledStatement': walk(node.body, scope); return;
      case 'BreakStatement': case 'ContinueStatement': case 'MetaProperty': return;
      case 'ClassDeclaration': case 'ClassExpression': walk(node.superClass, scope); walk(node.body, scope); return;
    }
    kids(node, c => walk(c, scope));
  };
  const g = { names: new Set(), parent: parentScope || null };
  collect(ast, g.names);
  walk(ast, g);
  return g;
}
let handlerCount = 0;
if (acorn && chk.status === 0) {
  const unknown = new Map();
  let globalScope = null;
  try {
    const ast = acorn.parse(js, { ecmaVersion: 'latest', sourceType: 'script', locations: true });
    globalScope = analyze(ast, null, (name, line) => { if (!unknown.has(name)) unknown.set(name, []); unknown.get(name).push(line + jsLine0); });
  } catch (e) { fail.push('脚本解析失败：' + e.message); }
  for (const [name, lines] of unknown) fail.push(`用到了没定义的名字 ${name}（输出文件第 ${lines.slice(0, 6).join('、')} 行）`);
  // 内联事件处理：静态标记里的，和脚本拼出来的标记里的
  const handlers = new Set();
  for (const m of html.matchAll(/\son[a-z]+="([^"]*)"/g)) handlers.add(m[1]);
  for (const m of js.matchAll(/\son[a-z]+=\\?"([^"]*?)\\?"/g)) handlers.add(m[1].replace(/\$\{[^}]*\}/g, '0'));
  if (globalScope) {
    for (const code of handlers) {
      handlerCount++;
      try {
        const ast = acorn.parse(`function __h(event){${code.replace(/&quot;/g, '"').replace(/&#39;/g, "'")}\n}`, { ecmaVersion: 'latest', sourceType: 'script', locations: true });
        analyze(ast, globalScope, name => fail.push(`内联处理函数用到了没定义的名字 ${name}：${code}`));
      } catch (e) { fail.push(`内联处理函数解析失败：${code}（${e.message}）`); }
    }
  }
}

// 5d. 脚本里按 id 取的元素，标记里都得有
const ids = new Set([...html.matchAll(/\sid="([^"]+)"/g)].map(m => m[1]));
const dupIds = [...html.matchAll(/\sid="([^"]+)"/g)].map(m => m[1]).filter((x, i, a) => a.indexOf(x) !== i);
for (const d of new Set(dupIds)) fail.push(`标记里 id 重复：${d}`);
const missingIds = new Set();
for (const m of js.matchAll(/(?:getElementById|byId|setText|setHtml|setHidden|setOff|showModal|hideModal|kpiTone|setLink)\(\s*'([^']+)'/g)) if (!ids.has(m[1]) && !m[1].endsWith('-')) missingIds.add(m[1]);   // 以 - 结尾的是拼接前缀，不是完整 id
for (const id of missingIds) fail.push(`脚本要找的元素 #${id} 在标记里没有`);

// 5e. 用到的样式类在样式表里有没有（只提醒，逐个人工看）
const css = (html.match(/<style>([\s\S]*?)<\/style>/) || [, ''])[1].replace(/\/\*[\s\S]*?\*\//g, '');
const cssClasses = new Set([...css.matchAll(/\.([A-Za-z_][\w-]*)/g)].map(m => m[1]));
const used = new Map();
const use = (c, where) => { if (/^[A-Za-z_][\w-]*$/.test(c) && !used.has(c)) used.set(c, where); };
for (const m of html.replace(/<style>[\s\S]*?<\/style>/, '').matchAll(/\sclass="([^"]*)"/g)) m[1].split(/\s+/).forEach(c => use(c, '标记'));
for (const m of js.matchAll(/class=\\?"([^"]*?)\\?"/g)) m[1].replace(/\$\{[^}]*\}/g, ' ').split(/\s+/).forEach(c => use(c, '脚本'));
for (const m of js.matchAll(/(?:className\s*=\s*|cls\s*[:=]\s*)'([^']*)'/g)) m[1].split(/\s+/).forEach(c => use(c, '脚本'));
for (const m of js.matchAll(/classList\.(?:add|remove|toggle|contains)\(\s*'([^']+)'/g)) use(m[1], '脚本');
const noStyle = [...used].filter(([c]) => !cssClasses.has(c));
if (noStyle.length) warn.push('没有对应样式的类：' + noStyle.map(([c, w]) => `${c}(${w})`).join(' '));

// —— 6. 输出 ——
const final = eol === '\r\n' ? out.replace(/\n/g, '\r\n') : out;
fs.writeFileSync(OUT, final);
const sha = crypto.createHash('sha256').update(Buffer.from(final, 'utf8')).digest('hex');
console.log(`组装：${final.split('\n').length - 1} 行，${Buffer.byteLength(final, 'utf8')} 字节，sha256 ${sha.slice(0, 16)}…；脚本 ${js.split('\n').length} 行，顶层函数 ${fnDecl.length} 个，内联处理 ${handlerCount} 种，元素 id ${ids.size} 个`);
for (const w of warn) console.log('提醒：' + w);
if (fail.length) {
  for (const f of fail) console.log('未通过：' + f);
  console.log(`共 ${fail.length} 项未通过，没有写入正式文件。组装结果在 ${OUT}`);
  process.exit(1);
}
if (WRITE) { fs.writeFileSync(DEST, final); console.log('检查全部通过，已写入 ' + DEST); }
else console.log('检查全部通过（没加 --write，只写了 ' + OUT + '）');
