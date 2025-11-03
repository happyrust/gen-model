// 批量任务管理 JavaScript
class BatchTasksManager {
    constructor() {
        this.batchTasks = [];
        this.templates = [];
        this.autoRefresh = true;
        this.refreshInterval = null;
        this.init();
    }

    async init() {
        this.bindEvents();
        await this.loadTemplates();
        await this.loadBatchTasks();
        this.startAutoRefresh();
    }

    bindEvents() {
        // 刷新按钮
        document.getElementById('refresh-btn').addEventListener('click', () => {
            this.loadBatchTasks();
        });

        // 创建批量任务按钮
        document.getElementById('create-batch-btn').addEventListener('click', () => {
            this.showCreateModal();
        });

        // 表单提交
        document.getElementById('create-batch-form').addEventListener('submit', (e) => {
            e.preventDefault();
            this.createBatchTask();
        });
    }

    async loadTemplates() {
        try {
            // 暂时使用模拟数据
            this.templates = [
                {
                    id: 'parse-all',
                    name: '批量解析',
                    description: '解析所有未解析的数据库',
                    taskType: 'parse'
                },
                {
                    id: 'generate-all',
                    name: '批量生成模型',
                    description: '为所有已解析的数据库生成模型',
                    taskType: 'generate'
                },
                {
                    id: 'spatial-all',
                    name: '批量构建空间树',
                    description: '为所有模型构建空间索引',
                    taskType: 'spatial'
                }
            ];
            this.renderTemplates();
        } catch (error) {
            console.error('Failed to load templates:', error);
        }
    }

    async loadBatchTasks() {
        try {
            const response = await fetch('/api/batch-tasks');
            if (response.ok) {
                this.batchTasks = await response.json();
            } else {
                // 使用模拟数据
                this.batchTasks = this.getMockBatchTasks();
            }
            this.updateStats();
            this.renderBatchTasks();
        } catch (error) {
            console.error('Failed to load batch tasks:', error);
            // 使用模拟数据
            this.batchTasks = this.getMockBatchTasks();
            this.updateStats();
            this.renderBatchTasks();
        }
    }

    getMockBatchTasks() {
        return [
            {
                id: 'batch-1',
                name: '批量解析 DESI 模块',
                type: 'parse',
                status: 'running',
                totalTasks: 10,
                completedTasks: 3,
                failedTasks: 0,
                dbNums: [7999, 8000, 8001, 8002, 8003, 8004, 8005, 8006, 8007, 8008],
                startTime: new Date(Date.now() - 600000).toISOString(),
                progress: 30
            },
            {
                id: 'batch-2',
                name: '批量生成模型 EQUI',
                type: 'generate',
                status: 'pending',
                totalTasks: 5,
                completedTasks: 0,
                failedTasks: 0,
                dbNums: [8010, 8011, 8012, 8013, 8014],
                startTime: null,
                progress: 0
            },
            {
                id: 'batch-3',
                name: '批量构建空间树',
                type: 'spatial',
                status: 'completed',
                totalTasks: 8,
                completedTasks: 8,
                failedTasks: 0,
                dbNums: [7990, 7991, 7992, 7993, 7994, 7995, 7996, 7997],
                startTime: new Date(Date.now() - 3600000).toISOString(),
                endTime: new Date(Date.now() - 1800000).toISOString(),
                progress: 100
            }
        ];
    }

    updateStats() {
        const stats = {
            total: this.batchTasks.length,
            running: this.batchTasks.filter(t => t.status === 'running').length,
            pending: this.batchTasks.filter(t => t.status === 'pending').length,
            completed: this.batchTasks.filter(t => t.status === 'completed').length,
            failed: this.batchTasks.filter(t => t.status === 'failed').length
        };

        document.getElementById('stat-total').textContent = stats.total;
        document.getElementById('stat-running').textContent = stats.running;
        document.getElementById('stat-pending').textContent = stats.pending;
        document.getElementById('stat-completed').textContent = stats.completed;
        document.getElementById('stat-failed').textContent = stats.failed;
    }

