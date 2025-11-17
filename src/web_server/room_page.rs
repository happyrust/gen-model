use crate::web_server::layout::render_layout_with_sidebar;
use axum::response::Html;

/// 房间计算管理页面
pub async fn room_management_page() -> Html<String> {
    let content = r#"
<div class="container mx-auto px-4 py-8">
    <div class="mb-8">
        <h1 class="text-3xl font-bold text-gray-900 mb-2">房间计算系统</h1>
        <p class="text-gray-600">管理房间关系计算、空间查询和数据维护</p>
    </div>

    <!-- 系统状态卡片 -->
    <div class="grid grid-cols-1 md:grid-cols-4 gap-6 mb-8">
        <div class="bg-white rounded-lg shadow p-6">
            <div class="flex items-center">
                <div class="p-2 bg-green-100 rounded-lg">
                    <svg class="w-6 h-6 text-green-600" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z"></path>
                    </svg>
                </div>
                <div class="ml-4">
                    <p class="text-sm font-medium text-gray-600">系统状态</p>
                    <p class="text-2xl font-semibold text-gray-900" id="system-status">正常</p>
                </div>
            </div>
        </div>

        <div class="bg-white rounded-lg shadow p-6">
            <div class="flex items-center">
                <div class="p-2 bg-blue-100 rounded-lg">
                    <svg class="w-6 h-6 text-blue-600" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 21V5a2 2 0 00-2-2H7a2 2 0 00-2 2v16m14 0h2m-2 0h-5m-9 0H3m2 0h5M9 7h1m-1 4h1m4-4h1m-1 4h1m-5 10v-5a1 1 0 011-1h2a1 1 0 011 1v5m-4 0h4"></path>
                    </svg>
                </div>
                <div class="ml-4">
                    <p class="text-sm font-medium text-gray-600">活跃任务</p>
                    <p class="text-2xl font-semibold text-gray-900" id="active-tasks">0</p>
                </div>
            </div>
        </div>

        <div class="bg-white rounded-lg shadow p-6">
            <div class="flex items-center">
                <div class="p-2 bg-purple-100 rounded-lg">
                    <svg class="w-6 h-6 text-purple-600" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 10V3L4 14h7v7l9-11h-7z"></path>
                    </svg>
                </div>
                <div class="ml-4">
                    <p class="text-sm font-medium text-gray-600">查询性能</p>
                    <p class="text-2xl font-semibold text-gray-900" id="query-performance">0.5ms</p>
                </div>
            </div>
        </div>

        <div class="bg-white rounded-lg shadow p-6">
            <div class="flex items-center">
                <div class="p-2 bg-yellow-100 rounded-lg">
                    <svg class="w-6 h-6 text-yellow-600" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 7v10c0 2.21 3.582 4 8 4s8-1.79 8-4V7M4 7c0 2.21 3.582 4 8 4s8-1.79 8-4M4 7c0-2.21 3.582-4 8-4s8 1.79 8 4"></path>
                    </svg>
                </div>
                <div class="ml-4">
                    <p class="text-sm font-medium text-gray-600">缓存命中率</p>
                    <p class="text-2xl font-semibold text-gray-900" id="cache-hit-rate">85%</p>
                </div>
            </div>
        </div>
    </div>

    <!-- 主要功能区域 -->
    <div class="grid grid-cols-1 lg:grid-cols-2 gap-8">
        <!-- 房间查询区域 -->
        <div class="bg-white rounded-lg shadow">
            <div class="px-6 py-4 border-b border-gray-200">
                <h2 class="text-lg font-semibold text-gray-900">房间空间查询</h2>
            </div>
            <div class="p-6">
                <form id="room-query-form" class="space-y-4">
                    <div class="grid grid-cols-3 gap-4">
                        <div>
                            <label class="block text-sm font-medium text-gray-700 mb-1">X 坐标</label>
                            <input type="number" step="0.001" id="query-x" class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500" placeholder="0.000">
                        </div>
                        <div>
                            <label class="block text-sm font-medium text-gray-700 mb-1">Y 坐标</label>
                            <input type="number" step="0.001" id="query-y" class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500" placeholder="0.000">
                        </div>
                        <div>
                            <label class="block text-sm font-medium text-gray-700 mb-1">Z 坐标</label>
                            <input type="number" step="0.001" id="query-z" class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500" placeholder="0.000">
                        </div>
                    </div>
                    <button type="submit" class="w-full bg-blue-600 text-white py-2 px-4 rounded-md hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-blue-500">
                        查询房间
                    </button>
                </form>
                
                <div id="query-result" class="mt-4 p-4 bg-gray-50 rounded-md hidden">
                    <h3 class="font-medium text-gray-900 mb-2">查询结果</h3>
                    <div id="query-result-content"></div>
                </div>
            </div>
        </div>

        <!-- 房间代码处理区域 -->
        <div class="bg-white rounded-lg shadow">
            <div class="px-6 py-4 border-b border-gray-200">
                <h2 class="text-lg font-semibold text-gray-900">房间代码标准化</h2>
            </div>
            <div class="p-6">
                <form id="room-code-form" class="space-y-4">
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">房间代码</label>
                        <textarea id="room-codes" rows="4" class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500" placeholder="输入房间代码，每行一个&#10;例如：&#10;SSC-A001&#10;HD-B102&#10;HH-ROOM203"></textarea>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">项目类型</label>
                        <select id="project-type" class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500">
                            <option value="">自动检测</option>
                            <option value="SSC">SSC 项目</option>
                            <option value="HD">HD 项目</option>
                            <option value="HH">HH 项目</option>
                        </select>
                    </div>
                    <button type="submit" class="w-full bg-green-600 text-white py-2 px-4 rounded-md hover:bg-green-700 focus:outline-none focus:ring-2 focus:ring-green-500">
                        处理代码
                    </button>
                </form>
                
                <div id="code-result" class="mt-4 hidden">
                    <h3 class="font-medium text-gray-900 mb-2">处理结果</h3>
                    <div id="code-result-content" class="space-y-2"></div>
                </div>
            </div>
        </div>
    </div>

    <!-- 任务管理区域 -->
    <div class="mt-8 bg-white rounded-lg shadow">
        <div class="px-6 py-4 border-b border-gray-200 flex justify-between items-center">
            <h2 class="text-lg font-semibold text-gray-900">房间计算任务</h2>
            <button id="create-task-btn" class="bg-blue-600 text-white px-4 py-2 rounded-md hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-blue-500">
                创建新任务
            </button>
        </div>
        <div class="p-6">
            <div id="tasks-list" class="space-y-4">
                <!-- 任务列表将通过 JavaScript 动态加载 -->
            </div>
        </div>
    </div>

    <!-- 创建任务模态框 -->
    <div id="create-task-modal" class="fixed inset-0 bg-gray-600 bg-opacity-50 hidden">
        <div class="flex items-center justify-center min-h-screen p-4">
            <div class="bg-white rounded-lg shadow-xl max-w-md w-full">
                <div class="px-6 py-4 border-b border-gray-200">
                    <h3 class="text-lg font-semibold text-gray-900">创建房间计算任务</h3>
                </div>
                <form id="create-task-form" class="p-6 space-y-4">
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">任务类型</label>
                        <select id="task-type" class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500">
                            <option value="RebuildRelations">重建房间关系</option>
                            <option value="UpdateRoomCodes">更新房间代码</option>
                            <option value="DataMigration">数据迁移</option>
                            <option value="DataValidation">数据验证</option>
                            <option value="CreateSnapshot">创建快照</option>
                        </select>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">房间关键词</label>
                        <input type="text" id="room-keywords" class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500" placeholder="-RM,-ROOM" value="-RM">
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-1">数据库编号</label>
                        <input type="text" id="database-numbers" class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500" placeholder="1516,7999">
                    </div>
                    <div class="flex items-center">
                        <input type="checkbox" id="force-rebuild" class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded">
                        <label for="force-rebuild" class="ml-2 block text-sm text-gray-900">强制重建</label>
                    </div>
                    <div class="flex justify-end space-x-3 pt-4">
                        <button type="button" id="cancel-task-btn" class="px-4 py-2 border border-gray-300 rounded-md text-gray-700 hover:bg-gray-50">
                            取消
                        </button>
                        <button type="submit" class="px-4 py-2 bg-blue-600 text-white rounded-md hover:bg-blue-700">
                            创建任务
                        </button>
                    </div>
                </form>
            </div>
        </div>
    </div>
</div>

<script>
// 房间查询功能
document.getElementById('room-query-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    
    const x = parseFloat(document.getElementById('query-x').value) || 0;
    const y = parseFloat(document.getElementById('query-y').value) || 0;
    const z = parseFloat(document.getElementById('query-z').value) || 0;
    
    try {
        const response = await fetch(`/api/room/query?point=${x},${y},${z}`);
        const result = await response.json();
        
        const resultDiv = document.getElementById('query-result');
        const contentDiv = document.getElementById('query-result-content');
        
        if (result.success && result.room_number) {
            contentDiv.innerHTML = `
                <div class="text-green-600">
                    <p><strong>房间号:</strong> ${result.room_number}</p>
                    <p><strong>查询时间:</strong> ${result.query_time_ms.toFixed(2)} ms</p>
                </div>
            `;
        } else {
            contentDiv.innerHTML = `
                <div class="text-yellow-600">
                    <p>未找到包含该点的房间</p>
                    <p><strong>查询时间:</strong> ${result.query_time_ms.toFixed(2)} ms</p>
                </div>
            `;
        }
        
        resultDiv.classList.remove('hidden');
    } catch (error) {
        console.error('查询失败:', error);
        alert('查询失败，请检查网络连接');
    }
});

