/// 简化的HTML模板渲染函数

pub fn render_simple_index_page() -> String {
    r##"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>AIOS 数据库管理平台</title>
    <link href="/static/simple-tailwind.css" rel="stylesheet">
    <link href="/static/simple-icons.css" rel="stylesheet">
    <style>
        /* 导航与头图 */
        .nav-gradient { background: linear-gradient(90deg, #2563eb 0%, #4f46e5 100%); }
        .hero { padding: 3.5rem 0 2rem; background:
            radial-gradient(1200px 600px at 50% -120px, rgba(59,130,246,.12), transparent),
            linear-gradient(180deg, #ffffff 0%, #f8fafc 100%);
        }
        .title { letter-spacing: .02em; }

        /* 卡片与图标 */
        /* 统一卡片视觉：无边框 + 阴影，避免线条叠加 */
        .feature-card { background:#fff; border:0; border-radius:12px; overflow:hidden;
            box-shadow: 0 6px 14px rgba(16,24,40,.06); transition: transform .2s ease, box-shadow .2s ease; }
        .feature-card:hover { transform: translateY(-4px); box-shadow: 0 14px 24px rgba(16,24,40,.12); }
        .status-card { background:#fff; border:0; border-radius:12px; overflow:hidden; }
        .filters-card { background:#fff; border:1px solid #eef2f7; border-radius:12px; overflow:hidden; }
        .icon-ring { width:64px; height:64px; border-radius:9999px; display:flex; align-items:center; justify-content:center; box-shadow: inset 0 0 0 6px rgba(37,99,235,.08); background:#eff6ff; }
        .icon-ring.green { background:#ecfdf5; box-shadow: inset 0 0 0 6px rgba(16,185,129,.08); }
        .icon-ring.purple { background:#f5f3ff; box-shadow: inset 0 0 0 6px rgba(168,85,247,.08); }

        /* 软提示 Badge */
        .badge-soft { display:inline-flex; align-items:center; gap:.4rem; padding:.5rem .75rem; border-radius:.5rem;
            background:#d1fae5; color:#065f46; border:1px solid #6ee7b7; }
    </style>
</head>
<body class="bg-gray-50">
    <div class="min-h-screen">
        <!-- 导航栏 -->
        <nav class="nav-gradient text-white shadow-lg">
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
                        <a href="/config" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-cog mr-2"></i>配置管理
                        </a>
                        <a href="/db-status" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-database mr-2"></i>系统状态
                        </a>
                    </div>
                </div>
            </div>
        </nav>

        <!-- 主要内容 -->
        <main class="max-w-7xl mx-auto py-6 px-4">
            <!-- 欢迎区域 / 头图 -->
            <section class="hero text-center mb-10 rounded">
                <h1 class="text-4xl font-bold text-gray-900 mb-3 title">
                    <i class="fas fa-database text-blue-600 mr-3"></i>
                    AIOS 数据库管理平台
                </h1>
                <p class="text-xl text-gray-600 mb-6">专业的数据库生成和空间树管理系统</p>
                <div class="max-w-2xl mx-auto">
                    <div class="badge-soft mx-auto">
                        <i class="fas fa-check-circle"></i>
                        系统运行正常 - 简单 Web UI 已成功启动
                    </div>
                </div>
            </section>

            <!-- 功能卡片 -->
            <div class="grid md:grid-cols-3 gap-8 mb-12">
                <!-- 数据生成卡片 -->
                <div class="feature-card p-6">
                    <div class="text-center">
                        <div class="icon-ring mx-auto mb-4">
                            <i class="fas fa-database text-2xl text-blue-600"></i>
                        </div>
                        <h3 class="text-xl font-semibold text-gray-900 mb-2">数据库生成</h3>
                        <p class="text-gray-600 mb-4">生成和管理数据库编号7999的数据</p>
                        <button onclick="createQuickTask(7999)" class="bg-blue-600 text-white px-6 py-2 rounded hover:bg-blue-700 transition">
                            立即执行
                        </button>
                    </div>
                </div>

                <!-- 空间树生成卡片 -->
                <div class="feature-card p-6">
                    <div class="text-center">
                        <div class="icon-ring green mx-auto mb-4">
                            <i class="fas fa-sitemap text-2xl text-green-600"></i>
                        </div>
                        <h3 class="text-xl font-semibold text-gray-900 mb-2">空间树生成</h3>
                        <p class="text-gray-600 mb-4">构建和优化空间关系树结构</p>
                        <a href="/tasks" class="bg-green-600 text-white px-6 py-2 rounded hover:bg-green-700 transition inline-block">
                            查看任务
                        </a>
                    </div>
                </div>

                <!-- 配置管理卡片 -->
                <div class="feature-card p-6">
                    <div class="text-center">
                        <div class="icon-ring purple mx-auto mb-4">
                            <i class="fas fa-cog text-2xl text-purple-600"></i>
                        </div>
                        <h3 class="text-xl font-semibold text-gray-900 mb-2">配置管理</h3>
                        <p class="text-gray-600 mb-4">管理系统配置和参数设置</p>
                        <a href="/config" class="bg-purple-600 text-white px-6 py-2 rounded hover:bg-purple-700 transition inline-block">
                            配置设置
                        </a>
                    </div>
                </div>
            </div>

            <!-- 系统状态 -->
            <div class="status-card p-6">
                <h2 class="text-2xl font-semibold text-gray-900 mb-4">
                    <i class="fas fa-chart-line text-blue-600 mr-2"></i>
                    系统状态
                </h2>
                <div class="grid md:grid-cols-4 gap-4">
                    <div class="text-center">
                        <div class="text-2xl font-bold text-blue-600">运行中</div>
                        <div class="text-gray-600">系统状态</div>
                    </div>
                    <div class="text-center">
                        <div class="text-2xl font-bold text-green-600">正常</div>
                        <div class="text-gray-600">数据库连接</div>
                    </div>
                    <div class="text-center">
                        <div class="text-2xl font-bold text-purple-600">0</div>
                        <div class="text-gray-600">活跃任务</div>
                    </div>
                    <div class="text-center">
                        <div class="text-2xl font-bold text-orange-600">待定</div>
                        <div class="text-gray-600">队列任务</div>
                    </div>
                </div>
            </div>
            <!-- 部署站点 -->
            <div class="mt-12">
                <div class="flex items-center justify-between mb-4">
                    <h2 class="text-2xl font-semibold text-gray-900">
                        <i class="fas fa-folder-open text-blue-600 mr-2"></i>
                        部署站点
                    </h2>
                    <div class="flex gap-3">
                        <a href="/deployment-sites" class="px-3 py-2 rounded bg-gray-200 text-gray-900 hover:bg-gray-300">查看全部</a>
                        <button onclick="reloadProjects()" class="px-3 py-2 rounded bg-blue-600 text-white hover:bg-blue-700">刷新</button>
                        <button onclick="window.location.href='/wizard'" class="px-3 py-2 rounded bg-green-600 text-white hover:bg-green-700">+ 创建站点</button>
                    </div>
                </div>
                <!-- 筛选栏 -->
                <div class="filters-card mb-4">
                    <div class="grid gap-3 md:grid-cols-4">
                    <div>
                        <input id="site_q" placeholder="搜索名称/描述/负责人" class="w-full border rounded px-3 py-2 text-sm" />
                    </div>
                    <div>
                        <select id="site_status" class="w-full border rounded px-3 py-2 text-sm">
                            <option value="">全部状态</option>
                            <option>Configuring</option>
                            <option>Deploying</option>
                            <option>Running</option>
                            <option>Failed</option>
                            <option>Stopped</option>
                        </select>
                    </div>
                    <div>
                        <select id="site_env" class="w-full border rounded px-3 py-2 text-sm">
                            <option value="">全部环境</option>
                            <option>dev</option>
                            <option>staging</option>
                            <option>prod</option>
                            <option>test</option>
                        </select>
                    </div>
                    <div>
                        <input id="site_owner" placeholder="负责人" class="w-full border rounded px-3 py-2 text-sm" />
                    </div>
                    <div class="md:col-span-4">
                        <div class="flex items-center gap-3">
                            <label class="text-sm text-gray-600">排序</label>
                            <select id="site_sort" class="border rounded px-3 py-2 text-sm">
                                <option value="updated_at:desc">最近更新</option>
                                <option value="name:asc">名称 (A→Z)</option>
                                <option value="name:desc">名称 (Z→A)</option>
                                <option value="created_at:asc">创建时间 (旧→新)</option>
                                <option value="created_at:desc">创建时间 (新→旧)</option>
                            </select>
                        </div>
                    </div>
                    </div>
                </div>
                <div id="projects-grid" data-per-page="6" class="grid gap-6 sm:grid-cols-2 lg:grid-cols-3"></div>
                <div id="sites-pager" class="mt-4 flex items-center justify-between text-sm text-gray-600"></div>
            </div>

            <!-- 详情弹窗 Modal -->
            <div id="project-modal" class="fixed inset-0 z-1000 hidden" aria-hidden="true">
              <div class="absolute inset-0 bg-black/50" onclick="closeProjectModal()"></div>
              <div class="relative max-w-3xl mx-auto mt-16 bg-white rounded-lg shadow-lg flex flex-col z-1010" style="max-height: 85vh;">
                <!-- 标题栏 -->
                <div class="flex items-center justify-between p-6 pb-4 border-b">
                  <h3 id="pm-title" class="text-xl font-semibold">部署站点详情</h3>
                  <button class="text-gray-400 hover:text-gray-600 p-1 rounded-lg hover:bg-gray-100 transition-colors" onclick="closeProjectModal()">
                    <i class="fas fa-times text-xl"></i>
                  </button>
                </div>

                <!-- 内容区域（可滚动） -->
                <div class="flex-1 overflow-y-auto px-6 py-4" style="max-height: calc(85vh - 160px);">
                  <div class="text-sm text-gray-600 flex items-center gap-3">
                    <span id="pm-status" class="inline-flex items-center px-2 py-0.5 rounded text-xs bg-gray-100 text-gray-700">状态</span>
                    <span id="pm-env" class="inline-flex items-center px-2 py-0.5 rounded text-xs bg-gray-100 text-gray-700">环境</span>
                  </div>
                  <div id="pm-hc-status" class="hidden mt-3 text-xs"></div>
                  <div id="pm-error" class="hidden mt-3 p-3 rounded bg-red-50 text-red-700 text-sm">
                    加载失败，请稍后重试。按 Enter 键可重试。
                    <div class="mt-2"><button class="px-3 py-1 rounded bg-red-600 text-white" onclick="retryLoadProjectDetail()">重试</button></div>
                  </div>
                  <div id="pm-content" class="mt-4 text-sm text-gray-700">正在加载...</div>
                </div>

                <!-- 底部按钮栏 -->
                <div class="border-t px-6 py-4 flex gap-3 justify-end bg-gray-50">
                  <button id="pm-copy" class="px-3 py-2 rounded bg-gray-100 hover:bg-gray-200 transition-colors" onclick="copySiteConfig()">复制配置</button>
                  <button id="pm-create-task" class="px-3 py-2 rounded bg-green-600 text-white hover:bg-green-700 transition-colors" onclick="createSiteTask()">为站点创建任务</button>
                  <a id="pm-open-url" href="#" target="_blank" class="px-3 py-2 rounded bg-blue-600 text-white hover:bg-blue-700 transition-colors hidden">打开地址</a>
                  <button id="pm-health" class="px-3 py-2 rounded bg-green-600 text-white hover:bg-green-700 transition-colors hidden" onclick="pmHealthCheck()">健康检查</button>
                  <button id="pm-restart-db" class="px-3 py-2 rounded bg-purple-600 text-white hover:bg-purple-700 transition-colors hidden" onclick="pmRestartDatabase()">重启数据库</button>
                  <button class="px-3 py-2 rounded bg-gray-200 hover:bg-gray-300 transition-colors" onclick="closeProjectModal()">关闭</button>
                </div>
              </div>
            </div>
        </main>
    </div>

    <script>
        // 密码可见性切换功能
        function togglePasswordVisibility(inputId, button) {
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
        }

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
    </script>
    <script src="/static/projects.js"></script>
</body>
</html>
    "##.to_string()
}

pub fn render_xtk_viewer_page() -> String {
    r##"<!DOCTYPE html>
<html lang=\"zh-CN\">
<head>
    <meta charset=\"UTF-8\">
    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">
    <title>XKT 预览 - AIOS</title>
    <link rel=\"stylesheet\" href=\"/static/simple-tailwind.css\">
    <link rel=\"stylesheet\" href=\"/static/ui.css\">
    <style>
        body { background:#0f172a; color:#e2e8f0; }
        .toolbar { background:rgba(15,23,42,.85); border-bottom:1px solid rgba(148,163,184,.2); backdrop-filter: blur(8px); }
        #viewerCanvas { width:100%; height:calc(100vh - 180px); display:block; border-radius:12px; background:#0b1120; }
        .panel { max-width:1040px; margin:0 auto; }
        .btn-primary { background:#2563eb; color:#fff; transition:all .2s ease; }
        .btn-primary:hover { background:#1d4ed8; transform:translateY(-1px); }
        .btn-secondary { background:rgba(148,163,184,.18); color:#e2e8f0; transition:all .2s ease; }
        .btn-secondary:hover { background:rgba(148,163,184,.3); transform:translateY(-1px); }
        .status-chip { display:inline-flex; align-items:center; gap:.4rem; padding:.4rem .75rem; border-radius:9999px; background:rgba(37,99,235,.15); color:#bfdbfe; font-size:.85rem; }
        label { color:#cbd5f5; font-weight:600; }
        input { color:#0f172a; }
    </style>
    <script src=\"https://cdn.jsdelivr.net/npm/@xeokit/xeokit-sdk/dist/xeokit-sdk.min.js\"></script>
</head>
<body>
    <header class=\"toolbar shadow-md\">
        <div class=\"panel px-6 py-4 flex flex-col gap-3 md:gap-0 md:flex-row md:items-center md:justify-between\">
            <div>
                <h1 class=\"text-2xl font-semibold text-blue-200\">XKT 模型预览</h1>
                <p class=\"text-sm text-slate-300 mt-1\">支持直接选择本地 XKT 文件或输入可访问的 URL</p>
            </div>
            <div class=\"status-chip\">
                <span class=\"inline-block w-2 h-2 rounded-full bg-green-400\"></span>
                xeokit SDK 在线加载
            </div>
        </div>
    </header>

    <main class=\"panel px-6 py-6 space-y-6\">
        <section class=\"bg-slate-900/60 border border-slate-700/60 rounded-xl p-5 space-y-4\">
            <div class=\"grid gap-4 md:grid-cols-2\">
                <div>
                    <label class=\"block mb-2\">从本地文件加载</label>
                    <input id=\"fileInput\" type=\"file\" accept=\".xkt\" class=\"w-full px-3 py-2 rounded bg-slate-800/70 border border-slate-600/70 text-slate-200\">
                    <p class=\"text-sm text-slate-400 mt-2\">选择本地 XKT 文件后会立即加载。</p>
                </div>
                <div>
                    <label class=\"block mb-2\">从 URL 加载</label>
                    <div class=\"flex gap-3\">
                        <input id=\"xktUrl\" type=\"text\" placeholder=\"例如 /static/models/sample.xkt\" class=\"flex-1 px-3 py-2 rounded bg-slate-800/70 border border-slate-600/70 text-slate-200\">
                        <button id=\"loadFromUrl\" class=\"btn-primary px-5 py-2 rounded\">加载</button>
                    </div>
                    <p class=\"text-sm text-slate-400 mt-2\">确保文件可通过浏览器访问，例如放在 /static 目录或开启文件服务。</p>
                </div>
            </div>
            <div class=\"text-sm text-slate-300\">
                当前模型：<span id=\"modelStatus\" class=\"text-blue-200\">未加载</span>
            </div>
        </section>

        <section class=\"bg-slate-900/60 border border-slate-700/60 rounded-xl p-5 space-y-4\">
            <div class=\"flex flex-col gap-3 md:flex-row md:items-center md:justify-between\">
                <div>
                    <h2 class=\"text-xl font-semibold text-slate-100\">手动生成 XKT</h2>
                    <p class=\"text-sm text-slate-400\">输入数据库号，选填参考号（支持单个元素导出），即可生成 XKT 文件。</p>
                </div>
                <div class=\"flex items-center gap-3\">
                    <label class=\"flex items-center gap-2 text-sm text-slate-300\">
                        <input id=\"manualCompress\" type=\"checkbox\" class=\"accent-blue-500\" checked>
                        启用压缩
                    </label>
                    <label class=\"flex items-center gap-2 text-sm text-slate-300\">
                        <input id=\"manualAutoload\" type=\"checkbox\" class=\"accent-blue-500\" checked>
                        生成后自动加载
                    </label>
                </div>
            </div>
            <div class=\"grid gap-4 md:grid-cols-3\">
                <div>
                    <label class=\"block mb-2\" for=\"manualDbno\">数据库号</label>
                    <input id=\"manualDbno\" type=\"number\" min=\"1\" placeholder=\"例如 7999\" class=\"w-full px-3 py-2 rounded bg-slate-800/70 border border-slate-600/70 text-slate-200\">
                </div>
                <div class=\"md:col-span-2\">
                    <label class=\"block mb-2\" for=\"manualRefno\">参考号（可选）</label>
                    <input id=\"manualRefno\" type=\"text\" placeholder=\"例如 SITE/1234\" class=\"w-full px-3 py-2 rounded bg-slate-800/70 border border-slate-600/70 text-slate-200\">
                </div>
            </div>
            <div class=\"flex flex-col gap-2 md:flex-row md:items-center md:gap-4\">
                <button id=\"manualGenerate\" class=\"btn-primary px-5 py-2 rounded w-full md:w-auto\">生成 XKT</button>
                <span id=\"manualGenerateStatus\" class=\"text-sm text-slate-400\"></span>
            </div>
        </section>

        <section class=\"bg-slate-900/60 border border-slate-700/60 rounded-xl p-5 space-y-4\">
            <div class=\"flex flex-col gap-4 md:flex-row md:items-center md:justify-between\">
                <div>
                    <label class=\"block text-lg font-semibold text-slate-100\">数据库列表</label>
                    <p class=\"text-sm text-slate-400 mt-1\">可选择一个或多个数据库号批量生成 XKT，支持全选。</p>
                </div>
                <div class=\"flex flex-wrap gap-3\">
                    <button id=\"refreshDbList\" class=\"btn-secondary px-4 py-2 rounded\">刷新列表</button>
                    <button id=\"generateSelected\" class=\"btn-primary px-4 py-2 rounded\">生成选中</button>
                    <button id=\"generateLoadSelected\" class=\"btn-primary px-4 py-2 rounded\">生成并加载选中</button>
                </div>
            </div>
            <div class=\"bg-slate-950/60 border border-slate-800/60 rounded-lg overflow-hidden\">
                <table class=\"min-w-full text-sm text-slate-200\">
                    <thead class=\"bg-slate-800/70 text-slate-300\">
                        <tr>
                            <th class=\"px-3 py-2 text-left w-12\">
                                <input type=\"checkbox\" id=\"selectAllDb\" class=\"accent-blue-500\">
                            </th>
                            <th class=\"px-3 py-2 text-left\">数据库号</th>
                            <th class=\"px-3 py-2 text-left\">名称</th>
                            <th class=\"px-3 py-2 text-left\">记录数</th>
                            <th class=\"px-3 py-2 text-left\">可用状态</th>
                            <th class=\"px-3 py-2 text-left\">最近更新</th>
                        </tr>
                    </thead>
                    <tbody id=\"dbListBody\" class=\"divide-y divide-slate-800/60\"></tbody>
                </table>
                <div id=\"dbListEmpty\" class=\"p-4 text-sm text-slate-400 hidden\">暂无数据库记录，请点击刷新按钮。</div>
            </div>
            <div id=\"generationLog\" class=\"bg-slate-950/50 border border-slate-800/60 rounded-lg p-3 h-40 overflow-y-auto text-xs text-slate-300 space-y-1\"></div>
        </section>

        <section>
            <canvas id=\"viewerCanvas\"></canvas>
        </section>
    </main>

    <script>
        (function() {
            if (!window.xeokit) {
                document.getElementById('modelStatus').innerText = 'xeokit SDK 未加载';
                return;
            }

            const viewer = new xeokit.Viewer({
                canvasId: 'viewerCanvas',
                transparent: true,
                xrayPickable: true
            });

            new xeokit.CameraControl(viewer, {
                doublePickFlyTo: true
            });

            viewer.camera.eye = [120, 60, 120];
            viewer.camera.look = [0, 0, 0];
            viewer.camera.up = [0, 1, 0];

            const xktLoader = new xeokit.XKTLoaderPlugin(viewer, {
                edges: true
            });

            const dbListBody = document.getElementById('dbListBody');
            const dbListEmpty = document.getElementById('dbListEmpty');
            const selectAllDb = document.getElementById('selectAllDb');
            const refreshDbListBtn = document.getElementById('refreshDbList');
            const generateSelectedBtn = document.getElementById('generateSelected');
            const generateLoadSelectedBtn = document.getElementById('generateLoadSelected');
            const generationLog = document.getElementById('generationLog');
            const manualDbnoInput = document.getElementById('manualDbno');
            const manualRefnoInput = document.getElementById('manualRefno');
            const manualCompressInput = document.getElementById('manualCompress');
            const manualAutoloadInput = document.getElementById('manualAutoload');
            const manualGenerateBtn = document.getElementById('manualGenerate');
            const manualGenerateStatus = document.getElementById('manualGenerateStatus');

            let dbList = [];
            const selectedDbnos = new Set();
            let isGenerating = false;
            let modelCounter = 0;
            let loadedModels = [];

            function appendLog(message, type = 'info') {
                if (!generationLog) { return; }
                const entry = document.createElement('div');
                const time = new Date().toLocaleTimeString();
                const colorClass = type === 'error'
                    ? 'text-rose-400'
                    : type === 'success'
                        ? 'text-emerald-300'
                        : 'text-slate-300';
                entry.className = colorClass;
                entry.textContent = `[${time}] ${message}`;
                generationLog.prepend(entry);
                if (generationLog.children.length > 200) {
                    generationLog.removeChild(generationLog.lastElementChild);
                }
            }

            function syncSelectAllState() {
                if (!selectAllDb) { return; }
                selectAllDb.checked = dbList.length > 0 && selectedDbnos.size === dbList.length;
                selectAllDb.indeterminate = selectedDbnos.size > 0 && selectedDbnos.size < dbList.length;
            }

            function renderDbList() {
                if (!dbListBody) { return; }
                dbListBody.innerHTML = '';
                if (!dbList.length) {
                    dbListEmpty?.classList.remove('hidden');
                    return;
                }
                dbListEmpty?.classList.add('hidden');
                dbList.forEach((item) => {
                    const row = document.createElement('tr');
                    row.className = 'hover:bg-slate-800/40';
                    const updatedText = (() => {
                        if (!item.last_updated) { return '-'; }
                        const t = new Date(item.last_updated);
                        return isNaN(t.getTime()) ? '-' : t.toLocaleString();
                    })();
                    const checked = selectedDbnos.has(item.dbnum) ? 'checked' : '';
                    row.innerHTML = `
                        <td class="px-3 py-2">
                            <input type="checkbox" class="accent-blue-500 db-checkbox" data-dbno="${item.dbnum}" ${checked}>
                        </td>
                        <td class="px-3 py-2 font-semibold text-blue-200">${item.dbnum}</td>
                        <td class="px-3 py-2">${item.name || '-'}</td>
                        <td class="px-3 py-2 text-right">${item.record_count ?? '-'}</td>
                        <td class="px-3 py-2">
                            <span class="px-2 py-1 rounded-full text-xs ${item.available ? 'bg-emerald-500/20 text-emerald-300' : 'bg-rose-500/20 text-rose-300'}">${item.available ? '可用' : '不可用'}</span>
                        </td>
                        <td class="px-3 py-2 text-slate-400">${updatedText}</td>
                    `;
                    const checkbox = row.querySelector('.db-checkbox');
                    checkbox?.addEventListener('change', (event) => {
                        const value = Number(event.target.dataset.dbno);
                        if (event.target.checked) {
                            selectedDbnos.add(value);
                        } else {
                            selectedDbnos.delete(value);
                        }
                        syncSelectAllState();
                    });
                    dbListBody.appendChild(row);
                });
                syncSelectAllState();
            }

            async function loadDbList() {
                try {
                    const response = await fetch('/api/databases');
                    if (!response.ok) {
                        throw new Error(`HTTP ${response.status}`);
                    }
                    const data = await response.json();
                    selectedDbnos.forEach((db) => {
                        if (!data.find((item) => item.dbnum === db)) {
                            selectedDbnos.delete(db);
                        }
                    });
                    dbList = data;
                    renderDbList();
                    appendLog(`数据库列表已更新（共 ${dbList.length} 个）`);
                } catch (error) {
                    console.error('加载数据库列表失败', error);
                    appendLog(`加载数据库列表失败: ${error.message}`, 'error');
                    dbList = [];
                    renderDbList();
                }
            }

            function getSelectedDbnos() {
                return Array.from(selectedDbnos).sort((a, b) => a - b);
            }

            function setButtonsDisabled(disabled) {
                [generateSelectedBtn, generateLoadSelectedBtn, refreshDbListBtn].forEach((btn) => {
                    if (!btn) { return; }
                    btn.disabled = disabled;
                    btn.classList.toggle('opacity-60', disabled);
                    btn.classList.toggle('cursor-not-allowed', disabled);
                });
            }

            async function loadModel(sourceUrl, label, options = {}) {
                const { preserve = false } = options;
                try {
                    if (!preserve) {
                        loadedModels.forEach((model) => {
                            xktLoader.destroyModel(model.id);
                        });
                        loadedModels = [];
                        viewer.scene.clear(true, true);
                    }

                    const modelId = `model_${++modelCounter}`;
                    document.getElementById('modelStatus').innerText = `加载中: ${label}`;

                    await xktLoader.load({
                        id: modelId,
                        src: sourceUrl,
                    });

                    viewer.cameraFlight.flyTo(modelId, { duration: 1.0 });
                    loadedModels.push({ id: modelId, label, sourceUrl });

                    if (sourceUrl.startsWith('blob:')) {
                        setTimeout(() => URL.revokeObjectURL(sourceUrl), 2000);
                    }

                    const statusText = loadedModels.length === 1
                        ? `已加载: ${label}`
                        : `已加载 ${loadedModels.length} 个模型（最新: ${label}）`;
                    document.getElementById('modelStatus').innerText = statusText;
                    appendLog(`加载模型成功: ${label}`, 'success');
                } catch (error) {
                    console.error('加载 XKT 失败', error);
                    document.getElementById('modelStatus').innerText = '加载失败';
                    appendLog(`加载模型失败: ${label}`, 'error');
                    throw error;
                }
            }

            async function generateXkt({ dbno, refno = null, compress = true }) {
                const targetLabel = refno ? `数据库 ${dbno} / 参考号 ${refno}` : `数据库 ${dbno}`;
                appendLog(`开始生成 ${targetLabel} 的 XKT...`);

                const payload = { dbno, compress };
                if (refno) {
                    payload.refno = refno;
                }

                const response = await fetch('/api/xkt/generate', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify(payload),
                });

                let data = {};
                try {
                    data = await response.json();
                } catch (_) {
                    // ignore json error for non-JSON responses
                }

                if (!response.ok || !data.success) {
                    const message = data.error || `生成失败 (HTTP ${response.status})`;
                    appendLog(`${targetLabel} 生成失败: ${message}`, 'error');
                    throw new Error(message);
                }

                appendLog(`${targetLabel} 生成完成: ${data.filename}`, 'success');
                return data;
            }

            async function generateSelected(autoLoad) {
                if (isGenerating) { return; }
                const dbnos = getSelectedDbnos();
                if (!dbnos.length) {
                    alert('请至少选择一个数据库编号');
                    return;
                }

                isGenerating = true;
                setButtonsDisabled(true);
                appendLog(`开始生成 ${dbnos.length} 个数据库的 XKT...`);

                try {
                    let firstModel = true;
                    for (const dbno of dbnos) {
                        const result = await generateXkt({ dbno });
                        if (autoLoad) {
                            await loadModel(result.url, `db${dbno}`, { preserve: !firstModel });
                            firstModel = false;
                        }
                    }
                    appendLog(autoLoad
                        ? `已生成并加载 ${dbnos.length} 个数据库`
                        : `已生成 ${dbnos.length} 个数据库`, 'success');
                } catch (error) {
                    console.error('生成 XKT 失败', error);
                    appendLog(`生成过程中出现错误: ${error.message}`, 'error');
                    alert('生成过程中出现错误，请检查控制台输出。');
                } finally {
                    setButtonsDisabled(false);
                    isGenerating = false;
                }
            }

            selectAllDb?.addEventListener('change', (event) => {
                selectedDbnos.clear();
                if (event.target.checked) {
                    dbList.forEach((item) => selectedDbnos.add(item.dbnum));
                }
                renderDbList();
            });

            refreshDbListBtn?.addEventListener('click', loadDbList);
            generateSelectedBtn?.addEventListener('click', () => generateSelected(false));
            generateLoadSelectedBtn?.addEventListener('click', () => generateSelected(true));

            manualGenerateBtn?.addEventListener('click', async () => {
                if (!manualDbnoInput) { return; }

                const dbnoValue = manualDbnoInput.value.trim();
                if (!dbnoValue) {
                    alert('请填写数据库号');
                    manualDbnoInput.focus();
                    return;
                }

                const dbno = Number(dbnoValue);
                if (!Number.isFinite(dbno) || dbno <= 0) {
                    alert('数据库号格式不正确');
                    manualDbnoInput.focus();
                    return;
                }

                const refno = manualRefnoInput?.value.trim() || null;
                const compress = manualCompressInput?.checked ?? true;
                const autoLoad = manualAutoloadInput?.checked ?? false;

                manualGenerateBtn.disabled = true;
                manualGenerateBtn.classList.add('opacity-60', 'cursor-not-allowed');
                if (manualGenerateStatus) {
                    manualGenerateStatus.textContent = '正在生成，请稍候...';
                    manualGenerateStatus.className = 'text-sm text-slate-300';
                }

                try {
                    const result = await generateXkt({ dbno, refno, compress });
                    if (manualGenerateStatus) {
                        manualGenerateStatus.textContent = `生成完成: ${result.filename}`;
                        manualGenerateStatus.className = 'text-sm text-emerald-300';
                    }

                    if (autoLoad && result?.url) {
                        const label = refno ? `${dbno}-${refno}` : `db${dbno}`;
                        await loadModel(result.url, label, { preserve: false });
                    }
                } catch (error) {
                    console.error('手动生成 XKT 失败', error);
                    if (manualGenerateStatus) {
                        manualGenerateStatus.textContent = `生成失败: ${error.message}`;
                        manualGenerateStatus.className = 'text-sm text-rose-400';
                    }
                } finally {
                    manualGenerateBtn.disabled = false;
                    manualGenerateBtn.classList.remove('opacity-60', 'cursor-not-allowed');
                }
            });

            document.getElementById('fileInput').addEventListener('change', (event) => {
                const file = event.target.files[0];
                if (!file) { return; }
                const objectUrl = URL.createObjectURL(file);
                loadModel(objectUrl, file.name, { preserve: false });
            });

            document.getElementById('loadFromUrl').addEventListener('click', () => {
                const url = document.getElementById('xktUrl').value.trim();
                loadModel(url, url || '未命名路径', { preserve: false });
            });

            loadDbList();
        })();
    </script>
</body>
</html>"##
        .to_string()
}

/// 新版首页（统一布局 + 侧栏）
pub fn render_index_with_sidebar() -> String {
    let content = r#"
        <!-- 欢迎区域 / 头图 -->
        <section class="hero text-center mb-10 rounded">
            <h1 class="text-4xl font-bold text-gray-900 mb-3">
                <i class="fas fa-database text-blue-600 mr-3"></i>
                AIOS 数据库管理平台
            </h1>
            <p class="text-xl text-gray-600 mb-6">专业的数据库生成和空间树管理系统</p>
            <div class="max-w-2xl mx-auto">
                <div class="badge-soft mx-auto">
                    <i class="fas fa-check-circle"></i>
                    系统运行正常 - 简单 Web UI 已成功启动
                </div>
            </div>
        </section>

        <!-- 首页优先展示：部署站点（缩略） -->
        <section class="mb-12">
            <div class="flex flex-col md:flex-row md:items-center md:justify-between gap-4 mb-6">
                <div>
                    <h2 class="section-title">
                        <span class="section-icon bg-blue-100 text-blue-600">
                            <i class="fas fa-server"></i>
                        </span>
                        部署站点
                    </h2>
                </div>
                <div class="flex flex-wrap gap-3">
                    <a id="home-view-all" href="/deployment-sites" class="btn btn--ghost">
                        <i class="fas fa-layer-group mr-2"></i>查看全部
                    </a>
                    <button type="button" onclick="reloadProjects()" class="btn btn--secondary">
                        <i class="fas fa-sync-alt mr-2"></i>刷新
                    </button>
                    <button type="button" onclick="window.location.href='/wizard'" class="btn btn--success">
                        <i class="fas fa-plus mr-2"></i>创建站点
                    </button>
                </div>
            </div>

            <!-- 统计与搜索合并栏 -->
            <div class="bg-white rounded-lg border border-gray-200 px-3 py-2 mb-4">
                <div class="flex items-center justify-between gap-3">
                    <!-- 左侧：统计信息 -->
                    <div class="flex items-center gap-3 text-xs text-gray-600">
                        <span class="font-medium text-gray-900">站点: <span id="stat-total" class="text-sm">1</span></span>
                        <span class="border-l pl-3">✅ <span id="stat-running">0</span></span>
                        <span>🚀 <span id="stat-deploying">0</span></span>
                        <span>❌ <span id="stat-failed">0</span></span>
                    </div>
                    <!-- 右侧：搜索和筛选 -->
                    <div class="flex gap-2">
                        <input id="site_q" class="px-2 py-1 border border-gray-300 rounded text-sm focus:outline-none focus:ring-1 focus:ring-blue-500" placeholder="搜索..." style="width:150px" />
                        <select id="site_status" class="px-2 py-1 border border-gray-300 rounded text-sm focus:outline-none focus:ring-1 focus:ring-blue-500">
                            <option value="">全部</option>
                            <option>Running</option>
                            <option>Failed</option>
                        </select>
                    </div>
                </div>
            </div>

            <div id="projects-grid" data-per-page="6" class="grid-cards grid-cards-lg home-projects-grid"></div>
            <div id="sites-pager" class="mt-4 flex flex-wrap items-center justify-between gap-3 text-sm text-gray-600"></div>
        </section>

            <!-- 详情弹窗 Modal -->
            <div id="project-modal" class="fixed inset-0 z-1000 hidden" aria-hidden="true">
              <div class="absolute inset-0 bg-black/50" onclick="closeProjectModal()"></div>
              <div class="relative max-w-3xl mx-auto mt-16 bg-white rounded-lg shadow-lg flex flex-col z-1010" style="max-height: 85vh;">
                <!-- 标题栏 -->
                <div class="flex items-center justify-between p-6 pb-4 border-b flex-shrink-0">
                  <h3 id="pm-title" class="text-xl font-semibold">部署站点详情</h3>
                  <button class="text-gray-400 hover:text-gray-600 p-1 rounded-lg hover:bg-gray-100 transition-colors" onclick="closeProjectModal()">
                    <i class="fas fa-times text-xl"></i>
                  </button>
                </div>

                <!-- 内容区域（可滚动） -->
                <div class="flex-1 overflow-y-auto px-6 py-4" style="max-height: calc(85vh - 160px);">
                  <div class="text-sm text-gray-600 flex items-center gap-3">
                    <span id="pm-status" class="inline-flex items-center px-2 py-0.5 rounded text-xs bg-gray-100 text-gray-700">状态</span>
                    <span id="pm-env" class="inline-flex items-center px-2 py-0.5 rounded text-xs bg-gray-100 text-gray-700">环境</span>
                  </div>
                  <div id="pm-hc-status" class="hidden mt-3 text-xs"></div>
                  <div id="pm-error" class="hidden mt-3 p-3 rounded bg-red-50 text-red-700 text-sm">
                    加载失败，请稍后重试。按 Enter 键可重试。
                    <div class="mt-2"><button class="px-3 py-1 rounded bg-red-600 text-white" onclick="retryLoadProjectDetail()">重试</button></div>
                  </div>
                  <div id="pm-content" class="mt-4 text-sm text-gray-700">正在加载...</div>
                </div>

                <!-- 底部按钮栏 -->
                <div class="border-t px-6 py-4 flex gap-3 justify-end bg-gray-50 flex-shrink-0">
                  <button id="pm-copy" class="px-3 py-2 rounded bg-gray-100" onclick="copySiteConfig()">复制配置</button>
                  <button id="pm-create-task" class="px-3 py-2 rounded bg-green-600 text-white" onclick="createSiteTask()">为站点创建任务</button>
                  <a id="pm-open-url" href="javascript:;" target="_blank" class="px-3 py-2 rounded bg-blue-600 text-white hidden">打开地址</a>
                  <button id="pm-health" class="px-3 py-2 rounded bg-green-600 text-white hidden" onclick="pmHealthCheck()">健康检查</button>
                  <button id="pm-restart-db" class="px-3 py-2 rounded bg-purple-600 text-white hidden" onclick="pmRestartDatabase()">重启数据库</button>
                  <button class="px-3 py-2 rounded bg-gray-200" onclick="closeProjectModal()">关闭</button>
                </div>
              </div>
            </div>


        <!-- 功能卡片 -->
        <div class="grid md:grid-cols-3 gap-8 mb-12">
            <!-- 数据生成卡片 -->
            <div class="card p-6">
                <div class="text-center">
                    <div class="icon-ring mx-auto mb-4">
                        <i class="fas fa-database text-2xl text-blue-600"></i>
                    </div>
                    <h3 class="text-xl font-semibold text-gray-900 mb-2">数据库生成</h3>
                    <p class="text-gray-600 mb-4">生成和管理数据库编号7999的数据</p>
                    <button onclick="createQuickTask(7999)" class="btn btn--primary">
                        立即执行
                    </button>
                </div>
            </div>

            <!-- 空间树生成卡片 -->
            <div class="card p-6">
                <div class="text-center">
                    <div class="icon-ring green mx-auto mb-4">
                        <i class="fas fa-sitemap text-2xl text-green-600"></i>
                    </div>
                    <h3 class="text-xl font-semibold text-gray-900 mb-2">空间树生成</h3>
                    <p class="text-gray-600 mb-4">构建和优化空间关系树结构</p>
                    <a href="/tasks" class="btn btn--success">查看任务</a>
                </div>
            </div>

            <!-- 配置管理卡片 -->
            <div class="card p-6">
                <div class="text-center">
                    <div class="icon-ring purple mx-auto mb-4">
                        <i class="fas fa-cog text-2xl text-purple-600"></i>
                    </div>
                    <h3 class="text-xl font-semibold text-gray-900 mb-2">配置管理</h3>
                    <p class="text-gray-600 mb-4">管理系统配置和参数设置</p>
                    <a href="/config" class="btn btn--purple">配置设置</a>
                </div>
            </div>
        </div>

        <!-- 系统状态 -->
        <div class="card p-6">
            <div class="flex flex-col md:flex-row md:items-center md:justify-between gap-4 mb-4">
                <h2 class="section-title">
                    <span class="section-icon bg-indigo-100 text-indigo-600">
                        <i class="fas fa-chart-bar"></i>
                    </span>
                    系统状态
                </h2>
                <a class="btn btn--ghost" href="/db-status">
                    <i class="fas fa-external-link-alt mr-2"></i>查看详情
                </a>
            </div>
            <div class="status-grid">
                <div class="status-item">
                    <div class="metric-label">任务队列</div>
                    <div class="metric-value is-loading">--</div>
                </div>
                <div class="status-item">
                    <div class="metric-label">已完成</div>
                    <div class="metric-value is-loading">--</div>
                </div>
                <div class="status-item">
                    <div class="metric-label">进行中</div>
                    <div class="metric-value is-loading">--</div>
                </div>
            </div>
        </div>
    "#;

    let extra_scripts = r#"
    <script>
      async function createQuickTask(dbNum) {
        try {
          const response = await fetch('/api/tasks', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
              name: '数据库 ' + dbNum + ' 快速生成',
              task_type: 'FullGeneration',
              config: {
                name: '数据库 ' + dbNum + ' 配置',
                manual_db_nums: [dbNum],
                gen_model: true,
                gen_mesh: true,
                gen_spatial_tree: true,
                apply_boolean_operation: true,
                mesh_tol_ratio: 3.0,
                room_keyword: '-RM',
                project_name: 'AvevaMarineSample',
                project_code: 1516
              }
            })
          });
          if (response.ok) {
            const task = await response.json();
            await fetch('/api/tasks/' + task.id + '/start', { method: 'POST' });
            alert('任务创建成功！正在跳转到任务管理页面...');
            window.location.href = '/tasks';
          } else {
            alert('任务创建失败，请稍后重试');
          }
        } catch(err) {
          console.error(err);
          alert('网络错误，请检查连接');
        }
      }
    </script>
    <script src="/static/projects.js"></script>
    "#;

    crate::web_server::layout::render_layout_with_sidebar(
        "AIOS 数据库管理平台",
        Some("home"),
        content,
        None,
        Some(extra_scripts),
    )
}

pub fn render_database_connection_page() -> String {
    r##"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>数据库连接管理 - AIOS 数据库管理平台</title>
    <link href="/static/simple-tailwind.css" rel="stylesheet">
    <link href="/static/simple-icons.css" rel="stylesheet">
    <style>
        .alert {
            padding: 12px 16px;
            border-radius: 8px;
            margin: 8px 0;
        }
        .alert-success {
            background-color: #d1fae5;
            color: #065f46;
            border: 1px solid #6ee7b7;
        }
        .alert-danger {
            background-color: #fee2e2;
            color: #991b1b;
            border: 1px solid #fca5a5;
        }
        .alert-warning {
            background-color: #fef3c7;
            color: #92400e;
            border: 1px solid #fcd34d;
        }
    </style>
</head>
<body class="bg-gray-50">
    <div class="min-h-screen">
        <!-- 导航栏 -->
        <nav class="bg-blue-600 text-white shadow-lg">
            <div class="max-w-7xl mx-auto px-4">
                <div class="flex justify-between items-center py-4">
                    <div class="flex items-center space-x-4">
                        <i class="fas fa-database text-2xl"></i>
                        <h1 class="text-xl font-bold">数据库连接管理</h1>
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
        <main class="max-w-7xl mx-auto px-4 py-8">
            <!-- 数据库启动管理卡片 -->
            <div class="bg-white rounded-lg shadow-lg p-6 mb-6">
                <div class="flex items-center justify-between mb-4">
                    <h2 class="text-xl font-semibold text-gray-900">
                        <i class="fas fa-rocket text-green-600 mr-2"></i>
                        数据库启动管理
                    </h2>
                </div>

                <!-- 启动配置表单 -->
                <div class="grid grid-cols-2 gap-4 mb-4">
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">服务器地址</label>
                        <input type="text" id="db-ip" value="127.0.0.1"
                               class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500">
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">端口</label>
                        <input type="number" id="db-port" value="8009"
                               class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500">
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">用户名</label>
                        <input type="text" id="db-user" value="root"
                               class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500">
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">密码</label>
                        <div class="relative">
                            <input type="password" id="db-password" value="root"
                                   class="w-full px-3 py-2 pr-10 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500">
                            <button type="button"
                                    onclick="togglePasswordVisibility('db-password', this)"
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
                        <label class="block text-sm font-medium text-gray-700 mb-1">命名空间(NS)</label>
                        <input type="number" id="project-code" value="1516"
                               class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500">
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">数据库(DB)</label>
                        <input type="text" id="project-name" value="AvevaMarineSample"
                               class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500">
                    </div>
                    <div class="col-span-2">
                        <label class="block text-sm font-medium text-gray-700 mb-1">数据库文件</label>
                        <input type="text" id="db-file" value="YCYK-E3D.rdb"
                               class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500">
                    </div>
                </div>

                <!-- 启动按钮和状态 -->
                <div class="flex items-center space-x-4">
                    <button id="db-start-button"
                            class="px-6 py-2 bg-green-600 text-white rounded-lg hover:bg-green-700 transition-colors">
                        启动
                    </button>
                    <button id="db-stop-button" disabled
                            class="px-6 py-2 bg-red-600 text-white rounded-lg hover:bg-red-700 transition-colors disabled:opacity-50">
                        停止
                    </button>
                    <button id="db-test-button" disabled
                            class="px-6 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors disabled:opacity-50">
                        测试连接
                    </button>
                </div>

                <!-- 消息显示区 -->
                <div id="db-startup-message" class="alert mt-4" style="display: none;"></div>

                <!-- 失败详情（可折叠） -->
                <div id="db-startup-error-details-container" class="mt-2" style="display: none;">
                    <div class="bg-red-50 border border-red-200 rounded-lg p-3">
                        <div class="flex items-center justify-between">
                            <span class="text-red-800 font-medium">
                                <i class="fas fa-exclamation-circle mr-1"></i>失败详情
                            </span>
                            <button id="copy-error-details" class="px-2 py-1 text-xs bg-red-600 text-white rounded hover:bg-red-700">
                                复制
                            </button>
                        </div>
                        <pre id="db-startup-error-details" class="text-red-700 text-sm whitespace-pre-wrap mt-2"></pre>
                    </div>
                </div>

                <!-- 进度显示区 -->
                <div id="db-startup-progress-container" class="mt-4" style="display: none;">
                    <div class="progress">
                        <div id="db-startup-progress" class="progress-bar" role="progressbar" style="width: 0%;" aria-valuenow="0" aria-valuemin="0" aria-valuemax="100"></div>
                    </div>
                    <div id="db-startup-progress-text" class="text-sm text-gray-600 mt-2"></div>
                </div>

                <!-- 启动进度显示 -->
                <div id="db-startup-progress-container" class="mt-4" style="display: none;">
                    <div class="bg-gray-200 rounded-full h-4 overflow-hidden">
                        <div id="db-startup-progress" class="bg-green-600 h-4 transition-all duration-300" style="width: 0%"></div>
                    </div>
                    <p id="db-startup-progress-text" class="text-sm text-gray-600 mt-2"></p>
                </div>

                <!-- 消息显示 -->
                <div id="db-startup-message" class="mt-4 alert" style="display: none;"></div>
            </div>

            <!-- 连接状态卡片 -->
            <div class="bg-white rounded-lg shadow-lg p-6 mb-6">
                <div class="flex items-center justify-between mb-2">
                    <div>
                        <h2 class="text-xl font-semibold text-gray-900">
                            <i class="fas fa-plug text-blue-600 mr-2"></i>
                            数据库连接状态
                        </h2>
                        <div class="mt-1 text-xs text-gray-500">
                            <span id="current-target-left">目标: 127.0.0.1:8009</span>
                        </div>
                    </div>
                    <div class="flex items-center space-x-3">
                        <span id="current-target" class="text-sm text-gray-600 hidden sm:inline">目标: 127.0.0.1:8009</span>
                        <button id="refresh-status" onclick="checkConnectionStatus()"
                                class="px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors">
                            <i class="fas fa-sync-alt mr-2"></i>刷新状态
                        </button>
                    </div>
                </div>

                <div id="connection-status" class="space-y-4">
                    <div class="flex items-center justify-center py-8">
                        <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600"></div>
                        <span class="ml-3 text-gray-600">检查连接状态中...</span>
                    </div>
                </div>
            </div>

            <!-- 启动脚本管理 -->
            <div class="bg-white rounded-lg shadow-lg p-6" id="startup-scripts-section">
                <div class="flex items-center justify-between mb-4">
                    <h2 class="text-xl font-semibold text-gray-900">
                        <i class="fas fa-play-circle text-green-600 mr-2"></i>
                        数据库启动脚本
                    </h2>
                    <button onclick="refreshStartupScripts()"
                            class="px-4 py-2 bg-green-600 text-white rounded-lg hover:bg-green-700 transition-colors">
                        <i class="fas fa-sync-alt mr-2"></i>刷新脚本
                    </button>
                </div>

                <div id="startup-scripts" class="space-y-4">
                    <div class="flex items-center justify-center py-8">
                        <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-green-600"></div>
                        <span class="ml-3 text-gray-600">加载启动脚本中...</span>
                    </div>
                </div>
            </div>
        </main>
    </div>

    <!-- 引入数据库启动管理器 -->
    <script src="/static/db_startup.js"></script>

    <script>
        let connectionCheckInterval;
        let lastConnectionStatus = null;

        // 页面加载时初始化
        document.addEventListener('DOMContentLoaded', function() {
            checkConnectionStatus();
            refreshStartupScripts();

            // 初始化数据库启动管理器
            if (window.dbStartupManager) {
                const ip = document.getElementById('db-ip').value || '127.0.0.1';
                const port = parseInt(document.getElementById('db-port').value || '8009');
                window.dbStartupManager.initializePageState(ip, port);
            }

            // 每30秒自动检查连接状态
            connectionCheckInterval = setInterval(checkConnectionStatus, 30000);
        });

        // 检查数据库连接状态
        async function checkConnectionStatus() {
            const statusContainer = document.getElementById('connection-status');
            const refreshButton = document.getElementById('refresh-status');
            const targetLabel = document.getElementById('current-target');

            // 显示加载状态
            refreshButton.disabled = true;
            refreshButton.innerHTML = '<i class="fas fa-spinner fa-spin mr-2"></i>检查中...';

            try {
                // 将界面上的配置透传给后端做体检，并更新目标标签
                const ipRaw = document.getElementById('db-ip').value || '127.0.0.1';
                const portRaw = document.getElementById('db-port').value || '8009';
                if (targetLabel) { targetLabel.textContent = `目标: ${ipRaw}:${portRaw}`; }
                const targetLabelLeft = document.getElementById('current-target-left');
                if (targetLabelLeft) { targetLabelLeft.textContent = `目标: ${ipRaw}:${portRaw}`; }

                const ip = encodeURIComponent(ipRaw);
                const port = encodeURIComponent(portRaw);
                const user = encodeURIComponent(document.getElementById('db-user').value || 'root');
                const password = encodeURIComponent(document.getElementById('db-password').value || '');
                const ns = encodeURIComponent((window.dbStartupManager?.config?.namespace) || document.getElementById('project-code')?.value || '');
                const db = encodeURIComponent((window.dbStartupManager?.config?.database) || document.getElementById('project-name')?.value || '');

                const qs = `/api/database/connection/check?ip=${ip}&port=${port}&user=${user}&password=${password}&namespace=${ns}&database=${db}`;
                const response = await fetch(qs);
                const status = await response.json();

                displayConnectionStatus(status);
                lastConnectionStatus = status;

                // 如果连接状态发生变化，刷新启动脚本
                if (shouldRefreshScripts(status)) {
                    refreshStartupScripts();
                }

            } catch (error) {
                console.error('检查连接状态失败:', error);
                statusContainer.innerHTML = `
                    <div class="bg-red-50 border border-red-200 rounded-lg p-4">
                        <div class="flex">
                            <i class="fas fa-exclamation-triangle text-red-600 mt-0.5 mr-3"></i>
                            <div>
                                <h3 class="text-red-800 font-medium">检查连接状态失败</h3>
                                <p class="text-red-600 text-sm mt-1">网络错误或服务器无法访问</p>
                            </div>
                        </div>
                    </div>
                `;
            } finally {
                refreshButton.disabled = false;
                refreshButton.innerHTML = '<i class="fas fa-sync-alt mr-2"></i>刷新状态';
            }
        }

        // 显示连接状态
        function displayConnectionStatus(status) {
            const statusContainer = document.getElementById('connection-status');
            const scriptsSection = document.getElementById('startup-scripts-section');

            if (status.connected) {
                statusContainer.innerHTML = `
                    <div class="bg-green-50 border border-green-200 rounded-lg p-4">
                        <div class="flex items-start">
                            <i class="fas fa-check-circle text-green-600 mt-0.5 mr-3"></i>
                            <div class="flex-1">
                                <h3 class="text-green-800 font-medium">数据库连接正常</h3>
                                <div class="text-green-600 text-sm mt-1 space-y-1">
                                    <p>服务器地址: ${status.config.ip}:${status.config.port}</p>
                                    <p>用户: ${status.config.user}</p>
                                    ${status.connection_time ? `<p>连接延迟: ${Math.round(status.connection_time.secs * 1000 + status.connection_time.nanos / 1000000)}ms</p>` : ''}
                                    <p>最后检查: ${new Date(status.last_check.secs_since_epoch * 1000).toLocaleString()}</p>
                                </div>
                            </div>
                        </div>
                    </div>
                `;
                scriptsSection.style.display = 'none';
            } else {
                statusContainer.innerHTML = `
                    <div class="bg-red-50 border border-red-200 rounded-lg p-4">
                        <div class="flex items-start">
                            <i class="fas fa-times-circle text-red-600 mt-0.5 mr-3"></i>
                            <div class="flex-1">
                                <h3 class="text-red-800 font-medium">数据库连接失败</h3>
                                <div class="text-red-600 text-sm mt-1 space-y-1">
                                    <p>服务器地址: ${status.config.ip}:${status.config.port}</p>
                                    <p>用户: ${status.config.user}</p>
                                    ${status.error_message ? `<p>错误信息: ${status.error_message}</p>` : ''}
                                    <p>最后检查: ${new Date(status.last_check.secs_since_epoch * 1000).toLocaleString()}</p>
                                </div>
                                <div class="mt-3 text-sm text-red-700">
                                    <p class="font-medium">建议操作:</p>
                                    <ul class="list-disc list-inside mt-1 space-y-1">
                                        <li>检查数据库服务器是否正在运行</li>
                                        <li>验证连接配置信息是否正确</li>
                                        <li>使用下方的启动脚本启动数据库实例</li>
                                    </ul>
                                </div>
                            </div>
                        </div>
                    </div>
                `;
                scriptsSection.style.display = 'block';
            }
        }

        // 刷新启动脚本列表
        async function refreshStartupScripts() {
            const scriptsContainer = document.getElementById('startup-scripts');

            try {
                const response = await fetch('/api/database/startup-scripts');
                const scripts = await response.json();

                displayStartupScripts(scripts);
            } catch (error) {
                console.error('获取启动脚本失败:', error);
                scriptsContainer.innerHTML = `
                    <div class="bg-red-50 border border-red-200 rounded-lg p-4">
                        <div class="flex">
                            <i class="fas fa-exclamation-triangle text-red-600 mt-0.5 mr-3"></i>
                            <div>
                                <h3 class="text-red-800 font-medium">加载启动脚本失败</h3>
                                <p class="text-red-600 text-sm mt-1">无法获取可用的启动脚本</p>
                            </div>
                        </div>
                    </div>
                `;
            }
        }

        // 显示启动脚本列表
        function displayStartupScripts(scripts) {
            const scriptsContainer = document.getElementById('startup-scripts');

            if (scripts.length === 0) {
                scriptsContainer.innerHTML = `
                    <div class="text-center py-8 text-gray-500">
                        <i class="fas fa-file-code text-4xl mb-4"></i>
                        <p>没有找到可用的启动脚本</p>
                    </div>
                `;
                return;
            }

            scriptsContainer.innerHTML = scripts.map(script => `
                <div class="border border-gray-200 rounded-lg p-4 hover:bg-gray-50 transition-colors">
                    <div class="flex items-center justify-between">
                        <div class="flex-1">
                            <div class="flex items-center">
                                <i class="fas fa-file-code text-gray-600 mr-2"></i>
                                <h3 class="font-medium text-gray-900">${script.name}</h3>
                                <span class="ml-2 px-2 py-1 text-xs rounded-full ${script.executable ? 'bg-green-100 text-green-800' : 'bg-yellow-100 text-yellow-800'}">
                                    ${script.executable ? '可执行' : '需要权限'}
                                </span>
                            </div>
                            <p class="text-sm text-gray-600 mt-1">${script.description}</p>
                            <p class="text-xs text-gray-500 mt-1">路径: ${script.path}</p>
                            <p class="text-xs text-gray-500">端口: ${script.port}</p>
                        </div>
                        <button onclick="startDatabaseInstance('${script.path}', ${script.port})"
                                class="ml-4 px-4 py-2 bg-green-600 text-white rounded-lg hover:bg-green-700 transition-colors">
                            <i class="fas fa-play mr-2"></i>启动
                        </button>
                    </div>
                </div>
            `).join('');
        }

        // 启动数据库实例
        async function startDatabaseInstance(scriptPath, port) {
            if (!confirm(`确定要启动数据库实例吗？\\n脚本: ${scriptPath}\\n端口: ${port}`)) {
                return;
            }

            try {
                const response = await fetch('/api/database/start-instance', {
                    method: 'POST',
                    headers: {
                        'Content-Type': 'application/json',
                    },
                    body: JSON.stringify({
                        script_path: scriptPath,
                        port: port
                    })
                });

                const result = await response.json();

                if (result.success) {
                    alert('数据库实例启动成功！\\n请稍等片刻后刷新连接状态。');

                    // 3秒后自动检查连接状态
                    setTimeout(() => {
                        checkConnectionStatus();
                    }, 3000);
                } else {
                    alert(`启动失败: ${result.message}`);
                }
            } catch (error) {
                console.error('启动数据库实例失败:', error);
                alert('启动过程中出现网络错误');
            }
        }

        // 判断是否需要刷新启动脚本
        function shouldRefreshScripts(currentStatus) {
            if (!lastConnectionStatus) return false;
            return lastConnectionStatus.connected !== currentStatus.connected;
        }

        // 页面卸载时清理定时器
        window.addEventListener('beforeunload', function() {
            if (connectionCheckInterval) {
                clearInterval(connectionCheckInterval);
            }
        });
    </script>
</body>
</html>
    "##.to_string()
}

pub fn render_simple_dashboard_page() -> String {
    r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>仪表板 - AIOS 数据库管理平台</title>
    <link href="/static/simple-tailwind.css" rel="stylesheet">
    <link href="/static/simple-icons.css" rel="stylesheet">
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
                        <a href="/" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-home mr-2"></i>首页
                        </a>
                        <a href="/dashboard" class="bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tachometer-alt mr-2"></i>仪表板
                        </a>
                        <a href="/tasks" class="hover:bg-blue-700 px-3 py-2 rounded">
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
        <main class="max-w-7xl mx-auto py-6 px-4">
            <h1 class="text-3xl font-bold text-gray-900 mb-8">
                <i class="fas fa-tachometer-alt text-blue-600 mr-3"></i>
                系统仪表板
            </h1>

            <!-- 状态卡片 -->
            <div class="grid md:grid-cols-4 gap-6 mb-8">
                <div class="bg-white rounded-lg shadow p-6">
                    <div class="flex items-center">
                        <div class="flex-shrink-0">
                            <i class="fas fa-server text-2xl text-blue-600"></i>
                        </div>
                        <div class="ml-4">
                            <p class="text-sm font-medium text-gray-500">系统状态</p>
                            <p class="text-2xl font-semibold text-gray-900">运行中</p>
                        </div>
                    </div>
                </div>

                <div class="bg-white rounded-lg shadow p-6">
                    <div class="flex items-center">
                        <div class="flex-shrink-0">
                            <i class="fas fa-database text-2xl text-green-600"></i>
                        </div>
                        <div class="ml-4">
                            <p class="text-sm font-medium text-gray-500">数据库连接</p>
                            <p class="text-2xl font-semibold text-gray-900">正常</p>
                        </div>
                    </div>
                </div>

                <div class="bg-white rounded-lg shadow p-6">
                    <div class="flex items-center">
                        <div class="flex-shrink-0">
                            <i class="fas fa-tasks text-2xl text-purple-600"></i>
                        </div>
                        <div class="ml-4">
                            <p class="text-sm font-medium text-gray-500">活跃任务</p>
                            <p class="text-2xl font-semibold text-gray-900">0</p>
                        </div>
                    </div>
                </div>

                <div class="bg-white rounded-lg shadow p-6">
                    <div class="flex items-center">
                        <div class="flex-shrink-0">
                            <i class="fas fa-clock text-2xl text-orange-600"></i>
                        </div>
                        <div class="ml-4">
                            <p class="text-sm font-medium text-gray-500">队列任务</p>
                            <p class="text-2xl font-semibold text-gray-900">0</p>
                        </div>
                    </div>
                </div>
            </div>

            <!-- 快速操作 -->
            <div class="bg-white rounded-lg shadow p-6">
                <h2 class="text-xl font-semibold text-gray-900 mb-4">快速操作</h2>
                <div class="grid md:grid-cols-3 gap-4">
                    <a href="/tasks" class="bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700 transition text-center">
                        <i class="fas fa-plus mr-2"></i>创建新任务
                    </a>
                    <a href="/config" class="bg-green-600 text-white px-4 py-2 rounded hover:bg-green-700 transition text-center">
                        <i class="fas fa-cog mr-2"></i>配置管理
                    </a>
                    <a href="/db-status" class="bg-purple-600 text-white px-4 py-2 rounded hover:bg-purple-700 transition text-center">
                        <i class="fas fa-database mr-2"></i>数据库状态
                    </a>
                </div>
            </div>
        </main>
    </div>
</body>
</html>
    "#.to_string()
}

pub fn render_simple_config_page() -> String {
    r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>配置管理 - AIOS 数据库管理平台</title>
    <link href="/static/simple-tailwind.css" rel="stylesheet">
    <link href="/static/simple-icons.css" rel="stylesheet">
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
        <main class="max-w-7xl mx-auto py-6 px-4">
            <h1 class="text-3xl font-bold text-gray-900 mb-8">
                <i class="fas fa-cog text-blue-600 mr-3"></i>
                配置管理
            </h1>

            <div class="bg-white rounded-lg shadow p-6">
                <p class="text-gray-600">配置管理功能正在开发中...</p>
                <div class="mt-4">
                    <a href="/" class="bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700 transition">
                        返回首页
                    </a>
                </div>
            </div>
        </main>
    </div>
</body>
</html>
    "#.to_string()
}

pub fn render_simple_generic_page(title: &str, content: &str) -> String {
    format!(
        r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{} - AIOS 数据库管理平台</title>
    <link href="/static/simple-tailwind.css" rel="stylesheet">
    <link href="/static/simple-icons.css" rel="stylesheet">
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
                    </div>
                </div>
            </div>
        </nav>

        <!-- 主要内容 -->
        <main class="max-w-7xl mx-auto py-6 px-4">
            <h1 class="text-3xl font-bold text-gray-900 mb-8">
                <i class="fas fa-info-circle text-blue-600 mr-3"></i>
                {}
            </h1>

            <div class="bg-white rounded-lg shadow p-6">
                <p class="text-gray-600">{}</p>
                <div class="mt-4">
                    <a href="/" class="bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700 transition">
                        返回首页
                    </a>
                </div>
            </div>
        </main>
    </div>
</body>
</html>
    "#,
        title, title, content
    )
}

/// 渲染高级任务管理页面
pub fn render_advanced_tasks_page() -> String {
    r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>任务队列管理</title>
    <link href="/static/simple-tailwind.css" rel="stylesheet">
    <link href="/static/simple-icons.css" rel="stylesheet">
    <!-- 可选：如需 x-data 绑定，可启用本地 Alpine.js -->
    <!-- <script src="/static/alpine.min.js" defer></script> -->
    <!-- Chart.js removed - not essential -->
</head>
<body class="bg-gray-50" x-data="taskManager()">
    <div class="min-h-screen">
        <!-- 导航栏 -->
        <nav class="bg-blue-600 text-white shadow-lg">
            <div class="max-w-7xl mx-auto px-4">
                <div class="flex justify-between items-center py-4">
                    <div class="flex items-center space-x-4">
                        <i class="fas fa-tasks text-2xl"></i>
                        <h1 class="text-xl font-bold">任务队列管理</h1>
                    </div>
                    <div class="flex space-x-4">
                        <a href="/" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-home mr-2"></i>首页
                        </a>
                        <a href="/dashboard" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-tachometer-alt mr-2"></i>仪表板
                        </a>
                        <a href="/batch-tasks" class="hover:bg-blue-700 px-3 py-2 rounded">
                            <i class="fas fa-layer-group mr-2"></i>批量任务
                        </a>
                    </div>
                </div>
            </div>
        </nav>

        <!-- 主要内容 -->
        <main class="max-w-7xl mx-auto py-6 px-4">
            <!-- 系统状态卡片 -->
            <div class="grid grid-cols-1 md:grid-cols-4 gap-6 mb-8">
                <div class="bg-white rounded-lg shadow p-6">
                    <div class="flex items-center">
                        <div class="flex-shrink-0">
                            <i class="fas fa-play-circle text-3xl text-green-500"></i>
                        </div>
                        <div class="ml-4">
                            <div class="text-sm font-medium text-gray-500">运行中任务</div>
                            <div class="text-2xl font-bold text-gray-900" x-text="stats.running">0</div>
                        </div>
                    </div>
                </div>

                <div class="bg-white rounded-lg shadow p-6">
                    <div class="flex items-center">
                        <div class="flex-shrink-0">
                            <i class="fas fa-clock text-3xl text-yellow-500"></i>
                        </div>
                        <div class="ml-4">
                            <div class="text-sm font-medium text-gray-500">等待队列</div>
                            <div class="text-2xl font-bold text-gray-900" x-text="stats.pending">0</div>
                        </div>
                    </div>
                </div>

                <div class="bg-white rounded-lg shadow p-6">
                    <div class="flex items-center">
                        <div class="flex-shrink-0">
                            <i class="fas fa-check-circle text-3xl text-blue-500"></i>
                        </div>
                        <div class="ml-4">
                            <div class="text-sm font-medium text-gray-500">已完成</div>
                            <div class="text-2xl font-bold text-gray-900" x-text="stats.completed">0</div>
                        </div>
                    </div>
                </div>

                <div class="bg-white rounded-lg shadow p-6">
                    <div class="flex items-center">
                        <div class="flex-shrink-0">
                            <i class="fas fa-exclamation-circle text-3xl text-red-500"></i>
                        </div>
                        <div class="ml-4">
                            <div class="text-sm font-medium text-gray-500">失败任务</div>
                            <div class="text-2xl font-bold text-gray-900" x-text="stats.failed">0</div>
                        </div>
                    </div>
                </div>
            </div>

            <!-- 任务筛选和控制 -->
            <div class="bg-white rounded-lg shadow mb-6">
                <div class="p-6 border-b border-gray-100">
                    <div class="flex flex-col md:flex-row md:items-center md:justify-between">
                        <div class="flex space-x-4 mb-4 md:mb-0">
                            <select x-model="filter.status" @change="filterTasks()" class="border rounded px-3 py-2">
                                <option value="">所有状态</option>
                                <option value="Pending">等待队列</option>
                                <option value="Running">运行中任务</option>
                                <option value="Completed">已完成</option>
                                <option value="Failed">失败任务</option>
                                <option value="Cancelled">已取消</option>
                            </select>

                            <select x-model="filter.type" @change="filterTasks()" class="border rounded px-3 py-2">
                                <option value="">所有类型</option>
                                <option value="ModelGeneration">模型生成</option>
                                <option value="SpatialTreeGeneration">空间树生成</option>
                                <option value="FullSync">完整同步</option>
                                <option value="IncrementalSync">增量同步</option>
                            </select>
                        </div>

                        <div class="flex space-x-2">
                            <button @click="refreshTasks()" type="button" class="bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700">
                                <i class="fas fa-sync-alt mr-2"></i>刷新
                            </button>
                            <button @click="openCreateModal()" type="button" class="bg-green-600 text-white px-4 py-2 rounded hover:bg-green-700" :class="{'opacity-60 cursor-not-allowed': deploymentSitesLoaded && deploymentSites.length === 0}" :disabled="deploymentSitesLoaded && deploymentSites.length === 0">
                                <i class="fas fa-plus mr-2"></i>新建任务
                            </button>
                        </div>
                        <div x-show="deploymentSitesLoaded && deploymentSites.length === 0" class="mt-2 text-xs text-red-500 flex items-center gap-2">
                            <i class="fas fa-info-circle"></i>
                            <span>
                                暂无可用部署站点，请先前往
                                <a href="/deployment-sites" class="text-blue-600 underline">部署站点</a>
                                或
                                <a href="/wizard" class="text-blue-600 underline">解析向导</a>
                                创建。
                            </span>
                            <button type="button" @click="loadDeploymentSites()" class="ml-2 px-2 py-1 bg-blue-50 text-blue-600 rounded hover:bg-blue-100">重新检查</button>
                        </div>
                    </div>
                </div>
            </div>

            <!-- 任务列表 -->
            <div class="bg-white rounded-lg shadow overflow-hidden">
                <div class="px-6 py-4 border-b border-gray-100">
                    <h3 class="text-lg font-medium">任务列表</h3>
                </div>

                <div class="overflow-x-auto">
                    <table class="min-w-full divide-y divide-gray-100">
                        <thead class="bg-gray-50">
                            <tr>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">任务信息</th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">状态</th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">进度</th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">创建时间</th>
                                <th class="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">操作</th>
                            </tr>
                        </thead>
                        <tbody class="bg-white divide-y divide-gray-200">
                            <template x-for="task in filteredTasks" :key="task.id">
                                <tr class="hover:bg-gray-50">
                                    <td class="px-6 py-4">
                                        <div class="flex items-center">
                                            <button @click="toggleTaskExpanded(task)"
                                                    class="mr-3 p-1 text-gray-400 hover:text-gray-600 focus:outline-none">
                                                <i class="fas fa-chevron-right transform transition-transform duration-200"
                                                   :class="{'rotate-90': task.expanded}"></i>
                                            </button>
                                            <div>
                                                <div class="text-sm font-medium text-gray-900" x-text="task.name"></div>
                                                <div class="text-sm text-gray-500" x-text="task.task_type"></div>
                                                <div class="text-xs text-gray-400" x-text="task.id"></div>

                                                <!-- 展开的日志内容 -->
                                                <div x-show="task.expanded" class="mt-4 border-l-4 border-blue-500 pl-4">
                                                    <h4 class="text-sm font-medium text-gray-900 mb-3 flex items-center">
                                                        <i class="fas fa-file-alt mr-2 text-blue-500"></i>
                                                        任务日志
                                                        <button @click="refreshTaskLogs(task.id)"
                                                                class="ml-3 text-xs text-blue-600 hover:text-blue-800">
                                                            <i class="fas fa-sync-alt mr-1"></i>刷新
                                                        </button>
                                                    </h4>

                                                    <div class="bg-white rounded border max-h-64 overflow-y-auto">
                                                        <template x-if="task.logs && task.logs.length > 0">
                                                            <div class="space-y-1 p-3">
                                                                <template x-for="log in task.logs.slice(-10)" :key="log.timestamp + log.message">
                                                                    <div class="flex items-start space-x-3 text-sm">
                                                                        <span class="flex-shrink-0 px-2 py-1 rounded text-xs font-medium"
                                                                              :class="getLogLevelColor(log.level)" x-text="log.level"></span>
                                                                        <div class="flex-1">
                                                                            <div class="text-gray-900" x-text="log.message"></div>
                                                                            <div class="text-xs text-gray-500 mt-1" x-text="formatDate(log.timestamp)"></div>
                                                                            <div x-show="log.details" class="text-xs text-gray-600 mt-1 bg-gray-50 p-2 rounded">
                                                                                <pre x-text="log.details" class="whitespace-pre-wrap"></pre>
                                                                            </div>
                                                                        </div>
                                                                    </div>
                                                                </template>
                                                            </div>
                                                        </template>
                                                        <template x-if="!task.logs || task.logs.length === 0">
                                                            <div class="p-3 text-center text-gray-500 text-sm">
                                                                <i class="fas fa-inbox text-gray-400 mb-2"></i>
                                                                <div>暂无日志</div>
                                                            </div>
                                                        </template>
                                                    </div>
                                                </div>
                                            </div>
                                        </div>
                                    </td>
                                    <td class="px-6 py-4">
                                        <span class="inline-flex px-2 py-1 text-xs font-semibold rounded-full"
                                              :class="getStatusColor(task.status)" x-text="getStatusText(task.status)"></span>
                                    </td>
                                    <td class="px-6 py-4">
                                        <div class="w-full bg-gray-200 rounded-full h-2.5">
                                            <div class="bg-blue-600 h-2.5 rounded-full transition-all duration-300"
                                                 :style="'width: ' + (task.progress?.percentage || 0) + '%'"></div>
                                        </div>
                                        <div class="text-xs text-gray-500 mt-1">
                                            <span x-text="(task.progress?.percentage || 0).toFixed(1)"></span>% -
                                            <span x-text="task.progress?.current_step || '等待开始'"></span>
                                        </div>
                                    </td>
                                    <td class="px-6 py-4 text-sm text-gray-500">
                                        <span x-text="formatDate(task.created_at)"></span>
                                    </td>
                                    <td class="px-6 py-4 text-sm space-x-2">
                                        <!-- 启动按钮 - 仅对等待中的任务显示 -->
                                        <button x-show="task.status === 'Pending'" @click="startTask(task.id)"
                                                class="text-green-600 hover:text-green-900" title="启动任务">
                                            <i class="fas fa-play"></i>
                                        </button>

                                        <!-- 停止按钮 - 对运行中任务显示 -->
                                        <button x-show="task.status === 'Running'" @click="stopTask(task.id)"
                                                class="text-red-600 hover:text-red-900" title="停止任务">
                                            <i class="fas fa-stop"></i>
                                        </button>

                                        <!-- 重启按钮 - 对失败的任务显示 -->
                                        <button x-show="task.status === 'Failed'" @click="restartTask(task.id)"
                                                class="text-orange-600 hover:text-orange-900" title="重新启动">
                                            <i class="fas fa-redo"></i>
                                        </button>

                                        <!-- 取消按钮 - 对失败任务也显示停止选项 -->
                                        <button x-show="task.status === 'Failed'" @click="stopTask(task.id)"
                                                class="text-red-600 hover:text-red-900" title="取消任务">
                                            <i class="fas fa-times"></i>
                                        </button>

                                        <button @click="viewTaskDetails(task)" class="text-blue-600 hover:text-blue-900" title="查看详情">
                                            <i class="fas fa-eye"></i>
                                        </button>
                                        <button @click="viewTaskLogs(task.id)" class="text-purple-600 hover:text-purple-900" title="查看日志">
                                            <i class="fas fa-file-alt"></i>
                                        </button>

                                        <!-- 删除按钮 - 对完成、失败、取消的任务显示 -->
                                        <button x-show="['Completed', 'Failed', 'Cancelled'].includes(task.status)"
                                                @click="deleteTask(task.id)" class="text-gray-600 hover:text-gray-900" title="删除任务">
                                            <i class="fas fa-trash"></i>
                                        </button>
                                        </td>
                                </tr>
                            </template>
                        </tbody>
                    </table>
                </div>

                <div x-show="filteredTasks.length === 0" class="text-center py-12">
                    <i class="fas fa-inbox text-4xl text-gray-400 mb-4"></i>
                    <p class="text-gray-500">暂无任务</p>
                </div>
            </div>
        </main>

        <!-- 新建任务模态框 -->
        <div x-show="showCreateModal" class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-1000"
             @click.self="showCreateModal = false"
             x-transition:enter="ease-out duration-300" x-transition:enter-start="opacity-0"
             x-transition:enter-end="opacity-100" x-transition:leave="ease-in duration-200"
             x-transition:leave-start="opacity-100" x-transition:leave-end="opacity-0">
            <div class="relative top-20 mx-auto p-5 border w-11/12 md:w-3/4 lg:w-1/2 shadow-lg rounded-md bg-white z-1010">
                <div class="mt-3">
                    <!-- 模态框标题 -->
                    <div class="flex items-center justify-between pb-4 border-b border-gray-100">
                        <h3 class="text-lg font-medium text-gray-900">新建任务</h3>
                        <button @click="showCreateModal = false" class="text-gray-400 hover:text-gray-600">
                            <i class="fas fa-times"></i>
                        </button>
                    </div>

                    <!-- 步骤指示器 -->
                    <div class="flex items-center justify-center space-x-4 py-4">
                        <div class="flex items-center">
                            <div class="w-8 h-8 rounded-full flex items-center justify-center text-sm font-medium"
                                 :class="createStep === 1 ? 'bg-blue-600 text-white' : 'bg-gray-200 text-gray-600'">1</div>
                            <span class="ml-2 text-sm text-gray-600">选择站点</span>
                        </div>
                        <div class="w-12 h-px bg-gray-300"></div>
                        <div class="flex items-center">
                            <div class="w-8 h-8 rounded-full flex items-center justify-center text-sm font-medium"
                                 :class="createStep === 2 ? 'bg-blue-600 text-white' : 'bg-gray-200 text-gray-600'">2</div>
                            <span class="ml-2 text-sm text-gray-600">配置任务</span>
                        </div>
                    </div>

                    <!-- 步骤1: 选择部署站点 -->
                    <div x-show="createStep === 1" class="py-4">
                        <h4 class="text-sm font-medium text-gray-900 mb-3">选择部署站点</h4>
                        <div class="space-y-3 max-h-64 overflow-y-auto">
                            <template x-for="site in deploymentSites" :key="site.id">
                                <div class="border rounded-lg p-4 cursor-pointer hover:bg-gray-50"
                                     :class="selectedSite?.id === site.id ? 'border-blue-500 bg-blue-50' : 'border-gray-200'"
                                     @click="selectedSite = site">
                                    <div class="flex items-start justify-between">
                                        <div class="flex-1">
                                            <div class="flex items-center">
                                                <h5 class="font-medium text-gray-900" x-text="site.name"></h5>
                                                <span class="ml-2 px-2 py-1 text-xs rounded-full"
                                                      :class="site.env === 'prod' ? 'bg-purple-100 text-purple-800' :
                                                             site.env === 'staging' ? 'bg-blue-100 text-blue-800' :
                                                             'bg-green-100 text-green-800'"
                                                      x-text="site.env"></span>
                                                <span class="ml-2 px-2 py-1 text-xs rounded-full"
                                                      :class="site.status === 'active' ? 'bg-green-100 text-green-800' : 'bg-gray-100 text-gray-800'"
                                                      x-text="site.status"></span>
                                            </div>
                                            <div class="mt-1 text-sm text-gray-600">
                                                <div>项目: <span x-text="site.config?.project_name"></span></div>
                                                <div>数据库: <span x-text="site.config?.db_type"></span> - <span x-text="site.config?.db_ip + ':' + site.config?.db_port"></span></div>
                                            </div>
                                        </div>
                                        <div x-show="selectedSite?.id === site.id" class="text-blue-600">
                                            <i class="fas fa-check-circle"></i>
                                        </div>
                                    </div>
                                </div>
                            </template>
                        </div>
                        <div x-show="deploymentSites.length === 0" class="text-center py-8 text-gray-500">
                            <i class="fas fa-server text-2xl mb-2"></i>
                            <div>暂无可用的部署站点</div>
                        </div>
                    </div>

                    <!-- 步骤2: 配置任务 -->
                    <div x-show="createStep === 2" class="py-4">
                        <h4 class="text-sm font-medium text-gray-900 mb-3">任务配置</h4>
                        <div class="space-y-4">
                            <div>
                                <label class="block text-sm font-medium text-gray-700 mb-2">任务名称</label>
                                <input type="text" x-model="taskConfig.name"
                                       class="w-full border border-gray-300 rounded-md px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
                                       placeholder="输入任务名称">
                            </div>

                            <div>
                                <label class="block text-sm font-medium text-gray-700 mb-2">任务类型</label>
                                <select x-model="taskConfig.task_type"
                                        class="w-full border border-gray-300 rounded-md px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500">
                                    <option value="ParsePdmsData">PDMS数据解析</option>
                                    <option value="FullGeneration">完整数据生成</option>
                                    <option value="ModelGeneration">模型生成</option>
                                    <option value="SpatialIndexing">空间索引构建</option>
                                </select>
                            </div>

                            <div class="grid grid-cols-2 gap-4">
                                <div class="flex items-center">
                                    <input type="checkbox" x-model="taskConfig.gen_model" id="gen_model"
                                           class="rounded border-gray-300 text-blue-600 focus:ring-blue-500">
                                    <label for="gen_model" class="ml-2 text-sm text-gray-700">生成模型</label>
                                </div>
                                <div class="flex items-center">
                                    <input type="checkbox" x-model="taskConfig.gen_mesh" id="gen_mesh"
                                           class="rounded border-gray-300 text-blue-600 focus:ring-blue-500">
                                    <label for="gen_mesh" class="ml-2 text-sm text-gray-700">生成网格</label>
                                </div>
                                <div class="flex items-center">
                                    <input type="checkbox" x-model="taskConfig.gen_spatial_tree" id="gen_spatial_tree"
                                           class="rounded border-gray-300 text-blue-600 focus:ring-blue-500">
                                    <label for="gen_spatial_tree" class="ml-2 text-sm text-gray-700">生成空间树</label>
                                </div>
                                <div class="flex items-center">
                                    <input type="checkbox" x-model="taskConfig.apply_boolean_operation" id="apply_boolean"
                                           class="rounded border-gray-300 text-blue-600 focus:ring-blue-500">
                                    <label for="apply_boolean" class="ml-2 text-sm text-gray-700">布尔运算</label>
                                </div>
                            </div>

                            <div>
                                <label class="block text-sm font-medium text-gray-700 mb-2">网格容差比例</label>
                                <input type="number" x-model="taskConfig.mesh_tol_ratio" step="0.1" min="0.1"
                                       class="w-full border border-gray-300 rounded-md px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500">
                            </div>
                        </div>
                    </div>

                    <!-- 模态框底部按钮 -->
                    <div class="flex justify-between pt-4 border-t">
                        <button @click="showCreateModal = false"
                                class="px-4 py-2 text-sm text-gray-600 hover:text-gray-800">取消</button>
                        <div class="space-x-2">
                            <button x-show="createStep === 2" @click="createStep = 1"
                                    class="px-4 py-2 text-sm bg-gray-200 text-gray-700 rounded hover:bg-gray-300">上一步</button>
                            <button x-show="createStep === 1" @click="nextStep()"
                                    :disabled="!selectedSite"
                                    class="px-4 py-2 text-sm bg-blue-600 text-white rounded hover:bg-blue-700 disabled:bg-gray-300 disabled:cursor-not-allowed">下一步</button>
                            <button x-show="createStep === 2" @click="createTaskFromSite()"
                                    :disabled="!taskConfig.name"
                                    class="px-4 py-2 text-sm bg-green-600 text-white rounded hover:bg-green-700 disabled:bg-gray-300 disabled:cursor-not-allowed">创建任务</button>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    </div>

    <script>
        function taskManager() {
            return {
                tasks: [],
                filteredTasks: [],
                stats: {
                    running: 0,
                    pending: 0,
                    completed: 0,
                    failed: 0
                },
                filter: {
                    status: '',
                    type: ''
                },
                showCreateModal: false,

                // 新建任务相关数据
                createStep: 1,
                deploymentSites: [],
                deploymentSitesLoaded: false,
                selectedSite: null,
                taskConfig: {
                    name: '',
                    task_type: 'ParsePdmsData',
                    gen_model: true,
                    gen_mesh: false,
                    gen_spatial_tree: true,
                    apply_boolean_operation: true,
                    mesh_tol_ratio: 3.0
                },

                init() {
                    this.loadTasks();
                    this.loadDeploymentSites();
                    // 每5秒刷新一次任务列表
                    setInterval(() => this.loadTasks(), 5000);
                },

                async loadTasks() {
                    try {
                        const response = await fetch('/api/tasks?limit=100');
                        const data = await response.json();
                        let tasks = data.tasks || [];

                        // 为每个任务设置初始状态和保持展开状态
                        for (let task of tasks) {
                            const existingTask = this.tasks.find(t => t.id === task.id);
                            // 保持现有的展开状态，新任务默认不展开
                            task.expanded = existingTask ? existingTask.expanded : false;
                            task.logs = existingTask ? existingTask.logs : [];
                        }

                        this.tasks = tasks;
                        this.updateStats();
                        this.filterTasks();
                    } catch (error) {
                        console.error('加载任务失败:', error);
                    }
                },

                updateStats() {
                    this.stats = {
                        running: this.tasks.filter(t => t.status === 'Running').length,
                        pending: this.tasks.filter(t => t.status === 'Pending').length,
                        completed: this.tasks.filter(t => t.status === 'Completed').length,
                        failed: this.tasks.filter(t => t.status === 'Failed').length
                    };
                },

                filterTasks() {
                    this.filteredTasks = this.tasks.filter(task => {
                        if (this.filter.status && task.status !== this.filter.status) return false;
                        if (this.filter.type && task.task_type !== this.filter.type) return false;
                        return true;
                    });
                },

                getStatusColor(status) {
                    const colors = {
                        Pending: 'bg-yellow-100 text-yellow-800',
                        Running: 'bg-blue-100 text-blue-800',
                        Completed: 'bg-green-100 text-green-800',
                        Failed: 'bg-red-100 text-red-800',
                        Cancelled: 'bg-gray-100 text-gray-800'
                    };
                    return colors[status] || 'bg-gray-100 text-gray-800';
                },

                getStatusText(status) {
                    const statusTexts = {
                        Pending: '等待队列',
                        Running: '运行中任务',
                        Completed: '已完成',
                        Failed: '失败任务',
                        Cancelled: '已取消'
                    };
                    return statusTexts[status] || status;
                },

                getLogLevelColor(level) {
                    const colors = {
                        'Info': 'bg-blue-100 text-blue-800',
                        'Warning': 'bg-yellow-100 text-yellow-800',
                        'Error': 'bg-red-100 text-red-800',
                        'Debug': 'bg-gray-100 text-gray-800'
                    };
                    return colors[level] || 'bg-gray-100 text-gray-800';
                },

                async toggleTaskExpanded(task) {
                    // 切换展开状态
                    task.expanded = !task.expanded;

                    // 如果展开且还没有日志，则加载日志
                    if (task.expanded && (!task.logs || task.logs.length === 0)) {
                        await this.refreshTaskLogs(task.id);
                    }
                },

                async refreshTaskLogs(taskId) {
                    try {
                        const response = await fetch(`/api/tasks/${taskId}/logs?limit=10`);
                        const data = await response.json();

                        // 更新任务的日志数据
                        const task = this.tasks.find(t => t.id === taskId);
                        if (task) {
                            task.logs = data.logs || [];
                        }
                        this.filterTasks();
                    } catch (error) {
                        console.error('刷新日志失败:', error);
                        // 即使加载日志失败，也要确保任务展开状态正确
                        const task = this.tasks.find(t => t.id === taskId);
                        if (task && !task.logs) {
                            task.logs = [];
                        }
                    }
                },

                formatDate(timestamp) {
                    if (!timestamp) return '-';
                    return new Date(timestamp).toLocaleString('zh-CN');
                },

                // 新建任务相关方法
                async loadDeploymentSites() {
                    this.deploymentSitesLoaded = false;
                    try {
                        const response = await fetch('/api/deployment-sites');
                        const data = await response.json();
                        this.deploymentSites = data.items || [];
                        this.deploymentSitesLoaded = true;
                    } catch (error) {
                        console.error('加载部署站点失败:', error);
                        this.deploymentSites = [];
                        this.deploymentSitesLoaded = true;
                    }
                },

                async openCreateModal() {
                    await this.loadDeploymentSites();
                    if (!this.deploymentSites || this.deploymentSites.length === 0) {
                        return;
                    }
                    this.resetCreateModal();
                    this.showCreateModal = true;
                },

                async nextStep() {
                    if (this.selectedSite) {
                        this.createStep = 2;
                        // 根据选中的站点配置初始化任务配置
                        const siteConfig = this.selectedSite.config;

                        // 自动生成任务名称：项目名+任务+流水号
                        try {
                            const response = await fetch('/api/tasks/next-number');
                            const data = await response.json();
                            if (data.success) {
                                const projectName = this.selectedSite.config?.project_name || this.selectedSite.name;
                                const taskNumber = String(data.next_number).padStart(4, '0');
                                this.taskConfig.name = `${projectName}-任务-${taskNumber}`;
                            } else {
                                // 如果获取失败，使用默认格式
                                this.taskConfig.name = `${this.selectedSite.name} - ${this.taskConfig.task_type}`;
                            }
                        } catch (error) {
                            console.error('获取任务序号失败:', error);
                            // 使用默认格式
                            this.taskConfig.name = `${this.selectedSite.name} - ${this.taskConfig.task_type}`;
                        }

                        if (siteConfig) {
                            this.taskConfig.gen_model = siteConfig.gen_model || false;
                            this.taskConfig.gen_mesh = siteConfig.gen_mesh || false;
                            this.taskConfig.gen_spatial_tree = siteConfig.gen_spatial_tree || false;
                            this.taskConfig.apply_boolean_operation = siteConfig.apply_boolean_operation || false;
                            this.taskConfig.mesh_tol_ratio = siteConfig.mesh_tol_ratio || 3.0;
                        }
                    }
                },

                resetCreateModal() {
                    this.createStep = 1;
                    this.selectedSite = null;
                    this.taskConfig = {
                        name: '',
                        task_type: 'ParsePdmsData',
                        gen_model: true,
                        gen_mesh: false,
                        gen_spatial_tree: true,
                        apply_boolean_operation: true,
                        mesh_tol_ratio: 3.0
                    };
                },

                async createTaskFromSite() {
                    if (!this.selectedSite || !this.taskConfig.name) {
                        alert('请选择站点和输入任务名称');
                        return;
                    }

                    try {
                        // 合并站点配置和任务配置
                        const siteConfig = this.selectedSite.config;
                        const payload = {
                            name: this.taskConfig.name,
                            task_type: this.taskConfig.task_type,
                            config: {
                                ...siteConfig,
                                name: this.taskConfig.name,
                                gen_model: this.taskConfig.gen_model,
                                gen_mesh: this.taskConfig.gen_mesh,
                                gen_spatial_tree: this.taskConfig.gen_spatial_tree,
                                apply_boolean_operation: this.taskConfig.apply_boolean_operation,
                                mesh_tol_ratio: this.taskConfig.mesh_tol_ratio
                            }
                        };

                        const response = await fetch('/api/tasks', {
                            method: 'POST',
                            headers: {
                                'Content-Type': 'application/json'
                            },
                            body: JSON.stringify(payload)
                        });

                        const result = await response.json();

                        if (response.ok) {
                            this.showCreateModal = false;
                            this.resetCreateModal();
                            this.loadTasks(); // 刷新任务列表
                            alert('任务创建成功！');
                        } else {
                            alert('任务创建失败: ' + (result.error || '未知错误'));
                        }
                    } catch (error) {
                        console.error('创建任务失败:', error);
                        alert('任务创建失败: ' + error.message);
                    }
                },

                async startTask(taskId) {
                    try {
                        await fetch(`/api/tasks/${taskId}/start`, { method: 'POST' });
                        this.loadTasks();
                    } catch (error) {
                        console.error('启动任务失败:', error);
                        alert('启动任务失败');
                    }
                },

                async stopTask(taskId) {
                    try {
                        await fetch(`/api/tasks/${taskId}/stop`, { method: 'POST' });
                        this.loadTasks();
                    } catch (error) {
                        console.error('停止任务失败:', error);
                        alert('停止任务失败');
                    }
                },

                async restartTask(taskId) {
                    if (!confirm('确定要重新启动这个任务吗？这将基于原配置重新创建并启动任务。')) return;
                    try {
                        const response = await fetch(`/api/tasks/${taskId}/restart`, { method: 'POST' });
                        if (response.ok) {
                            this.loadTasks();
                            alert('任务重启成功！');
                        } else {
                            const result = await response.json();
                            alert('重启失败: ' + (result.error || '未知错误'));
                        }
                    } catch (error) {
                        console.error('重启任务失败:', error);
                        alert('重启任务失败: ' + error.message);
                    }
                },

                async deleteTask(taskId) {
                    if (!confirm('确定要删除这个任务吗？')) return;
                    try {
                        await fetch(`/api/tasks/${taskId}`, { method: 'DELETE' });
                        this.loadTasks();
                    } catch (error) {
                        console.error('删除任务失败:', error);
                        alert('删除任务失败');
                    }
                },

                viewTaskDetails(task) {
                    // 这里可以打开一个模态框显示任务详情
                    alert('任务详情: ' + JSON.stringify(task, null, 2));
                },

                viewTaskLogs(taskId) {
                    window.open(`/tasks/${taskId}/logs`, '_blank');
                },

                refreshTasks() {
                    this.loadTasks();
                }
            }
        }
    </script>
</body>
</html>
    "#.to_string()
}

/// 渲染任务详情页面
pub fn render_task_detail_page(task_id: String) -> String {
    format!(
        r##"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>任务详情 - AIOS 数据库管理平台</title>
    <link href="/static/simple-tailwind.css" rel="stylesheet">
    <link href="/static/simple-icons.css" rel="stylesheet">
    <script src="/static/alpine.min.js" defer></script>
    <style>
        .task-detail-grid {{ display: grid; grid-template-columns: 1fr 2fr; gap: 1rem; }}
        .detail-label {{ font-weight: 600; color: #4B5563; }}
        .detail-value {{ color: #1F2937; }}
        @media (max-width: 768px) {{
            .task-detail-grid {{ grid-template-columns: 1fr; }}
        }}
    </style>
</head>
<body class="bg-gray-50">
    <div class="min-h-screen" x-data="taskDetailApp('{}')">
        <!-- 页面标题栏 -->
        <div class="bg-white shadow-sm border-b">
            <div class="max-w-7xl mx-auto px-4 py-4 sm:px-6 lg:px-8">
                <div class="flex justify-between items-center">
                    <h1 class="text-2xl font-bold text-gray-800">
                        <i class="fas fa-tasks mr-2"></i>任务详情
                    </h1>
                    <div class="flex space-x-2">
                        <a href="/tasks" class="px-4 py-2 text-sm bg-gray-100 text-gray-700 rounded-lg hover:bg-gray-200">
                            <i class="fas fa-arrow-left mr-2"></i>返回任务列表
                        </a>
                    </div>
                </div>
            </div>
        </div>

        <!-- 主要内容区域 -->
        <div class="max-w-7xl mx-auto px-4 py-8 sm:px-6 lg:px-8">
            <!-- 任务基本信息卡片 -->
            <div class="bg-white rounded-lg shadow-md p-6 mb-6">
                <h2 class="text-xl font-semibold mb-4 text-gray-800">
                    <i class="fas fa-info-circle mr-2"></i>基本信息
                </h2>
                <div class="task-detail-grid">
                    <div>
                        <span class="detail-label">任务ID:</span>
                    </div>
                    <div>
                        <span class="detail-value font-mono" x-text="task?.id || '加载中...'"></span>
                    </div>

                    <div>
                        <span class="detail-label">任务名称:</span>
                    </div>
                    <div>
                        <input type="text" x-model="task.name"
                               class="w-full px-3 py-1 border rounded-md focus:ring-2 focus:ring-blue-500"
                               @change="updateTask()">
                    </div>

                    <div>
                        <span class="detail-label">任务类型:</span>
                    </div>
                    <div>
                        <select x-model="task.task_type"
                                class="w-full px-3 py-1 border rounded-md focus:ring-2 focus:ring-blue-500"
                                @change="updateTask()">
                            <option value="DataParsingWizard">数据解析</option>
                            <option value="ModelGeneration">模型生成</option>
                            <option value="SpatialCalculation">空间计算</option>
                        </select>
                    </div>

                    <div>
                        <span class="detail-label">状态:</span>
                    </div>
                    <div>
                        <span class="px-3 py-1 rounded-full text-sm font-medium"
                              :class="getStatusClass(task?.status)"
                              x-text="getStatusLabel(task?.status)"></span>
                    </div>

                    <div>
                        <span class="detail-label">创建时间:</span>
                    </div>
                    <div>
                        <span class="detail-value" x-text="formatDate(task?.created_at)"></span>
                    </div>

                    <div>
                        <span class="detail-label">更新时间:</span>
                    </div>
                    <div>
                        <span class="detail-value" x-text="formatDate(task?.updated_at)"></span>
                    </div>

                    <div>
                        <span class="detail-label">部署站点:</span>
                    </div>
                    <div>
                        <span class="detail-value" x-text="task?.site_name || '未指定'"></span>
                    </div>
                </div>
            </div>

            <!-- 任务配置卡片 -->
            <div class="bg-white rounded-lg shadow-md p-6 mb-6">
                <h2 class="text-xl font-semibold mb-4 text-gray-800">
                    <i class="fas fa-cog mr-2"></i>任务配置
                </h2>
                <div class="space-y-4">
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">描述</label>
                        <textarea x-model="task.description"
                                  rows="3"
                                  class="w-full px-3 py-2 border rounded-md focus:ring-2 focus:ring-blue-500"
                                  @change="updateTask()"
                                  placeholder="任务描述..."></textarea>
                    </div>

                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">参数配置 (JSON)</label>
                        <textarea x-model="taskParamsJson"
                                  rows="6"
                                  class="w-full px-3 py-2 border rounded-md font-mono text-sm focus:ring-2 focus:ring-blue-500"
                                  @change="updateParams()"
                                  placeholder="{{}}"></textarea>
                    </div>
                </div>
            </div>

            <!-- 进度信息卡片 -->
            <div class="bg-white rounded-lg shadow-md p-6 mb-6" x-show="task?.status === 'Running'">
                <h2 class="text-xl font-semibold mb-4 text-gray-800">
                    <i class="fas fa-chart-line mr-2"></i>执行进度
                </h2>
                <div class="space-y-3">
                    <div class="flex justify-between text-sm">
                        <span x-text="task?.progress?.current_step || '准备中'"></span>
                        <span x-text="(task?.progress?.percentage || 0) + '%'"></span>
                    </div>
                    <div class="w-full bg-gray-200 rounded-full h-3">
                        <div class="bg-blue-600 h-3 rounded-full transition-all duration-300"
                             :style="`width: ${{(task?.progress?.percentage || 0)}}%`"></div>
                    </div>
                </div>
            </div>

            <!-- 操作按钮 -->
            <div class="flex justify-between">
                <div class="flex space-x-3">
                    <button @click="saveChanges()"
                            class="px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700">
                        <i class="fas fa-save mr-2"></i>保存修改
                    </button>
                    <button @click="viewLogs()"
                            class="px-4 py-2 bg-purple-600 text-white rounded-lg hover:bg-purple-700">
                        <i class="fas fa-file-alt mr-2"></i>查看日志
                    </button>
                    <template x-if="task?.status === 'Pending'">
                        <button @click="startTask()"
                                class="px-4 py-2 bg-green-600 text-white rounded-lg hover:bg-green-700">
                            <i class="fas fa-play mr-2"></i>启动任务
                        </button>
                    </template>
                    <template x-if="task?.status === 'Running'">
                        <button @click="stopTask()"
                                class="px-4 py-2 bg-red-600 text-white rounded-lg hover:bg-red-700">
                            <i class="fas fa-stop mr-2"></i>停止任务
                        </button>
                    </template>
                    <template x-if="['Failed', 'Stopped'].includes(task?.status)">
                        <button @click="restartTask()"
                                class="px-4 py-2 bg-orange-600 text-white rounded-lg hover:bg-orange-700">
                            <i class="fas fa-redo mr-2"></i>重启任务
                        </button>
                    </template>
                </div>
                <div>
                    <button @click="deleteTask()"
                            x-show="['Completed', 'Failed', 'Cancelled', 'Pending'].includes(task?.status)"
                            class="px-4 py-2 bg-red-600 text-white rounded-lg hover:bg-red-700">
                        <i class="fas fa-trash mr-2"></i>删除任务
                    </button>
                </div>
            </div>
        </div>
    </div>

    <script>
        function taskDetailApp(taskId) {{
            return {{
                task: null,
                taskParamsJson: '',

                async init() {{
                    await this.loadTask();
                    // 定期刷新任务状态
                    setInterval(() => {{
                        if (this.task?.status === 'Running') {{
                            this.loadTask();
                        }}
                    }}, 2000);
                }},

                async loadTask() {{
                    try {{
                        const response = await fetch('/api/tasks/' + taskId);
                        if (response.ok) {{
                            this.task = await response.json();
                            this.taskParamsJson = JSON.stringify(this.task.params || {{}}, null, 2);
                        }}
                    }} catch (error) {{
                        console.error('加载任务失败:', error);
                    }}
                }},

                async updateTask() {{
                    // 这里可以实现自动保存功能
                }},

                async updateParams() {{
                    try {{
                        this.task.params = JSON.parse(this.taskParamsJson);
                    }} catch (e) {{
                        console.error('JSON格式错误');
                    }}
                }},

                async saveChanges() {{
                    try {{
                        const response = await fetch('/api/tasks/' + taskId, {{
                            method: 'PUT',
                            headers: {{ 'Content-Type': 'application/json' }},
                            body: JSON.stringify(this.task)
                        }});
                        if (response.ok) {{
                            alert('保存成功！');
                            await this.loadTask();
                        }}
                    }} catch (error) {{
                        alert('保存失败：' + error);
                    }}
                }},

                async startTask() {{
                    if (confirm('确定要启动此任务吗？')) {{
                        try {{
                            const response = await fetch('/api/tasks/' + taskId + '/start', {{ method: 'POST' }});
                            if (response.ok) {{
                                await this.loadTask();
                            }}
                        }} catch (error) {{
                            alert('启动失败：' + error);
                        }}
                    }}
                }},

                async stopTask() {{
                    if (confirm('确定要停止此任务吗？')) {{
                        try {{
                            const response = await fetch('/api/tasks/' + taskId + '/stop', {{ method: 'POST' }});
                            if (response.ok) {{
                                await this.loadTask();
                            }}
                        }} catch (error) {{
                            alert('停止失败：' + error);
                        }}
                    }}
                }},

                async restartTask() {{
                    if (confirm('确定要重启此任务吗？')) {{
                        try {{
                            const response = await fetch('/api/tasks/' + taskId + '/restart', {{ method: 'POST' }});
                            if (response.ok) {{
                                await this.loadTask();
                            }}
                        }} catch (error) {{
                            alert('重启失败：' + error);
                        }}
                    }}
                }},

                async deleteTask() {{
                    if (confirm('确定要删除此任务吗？\\n\\n注意：此操作不可恢复！')) {{
                        try {{
                            const response = await fetch('/api/tasks/' + taskId, {{ method: 'DELETE' }});
                            if (response.ok) {{
                                alert('任务已删除');
                                window.location.href = '/tasks';
                            }}
                        }} catch (error) {{
                            alert('删除失败：' + error);
                        }}
                    }}
                }},

                viewLogs() {{
                    window.location.href = '/tasks/' + taskId + '/logs';
                }},

                getStatusClass(status) {{
                    const classes = {{
                        'Pending': 'bg-gray-100 text-gray-800',
                        'Running': 'bg-blue-100 text-blue-800',
                        'Completed': 'bg-green-100 text-green-800',
                        'Failed': 'bg-red-100 text-red-800',
                        'Stopped': 'bg-yellow-100 text-yellow-800',
                        'Cancelled': 'bg-gray-100 text-gray-800'
                    }};
                    return classes[status] || 'bg-gray-100 text-gray-800';
                }},

                getStatusLabel(status) {{
                    const labels = {{
                        'Pending': '等待中',
                        'Running': '运行中',
                        'Completed': '已完成',
                        'Failed': '失败',
                        'Stopped': '已停止',
                        'Cancelled': '已取消'
                    }};
                    return labels[status] || status;
                }},

                formatDate(dateStr) {{
                    if (!dateStr) return '未知';
                    const date = new Date(dateStr);
                    return date.toLocaleString('zh-CN');
                }}
            }};
        }}
    </script>
</body>
</html>
    "##,
        task_id
    )
}

/// 渲染任务日志页面
pub fn render_task_logs_page(task_id: String) -> String {
    format!(
        r##"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>任务日志详情 - AIOS 数据库管理平台</title>
    <link href="/static/simple-tailwind.css" rel="stylesheet">
    <link href="/static/simple-icons.css" rel="stylesheet">
    <!-- 可选：如需 x-data 绑定，可启用本地 Alpine.js -->
    <!-- <script src="/static/alpine.min.js" defer></script> -->
    <style>
        .log-entry {{
            transition: background-color 0.2s;
        }}
        .log-entry:hover {{
            background-color: #f9fafb;
        }}
        .log-timestamp {{
            font-family: "Courier New", monospace;
        }}
    </style>
</head>
<body class="bg-gray-50" x-data="taskLogsViewer()">
    <div class="min-h-screen">
        <!-- 导航栏 -->
        <nav class="bg-blue-600 text-white shadow-lg">
            <div class="max-w-7xl mx-auto px-4">
                <div class="flex justify-between items-center py-4">
                    <div class="flex items-center space-x-4">
                        <i class="fas fa-file-alt text-2xl"></i>
                        <h1 class="text-xl font-bold">任务日志详情</h1>
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
        <main class="max-w-7xl mx-auto py-6 px-4">
            <!-- 任务信息卡片 -->
            <div class="bg-white rounded-lg shadow mb-6 p-6">
                <div class="flex items-center justify-between mb-4">
                    <h2 class="text-xl font-bold text-gray-900">任务信息</h2>
                    <div class="flex space-x-2">
                        <button @click="loadLogs()" class="bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700">
                            <i class="fas fa-refresh mr-2"></i>刷新
                        </button>
                        <button @click="downloadLogs()" class="bg-green-600 text-white px-4 py-2 rounded hover:bg-green-700">
                            <i class="fas fa-download mr-2"></i>下载日志
                        </button>
                    </div>
                </div>
                <div class="grid grid-cols-1 md:grid-cols-3 gap-4" x-show="taskInfo.id">
                    <div>
                        <span class="text-gray-500">任务ID：</span>
                        <span class="font-mono text-sm" x-text="taskInfo.id"></span>
                    </div>
                    <div>
                        <span class="text-gray-500">任务名称：</span>
                        <span x-text="taskInfo.name"></span>
                    </div>
                    <div>
                        <span class="text-gray-500">任务状态：</span>
                        <span class="px-2 py-1 rounded text-xs font-semibold"
                              :class="getStatusColor(taskInfo.status)" x-text="taskInfo.status"></span>
                    </div>
                </div>
            </div>

            <!-- 日志筛选 -->
            <div class="bg-white rounded-lg shadow mb-6 p-4">
                <div class="flex flex-col md:flex-row md:items-center md:justify-between space-y-4 md:space-y-0">
                    <div class="flex space-x-4">
                        <select x-model="filters.level" @change="applyFilters()" class="border rounded px-3 py-2">
                            <option value="">所有级别</option>
                            <option value="Info">信息</option>
                            <option value="Warning">警告</option>
                            <option value="Error">错误</option>
                            <option value="Debug">调试</option>
                        </select>
                        <input type="text" x-model="filters.search" @input="applyFilters()"
                               placeholder="搜索日志内容..." class="border rounded px-3 py-2 w-64">
                    </div>
                    <div class="text-sm text-gray-500">
                        显示 <span x-text="filteredLogs.length"></span> / <span x-text="logs.length"></span> 条日志
                    </div>
                </div>
            </div>

            <!-- 日志内容 -->
            <div class="bg-white rounded-lg shadow">
                <div class="max-h-96 overflow-y-auto">
                    <template x-for="log in filteredLogs" :key="log.timestamp + log.message">
                        <div class="log-entry p-4 border-b border-gray-100 last:border-b-0">
                            <div class="flex items-start space-x-4">
                                <div class="flex-shrink-0">
                                    <span class="px-2 py-1 rounded text-xs font-semibold"
                                          :class="getLevelColor(log.level)" x-text="log.level"></span>
                                </div>
                                <div class="flex-1">
                                    <div class="text-sm text-gray-900" x-text="log.message"></div>
                                    <div class="text-xs text-gray-500 mt-1 log-timestamp" x-text="formatTimestamp(log.timestamp)"></div>
                                </div>
                            </div>
                            <div x-show="log.details" class="mt-2 ml-12">
                                <pre class="text-xs text-gray-700 bg-gray-50 rounded p-2" x-text="log.details"></pre>
                            </div>
                        </div>
                    </template>
                </div>

                <div x-show="filteredLogs.length === 0" class="text-center py-12">
                    <i class="fas fa-file-alt text-4xl text-gray-400 mb-4"></i>
                    <p class="text-gray-500">暂无日志</p>
                </div>
            </div>
        </main>
    </div>

    <script>
        function taskLogsViewer() {{
            return {{
                taskId: '{task_id}',
                taskInfo: {{}},
                logs: [],
                filteredLogs: [],
                filters: {{
                    level: '',
                    search: ''
                }},

                init() {{
                    this.loadLogs();
                    // 每10秒刷新一次日志
                    setInterval(() => this.loadLogs(), 10000);
                }},

                async loadLogs() {{
                    try {{
                        const response = await fetch(`/api/tasks/${{this.taskId}}/logs?limit=200`);
                        const data = await response.json();

                        this.taskInfo = data.task || {{}};
                        this.logs = data.logs || [];
                        this.applyFilters();
                    }} catch (error) {{
                        console.error('加载日志失败:', error);
                    }}
                }},

                applyFilters() {{
                    let filtered = this.logs;

                    if (this.filters.level) {{
                        filtered = filtered.filter(log => log.level === this.filters.level);
                    }}

                    if (this.filters.search) {{
                        const search = this.filters.search.toLowerCase();
                        filtered = filtered.filter(log =>
                            log.message.toLowerCase().includes(search) ||
                            (log.details && log.details.toLowerCase().includes(search))
                        );
                    }}

                    this.filteredLogs = filtered;
                }},

                downloadLogs() {{
                    const content = this.logs.map(log => {{
                        let line = `[${{this.formatTimestamp(log.timestamp)}}] [${{log.level}}] ${{log.message}}`;
                        if (log.details) {{
                            line += `\n${{log.details}}`;
                        }}
                        return line;
                    }}).join('\n\n');

                    const blob = new Blob([content], {{ type: 'text/plain' }});
                    const url = window.URL.createObjectURL(blob);
                    const a = document.createElement('a');
                    a.href = url;
                    a.download = `task_${{this.taskId}}_logs.txt`;
                    a.click();
                    window.URL.revokeObjectURL(url);
                }},

                formatTimestamp(timestamp) {{
                    try {{
                        return new Date(timestamp).toLocaleString('zh-CN');
                    }} catch (e) {{
                        return timestamp;
                    }}
                }},

                getStatusColor(status) {{
                    const colors = {{
                        'Pending': 'bg-yellow-100 text-yellow-800',
                        'Running': 'bg-blue-100 text-blue-800',
                        'Completed': 'bg-green-100 text-green-800',
                        'Failed': 'bg-red-100 text-red-800',
                        'Cancelled': 'bg-gray-100 text-gray-800'
                    }};
                    return colors[status] || 'bg-gray-100 text-gray-800';
                }},

                getLevelColor(level) {{
                    const colors = {{
                        'Info': 'bg-blue-100 text-blue-800',
                        'Warning': 'bg-yellow-100 text-yellow-800',
                        'Error': 'bg-red-100 text-red-800',
                        'Debug': 'bg-gray-100 text-gray-800'
                    }};
                    return colors[level] || 'bg-gray-100 text-gray-800';
                }}
            }}
        }}
    </script>
</body>
</html>
    "##,
        task_id = task_id
    )
}

/// 统一布局版：仪表板
pub fn render_dashboard_page_with_sidebar() -> String {
    let content = r#"
        <h1 class="text-3xl font-bold text-gray-900 mb-8">
            <i class="fas fa-tachometer-alt text-blue-600 mr-3"></i>
            系统仪表板
        </h1>
        <div class="grid md:grid-cols-4 gap-6 mb-8">
            <div class="card p-6">
                <div class="flex items-center">
                    <div class="flex-shrink-0"><i class="fas fa-server text-2xl text-blue-600"></i></div>
                    <div class="ml-4">
                        <p class="text-sm font-medium text-gray-500">系统状态</p>
                        <p class="text-2xl font-semibold text-gray-900">运行中</p>
                    </div>
                </div>
            </div>
            <div class="card p-6">
                <div class="flex items-center">
                    <div class="flex-shrink-0"><i class="fas fa-database text-2xl text-green-600"></i></div>
                    <div class="ml-4">
                        <p class="text-sm font-medium text-gray-500">数据库连接</p>
                        <p class="text-2xl font-semibold text-gray-900">正常</p>
                    </div>
                </div>
            </div>
            <div class="card p-6">
                <div class="flex items-center">
                    <div class="flex-shrink-0"><i class="fas fa-tasks text-2xl text-purple-600"></i></div>
                    <div class="ml-4">
                        <p class="text-sm font-medium text-gray-500">任务队列</p>
                        <p class="text-2xl font-semibold text-gray-900">3</p>
                    </div>
                </div>
            </div>
            <div class="card p-6">
                <div class="flex items-center">
                    <div class="flex-shrink-0"><i class="fas fa-check text-2xl text-green-600"></i></div>
                    <div class="ml-4">
                        <p class="text-sm font-medium text-gray-500">成功率</p>
                        <p class="text-2xl font-semibold text-gray-900">99.5%</p>
                    </div>
                </div>
            </div>
        </div>
        <div class="grid md:grid-cols-3 gap-6">
            <div class="card p-6">
                <h2 class="text-lg font-semibold text-gray-900 mb-4"><i class="fas fa-rocket mr-2 text-blue-600"></i>快速开始</h2>
                <div class="space-y-3">
                    <a href="/tasks" class="btn btn--primary">创建新任务</a>
                    <a href="/config" class="btn btn--secondary">配置参数</a>
                    <a href="/db-status" class="btn btn--secondary">查看系统状态</a>
                </div>
            </div>
            <div class="card p-6 md:col-span-2">
                <h2 class="text-lg font-semibold text-gray-900 mb-4"><i class="fas fa-history mr-2 text-blue-600"></i>最近活动</h2>
                <div class="space-y-3">
                    <div class="flex items-center justify-between">
                        <div class="flex items-center"><i class="fas fa-check text-green-600 mr-2"></i><span>任务 #1234 完成（数据库7999）</span></div>
                        <span class="text-sm text-gray-500">5 分钟前</span>
                    </div>
                    <div class="flex items-center justify-between">
                        <div class="flex items-center"><i class="fas fa-play text-blue-600 mr-2"></i><span>已启动解析任务（数据库8888）</span></div>
                        <span class="text-sm text-gray-500">1 小时前</span>
                    </div>
                    <div class="flex items-center justify-between">
                        <div class="flex items-center"><i class="fas fa-exclamation-triangle text-amber-600 mr-2"></i><span>检测到 2 个站点需要增量同步</span></div>
                        <span class="text-sm text-gray-500">昨天</span>
                    </div>
                </div>
            </div>
        </div>
    "#;

    crate::web_server::layout::render_layout_with_sidebar(
        "仪表板 - AIOS 数据库管理平台",
        Some("dashboard"),
        content,
        None,
        None,
    )
}

/// 统一布局版：配置管理
pub fn render_config_page_with_sidebar() -> String {
    let content = r#"
        <h1 class="text-3xl font-bold text-gray-900 mb-8">
            <i class="fas fa-cog text-blue-600 mr-3"></i>
            配置管理
        </h1>
        <div class="grid md:grid-cols-2 gap-6">
            <div class="card p-6">
                <h2 class="text-lg font-semibold text-gray-900 mb-4"><i class="fas fa-database mr-2"></i>数据库配置</h2>
                <p class="text-gray-600">管理数据库连接和命名空间设置等。</p>
            </div>
            <div class="card p-6">
                <h2 class="text-lg font-semibold text-gray-900 mb-4"><i class="fas fa-cogs mr-2"></i>系统参数</h2>
                <p class="text-gray-600">设置模型生成、空间树构建等参数。</p>
            </div>
        </div>

        <div class="mt-6 grid md:grid-cols-2 gap-6" x-data="cfgRuntimeBox()">
            <div class="card p-6">
                <h2 class="text-lg font-semibold text-gray-900 mb-4"><i class="fas fa-broadcast-tower mr-2"></i>运行时（MQTT/Watcher）</h2>
                <div class="text-sm text-gray-700 space-y-1">
                    <div>激活环境：<span class="font-mono" x-text="runtime.env_id||'-'"></span></div>
                    <div>MQTT连接：<span :class="runtime.mqtt_connected===true?'text-green-600':(runtime.mqtt_connected===false?'text-red-600':'text-gray-500')"
                        x-text="runtime.mqtt_connected===true?'已连接':(runtime.mqtt_connected===false?'未连接':'未知')"></span></div>
                    <div class="pt-2">
                        <button @click="refresh()" class="px-3 py-1.5 bg-gray-100 rounded hover:bg-gray-200"><i class="fas fa-sync mr-1"></i>刷新状态</button>
                        <a href="/remote-sync" class="ml-2 px-3 py-1.5 bg-blue-600 text-white rounded hover:bg-blue-700"><i class="fas fa-wrench mr-1"></i>管理异地环境</a>
                    </div>
                </div>
            </div>

            <div class="card p-6">
                <h2 class="text-lg font-semibold text-gray-900 mb-4"><i class="fas fa-file-import mr-2"></i>从 DbOption 导入环境</h2>
                <div class="text-sm text-gray-700">
                    <p>快速根据当前 DbOption.toml 中的 MQTT/文件服务/地区参数生成一个“异地环境”。</p>
                    <div class="mt-3 space-x-2">
                        <button @click="viewConfig()" class="px-3 py-1.5 bg-gray-100 rounded hover:bg-gray-200"><i class="fas fa-eye mr-1"></i>查看当前运行配置</button>
                        <button @click="doImport()" class="px-3 py-1.5 bg-green-600 text-white rounded hover:bg-green-700"><i class="fas fa-plus mr-1"></i>从 DbOption 导入</button>
                        <button @click="doImportAndActivate()" class="px-3 py-1.5 bg-indigo-600 text-white rounded hover:bg-indigo-700"><i class="fas fa-play mr-1"></i>导入并激活</button>
                    </div>
                    <pre class="mt-3 bg-gray-50 p-3 rounded text-xs overflow-x-auto" x-text="pretty(config)"></pre>
                </div>
            </div>
        </div>
    "#;

    let extra_head = Some(r#"<script src="/static/alpine.min.js" defer></script>"#);
    let extra_scripts = Some(
        r#"
    <script>
    function cfgRuntimeBox(){
      return {
        runtime: {env_id:null, mqtt_connected:null},
        config: {},
        pretty(o){ try { return JSON.stringify(o, null, 2) } catch{ return '' } },
        async refresh(){ const r = await fetch('/api/remote-sync/runtime/status'); const d = await r.json(); if(d.status==='success'){ this.runtime = { env_id: d.env_id, mqtt_connected: d.mqtt_connected }; } },
        async viewConfig(){ const r = await fetch('/api/remote-sync/runtime/config'); const d = await r.json(); if(d.status==='success'){ this.config = d.config; } },
        async doImport(){ const r = await fetch('/api/remote-sync/envs/import-from-dboption',{method:'POST'}); const d = await r.json(); if(d.status==='success'){ alert('已导入，环境ID: '+d.id); window.location.href = `/remote-sync?env=${encodeURIComponent(d.id)}`; } else { alert('导入失败'); } },
        async doImportAndActivate(){ const r = await fetch('/api/remote-sync/envs/import-from-dboption',{method:'POST'}); const d = await r.json(); if(d.status==='success'){ await fetch(`/api/remote-sync/envs/${d.id}/activate`,{method:'POST'}); window.location.href = `/remote-sync?env=${encodeURIComponent(d.id)}`; } else { alert('导入失败'); } },
        async init(){ await this.refresh(); }
      }
    }
    </script>
    "#,
    );

    crate::web_server::layout::render_layout_with_sidebar(
        "配置管理 - AIOS 数据库管理平台",
        Some("config"),
        content,
        extra_head,
        extra_scripts,
    )
}

/// 统一布局版：部署站点管理
pub fn render_deployment_sites_page_with_sidebar() -> String {
    let content = r##"
        <div class="flex items-center justify-between mb-4">
            <h1 class="text-2xl font-bold text-gray-900">
                <i class="fas fa-server text-blue-600 mr-2"></i>部署站点管理
            </h1>
            <div class="flex gap-3">
                <button onclick="importDeploymentSiteFromDbOption()" class="px-3 py-2 rounded bg-purple-600 text-white hover:bg-purple-700">
                    <i class="fas fa-file-import mr-1"></i>从 DbOption 导入
                </button>
                <button id="copy-share-link" class="px-3 py-2 rounded bg-gray-200 hover:bg-gray-300">复制分享链接</button>
                <button onclick="reloadProjects()" class="px-3 py-2 rounded bg-blue-600 text-white hover:bg-blue-700">刷新</button>
                <button onclick="window.location.href='/wizard'" class="px-3 py-2 rounded bg-green-600 text-white hover:bg-green-700">+ 创建站点</button>
            </div>
        </div>

        <div class="grid grid-cols-1 md:grid-cols-5 gap-4 mb-4">
            <div class="card p-4 cursor-pointer hover:bg-gray-50" data-status="">
                <div class="flex items-center justify-between">
                    <div>
                        <p class="text-sm text-gray-600">监控站点</p>
                        <p id="stat-total" class="text-2xl font-bold text-gray-900">--</p>
                    </div>
                    <i class="fas fa-server text-2xl text-blue-600"></i>
                </div>
            </div>
            <div class="card p-4 cursor-pointer hover:bg-gray-50" data-status="Running">
                <div class="flex items-center justify-between">
                    <div>
                        <p class="text-sm text-gray-600">运行中</p>
                        <p id="stat-running" class="text-2xl font-bold text-green-600">--</p>
                    </div>
                    <i class="fas fa-check-circle text-2xl text-green-600"></i>
                </div>
            </div>
            <div class="card p-4 cursor-pointer hover:bg-gray-50" data-status="Deploying">
                <div class="flex items-center justify-between">
                    <div>
                        <p class="text-sm text-gray-600">部署中</p>
                        <p id="stat-deploying" class="text-2xl font-bold text-blue-600">--</p>
                    </div>
                    <i class="fas fa-rocket text-2xl text-blue-600"></i>
                </div>
            </div>
            <div class="card p-4 cursor-pointer hover:bg-gray-50" data-status="Configuring">
                <div class="flex items-center justify-between">
                    <div>
                        <p class="text-sm text-gray-600">配置中</p>
                        <p id="stat-configuring" class="text-2xl font-bold text-amber-600">--</p>
                    </div>
                    <i class="fas fa-cogs text-2xl text-amber-600"></i>
                </div>
            </div>
            <div class="card p-4 cursor-pointer hover:bg-gray-50" data-status="Failed">
                <div class="flex items-center justify-between">
                    <div>
                        <p class="text-sm text-gray-600">失败</p>
                        <p id="stat-failed" class="text-2xl font-bold text-red-600">--</p>
                    </div>
                    <i class="fas fa-times-circle text-2xl text-red-600"></i>
                </div>
            </div>
        </div>

        <!-- 筛选栏 -->
        <div class="card p-4 mb-4">
          <div class="grid gap-3 md:grid-cols-4">
            <div>
              <input id="site_q" placeholder="搜索名称/描述/负责人" class="w-full border rounded px-3 py-2 text-sm" />
            </div>
            <div>
              <select id="site_status" class="w-full border rounded px-3 py-2 text-sm">
                <option value="">全部状态</option>
                <option>Configuring</option>
                <option>Deploying</option>
                <option>Running</option>
                <option>Failed</option>
                <option>Stopped</option>
              </select>
            </div>
            <div>
              <select id="site_env" class="w-full border rounded px-3 py-2 text-sm">
                <option value="">全部环境</option>
                <option>dev</option>
                <option>staging</option>
                <option>prod</option>
                <option>test</option>
              </select>
            </div>
            <div>
              <input id="site_owner" placeholder="负责人" class="w-full border rounded px-3 py-2 text-sm" />
            </div>
            <div class="md:col-span-4">
              <div class="flex flex-wrap items-center gap-3">
                <label class="text-sm text-gray-600">排序</label>
                <select id="site_sort" class="border rounded px-3 py-2 text-sm">
                  <option value="updated_at:desc">最近更新</option>
                  <option value="name:asc">名称 (A→Z)</option>
                  <option value="name:desc">名称 (Z→A)</option>
                  <option value="created_at:asc">创建时间 (旧→新)</option>
                  <option value="created_at:desc">创建时间 (新→旧)</option>
                </select>
                <div class="ml-auto flex items-center gap-2">
                  <label class="text-sm text-gray-600">每页</label>
                  <select id="site_per_page" class="border rounded px-2 py-1 text-sm">
                    <option value="6">6</option>
                    <option value="12" selected>12</option>
                    <option value="24">24</option>
                    <option value="48">48</option>
                  </select>
                  <div class="h-5 w-px bg-gray-300"></div>
                  <div class="inline-flex rounded overflow-hidden border border-gray-300">
                    <button id="view_grid" class="px-3 py-1 text-sm bg-gray-100 hover:bg-gray-200" data-view="grid"><i class="fas fa-th"></i></button>
                    <button id="view_list" class="px-3 py-1 text-sm bg-white hover:bg-gray-100" data-view="list"><i class="fas fa-list"></i></button>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>

        <div id="projects-grid" class="grid-cards grid-cards-lg"></div>
        <div id="sites-pager" class="mt-4 flex items-center justify-between text-sm text-gray-600"></div>

        <!-- 详情弹窗 Modal -->
        <div id="project-modal" class="fixed inset-0 z-1000 hidden" aria-hidden="true">
          <div class="absolute inset-0 bg-black/50" onclick="closeProjectModal()"></div>
          <div class="relative max-w-3xl mx-auto mt-16 bg-white rounded-lg shadow-lg flex flex-col z-1010" style="max-height: 85vh;">
            <!-- 标题栏 -->
            <div class="flex items-center justify-between p-6 pb-4 border-b flex-shrink-0">
              <h3 id="pm-title" class="text-xl font-semibold">部署站点详情</h3>
              <button class="text-gray-400 hover:text-gray-600 p-1 rounded-lg hover:bg-gray-100 transition-colors" onclick="closeProjectModal()">
                <i class="fas fa-times text-xl"></i>
              </button>
            </div>

            <!-- 内容区域（可滚动） -->
            <div class="flex-1 overflow-y-auto px-6 py-4" style="max-height: calc(85vh - 160px);">
              <div class="text-sm text-gray-600 flex items-center gap-3">
                <span id="pm-status" class="inline-flex items-center px-2 py-0.5 rounded text-xs bg-gray-100 text-gray-700">状态</span>
                <span id="pm-env" class="inline-flex items-center px-2 py-0.5 rounded text-xs bg-gray-100 text-gray-700">环境</span>
              </div>
              <div id="pm-hc-status" class="hidden mt-3 text-xs"></div>
              <div id="pm-error" class="hidden mt-3 p-3 rounded bg-red-50 text-red-700 text-sm">
                加载失败，请稍后重试。按 Enter 键可重试。
                <div class="mt-2"><button class="px-3 py-1 rounded bg-red-600 text-white" onclick="retryLoadProjectDetail()">重试</button></div>
              </div>
              <div id="pm-content" class="mt-4 text-sm text-gray-700">正在加载...</div>
            </div>

            <!-- 底部按钮栏 -->
            <div class="border-t px-6 py-4 flex gap-3 justify-end bg-gray-50 flex-shrink-0">
              <button id="pm-copy" class="px-3 py-2 rounded bg-gray-100" onclick="copySiteConfig()">复制配置</button>
              <button id="pm-create-task" class="px-3 py-2 rounded bg-green-600 text-white" onclick="createSiteTask()">为站点创建任务</button>
              <a id="pm-open-url" href="javascript:;" target="_blank" class="px-3 py-2 rounded bg-blue-600 text-white hidden">打开地址</a>
              <button id="pm-health" class="px-3 py-2 rounded bg-green-600 text-white hidden" onclick="pmHealthCheck()">健康检查</button>
              <button id="pm-restart-db" class="px-3 py-2 rounded bg-purple-600 text-white hidden" onclick="pmRestartDatabase()">重启数据库</button>
              <button class="px-3 py-2 rounded bg-gray-200" onclick="closeProjectModal()">关闭</button>
            </div>
          </div>
        </div>
    "##;

    let extra_scripts = r#"<script src="/static/projects.js"></script>"#;

    crate::web_server::layout::render_layout_with_sidebar(
        "部署站点管理 - AIOS",
        Some("deploy-sites"),
        content,
        None,
        Some(extra_scripts),
    )
}
