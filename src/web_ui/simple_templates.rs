/// 简化的HTML模板渲染函数

pub fn render_simple_index_page() -> String {
    r##"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>AIOS 数据库管理平台</title>
    <script src="https://cdn.tailwindcss.com"></script>
    <link href="https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.0.0/css/all.min.css" rel="stylesheet">
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
            <!-- 欢迎区域 -->
            <div class="text-center mb-12">
                <h1 class="text-4xl font-bold text-gray-900 mb-4">
                    <i class="fas fa-database text-blue-600 mr-3"></i>
                    AIOS 数据库管理平台
                </h1>
                <p class="text-xl text-gray-600 mb-8">
                    专业的数据库生成和空间树管理系统
                </p>
                <div class="max-w-2xl mx-auto">
                    <div class="bg-green-50 text-green-800 border border-green-200 rounded px-4 py-3">
                        <i class="fas fa-check-circle mr-2"></i>
                        系统运行正常 - 简单 Web UI 已成功启动
                    </div>
                </div>
            </div>

            <!-- 功能卡片 -->
            <div class="grid md:grid-cols-3 gap-8 mb-12">
                <!-- 数据生成卡片 -->
                <div class="bg-white rounded-lg shadow-lg p-6 hover:shadow-xl transition-shadow">
                    <div class="text-center">
                        <div class="bg-blue-100 w-16 h-16 rounded-full flex items-center justify-center mx-auto mb-4">
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
                <div class="bg-white rounded-lg shadow-lg p-6 hover:shadow-xl transition-shadow">
                    <div class="text-center">
                        <div class="bg-green-100 w-16 h-16 rounded-full flex items-center justify-center mx-auto mb-4">
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
                <div class="bg-white rounded-lg shadow-lg p-6 hover:shadow-xl transition-shadow">
                    <div class="text-center">
                        <div class="bg-purple-100 w-16 h-16 rounded-full flex items-center justify-center mx-auto mb-4">
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
            <div class="bg-white rounded-lg shadow-lg p-6">
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
                        <button onclick="reloadProjects()" class="px-3 py-2 rounded bg-blue-600 text-white hover:bg-blue-700">刷新</button>
                        <button onclick="window.location.href='/wizard'" class="px-3 py-2 rounded bg-green-600 text-white hover:bg-green-700">+ 创建站点</button>
                    </div>
                </div>
                <!-- 筛选栏 -->
                <div class="mb-4 grid gap-3 md:grid-cols-4">
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
                <div id="projects-grid" class="grid gap-6 sm:grid-cols-2 lg:grid-cols-3"></div>
                <div id="sites-pager" class="mt-4 flex items-center justify-between text-sm text-gray-600"></div>
            </div>

            <!-- 详情弹窗 Modal -->
            <div id="project-modal" class="fixed inset-0 z-50 hidden" aria-hidden="true">
              <div class="absolute inset-0 bg-black/50" onclick="closeProjectModal()"></div>
              <div class="relative max-w-3xl mx-auto mt-16 bg-white rounded-lg shadow-lg p-6">
                <div class="flex items-start justify-between">
                  <h3 id="pm-title" class="text-xl font-semibold">部署站点详情</h3>
                  <button class="text-gray-400 hover:text-gray-600" onclick="closeProjectModal()">
                    <i class="fas fa-times"></i>
                  </button>
                </div>
                <div class="mt-2 text-sm text-gray-600 flex items-center gap-3">
                  <span id="pm-status" class="inline-flex items-center px-2 py-0.5 rounded text-xs bg-gray-100 text-gray-700">状态</span>
                  <span id="pm-env" class="inline-flex items-center px-2 py-0.5 rounded text-xs bg-gray-100 text-gray-700">环境</span>
                </div>
                <div id="pm-hc-status" class="hidden mt-3 text-xs"></div>
                <div id="pm-error" class="hidden mt-3 p-3 rounded bg-red-50 text-red-700 text-sm">
                  加载失败，请稍后重试。按 Enter 键可重试。
                  <div class="mt-2"><button class="px-3 py-1 rounded bg-red-600 text-white" onclick="retryLoadProjectDetail()">重试</button></div>
                </div>
                <div id="pm-content" class="mt-4 text-sm text-gray-700">正在加载...</div>
                <div class="mt-6 flex gap-3 justify-end">
                  <button id="pm-copy" class="px-3 py-2 rounded bg-gray-100" onclick="copySiteConfig()">复制配置</button>
                  <button id="pm-create-task" class="px-3 py-2 rounded bg-green-600 text-white" onclick="createSiteTask()">为站点创建任务</button>
                  <a id="pm-open-url" href="#" target="_blank" class="px-3 py-2 rounded bg-blue-600 text-white hidden">打开地址</a>
                  <button id="pm-health" class="px-3 py-2 rounded bg-green-600 text-white hidden" onclick="pmHealthCheck()">健康检查</button>
                  <button id="pm-restart-db" class="px-3 py-2 rounded bg-purple-600 text-white hidden" onclick="pmRestartDatabase()">重启数据库</button>
                  <button class="px-3 py-2 rounded bg-gray-200" onclick="closeProjectModal()">关闭</button>
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

        // 兜底：若外部脚本加载异常，依然可打开弹窗
        window.__openModal = function(){
            const m = document.getElementById('project-modal');
            if(m){ m.classList.remove('hidden'); m.style.display='block'; }
        };
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

pub fn render_database_connection_page() -> String {
    r##"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>数据库连接管理 - AIOS 数据库管理平台</title>
    <script src="https://cdn.tailwindcss.com"></script>
    <link href="https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.0.0/css/all.min.css" rel="stylesheet">
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
                        <input type="text" id="db-ip" value="localhost" 
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
                    <div class="col-span-2">
                        <label class="block text-sm font-medium text-gray-700 mb-1">数据库文件</label>
                        <input type="text" id="db-file" value="ams-8009-test.db" 
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
                <div class="flex items-center justify-between mb-4">
                    <h2 class="text-xl font-semibold text-gray-900">
                        <i class="fas fa-plug text-blue-600 mr-2"></i>
                        数据库连接状态
                    </h2>
                    <button id="refresh-status" onclick="checkConnectionStatus()" 
                            class="px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors">
                        <i class="fas fa-sync-alt mr-2"></i>刷新状态
                    </button>
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
                const ip = document.getElementById('db-ip').value || 'localhost';
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
            
            // 显示加载状态
            refreshButton.disabled = true;
            refreshButton.innerHTML = '<i class="fas fa-spinner fa-spin mr-2"></i>检查中...';

            try {
                const response = await fetch('/api/database/connection/check');
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
    <script src="https://cdn.tailwindcss.com"></script>
    <link href="https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.0.0/css/all.min.css" rel="stylesheet">
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
    <script src="https://cdn.tailwindcss.com"></script>
    <link href="https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.0.0/css/all.min.css" rel="stylesheet">
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
    format!(r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{} - AIOS 数据库管理平台</title>
    <script src="https://cdn.tailwindcss.com"></script>
    <link href="https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.0.0/css/all.min.css" rel="stylesheet">
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
    "#, title, title, content)
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
    <script src="https://cdn.tailwindcss.com"></script>
    <script src="https://unpkg.com/alpinejs@3.x.x/dist/cdn.min.js" defer></script>
    <link href="https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.0.0/css/all.min.css" rel="stylesheet">
    <script src="https://cdn.jsdelivr.net/npm/chart.js"></script>
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
                <div class="p-6 border-b">
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
                            <button @click="refreshTasks()" class="bg-blue-600 text-white px-4 py-2 rounded hover:bg-blue-700">
                                <i class="fas fa-sync-alt mr-2"></i>刷新
                            </button>
                            <button @click="showCreateModal = true; resetCreateModal()" class="bg-green-600 text-white px-4 py-2 rounded hover:bg-green-700">
                                <i class="fas fa-plus mr-2"></i>新建任务
                            </button>
                        </div>
                    </div>
                </div>
            </div>

            <!-- 任务列表 -->
            <div class="bg-white rounded-lg shadow overflow-hidden">
                <div class="px-6 py-4 border-b">
                    <h3 class="text-lg font-medium">任务列表</h3>
                </div>
                
                <div class="overflow-x-auto">
                    <table class="min-w-full divide-y divide-gray-200">
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
        <div x-show="showCreateModal" class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-50" 
             x-transition:enter="ease-out duration-300" x-transition:enter-start="opacity-0" 
             x-transition:enter-end="opacity-100" x-transition:leave="ease-in duration-200" 
             x-transition:leave-start="opacity-100" x-transition:leave-end="opacity-0">
            <div class="relative top-20 mx-auto p-5 border w-11/12 md:w-3/4 lg:w-1/2 shadow-lg rounded-md bg-white">
                <div class="mt-3">
                    <!-- 模态框标题 -->
                    <div class="flex items-center justify-between pb-4 border-b">
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
                    try {
                        const response = await fetch('/api/deployment-sites');
                        const data = await response.json();
                        this.deploymentSites = data.items || [];
                    } catch (error) {
                        console.error('加载部署站点失败:', error);
                        this.deploymentSites = [];
                    }
                },
                
                nextStep() {
                    if (this.selectedSite) {
                        this.createStep = 2;
                        // 根据选中的站点配置初始化任务配置
                        const siteConfig = this.selectedSite.config;
                        this.taskConfig.name = `${this.selectedSite.name} - ${this.taskConfig.task_type}`;
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

/// 渲染任务日志页面
pub fn render_task_logs_page(task_id: String) -> String {
    format!(r##"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>任务日志详情 - AIOS 数据库管理平台</title>
    <script src="https://cdn.tailwindcss.com"></script>
    <script src="https://unpkg.com/alpinejs@3.x.x/dist/cdn.min.js" defer></script>
    <link href="https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.0.0/css/all.min.css" rel="stylesheet">
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
                        <div class="log-entry p-4 border-b border-gray-200 last:border-b-0">
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
    "##, task_id = task_id)
}
