/// 批量任务管理页面模板
pub fn batch_tasks_page() -> String {
    r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>批量任务管理 - AIOS 数据库管理平台</title>
    <script src="https://cdn.tailwindcss.com"></script>
    <script src="https://unpkg.com/alpinejs@3.x.x/dist/cdn.min.js" defer></script>
    <link rel="stylesheet" href="https://cdnjs.cloudflare.com/ajax/libs/font-awesome/6.0.0/css/all.min.css">
</head>
<body class="bg-gray-50">
    <div class="min-h-screen" x-data="batchTasksApp()">
        <!-- 导航栏 -->
        <nav class="bg-white shadow-sm border-b">
            <div class="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
                <div class="flex justify-between h-16">
                    <div class="flex items-center">
                        <h1 class="text-xl font-semibold text-gray-900">
                            <i class="fas fa-tasks mr-2 text-blue-600"></i>
                            批量任务管理
                        </h1>
                    </div>
                    <div class="flex items-center space-x-4">
                        <a href="/" class="text-gray-600 hover:text-gray-900">
                            <i class="fas fa-home mr-1"></i>
                            返回首页
                        </a>
                        <a href="/tasks" class="text-gray-600 hover:text-gray-900">
                            <i class="fas fa-list mr-1"></i>
                            任务管理
                        </a>
                    </div>
                </div>
            </div>
        </nav>

        <!-- 主要内容 -->
        <div class="max-w-7xl mx-auto py-6 sm:px-6 lg:px-8">
            <!-- 页面标题和操作按钮 -->
            <div class="px-4 py-6 sm:px-0">
                <div class="flex justify-between items-center mb-6">
                    <div>
                        <h2 class="text-2xl font-bold text-gray-900">批量任务管理</h2>
                        <p class="mt-1 text-sm text-gray-600">创建和管理批量数据库处理任务</p>
                    </div>
                    <button @click="showCreateModal = true" 
                            class="bg-blue-600 hover:bg-blue-700 text-white px-4 py-2 rounded-md text-sm font-medium">
                        <i class="fas fa-plus mr-2"></i>
                        创建批量任务
                    </button>
                </div>

                <!-- 任务模板卡片 -->
                <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6 mb-8">
                    <template x-for="template in templates" :key="template.id">
                        <div class="bg-white overflow-hidden shadow rounded-lg hover:shadow-md transition-shadow">
                            <div class="p-6">
                                <div class="flex items-center">
                                    <div class="flex-shrink-0">
                                        <i class="fas fa-cog text-2xl text-blue-600"></i>
                                    </div>
                                    <div class="ml-4 flex-1">
                                        <h3 class="text-lg font-medium text-gray-900" x-text="template.name"></h3>
                                        <p class="text-sm text-gray-500" x-text="template.description"></p>
                                    </div>
                                </div>
                                <div class="mt-4">
                                    <div class="flex items-center text-sm text-gray-500 mb-2">
                                        <i class="fas fa-clock mr-2"></i>
                                        <span x-text="template.estimated_duration ? `预估时间: ${Math.floor(template.estimated_duration / 60)}分钟` : '时间未知'"></span>
                                    </div>
                                    <div class="flex items-center text-sm text-gray-500">
                                        <i class="fas fa-database mr-2"></i>
                                        <span x-text="`默认数据库: ${template.default_config.manual_db_nums.join(', ')}`"></span>
                                    </div>
                                </div>
                                <div class="mt-4">
                                    <button @click="selectTemplate(template)" 
                                            class="w-full bg-blue-50 hover:bg-blue-100 text-blue-700 px-4 py-2 rounded-md text-sm font-medium">
                                        使用此模板
                                    </button>
                                </div>
                            </div>
                        </div>
                    </template>
                </div>

                <!-- 批量任务列表 -->
                <div class="bg-white shadow overflow-hidden sm:rounded-md">
                    <div class="px-4 py-5 sm:px-6">
                        <h3 class="text-lg leading-6 font-medium text-gray-900">批量任务历史</h3>
                        <p class="mt-1 max-w-2xl text-sm text-gray-500">已创建的批量任务和执行状态</p>
                    </div>
                    <ul class="divide-y divide-gray-200">
                        <template x-for="batch in batchHistory" :key="batch.id">
                            <li class="px-4 py-4 hover:bg-gray-50">
                                <div class="flex items-center justify-between">
                                    <div class="flex-1">
                                        <div class="flex items-center">
                                            <h4 class="text-sm font-medium text-gray-900" x-text="batch.name"></h4>
                                            <span class="ml-2 inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium"
                                                  :class="getBatchStatusClass(batch.status)" x-text="batch.status"></span>
                                        </div>
                                        <div class="mt-1 text-sm text-gray-500">
                                            <span x-text="`${batch.total_tasks} 个任务`"></span>
                                            <span class="mx-2">•</span>
                                            <span x-text="`完成: ${batch.completed_tasks}`"></span>
                                            <span class="mx-2">•</span>
                                            <span x-text="`失败: ${batch.failed_tasks}`"></span>
                                        </div>
                                    </div>
                                    <div class="flex items-center space-x-2">
                                        <button @click="viewBatchDetails(batch)" 
                                                class="text-blue-600 hover:text-blue-900">
                                            <i class="fas fa-eye"></i>
                                        </button>
                                    </div>
                                </div>
                            </li>
                        </template>
                    </ul>
                </div>
            </div>
        </div>

        <!-- 创建批量任务模态框 -->
        <div x-show="showCreateModal" 
             x-transition:enter="transition ease-out duration-300"
             x-transition:enter-start="opacity-0"
             x-transition:enter-end="opacity-100"
             x-transition:leave="transition ease-in duration-200"
             x-transition:leave-start="opacity-100"
             x-transition:leave-end="opacity-0"
             class="fixed inset-0 bg-gray-600 bg-opacity-50 overflow-y-auto h-full w-full z-50">
            <div class="relative top-10 mx-auto p-5 border w-11/12 max-w-2xl shadow-lg rounded-md bg-white">
                <div class="mt-3">
                    <div class="flex justify-between items-center mb-4">
                        <h3 class="text-lg font-medium text-gray-900">创建批量任务</h3>
                        <button @click="showCreateModal = false" class="text-gray-400 hover:text-gray-600">
                            <i class="fas fa-times text-xl"></i>
                        </button>
                    </div>
                    
                    <form @submit.prevent="createBatchTasks" class="space-y-4">
                        <!-- 任务模板选择 -->
                        <div>
                            <label class="block text-sm font-medium text-gray-700 mb-2">任务模板</label>
                            <select x-model="batchForm.template_id" required
                                    class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500">
                                <option value="">请选择任务模板</option>
                                <template x-for="template in templates" :key="template.id">
                                    <option :value="template.id" x-text="template.name"></option>
                                </template>
                            </select>
                        </div>

                        <!-- 任务名称前缀 -->
                        <div>
                            <label class="block text-sm font-medium text-gray-700 mb-2">任务名称前缀</label>
                            <input type="text" x-model="batchForm.batch_config.name_prefix" required
                                   placeholder="例如: 批量几何生成"
                                   class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500">
                        </div>

                        <!-- 数据库编号 -->
                        <div>
                            <label class="block text-sm font-medium text-gray-700 mb-2">数据库编号</label>
                            <textarea x-model="dbNumsText" @input="updateDbNums" required
                                      placeholder="请输入数据库编号，用逗号分隔，例如: 7999, 8000, 1112"
                                      rows="3"
                                      class="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"></textarea>
                            <p class="mt-1 text-sm text-gray-500">
                                将为每个数据库编号创建一个独立的任务
                            </p>
                        </div>

                        <!-- 执行选项 -->
                        <div class="space-y-3">
                            <div class="flex items-center">
                                <input type="checkbox" x-model="batchForm.batch_config.parallel_execution" 
                                       class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded">
                                <label class="ml-2 block text-sm text-gray-700">并行执行（同时执行多个任务）</label>
                            </div>
                            
                            <div x-show="batchForm.batch_config.parallel_execution" class="ml-6">
                                <label class="block text-sm font-medium text-gray-700 mb-1">最大并发数</label>
                                <input type="number" x-model="batchForm.batch_config.max_concurrent" 
                                       min="1" max="10" value="3"
                                       class="w-20 px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500">
                            </div>

                            <div class="flex items-center">
                                <input type="checkbox" x-model="batchForm.batch_config.continue_on_failure" 
                                       class="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded">
                                <label class="ml-2 block text-sm text-gray-700">失败时继续执行其他任务</label>
                            </div>
                        </div>

                        <!-- 预览信息 -->
                        <div x-show="batchForm.batch_config.db_nums.length > 0" 
                             class="bg-blue-50 border border-blue-200 rounded-md p-3">
                            <h4 class="text-sm font-medium text-blue-800 mb-2">任务预览</h4>
                            <p class="text-sm text-blue-700">
                                将创建 <span x-text="batchForm.batch_config.db_nums.length" class="font-medium"></span> 个任务，
                                <span x-text="batchForm.batch_config.parallel_execution ? '并行执行' : '串行执行'"></span>
                            </p>
                            <div class="mt-2 text-xs text-blue-600">
                                数据库编号: <span x-text="batchForm.batch_config.db_nums.join(', ')"></span>
                            </div>
                        </div>

                        <!-- 提交按钮 -->
                        <div class="flex justify-end space-x-3 pt-4">
                            <button type="button" @click="showCreateModal = false"
                                    class="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-200 rounded-md hover:bg-gray-300">
                                取消
                            </button>
                            <button type="submit" :disabled="batchForm.batch_config.db_nums.length === 0"
                                    class="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed">
                                <i class="fas fa-plus mr-2"></i>
                                创建批量任务
                            </button>
                        </div>
                    </form>
                </div>
            </div>
        </div>
    </div>

    <script>
        function batchTasksApp() {
            return {
                templates: [],
                batchHistory: [],
                showCreateModal: false,
                dbNumsText: '',
                batchForm: {
                    template_id: '',
                    batch_config: {
                        name_prefix: '',
                        db_nums: [],
                        parallel_execution: false,
                        max_concurrent: 3,
                        continue_on_failure: true
                    }
                },

                async init() {
                    await this.loadTemplates();
                    await this.loadBatchHistory();
                },

                async loadTemplates() {
                    try {
                        const response = await fetch('/api/templates');
                        if (response.ok) {
                            this.templates = await response.json();
                        }
                    } catch (error) {
                        console.error('Failed to load templates:', error);
                    }
                },

                async loadBatchHistory() {
                    // 模拟批量任务历史数据
                    this.batchHistory = [
                        {
                            id: '1',
                            name: '批量几何生成 - 2025-09-02',
                            status: '进行中',
                            total_tasks: 5,
                            completed_tasks: 2,
                            failed_tasks: 0
                        },
                        {
                            id: '2',
                            name: '批量空间树构建 - 2025-09-01',
                            status: '已完成',
                            total_tasks: 3,
                            completed_tasks: 3,
                            failed_tasks: 0
                        }
                    ];
                },

                selectTemplate(template) {
                    this.batchForm.template_id = template.id;
                    this.batchForm.batch_config.name_prefix = template.name;
                    this.dbNumsText = template.default_config.manual_db_nums.join(', ');
                    this.updateDbNums();
                    this.showCreateModal = true;
                },

                updateDbNums() {
                    const nums = this.dbNumsText.split(',')
                        .map(s => s.trim())
                        .filter(s => s && !isNaN(s))
                        .map(s => parseInt(s));
                    this.batchForm.batch_config.db_nums = [...new Set(nums)]; // 去重
                },

                async createBatchTasks() {
                    try {
                        const response = await fetch('/api/tasks/batch', {
                            method: 'POST',
                            headers: {
                                'Content-Type': 'application/json',
                            },
                            body: JSON.stringify(this.batchForm)
                        });

                        if (response.ok) {
                            const result = await response.json();
                            alert(`成功创建 ${result.tasks.length} 个批量任务！`);
                            this.showCreateModal = false;
                            this.resetForm();
                            await this.loadBatchHistory();
                        } else {
                            const error = await response.text();
                            alert(`创建失败: ${error}`);
                        }
                    } catch (error) {
                        console.error('Error creating batch tasks:', error);
                        alert('网络错误，请重试');
                    }
                },

                resetForm() {
                    this.batchForm = {
                        template_id: '',
                        batch_config: {
                            name_prefix: '',
                            db_nums: [],
                            parallel_execution: false,
                            max_concurrent: 3,
                            continue_on_failure: true
                        }
                    };
                    this.dbNumsText = '';
                },

                getBatchStatusClass(status) {
                    switch (status) {
                        case '进行中': return 'bg-blue-100 text-blue-800';
                        case '已完成': return 'bg-green-100 text-green-800';
                        case '失败': return 'bg-red-100 text-red-800';
                        default: return 'bg-gray-100 text-gray-800';
                    }
                },

                viewBatchDetails(batch) {
                    alert(`批量任务详情：\n名称: ${batch.name}\n状态: ${batch.status}\n总任务数: ${batch.total_tasks}`);
                }
            }
        }
    </script>
</body>
</html>
    "#.to_string()
}