// 房间代码处理功能
document.getElementById('room-code-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    
    const codes = document.getElementById('room-codes').value
        .split('\n')
        .map(code => code.trim())
        .filter(code => code.length > 0);
    
    const projectType = document.getElementById('project-type').value;
    
    if (codes.length === 0) {
        alert('请输入至少一个房间代码');
        return;
    }
    
    try {
        const response = await fetch('/api/room/process-codes', {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
            },
            body: JSON.stringify({
                codes: codes,
                project_type: projectType || null
            })
        });
        
        const result = await response.json();
        
        const resultDiv = document.getElementById('code-result');
        const contentDiv = document.getElementById('code-result-content');
        
        contentDiv.innerHTML = result.results.map(r => `
            <div class="p-3 border rounded-md ${r.success ? 'border-green-200 bg-green-50' : 'border-red-200 bg-red-50'}">
                <div class="flex justify-between items-start">
                    <div>
                        <p><strong>输入:</strong> ${r.input}</p>
                        ${r.standardized_code ? `<p><strong>标准化:</strong> ${r.standardized_code}</p>` : ''}
                        ${r.project_prefix ? `<p><strong>项目:</strong> ${r.project_prefix}</p>` : ''}
                        ${r.area_code ? `<p><strong>区域:</strong> ${r.area_code}</p>` : ''}
                        ${r.room_number ? `<p><strong>房间号:</strong> ${r.room_number}</p>` : ''}
                    </div>
                    <span class="px-2 py-1 text-xs rounded-full ${r.success ? 'bg-green-100 text-green-800' : 'bg-red-100 text-red-800'}">
                        ${r.success ? '成功' : '失败'}
                    </span>
                </div>
                ${r.errors.length > 0 ? `<div class="mt-2 text-red-600 text-sm">${r.errors.join(', ')}</div>` : ''}
                ${r.warnings.length > 0 ? `<div class="mt-2 text-yellow-600 text-sm">${r.warnings.join(', ')}</div>` : ''}
            </div>
        `).join('');
        
        resultDiv.classList.remove('hidden');
    } catch (error) {
        console.error('处理失败:', error);
        alert('处理失败，请检查网络连接');
    }
});

