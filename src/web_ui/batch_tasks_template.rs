/// 批量任务管理页面模板
pub fn batch_tasks_page() -> String {
    r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>批量任务管理 - AIOS Database</title>
    <link rel="stylesheet" href="/static/simple-tailwind.css">
    <link rel="stylesheet" href="/static/simple-icons.css">
    <style>
        .task-pending { background-color: #f3f4f6; color: #6b7280; }
        .task-running { background-color: #dbeafe; color: #1e40af; }
        .task-completed { background-color: #d1fae5; color: #065f46; }
        .task-failed { background-color: #fee2e2; color: #991b1b; }
        .task-cancelled { background-color: #fef3c7; color: #92400e; }

        @keyframes rotate {
            from { transform: rotate(0deg); }
            to { transform: rotate(360deg); }
        }

        .task-spinner {
            display: inline-block;
            animation: rotate 1s linear infinite;
        }

        .progress-bar {
            transition: width 0.3s ease-in-out;
        }

        .batch-item {
            transition: all 0.3s ease;
        }

        .batch-item:hover {
            transform: translateX(4px);
        }
    </style>
</head>
<body class="bg-gray-50">
    <!-- 导航栏 -->
    <nav class="bg-white shadow-sm border-b">
        <div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
            <div class="flex justify-between h-16">
                <div class="flex items-center">
                    <i class="fas fa-tasks text-2xl text-blue-600 mr-3"></i>
                    <h1 class="text-xl font-bold text-gray-900">批量任务管理中心</h1>
                </div>
                <div class="flex items-center space-x-4">
                    <button id="refresh-btn" class="px-4 py-2 text-sm bg-blue-600 text-white rounded-lg hover:bg-blue-700">
                        <i class="fas fa-sync-alt mr-2"></i>刷新
                    </button>
                    <button id="create-batch-btn" class="px-4 py-2 text-sm bg-green-600 text-white rounded-lg hover:bg-green-700">
                        <i class="fas fa-plus mr-2"></i>创建批量任务
                    </button>
                </div>
            </div>
        </div>
    </nav>

    <!-- 主内容区 -->
    <div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-6">
        <!-- 统计概览 -->
        <div class="grid grid-cols-1 md:grid-cols-5 gap-4 mb-6">
            <div class="bg-white rounded-lg shadow p-4">
                <div class="flex items-center justify-between">
                    <div>
                        <p class="text-sm text-gray-500">总任务</p>
                        <p class="text-2xl font-bold text-gray-900" id="stat-total">0</p>
                    </div>
                    <i class="fas fa-tasks text-3xl text-blue-500"></i>
                </div>
            </div>
            <div class="bg-white rounded-lg shadow p-4">
                <div class="flex items-center justify-between">
                    <div>
                        <p class="text-sm text-gray-500">运行中</p>
                        <p class="text-2xl font-bold text-blue-600" id="stat-running">0</p>
                    </div>
                    <i class="fas fa-play-circle text-3xl text-blue-500"></i>
                </div>
            </div>
            <div class="bg-white rounded-lg shadow p-4">
                <div class="flex items-center justify-between">
                    <div>
                        <p class="text-sm text-gray-500">等待中</p>
                        <p class="text-2xl font-bold text-gray-600" id="stat-pending">0</p>
                    </div>
                    <i class="fas fa-clock text-3xl text-gray-500"></i>
                </div>
            </div>
            <div class="bg-white rounded-lg shadow p-4">
                <div class="flex items-center justify-between">
                    <div>
                        <p class="text-sm text-gray-500">已完成</p>
                        <p class="text-2xl font-bold text-green-600" id="stat-completed">0</p>
                    </div>
                    <i class="fas fa-check-circle text-3xl text-green-500"></i>
                </div>
            </div>
            <div class="bg-white rounded-lg shadow p-4">
                <div class="flex items-center justify-between">
                    <div>
                        <p class="text-sm text-gray-500">失败</p>
                        <p class="text-2xl font-bold text-red-600" id="stat-failed">0</p>
                    </div>
                    <i class="fas fa-times-circle text-3xl text-red-500"></i>
                </div>
            </div>
        </div>

        <!-- 批量任务列表 -->
        <div class="bg-white rounded-lg shadow">
            <div class="px-6 py-4 border-b">
                <h2 class="text-lg font-semibold text-gray-900">批量任务队列</h2>
            </div>
            <div class="p-6">
                <div id="batch-tasks-container" class="space-y-4">
                    <!-- 批量任务卡片将通过 JavaScript 动态生成 -->
                </div>
            </div>
        </div>

        <!-- 任务模板 -->
        <div class="mt-6 bg-white rounded-lg shadow">
            <div class="px-6 py-4 border-b">
                <h2 class="text-lg font-semibold text-gray-900">任务模板</h2>
            </div>
            <div class="p-6">
                <div class="grid grid-cols-1 md:grid-cols-3 gap-4" id="templates-container">
                    <!-- 模板卡片将通过 JavaScript 动态生成 -->
                </div>
            </div>
        </div>
    </div>

    <!-- 创建批量任务模态框 -->
    <div id="create-modal" class="hidden fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-50">
        <div class="relative top-20 mx-auto p-5 border w-11/12 max-w-2xl shadow-lg rounded-md bg-white">
            <div class="flex justify-between items-center pb-3 border-b">
                <h3 class="text-xl font-bold text-gray-900">创建批量任务</h3>
                <button onclick="closeCreateModal()" class="text-gray-400 hover:text-gray-600">
                    <i class="fas fa-times text-xl"></i>
                </button>
            </div>
            <div class="mt-4">
                <form id="create-batch-form" class="space-y-4">
                    <div>
                        <label class="block text-sm font-medium text-gray-700">任务类型</label>
                        <select id="task-type" class="mt-1 block w-full px-3 py-2 border border-gray-300 rounded-md">
                            <option value="parse">批量解析</option>
                            <option value="generate">批量生成模型</option>
                            <option value="spatial">批量构建空间树</option>
                            <option value="update">批量更新</option>
                        </select>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700">任务名称</label>
                        <input type="text" id="batch-name" placeholder="输入批量任务名称"
                               class="mt-1 block w-full px-3 py-2 border border-gray-300 rounded-md">
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700">数据库编号列表</label>
                        <textarea id="db-nums" rows="3" placeholder="输入数据库编号，用逗号或换行分隔，例如: 7999, 8000, 8001"
                                  class="mt-1 block w-full px-3 py-2 border border-gray-300 rounded-md"></textarea>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700">并发数</label>
                        <input type="number" id="concurrency" value="3" min="1" max="10"
                               class="mt-1 block w-full px-3 py-2 border border-gray-300 rounded-md">
                    </div>
                    <div>
                        <label class="flex items-center">
                            <input type="checkbox" id="auto-start" checked class="mr-2">
                            <span class="text-sm font-medium text-gray-700">立即开始执行</span>
                        </label>
                    </div>
                    <div class="flex justify-end space-x-2 pt-4">
                        <button type="button" onclick="closeCreateModal()"
                                class="px-4 py-2 bg-gray-300 text-gray-700 rounded-md hover:bg-gray-400">
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

    <!-- 任务详情模态框 -->
    <div id="detail-modal" class="hidden fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-50">
        <div class="relative top-20 mx-auto p-5 border w-11/12 max-w-4xl shadow-lg rounded-md bg-white">
            <div class="flex justify-between items-center pb-3 border-b">
                <h3 class="text-xl font-bold text-gray-900" id="detail-title">任务详情</h3>
                <button onclick="closeDetailModal()" class="text-gray-400 hover:text-gray-600">
                    <i class="fas fa-times text-xl"></i>
                </button>
            </div>
            <div class="mt-4" id="detail-content">
                <!-- 详情内容将动态加载 -->
            </div>
        </div>
    </div>

    <script src="/static/batch_tasks.js"></script>
</body>
</html>
    "#.to_string()
}
