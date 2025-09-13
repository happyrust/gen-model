/* 部署站点列表与详情弹窗逻辑（集成主程序） */
(function(){
  'use strict';

  let PROJECTS = [];
  let CURRENT_ID = null;
  let FILTERS = { q:'', status:'', env:'', owner:'' };

  // 工具
  const $ = (id)=>document.getElementById(id);
  const escHtml = (s)=>String(s==null?"":s)
      .replace(/&/g,'&amp;').replace(/</g,'&lt;')
      .replace(/>/g,'&gt;').replace(/"/g,'&quot;').replace(/'/g,'&#39;');
  const ts = ()=> new Date().toLocaleString();

  function setHidden(el, hidden){ el.classList[hidden? 'add':'remove']('hidden'); }

  // 密码可见性切换功能
  window.togglePasswordVisibility = function(inputId, button) {
    const input = document.getElementById(inputId);
    const eyeIcon = button.querySelector('.eye-icon');
    const eyeSlashIcon = button.querySelector('.eye-slash-icon');
    
    if (input.type === 'password') {
      input.type = 'text';
      eyeIcon.classList.add('hidden');
      eyeSlashIcon.classList.remove('hidden');
    } else {
      input.type = 'password';
      eyeIcon.classList.remove('hidden');
      eyeSlashIcon.classList.add('hidden');
    }
  };

  function statusBadge(status){
    const map = {
      Running: 'bg-green-100 text-green-700',
      Deploying: 'bg-blue-100 text-blue-700',
      Configuring: 'bg-amber-100 text-amber-700',
      Ok: 'bg-green-100 text-green-700',
      Healthy: 'bg-green-100 text-green-700',
      Pending: 'bg-yellow-100 text-yellow-700',
      Failed: 'bg-red-100 text-red-700',
      Error: 'bg-red-100 text-red-700',
      Stopped: 'bg-gray-100 text-gray-700'
    };
    return map[status] || 'bg-gray-100 text-gray-700';
  }
  function envBadge(env){
    const map = { prod:'bg-purple-100 text-purple-700',
                  staging:'bg-blue-100 text-blue-700',
                  dev:'bg-amber-100 text-amber-700',
                  test:'bg-gray-100 text-gray-700' };
    return map[String(env||'').toLowerCase()] || 'bg-gray-100 text-gray-700';
  }

  // 辅助：构建查询、格式化
  function buildQuery(){
    const params = [];
    if(FILTERS.q) params.push('q='+encodeURIComponent(FILTERS.q));
    if(FILTERS.status) params.push('status='+encodeURIComponent(FILTERS.status));
    if(FILTERS.env) params.push('env='+encodeURIComponent(FILTERS.env));
    if(FILTERS.owner) params.push('owner='+encodeURIComponent(FILTERS.owner));
    params.push('sort=updated_at:desc');
    params.push('per_page=30');
    return params.length? ('?'+params.join('&')) : '';
  }

  function formatTime(v){
    try{
      if(!v) return '';
      if(typeof v === 'number'){
        const d = v>1e12? new Date(v): new Date(v*1000);
        return d.toLocaleString();
      }
      return new Date(v).toLocaleString();
    }catch(_){ return String(v); }
  }

  function formatSize(bytes){
    const b = Number(bytes)||0; if(b<1024) return b+' B';
    const u=['KB','MB','GB','TB']; let i=-1, n=b;
    do{ n/=1024; i++; }while(n>=1024&&i<u.length-1);
    return n.toFixed(1)+' '+u[i];
  }

  // 载入与渲染列表
  async function loadProjects(){
    try{
      const resp = await fetch('/api/deployment-sites'+buildQuery());
      const data = await resp.json();
      const items = Array.isArray(data)? data : (data.items || []);
      PROJECTS = items;
      renderProjects(items);
    }catch(err){
      console.error('loadProjects error:', err);
      $('projects-grid').innerHTML = '<div class="text-red-600">加载部署站点失败</div>';
    }
  }

  // 暴露刷新函数给页面按钮
  window.reloadProjects = function(){ loadProjects(); };

  // 快速新建站点：建议使用 /wizard，此处暂保留占位
  window.createProjectQuick = async function(){
    try {
      const name = prompt('站点名称'); if(!name) return;
      const env = prompt('环境 (dev/staging/prod/test)', 'dev') || '';
      const owner = prompt('负责人', '站点管理员') || '';
      const description = prompt('描述(可选)', '') || '';

      const payload = { name, description, env, owner, selected_projects: [], config: { name: name, manual_db_nums: [], project_name: 'AvevaMarineSample', project_code: 1516, mdb_name: 'ALL', module: 'DESI', db_type: 'surrealdb', surreal_ns: 1516, db_ip: 'localhost', db_port: '8009', db_user: 'root', db_password: 'root', gen_model: true, gen_mesh: true, gen_spatial_tree: true, apply_boolean_operation: true, mesh_tol_ratio: 3.0, room_keyword: '-RM' } };
      const resp = await fetch('/api/deployment-sites', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload)
      });
      if(!resp.ok){
        const t = await resp.text();
        alert('创建失败: ' + t);
        return;
      }
      await loadProjects();
    } catch (e) {
      alert('创建失败: ' + e);
    }
  };

  function renderProjects(items){
    const grid = $('projects-grid');
    if(!grid) return;
    if(!items || !items.length){ grid.innerHTML = '<div class="text-gray-500">暂无部署站点</div>'; return; }
    grid.innerHTML = items.map(p=>{
      const id = escHtml(p.id||'');
      const name = escHtml(p.name||id||'未命名项目');
      const status = escHtml(p.status||'');
      const env = escHtml(p.env||'');
      const url = escHtml(p.url||'');
      const owner = escHtml(p.owner||'');
      const updated = escHtml(p.updated_at||'');
      return `
      <div class="bg-white rounded-lg shadow p-5 hover:shadow-lg transition">
        <div class="flex items-start justify-between">
          <div class="min-w-0">
            <div class="text-lg font-semibold text-gray-900 truncate">${name}</div>
            <div class="mt-1 flex gap-2 text-xs text-gray-600">
              <span class="inline-flex px-2 py-0.5 rounded ${statusBadge(status)}">${status||'未知'}</span>
              <span class="inline-flex px-2 py-0.5 rounded ${envBadge(env)}">${env||'—'}</span>
            </div>
          </div>
        </div>
        <div class="mt-3 space-y-1 text-xs text-gray-600">
          ${owner? `<div><i class=\"fa fa-user w-3 mr-1\"></i> 负责人: ${owner}</div>`:''}
          ${url? `<div><i class=\"fa fa-link w-3 mr-1\"></i> <a class=\"text-blue-600 hover:underline\" target=\"_blank\" href=\"${url}\">${url}</a></div>`:''}
          ${updated? `<div><i class=\"fa fa-clock w-3 mr-1\"></i> 更新: ${updated}</div>`:''}
        </div>
        <div class="mt-4 flex gap-2">
          <button class="px-3 py-1.5 rounded bg-blue-600 text-white" data-view-site="${encodeURIComponent(id)}" onclick="viewProjectDetails('${encodeURIComponent(id)}')">
            <i class="fa fa-eye mr-1"></i> 查看详情
          </button>
          ${url? `<a class="px-3 py-1.5 rounded bg-gray-200" href="${url}" target="_blank">打开</a>`:''}
          <button class="px-3 py-1.5 rounded bg-red-600 text-white hover:bg-red-700" onclick="deleteDeploymentSite('${encodeURIComponent(id)}', '${escHtml(name)}')">
            <i class="fa fa-trash mr-1"></i> 删除
          </button>
        </div>
      </div>`;
    }).join('');
  }

  // 弹窗
  window.openProjectModal = function(){
    const modal = $('project-modal');
    if(!modal){ console.error('[projects.js] modal not found'); return; }
    setHidden(modal, false);
    modal.style.display = 'block'; // 双保险，避免样式冲突
    $('pm-hc-status').textContent='';
    setHidden($('pm-hc-status'), true);
    setHidden($('pm-error'), true);
  };
  window.closeProjectModal = function(){ const modal=$('project-modal'); if(!modal) return; setHidden(modal, true); modal.style.display='none'; };

  window.viewProjectDetails = function(encodedId){
    const id = decodeURIComponent(encodedId||'');
    CURRENT_ID = id;
    try { openProjectModal(); } catch(e){ console.error('openProjectModal failed', e); }
    loadProjectDetail(id);
  };

  window.retryLoadProjectDetail = function(){ if(CURRENT_ID){ loadProjectDetail(CURRENT_ID); }};

  async function loadProjectDetail(id){
    $('pm-title').textContent = '部署站点详情';
    $('pm-content').textContent = '正在加载...';
    setHidden($('pm-open-url'), true);
    setHidden($('pm-health'), true);
    setHidden($('pm-error'), true);
    setHidden($('pm-hc-status'), true);

    try{
      const resp = await fetch('/api/deployment-sites/' + encodeURIComponent(id));
      if(!resp.ok){ throw new Error('HTTP '+resp.status); }
      const p = await resp.json();
      // 保存当前站点详情，供后续操作（如重启数据库）
      window.__currentSiteDetail = p;

      $('pm-title').textContent = p.name || p.id || '部署站点详情';
      $('pm-status').textContent = p.status || '未知';
      $('pm-status').className = `inline-flex items-center px-2 py-0.5 rounded text-xs ${statusBadge(p.status||'')}`;
      $('pm-env').textContent = p.env || '—';
      $('pm-env').className = `inline-flex items-center px-2 py-0.5 rounded text-xs ${envBadge(p.env||'')}`;

      const cfg = p.config || {};
      const cfgLines = [
        `<div><span class=\"text-gray-500\">配置名称：</span>${escHtml(cfg.name)}</div>`,
        `<div><span class=\"text-gray-500\">数据库号：</span>${escHtml((cfg.manual_db_nums||[]).join(', '))}</div>`,
        `<div><span class=\"text-gray-500\">项目：</span>${escHtml(cfg.project_name)} (${escHtml(cfg.project_code)})</div>`,
        `<div><span class=\"text-gray-500\">MDB/模块：</span>${escHtml(cfg.mdb_name)} / ${escHtml(cfg.module)}</div>`,
        `<div><span class=\"text-gray-500\">数据库类型：</span>${escHtml(cfg.db_type)}</div>`,
        `<div><span class=\"text-gray-500\">Surreal NS：</span>${escHtml(cfg.surreal_ns)}</div>`,
        `<div><span class=\"text-gray-500\">连接：</span>${escHtml(cfg.db_ip)}:${escHtml(cfg.db_port)} (${escHtml(cfg.db_user)})</div>`,
        `<div><span class=\"text-gray-500\">生成模型/网格/空间树：</span>${cfg.gen_model?'是':'否'} / ${cfg.gen_mesh?'是':'否'} / ${cfg.gen_spatial_tree?'是':'否'}</div>`,
        `<div><span class=\"text-gray-500\">布尔运算：</span>${cfg.apply_boolean_operation?'是':'否'}</div>`,
        `<div><span class=\"text-gray-500\">网格容差：</span>${escHtml(cfg.mesh_tol_ratio)}</div>`,
        cfg.room_keyword? `<div><span class=\"text-gray-500\">房间关键字：</span>${escHtml(cfg.room_keyword)}</div>`: '',
        cfg.target_sesno? `<div><span class=\"text-gray-500\">目标会话号：</span>${escHtml(cfg.target_sesno)}</div>`: ''
      ].filter(Boolean);

      const metaLines = [];
      if(p.id) metaLines.push(`<div><span class=\"text-gray-500\">ID：</span>${escHtml(p.id)}</div>`);
      if(p.description) metaLines.push(`<div><span class=\"text-gray-500\">描述：</span>${escHtml(p.description)}</div>`);
      if(p.owner) metaLines.push(`<div><span class=\"text-gray-500\">负责人：</span>${escHtml(p.owner)}</div>`);
      if(p.env) metaLines.push(`<div><span class=\"text-gray-500\">环境：</span>${escHtml(p.env)}</div>`);
      if(p.url) metaLines.push(`<div><span class=\"text-gray-500\">URL：</span><a class=\"text-blue-600 hover:underline\" href=\"${escHtml(p.url)}\" target=\"_blank\">${escHtml(p.url)}</a></div>`);
      if(Array.isArray(p.e3d_projects)) metaLines.push(`<div><span class=\"text-gray-500\">E3D 项目数：</span>${p.e3d_projects.length}</div>`);

      let e3dHtml = '';
      if(Array.isArray(p.e3d_projects) && p.e3d_projects.length){
        e3dHtml = `
          <div class=\"bg-white border border-gray-200 rounded p-3\">
            <h4 class=\"font-semibold text-gray-800 mb-2\">E3D 项目</h4>
            <div class=\"overflow-x-auto\">
              <table class=\"min-w-full text-xs\">
                <thead><tr class=\"text-left text-gray-500\"><th class=\"py-1 pr-4\">名称</th><th class=\"py-1 pr-4\">路径</th><th class=\"py-1 pr-4\">DB 文件</th><th class=\"py-1 pr-4\">大小</th><th class=\"py-1 pr-4\">修改时间</th></tr></thead>
                <tbody>
                ${p.e3d_projects.map(proj=>`
                  <tr>
                    <td class=\\"py-1 pr-4\\">${escHtml(proj.name||'')}</td>
                    <td class=\\"py-1 pr-4\\"><span title=\\"${escHtml(proj.path||'')}\\" class=\\"truncate inline-block max-w-[320px]\\">${escHtml(proj.path||'')}</span></td>
                    <td class=\\"py-1 pr-4\\">${escHtml(proj.db_file_count||0)}</td>
                    <td class=\\"py-1 pr-4\\">${formatSize(proj.size_bytes)}</td>
                    <td class=\\"py-1 pr-4\\">${formatTime(proj.last_modified)}</td>
                  </tr>`).join('')}
                </tbody>
              </table>
            </div>
          </div>`;
      }

      // 创建任务操作区域
      const taskActionsHtml = `
        <div class=\"bg-blue-50 border border-blue-200 rounded p-4 mt-4\">
          <h4 class=\"font-semibold text-blue-800 mb-3 flex items-center justify-between\">
            <span><i class=\"fas fa-tasks mr-2\"></i>任务操作</span>
            <button onclick=\"editSiteConfiguration('${escHtml(id)}')\" 
                    class=\"px-3 py-1.5 bg-orange-600 text-white rounded-md hover:bg-orange-700 transition-colors text-sm\">
              <i class=\"fas fa-edit mr-1\"></i>编辑配置
            </button>
          </h4>
          <div class=\"grid grid-cols-1 md:grid-cols-3 gap-3\">
            <button onclick=\"launchParsingTask('${escHtml(id)}')\" 
                    class=\"px-4 py-2 bg-green-600 text-white rounded-md hover:bg-green-700 transition-colors\">
              <i class=\"fas fa-play mr-2\"></i>启动解析任务
            </button>
            <button onclick=\"launchModelingTask('${escHtml(id)}')\" 
                    class=\"px-4 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700 transition-colors\">
              <i class=\"fas fa-cube mr-2\"></i>启动建模任务
            </button>
            <button onclick=\"launchSpatialTask('${escHtml(id)}')\" 
                    class=\"px-4 py-2 bg-purple-600 text-white rounded-md hover:bg-purple-700 transition-colors\">
              <i class=\"fas fa-sitemap mr-2\"></i>启动空间索引
            </button>
          </div>
          <div class=\"mt-3 text-xs text-blue-700\">
            点击任务按钮将基于当前配置创建和启动相应的处理任务 | 可以先编辑配置调整参数
          </div>
        </div>`;

      // 增强的配置参数显示
      const enhancedCfgLines = [
        // 项目信息
        '<div class=\"mb-4\"><h5 class=\"font-medium text-gray-700 mb-2 border-b border-gray-200 pb-1\">项目信息</h5>',
        '<div class=\"grid grid-cols-1 md:grid-cols-2 gap-2 text-sm\">',
        `  <div><span class=\"text-gray-500\">配置名称：</span>${escHtml(cfg.name)}</div>`,
        `  <div><span class=\"text-gray-500\">项目名称：</span>${escHtml(cfg.project_name)} (代码: ${escHtml(cfg.project_code)})</div>`,
        `  <div><span class=\"text-gray-500\">MDB名称：</span>${escHtml(cfg.mdb_name)}</div>`,
        `  <div><span class=\"text-gray-500\">模块类型：</span>${escHtml(cfg.module)}</div>`,
        `  <div><span class=\"text-gray-500\">数据库编号：</span>${escHtml((cfg.manual_db_nums||[]).join(', ') || '自动扫描')}</div>`,
        '</div></div>',
        
        // 数据库连接
        '<div class=\"mb-4\"><h5 class=\"font-medium text-gray-700 mb-2 border-b border-gray-200 pb-1\">数据库连接</h5>',
        '<div class=\"grid grid-cols-1 md:grid-cols-2 gap-2 text-sm\">',
        `  <div><span class=\"text-gray-500\">数据库类型：</span>${escHtml(cfg.db_type)}</div>`,
        `  <div><span class=\"text-gray-500\">连接地址：</span>${escHtml(cfg.db_ip)}:${escHtml(cfg.db_port)}</div>`,
        `  <div><span class=\"text-gray-500\">用户名：</span>${escHtml(cfg.db_user)}</div>`,
        cfg.surreal_ns ? `  <div><span class=\"text-gray-500\">Surreal 命名空间：</span>${escHtml(cfg.surreal_ns)}</div>` : '',
        '</div></div>',
        
        // 生成选项
        '<div class=\"mb-4\"><h5 class=\"font-medium text-gray-700 mb-2 border-b border-gray-200 pb-1\">生成选项</h5>',
        '<div class=\"grid grid-cols-2 md:grid-cols-4 gap-2 text-sm\">',
        `  <div class=\"flex items-center\"><i class=\"fas fa-${cfg.gen_model?'check text-green-600':'times text-red-600'} mr-1\"></i>生成几何模型</div>`,
        `  <div class=\"flex items-center\"><i class=\"fas fa-${cfg.gen_mesh?'check text-green-600':'times text-red-600'} mr-1\"></i>生成网格数据</div>`,
        `  <div class=\"flex items-center\"><i class=\"fas fa-${cfg.gen_spatial_tree?'check text-green-600':'times text-red-600'} mr-1\"></i>生成空间树</div>`,
        `  <div class=\"flex items-center\"><i class=\"fas fa-${cfg.apply_boolean_operation?'check text-green-600':'times text-red-600'} mr-1\"></i>布尔运算</div>`,
        '</div></div>',
        
        // 高级选项
        '<div class=\"mb-4\"><h5 class=\"font-medium text-gray-700 mb-2 border-b border-gray-200 pb-1\">高级选项</h5>',
        '<div class=\"grid grid-cols-1 md:grid-cols-2 gap-2 text-sm\">',
        `  <div><span class=\"text-gray-500\">网格容差比率：</span>${escHtml(cfg.mesh_tol_ratio)}</div>`,
        cfg.room_keyword ? `  <div><span class=\"text-gray-500\">房间关键字：</span>${escHtml(cfg.room_keyword)}</div>` : '',
        cfg.target_sesno ? `  <div><span class=\"text-gray-500\">目标会话号：</span>${escHtml(cfg.target_sesno)}</div>` : '',
        '</div></div>'
      ].filter(Boolean);

      // 根据数据库类型决定是否显示“重启数据库”按钮（目前仅支持 SurrealDB）
      try {
        const dbType = String((cfg.db_type||'')).toLowerCase();
        const restartBtn = $('pm-restart-db');
        if (restartBtn) {
          if (dbType === 'surrealdb') {
            setHidden(restartBtn, false);
            restartBtn.title = '根据当前配置重启 SurrealDB 实例';
          } else {
            setHidden(restartBtn, true);
            restartBtn.title = '';
          }
        }
      } catch(_) {}

      $('pm-content').innerHTML = `
        <div class=\"space-y-4\">
          <div class=\"bg-gray-50 border border-gray-200 rounded p-3\">
            <h4 class=\"font-semibold text-gray-800 mb-2\">基本信息</h4>
            ${metaLines.join('') || '<div class=\\"text-gray-500\\">无</div>'}
          </div>
          <div class=\"bg-white border border-gray-200 rounded p-3\">
            <h4 class=\"font-semibold text-gray-800 mb-3\">完整配置参数</h4>
            <div class=\"space-y-2\">${enhancedCfgLines.join('')}</div>
            <details class=\"mt-3\"><summary class=\"text-blue-600 cursor-pointer text-sm\">查看原始配置 JSON</summary>
              <pre class=\"text-xs bg-gray-50 border rounded p-2 overflow-x-auto mt-2\">${escHtml(JSON.stringify(cfg, null, 2))}</pre>
            </details>
          </div>
          ${e3dHtml}
          ${taskActionsHtml}
        </div>`;

      if(p.url){ const a=$('pm-open-url'); a.href = p.url; setHidden(a,false); } else { setHidden($('pm-open-url'), true); }
      // 部署站点暂不提供健康检查按钮
      setHidden($('pm-health'), true);

    }catch(err){
      console.error('loadProjectDetail error:', err);
      $('pm-content').textContent = '';
      setHidden($('pm-error'), false);
    }
  }

  function setHealthStatus(msg, kind){
    const el = $('pm-hc-status');
    el.textContent = String(msg||'');
    el.className = 'mt-3 text-xs ' + (kind==='ok'? 'text-green-700' : kind==='err'? 'text-red-700' : 'text-gray-700');
    setHidden(el, false);
  }

  window.pmHealthCheck = async function(){
    const btn = $('pm-health');
    const url = btn.dataset.healthUrl;
    if(!url){ setHealthStatus('未配置健康检查地址','err'); return; }
    const startedAt = new Date();
    const t0 = performance.now();
    setHealthStatus('健康检查中...','info');
    try{
      const ctrl = new AbortController();
      const to = setTimeout(()=>ctrl.abort(), 5000);
      const resp = await fetch(url, { signal: ctrl.signal });
      clearTimeout(to);
      const ms = Math.max(0, Math.round(performance.now()-t0));
      const succ = resp.ok;
      setHealthStatus((succ? '健康检查成功':'健康检查失败') + ' | ' + startedAt.toLocaleString() + ' | ' + ms + 'ms', succ? 'ok':'err');
    }catch(err){
      const ms = Math.max(0, Math.round(performance.now()-t0));
      setHealthStatus('健康检查失败：' + (err && err.name==='AbortError'?'超时':'网络错误') + ' | ' + startedAt.toLocaleString() + ' | ' + ms + 'ms','err');
  }

  // 使用当前弹窗中的配置重启数据库（仅 SurrealDB）
  window.pmRestartDatabase = async function(){
    try {
      const detail = window.__currentSiteDetail || null;
      if (!detail || !detail.config) {
        alert('未加载站点配置，无法重启数据库');
        return;
      }
      const cfg = detail.config || {};
      const dbType = String((cfg.db_type||'')).toLowerCase();
      if (dbType !== 'surrealdb') {
        alert('当前仅支持 SurrealDB 重启');
        return;
      }

      const body = {
        mode: 'local',               // 简化：按本机控制处理；如需 SSH 可扩展
        bind_ip: cfg.db_ip,
        bind_port: parseInt(cfg.db_port || '0') || undefined,
        db_user: cfg.db_user,
        db_password: cfg.db_password,
        project_name: cfg.project_name,
      };

      const res = await fetch('/api/surreal/restart', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      });
      let data = {};
      try { data = await res.json(); } catch(_){ }
      alert(data.message || (data.success ? '重启命令已发送' : '重启失败'));
    } catch (e) {
      console.error('pmRestartDatabase error', e);
      alert('重启请求发送失败：' + (e && e.message || '未知错误'));
    }
  };
  };

  // 全局键盘事件
  document.addEventListener('keydown', (e)=>{
    const modalOpen = $('project-modal') && !$('project-modal').classList.contains('hidden');
    if(e.key === 'Escape' && modalOpen){ closeProjectModal(); }
    const errVisible = $('pm-error') && !$('pm-error').classList.contains('hidden');
    if(e.key === 'Enter' && modalOpen && errVisible){ window.retryLoadProjectDetail(); }
  });

  // 删除部署站点
  window.deleteDeploymentSite = async function(encodedId, siteName){
    const id = decodeURIComponent(encodedId||'');
    const displayName = siteName || id || '未知站点';
    
    if (!confirm(`确定要删除部署站点"${displayName}"吗？\n\n此操作将同时删除相关的任务记录，且不可撤销！`)) {
      return;
    }
    
    try {
      const response = await fetch(`/api/deployment-sites/${encodeURIComponent(id)}`, {
        method: 'DELETE'
      });
      
      if (response.ok) {
        const result = await response.json();
        const source = result.source === 'sqlite' ? 'SQLite数据库' : 'SurrealDB数据库';
        alert(`部署站点"${displayName}"已成功从${source}中删除！`);
        
        // 刷新列表
        await loadProjects();
        
        // 如果当前打开的详情页是被删除的站点，关闭弹窗
        if (CURRENT_ID === id) {
          closeProjectModal();
        }
      } else {
        if (response.status === 404) {
          alert('部署站点不存在，可能已被删除');
          await loadProjects(); // 刷新列表以反映当前状态
        } else {
          const errorText = await response.text();
          alert(`删除失败: ${errorText}`);
        }
      }
    } catch (error) {
      console.error('删除部署站点失败:', error);
      alert(`删除过程中发生网络错误: ${error.message}`);
    }
  };

  // 任务启动函数
  window.launchParsingTask = async function(encodedId) {
    const id = decodeURIComponent(encodedId || '');
    
    try {
      // 获取部署站点配置
      const resp = await fetch('/api/deployment-sites/' + encodeURIComponent(id));
      if (!resp.ok) throw new Error('无法获取部署站点配置');
      
      const deploymentSite = await resp.json();
      const config = deploymentSite.config || {};
      
      // 处理数据库编号：如果为空且 mdb_name="ALL"，使用 project_code 作为默认
      let manualDbNums = Array.isArray(config.manual_db_nums) ? config.manual_db_nums : [];
      if (manualDbNums.length === 0 && config.mdb_name === "ALL" && config.project_code) {
        manualDbNums = [config.project_code];
      }
      
      const taskPayload = {
        name: `解析任务-${deploymentSite.name || id}`,
        task_type: 'ParsePdmsData',
        config: {
          ...config,  // 保留所有原配置字段
          manual_db_nums: manualDbNums,  // 确保此字段不为空
          gen_model: false,  // 只解析，不生成模型
          gen_mesh: false,
          gen_spatial_tree: false
        }
      };
      
      const createResp = await fetch('/api/tasks', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(taskPayload)
      });
      
      if (createResp.ok) {
        const result = await createResp.json();
        // 创建后立即启动任务
        try {
          const startResp = await fetch(`/api/tasks/${encodeURIComponent(result.id)}/start`, { method: 'POST' });
          if (!startResp.ok) {
            const msg = await startResp.text();
            throw new Error(`启动失败: ${msg}`);
          }
        } catch (e) {
          console.error('任务启动失败:', e);
          alert(`任务已创建，但启动失败：${e.message}`);
          return;
        }
        // 启动成功后跳转到任务管理页面
        window.location.href = '/tasks';
      } else {
        let errorText;
        try {
          const errJson = await createResp.json();
          errorText = errJson.error || JSON.stringify(errJson);
        } catch (_) {
          errorText = await createResp.text();
        }
        console.error('任务创建失败:', errorText);
        throw new Error(`任务创建失败: ${errorText}`);
      }
    } catch (error) {
      console.error('启动解析任务失败:', error);
      alert(`启动解析任务失败: ${error.message}`);
    }
  };

  window.launchModelingTask = async function(encodedId) {
    const id = decodeURIComponent(encodedId || '');
    
    try {
      const resp = await fetch('/api/deployment-sites/' + encodeURIComponent(id));
      if (!resp.ok) throw new Error('无法获取部署站点配置');
      
      const deploymentSite = await resp.json();
      const config = deploymentSite.config || {};
      
      // 处理数据库编号：如果为空且 mdb_name="ALL"，使用 project_code 作为默认
      let manualDbNums = Array.isArray(config.manual_db_nums) ? config.manual_db_nums : [];
      if (manualDbNums.length === 0 && config.mdb_name === "ALL" && config.project_code) {
        manualDbNums = [config.project_code];
      }

      const taskPayload = {
        name: `建模任务-${deploymentSite.name || id}`,
        task_type: 'GenerateModel',
        config: {
          ...config,
          manual_db_nums: manualDbNums,
          gen_model: true,
          gen_mesh: config.gen_mesh !== false,  // 保持原配置
          gen_spatial_tree: false  // 建模时不生成空间树
        }
      };
      
      const createResp = await fetch('/api/tasks', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(taskPayload)
      });
      
      if (createResp.ok) {
        const result = await createResp.json();
        // 直接跳转到任务管理页面，无需弹窗
        window.location.href = '/tasks';
      } else {
        let errorText;
        try { const errJson = await createResp.json(); errorText = errJson.error || JSON.stringify(errJson); }
        catch(_) { errorText = await createResp.text(); }
        throw new Error(`任务创建失败: ${errorText}`);
      }
    } catch (error) {
      console.error('启动建模任务失败:', error);
      alert(`启动建模任务失败: ${error.message}`);
    }
  };

  window.launchSpatialTask = async function(encodedId) {
    const id = decodeURIComponent(encodedId || '');
    
    try {
      const resp = await fetch('/api/deployment-sites/' + encodeURIComponent(id));
      if (!resp.ok) throw new Error('无法获取部署站点配置');
      
      const deploymentSite = await resp.json();
      const config = deploymentSite.config || {};
      
      // 处理数据库编号：如果为空且 mdb_name="ALL"，使用 project_code 作为默认
      let manualDbNums = Array.isArray(config.manual_db_nums) ? config.manual_db_nums : [];
      if (manualDbNums.length === 0 && config.mdb_name === "ALL" && config.project_code) {
        manualDbNums = [config.project_code];
      }

      const taskPayload = {
        name: `空间索引任务-${deploymentSite.name || id}`,
        task_type: 'GenerateSpatialIndex',
        config: {
          ...config,
          manual_db_nums: manualDbNums,
          gen_model: false,  // 空间索引任务只生成空间树
          gen_mesh: false,
          gen_spatial_tree: true
        }
      };
      
      const createResp = await fetch('/api/tasks', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(taskPayload)
      });
      
      if (createResp.ok) {
        const result = await createResp.json();
        // 直接跳转到任务管理页面，无需弹窗
        window.location.href = '/tasks';
      } else {
        let errorText;
        try { const errJson = await createResp.json(); errorText = errJson.error || JSON.stringify(errJson); }
        catch(_) { errorText = await createResp.text(); }
        throw new Error(`任务创建失败: ${errorText}`);
      }
    } catch (error) {
      console.error('启动空间索引任务失败:', error);
      alert(`启动空间索引任务失败: ${error.message}`);
    }
  };

  // 编辑站点配置功能
  window.editSiteConfiguration = async function(encodedId) {
    const id = decodeURIComponent(encodedId || '');
    
    try {
      // 获取当前配置
      const resp = await fetch('/api/deployment-sites/' + encodeURIComponent(id));
      if (!resp.ok) throw new Error('无法获取部署站点配置');
      
      const deploymentSite = await resp.json();
      const config = deploymentSite.config || {};
      
      // 创建编辑表单模态框
      const modalHtml = `
        <div id="config-edit-modal" class="fixed inset-0 bg-gray-500 bg-opacity-75 flex items-center justify-center z-50">
          <div class="bg-white rounded-lg p-6 w-full max-w-4xl max-h-[90vh] overflow-y-auto">
            <div class="flex justify-between items-center mb-4">
              <h3 class="text-lg font-medium">编辑配置 - ${escHtml(deploymentSite.name)}</h3>
              <button onclick="closeConfigEditModal()" class="text-gray-400 hover:text-gray-600">
                <i class="fas fa-times text-xl"></i>
              </button>
            </div>
            
            <form id="config-edit-form" onsubmit="saveConfigChanges(event, '${escHtml(id)}')">
              <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
                
                <!-- 项目信息 -->
                <div class="space-y-4">
                  <h4 class="text-md font-medium text-gray-700 border-b border-gray-200 pb-2">项目信息</h4>
                  
                  <div>
                    <label class="block text-sm font-medium text-gray-700 mb-1">配置名称</label>
                    <input type="text" name="name" value="${escHtml(config.name || '')}" 
                           class="w-full px-3 py-2 border border-gray-300 rounded-md">
                  </div>
                  
                  <div>
                    <label class="block text-sm font-medium text-gray-700 mb-1">项目名称</label>
                    <input type="text" name="project_name" value="${escHtml(config.project_name || '')}" 
                           class="w-full px-3 py-2 border border-gray-300 rounded-md">
                  </div>
                  
                  <div>
                    <label class="block text-sm font-medium text-gray-700 mb-1">项目代码</label>
                    <input type="number" name="project_code" value="${escHtml(config.project_code || '')}" 
                           onchange="document.querySelector('input[name=surreal_ns]').value = this.value"
                           class="w-full px-3 py-2 border border-gray-300 rounded-md">
                  </div>
                  
                  <div>
                    <label class="block text-sm font-medium text-gray-700 mb-1">MDB名称</label>
                    <input type="text" name="mdb_name" value="${escHtml(config.mdb_name || '')}" 
                           class="w-full px-3 py-2 border border-gray-300 rounded-md">
                  </div>
                  
                  <div>
                    <label class="block text-sm font-medium text-gray-700 mb-1">模块类型</label>
                    <select name="module" class="w-full px-3 py-2 border border-gray-300 rounded-md">
                      <option value="DESI" ${config.module === 'DESI' ? 'selected' : ''}>DESI</option>
                      <option value="PIPE" ${config.module === 'PIPE' ? 'selected' : ''}>PIPE</option>
                      <option value="STRU" ${config.module === 'STRU' ? 'selected' : ''}>STRU</option>
                      <option value="HVAC" ${config.module === 'HVAC' ? 'selected' : ''}>HVAC</option>
                    </select>
                  </div>
                  
                  <div>
                    <label class="block text-sm font-medium text-gray-700 mb-1">数据库编号</label>
                    <input type="text" name="manual_db_nums" value="${escHtml((config.manual_db_nums || []).join(','))}" 
                           placeholder="多个编号用逗号分隔，留空表示自动扫描全部"
                           class="w-full px-3 py-2 border border-gray-300 rounded-md">
                  </div>
                </div>
                
                <!-- 数据库连接 -->
                <div class="space-y-4">
                  <h4 class="text-md font-medium text-gray-700 border-b border-gray-200 pb-2">数据库连接</h4>
                  
                  <div>
                    <label class="block text-sm font-medium text-gray-700 mb-1">数据库类型</label>
                    <select name="db_type" class="w-full px-3 py-2 border border-gray-300 rounded-md">
                      <option value="surrealdb" ${config.db_type === 'surrealdb' ? 'selected' : ''}>SurrealDB</option>
                      <option value="mysql" ${config.db_type === 'mysql' ? 'selected' : ''}>MySQL</option>
                      <option value="postgresql" ${config.db_type === 'postgresql' ? 'selected' : ''}>PostgreSQL</option>
                    </select>
                  </div>
                  
                  <div>
                    <label class="block text-sm font-medium text-gray-700 mb-1">数据库IP</label>
                    <input type="text" name="db_ip" value="${escHtml(config.db_ip || '')}" 
                           class="w-full px-3 py-2 border border-gray-300 rounded-md">
                  </div>
                  
                  <div>
                    <label class="block text-sm font-medium text-gray-700 mb-1">端口号</label>
                    <input type="text" name="db_port" value="${escHtml(config.db_port || '')}" 
                           class="w-full px-3 py-2 border border-gray-300 rounded-md">
                  </div>
                  
                  <div>
                    <label class="block text-sm font-medium text-gray-700 mb-1">用户名</label>
                    <input type="text" name="db_user" value="${escHtml(config.db_user || '')}" 
                           class="w-full px-3 py-2 border border-gray-300 rounded-md">
                  </div>
                  
                  <div>
                    <label class="block text-sm font-medium text-gray-700 mb-1">密码</label>
                    <div class="relative">
                      <input type="password" name="db_password" id="config-db-password-${projectCode}" 
                             value="${escHtml(config.db_password || '')}" 
                             class="w-full px-3 py-2 pr-10 border border-gray-300 rounded-md">
                      <button type="button" 
                              onclick="togglePasswordVisibility('config-db-password-${projectCode}', this)"
                              class="absolute inset-y-0 right-0 flex items-center pr-3 text-gray-500 hover:text-gray-700">
                        <svg class="w-5 h-5 eye-icon" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"></path>
                          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z"></path>
                        </svg>
                        <svg class="w-5 h-5 eye-slash-icon hidden" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13.875 18.825A10.05 10.05 0 0112 19c-4.478 0-8.268-2.943-9.543-7a9.97 9.97 0 011.563-3.029m5.858.908a3 3 0 114.243 4.243M9.878 9.878l4.242 4.242M9.88 9.88l-3.29-3.29m7.532 7.532l3.29 3.29M3 3l3.59 3.59m0 0A9.953 9.953 0 0112 5c4.478 0 8.268 2.943 9.543 7a10.025 10.025 0 01-4.132 5.411m0 0L21 21"></path>
                        </svg>
                      </button>
                    </div>
                  </div>
                  
                  <div>
                    <label class="block text-sm font-medium text-gray-700 mb-1">
                      Surreal 命名空间
                      <span class="text-xs text-gray-500 ml-1">(自动使用项目代码)</span>
                    </label>
                    <input type="number" name="surreal_ns" value="${escHtml(config.surreal_ns || config.project_code || '')}" 
                           readonly
                           class="w-full px-3 py-2 border border-gray-300 rounded-md bg-gray-100 cursor-not-allowed">
                  </div>
                </div>
                
                <!-- 生成选项 -->
                <div class="md:col-span-2 space-y-4">
                  <h4 class="text-md font-medium text-gray-700 border-b border-gray-200 pb-2">生成选项</h4>
                  
                  <div class="grid grid-cols-2 md:grid-cols-4 gap-4">
                    <label class="flex items-center">
                      <input type="checkbox" name="gen_model" ${config.gen_model ? 'checked' : ''} 
                             class="mr-2 h-4 w-4 text-blue-600">
                      <span class="text-sm">生成几何模型</span>
                    </label>
                    
                    <label class="flex items-center">
                      <input type="checkbox" name="gen_mesh" ${config.gen_mesh ? 'checked' : ''} 
                             class="mr-2 h-4 w-4 text-blue-600">
                      <span class="text-sm">生成网格数据</span>
                    </label>
                    
                    <label class="flex items-center">
                      <input type="checkbox" name="gen_spatial_tree" ${config.gen_spatial_tree ? 'checked' : ''} 
                             class="mr-2 h-4 w-4 text-blue-600">
                      <span class="text-sm">生成空间树</span>
                    </label>
                    
                    <label class="flex items-center">
                      <input type="checkbox" name="apply_boolean_operation" ${config.apply_boolean_operation ? 'checked' : ''} 
                             class="mr-2 h-4 w-4 text-blue-600">
                      <span class="text-sm">布尔运算</span>
                    </label>
                  </div>
                  
                  <div class="grid grid-cols-2 gap-4">
                    <div>
                      <label class="block text-sm font-medium text-gray-700 mb-1">网格容差比例</label>
                      <input type="number" name="mesh_tol_ratio" value="${escHtml(config.mesh_tol_ratio || 3)}" 
                             step="0.1" min="0.1" max="10" 
                             class="w-full px-3 py-2 border border-gray-300 rounded-md">
                    </div>
                    
                    <div>
                      <label class="block text-sm font-medium text-gray-700 mb-1">房间关键字</label>
                      <input type="text" name="room_keyword" value="${escHtml(config.room_keyword || '')}" 
                             class="w-full px-3 py-2 border border-gray-300 rounded-md">
                    </div>
                  </div>
                </div>
              </div>
              
              <div class="flex justify-end space-x-3 mt-6 pt-4 border-t border-gray-200">
                <button type="button" onclick="closeConfigEditModal()" 
                        class="px-4 py-2 border border-gray-300 rounded-md text-gray-700 hover:bg-gray-50">
                  取消
                </button>
                <button type="submit" 
                        class="px-4 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700">
                  保存配置
                </button>
              </div>
            </form>
          </div>
        </div>
      `;
      
      // 插入模态框
      document.body.insertAdjacentHTML('beforeend', modalHtml);
      
    } catch (error) {
      console.error('打开编辑配置失败:', error);
      alert(`打开编辑配置失败: ${error.message}`);
    }
  };

  // 关闭配置编辑模态框
  window.closeConfigEditModal = function() {
    const modal = document.getElementById('config-edit-modal');
    if (modal) {
      modal.remove();
    }
  };

  // 保存配置更改
  window.saveConfigChanges = async function(event, siteId) {
    event.preventDefault();
    
    try {
      const form = event.target;
      const formData = new FormData(form);
      
      // 构建配置对象
      const updatedConfig = {
        name: formData.get('name'),
        project_name: formData.get('project_name'),
        project_code: parseInt(formData.get('project_code')),
        mdb_name: formData.get('mdb_name'),
        module: formData.get('module'),
        manual_db_nums: formData.get('manual_db_nums').split(',').map(s => s.trim()).filter(s => s).map(s => parseInt(s)),
        db_type: formData.get('db_type'),
        db_ip: formData.get('db_ip'),
        db_port: formData.get('db_port'),
        db_user: formData.get('db_user'),
        db_password: formData.get('db_password'),
        surreal_ns: parseInt(formData.get('surreal_ns')) || null,
        gen_model: formData.has('gen_model'),
        gen_mesh: formData.has('gen_mesh'),
        gen_spatial_tree: formData.has('gen_spatial_tree'),
        apply_boolean_operation: formData.has('apply_boolean_operation'),
        mesh_tol_ratio: parseFloat(formData.get('mesh_tol_ratio')),
        room_keyword: formData.get('room_keyword'),
        target_sesno: null
      };
      
      // 发送更新请求
      const updateResp = await fetch('/api/deployment-sites/' + encodeURIComponent(siteId), {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ config: updatedConfig })
      });
      
      if (updateResp.ok) {
        alert('配置已成功保存！');
        closeConfigEditModal();
        
        // 如果当前打开的是这个站点的详情，刷新详情
        if (CURRENT_ID === siteId) {
          loadProjectDetail(siteId);
        }
      } else {
        const errorText = await updateResp.text();
        throw new Error(`保存失败: ${errorText}`);
      }
    } catch (error) {
      console.error('保存配置失败:', error);
      alert(`保存配置失败: ${error.message}`);
    }
  };

  document.addEventListener('DOMContentLoaded', ()=>{
    const q = $('site_q'), st = $('site_status'), env = $('site_env'), owner = $('site_owner');
    const apply = ()=>{ FILTERS = { q:q?.value.trim()||'', status:st?.value||'', env:env?.value||'', owner:owner?.value.trim()||'' }; loadProjects(); };
    if(q){ q.addEventListener('input', ()=>{ clearTimeout(q._t); q._t = setTimeout(apply, 300); }); }
    if(st){ st.addEventListener('change', apply); }
    if(env){ env.addEventListener('change', apply); }
    if(owner){ owner.addEventListener('input', ()=>{ clearTimeout(owner._t); owner._t = setTimeout(apply, 300); }); }
    loadProjects();
  });
})();