// 模态框控制
document.getElementById('create-task-btn').addEventListener('click', () => {
    document.getElementById('create-task-modal').classList.remove('hidden');
});

document.getElementById('cancel-task-btn').addEventListener('click', () => {
    document.getElementById('create-task-modal').classList.add('hidden');
});

// 创建任务
document.getElementById('create-task-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    
    const taskType = document.getElementById('task-type').value;
    const roomKeywords = document.getElementById('room-keywords').value.split(',').map(k => k.trim());
    const databaseNumbers = document.getElementById('database-numbers').value
        .split(',')
        .map(n => parseInt(n.trim()))
        .filter(n => !isNaN(n));
    const forceRebuild = document.getElementById('force-rebuild').checked;
    
    try {
        const response = await fetch('/api/room/tasks', {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
            },
            body: JSON.stringify({
                task_type: taskType,
                config: {
                    room_keywords: roomKeywords,
                    database_numbers: databaseNumbers,
                    force_rebuild: forceRebuild,
                    validation_options: {
                        check_room_codes: true,
                        check_spatial_consistency: true,
                        check_reference_integrity: true
                    }
                }
            })
        });
        
        const task = await response.json();
        
        document.getElementById('create-task-modal').classList.add('hidden');
        loadTasks(); // 重新加载任务列表
        
        alert(`任务创建成功: ${task.id}`);
    } catch (error) {
        console.error('创建任务失败:', error);
        alert('创建任务失败，请检查网络连接');
    }
});

// 加载系统状态
async function loadSystemStatus() {
    try {
        const response = await fetch('/api/room/status');
        const status = await response.json();
        
        document.getElementById('system-status').textContent = status.system_health;
        document.getElementById('active-tasks').textContent = status.active_tasks;
        document.getElementById('query-performance').textContent = 
            status.metrics.query.avg_query_time_ms.toFixed(1) + 'ms';
        document.getElementById('cache-hit-rate').textContent = 
            (status.cache_status.hit_rate * 100).toFixed(0) + '%';
    } catch (error) {
        console.error('加载系统状态失败:', error);
    }
}

// 加载任务列表
async function loadTasks() {
    // TODO: 实现任务列表加载
    const tasksDiv = document.getElementById('tasks-list');
    tasksDiv.innerHTML = '<p class="text-gray-500">暂无活跃任务</p>';
}

// 页面加载时初始化
document.addEventListener('DOMContentLoaded', () => {
    loadSystemStatus();
    loadTasks();
    
    // 定期更新状态
    setInterval(loadSystemStatus, 30000); // 每30秒更新一次
    setInterval(loadTasks, 10000); // 每10秒更新任务列表
});
</script>
"#;

    Html(render_layout_with_sidebar(
        "房间计算系统",
        Some("tasks"),
        content,
        None,
        None,
    ))
}
