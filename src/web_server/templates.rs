/// HTML模板渲染函数

pub fn render_index_page() -> String {
    r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>AIOS 数据库管理平台</title>
    <link href="/static/simple-tailwind.css" rel="stylesheet">
    <link href="/static/simple-icons.css" rel="stylesheet">
    <script src="/static/alpine.min.js" defer></script>
</head>
<body class="bg-gray-50">
    <div class="min-h-screen">
        <!-- 导航栏 -->
        <nav class="bg-blue-600 text-white shadow-lg">
            <div class="max-w-7xl mx-auto px-4">
                <div class="flex justify-between items-center py-4">
                    <div class="flex items-center space-x-4">
                        <i class="fas fa-database text-2xl"></i>
                        <h1 class="text-xl font-bold">AIOS 数据库管理平台</h1>
                    </div>
                    <div class="flex space-x-4">
                        <a href="/dashboard" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tachometer-alt mr-2"></i>仪表板
                        </a>
                        <a href="/tasks" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tasks mr-2"></i>任务管理
                        </a>
                        <a href="/batch-tasks" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-layer-group mr-2"></i>批量任务
                        </a>
                        <a href="/config" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-cog mr-2"></i>配置管理
                        </a>
                        <a href="/db-status" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-database mr-2"></i>数据库状态
                        </a>
                        <a href="/sqlite-spatial" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-vector-square mr-2"></i>空间查询
                        </a>
                        <a href="/wizard" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-magic mr-2"></i>解析向导
                        </a>
                    </div>
                </div>
            </div>
        </nav>

        <!-- 主要内容 -->
        <div class="max-w-7xl mx-auto px-4 py-8">
            <!-- 欢迎区域 -->
            <div class="bg-white rounded-lg shadow-md p-8 mb-8">
                <div class="text-center">
                    <h2 class="text-3xl font-bold text-gray-800 mb-4">
                        欢迎使用 AIOS 数据库管理平台
                    </h2>
                    <p class="text-gray-600 text-lg mb-6">
                        强大的数据库生成和空间树管理工具，支持实时监控和配置管理
                    </p>
                    <div class="flex justify-center space-x-4">
                        <a href="/dashboard" class="bg-blue-600 text-white px-6 py-3 rounded-lg hover:bg-blue-700 transition">
                            <i class="fas fa-chart-line mr-2"></i>查看仪表板
                        </a>
                        <a href="/tasks" class="bg-green-600 text-white px-6 py-3 rounded-lg hover:bg-green-700 transition">
                            <i class="fas fa-plus mr-2"></i>创建新任务
                        </a>
                    </div>
                </div>
            </div>

            <!-- 功能卡片 -->
            <div class="grid md:grid-cols-4 gap-6">
                <div class="bg-white rounded-lg shadow-md p-6">
                    <div class="flex items-center mb-4">
                        <div class="bg-blue-100 p-3 rounded-full">
                            <i class="fas fa-database text-blue-600 text-xl"></i>
                        </div>
                        <h3 class="text-xl font-semibold ml-4">数据生成</h3>
                    </div>
                    <p class="text-gray-600 mb-4">
                        支持指定数据库编号进行数据生成，包括几何数据、网格数据和布尔运算处理
                    </p>
                    <ul class="text-sm text-gray-500 space-y-1">
                        <li>• 支持多数据库并行处理</li>
                        <li>• 实时进度监控</li>
                        <li>• 错误处理和重试机制</li>
                    </ul>
                </div>

                <div class="bg-white rounded-lg shadow-md p-6">
                    <div class="flex items-center mb-4">
                        <div class="bg-green-100 p-3 rounded-full">
                            <i class="fas fa-sitemap text-green-600 text-xl"></i>
                        </div>
                        <h3 class="text-xl font-semibold ml-4">空间树生成</h3>
                    </div>
                    <p class="text-gray-600 mb-4">
                        自动构建房间关系和空间层级结构，支持自定义房间关键字匹配
                    </p>
                    <ul class="text-sm text-gray-500 space-y-1">
                        <li>• 智能房间识别</li>
                        <li>• 空间关系计算</li>
                        <li>• AABB树优化</li>
                    </ul>
                </div>

                <div class="bg-white rounded-lg shadow-md p-6">
                    <div class="flex items-center mb-4">
                        <div class="bg-purple-100 p-3 rounded-full">
                            <i class="fas fa-cogs text-purple-600 text-xl"></i>
                        </div>
                        <h3 class="text-xl font-semibold ml-4">配置管理</h3>
                    </div>
                    <p class="text-gray-600 mb-4">
                        灵活的配置管理系统，支持配置模板和批量操作
                    </p>
                    <ul class="text-sm text-gray-500 space-y-1">
                        <li>• 配置模板管理</li>
                        <li>• 参数验证</li>
                        <li>• 配置导入导出</li>
                    </ul>
                </div>

                <!-- 数据库同步状态 -->
                <div class="bg-white rounded-lg shadow-md p-6">
                    <div class="flex items-center mb-4">
                        <div class="bg-purple-100 p-3 rounded-full">
                            <i class="fas fa-sync-alt text-purple-600 text-xl"></i>
                        </div>
                        <h3 class="text-xl font-semibold ml-4">数据库同步状态</h3>
                    </div>
                    <div class="grid grid-cols-3 gap-3 text-center mb-4">
                        <div>
                            <div class="text-sm text-gray-500">总数据库</div>
                            <div id="syncTotal" class="text-xl font-bold text-gray-800">-</div>
                        </div>
                        <div>
                            <div class="text-sm text-gray-500">需更新</div>
                            <div id="syncNeeds" class="text-xl font-bold text-orange-600">-</div>
                        </div>
                        <div>
                            <div class="text-sm text-gray-500">更新中</div>
                            <div id="syncUpdating" class="text-xl font-bold text-blue-600">-</div>
                        </div>
                    </div>
                    <a href="/db-status" class="inline-block bg-purple-600 text-white px-4 py-2 rounded hover:bg-purple-700 transition">
                        查看同步状态
                    </a>
                </div>
            </div>

            <!-- 已部署项目列表 -->
            <div class="bg-white rounded-lg shadow-md p-6 mt-8">
                <div class="flex justify-between items-center mb-4">
                    <h3 class="text-xl font-semibold">已部署项目</h3>
                    <div class="space-x-4">
                        <button onclick="toggleCreateForm()" class="bg-blue-600 text-white px-3 py-1.5 rounded hover:bg-blue-700">新建项目</button>
                        <a href="#" onclick="reloadProjectsCards(); return false;" class="text-blue-600 hover:underline ">刷新</a>
                        <a href="# " onclick="bootstrapDemo(); return false;" class="text-gray-600 hover:underline ">添加示例</a>
                    </div>
                </div>
                <!-- 新建项目表单（默认隐藏） -->
                <div id="createForm" class="hidden border border-gray-200 rounded p-4 mb-4">
                    <div class="grid md:grid-cols-3 gap-4">
                        <div>
                            <label class="block text-sm text-gray-600 mb-1">名称</label>
                            <input id="proj_name" class="w-full border rounded px-3 py-2" placeholder="如: demo " />
                        </div>
                        <div>
                            <label class="block text-sm text-gray-600 mb-1">环境</label>
                            <input id="proj_env" class="w-full border rounded px-3 py-2" placeholder="dev/staging/prod " />
                        </div>
                        <div>
                            <label class="block text-sm text-gray-600 mb-1">负责人</label>
                            <input id="proj_owner" class="w-full border rounded px-3 py-2" placeholder="如: alice " />
                        </div>
                        <div>
                            <label class="block text-sm text-gray-600 mb-1">版本</label>
                            <input id="proj_version" class="w-full border rounded px-3 py-2" placeholder="v1.0.0" />
                        </div>
                        <div class="md:col-span-2">
                            <label class="block text-sm text-gray-600 mb-1">访问地址</label>
                            <input id="proj_url" class="w-full border rounded px-3 py-2" placeholder="http://localhost:9000" />
                        </div>
                        <div class="md:col-span-3">
                            <label class="block text-sm text-gray-600 mb-1">健康检查地址</label>
                            <input id="proj_health" class="w-full border rounded px-3 py-2" placeholder="http://localhost:9000/health" />
                        </div>
                    </div>
                    <div class="mt-4 flex items-center space-x-3">
                        <button onclick="createProject()" class="bg-green-600 text-white px-4 py-2 rounded hover:bg-green-700">保存</button>
                        <button onclick="toggleCreateForm()" class="px-4 py-2 rounded border">取消</button>
                        <span id="createMsg" class="text-sm text-gray-500"></span>
                    </div>
                </div>
                <!-- 卡片容器 -->
                <div id="projectsGrid" class="grid gap-4 grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
                    <div class="col-span-full text-gray-500">加载中...</div>
                </div>
            </div>

            <!-- 快速操作 -->
            <div class="bg-white rounded-lg shadow-md p-6 mt-8">
                <h3 class="text-xl font-semibold mb-4">快速操作</h3>
                <div class="grid md:grid-cols-2 gap-4">
                    <div class="border border-gray-200 rounded-lg p-4">
                        <h4 class="font-semibold mb-2">
                            <i class="fas fa-rocket text-blue-600 mr-2"></i>
                            数据库 7999 生成
                        </h4>
                        <p class="text-gray-600 text-sm mb-3">
                            使用预设配置快速生成数据库编号 7999 的数据和空间树
                        </p>
                        <button onclick="createQuickTask(7999)" 
                                class="bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700 transition">
                            立即执行
                        </button>
                    </div>
                    <div class="border border-gray-200 rounded-lg p-4">
                        <h4 class="font-semibold mb-2">
                            <i class="fas fa-list text-green-600 mr-2"></i>
                            查看任务状态
                        </h4>
                        <p class="text-gray-600 text-sm mb-3">
                            查看当前运行的任务和历史记录
                        </p>
                        <a href="/tasks" class="bg-green-600 text-white px-4 py-2 rounded hover:bg-green-700 transition inline-block">
                            查看任务
                        </a>
                    </div>
                </div>
            </div>
        </div>
    </div>

    <script>
        async function createQuickTask(dbNum) {
            try {
                const response = await fetch("/api/tasks", {
                    method: "POST",
                    headers: {
                        "Content-Type": "application/json",
                    },
                    body: JSON.stringify({
                        name: "数据库 " + dbNum + " 快速生成",
                        task_type: "FullGeneration",
                        config: {
                            name: "数据库 " + dbNum + " 配置",
                            manual_db_nums: [dbNum],
                            gen_model: true,
                            gen_mesh: true,
                            gen_spatial_tree: true,
                            apply_boolean_operation: true,
                            mesh_tol_ratio: 3.0,
                            room_keyword: "-RM",
                            project_name: "AvevaMarineSample",
                            project_code: 1516
                        }
                    })
                });
                
                if (response.ok) {
                    const task = await response.json();
                    // 启动任务
                    await fetch("/api/tasks/" + task.id + "/start", { method: "POST" });
                    alert("任务创建成功！正在跳转到任务管理页面...");
                    window.location.href = "/tasks";
                } else {
                    alert("任务创建失败，请稍后重试");
                }
            } catch (error) {
                console.error("Error:", error);
                alert("网络错误，请检查连接");
            }
        }

        // 加载数据库同步概览
        (async function loadDbSyncSummary(){
            try {
                const res = await fetch("/api/db-status");
                const data = await res.json();
                if (data && data.status === "success") {
                    const list = data.data || [];
                    const total = list.length;
                    const needs = list.filter(x => x.needs_update).length;
                    const updating = list.filter(x => x.updating).length;
                    document.getElementById("syncTotal").textContent = total;
                    document.getElementById("syncNeeds").textContent = needs;
                    document.getElementById("syncUpdating").textContent = updating;
                }
            } catch(e) {
                // 忽略错误，保持占位符
            }
        })();

        function toggleCreateForm(){
            const f = document.getElementById("createForm");
            f.classList.toggle("hidden");
            document.getElementById("createMsg").textContent="";
        }

        async function reloadProjects() {
            try {
                const res = await fetch("/api/projects");
                const data = await res.json();
                const tbody = document.getElementById("projectsBody");
                const items = data.items || [];
                if (items.length === 0) {
                    tbody.innerHTML = "<tr><td class="px-4 py-3 text-gray-500" colspan="8">暂无项目</td></tr>";
                    return;
                }
                tbody.innerHTML = items.map(p => {
                    const badgeColor = p.status === "Running" ? "bg-green-100 text-green-800" : (p.status === "Deploying" ? "bg-blue-100 text-blue-800" : (p.status === "Failed" ? "bg-red-100 text-red-800" : "bg-gray-100 text-gray-800"));
                    const url = p.url ? `<a class=\"text-blue-600 hover:underline\" href=\"${p.url}\" target=\"_blank\">打开</a>` : "";
                    return `
                        <tr>
                            <td class="px-4 py-3 font-medium text-gray-900">${p.name || ""}</td>
                            <td class="px-4 py-3 text-gray-700">${p.env || ""}</td>
                            <td class="px-4 py-3"><span class="px-2 py-1 rounded text-xs ${badgeColor}">${p.status || ""}</span></td>
                            <td class="px-4 py-3 text-gray-700">${p.owner || ""}</td>
                            <td class="px-4 py-3 text-gray-700">${p.version || ""}</td>
                            <td class="px-4 py-3">${url}</td>
                            <td class="px-4 py-3 text-gray-500">${p.updated_at || ""}</td>
                            <td class="px-4 py-3 text-sm space-x-2">
                               <button onclick="healthcheckProject("${p.id || ""}")" class="px-2 py-1 rounded border">健康检查</button>
                               <button onclick="deleteProject("${p.id || ""}")" class="px-2 py-1 rounded border text-red-600">删除</button>
                            </td>
                        </tr>`;
                }).join("");
            } catch (e) {
                const tbody = document.getElementById("projectsBody");
                tbody.innerHTML = "<tr><td class="px-4 py-3 text-red-500" colspan="8">加载失败，请稍后重试</td></tr>";
            }
        }

        // 首次加载（卡片渲染）
        reloadProjectsCards();

        async function createProject(){
            const name = document.getElementById("proj_name").value.trim();
            const env = document.getElementById("proj_env").value.trim();
            const owner = document.getElementById("proj_owner").value.trim();
            const version = document.getElementById("proj_version").value.trim();
            const url = document.getElementById("proj_url").value.trim();
            const health = document.getElementById("proj_health").value.trim();
            const msg = document.getElementById("createMsg");
            if(!name){ msg.textContent="名称必填"; return; }
            try {
                const res = await fetch("/api/projects", {
                    method:"POST", headers:{"Content-Type":"application/json"},
                    body: JSON.stringify({ name, env, owner, version, url, health_url: health, status: "Running" })
                });
                if(!res.ok){ const e=await res.json().catch(()=>({error:"失败"})); msg.textContent = e.error || "创建失败"; return; }
                msg.textContent = "创建成功";
                reloadProjectsCards();
                setTimeout(()=>{ toggleCreateForm(); }, 400);
            } catch(e){ msg.textContent="网络错误"; }
        }

        async function healthcheckProject(id){
            if(!id) return;
            try { await fetch(`/api/projects/${encodeURIComponent(id)}/healthcheck`, { method:"POST"}); reloadProjectsCards(); }
            catch(e){}
        }

        async function deleteProject(id){
            if(!id) return;
            if(!confirm("确认删除该项目？")) return;
            try { await fetch(`/api/projects/${encodeURIComponent(id)}`, { method:"DELETE"}); reloadProjectsCards(); }
            catch(e){}
        }

        // 卡片版项目加载
        async function reloadProjectsCards() {
            try {
                const res = await fetch("/api/projects");
                const data = await res.json();
                const grid = document.getElementById("projectsGrid");
                const items = data.items || [];
                if (items.length === 0) {
                    grid.innerHTML = "<div class="col-span-full text-gray-500">暂无项目</div>";
                    return;
                }
                grid.innerHTML = items.map(p => renderProjectCard(p)).join("");
            } catch (e) {
                const grid = document.getElementById("projectsGrid");
                grid.innerHTML = "<div class="col-span-full text-red-500">加载失败，请稍后重试</div>";
            }
        }

        function renderProjectCard(p){
            const badge = (status)=>{
                switch(status){
                    case "Running": return "bg-green-100 text-green-700";
                    case "Deploying": return "bg-blue-100 text-blue-700";
                    case "Failed": return "bg-red-100 text-red-700";
                    case "Stopped": return "bg-gray-100 text-gray-700";
                    default: return "bg-gray-100 text-gray-700";
                }
            };
            const safe = (v)=> v ?? "";
            const openBtn = p.url ? `<a href="${p.url}" target="_blank" class="inline-flex items-center px-3 py-1.5 rounded bg-blue-600 text-white hover:bg-blue-700 text-sm"><i class=\"fas fa-up-right-from-square mr-2\"></i>打开</a>` : "";
            return `
                <div class="border rounded-lg p-4 hover:shadow transition">
                    <div class="flex items-start justify-between">
                        <div>
                            <div class="flex items-center space-x-2">
                                <h4 class="text-lg font-semibold text-gray-900">${safe(p.name)}</h4>
                                <span class="px-2 py-0.5 rounded text-xs ${badge(p.status)}">${safe(p.status)}</span>
                            </div>
                            <div class="text-sm text-gray-500 mt-1">环境: ${safe(p.env)}</div>
                        </div>
                        <div class="text-sm text-gray-500">${safe(p.updated_at)}</div>
                    </div>
                    <div class="mt-3 grid grid-cols-2 gap-2 text-sm">
                        <div class="text-gray-600">负责人: <span class="text-gray-800">${safe(p.owner)}</span></div>
                        <div class="text-gray-600">版本: <span class="text-gray-800">${safe(p.version)}</span></div>
                    </div>
                    <div class="mt-4 flex items-center justify-between">
                        <div class="space-x-2">
                            ${openBtn}
                            <button onclick="healthcheckProject("${safe(p.id)}")" class="inline-flex items-center px-3 py-1.5 rounded border text-sm"><i class="fas fa-heart-pulse mr-2"></i>健康检查</button>
                            <button onclick="editProject("${safe(p.id)}")" class="inline-flex items-center px-3 py-1.5 rounded border text-sm"><i class="fas fa-pen-to-square mr-2"></i>编辑</button>
                            <a href="/wizard" class="inline-flex items-center px-3 py-1.5 rounded border text-sm"><i class="fas fa-magic mr-2"></i>进入向导</a>
                        </div>
                        <button onclick="deleteProject("${safe(p.id)}")" class="inline-flex items-center px-3 py-1.5 rounded border text-red-600 text-sm"><i class="fas fa-trash mr-2"></i>删除</button>
                    </div>
                </div>`;
        }

        async function bootstrapDemo(){
            try{
                const res = await fetch("/api/projects/demo", {method:"POST"});
                if(res.ok){ reloadProjectsCards(); }
                else{ alert("添加示例失败"); }
            }catch(e){ alert("网络错误"); }
        }

        async function editProject(id){
            try{
                const res = await fetch(`/api/projects/${encodeURIComponent(id)}`);
                if(!res.ok){ alert("读取失败"); return; }
                const data = await res.json();
                const p = data.item || {};
                const name = prompt("名称", p.name || ""); if(name===null) return;
                const env = prompt("环境", p.env || ""); if(env===null) return;
                const version = prompt("版本", p.version || ""); if(version===null) return;
                const url = prompt("访问地址", p.url || ""); if(url===null) return;
                const owner = prompt("负责人", p.owner || ""); if(owner===null) return;
                const status = prompt("状态(Running/Deploying/Failed/Stopped)", p.status || "Running"); if(status===null) return;
                const health_url = prompt("健康检查地址", p.health_url || ""); if(health_url===null) return;

                const payload = { name, env, version, url, owner, status, health_url };
                const upd = await fetch(`/api/projects/${encodeURIComponent(id)}`, { method:"PUT", headers:{"Content-Type":"application/json"}, body: JSON.stringify(payload)});
                if(upd.ok){ reloadProjectsCards(); } else { alert("更新失败"); }
            }catch(e){ alert("网络错误"); }
        }
    </script>
</body>
</html>
    "#.to_string()
}