    renderBatchTasks() {
        const container = document.getElementById('batch-tasks-container');

        if (this.batchTasks.length === 0) {
            container.innerHTML = `
                <div class="text-center py-8 text-gray-500">
                    <i class="fas fa-inbox text-4xl mb-2"></i>
                    <p>暂无批量任务</p>
                </div>
            `;
            return;
        }

        container.innerHTML = this.batchTasks.map(task => `
            <div class="batch-item border rounded-lg p-4 hover:shadow-md ${this.getStatusBgClass(task.status)}">
                <div class="flex items-center justify-between">
                    <div class="flex-1">
                        <div class="flex items-center">
                            <h3 class="text-lg font-medium text-gray-900">${task.name}</h3>
                            <span class="ml-3 px-2 py-1 text-xs rounded-full ${this.getStatusClass(task.status)}">
                                ${this.getStatusText(task.status)}
                            </span>
                            ${task.status === 'running' ? '<i class="fas fa-spinner task-spinner ml-2"></i>' : ''}
                        </div>
                        <div class="mt-2 text-sm text-gray-600">
                            <span>类型: ${this.getTaskTypeText(task.type)}</span>
                            <span class="mx-2">•</span>
                            <span>数据库: ${task.dbNums.length} 个</span>
                            <span class="mx-2">•</span>
                            <span>进度: ${task.completedTasks}/${task.totalTasks}</span>
                            ${task.failedTasks > 0 ? `<span class="mx-2 text-red-600">• 失败: ${task.failedTasks}</span>` : ''}
                        </div>
                        ${task.status === 'running' || task.status === 'completed' ? `
                            <div class="mt-2">
                                <div class="w-full bg-gray-200 rounded-full h-2">
                                    <div class="progress-bar bg-blue-600 h-2 rounded-full" style="width: ${task.progress}%"></div>
                                </div>
                            </div>
                        ` : ''}
                        <div class="mt-2 text-xs text-gray-500">
                            ${task.startTime ? `开始时间: ${new Date(task.startTime).toLocaleString()}` : '未开始'}
                            ${task.endTime ? ` • 结束时间: ${new Date(task.endTime).toLocaleString()}` : ''}
                        </div>
                    </div>
                    <div class="flex items-center space-x-2 ml-4">
                        ${task.status === 'pending' ? `
                            <button onclick="batchTasksManager.startTask('${task.id}')"
                                    class="px-3 py-1 bg-green-600 text-white rounded hover:bg-green-700">
                                <i class="fas fa-play"></i>
                            </button>
                        ` : ''}
                        ${task.status === 'running' ? `
                            <button onclick="batchTasksManager.pauseTask('${task.id}')"
                                    class="px-3 py-1 bg-yellow-600 text-white rounded hover:bg-yellow-700">
                                <i class="fas fa-pause"></i>
                            </button>
                            <button onclick="batchTasksManager.stopTask('${task.id}')"
                                    class="px-3 py-1 bg-red-600 text-white rounded hover:bg-red-700">
                                <i class="fas fa-stop"></i>
                            </button>
                        ` : ''}
                        <button onclick="batchTasksManager.viewTaskDetails('${task.id}')"
                                class="px-3 py-1 bg-blue-600 text-white rounded hover:bg-blue-700">
                            <i class="fas fa-eye"></i>
                        </button>
                    </div>
                </div>
            </div>
        `).join('');
    }

    renderTemplates() {
        const container = document.getElementById('templates-container');

        container.innerHTML = this.templates.map(template => `
            <div class="border rounded-lg p-4 hover:shadow-md cursor-pointer"
                 onclick="batchTasksManager.useTemplate('${template.id}')">
                <div class="flex items-center mb-2">
                    <i class="fas fa-file-alt text-2xl text-blue-600 mr-3"></i>
                    <div>
                        <h4 class="font-medium text-gray-900">${template.name}</h4>
                        <p class="text-sm text-gray-500">${template.description}</p>
                    </div>
                </div>
                <button class="mt-2 w-full px-3 py-1 bg-blue-50 text-blue-700 rounded hover:bg-blue-100">
                    使用此模板
                </button>
            </div>
        `).join('');
    }

    getStatusClass(status) {
        const classes = {
            'pending': 'task-pending',
            'running': 'task-running',
            'completed': 'task-completed',
            'failed': 'task-failed',
            'cancelled': 'task-cancelled'
        };
        return classes[status] || 'bg-gray-100 text-gray-800';
    }

    getStatusBgClass(status) {
        const classes = {
            'running': 'border-blue-300',
            'completed': 'border-green-300',
            'failed': 'border-red-300',
            'pending': 'border-gray-300',
            'cancelled': 'border-yellow-300'
        };
        return classes[status] || '';
    }

    getStatusText(status) {
        const texts = {
            'pending': '等待中',
            'running': '运行中',
            'completed': '已完成',
            'failed': '失败',
            'cancelled': '已取消'
        };
        return texts[status] || status;
    }

    getTaskTypeText(type) {
        const texts = {
            'parse': '解析',
            'generate': '生成模型',
            'spatial': '空间树',
            'update': '更新'
        };
        return texts[type] || type;
    }

    showCreateModal() {
        document.getElementById('create-modal').classList.remove('hidden');
    }

    useTemplate(templateId) {
        const template = this.templates.find(t => t.id === templateId);
        if (template) {
            document.getElementById('task-type').value = template.taskType;
            document.getElementById('batch-name').value = template.name;
            this.showCreateModal();
        }
    }

