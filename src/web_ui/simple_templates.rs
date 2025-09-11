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
                  <button class="px-3 py-2 rounded bg-gray-200" onclick="closeProjectModal()">关闭</button>
                </div>
              </div>
            </div>
        </main>
    </div>

    <script>
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

    <script>
        let connectionCheckInterval;
        let lastConnectionStatus = null;

        // 页面加载时初始化
        document.addEventListener('DOMContentLoaded', function() {
            checkConnectionStatus();
            refreshStartupScripts();
            
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
                                <option value="pending">等待中</option>
                                <option value="running">运行中</option>
                                <option value="completed">已完成</option>
                                <option value="failed">已失败</option>
                                <option value="cancelled">已取消</option>
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
                            <button @click="showCreateModal = true" class="bg-green-600 text-white px-4 py-2 rounded hover:bg-green-700">
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
                                        <div>
                                            <div class="text-sm font-medium text-gray-900" x-text="task.name"></div>
                                            <div class="text-sm text-gray-500" x-text="task.task_type"></div>
                                            <div class="text-xs text-gray-400" x-text="task.id"></div>
                                        </div>
                                    </td>
                                    <td class="px-6 py-4">
                                        <span class="inline-flex px-2 py-1 text-xs font-semibold rounded-full"
                                              :class="getStatusColor(task.status)" x-text="task.status"></span>
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
                                        <button x-show="task.status === 'pending'" @click="startTask(task.id)"
                                                class="text-green-600 hover:text-green-900">
                                            <i class="fas fa-play"></i>
                                        </button>
                                        <button x-show="task.status === 'running'" @click="stopTask(task.id)"
                                                class="text-red-600 hover:text-red-900">
                                            <i class="fas fa-stop"></i>
                                        </button>
                                        <button @click="viewTaskDetails(task)" class="text-blue-600 hover:text-blue-900">
                                            <i class="fas fa-eye"></i>
                                        </button>
                                        <button @click="viewTaskLogs(task.id)" class="text-purple-600 hover:text-purple-900">
                                            <i class="fas fa-file-alt"></i>
                                        </button>
                                        <button x-show="['completed', 'failed', 'cancelled'].includes(task.status)" 
                                                @click="deleteTask(task.id)" class="text-red-600 hover:text-red-900">
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
                
                init() {
                    this.loadTasks();
                    // 每5秒刷新一次任务列表
                    setInterval(() => this.loadTasks(), 5000);
                },
                
                async loadTasks() {
                    try {
                        const response = await fetch('/api/tasks?limit=100');
                        const data = await response.json();
                        this.tasks = data.tasks || [];
                        this.updateStats();
                        this.filterTasks();
                    } catch (error) {
                        console.error('加载任务失败:', error);
                    }
                },
                
                updateStats() {
                    this.stats = {
                        running: this.tasks.filter(t => t.status === 'running').length,
                        pending: this.tasks.filter(t => t.status === 'pending').length,
                        completed: this.tasks.filter(t => t.status === 'completed').length,
                        failed: this.tasks.filter(t => t.status === 'failed').length
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
                        pending: 'bg-yellow-100 text-yellow-800',
                        running: 'bg-blue-100 text-blue-800',
                        completed: 'bg-green-100 text-green-800',
                        failed: 'bg-red-100 text-red-800',
                        cancelled: 'bg-gray-100 text-gray-800'
                    };
                    return colors[status] || 'bg-gray-100 text-gray-800';
                },
                
                formatDate(timestamp) {
                    if (!timestamp) return '-';
                    return new Date(timestamp).toLocaleString('zh-CN');
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