/// SQLite 空间索引测试页面
pub fn render_sqlite_spatial_page() -> String {
    r#"<!DOCTYPE html>
<html lang=\"zh-CN\">
<head>
  <meta charset=\"UTF-8\" />
  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\" />
  <title>SQLite 空间索引测试</title>
    <link href=\"/static/simple-tailwind.css\" rel=\"stylesheet\"> 
  <script src=\"/static/alpine.min.js\" defer></script>
</head>
<body class=\"bg-gray-50\">
  <div class=\"max-w-5xl mx-auto px-4 py-6\">
    <h1 class=\"text-2xl font-bold mb-4\">SQLite 空间索引测试</h1>
    <div class=\"bg-white rounded shadow p-4 mb-6\">
      <h2 class=\"font-semibold mb-2\">构建/重建索引</h2>
      <p class=\"text-sm text-gray-600 mb-2\">从本地 redb 缓存导入 AABB 到 SQLite RTree。</p>
      <button id=\"rebuildBtn\" class=\"bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700\">开始重建</button>
      <span id=\"rebuildMsg\" class=\"ml-3 text-sm\"></span>
    </div>

    <div class=\"bg-white rounded shadow p-4\">
      <h2 class=\"font-semibold mb-3\">AABB 相交查询</h2>
      <div class=\"grid grid-cols-2 md:grid-cols-6 gap-3\">
        <div><label class=\"text-xs text-gray-500\">min_x</label><input id=\"minx\" type=\"number\" step=\"0.01\" class=\"w-full border rounded px-2 py-1\" value=\"0\"/></div>
        <div><label class=\"text-xs text-gray-500\">max_x</label><input id=\"maxx\" type=\"number\" step=\"0.01\" class=\"w-full border rounded px-2 py-1\" value=\"1\"/></div>
        <div><label class=\"text-xs text-gray-500\">min_y</label><input id=\"miny\" type=\"number\" step=\"0.01\" class=\"w-full border rounded px-2 py-1\" value=\"0\"/></div>
        <div><label class=\"text-xs text-gray-500\">max_y</label><input id=\"maxy\" type=\"number\" step=\"0.01\" class=\"w-full border rounded px-2 py-1\" value=\"1\"/></div>
        <div><label class=\"text-xs text-gray-500\">min_z</label><input id=\"minz\" type=\"number\" step=\"0.01\" class=\"w-full border rounded px-2 py-1\" value=\"0\"/></div>
        <div><label class=\"text-xs text-gray-500\">max_z</label><input id=\"maxz\" type=\"number\" step=\"0.01\" class=\"w-full border rounded px-2 py-1\" value=\"1\"/></div>
      </div>
      <div class=\"mt-3 flex items-center gap-3\">
        <button id=\"queryBtn\" class=\"bg-green-600 text-white px-4 py-2 rounded hover:bg-green-700\">查询</button>
        <span id=\"queryMsg\" class=\"text-sm\"></span>
      </div>
      <div class=\"mt-4 overflow-x-auto\">
        <table class=\"min-w-full text-sm\">
          <thead><tr class=\"text-left text-gray-500\"><th class=\"py-1 pr-4\">#</th><th class=\"py-1 pr-4\">Refno</th><th class=\"py-1 pr-4\">AABB</th></tr></thead>
          <tbody id=\"resultBody\"></tbody>
        </table>
      </div>
    </div>
  </div>
  <script>
    async function rebuild() {
      const msg = document.getElementById("rebuildMsg");
      msg.textContent = "执行中...";
      try {
        const res = await fetch("/api/sqlite-spatial/rebuild", { method: "POST" });
        const data = await res.json();
        if (data.success) {
          msg.textContent = `完成，导入 ${data.rows} 条记录`;
        } else {
          msg.textContent = `失败：${data.error || "未知错误"}`;
        }
      } catch (e) {
        msg.textContent = "网络错误";
      }
    }
    async function query() {
      const q = new URLSearchParams({
        minx: document.getElementById("minx").value,
        maxx: document.getElementById("maxx").value,
        miny: document.getElementById("miny").value,
        maxy: document.getElementById("maxy").value,
        minz: document.getElementById("minz").value,
        maxz: document.getElementById("maxz").value,
      });
      const msg = document.getElementById("queryMsg");
      msg.textContent = "查询中...";
      try {
        const res = await fetch("/api/sqlite-spatial/query?" + q.toString());
        const data = await res.json();
        msg.textContent = `共 ${data.results?.length || 0} 条`;
        const body = document.getElementById("resultBody");
        body.innerHTML = "";
        (data.results || []).forEach((r, idx) => {
          const tr = document.createElement("tr");
          tr.innerHTML = `<td class="py-1 pr-4">${idx+1}</td>`+
            `<td class="py-1 pr-4">${r.refno}</td>`+
            `<td class="py-1 pr-4">[${r.aabb?.min?.x?.toFixed(2)||"-"}, ${r.aabb?.min?.y?.toFixed(2)||"-"}, ${r.aabb?.min?.z?.toFixed(2)||"-"}] → `+
            `[${r.aabb?.max?.x?.toFixed(2)||"-"}, ${r.aabb?.max?.y?.toFixed(2)||"-"}, ${r.aabb?.max?.z?.toFixed(2)||"-"}]`;
          body.appendChild(tr);
        });
      } catch (e) {
        msg.textContent = "网络错误";
      }
    }
    document.getElementById("rebuildBtn").addEventListener("click", rebuild);
    document.getElementById("queryBtn").addEventListener("click", query);
  </script>
</body>
</html>"#.to_string()
}

pub fn render_dashboard_page() -> String {
    r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>仪表板 - AIOS 数据库管理平台</title>
    <link href="/static/simple-tailwind.css" rel="stylesheet">
    <link href="/static/simple-icons.css" rel="stylesheet">
    <script src="/static/alpine.min.js" defer></script>
    <!-- 本地优先，若不存在则回退 CDN -->
    <script src="/static/chart.umd.min.js"></script>
    <script>if(typeof Chart==='undefined'){var s=document.createElement('script');s.src='https://cdn.jsdelivr.net/npm/chart.js';document.head.appendChild(s);}</script>
</head>
<body class="bg-gray-50" x-data="dashboard()">
    <div class="min-h-screen">
        <!-- 导航栏 -->
        <nav class="bg-blue-600 text-white shadow-lg">
            <div class="max-w-7xl mx-auto px-4">
                <div class="flex justify-between items-center py-4">
                    <div class="flex items-center space-x-4">
                        <i class="fas fa-database text-2xl"></i>
                        <h1 class="text-xl font-bold">AIOS 数据库管理平台</h1>
                    </div>
                    <div class="flex space-x-4">
                        <a href="/" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-home mr-2"></i>首页
                        </a>
                        <a href="/dashboard" class="bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tachometer-alt mr-2"></i>仪表板
                        </a>
                        <a href="/tasks" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tasks mr-2"></i>任务管理
                        </a>
                        <a href="/batch-tasks" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-layer-group mr-2"></i>批量任务
                        </a>
                        <a href="/db-status" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-database mr-2"></i>数据库状态
                        </a>
                        <a href="/config" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-cog mr-2"></i>配置管理
                        </a>
                    </div>
                </div>
            </div>
        </nav>

        <!-- 主要内容 -->
        <div class="max-w-7xl mx-auto px-4 py-8">
            <!-- 状态卡片 -->
            <div class="grid md:grid-cols-4 gap-6 mb-8">
                <div class="bg-white rounded-lg shadow-md p-6">
                    <div class="flex items-center justify-between">
                        <div>
                            <p class="text-gray-500 text-sm">活跃任务</p>
                            <p class="text-2xl font-bold text-blue-600" x-text="status.active_tasks"></p>
                        </div>
                        <i class="fas fa-play-circle text-blue-600 text-3xl"></i>
                    </div>
                </div>
                
                <div class="bg-white rounded-lg shadow-md p-6">
                    <div class="flex items-center justify-between">
                        <div>
                            <p class="text-gray-500 text-sm">CPU 使用率</p>
                            <p class="text-2xl font-bold text-green-600" x-text="status.cpu_usage + "%""></p>
                        </div>
                        <i class="fas fa-microchip text-green-600 text-3xl"></i>
                    </div>
                </div>
                
                <div class="bg-white rounded-lg shadow-md p-6">
                    <div class="flex items-center justify-between">
                        <div>
                            <p class="text-gray-500 text-sm">内存使用率</p>
                            <p class="text-2xl font-bold text-yellow-600" x-text="status.memory_usage + "%""></p>
                        </div>
                        <i class="fas fa-memory text-yellow-600 text-3xl"></i>
                    </div>
                </div>
                
                <div class="bg-white rounded-lg shadow-md p-6">
                    <div class="flex items-center justify-between">
                        <div>
                            <p class="text-gray-500 text-sm">数据库状态</p>
                            <p class="text-2xl font-bold" 
                               :class="status.database_connected ? "text-green-600" : "text-red-600""
                               x-text="status.database_connected ? "已连接" : "未连接""></p>
                        </div>
                        <i class="fas fa-database text-3xl"
                           :class="status.database_connected ? "text-green-600" : "text-red-600""></i>
                    </div>
                </div>
            </div>

            <!-- 快速操作面板 -->
            <div class="grid md:grid-cols-4 gap-4 mb-8">
                <a href="/tasks" class="bg-white rounded-lg shadow-md p-4 hover:shadow-lg transition-shadow">
                    <div class="flex items-center">
                        <div class="bg-blue-100 p-3 rounded-full mr-3">
                            <i class="fas fa-plus text-blue-600"></i>
                        </div>
                        <div>
                            <p class="text-sm text-gray-500">创建</p>
                            <p class="font-semibold">新任务</p>
                        </div>
                    </div>
                </a>
                
                <a href="/batch-tasks" class="bg-white rounded-lg shadow-md p-4 hover:shadow-lg transition-shadow">
                    <div class="flex items-center">
                        <div class="bg-green-100 p-3 rounded-full mr-3">
                            <i class="fas fa-layer-group text-green-600"></i>
                        </div>
                        <div>
                            <p class="text-sm text-gray-500">批量</p>
                            <p class="font-semibold">任务管理</p>
                        </div>
                    </div>
                </a>
                
                <a href="/db-status" class="bg-white rounded-lg shadow-md p-4 hover:shadow-lg transition-shadow">
                    <div class="flex items-center">
                        <div class="bg-purple-100 p-3 rounded-full mr-3">
                            <i class="fas fa-database text-purple-600"></i>
                        </div>
                        <div>
                            <p class="text-sm text-gray-500">数据库</p>
                            <p class="font-semibold">状态监控</p>
                        </div>
                    </div>
                </a>
                
                <a href="/config" class="bg-white rounded-lg shadow-md p-4 hover:shadow-lg transition-shadow">
                    <div class="flex items-center">
                        <div class="bg-orange-100 p-3 rounded-full mr-3">
                            <i class="fas fa-cog text-orange-600"></i>
                        </div>
                        <div>
                            <p class="text-sm text-gray-500">系统</p>
                            <p class="font-semibold">配置管理</p>
                        </div>
                    </div>
                </a>
            </div>

            <!-- 图表区域 -->
            <div class="grid md:grid-cols-2 gap-6 mb-8">
                <div class="bg-white rounded-lg shadow-md p-6">
                    <h3 class="text-lg font-semibold mb-4">任务执行趋势</h3>
                    <canvas id="taskChart" width="400" height="200"></canvas>
                </div>
                
                <div class="bg-white rounded-lg shadow-md p-6">
                    <h3 class="text-lg font-semibold mb-4">系统资源使用</h3>
                    <canvas id="resourceChart" width="400" height="200"></canvas>
                </div>
            </div>

            <!-- 最近任务 -->
            <div class="bg-white rounded-lg shadow-md p-6">
                <h3 class="text-lg font-semibold mb-4">最近任务</h3>
                <div class="overflow-x-auto">
                    <table class="min-w-full table-auto">
                        <thead>
                            <tr class="bg-gray-50">
                                <th class="px-4 py-2 text-left">任务名称</th>
                                <th class="px-4 py-2 text-left">类型</th>
                                <th class="px-4 py-2 text-left">状态</th>
                                <th class="px-4 py-2 text-left">进度</th>
                                <th class="px-4 py-2 text-left">创建时间</th>
                            </tr>
                        </thead>
                        <tbody>
                            <template x-for="task in recentTasks" :key="task.id">
                                <tr class="border-t">
                                    <td class="px-4 py-2" x-text="task.name"></td>
                                    <td class="px-4 py-2" x-text="task.task_type"></td>
                                    <td class="px-4 py-2">
                                        <span class="px-2 py-1 rounded-full text-xs"
                                              :class="getStatusClass(task.status)"
                                              x-text="getStatusText(task.status)"></span>
                                    </td>
                                    <td class="px-4 py-2">
                                        <div class="w-full bg-gray-200 rounded-full h-2">
                                            <div class="bg-blue-600 h-2 rounded-full"
                                                 :style="`width: ${task.progress.percentage}%`"></div>
                                        </div>
                                        <span class="text-xs text-gray-500" x-text="`${task.progress.percentage}%`"></span>
                                    </td>
                                    <td class="px-4 py-2 text-sm text-gray-500" x-text="formatTime(task.created_at)"></td>
                                </tr>
                            </template>
                        </tbody>
                    </table>
                </div>
            </div>
        </div>
    </div>

    <script>
        function dashboard() {
            return {
                status: {
                    active_tasks: 0,
                    cpu_usage: 0,
                    memory_usage: 0,
                    database_connected: false
                },
                recentTasks: [],
                
                async init() {
                    await this.loadStatus();
                    await this.loadRecentTasks();
                    this.initCharts();
                    
                    // 定期更新数据
                    setInterval(() => {
                        this.loadStatus();
                        this.loadRecentTasks();
                    }, 5000);
                },
                
                async loadStatus() {
                    try {
                        const response = await fetch("/api/status");
                        this.status = await response.json();
                    } catch (error) {
                        console.error("Failed to load status:", error);
                    }
                },
                
                async loadRecentTasks() {
                    try {
                        const response = await fetch("/api/tasks?limit=10");
                        const data = await response.json();
                        this.recentTasks = data.tasks;
                    } catch (error) {
                        console.error("Failed to load tasks:", error);
                    }
                },
                
                getStatusClass(status) {
                    const classes = {
                        "Pending": "bg-yellow-100 text-yellow-800",
                        "Running": "bg-blue-100 text-blue-800",
                        "Completed": "bg-green-100 text-green-800",
                        "Failed": "bg-red-100 text-red-800",
                        "Cancelled": "bg-gray-100 text-gray-800"
                    };
                    return classes[status] || "bg-gray-100 text-gray-800";
                },
                
                getStatusText(status) {
                    const texts = {
                        "Pending": "等待中",
                        "Running": "运行中",
                        "Completed": "已完成",
                        "Failed": "失败",
                        "Cancelled": "已取消"
                    };
                    return texts[status] || status;
                },
                
                formatTime(timestamp) {
                    if (!timestamp) return "未知时间";

                    // 处理不同的时间戳格式
                    let date;
                    if (typeof timestamp === "object" && timestamp.secs_since_epoch) {
                        // Rust SystemTime 格式: { secs_since_epoch: number, nanos_since_epoch: number }
                        date = new Date(timestamp.secs_since_epoch * 1000 + timestamp.nanos_since_epoch / 1000000);
                    } else if (typeof timestamp === "number") {
                        // Unix 时间戳（秒或毫秒）
                        date = timestamp > 1000000000000 ? new Date(timestamp) : new Date(timestamp * 1000);
                    } else if (typeof timestamp === "string") {
                        // ISO 字符串格式
                        date = new Date(timestamp);
                    } else {
                        // 尝试直接构造
                        date = new Date(timestamp);
                    }

                    // 检查日期是否有效
                    if (isNaN(date.getTime())) {
                        console.warn("Invalid timestamp:", timestamp);
                        return "无效时间";
                    }

                    return date.toLocaleString("zh-CN", {
                        year: "numeric",
                        month: "2-digit",
                        day: "2-digit",
                        hour: "2-digit",
                        minute: "2-digit",
                        second: "2-digit"
                    });
                },
                
                initCharts() {
                    // 任务执行趋势图
                    const taskCtx = document.getElementById("taskChart").getContext("2d");
                    new Chart(taskCtx, {
                        type: "line",
                        data: {
                            labels: ["1小时前", "45分钟前", "30分钟前", "15分钟前", "现在"],
                            datasets: [{
                                label: "完成任务数",
                                data: [2, 4, 3, 6, 8],
                                borderColor: "rgb(59, 130, 246)",
                                backgroundColor: "rgba(59, 130, 246, 0.1)",
                                tension: 0.4
                            }]
                        },
                        options: {
                            responsive: true,
                            scales: {
                                y: {
                                    beginAtZero: true
                                }
                            }
                        }
                    });
                    
                    // 系统资源使用图
                    const resourceCtx = document.getElementById("resourceChart").getContext("2d");
                    new Chart(resourceCtx, {
                        type: "doughnut",
                        data: {
                            labels: ["CPU", "内存", "磁盘"],
                            datasets: [{
                                data: [45, 68, 32],
                                backgroundColor: [
                                    "rgb(59, 130, 246)",
                                    "rgb(16, 185, 129)",
                                    "rgb(245, 158, 11)"
                                ]
                            }]
                        },
                        options: {
                            responsive: true
                        }
                    });
                }
            }
        }
    </script>
</body>
</html>
    "#.to_string()
}