    async createBatchTask() {
        const formData = {
            name: document.getElementById('batch-name').value,
            type: document.getElementById('task-type').value,
            dbNums: document.getElementById('db-nums').value
                .split(/[,\n]/)
                .map(num => num.trim())
                .filter(num => num)
                .map(num => parseInt(num)),
            concurrency: parseInt(document.getElementById('concurrency').value),
            autoStart: document.getElementById('auto-start').checked
        };

        if (!formData.name || formData.dbNums.length === 0) {
            alert('请填写任务名称和数据库编号');
            return;
        }

        try {
            const response = await fetch('/api/batch-tasks', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(formData)
            });

            if (response.ok) {
                alert('批量任务创建成功');
                this.closeCreateModal();
                await this.loadBatchTasks();
            } else {
                // 模拟创建成功
                alert('批量任务创建成功（模拟）');
                this.closeCreateModal();
                await this.loadBatchTasks();
            }
        } catch (error) {
            console.error('Failed to create batch task:', error);
            // 模拟创建成功
            alert('批量任务创建成功（模拟）');
            this.closeCreateModal();
            await this.loadBatchTasks();
        }
    }

    async startTask(taskId) {
        try {
            await fetch(`/api/batch-tasks/${taskId}/start`, { method: 'POST' });
            await this.loadBatchTasks();
        } catch (error) {
            console.error('Failed to start task:', error);
        }
    }

    async pauseTask(taskId) {
        try {
            await fetch(`/api/batch-tasks/${taskId}/pause`, { method: 'POST' });
            await this.loadBatchTasks();
        } catch (error) {
            console.error('Failed to pause task:', error);
        }
    }

    async stopTask(taskId) {
        if (confirm('确定要停止此批量任务吗？')) {
            try {
                await fetch(`/api/batch-tasks/${taskId}/stop`, { method: 'POST' });
                await this.loadBatchTasks();
            } catch (error) {
                console.error('Failed to stop task:', error);
            }
        }
    }

    viewTaskDetails(taskId) {
        const task = this.batchTasks.find(t => t.id === taskId);
        if (!task) return;

        document.getElementById('detail-title').textContent = `任务详情 - ${task.name}`;
        document.getElementById('detail-content').innerHTML = `
            <div class="space-y-4">
                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium text-gray-700">任务名称</label>
                        <p class="mt-1 text-sm text-gray-900">${task.name}</p>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700">任务类型</label>
                        <p class="mt-1 text-sm text-gray-900">${this.getTaskTypeText(task.type)}</p>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700">状态</label>
                        <span class="mt-1 inline-block px-2 py-1 text-xs rounded-full ${this.getStatusClass(task.status)}">
                            ${this.getStatusText(task.status)}
                        </span>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-gray-700">进度</label>
                        <p class="mt-1 text-sm text-gray-900">${task.completedTasks}/${task.totalTasks} (${task.progress}%)</p>
                    </div>
                </div>

                <div>
                    <label class="block text-sm font-medium text-gray-700 mb-2">数据库编号列表</label>
                    <div class="bg-gray-50 p-3 rounded-md">
                        <div class="flex flex-wrap gap-2">
                            ${task.dbNums.map(num => `
                                <span class="px-2 py-1 bg-white border rounded text-sm">${num}</span>
                            `).join('')}
                        </div>
                    </div>
                </div>

                ${task.status === 'running' || task.status === 'completed' ? `
                    <div>
                        <label class="block text-sm font-medium text-gray-700 mb-2">任务进度</label>
                        <div class="w-full bg-gray-200 rounded-full h-4">
                            <div class="bg-blue-600 h-4 rounded-full flex items-center justify-center text-xs text-white"
                                 style="width: ${task.progress}%">
                                ${task.progress}%
                            </div>
                        </div>
                    </div>
                ` : ''}

                <div class="grid grid-cols-2 gap-4 text-sm">
                    <div>
                        <label class="block font-medium text-gray-700">开始时间</label>
                        <p class="mt-1 text-gray-900">${task.startTime ? new Date(task.startTime).toLocaleString() : '未开始'}</p>
                    </div>
                    ${task.endTime ? `
                        <div>
                            <label class="block font-medium text-gray-700">结束时间</label>
                            <p class="mt-1 text-gray-900">${new Date(task.endTime).toLocaleString()}</p>
                        </div>
                    ` : ''}
                </div>
            </div>
        `;

        document.getElementById('detail-modal').classList.remove('hidden');
    }

    closeCreateModal() {
        document.getElementById('create-modal').classList.add('hidden');
        // 清空表单
        document.getElementById('create-batch-form').reset();
    }

    closeDetailModal() {
        document.getElementById('detail-modal').classList.add('hidden');
    }

    startAutoRefresh() {
        if (this.autoRefresh) {
            this.refreshInterval = setInterval(() => {
                this.loadBatchTasks();
            }, 5000); // 每5秒刷新一次
        }
    }

    stopAutoRefresh() {
        if (this.refreshInterval) {
            clearInterval(this.refreshInterval);
            this.refreshInterval = null;
        }
    }
}

// 全局函数供HTML调用
function closeCreateModal() {
    batchTasksManager.closeCreateModal();
}

function closeDetailModal() {
    batchTasksManager.closeDetailModal();
}

// 初始化
const batchTasksManager = new BatchTasksManager();