/// 空间计算/桥架校核 工具页
pub fn render_space_tools_page() -> String {
    r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>空间计算工具 - AIOS</title>
    <link href="/static/simple-tailwind.css" rel="stylesheet">
    <link href="/static/simple-icons.css" rel="stylesheet">
    <script src="/static/alpine.min.js" defer></script>
    <style> code, pre { font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace; } </style>
    <script>
      async function postJSON(url, body){
        const res = await fetch(url, {method:"POST", headers:{"Content-Type":"application/json"}, body: JSON.stringify(body)});
        if(!res.ok) throw new Error(await res.text());
        return res.json();
      }
      function setResult(id, data){
        const el = document.getElementById(id);
        el.textContent = JSON.stringify(data, null, 2);
      }
    </script>
  </head>
  <body class="bg-gray-50">
    <nav class="bg-blue-600 text-white shadow-lg">
      <div class="max-w-7xl mx-auto px-4">
        <div class="flex justify-between items-center py-4">
          <div class="flex items-center space-x-3">
            <i class="fas fa-compass text-2xl"></i>
            <h1 class="text-xl font-bold">空间计算工具</h1>
          </div>
          <div class="flex space-x-4">
            <a href="/" class="hover:bg-blue-700 px-3 py-2 rounded">首页</a>
            <a href="/tasks" class="hover:bg-blue-700 px-3 py-2 rounded">任务</a>
          </div>
        </div>
      </div>
    </nav>

    <div class="max-w-7xl mx-auto px-4 py-6">
      <!-- 连接与范围 -->
      <div class="bg-white rounded-lg shadow p-6 mb-6">
        <h2 class="text-lg font-semibold mb-4">连接与范围</h2>
        <div class="grid md:grid-cols-5 gap-4">
          <div>
            <label class="text-sm text-gray-500">数据库号</label>
            <input id="dbnum" type="number" value="7999" class="w-full border rounded px-3 py-2"/>
          </div>
          <div>
            <label class="text-sm text-gray-500">SUPPO refno</label>
            <input id="suppo_refno" type="number" placeholder="必填" class="w-full border rounded px-3 py-2"/>
          </div>
          <div>
            <label class="text-sm text-gray-500">容差 (mm)</label>
            <input id="tol" type="number" step="0.1" value="2.0" class="w-full border rounded px-3 py-2"/>
          </div>
          <div>
            <label class="text-sm text-gray-500">类型</label>
            <select id="suppo_type" class="w-full border rounded px-3 py-2">
              <option value="S1">S1</option>
              <option value="S2">S2</option>
            </select>
          </div>
          <div>
            <label class="text-sm text-gray-500">搜索半径</label>
            <input id="radius" type="number" step="0.1" value="500" class="w-full border rounded px-3 py-2"/>
          </div>
        </div>
      </div>

      <!-- 计算卡片 -->
      <div class="grid md:grid-cols-2 gap-6">
        <div class="bg-white rounded-lg shadow p-6">
          <h3 class="font-semibold mb-2">1) 支架 → 桥架识别</h3>
          <button class="bg-blue-600 text-white px-4 py-2 rounded" onclick="(async()=>{
            const body={dbnum:+dbnum.value,suppo_refno:+suppo_refno.value,tolerance:+tol.value};
            const data=await postJSON("/api/space/suppo-trays", body); setResult("res_trays", data);
          })()">执行</button>
          <pre id="res_trays" class="bg-gray-50 border rounded p-3 mt-3 text-sm overflow-auto"></pre>
        </div>

        <div class="bg-white rounded-lg shadow p-6">
          <h3 class="font-semibold mb-2">2) 预埋板编号</h3>
          <button class="bg-blue-600 text-white px-4 py-2 rounded" onclick="(async()=>{
            const body={dbnum:+dbnum.value,suppo_refno:+suppo_refno.value,tolerance:+tol.value};
            const data=await postJSON("/api/space/fitting", body); setResult("res_fitting", data);
          })()">执行</button>
          <pre id="res_fitting" class="bg-gray-50 border rounded p-3 mt-3 text-sm overflow-auto"></pre>
        </div>

        <div class="bg-white rounded-lg shadow p-6">
          <h3 class="font-semibold mb-2">3) 距墙/定位块</h3>
          <button class="bg-blue-600 text-white px-4 py-2 rounded" onclick="(async()=>{
            const body={dbnum:+dbnum.value,suppo_refno:+suppo_refno.value,suppo_type:suppo_type.value,search_radius:+radius.value};
            const data=await postJSON("/api/space/wall-distance", body); setResult("res_wall", data);
          })()">执行</button>
          <pre id="res_wall" class="bg-gray-50 border rounded p-3 mt-3 text-sm overflow-auto"></pre>
        </div>

        <div class="bg-white rounded-lg shadow p-6">
          <h3 class="font-semibold mb-2">4) 与预埋板相对定位</h3>
          <button class="bg-blue-600 text-white px-4 py-2 rounded" onclick="(async()=>{
            const body={dbnum:+dbnum.value,suppo_refno:+suppo_refno.value,tolerance:+tol.value};
            const data=await postJSON("/api/space/fitting-offset", body); setResult("res_fitoff", data);
          })()">执行</button>
          <pre id="res_fitoff" class="bg-gray-50 border rounded p-3 mt-3 text-sm overflow-auto"></pre>
        </div>

        <div class="bg-white rounded-lg shadow p-6">
          <h3 class="font-semibold mb-2">5) 与钢结构相对定位</h3>
          <button class="bg-blue-600 text-white px-4 py-2 rounded" onclick="(async()=>{
            const body={dbnum:+dbnum.value,suppo_refno:+suppo_refno.value,suppo_type:suppo_type.value,search_radius:+radius.value};
            const data=await postJSON("/api/space/steel-relative", body); setResult("res_steel", data);
          })()">执行</button>
          <pre id="res_steel" class="bg-gray-50 border rounded p-3 mt-3 text-sm overflow-auto"></pre>
        </div>

        <div class="bg-white rounded-lg shadow p-6">
          <h3 class="font-semibold mb-2">6) 托盘跨度（左右）</h3>
          <div class="grid grid-cols-2 gap-3 mb-2">
            <div>
              <label class="text-sm text-gray-500">邻域窗口</label>
              <input id="win" type="number" step="0.1" value="500" class="w-full border rounded px-3 py-2"/>
            </div>
          </div>
          <button class="bg-blue-600 text-white px-4 py-2 rounded" onclick="(async()=>{
            const body={dbnum:+dbnum.value,suppo_refno:+suppo_refno.value,neighbor_window:+win.value};
            const data=await postJSON("/api/space/tray-span", body); setResult("res_span", data);
          })()">执行</button>
          <pre id="res_span" class="bg-gray-50 border rounded p-3 mt-3 text-sm overflow-auto"></pre>
        </div>
      </div>
    </div>
  </body>
</html>
    "#.to_string()
}

pub fn render_config_page() -> String {
    r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>配置管理 - AIOS 数据库管理平台</title>
    <link href="/static/simple-tailwind.css" rel="stylesheet">
    <link href="/static/simple-icons.css" rel="stylesheet">
    <script src="/static/alpine.min.js" defer></script>
</head>
<body class="bg-gray-50" x-data="configManager()">
    <div class="min-h-screen">
        <!-- 导航栏 -->
        <nav class="bg-blue-600 text-white shadow-lg">
            <div class="max-w-7xl mx-auto px-4">
                <div class="flex justify-between items-center py-4">
                    <div class="flex items-center space-x-4">
                        <i class="fas fa-database text-2xl"></i>
                        <h1 class="text-xl font-bold">AIOS 数据库管理平台</h1>
                    </div>
                    <div class="flex space-x-4">
                        <a href="/" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-home mr-2"></i>首页
                        </a>
                        <a href="/dashboard" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tachometer-alt mr-2"></i>仪表板
                        </a>
                        <a href="/tasks" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tasks mr-2"></i>任务管理
                        </a>
                        <a href="/config" class="bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-cog mr-2"></i>配置管理
                        </a>
                    </div>
                </div>
            </div>
        </nav>

        <!-- 主要内容 -->
        <div class="max-w-7xl mx-auto px-4 py-8">
                <div class="flex justify-between items-center mb-6">
                    <h2 class="text-2xl font-bold text-gray-800">配置管理</h2>
                <div class="flex items-center space-x-4">
                    <div class="flex items-center text-sm">
                        <span :class="surrealStatus.listening ? "bg-green-500" : "bg-gray-400""
                              class="inline-block w-2 h-2 rounded-full mr-2"></span>
                        <span class="text-gray-700" x-text="surrealStatusText()"></span>
                    </div>
                    <div class="flex items-center space-x-2 text-sm text-gray-700">
                        <label class="flex items-center space-x-1">
                            <input type="radio" name="ctrlMode" value="local" x-model="controlMode">
                            <span>本机</span>
                        </label>
                        <label class="flex items-center space-x-1">
                            <input type="radio" name="ctrlMode" value="ssh" x-model="controlMode">
                            <span>远程(SSH)</span>
                        </label>
                    </div>
                    <button @click="startSurreal()"
                            class="bg-blue-600 text-white px-4 py-2 rounded-lg hover:bg-blue-700 transition">
                        <i class="fas fa-play mr-2"></i>启动 SurrealDB
                    </button>
                    <button @click="stopSurreal()"
                            class="bg-red-600 text-white px-4 py-2 rounded-lg hover:bg-red-700 transition">
                        <i class="fas fa-stop mr-2"></i>停止 SurrealDB
                    </button>
                    <button @click="restartSurreal()" x-show="surrealStatus.listening"
                            class="bg-purple-600 text-white px-4 py-2 rounded-lg hover:bg-purple-700 transition">
                        <i class="fas fa-rotate mr-2"></i>重启
                    </button>
                    <button @click="saveConfig()"
                            class="bg-green-600 text-white px-4 py-2 rounded-lg hover:bg-green-700 transition">
                        <i class="fas fa-save mr-2"></i>保存配置
                    </button>
                    <button @click="resetConfig()"
                            class="bg-gray-600 text-white px-4 py-2 rounded-lg hover:bg-gray-700 transition">
                        <i class="fas fa-undo mr-2"></i>重置
                    </button>
                </div>
            </div>

            <div x-show="controlMode==="ssh"" class="bg-yellow-50 border border-yellow-200 rounded-lg p-4 mb-4">
                <h4 class="font-medium text-yellow-800 mb-2">
                    <i class="fas fa-plug mr-2"></i>远程 SSH 参数
                </h4>
                <div class="grid md:grid-cols-4 gap-4 text-sm">
                    <div>
                        <label class="block text-gray-700 mb-1">主机</label>
                        <input x-model="ssh.host" type="text" placeholder="192.168.1.10"
                               class="w-full border border-yellow-300 rounded px-2 py-1 focus:outline-none">
                    </div>
                    <div>
                        <label class="block text-gray-700 mb-1">端口</label>
                        <input x-model.number="ssh.port" type="number" placeholder="22"
                               class="w-full border border-yellow-300 rounded px-2 py-1 focus:outline-none">
                    </div>
                    <div>
                        <label class="block text-gray-700 mb-1">用户</label>
                        <input x-model="ssh.user" type="text" placeholder="root"
                               class="w-full border border-yellow-300 rounded px-2 py-1 focus:outline-none">
                    </div>
                    <div>
                        <label class="block text-gray-700 mb-1">密码（可选）</label>
                        <input x-model="ssh.password" type="password" placeholder="建议改用密钥"
                               class="w-full border border-yellow-300 rounded px-2 py-1 focus:outline-none">
                        <p class="text-xs text-yellow-700 mt-1">如未安装 sshpass，请使用密钥登录</p>
                    </div>
                </div>
            </div>

            <div class="grid lg:grid-cols-3 gap-6">
                <!-- 配置模板 -->
                <div class="bg-white rounded-lg shadow-md p-6">
                    <h3 class="text-lg font-semibold mb-4">
                        <i class="fas fa-templates text-blue-600 mr-2"></i>
                        配置模板
                    </h3>
                    <div class="space-y-3">
                        <template x-for="(template, key) in templates" :key="key">
                            <div class="border rounded-lg p-3 cursor-pointer hover:bg-gray-50"
                                 @click="loadTemplate(key)"
                                 :class="selectedTemplate === key ? "border-blue-500 bg-blue-50" : "border-gray-200"">
                                <div class="font-medium" x-text="template.name"></div>
                                <div class="text-sm text-gray-500">
                                    数据库: <span x-text="template.manual_db_nums.join(", ")"></span>
                                </div>
                                <div class="text-xs text-gray-400 mt-1">
                                    <span x-show="template.gen_model">模型</span>
                                    <span x-show="template.gen_mesh" class="ml-2">网格</span>
                                    <span x-show="template.gen_spatial_tree" class="ml-2">空间树</span>
                                </div>
                            </div>
                        </template>
                    </div>
                </div>

                <!-- 配置表单 -->
                <div class="lg:col-span-2 bg-white rounded-lg shadow-md p-6">
                    <h3 class="text-lg font-semibold mb-4">
                        <i class="fas fa-cog text-green-600 mr-2"></i>
                        配置详情
                    </h3>

                    <form @submit.prevent="saveConfig()">
                        <div class="grid md:grid-cols-2 gap-6">
                            <!-- 基本配置 -->
                            <div class="space-y-4">
                                <h4 class="font-medium text-gray-800 border-b pb-2">基本配置</h4>

                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-2">配置名称</label>
                                    <input x-model="config.name" type="text"
                                           class="w-full border border-gray-300 rounded-md px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500">
                                </div>

                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-2">项目名称</label>
                                    <input x-model="config.project_name" type="text"
                                           class="w-full border border-gray-300 rounded-md px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500">
                                </div>

                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-2">项目代码</label>
                                    <input x-model="config.project_code" type="number"
                                           class="w-full border border-gray-300 rounded-md px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500">
                                </div>

                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-2">数据库编号</label>
                                    <input x-model="dbNumsInput" type="text"
                                           placeholder="例如: 7999,1112,8000"
                                           class="w-full border border-gray-300 rounded-md px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500">
                                    <p class="text-xs text-gray-500 mt-1">多个编号用逗号分隔</p>
                                </div>
                            </div>

                            <!-- 生成选项 -->
                            <div class="space-y-4">
                                <h4 class="font-medium text-gray-800 border-b pb-2">生成选项</h4>

                                <div class="space-y-3">
                                    <label class="flex items-center">
                                        <input x-model="config.gen_model" type="checkbox"
                                               class="rounded border-gray-300 text-blue-600 focus:ring-blue-500">
                                        <span class="ml-2 text-sm text-gray-700">生成模型数据</span>
                                    </label>

                                    <label class="flex items-center">
                                        <input x-model="config.gen_mesh" type="checkbox"
                                               class="rounded border-gray-300 text-blue-600 focus:ring-blue-500">
                                        <span class="ml-2 text-sm text-gray-700">生成网格数据</span>
                                    </label>

                                    <label class="flex items-center">
                                        <input x-model="config.gen_spatial_tree" type="checkbox"
                                               class="rounded border-gray-300 text-blue-600 focus:ring-blue-500">
                                        <span class="ml-2 text-sm text-gray-700">生成空间树</span>
                                    </label>

                                    <label class="flex items-center">
                                        <input x-model="config.apply_boolean_operation" type="checkbox"
                                               class="rounded border-gray-300 text-blue-600 focus:ring-blue-500">
                                        <span class="ml-2 text-sm text-gray-700">应用布尔运算</span>
                                    </label>
                                </div>

                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-2">网格容差比率</label>
                                    <input x-model="config.mesh_tol_ratio" type="number" step="0.1"
                                           class="w-full border border-gray-300 rounded-md px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500">
                                </div>

                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-2">房间关键字</label>
                                    <input x-model="config.room_keyword" type="text"
                                           class="w-full border border-gray-300 rounded-md px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500">
                                    <p class="text-xs text-gray-500 mt-1">用于识别房间的关键字模式</p>
                                </div>
                            </div>
                        </div>

                        <!-- 配置预览 -->
                        <div class="mt-6 p-4 bg-gray-50 rounded-lg">
                            <h4 class="font-medium text-gray-800 mb-2">配置预览</h4>
                            <pre class="text-xs text-gray-600 overflow-x-auto" x-text="JSON.stringify(config, null, 2)"></pre>
                        </div>
                    </form>
                </div>
            </div>
        </div>
    </div>

    <script>
        function configManager() {
            return {
                config: {
                    name: "默认配置",
                    manual_db_nums: [7999],
                    gen_model: true,
                    gen_mesh: true,
                    gen_spatial_tree: true,
                    apply_boolean_operation: true,
                    mesh_tol_ratio: 3.0,
                    room_keyword: "-RM",
                    project_name: "AvevaMarineSample",
                    project_code: 1516
                },
                templates: {},
                selectedTemplate: "",
                dbNumsInput: "7999",
                surrealStatus: { status: "unknown", listening: false, connected: false, address: "" },
                statusTimer: null,
                controlMode: "local",
                ssh: { host: "", port: 22, user: "", password: "" },

                async init() {
                    await this.loadConfig();
                    await this.loadTemplates();
                    this.updateDbNumsInput();
                    await this.refreshSurrealStatus();
                    this.statusTimer = setInterval(() => this.refreshSurrealStatus(), 3000);
                },

                async loadConfig() {
                    try {
                        const response = await fetch("/api/config");
                        if (response.ok) {
                            this.config = await response.json();
                            this.updateDbNumsInput();
                        }
                    } catch (error) {
                        console.error("Failed to load config:", error);
                    }
                },

                async loadTemplates() {
                    try {
                        const response = await fetch("/api/config/templates");
                        if (response.ok) {
                            const data = await response.json();
                            this.templates = data.templates;
                        }
                    } catch (error) {
                        console.error("Failed to load templates:", error);
                    }
                },

                async saveConfig() {
                    try {
                        // 更新数据库编号
                        this.config.manual_db_nums = this.dbNumsInput
                            .split(",")
                            .map(n => parseInt(n.trim()))
                            .filter(n => !isNaN(n));

                        const response = await fetch("/api/config", {
                            method: "POST",
                            headers: { "Content-Type": "application/json" },
                            body: JSON.stringify({ config: this.config })
                        });

                        if (response.ok) {
                            alert("配置保存成功！");
                        } else {
                            alert("配置保存失败");
                        }
                    } catch (error) {
                        console.error("Error saving config:", error);
                        alert("网络错误");
                    }
                },

                async startSurreal() {
                    try {
                        const body = this.controlMode === "ssh" ? { mode: "ssh", ssh: { ...this.ssh } } : { mode: "local" };
                        const res = await fetch("/api/surreal/start", {
                            method: "POST",
                            headers: { "Content-Type": "application/json" },
                            body: JSON.stringify(body)
                        });
                        const data = await res.json();
                        if (data.success) {
                            alert(data.message || "SurrealDB 启动成功");
                        } else {
                            alert(data.message || "SurrealDB 启动失败");
                        }
                        await this.refreshSurrealStatus();
                    } catch (e) {
                        console.error("start surreal error", e);
                        alert("网络错误，无法启动 SurrealDB");
                    }
                },
                async stopSurreal() {
                    try {
                        const body = this.controlMode === "ssh" ? { mode: "ssh", ssh: { ...this.ssh } } : { mode: "local" };
                        const res = await fetch("/api/surreal/stop", {
                            method: "POST",
                            headers: { "Content-Type": "application/json" },
                            body: JSON.stringify(body)
                        });
                        const data = await res.json();
                        if (data.success) {
                            alert(data.message || "SurrealDB 已停止");
                        } else {
                            alert(data.message || "SurrealDB 停止失败");
                        }
                        await this.refreshSurrealStatus();
                    } catch (e) {
                        console.error("stop surreal error", e);
                        alert("网络错误，无法停止 SurrealDB");
                    }
                },

                async restartSurreal() {
                    try {
                        const body = this.controlMode === "ssh" ? { mode: "ssh", ssh: { ...this.ssh } } : { mode: "local" };
                        const res = await fetch("/api/surreal/restart", {
                            method: "POST",
                            headers: { "Content-Type": "application/json" },
                            body: JSON.stringify(body)
                        });
                        const data = await res.json();
                        if (data.success) {
                            alert(data.message || "已重启 SurrealDB");
                        } else {
                            alert(data.message || "重启失败");
                        }
                        await this.refreshSurrealStatus();
                    } catch (e) {
                        console.error("restart surreal error", e);
                        alert("网络错误，无法重启 SurrealDB");
                    }
                },

                resetConfig() {
                    if (confirm("确定要重置配置吗？")) {
                        this.config = {
                            name: "默认配置",
                            manual_db_nums: [7999],
                            gen_model: true,
                            gen_mesh: true,
                            gen_spatial_tree: true,
                            apply_boolean_operation: true,
                            mesh_tol_ratio: 3.0,
                            room_keyword: "-RM",
                            project_name: "AvevaMarineSample",
                            project_code: 1516
                        };
                        this.dbNumsInput = "7999";
                        this.selectedTemplate = "";
                    }
                },
                
                loadTemplate(templateKey) {
                    this.selectedTemplate = templateKey;
                    this.config = { ...this.templates[templateKey] };
                    this.updateDbNumsInput();
                },

                updateDbNumsInput() {
                    this.dbNumsInput = this.config.manual_db_nums.join(", ");
                },

                async refreshSurrealStatus() {
                    try {
                        const res = await fetch("/api/surreal/status");
                        if (res.ok) {
                            const data = await res.json();
                            this.surrealStatus = {
                                status: data.status,
                                listening: !!data.listening,
                                connected: !!data.connected,
                                address: data.address || ""
                            };
                        }
                    } catch (e) {
                        // ignore
                    }
                },

                surrealStatusText() {
                    if (!this.surrealStatus) return "状态未知";
                    const addr = this.surrealStatus.address ? `@ ${this.surrealStatus.address}` : "";
                    if (this.surrealStatus.listening) {
                        return `SurrealDB 运行中 ${addr}`;
                    } else {
                        return `SurrealDB 未运行 ${addr}`;
                    }
                }
            }
        }
    </script>
</body>
</html>
    "#.to_string()
}

pub fn render_tasks_page() -> String {
    r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>任务管理 - AIOS 数据库管理平台</title>
    <link href="/static/simple-tailwind.css" rel="stylesheet">
    <link href="/static/simple-icons.css" rel="stylesheet">
    <script src="/static/alpine.min.js" defer></script>
</head>
<body class="bg-gray-50" x-data="taskManager()">
    <div class="min-h-screen">
        <!-- 导航栏 -->
        <nav class="bg-blue-600 text-white shadow-lg">
            <div class="max-w-7xl mx-auto px-4">
                <div class="flex justify-between items-center py-4">
                    <div class="flex items-center space-x-4">
                        <i class="fas fa-database text-2xl"></i>
                        <h1 class="text-xl font-bold">AIOS 数据库管理平台</h1>
                    </div>
                    <div class="flex space-x-4">
                        <a href="/" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-home mr-2"></i>首页
                        </a>
                        <a href="/dashboard" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tachometer-alt mr-2"></i>仪表板
                        </a>
                        <a href="/tasks" class="bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tasks mr-2"></i>任务管理
                        </a>
                        <a href="/config" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-cog mr-2"></i>配置管理
                        </a>
                    </div>
                </div>
            </div>
        </nav>

        <!-- 主要内容 -->
        <div class="max-w-7xl mx-auto px-4 py-8">
            <!-- 页面标题和操作按钮 -->
            <div class="flex justify-between items-center mb-6">
                <h2 class="text-2xl font-bold text-gray-800">任务管理</h2>
                <div class="flex space-x-4">
                    <button @click="checkSitesBeforeCreate()"
                            class="bg-blue-600 text-white px-4 py-2 rounded-lg hover:bg-blue-700 transition">
                        <i class="fas fa-plus mr-2"></i>创建新任务
                    </button>
                    <button @click="refreshTasks()"
                            class="bg-gray-600 text-white px-4 py-2 rounded-lg hover:bg-gray-700 transition">
                        <i class="fas fa-sync-alt mr-2"></i>刷新
                    </button>
                </div>
            </div>

            <!-- 快速操作卡片 -->
            <div class="grid md:grid-cols-3 gap-4 mb-6">
                <div class="bg-white rounded-lg shadow-md p-4">
                    <h3 class="font-semibold mb-2">
                        <i class="fas fa-database text-blue-600 mr-2"></i>
                        数据库 7999 生成
                    </h3>
                    <p class="text-gray-600 text-sm mb-3">快速生成数据库编号 7999 的完整数据</p>
                    <button @click="createQuickTask(7999, "FullGeneration")"
                            class="bg-blue-600 text-white px-3 py-1 rounded text-sm hover:bg-blue-700 transition">
                        立即执行
                    </button>
                </div>

                <div class="bg-white rounded-lg shadow-md p-4">
                    <h3 class="font-semibold mb-2">
                        <i class="fas fa-sitemap text-green-600 mr-2"></i>
                        空间树生成
                    </h3>
                    <p class="text-gray-600 text-sm mb-3">仅生成空间树和房间关系</p>
                    <button @click="createQuickTask(7999, "SpatialTreeGeneration")"
                            class="bg-green-600 text-white px-3 py-1 rounded text-sm hover:bg-green-700 transition">
                        立即执行
                    </button>
                </div>

                <div class="bg-white rounded-lg shadow-md p-4">
                    <h3 class="font-semibold mb-2">
                        <i class="fas fa-cube text-purple-600 mr-2"></i>
                        网格生成
                    </h3>
                    <p class="text-gray-600 text-sm mb-3">仅生成网格数据和布尔运算</p>
                    <button @click="createQuickTask(7999, "MeshGeneration")"
                            class="bg-purple-600 text-white px-3 py-1 rounded text-sm hover:bg-purple-700 transition">
                        立即执行
                    </button>
                </div>
            </div>

            <!-- 任务列表 -->
            <div class="bg-white rounded-lg shadow-md">
                <div class="p-6 border-b">
                    <div class="flex justify-between items-center">
                        <h3 class="text-lg font-semibold">任务列表</h3>
                        <div class="flex space-x-2">
                            <select x-model="statusFilter" @change="filterTasks()"
                                    class="border rounded px-3 py-1 text-sm">
                                <option value="">所有状态</option>
                                <option value="pending">等待中</option>
                                <option value="running">运行中</option>
                                <option value="completed">已完成</option>
                                <option value="failed">失败</option>
                                <option value="cancelled">已取消</option>
                            </select>
                        </div>
                    </div>
                </div>

                <div class="overflow-x-auto">
                    <table class="min-w-full table-auto">
                        <thead class="bg-gray-50">
                            <tr>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    任务名称
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    类型
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    状态
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    进度
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    创建时间
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    操作
                                </th>
                            </tr>
                        </thead>
                        <tbody class="bg-white divide-y divide-gray-200">
                            <template x-for="task in filteredTasks" :key="task.id">
                                <tr>
                                    <td class="px-6 py-4 whitespace-nowrap">
                                        <div class="text-sm font-medium text-gray-900" x-text="task.name"></div>
                                        <div class="text-sm text-gray-500" x-text=""ID: " + task.id.substring(0, 8)"></div>
                                    </td>
                                    <td class="px-6 py-4 whitespace-nowrap">
                                        <span class="text-sm text-gray-900" x-text="getTaskTypeText(task.task_type)"></span>
                                    </td>
                                    <td class="px-6 py-4 whitespace-nowrap">
                                        <span class="px-2 inline-flex text-xs leading-5 font-semibold rounded-full"
                                              :class="getStatusClass(task.status)"
                                              x-text="getStatusText(task.status)"></span>
                                    </td>
                                    <td class="px-6 py-4 whitespace-nowrap">
                                        <div class="w-full bg-gray-200 rounded-full h-2 mb-1">
                                            <div class="bg-blue-600 h-2 rounded-full transition-all duration-300"
                                                 :style="`width: ${task.progress.percentage}%`"></div>
                                        </div>
                                        <div class="text-xs space-y-1">
                                            <div class="flex justify-between">
                                                <span class="text-gray-600" x-text="`${Math.round(task.progress.percentage)}%`"></span>
                                                <span class="text-gray-500" x-text="`步骤 ${task.progress.current_step_number}/${task.progress.total_steps}`"></span>
                                            </div>
                                            <div class="text-gray-700 font-medium" x-text="task.progress.current_step"></div>
                                        </div>
                                    </td>
                                    <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
                                        <span x-text="formatTime(task.created_at)"></span>
                                    </td>
                                    <td class="px-6 py-4 whitespace-nowrap text-sm font-medium">
                                        <div class="flex space-x-2">
                                            <button x-show="task.status === "Pending""
                                                    @click="startTask(task.id)"
                                                    class="text-green-600 hover:text-green-900">
                                                <i class="fas fa-play"></i>
                                            </button>
                                            <button x-show="task.status === "Running""
                                                    @click="stopTask(task.id)"
                                                    class="text-red-600 hover:text-red-900">
                                                <i class="fas fa-stop"></i>
                                            </button>
                                            <button @click="viewTaskDetails(task)"
                                                    class="text-blue-600 hover:text-blue-900"
                                                    title="查看详情">
                                                <i class="fas fa-eye"></i>
                                            </button>
                                            <button @click="editTask(task.id)"
                                                    class="text-green-600 hover:text-green-900"
                                                    title="编辑任务">
                                                <i class="fas fa-edit"></i>
                                            </button>
                                            <button @click="viewTaskLogs(task.id)"
                                                    class="text-purple-600 hover:text-purple-900"
                                                    title="查看日志">
                                                <i class="fas fa-file-alt"></i>
                                            </button>
                                            <button x-show="task.status === "Failed""
                                                    @click="viewErrorDetails(task.id)"
                                                    class="text-orange-600 hover:text-orange-900"
                                                    title="查看错误详情">
                                                <i class="fas fa-exclamation-triangle"></i>
                                            </button>
                                            <button x-show="['Completed', 'Failed', 'Cancelled', 'Pending'].includes(task.status)"
                                                    @click="deleteTask(task.id)"
                                                    class="text-red-600 hover:text-red-900"
                                                    title="删除任务">
                                                <i class="fas fa-trash"></i>
                                            </button>
                                        </div>
                                    </td>
                                </tr>
                            </template>
                        </tbody>
                    </table>
                </div>
            </div>
        </div>
    </div>

    <!-- 创建任务模态框 -->
    <div x-show="showCreateModal"
         x-transition:enter="transition ease-out duration-300"
         x-transition:enter-start="opacity-0"
         x-transition:enter-end="opacity-100"
         x-transition:leave="transition ease-in duration-200"
         x-transition:leave-start="opacity-100"
         x-transition:leave-end="opacity-0"
         class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-1000">
        <div class="relative top-20 mx-auto p-5 border w-96 shadow-lg rounded-md bg-white z-1010">
            <div class="mt-3">
                <h3 class="text-lg font-medium text-gray-900 mb-4">创建新任务</h3>
                <form @submit.prevent="createTask()">
                    <div class="mb-4">
                        <label class="block text-sm font-medium text-gray-700 mb-2">任务名称</label>
                        <input x-model="newTask.name" type="text" required
                               class="w-full border border-gray-300 rounded-md px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500">
                    </div>

                    <div class="mb-4">
                        <label class="block text-sm font-medium text-gray-700 mb-2">任务类型</label>
                        <select x-model="newTask.task_type" required
                                class="w-full border border-gray-300 rounded-md px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500">
                            <option value="DataGeneration">数据生成</option>
                            <option value="SpatialTreeGeneration">空间树生成</option>
                            <option value="FullGeneration">完整生成</option>
                            <option value="MeshGeneration">网格生成</option>
                        </select>
                    </div>

                    <div class="mb-4">
                        <label class="block text-sm font-medium text-gray-700 mb-2">数据库编号</label>
                        <input x-model="newTask.config.manual_db_nums" type="text"
                               placeholder="例如: 7999,1112,8000"
                               class="w-full border border-gray-300 rounded-md px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500">
                        <p class="text-xs text-gray-500 mt-1">多个编号用逗号分隔</p>
                    </div>

                    <div class="flex justify-end space-x-3">
                        <button type="button" @click="showCreateModal = false"
                                class="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-200 rounded-md hover:bg-gray-300">
                            取消
                        </button>
                        <button type="submit"
                                class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700">
                            创建任务
                        </button>
                    </div>
                </form>
            </div>
        </div>
    </div>

    <!-- 任务详情模态框 -->
    <div x-show="showDetailsModal"
         x-transition:enter="transition ease-out duration-300"
         x-transition:enter-start="opacity-0"
         x-transition:enter-end="opacity-100"
         x-transition:leave="transition ease-in duration-200"
         x-transition:leave-start="opacity-100"
         x-transition:leave-end="opacity-0"
         class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-1000">
        <div class="relative top-10 mx-auto p-5 border w-11/12 max-w-5xl shadow-lg rounded-md bg-white z-1010">
            <div class="mt-3">
                <div class="flex justify-between items-center mb-4">
                    <h3 class="text-lg font-medium text-gray-900">
                        <i class="fas fa-eye mr-2 text-blue-600"></i>
                        任务详情
                    </h3>
                    <button @click="closeDetails()" class="text-gray-400 hover:text-gray-600">
                        <i class="fas fa-times text-xl"></i>
                    </button>
                </div>

                <div x-show="taskDetails" class="space-y-6">
                    <!-- 基本信息 -->
                    <div class="bg-gray-50 border border-gray-200 rounded-lg p-4">
                        <h4 class="font-semibold text-gray-800 mb-2">基本信息</h4>
                        <div class="grid md:grid-cols-2 lg:grid-cols-3 gap-3 text-sm">
                            <div><span class="text-gray-600">任务名称：</span><span class="font-medium" x-text="taskDetails?.name"></span></div>
                            <div><span class="text-gray-600">任务ID：</span><span class="font-mono" x-text="taskDetails?.id"></span></div>
                            <div>
                                <span class="text-gray-600">任务类型：</span>
                                <span class="font-medium" x-text="getTaskTypeText(taskDetails?.task_type)"></span>
                            </div>
                            <div>
                                <span class="text-gray-600">状态：</span>
                                <span class="px-2 py-0.5 rounded text-xs font-medium" :class="getStatusClass(taskDetails?.status)" x-text="getStatusText(taskDetails?.status)"></span>
                            </div>
                            <div><span class="text-gray-600">优先级：</span><span class="font-medium" x-text="taskDetails?.priority"></span></div>
                            <div><span class="text-gray-600">依赖任务数：</span><span class="font-medium" x-text="(taskDetails?.dependencies||[]).length"></span></div>
                        </div>
                    </div>

                    <!-- 时间与时长 -->
                    <div class="bg-white border border-gray-200 rounded-lg p-4">
                        <h4 class="font-semibold text-gray-800 mb-2">时间信息</h4>
                        <div class="grid md:grid-cols-2 lg:grid-cols-3 gap-3 text-sm">
                            <div><span class="text-gray-600">创建时间：</span><span x-text="formatTime(taskDetails?.created_at)"></span></div>
                            <div><span class="text-gray-600">开始时间：</span><span x-text="formatTime(taskDetails?.started_at)"></span></div>
                            <div><span class="text-gray-600">完成时间：</span><span x-text="formatTime(taskDetails?.completed_at)"></span></div>
                            <div><span class="text-gray-600">预估时长：</span><span x-text="formatDurationSec(taskDetails?.estimated_duration)"></span></div>
                            <div><span class="text-gray-600">实际时长：</span><span x-text="formatDurationMs(taskDetails?.actual_duration)"></span></div>
                        </div>
                    </div>

                    <!-- 进度 -->
                    <div class="bg-white border border-gray-200 rounded-lg p-4">
                        <h4 class="font-semibold text-gray-800 mb-3">进度</h4>
                        <div class="mb-2 flex items-center justify-between text-sm">
                            <div>
                                <span class="text-gray-600">当前步骤：</span>
                                <span class="font-medium" x-text="taskDetails?.progress?.current_step"></span>
                                <span class="text-gray-500 ml-2" x-text="`${taskDetails?.progress?.current_step_number||0}/${taskDetails?.progress?.total_steps||0}`"></span>
                            </div>
                            <div class="text-gray-600">
                                <span>预计剩余：</span>
                                <span x-text="formatDurationSec(taskDetails?.progress?.estimated_remaining_seconds)"></span>
                            </div>
                        </div>
                        <div class="w-full bg-gray-200 rounded-full h-3" :title="`${taskDetails?.progress?.percentage||0}% (${taskDetails?.progress?.processed_items||0}/${taskDetails?.progress?.total_items||0})`">
                            <div class="bg-blue-600 h-3 rounded-full" :style="`width: ${taskDetails?.progress?.percentage||0}%`"></div>
                        </div>
                        <div class="mt-2 text-xs text-gray-500">
                            <span>已处理 <span x-text="taskDetails?.progress?.processed_items||0"></span> / 总计 <span x-text="taskDetails?.progress?.total_items||0"></span></span>
                        </div>
                    </div>

                    <!-- 配置信息（关键字段） -->
                    <div class="bg-gray-50 border border-gray-200 rounded-lg p-4">
                        <h4 class="font-semibold text-gray-800 mb-2">配置信息</h4>
                        <div class="grid md:grid-cols-2 lg:grid-cols-3 gap-3 text-sm">
                            <div><span class="text-gray-600">项目：</span><span class="font-medium" x-text="taskDetails?.config?.project_name"></span></div>
                            <div><span class="text-gray-600">项目代码：</span><span class="font-medium" x-text="taskDetails?.config?.project_code"></span></div>
                            <div><span class="text-gray-600">MDB：</span><span class="font-medium" x-text="taskDetails?.config?.mdb_name"></span></div>
                            <div><span class="text-gray-600">模块：</span><span class="font-medium" x-text="taskDetails?.config?.module"></span></div>
                            <div><span class="text-gray-600">数据库类型：</span><span class="font-medium" x-text="taskDetails?.config?.db_type"></span></div>
                            <div><span class="text-gray-600">SurrealNS：</span><span class="font-medium" x-text="taskDetails?.config?.surreal_ns"></span></div>
                            <div><span class="text-gray-600">DB 地址：</span><span class="font-medium" x-text="`${taskDetails?.config?.db_ip||''}:${taskDetails?.config?.db_port||''}`"></span></div>
                            <div><span class="text-gray-600">DB 用户：</span><span class="font-medium" x-text="taskDetails?.config?.db_user"></span></div>
                            <div><span class="text-gray-600">数据库号：</span><span class="font-medium" x-text="(taskDetails?.config?.manual_db_nums||[]).join(', ')"></span></div>
                            <div><span class="text-gray-600">生成模型：</span><span class="font-medium" x-text="taskDetails?.config?.gen_model ? '是' : '否'"></span></div>
                            <div><span class="text-gray-600">生成网格：</span><span class="font-medium" x-text="taskDetails?.config?.gen_mesh ? '是' : '否'"></span></div>
                            <div><span class="text-gray-600">生成空间树：</span><span class="font-medium" x-text="taskDetails?.config?.gen_spatial_tree ? '是' : '否'"></span></div>
                            <div><span class="text-gray-600">布尔运算：</span><span class="font-medium" x-text="taskDetails?.config?.apply_boolean_operation ? '是' : '否'"></span></div>
                            <div><span class="text-gray-600">网格容差：</span><span class="font-medium" x-text="taskDetails?.config?.mesh_tol_ratio"></span></div>
                            <div><span class="text-gray-600">房间关键字：</span><span class="font-medium" x-text="taskDetails?.config?.room_keyword"></span></div>
                            <div x-show="taskDetails?.config?.target_sesno"><span class="text-gray-600">目标会话号：</span><span class="font-medium" x-text="taskDetails?.config?.target_sesno"></span></div>
                        </div>

                        <!-- 原始配置JSON -->
                        <details class="mt-3">
                            <summary class="cursor-pointer text-sm text-blue-600">展开原始配置 JSON</summary>
                            <pre class="text-xs text-gray-700 bg-white border rounded mt-2 p-2 overflow-x-auto" x-text="JSON.stringify(taskDetails?.config, null, 2)"></pre>
                        </details>
                    </div>

                    <!-- 最近日志预览 -->
                    <div class="bg-white border border-gray-200 rounded-lg p-4">
                        <div class="flex justify-between items-center mb-2">
                            <h4 class="font-semibold text-gray-800">最近日志</h4>
                            <div class="text-sm text-gray-500">显示 <span x-text="detailsLogsLimit"></span> 条</div>
                        </div>
                        <div class="max-h-56 overflow-y-auto space-y-2">
                            <template x-for="log in detailsLogs" :key="log.timestamp">
                                <div class="flex items-start space-x-2 text-sm">
                                    <span class="text-gray-500 text-xs whitespace-nowrap" x-text="formatTime(log.timestamp)"></span>
                                    <span class="px-2 py-0.5 rounded text-xs font-medium" :class="getLogLevelClass(log.level)"><span x-text="log.level"></span></span>
                                    <span class="flex-1 break-words" x-text="log.message"></span>
                                </div>
                            </template>
                            <div x-show="!detailsLogs || detailsLogs.length === 0" class="text-center text-gray-500 py-4 text-sm">暂无日志</div>
                        </div>
                        <div class="flex justify-end mt-3 space-x-2">
                            <button @click="viewTaskLogs(taskDetails.id)" class="px-3 py-1.5 text-sm text-white bg-purple-600 rounded hover:bg-purple-700"><i class="fas fa-file-alt mr-1"></i> 查看全部日志</button>
                        </div>
                    </div>

                    <!-- 依赖与错误 -->
                    <div class="grid md:grid-cols-2 gap-4">
                        <div class="bg-white border border-gray-200 rounded-lg p-4">
                            <h4 class="font-semibold text-gray-800 mb-2">依赖任务</h4>
                            <template x-if="(taskDetails?.dependencies||[]).length > 0">
                                <ul class="list-disc list-inside text-sm text-gray-700">
                                    <template x-for="dep in taskDetails.dependencies" :key="dep">
                                        <li class="font-mono" x-text="dep"></li>
                                    </template>
                                </ul>
                            </template>
                            <p x-show="!(taskDetails?.dependencies||[]).length" class="text-sm text-gray-500">无</p>
                        </div>
                        <div class="bg-white border border-gray-200 rounded-lg p-4">
                            <h4 class="font-semibold text-gray-800 mb-2">错误信息</h4>
                            <div x-show="taskDetails?.error" class="text-sm text-red-700 bg-red-50 border border-red-200 rounded p-2" x-text="taskDetails.error"></div>
                            <p x-show="!taskDetails?.error" class="text-sm text-gray-500">无</p>
                        </div>
                    </div>

                    <div class="flex justify-end mt-4 space-x-2">
                        <button @click="copyTaskJson()" class="px-4 py-2 text-sm font-medium text-blue-700 bg-blue-100 rounded-md hover:bg-blue-200"><i class="fas fa-copy mr-2"></i>复制任务JSON</button>
                        <button @click="closeDetails()" class="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-200 rounded-md hover:bg-gray-300">关闭</button>
                        <button @click="viewTaskLogs(taskDetails.id)" class="px-4 py-2 text-sm font-medium text-white bg-purple-600 rounded-md hover:bg-purple-700"><i class="fas fa-file-alt mr-2"></i>查看日志</button>
                        <button x-show="taskDetails?.status === 'Failed'" @click="viewErrorDetails(taskDetails.id)" class="px-4 py-2 text-sm font-medium text-white bg-orange-600 rounded-md hover:bg-orange-700"><i class="fas fa-exclamation-triangle mr-2"></i>错误详情</button>
                    </div>
                </div>
            </div>
        </div>
    </div>

    <!-- 错误详情模态框 -->
    <div x-show="showErrorModal"
         x-transition:enter="transition ease-out duration-300"
         x-transition:enter-start="opacity-0"
         x-transition:enter-end="opacity-100"
         x-transition:leave="transition ease-in duration-200"
         x-transition:leave-start="opacity-100"
         x-transition:leave-end="opacity-0"
         class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-1000">
        <div class="relative top-10 mx-auto p-5 border w-4/5 max-w-4xl shadow-lg rounded-md bg-white z-1010">
            <div class="mt-3">
                <div class="flex justify-between items-center mb-4">
                    <h3 class="text-lg font-medium text-red-600">
                        <i class="fas fa-exclamation-triangle mr-2"></i>
                        任务执行错误详情
                    </h3>
                    <button @click="showErrorModal = false" class="text-gray-400 hover:text-gray-600">
                        <i class="fas fa-times text-xl"></i>
                    </button>
                </div>

                <div x-show="errorDetails" class="space-y-6">
                    <!-- 基本错误信息 -->
                    <div class="bg-red-50 border border-red-200 rounded-lg p-4">
                        <h4 class="font-semibold text-red-800 mb-2">错误概要</h4>
                        <div class="grid md:grid-cols-2 gap-4 text-sm">
                            <div>
                                <span class="font-medium text-gray-700">任务名称:</span>
                                <span x-text="errorDetails?.task_name" class="ml-2"></span>
                            </div>
                            <div>
                                <span class="font-medium text-gray-700">错误代码:</span>
                                <span x-text="errorDetails?.error_details?.error_code" class="ml-2 font-mono bg-red-100 px-2 py-1 rounded"></span>
                            </div>
                            <div class="md:col-span-2">
                                <span class="font-medium text-gray-700">失败步骤:</span>
                                <span x-text="errorDetails?.error_details?.failed_step" class="ml-2"></span>
                            </div>
                            <div class="md:col-span-2">
                                <span class="font-medium text-gray-700">错误消息:</span>
                                <div x-text="errorDetails?.error_details?.detailed_message" class="ml-2 mt-1 p-2 bg-white border rounded text-red-700"></div>
                            </div>
                        </div>
                    </div>

                    <!-- 解决方案 -->
                    <div class="bg-blue-50 border border-blue-200 rounded-lg p-4">
                        <h4 class="font-semibold text-blue-800 mb-2">
                            <i class="fas fa-lightbulb mr-2"></i>
                            建议解决方案
                        </h4>
                        <ul class="space-y-2">
                            <template x-for="solution in errorDetails?.error_details?.suggested_solutions" :key="solution">
                                <li class="flex items-start">
                                    <i class="fas fa-arrow-right text-blue-600 mt-1 mr-2 text-xs"></i>
                                    <span x-text="solution" class="text-sm text-blue-700"></span>
                                </li>
                            </template>
                        </ul>
                    </div>

                    <!-- 相关配置 -->
                    <div x-show="errorDetails?.error_details?.related_config" class="bg-gray-50 border border-gray-200 rounded-lg p-4">
                        <h4 class="font-semibold text-gray-800 mb-2">
                            <i class="fas fa-cog mr-2"></i>
                            相关配置信息
                        </h4>
                        <pre class="text-xs text-gray-600 overflow-x-auto bg-white p-2 rounded border"
                             x-text="JSON.stringify(errorDetails?.error_details?.related_config, null, 2)"></pre>
                    </div>

                    <!-- 错误日志 -->
                    <div class="bg-gray-50 border border-gray-200 rounded-lg p-4">
                        <h4 class="font-semibold text-gray-800 mb-2">
                            <i class="fas fa-file-alt mr-2"></i>
                            错误日志
                        </h4>
                        <div class="max-h-64 overflow-y-auto space-y-2">
                            <template x-for="log in errorDetails?.error_logs" :key="log.timestamp">
                                <div class="flex items-start space-x-2 text-sm">
                                    <span class="text-gray-500 text-xs whitespace-nowrap" x-text="formatTime(log.timestamp)"></span>
                                    <span class="px-2 py-1 rounded text-xs font-medium"
                                          :class="log.level === "Critical" ? "bg-red-100 text-red-800" : "bg-orange-100 text-orange-800"">
                                        <span x-text="log.level"></span>
                                    </span>
                                    <span x-text="log.message" class="flex-1"></span>
                                </div>
                            </template>
                        </div>
                    </div>

                    <!-- 堆栈跟踪 -->
                    <div x-show="errorDetails?.error_details?.stack_trace" class="bg-gray-50 border border-gray-200 rounded-lg p-4">
                        <h4 class="font-semibold text-gray-800 mb-2">
                            <i class="fas fa-code mr-2"></i>
                            堆栈跟踪
                        </h4>
                        <pre class="text-xs text-gray-600 overflow-x-auto bg-white p-2 rounded border max-h-32"
                             x-text="errorDetails?.error_details?.stack_trace"></pre>
                    </div>
                </div>

                <div class="flex justify-end mt-6">
                    <button @click="showErrorModal = false"
                            class="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-200 rounded-md hover:bg-gray-300">
                        关闭
                    </button>
                </div>
            </div>
        </div>
    </div>

    <!-- 任务日志查看模态框 -->
    <div x-show="showLogsModal"
         x-transition:enter="transition ease-out duration-300"
         x-transition:enter-start="opacity-0"
         x-transition:enter-end="opacity-100"
         x-transition:leave="transition ease-in duration-200"
         x-transition:leave-start="opacity-100"
         x-transition:leave-end="opacity-0"
         class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-1000">
        <div class="relative top-10 mx-auto p-5 border w-4/5 max-w-5xl shadow-lg rounded-md bg-white z-1010">
            <div class="mt-3">
                <div class="flex justify-between items-center mb-4">
                    <h3 class="text-lg font-medium text-purple-600">
                        <i class="fas fa-file-alt mr-2"></i>
                        任务日志查看
                        <span x-show="currentTaskLogs" x-text=""- " + currentTaskLogs?.task_name" class="text-gray-600"></span>
                    </h3>
                    <button @click="showLogsModal = false" class="text-gray-400 hover:text-gray-600">
                        <i class="fas fa-times text-xl"></i>
                    </button>
                </div>

                <!-- 日志过滤器 -->
                <div class="mb-4 flex flex-wrap gap-4 items-center">
                    <div class="flex items-center space-x-2">
                        <label class="text-sm font-medium text-gray-700">日志级别:</label>
                        <select x-model="logLevelFilter" @change="filterLogs()"
                                class="border border-gray-300 rounded px-2 py-1 text-sm">
                            <option value="">全部</option>
                            <option value="Debug">Debug</option>
                            <option value="Info">Info</option>
                            <option value="Warning">Warning</option>
                            <option value="Error">Error</option>
                            <option value="Critical">Critical</option>
                        </select>
                    </div>
                    <div class="flex items-center space-x-2">
                        <label class="text-sm font-medium text-gray-700">搜索:</label>
                        <input x-model="logSearchQuery" @input="filterLogs()"
                               type="text" placeholder="搜索日志内容..."
                               class="border border-gray-300 rounded px-2 py-1 text-sm w-48">
                    </div>
                    <button @click="refreshTaskLogs()"
                            class="px-3 py-1 text-sm bg-purple-600 text-white rounded hover:bg-purple-700">
                        <i class="fas fa-sync-alt mr-1"></i>刷新
                    </button>
                    <button @click="viewFullTaskLogs()"
                            class="px-3 py-1 text-sm bg-blue-600 text-white rounded hover:bg-blue-700">
                        <i class="fas fa-external-link-alt mr-1"></i>详细查看
                    </button>
                </div>

                <!-- 日志内容 -->
                <div x-show="currentTaskLogs" class="bg-gray-50 border border-gray-200 rounded-lg p-4">
                    <div class="flex justify-between items-center mb-2">
                        <h4 class="font-semibold text-gray-800">
                            日志记录
                            <span x-text=""(共 " + (currentTaskLogs?.total_count || 0) + " 条)"" class="text-sm text-gray-600"></span>
                        </h4>
                        <div class="text-sm text-gray-600">
                            任务状态: <span x-text="getStatusText(currentTaskLogs?.task_status)"
                                      :class="getStatusClass(currentTaskLogs?.task_status)"
                                      class="px-2 py-1 rounded text-xs font-medium"></span>
                        </div>
                    </div>
                    <div class="max-h-96 overflow-y-auto space-y-2 bg-white border rounded p-2">
                        <template x-for="log in filteredTaskLogs" :key="log.timestamp">
                            <div class="flex items-start space-x-2 text-sm py-1 border-b border-gray-100 last:border-b-0">
                                <span class="text-gray-500 text-xs whitespace-nowrap font-mono"
                                      x-text="formatTime(log.timestamp)"></span>
                                <span class="px-2 py-1 rounded text-xs font-medium whitespace-nowrap"
                                      :class="getLogLevelClass(log.level)">
                                    <span x-text="log.level"></span>
                                </span>
                                <span x-text="log.message" class="flex-1 break-words"></span>
                            </div>
                        </template>
                        <div x-show="!filteredTaskLogs || filteredTaskLogs.length === 0"
                             class="text-center text-gray-500 py-4">
                            暂无日志记录
                        </div>
                    </div>
                </div>

                <div class="flex justify-end mt-6 space-x-2">
                    <button @click="showLogsModal = false"
                            class="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-200 rounded-md hover:bg-gray-300">
                        关闭
                    </button>
                </div>
            </div>
        </div>
    </div>

    <script>
        function taskManager() {
            return {
                tasks: [],
                filteredTasks: [],
                statusFilter: "",
                showCreateModal: false,
                showDetailsModal: false,
                showErrorModal: false,
                taskDetails: null,
                detailsTimer: null,
                detailsLogs: [],
                detailsLogsLimit: 10,
                errorDetails: null,
                showLogsModal: false,
                currentTaskLogs: null,
                filteredTaskLogs: [],
                logLevelFilter: "",
                logSearchQuery: "",
                newTask: {
                    name: "",
                    task_type: "FullGeneration",
                    config: {
                        name: "自定义配置",
                        manual_db_nums: "7999",
                        gen_model: true,
                        gen_mesh: true,
                        gen_spatial_tree: true,
                        apply_boolean_operation: true,
                        mesh_tol_ratio: 3.0,
                        room_keyword: "-RM",
                        project_name: "AvevaMarineSample",
                        project_code: 1516
                    }
                },

                getTaskTypeText(taskType) {
                    const map = {
                        "database_generation": "数据解析",
                        "DataGeneration": "数据生成",
                        "SpatialTreeGeneration": "空间计算",
                        "FullGeneration": "完整生成",
                        "MeshGeneration": "网格生成",
                        "ParsePdmsData": "数据解析",
                        "GenerateModel": "模型生成",
                        "GenerateSpatialIndex": "空间计算",
                        "GenerateGeometry": "几何生成",
                        "BuildSpatialIndex": "空间索引",
                        "BatchDatabaseProcess": "批量处理",
                        "BatchGeometryGeneration": "批量生成",
                        "DataExport": "数据导出",
                        "DataImport": "数据导入",
                        "DataParsingWizard": "数据解析"
                    };
                    if (typeof taskType === 'string') return map[taskType] || taskType;
                    // 兼容 serde 外部标记：例如 { "Custom": "Xxx" }
                    if (taskType && typeof taskType === 'object') {
                        const keys = Object.keys(taskType);
                        if (keys.length === 1) {
                            const k = keys[0];
                            if (k === 'Custom') return `自定义(${taskType[k]})`;
                            return map[k] || k;
                        }
                    }
                    return '未知';
                },

                formatDurationSec(sec) {
                    if (sec == null) return "-";
                    const s = Number(sec);
                    if (!isFinite(s)) return "-";
                    const h = Math.floor(s / 3600);
                    const m = Math.floor((s % 3600) / 60);
                    const ss = Math.floor(s % 60);
                    if (h > 0) return `${h}小时${m}分${ss}秒`;
                    if (m > 0) return `${m}分${ss}秒`;
                    return `${ss}秒`;
                },

                formatDurationMs(ms) {
                    if (ms == null) return "-";
                    const s = Number(ms) / 1000.0;
                    return this.formatDurationSec(s);
                },

                async init() {
                    await this.loadTasks();
                    this.filterTasks();

                    // 定期刷新任务状态
                    setInterval(() => {
                        this.loadTasks();
                    }, 3000);
                },

                async checkSitesBeforeCreate() {
                    try {
                        // 检查是否有部署站点
                        const response = await fetch('/api/deployment-sites');
                        const data = await response.json();
                        const sites = Array.isArray(data) ? data : (data.items || []);

                        if (!sites || sites.length === 0) {
                            // 没有部署站点，提示用户
                            if (confirm('暂无可用的部署站点！\n\n创建任务需要先配置部署站点。\n是否现在创建一个新的部署站点？')) {
                                // 跳转到创建站点向导
                                window.location.href = '/wizard';
                            }
                            return;
                        }

                        // 有站点，显示创建任务模态框
                        this.showCreateModal = true;
                    } catch (error) {
                        console.error('检查部署站点失败:', error);
                        alert('检查部署站点失败，请稍后重试。');
                    }
                },

                async loadTasks() {
                    try {
                        const response = await fetch("/api/tasks");
                        const data = await response.json();
                        this.tasks = data.tasks;
                        this.filterTasks();
                    } catch (error) {
                        console.error("Failed to load tasks:", error);
                    }
                },

                filterTasks() {
                    if (!this.statusFilter) {
                        this.filteredTasks = this.tasks;
                    } else {
                        this.filteredTasks = this.tasks.filter(task =>
                            task.status.toLowerCase() === this.statusFilter
                        );
                    }
                },

                async createTask() {
                    try {
                        // 处理数据库编号
                        const dbNums = this.newTask.config.manual_db_nums
                            .split(",")
                            .map(n => parseInt(n.trim()))
                            .filter(n => !isNaN(n));

                        const taskData = {
                            ...this.newTask,
                            config: {
                                ...this.newTask.config,
                                manual_db_nums: dbNums
                            }
                        };

                        const response = await fetch("/api/tasks", {
                            method: "POST",
                            headers: { "Content-Type": "application/json" },
                            body: JSON.stringify(taskData)
                        });

                        if (response.ok) {
                            this.showCreateModal = false;
                            this.resetNewTask();
                            await this.loadTasks();
                        } else {
                            // 获取详细错误信息
                            let errorMessage = "创建任务失败";
                            try {
                                const errorData = await response.json();
                                if (errorData.error) {
                                    errorMessage = errorData.error;
                                } else if (errorData.message) {
                                    errorMessage = errorData.message;
                                }
                            } catch (e) {
                                // 如果无法解析JSON，尝试获取文本
                                try {
                                    const errorText = await response.text();
                                    if (errorText) {
                                        errorMessage = `创建任务失败: ${errorText}`;
                                    }
                                } catch (e2) {
                                    errorMessage = `创建任务失败 (HTTP ${response.status})`;
                                }
                            }
                            alert(errorMessage);
                        }
                    } catch (error) {
                        console.error("Error creating task:", error);
                        alert(`网络错误: ${error.message}`);
                    }
                },

                async createQuickTask(dbNum, taskType) {
                    const taskTypeNames = {
                        "FullGeneration": "完整生成",
                        "DataGeneration": "数据生成",
                        "SpatialTreeGeneration": "空间树生成",
                        "MeshGeneration": "网格生成"
                    };

                    try {
                        const response = await fetch("/api/tasks", {
                            method: "POST",
                            headers: { "Content-Type": "application/json" },
                            body: JSON.stringify({
                                name: `数据库 ${dbNum} ${taskTypeNames[taskType]}`,
                                task_type: taskType,
                                config: {
                                    name: `数据库 ${dbNum} 配置`,
                                    manual_db_nums: [dbNum],
                                    gen_model: true,
                                    gen_mesh: true,
                                    gen_spatial_tree: true,
                                    apply_boolean_operation: true,
                                    mesh_tol_ratio: 3.0,
                                    room_keyword: "-RM",
                                    project_name: "AvevaMarineSample",
                                    project_code: 1516
                                }
                            })
                        });

                        if (response.ok) {
                            const task = await response.json();
                            // 自动启动任务
                            await this.startTask(task.id);
                            await this.loadTasks();
                        } else {
                            // 获取详细错误信息
                            let errorMessage = "创建任务失败";
                            try {
                                const errorData = await response.json();
                                if (errorData.error) {
                                    errorMessage = errorData.error;
                                } else if (errorData.message) {
                                    errorMessage = errorData.message;
                                }
                            } catch (e) {
                                try {
                                    const errorText = await response.text();
                                    if (errorText) {
                                        errorMessage = `创建任务失败: ${errorText}`;
                                    }
                                } catch (e2) {
                                    errorMessage = `创建任务失败 (HTTP ${response.status})`;
                                }
                            }
                            alert(errorMessage);
                        }
                    } catch (error) {
                        console.error("Error:", error);
                        alert("网络错误");
                    }
                },

                async startTask(taskId) {
                    try {
                        const response = await fetch(`/api/tasks/${taskId}/start`, {
                            method: "POST"
                        });

                        if (response.ok) {
                            await this.loadTasks();
                        } else {
                            alert("启动任务失败");
                        }
                    } catch (error) {
                        console.error("Error starting task:", error);
                    }
                },

                async stopTask(taskId) {
                    try {
                        const response = await fetch(`/api/tasks/${taskId}/stop`, {
                            method: "POST"
                        });

                        if (response.ok) {
                            await this.loadTasks();
                        } else {
                            alert("停止任务失败");
                        }
                    } catch (error) {
                        console.error("Error stopping task:", error);
                    }
                },

                async deleteTask(taskId) {
                    if (!confirm("确定要删除这个任务吗？\n\n注意：此操作不可恢复！")) return;

                    try {
                        const response = await fetch(`/api/tasks/${taskId}`, {
                            method: "DELETE"
                        });

                        if (response.ok) {
                            await this.loadTasks();
                            // 显示成功提示
                            this.showNotification("任务已成功删除", "success");
                        } else {
                            alert("删除任务失败");
                        }
                    } catch (error) {
                        console.error("Error deleting task:", error);
                        alert("删除任务失败：" + error.message);
                    }
                },

                editTask(taskId) {
                    // 跳转到任务详情页面进行编辑
                    window.location.href = `/tasks/${taskId}`;
                },

                showNotification(message, type = "info") {
                    // 简单的通知显示
                    const notification = document.createElement("div");
                    notification.className = `fixed top-4 right-4 px-6 py-3 rounded-lg shadow-lg z-50 ${
                        type === "success" ? "bg-green-500" :
                        type === "error" ? "bg-red-500" :
                        "bg-blue-500"
                    } text-white`;
                    notification.textContent = message;
                    document.body.appendChild(notification);
                    setTimeout(() => {
                        notification.remove();
                    }, 3000);
                },

                refreshTasks() {
                    this.loadTasks();
                },

                resetNewTask() {
                    this.newTask = {
                        name: "",
                        task_type: "FullGeneration",
                        config: {
                            name: "自定义配置",
                            manual_db_nums: "7999",
                            gen_model: true,
                            gen_mesh: true,
                            gen_spatial_tree: true,
                            apply_boolean_operation: true,
                            mesh_tol_ratio: 3.0,
                            room_keyword: "-RM",
                            project_name: "AvevaMarineSample",
                            project_code: 1516
                        }
                    };
                },

                getStatusClass(status) {
                    const classes = {
                        "Pending": "bg-yellow-100 text-yellow-800",
                        "Running": "bg-blue-100 text-blue-800",
                        "Completed": "bg-green-100 text-green-800",
                        "Failed": "bg-red-100 text-red-800",
                        "Cancelled": "bg-gray-100 text-gray-800"
                    };
                    return classes[status] || "bg-gray-100 text-gray-800";
                },

                getStatusText(status) {
                    const texts = {
                        "Pending": "等待中",
                        "Running": "运行中",
                        "Completed": "已完成",
                        "Failed": "失败",
                        "Cancelled": "已取消"
                    };
                    return texts[status] || status;
                },

                getTaskTypeText(taskType) {
                    const texts = {
                        "database_generation": "数据解析",
                        "DataGeneration": "数据生成",
                        "SpatialTreeGeneration": "空间计算",
                        "FullGeneration": "完整生成",
                        "MeshGeneration": "网格生成",
                        "ParsePdmsData": "数据解析",
                        "GenerateModel": "模型生成",
                        "GenerateSpatialIndex": "空间计算"
                    };
                    return texts[taskType] || taskType;
                },

                formatTime(timestamp) {
                    if (!timestamp) return "未知时间";

                    // 处理不同的时间戳格式
                    let date;
                    if (typeof timestamp === "object" && timestamp.secs_since_epoch) {
                        // Rust SystemTime 格式: { secs_since_epoch: number, nanos_since_epoch: number }
                        date = new Date(timestamp.secs_since_epoch * 1000 + timestamp.nanos_since_epoch / 1000000);
                    } else if (typeof timestamp === "number") {
                        // Unix 时间戳（秒或毫秒）
                        date = timestamp > 1000000000000 ? new Date(timestamp) : new Date(timestamp * 1000);
                    } else if (typeof timestamp === "string") {
                        // ISO 字符串格式
                        date = new Date(timestamp);
                    } else {
                        // 尝试直接构造
                        date = new Date(timestamp);
                    }

                    // 检查日期是否有效
                    if (isNaN(date.getTime())) {
                        console.warn("Invalid timestamp:", timestamp);
                        return "无效时间";
                    }

                    return date.toLocaleString("zh-CN", {
                        year: "numeric",
                        month: "2-digit",
                        day: "2-digit",
                        hour: "2-digit",
                        minute: "2-digit",
                        second: "2-digit"
                    });
                },

                async viewTaskDetails(task) {
                    try {
                        const id = typeof task === 'string' ? task : task.id;
                        await this.loadTaskDetails(id);
                        await this.loadDetailsLogs(id);
                        this.showDetailsModal = true;
                        // 开始定时刷新详情，直到关闭
                        if (this.detailsTimer) { clearInterval(this.detailsTimer); this.detailsTimer = null; }
                        this.detailsTimer = setInterval(async () => {
                            try {
                                await this.loadTaskDetails(id);
                                await this.loadDetailsLogs(id);
                            } catch (_) {}
                        }, 2000);
                    } catch (e) {
                        console.error('获取任务详情失败:', e);
                        alert('获取任务详情失败');
                    }
                },
                
                // 当关闭详情时清理定时器
                closeDetails() {
                    this.showDetailsModal = false;
                    if (this.detailsTimer) { clearInterval(this.detailsTimer); this.detailsTimer = null; }
                },

                async loadTaskDetails(id) {
                    const r = await fetch(`/api/tasks/${id}`);
                    if (r.ok) this.taskDetails = await r.json();
                },

                async loadDetailsLogs(id) {
                    const r = await fetch(`/api/tasks/${id}/logs?limit=${this.detailsLogsLimit}`);
                    if (r.ok) {
                        const data = await r.json();
                        this.detailsLogs = data.logs || [];
                    }
                },

                async copyTaskJson() {
                    try {
                        const txt = JSON.stringify(this.taskDetails || {}, null, 2);
                        if (navigator.clipboard && navigator.clipboard.writeText) {
                            await navigator.clipboard.writeText(txt);
                        } else {
                            const ta = document.createElement('textarea');
                            ta.value = txt; document.body.appendChild(ta); ta.select(); document.execCommand('copy'); document.body.removeChild(ta);
                        }
                        alert('已复制任务 JSON');
                    } catch (e) {
                        console.error('复制失败', e);
                        alert('复制失败');
                    }
                },

                async viewErrorDetails(taskId) {
                    try {
                        const response = await fetch(`/api/tasks/${taskId}/error`);
                        if (response.ok) {
                            this.errorDetails = await response.json();
                            this.showErrorModal = true;
                        } else {
                            alert("获取错误详情失败");
                        }
                    } catch (error) {
                        console.error("Error fetching error details:", error);
                        alert("网络错误");
                    }
                },

                // 查看任务日志
                async viewTaskLogs(taskId) {
                    try {
                        const response = await fetch(`/api/tasks/${taskId}/logs?limit=50`);
                        if (response.ok) {
                            this.currentTaskLogs = await response.json();
                            this.filteredTaskLogs = this.currentTaskLogs.logs || [];
                            this.logLevelFilter = "";
                            this.logSearchQuery = "";
                            this.showLogsModal = true;
                        } else {
                            alert("获取任务日志失败");
                        }
                    } catch (error) {
                        console.error("Error fetching task logs:", error);
                        alert("网络错误");
                    }
                },

                // 刷新任务日志
                async refreshTaskLogs() {
                    if (this.currentTaskLogs) {
                        await this.viewTaskLogs(this.currentTaskLogs.task_id);
                    }
                },

                // 过滤日志
                filterLogs() {
                    if (!this.currentTaskLogs || !this.currentTaskLogs.logs) {
                        this.filteredTaskLogs = [];
                        return;
                    }

                    let logs = [...this.currentTaskLogs.logs];

                    // 按级别过滤
                    if (this.logLevelFilter) {
                        logs = logs.filter(log => log.level === this.logLevelFilter);
                    }

                    // 按搜索关键词过滤
                    if (this.logSearchQuery) {
                        const query = this.logSearchQuery.toLowerCase();
                        logs = logs.filter(log =>
                            log.message.toLowerCase().includes(query)
                        );
                    }

                    this.filteredTaskLogs = logs;
                },

                // 查看完整日志（跳转到独立页面）
                viewFullTaskLogs() {
                    if (this.currentTaskLogs) {
                        window.open(`/tasks/${this.currentTaskLogs.task_id}/logs`, "_blank");
                    }
                },

                // 获取日志级别样式
                getLogLevelClass(level) {
                    switch (level) {
                        case "Debug": return "bg-gray-100 text-gray-800";
                        case "Info": return "bg-blue-100 text-blue-800";
                        case "Warning": return "bg-yellow-100 text-yellow-800";
                        case "Error": return "bg-red-100 text-red-800";
                        case "Critical": return "bg-red-200 text-red-900";
                        default: return "bg-gray-100 text-gray-800";
                    }
                }
            }
        }
    </script>
</body>
</html>
    "#.to_string()
}

pub fn render_task_logs_page(task_id: String) -> String {
    format!(r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>任务日志详情 - AIOS 数据库管理平台</title>
    <link href="/static/simple-tailwind.css" rel="stylesheet">
    <link href="/static/simple-icons.css" rel="stylesheet">
    <script src="/static/alpine.min.js" defer></script>
    <style>
        .log-entry {{
            transition: background-color 0.2s;
        }}
        .log-entry:hover {{
            background-color: #f9fafb;
        }}
        .log-timestamp {{
            font-family: "Courier New", monospace;
            font-size: 0.75rem;
        }}
    </style>
</head>
<body class="bg-gray-50" x-data="taskLogsViewer("{}")">
    <div class="min-h-screen">
        <!-- 导航栏 -->
        <nav class="bg-blue-600 text-white shadow-lg">
            <div class="max-w-7xl mx-auto px-4">
                <div class="flex justify-between items-center py-4">
                    <div class="flex items-center space-x-4">
                        <i class="fas fa-database text-2xl"></i>
                        <h1 class="text-xl font-bold">AIOS 数据库管理平台</h1>
                    </div>
                    <div class="flex space-x-4">
                        <a href="/" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-home mr-2"></i>首页
                        </a>
                        <a href="/dashboard" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tachometer-alt mr-2"></i>仪表板
                        </a>
                        <a href="/tasks" class="bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tasks mr-2"></i>任务管理
                        </a>
                        <a href="/batch-tasks" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-layer-group mr-2"></i>批量任务
                        </a>
                        <a href="/db-status" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-database mr-2"></i>数据库状态
                        </a>
                        <a href="/wizard" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-magic mr-2"></i>解析向导
                        </a>
                        <a href="/deployment-sites" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-server mr-2"></i>部署站点
                        </a>
                    </div>
                </div>
            </div>
        </nav>

        <!-- 主要内容 -->
        <div class="max-w-7xl mx-auto px-4 py-8">
            <!-- 页面标题和操作 -->
            <div class="flex justify-between items-center mb-6">
                <div>
                    <h2 class="text-2xl font-bold text-gray-800 flex items-center">
                        <i class="fas fa-file-alt mr-3 text-purple-600"></i>
                        任务日志详情
                    </h2>
                    <p x-show="taskInfo" class="text-gray-600 mt-1">
                        <span x-text="taskInfo?.name"></span>
                        <span class="mx-2">•</span>
                        <span x-text=""ID: " + taskId.substring(0, 8)"></span>
                        <span class="mx-2">•</span>
                        状态: <span x-text="getStatusText(taskInfo?.status)"
                                  :class="getStatusClass(taskInfo?.status)"
                                  class="px-2 py-1 rounded text-xs font-medium"></span>
                    </p>
                </div>
                <div class="flex space-x-3">
                    <button @click="refreshLogs()"
                            class="bg-purple-600 text-white px-4 py-2 rounded-lg hover:bg-purple-700 transition">
                        <i class="fas fa-sync-alt mr-2"></i>刷新日志
                    </button>
                    <button @click="toggleAutoRefresh()"
                            :class="autoRefresh ? "bg-green-600 hover:bg-green-700" : "bg-gray-600 hover:bg-gray-700""
                            class="text-white px-4 py-2 rounded-lg transition">
                        <i class="fas fa-play mr-2" x-show="!autoRefresh"></i>
                        <i class="fas fa-pause mr-2" x-show="autoRefresh"></i>
                        <span x-text="autoRefresh ? "停止自动刷新" : "开启自动刷新""></span>
                    </button>
                    <a href="/tasks" class="bg-gray-600 text-white px-4 py-2 rounded-lg hover:bg-gray-700 transition">
                        <i class="fas fa-arrow-left mr-2"></i>返回任务列表
                    </a>
                </div>
            </div>

            <!-- 过滤器和搜索 -->
            <div class="bg-white rounded-lg shadow-md p-6 mb-6">
                <div class="flex flex-wrap gap-4 items-center">
                    <div class="flex items-center space-x-2">
                        <label class="text-sm font-medium text-gray-700">日志级别:</label>
                        <select x-model="filters.level" @change="applyFilters()"
                                class="border border-gray-300 rounded px-3 py-2 text-sm">
                            <option value="">全部级别</option>
                            <option value="Debug">Debug</option>
                            <option value="Info">Info</option>
                            <option value="Warning">Warning</option>
                            <option value="Error">Error</option>
                            <option value="Critical">Critical</option>
                        </select>
                    </div>
                    <div class="flex items-center space-x-2">
                        <label class="text-sm font-medium text-gray-700">搜索:</label>
                        <input x-model="filters.search" @input="applyFilters()"
                               type="text" placeholder="搜索日志内容..."
                               class="border border-gray-300 rounded px-3 py-2 text-sm w-64">
                    </div>
                    <div class="flex items-center space-x-2">
                        <label class="text-sm font-medium text-gray-700">显示条数:</label>
                        <select x-model="filters.limit" @change="loadLogs()"
                                class="border border-gray-300 rounded px-2 py-2 text-sm">
                            <option value="50">50条</option>
                            <option value="100">100条</option>
                            <option value="200">200条</option>
                            <option value="500">500条</option>
                        </select>
                    </div>
                    <button @click="clearFilters()"
                            class="px-3 py-2 text-sm bg-gray-500 text-white rounded hover:bg-gray-600">
                        <i class="fas fa-times mr-1"></i>清除过滤
                    </button>
                </div>
            </div>

            <!-- 日志统计 -->
            <div x-show="logStats" class="grid grid-cols-2 md:grid-cols-6 gap-4 mb-6">
                <div class="bg-white rounded-lg shadow p-4 text-center">
                    <div class="text-2xl font-bold text-gray-800" x-text="logStats?.total || 0"></div>
                    <div class="text-sm text-gray-600">总计</div>
                </div>
                <div class="bg-white rounded-lg shadow p-4 text-center">
                    <div class="text-2xl font-bold text-blue-600" x-text="logStats?.info || 0"></div>
                    <div class="text-sm text-gray-600">Info</div>
                </div>
                <div class="bg-white rounded-lg shadow p-4 text-center">
                    <div class="text-2xl font-bold text-yellow-600" x-text="logStats?.warning || 0"></div>
                    <div class="text-sm text-gray-600">Warning</div>
                </div>
                <div class="bg-white rounded-lg shadow p-4 text-center">
                    <div class="text-2xl font-bold text-red-600" x-text="logStats?.error || 0"></div>
                    <div class="text-sm text-gray-600">Error</div>
                </div>
                <div class="bg-white rounded-lg shadow p-4 text-center">
                    <div class="text-2xl font-bold text-red-800" x-text="logStats?.critical || 0"></div>
                    <div class="text-sm text-gray-600">Critical</div>
                </div>
                <div class="bg-white rounded-lg shadow p-4 text-center">
                    <div class="text-2xl font-bold text-gray-600" x-text="logStats?.debug || 0"></div>
                    <div class="text-sm text-gray-600">Debug</div>
                </div>
            </div>

            <!-- 日志内容 -->
            <div class="bg-white rounded-lg shadow-md">
                <div class="p-6 border-b border-gray-200">
                    <div class="flex justify-between items-center">
                        <h3 class="text-lg font-semibold text-gray-800">
                            日志记录
                            <span x-show="filteredLogs" x-text=""(显示 " + filteredLogs.length + " 条)"" class="text-sm text-gray-600"></span>
                        </h3>
                        <div x-show="loading" class="flex items-center text-gray-600">
                            <i class="fas fa-spinner fa-spin mr-2"></i>
                            加载中...
                        </div>
                    </div>
                </div>

                <div class="max-h-screen overflow-y-auto">
                    <template x-for="log in filteredLogs" :key="log.timestamp">
                        <div class="log-entry border-b border-gray-100 p-4 hover:bg-gray-50">
                            <div class="flex items-start space-x-3">
                                <div class="flex-shrink-0">
                                    <span class="log-timestamp text-gray-500" x-text="formatDetailedTime(log.timestamp)"></span>
                                </div>
                                <div class="flex-shrink-0">
                                    <span class="px-2 py-1 rounded text-xs font-medium"
                                          :class="getLogLevelClass(log.level)"
                                          x-text="log.level"></span>
                                </div>
                                <div class="flex-1 min-w-0">
                                    <p class="text-sm text-gray-900 break-words" x-text="log.message"></p>
                                    <div x-show="log.error_code" class="mt-1">
                                        <span class="text-xs text-gray-500">错误代码: </span>
                                        <span class="text-xs font-mono bg-gray-100 px-1 rounded" x-text="log.error_code"></span>
                                    </div>
                                    <div x-show="log.stack_trace" class="mt-2">
                                        <details class="text-xs">
                                            <summary class="cursor-pointer text-gray-600 hover:text-gray-800">
                                                <i class="fas fa-code mr-1"></i>查看堆栈跟踪
                                            </summary>
                                            <pre class="mt-2 p-2 bg-gray-100 rounded text-xs overflow-x-auto" x-text="log.stack_trace"></pre>
                                        </details>
                                    </div>
                                </div>
                            </div>
                        </div>
                    </template>

                    <div x-show="!filteredLogs || filteredLogs.length === 0"
                         class="text-center text-gray-500 py-12">
                        <i class="fas fa-file-alt text-4xl mb-4"></i>
                        <p>暂无日志记录</p>
                    </div>
                </div>
            </div>
        </div>
    </div>

    <script>
        function taskLogsViewer(taskId) {{
            return {{
                taskId: taskId,
                taskInfo: null,
                logs: [],
                filteredLogs: [],
                logStats: null,
                loading: false,
                autoRefresh: false,
                refreshInterval: null,
                filters: {{
                    level: "",
                    search: "",
                    limit: 100
                }},

                async init() {{
                    await this.loadTaskInfo();
                    await this.loadLogs();
                    this.calculateStats();
                }},

                async loadTaskInfo() {{
                    try {{
                        const response = await fetch(`/api/tasks/${{this.taskId}}`);
                        if (response.ok) {{
                            this.taskInfo = await response.json();
                        }}
                    }} catch (error) {{
                        console.error("Error loading task info:", error);
                    }}
                }},

                async loadLogs() {{
                    this.loading = true;
                    try {{
                        const params = new URLSearchParams({{
                            limit: this.filters.limit
                        }});

                        const response = await fetch(`/api/tasks/${{this.taskId}}/logs?${{params}}`);
                        if (response.ok) {{
                            const data = await response.json();
                            this.logs = data.logs || [];
                            this.applyFilters();
                            this.calculateStats();
                        }} else {{
                            alert("获取日志失败");
                        }}
                    }} catch (error) {{
                        console.error("Error loading logs:", error);
                        alert("网络错误");
                    }} finally {{
                        this.loading = false;
                    }}
                }},

                applyFilters() {{
                    let filtered = [...this.logs];

                    // 按级别过滤
                    if (this.filters.level) {{
                        filtered = filtered.filter(log => log.level === this.filters.level);
                    }}

                    // 按搜索关键词过滤
                    if (this.filters.search) {{
                        const query = this.filters.search.toLowerCase();
                        filtered = filtered.filter(log =>
                            log.message.toLowerCase().includes(query)
                        );
                    }}

                    this.filteredLogs = filtered;
                }},

                calculateStats() {{
                    const stats = {{
                        total: this.logs.length,
                        debug: 0,
                        info: 0,
                        warning: 0,
                        error: 0,
                        critical: 0
                    }};

                    this.logs.forEach(log => {{
                        switch (log.level.toLowerCase()) {{
                            case "debug": stats.debug++; break;
                            case "info": stats.info++; break;
                            case "warning": stats.warning++; break;
                            case "error": stats.error++; break;
                            case "critical": stats.critical++; break;
                        }}
                    }});

                    this.logStats = stats;
                }},

                clearFilters() {{
                    this.filters.level = "";
                    this.filters.search = "";
                    this.applyFilters();
                }},

                async refreshLogs() {{
                    await this.loadLogs();
                }},

                toggleAutoRefresh() {{
                    this.autoRefresh = !this.autoRefresh;

                    if (this.autoRefresh) {{
                        this.refreshInterval = setInterval(() => {{
                            this.loadLogs();
                        }}, 5000); // 每5秒刷新一次
                    }} else {{
                        if (this.refreshInterval) {{
                            clearInterval(this.refreshInterval);
                            this.refreshInterval = null;
                        }}
                    }}
                }},

                formatDetailedTime(timestamp) {{
                    const date = new Date(timestamp);
                    return date.toLocaleString("zh-CN", {{
                        year: "numeric",
                        month: "2-digit",
                        day: "2-digit",
                        hour: "2-digit",
                        minute: "2-digit",
                        second: "2-digit",
                        fractionalSecondDigits: 3
                    }});
                }},

                getLogLevelClass(level) {{
                    switch (level) {{
                        case "Debug": return "bg-gray-100 text-gray-800";
                        case "Info": return "bg-blue-100 text-blue-800";
                        case "Warning": return "bg-yellow-100 text-yellow-800";
                        case "Error": return "bg-red-100 text-red-800";
                        case "Critical": return "bg-red-200 text-red-900";
                        default: return "bg-gray-100 text-gray-800";
                    }}
                }},

                getStatusText(status) {{
                    switch (status) {{
                        case "Pending": return "等待中";
                        case "Running": return "运行中";
                        case "Completed": return "已完成";
                        case "Failed": return "失败";
                        case "Cancelled": return "已取消";
                        default: return status;
                    }}
                }},

                getStatusClass(status) {{
                    switch (status) {{
                        case "Pending": return "bg-yellow-100 text-yellow-800";
                        case "Running": return "bg-blue-100 text-blue-800";
                        case "Completed": return "bg-green-100 text-green-800";
                        case "Failed": return "bg-red-100 text-red-800";
                        case "Cancelled": return "bg-gray-100 text-gray-800";
                        default: return "bg-gray-100 text-gray-800";
                    }}
                }}
            }}
        }}
    </script>
</body>
</html>
    "#, task_id)
}

/// 渲染数据库状态管理页面
pub fn render_db_status_page() -> String {
    format!(r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>数据库状态管理 - AIOS 数据库管理平台</title>
    <link href="/static/simple-tailwind.css" rel="stylesheet">
    <link href="/static/simple-icons.css" rel="stylesheet">
    <script src="/static/alpine.min.js" defer></script>
    <style>
        .status-badge { display:inline-block; padding:0.25rem 0.5rem; border-radius:9999px; font-size:0.75rem; font-weight:500; }
        .status-parsed { background-color: rgb(220 252 231); color: rgb(22 101 52); }
        .status-not-parsed { background-color: rgb(243 244 246); color: rgb(31 41 55); }
        .status-parsing { background-color: rgb(254 249 195); color: rgb(133 77 14); }
        .status-parse-failed { background-color: rgb(254 226 226); color: rgb(153 27 27); }
        .status-generated { background-color: rgb(219 234 254); color: rgb(30 64 175); }
        .status-not-generated { background-color: rgb(243 244 246); color: rgb(31 41 55); }
        .status-generating { background-color: rgb(243 232 255); color: rgb(107 33 168); }
        .status-generation-failed { background-color: rgb(254 226 226); color: rgb(153 27 27); }
        .needs-update { background-color: rgb(255 247 237); border-left: 4px solid rgb(251 146 60); }
    </style>
</head>
<body class="bg-gray-50" x-data="dbStatusManager()">
    <div class="min-h-screen">
        <!-- 导航栏 -->
        <nav class="bg-blue-600 text-white shadow-lg">
            <div class="max-w-7xl mx-auto px-4">
                <div class="flex justify-between items-center py-4">
                    <div class="flex items-center space-x-3">
                        <i class="fas fa-database text-2xl"></i>
                        <h1 class="text-xl font-bold">AIOS 数据库管理平台</h1>
                    </div>
                    <div class="flex space-x-4">
                        <a href="/" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-home mr-2"></i>首页
                        </a>
                        <a href="/dashboard" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tachometer-alt mr-2"></i>仪表板
                        </a>
                        <a href="/tasks" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tasks mr-2"></i>任务管理
                        </a>
                        <a href="/config" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-cog mr-2"></i>配置管理
                        </a>
                        <a href="/db-status" class="bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-database mr-2"></i>数据库状态
                        </a>
                    </div>
                </div>
            </div>
        </nav>

        <!-- 主要内容 -->
        <div class="max-w-7xl mx-auto px-4 py-8">
            <!-- 页面标题和操作 -->
            <div class="flex justify-between items-center mb-8">
                <div>
                    <h2 class="text-3xl font-bold text-gray-800">数据库状态管理</h2>
                    <p class="text-gray-600 mt-2">监控和管理PDMS数据库的解析状态、模型生成状态和版本信息</p>
                </div>
                <div class="flex space-x-3">
                    <button @click="refreshStatus()"
                            class="bg-blue-600 text-white px-4 py-2 rounded-lg hover:bg-blue-700 transition">
                        <i class="fas fa-sync-alt mr-2"></i>刷新状态
                    </button>
                    <button @click="checkVersions()"
                            class="bg-green-600 text-white px-4 py-2 rounded-lg hover:bg-green-700 transition">
                        <i class="fas fa-check-circle mr-2"></i>检查版本
                    </button>
                    <button @click="showBatchUpdateModal = true"
                            class="bg-orange-600 text-white px-4 py-2 rounded-lg hover:bg-orange-700 transition">
                        <i class="fas fa-upload mr-2"></i>批量更新
                    </button>
                </div>
            </div>

            <!-- 统计卡片 -->
            <div class="grid grid-cols-1 md:grid-cols-4 gap-6 mb-8">
                <div class="bg-white rounded-lg shadow-md p-6">
                    <div class="flex items-center">
                        <div class="p-3 rounded-full bg-blue-100 text-blue-600">
                            <i class="fas fa-database text-xl"></i>
                        </div>
                        <div class="ml-4">
                            <p class="text-sm font-medium text-gray-600">总数据库数</p>
                            <p class="text-2xl font-bold text-gray-900" x-text="stats.total"></p>
                        </div>
                    </div>
                </div>

                <div class="bg-white rounded-lg shadow-md p-6">
                    <div class="flex items-center">
                        <div class="p-3 rounded-full bg-green-100 text-green-600">
                            <i class="fas fa-check-circle text-xl"></i>
                        </div>
                        <div class="ml-4">
                            <p class="text-sm font-medium text-gray-600">已解析</p>
                            <p class="text-2xl font-bold text-gray-900" x-text="stats.parsed"></p>
                        </div>
                    </div>
                </div>

                <div class="bg-white rounded-lg shadow-md p-6">
                    <div class="flex items-center">
                        <div class="p-3 rounded-full bg-purple-100 text-purple-600">
                            <i class="fas fa-cube text-xl"></i>
                        </div>
                        <div class="ml-4">
                            <p class="text-sm font-medium text-gray-600">已生成模型</p>
                            <p class="text-2xl font-bold text-gray-900" x-text="stats.generated"></p>
                        </div>
                    </div>
                </div>

                <div class="bg-white rounded-lg shadow-md p-6">
                    <div class="flex items-center">
                        <div class="p-3 rounded-full bg-orange-100 text-orange-600">
                            <i class="fas fa-exclamation-triangle text-xl"></i>
                        </div>
                        <div class="ml-4">
                            <p class="text-sm font-medium text-gray-600">需要更新</p>
                            <p class="text-2xl font-bold text-gray-900" x-text="stats.needsUpdate"></p>
                        </div>
                    </div>
                </div>
            </div>

            <!-- 过滤器 -->
            <div class="bg-white rounded-lg shadow-md p-6 mb-8">
                <h3 class="text-lg font-semibold mb-4">过滤器</h3>
                <div class="grid grid-cols-1 md:grid-cols-4 gap-4">
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">项目</label>
                        <select x-model="filters.project" @change="applyFilters()"
                                class="w-full border border-gray-300 rounded-md px-3 py-2">
                            <option value="">全部项目</option>
                            <option value="AvevaMarineSample">AvevaMarineSample</option>
                        </select>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">数据库类型</label>
                        <select x-model="filters.dbType" @change="applyFilters()"
                                class="w-full border border-gray-300 rounded-md px-3 py-2">
                            <option value="">全部类型</option>
                            <option value="DESI">DESI</option>
                            <option value="CATA">CATA</option>
                            <option value="DICT">DICT</option>
                            <option value="SYST">SYST</option>
                            <option value="GLB">GLB</option>
                            <option value="GLOB">GLOB</option>
                        </select>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">状态</label>
                        <select x-model="filters.status" @change="applyFilters()"
                                class="w-full border border-gray-300 rounded-md px-3 py-2">
                            <option value="">全部状态</option>
                            <option value="parsed">已解析</option>
                            <option value="not_parsed">未解析</option>
                            <option value="generated">已生成模型</option>
                            <option value="not_generated">未生成模型</option>
                        </select>
                    </div>
                    <div class="flex items-end">
                        <label class="flex items-center">
                            <input type="checkbox" x-model="filters.needsUpdateOnly" @change="applyFilters()"
                                   class="rounded border-gray-300 text-blue-600 mr-2">
                            <span class="text-sm font-medium text-gray-700">仅显示需要更新</span>
                        </label>
                    </div>
                </div>
            </div>

            <!-- 数据库状态表 -->
            <div class="bg-white rounded-lg shadow-md overflow-hidden">
                <div class="px-6 py-4 border-b border-gray-200">
                    <div class="flex justify-between items-center">
                        <h3 class="text-lg font-semibold text-gray-900">数据库状态列表</h3>
                        <div class="flex items-center space-x-2">
                            <span class="text-sm text-gray-500">共 <span x-text="filteredDbList.length"></span> 个数据库</span>
                        </div>
                    </div>
                </div>

                <div class="overflow-x-auto">
                    <table class="min-w-full divide-y divide-gray-200">
                        <thead class="bg-gray-50">
                            <tr>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    <input type="checkbox" @change="toggleSelectAll()"
                                           :checked="selectedDbs.length === filteredDbList.length && filteredDbList.length > 0"
                                           class="rounded border-gray-300 text-blue-600">
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    数据库编号
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    文件名
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    类型
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    解析状态
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    模型状态
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    记录数
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    会话号
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    更新时间
                                </th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                                    操作
                                </th>
                            </tr>
                        </thead>
                        <tbody class="bg-white divide-y divide-gray-200">
                            <template x-for="db in filteredDbList" :key="db.dbnum">
                                <tr :class="db.needs_update ? "needs-update" : """>
                                    <td class="px-6 py-4 whitespace-nowrap">
                                        <input type="checkbox" :value="db.dbnum" x-model="selectedDbs"
                                               class="rounded border-gray-300 text-blue-600">
                                    </td>
                                    <td class="px-6 py-4 whitespace-nowrap">
                                        <div class="text-sm font-medium text-gray-900" x-text="db.dbnum"></div>
                                        <div class="text-sm text-gray-500" x-text="db.project"></div>
                                    </td>
                                    <td class="px-6 py-4 whitespace-nowrap">
                                        <div class="text-sm text-gray-900" x-text="db.file_name || "未知""></div>
                                        <div class="text-sm text-gray-500" x-show="db.file_version">
                                            文件版本: <span x-text="db.file_version?.file_version || "N/A""></span>
                                        </div>
                                    </td>
                                    <td class="px-6 py-4 whitespace-nowrap">
                                        <span class="px-2 inline-flex text-xs leading-5 font-semibold rounded-full bg-gray-100 text-gray-800"
                                              x-text="db.db_type"></span>
                                    </td>
                                    <td class="px-6 py-4 whitespace-nowrap">
                                        <span class="status-badge"
                                              :class="getParseStatusClass(db.parse_status)"
                                              x-text="getParseStatusText(db.parse_status)"></span>
                                    </td>
                                    <td class="px-6 py-4 whitespace-nowrap">
                                        <span class="status-badge"
                                              :class="getModelStatusClass(db.model_status)"
                                              x-text="getModelStatusText(db.model_status)"></span>
                                    </td>
                                    <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                        <span x-text="db.count.toLocaleString()"></span>
                                    </td>
                                    <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                                        <span x-text="db.sesno"></span>
                                    </td>
                                    <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
                                        <span x-text="formatTime(db.updated_at)"></span>
                                    </td>
                                    <td class="px-6 py-4 whitespace-nowrap text-sm font-medium">
                                        <div class="flex space-x-2">
                                            <button @click="updateSingleDb(db.dbnum)"
                                                    class="text-blue-600 hover:text-blue-900"
                                                    title="增量更新">
                                                <i class="fas fa-sync-alt"></i>
                                            </button>
                                            <button @click="viewDbDetail(db.dbnum)"
                                                    class="text-green-600 hover:text-green-900"
                                                    title="查看详情">
                                                <i class="fas fa-eye"></i>
                                            </button>
                                            <button x-show="db.needs_update"
                                                    @click="updateSingleDb(db.dbnum)"
                                                    class="text-orange-600 hover:text-orange-900"
                                                    title="需要更新">
                                                <i class="fas fa-exclamation-triangle"></i>
                                            </button>
                                        </div>
                                    </td>
                                </tr>
                            </template>
                        </tbody>
                    </table>
                </div>
            </div>
        </div>
    </div>

    <!-- 批量更新模态框 -->
    <div x-show="showBatchUpdateModal"
         x-transition:enter="transition ease-out duration-300"
         x-transition:enter-start="opacity-0"
         x-transition:enter-end="opacity-100"
         x-transition:leave="transition ease-in duration-200"
         x-transition:leave-start="opacity-100"
         x-transition:leave-end="opacity-0"
         class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-1000">
        <div class="relative top-20 mx-auto p-5 border w-96 shadow-lg rounded-md bg-white z-1010">
            <div class="mt-3">
                <h3 class="text-lg font-medium text-gray-900 mb-4">批量更新数据库</h3>
                <div class="mb-4">
                    <p class="text-sm text-gray-600 mb-2">
                        已选择 <span x-text="selectedDbs.length"></span> 个数据库
                    </p>
                    <div class="max-h-32 overflow-y-auto bg-gray-50 p-2 rounded">
                        <template x-for="dbnum in selectedDbs" :key="dbnum">
                            <span class="inline-block bg-blue-100 text-blue-800 text-xs px-2 py-1 rounded mr-1 mb-1"
                                  x-text="dbnum"></span>
                        </template>
                    </div>
                </div>

                <div class="mb-4">
                    <label class="block text-sm font-medium text-gray-700 mb-2">更新类型</label>
                    <select x-model="batchUpdateType"
                            class="w-full border border-gray-300 rounded-md px-3 py-2">
                        <option value="ParseOnly">仅解析数据</option>
                        <option value="ParseAndModel">解析并生成模型</option>
                        <option value="Full">完整更新（解析+模型+网格）</option>
                    </select>
                </div>

                <div class="mb-4">
                    <label class="flex items-center">
                        <input type="checkbox" x-model="forceUpdate"
                               class="rounded border-gray-300 text-blue-600 mr-2">
                        <span class="text-sm text-gray-700">强制更新（忽略版本检查）</span>
                    </label>
                </div>

                <div class="flex justify-end space-x-3">
                    <button @click="showBatchUpdateModal = false"
                            class="bg-gray-300 text-gray-700 px-4 py-2 rounded hover:bg-gray-400 transition">
                        取消
                    </button>
                    <button @click="executeBatchUpdate()"
                            class="bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700 transition">
                        开始更新
                    </button>
                </div>
            </div>
        </div>
    </div>

    <script>
        function dbStatusManager() {{
            return {{
                dbList: [],
                filteredDbList: [],
                selectedDbs: [],
                showBatchUpdateModal: false,
                batchUpdateType: "ParseOnly",
                forceUpdate: false,
                stats: {{
                    total: 0,
                    parsed: 0,
                    generated: 0,
                    needsUpdate: 0
                }},
                filters: {{
                    project: "",
                    dbType: "",
                    status: "",
                    needsUpdateOnly: false
                }},

                async init() {{
                    await this.loadDbStatus();
                    this.applyFilters();

                    // 定期刷新数据
                    setInterval(() => {{
                        this.loadDbStatus();
                    }}, 30000); // 30秒刷新一次
                }},

                async loadDbStatus() {{
                    try {{
                        const response = await fetch("/api/db-status");
                        if (response.ok) {{
                            const data = await response.json();
                            this.dbList = data.status_list || [];
                            this.updateStats();
                            this.applyFilters();
                        }}
                    }} catch (error) {{
                        console.error("Failed to load database status:", error);
                    }}
                }},

                updateStats() {{
                    this.stats.total = this.dbList.length;
                    this.stats.parsed = this.dbList.filter(db => db.parse_status === "Parsed").length;
                    this.stats.generated = this.dbList.filter(db => db.model_status === "Generated").length;
                    this.stats.needsUpdate = this.dbList.filter(db => db.needs_update).length;
                }},

                applyFilters() {{
                    this.filteredDbList = this.dbList.filter(db => {{
                        if (this.filters.project && db.project !== this.filters.project) return false;
                        if (this.filters.dbType && db.db_type !== this.filters.dbType) return false;
                        if (this.filters.needsUpdateOnly && !db.needs_update) return false;

                        if (this.filters.status) {{
                            switch (this.filters.status) {{
                                case "parsed":
                                    return db.parse_status === "Parsed";
                                case "not_parsed":
                                    return db.parse_status !== "Parsed";
                                case "generated":
                                    return db.model_status === "Generated";
                                case "not_generated":
                                    return db.model_status !== "Generated";
                            }}
                        }}

                        return true;
                    }});
                }},

                async refreshStatus() {{
                    await this.loadDbStatus();
                }},

                async checkVersions() {{
                    try {{
                        const response = await fetch("/api/db-status/check-versions");
                        if (response.ok) {{
                            const data = await response.json();
                            alert(`版本检查完成！需要更新的数据库: ${{data.needs_update_count}} 个`);
                            await this.loadDbStatus();
                        }}
                    }} catch (error) {{
                        console.error("Failed to check versions:", error);
                        alert("版本检查失败");
                    }}
                }},

                toggleSelectAll() {{
                    if (this.selectedDbs.length === this.filteredDbList.length) {{
                        this.selectedDbs = [];
                    }} else {{
                        this.selectedDbs = this.filteredDbList.map(db => db.dbnum);
                    }}
                }},

                async updateSingleDb(dbnum) {{
                    await this.executeUpdate([dbnum], "ParseAndModel");
                }},

                async executeBatchUpdate() {{
                    if (this.selectedDbs.length === 0) {{
                        alert("请选择要更新的数据库");
                        return;
                    }}

                    await this.executeUpdate(this.selectedDbs, this.batchUpdateType);
                    this.showBatchUpdateModal = false;
                    this.selectedDbs = [];
                }},

                async executeUpdate(dbnums, updateType) {{
                    try {{
                        const response = await fetch("/api/db-status/update", {{
                            method: "POST",
                            headers: {{ "Content-Type": "application/json" }},
                            body: JSON.stringify({{
                                dbnums: dbnums,
                                update_type: updateType,
                                force_update: this.forceUpdate
                            }})
                        }});

                        if (response.ok) {{
                            const data = await response.json();
                            alert(`更新任务已启动！任务ID: ${{data.task_id}}`);
                            // 跳转到任务管理页面
                            window.location.href = "/tasks";
                        }} else {{
                            alert("更新任务启动失败");
                        }}
                    }} catch (error) {{
                        console.error("Failed to execute update:", error);
                        alert("网络错误");
                    }}
                }},

                viewDbDetail(dbnum) {{
                    // 这里可以实现查看详情的逻辑
                    alert(`查看数据库 ${{dbnum}} 的详细信息`);
                }},

                getParseStatusClass(status) {{
                    const classes = {{
                        "Parsed": "status-parsed",
                        "NotParsed": "status-not-parsed",
                        "Parsing": "status-parsing",
                        "ParseFailed": "status-parse-failed"
                    }};
                    return classes[status] || "status-not-parsed";
                }},

                getParseStatusText(status) {{
                    const texts = {{
                        "Parsed": "已解析",
                        "NotParsed": "未解析",
                        "Parsing": "解析中",
                        "ParseFailed": "解析失败"
                    }};
                    return texts[status] || "未知";
                }},

                getModelStatusClass(status) {{
                    const classes = {{
                        "Generated": "status-generated",
                        "NotGenerated": "status-not-generated",
                        "Generating": "status-generating",
                        "GenerationFailed": "status-generation-failed"
                    }};
                    return classes[status] || "status-not-generated";
                }},

                getModelStatusText(status) {{
                    const texts = {{
                        "Generated": "已生成",
                        "NotGenerated": "未生成",
                        "Generating": "生成中",
                        "GenerationFailed": "生成失败"
                    }};
                    return texts[status] || "未知";
                }},

                formatTime(timestamp) {{
                    if (!timestamp) return "未知时间";

                    let date;
                    if (typeof timestamp === "object" && timestamp.secs_since_epoch) {{
                        date = new Date(timestamp.secs_since_epoch * 1000 + timestamp.nanos_since_epoch / 1000000);
                    }} else if (typeof timestamp === "number") {{
                        date = timestamp > 1000000000000 ? new Date(timestamp) : new Date(timestamp * 1000);
                    }} else if (typeof timestamp === "string") {{
                        date = new Date(timestamp);
                    }} else {{
                        date = new Date(timestamp);
                    }}

                    if (isNaN(date.getTime())) {{
                        return "无效时间";
                    }}

                    return date.toLocaleString("zh-CN", {{
                        year: "numeric",
                        month: "2-digit",
                        day: "2-digit",
                        hour: "2-digit",
                        minute: "2-digit"
                    }});
                }}
            }}
        }}
    </script>
</body>
</html>
"#)
}

/// 渲染数据解析向导页面
pub fn render_wizard_page() -> String {
    r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>数据解析向导 - AIOS 数据库管理平台</title>
    <link href="/static/simple-tailwind.css" rel="stylesheet">
    <link href="/static/simple-icons.css" rel="stylesheet">
    <script src="/static/alpine.min.js" defer></script>
</head>
<body class="bg-gray-50" x-data="wizardManager()">
    <div class="min-h-screen">
        <!-- 导航栏 -->
        <nav class="bg-blue-600 text-white shadow-lg">
            <div class="max-w-7xl mx-auto px-4">
                <div class="flex justify-between items-center py-4">
                    <div class="flex items-center space-x-4">
                        <i class="fas fa-magic text-2xl"></i>
                        <h1 class="text-xl font-bold">数据解析向导</h1>
                    </div>
                    <div class="flex space-x-4">
                        <a href="/" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-home mr-2"></i>首页
                        </a>
                        <a href="/dashboard" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tachometer-alt mr-2"></i>仪表板
                        </a>
                        <a href="/tasks" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tasks mr-2"></i>任务管理
                        </a>
                        <a href="/db-status" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-database mr-2"></i>数据库状态
                        </a>
                    </div>
                </div>
            </div>
        </nav>

        <!-- 主要内容 -->
        <div class="max-w-7xl mx-auto px-4 py-8">
            <!-- 步骤指示器 -->
            <div class="mb-8">
                <div class="flex items-center justify-center">
                    <div class="flex items-center space-x-4">
                        <!-- 步骤1: 选择目录 -->
                        <div class="flex items-center">
                            <div class="flex items-center justify-center w-10 h-10 rounded-full"
                                 :class="currentStep >= 1 ? "bg-blue-600 text-white" : "bg-gray-300 text-gray-600"">
                                <i class="fas fa-folder"></i>
                            </div>
                            <span class="ml-2 text-sm font-medium"
                                  :class="currentStep >= 1 ? "text-blue-600" : "text-gray-500"">选择目录</span>
                        </div>

                        <div class="w-16 h-1 bg-gray-300" :class="currentStep >= 2 ? "bg-blue-600" : "bg-gray-300""></div>

                        <!-- 步骤2: 选择项目 -->
                        <div class="flex items-center">
                            <div class="flex items-center justify-center w-10 h-10 rounded-full"
                                 :class="currentStep >= 2 ? "bg-blue-600 text-white" : "bg-gray-300 text-gray-600"">
                                <i class="fas fa-project-diagram"></i>
                            </div>
                            <span class="ml-2 text-sm font-medium"
                                  :class="currentStep >= 2 ? "text-blue-600" : "text-gray-500"">选择项目</span>
                        </div>

                        <div class="w-16 h-1 bg-gray-300" :class="currentStep >= 3 ? "bg-blue-600" : "bg-gray-300""></div>

                        <!-- 步骤3: 配置参数 -->
                        <div class="flex items-center">
                            <div class="flex items-center justify-center w-10 h-10 rounded-full"
                                 :class="currentStep >= 3 ? "bg-blue-600 text-white" : "bg-gray-300 text-gray-600"">
                                <i class="fas fa-cogs"></i>
                            </div>
                            <span class="ml-2 text-sm font-medium"
                                  :class="currentStep >= 3 ? "text-blue-600" : "text-gray-500"">配置参数</span>
                        </div>

                        <div class="w-16 h-1 bg-gray-300" :class="currentStep >= 4 ? "bg-blue-600" : "bg-gray-300""></div>

                        <!-- 步骤4: 执行任务 -->
                        <div class="flex items-center">
                            <div class="flex items-center justify-center w-10 h-10 rounded-full"
                                 :class="currentStep >= 4 ? "bg-blue-600 text-white" : "bg-gray-300 text-gray-600"">
                                <i class="fas fa-play"></i>
                            </div>
                            <span class="ml-2 text-sm font-medium"
                                  :class="currentStep >= 4 ? "text-blue-600" : "text-gray-500"">执行任务</span>
                        </div>
                    </div>
                </div>
            </div>

            <!-- 步骤内容 -->
            <div class="bg-white rounded-lg shadow-lg p-6">
                <!-- 步骤1: 选择目录 -->
                <div x-show="currentStep === 1" x-transition>
                    <h2 class="text-2xl font-bold mb-6 text-gray-800">
                        <i class="fas fa-folder mr-2 text-blue-600"></i>选择项目目录
                    </h2>

                    <div class="space-y-4">
                        <div>
                            <label class="block text-sm font-medium text-gray-700 mb-2">
                                项目根目录路径
                            </label>
                            <div class="flex space-x-2">
                                <input type="text"
                                       x-model="directoryPath"
                                       class="flex-1 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
                                       placeholder="例如: /Volumes/DPC/work/e3d_models">
                                <button @click="scanDirectory()"
                                        :disabled="!directoryPath || scanning"
                                        class="px-4 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700 disabled:bg-gray-400">
                                    <i class="fas fa-search mr-2"></i>
                                    <span x-text="scanning ? "扫描中..." : "扫描""></span>
                                </button>
                            </div>
                        </div>

                        <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                            <div>
                                <label class="flex items-center">
                                    <input type="checkbox" x-model="scanRecursive" class="mr-2">
                                    <span class="text-sm text-gray-700">递归扫描子目录</span>
                                </label>
                            </div>
                            <div>
                                <label class="block text-sm text-gray-700">
                                    最大扫描深度:
                                    <select x-model="maxDepth" class="ml-2 px-2 py-1 border border-gray-300 rounded">
                                        <option value="1">1层</option>
                                        <option value="2">2层</option>
                                        <option value="3">3层</option>
                                        <option value="5">5层</option>
                                    </select>
                                </label>
                            </div>
                        </div>

                        <!-- 扫描结果 -->
                        <div x-show="scanResult" class="mt-6">
                            <div class="bg-green-50 border border-green-200 rounded-md p-4">
                                <h3 class="text-lg font-medium text-green-800 mb-2">
                                    <i class="fas fa-check-circle mr-2"></i>扫描完成
                                </h3>
                                <div class="text-sm text-green-700">
                                    <p>扫描目录: <span x-text="scanResult?.root_directory"></span></p>
                                    <p>找到项目: <span x-text="scanResult?.projects?.length || 0"></span> 个</p>
                                    <p>扫描耗时: <span x-text="scanResult?.scan_duration_ms"></span> 毫秒</p>
                                    <p>扫描目录数: <span x-text="scanResult?.scanned_directories"></span> 个</p>
                                </div>
                            </div>
                        </div>

                        <!-- 错误信息 -->
                        <div x-show="scanResult?.errors?.length > 0" class="mt-4">
                            <div class="bg-yellow-50 border border-yellow-200 rounded-md p-4">
                                <h4 class="text-sm font-medium text-yellow-800 mb-2">扫描警告:</h4>
                                <ul class="text-sm text-yellow-700 space-y-1">
                                    <template x-for="error in scanResult.errors">
                                        <li x-text="error"></li>
                                    </template>
                                </ul>
                            </div>
                        </div>
                    </div>

                    <div class="flex justify-end mt-8">
                        <button @click="nextStep()"
                                :disabled="!scanResult || scanResult.projects.length === 0"
                                class="px-6 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700 disabled:bg-gray-400">
                            下一步 <i class="fas fa-arrow-right ml-2"></i>
                        </button>
                    </div>
                </div>"#.to_string()
}

/// 部署站点管理页面
pub fn render_deployment_sites_page() -> String {
    r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>部署站点管理 - AIOS 平台</title>
    <link href="/static/simple-tailwind.css" rel="stylesheet">
    <link href="/static/simple-icons.css" rel="stylesheet">
    <script src="/static/alpine.min.js" defer></script>
</head>
<body class="bg-gray-50">
    <div class="min-h-screen">
        <!-- 导航栏 -->
        <nav class="bg-blue-600 text-white shadow-lg">
            <div class="max-w-7xl mx-auto px-4">
                <div class="flex justify-between items-center py-4">
                    <div class="flex items-center space-x-4">
                        <i class="fas fa-server text-2xl"></i>
                        <h1 class="text-xl font-bold">部署站点管理</h1>
                    </div>
                    <div class="flex space-x-4">
                        <a href="/" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-home mr-2"></i>首页
                        </a>
                        <a href="/tasks" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tasks mr-2"></i>任务管理
                        </a>
                    </div>
                </div>
            </div>
        </nav>

        <!-- 主要内容 -->
        <div class="max-w-7xl mx-auto px-4 py-8" x-data="deploymentSitesApp()">
            <!-- 页面标题和操作按钮 -->
            <div class="flex justify-between items-center mb-6">
                <h2 class="text-2xl font-bold text-gray-800">部署站点管理</h2>
                <button @click="showCreateModal = true" 
                        class="bg-blue-600 hover:bg-blue-700 text-white px-4 py-2 rounded-md">
                    <i class="fas fa-plus mr-2"></i>新增部署站点
                </button>
            </div>

            <!-- 搜索和过滤 -->
            <div class="bg-white rounded-lg shadow-md p-4 mb-6">
                <div class="grid grid-cols-1 md:grid-cols-4 gap-4">
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">搜索</label>
                        <input type="text" x-model="searchQuery" @input="searchSites()"
                               placeholder="站点名称、负责人..." 
                               class="w-full px-3 py-2 border border-gray-300 rounded-md">
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">状态</label>
                        <select x-model="statusFilter" @change="filterSites()"
                                class="w-full px-3 py-2 border border-gray-300 rounded-md">
                            <option value="">全部状态</option>
                            <option value="Configuring">配置中</option>
                            <option value="Deploying">部署中</option>
                            <option value="Running">运行中</option>
                            <option value="Failed">失败</option>
                            <option value="Stopped">已停止</option>
                        </select>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">环境</label>
                        <select x-model="envFilter" @change="filterSites()"
                                class="w-full px-3 py-2 border border-gray-300 rounded-md">
                            <option value="">全部环境</option>
                            <option value="prod">生产环境</option>
                            <option value="staging">测试环境</option>
                            <option value="dev">开发环境</option>
                        </select>
                    </div>
                    <div class="flex items-end">
                        <button @click="refreshSites()" 
                                class="w-full bg-gray-600 hover:bg-gray-700 text-white px-4 py-2 rounded-md">
                            <i class="fas fa-refresh mr-2"></i>刷新
                        </button>
                    </div>
                </div>
            </div>

            <!-- 站点列表 -->
            <div class="bg-white rounded-lg shadow-md overflow-hidden">
                <div class="overflow-x-auto">
                    <table class="min-w-full divide-y divide-gray-200">
                        <thead class="bg-gray-50">
                            <tr>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">站点信息</th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">状态</th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">环境</th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">E3D项目</th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">负责人</th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">创建时间</th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">操作</th>
                            </tr>
                        </thead>
                        <tbody class="bg-white divide-y divide-gray-200">
                            <template x-for="site in sites" :key="site.id">
                                <tr class="hover:bg-gray-50">
                                    <td class="px-6 py-4 whitespace-nowrap">
                                        <div>
                                            <div class="text-sm font-medium text-gray-900" x-text="site.name"></div>
                                            <div class="text-sm text-gray-500" x-text="site.description || '无描述'"></div>
                                        </div>
                                    </td>
                                    <td class="px-6 py-4 whitespace-nowrap">
                                        <span class="inline-flex px-2 py-1 text-xs font-semibold rounded-full"
                                              :class="getStatusColor(site.status)" x-text="getStatusText(site.status)">
                                        </span>
                                    </td>
                                    <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900" x-text="site.env || '-'"></td>
                                    <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900" x-text="site.e3d_projects?.length || 0"></td>
                                    <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-900" x-text="site.owner || '-'"></td>
                                    <td class="px-6 py-4 whitespace-nowrap text-sm text-gray-500" x-text="formatDate(site.created_at)"></td>
                                    <td class="px-6 py-4 whitespace-nowrap text-sm font-medium space-x-2">
                                        <button @click="viewSiteDetail(site)" 
                                                class="text-blue-600 hover:text-blue-900">
                                            <i class="fas fa-eye"></i>
                                        </button>
                                        <button @click="editSite(site)" 
                                                class="text-yellow-600 hover:text-yellow-900">
                                            <i class="fas fa-edit"></i>
                                        </button>
                                        <button @click="createSiteTask(site)" 
                                                class="text-green-600 hover:text-green-900">
                                            <i class="fas fa-play"></i>
                                        </button>
                                        <button @click="deleteSite(site)" 
                                                class="text-red-600 hover:text-red-900">
                                            <i class="fas fa-trash"></i>
                                        </button>
                                    </td>
                                </tr>
                            </template>
                        </tbody>
                    </table>
                </div>

                <!-- 空状态 -->
                <div x-show="sites.length === 0" class="text-center py-12">
                    <i class="fas fa-server text-gray-400 text-6xl mb-4"></i>
                    <h3 class="text-lg font-medium text-gray-900 mb-2">暂无部署站点</h3>
                    <p class="text-gray-500 mb-4">点击上方按钮创建第一个部署站点</p>
                </div>
            </div>

            <!-- 分页 -->
            <div x-show="totalPages > 1" class="mt-6 flex justify-center">
                <nav class="flex space-x-2">
                    <button @click="changePage(currentPage - 1)" 
                            :disabled="currentPage <= 1"
                            class="px-3 py-2 border border-gray-300 rounded-md disabled:opacity-50">
                        上一页
                    </button>
                    <template x-for="page in pageNumbers" :key="page">
                        <button @click="changePage(page)" 
                                :class="page === currentPage ? 'bg-blue-600 text-white' : 'bg-white text-gray-700'"
                                class="px-3 py-2 border border-gray-300 rounded-md">
                            <span x-text="page"></span>
                        </button>
                    </template>
                    <button @click="changePage(currentPage + 1)" 
                            :disabled="currentPage >= totalPages"
                            class="px-3 py-2 border border-gray-300 rounded-md disabled:opacity-50">
                        下一页
                    </button>
                </nav>
            </div>

            <!-- 创建站点模态框 -->
            <div x-show="showCreateModal" x-cloak 
                 class="fixed inset-0 bg-gray-500 bg-opacity-75 flex items-center justify-center z-1000">
                <div class="bg-white rounded-lg p-6 w-full max-w-2xl max-h-screen overflow-y-auto z-1010">
                    <div class="flex justify-between items-center mb-4">
                        <h3 class="text-lg font-medium">创建部署站点</h3>
                        <button @click="showCreateModal = false" class="text-gray-400 hover:text-gray-600">
                            <i class="fas fa-times"></i>
                        </button>
                    </div>
                    
                    <form @submit.prevent="createSite()">
                        <div class="space-y-4">
                            <div>
                                <label class="block text-sm font-medium text-gray-700 mb-1">站点名称 *</label>
                                <input type="text" x-model="newSite.name" required
                                       class="w-full px-3 py-2 border border-gray-300 rounded-md">
                            </div>
                            
                            <div>
                                <label class="block text-sm font-medium text-gray-700 mb-1">站点描述</label>
                                <textarea x-model="newSite.description" rows="3"
                                          class="w-full px-3 py-2 border border-gray-300 rounded-md"></textarea>
                            </div>
                            
                            <div class="grid grid-cols-2 gap-4">
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">环境</label>
                                    <select x-model="newSite.env" class="w-full px-3 py-2 border border-gray-300 rounded-md">
                                        <option value="dev">开发环境</option>
                                        <option value="staging">测试环境</option>
                                        <option value="prod">生产环境</option>
                                    </select>
                                </div>
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">负责人</label>
                                    <input type="text" x-model="newSite.owner"
                                           class="w-full px-3 py-2 border border-gray-300 rounded-md">
                                </div>
                            </div>
                            
                            <div>
                                <label class="block text-sm font-medium text-gray-700 mb-1">E3D项目路径 (每行一个)</label>
                                <textarea x-model="newSite.selectedProjectsText" rows="4"
                                          placeholder="/path/to/project1&#10;/path/to/project2"
                                          class="w-full px-3 py-2 border border-gray-300 rounded-md"></textarea>
                            </div>
                            
                            <div>
                                <label class="block text-sm font-medium text-gray-700 mb-1">项目名称</label>
                                <input type="text" x-model="newSite.config.project_name" value="AvevaMarineSample"
                                       class="w-full px-3 py-2 border border-gray-300 rounded-md">
                            </div>
                            
                            <div class="grid grid-cols-2 gap-4">
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">数据库IP</label>
                                    <input type="text" x-model="newSite.config.db_ip" value="localhost"
                                           class="w-full px-3 py-2 border border-gray-300 rounded-md">
                                </div>
                                <div>
                                    <label class="block text-sm font-medium text-gray-700 mb-1">数据库端口</label>
                                    <input type="text" x-model="newSite.config.db_port" value="8009"
                                           class="w-full px-3 py-2 border border-gray-300 rounded-md">
                                </div>
                            </div>
                        </div>
                        
                        <div class="flex justify-end mt-6 space-x-3">
                            <button type="button" @click="showCreateModal = false"
                                    class="px-4 py-2 border border-gray-300 rounded-md text-gray-700 hover:bg-gray-50">
                                取消
                            </button>
                            <button type="submit" 
                                    class="px-4 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700">
                                创建站点
                            </button>
                        </div>
                    </form>
                </div>
            </div>

            <!-- 任务创建模态框 -->
            <div x-show="showTaskModal" x-cloak 
                 class="fixed inset-0 bg-gray-500 bg-opacity-75 flex items-center justify-center z-1000">
                <div class="bg-white rounded-lg p-6 w-full max-w-md z-1010">
                    <div class="flex justify-between items-center mb-4">
                        <h3 class="text-lg font-medium">为站点创建任务</h3>
                        <button @click="showTaskModal = false" class="text-gray-400 hover:text-gray-600">
                            <i class="fas fa-times"></i>
                        </button>
                    </div>
                    
                    <form @submit.prevent="submitCreateTask()">
                        <div class="space-y-4">
                            <div>
                                <label class="block text-sm font-medium text-gray-700 mb-1">任务类型</label>
                                <select x-model="taskRequest.task_type" class="w-full px-3 py-2 border border-gray-300 rounded-md">
                                    <option value="DataGeneration">数据生成</option>
                                    <option value="SpatialTreeGeneration">空间树生成</option>
                                    <option value="FullGeneration">完整生成</option>
                                    <option value="ParsePdmsData">解析PDMS数据</option>
                                </select>
                            </div>
                            
                            <div>
                                <label class="block text-sm font-medium text-gray-700 mb-1">任务优先级</label>
                                <select x-model="taskRequest.priority" class="w-full px-3 py-2 border border-gray-300 rounded-md">
                                    <option value="Low">低</option>
                                    <option value="Normal">普通</option>
                                    <option value="High">高</option>
                                    <option value="Urgent">紧急</option>
                                </select>
                            </div>
                        </div>
                        
                        <div class="flex justify-end mt-6 space-x-3">
                            <button type="button" @click="showTaskModal = false"
                                    class="px-4 py-2 border border-gray-300 rounded-md text-gray-700 hover:bg-gray-50">
                                取消
                            </button>
                            <button type="submit" 
                                    class="px-4 py-2 bg-green-600 text-white rounded-md hover:bg-green-700">
                                创建任务
                            </button>
                        </div>
                    </form>
                </div>
            </div>
        </div>
    </div>

    <!-- JavaScript -->
    <script src="/static/deployment-sites.js"></script>
</body>
</html>
"#.to_string()
}
