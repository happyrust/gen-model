// 数据库状态管理页面 JavaScript
class DatabaseStatusManager {
    constructor() {
        this.databases = [];
        this.selectedDbs = new Set();
        this.currentPage = 1;
        this.pageSize = 20;
        this.sortField = 'db_num';
        this.sortOrder = 'asc';
        this.filters = {
            module: '',
            status: '',
            needs_update: false
        };
        this.isLoading = false;
        this.autoRefreshEnabled = true;
        this.refreshInterval = null;
        this.runningTasks = new Map(); // 跟踪正在运行的任务
    }

    // 初始化
    async init() {
        this.bindEvents();
        this.setupAutoRefresh();
        await this.loadDatabases();
    }

    // 设置自动刷新
    setupAutoRefresh() {
        if (this.refreshInterval) {
            clearInterval(this.refreshInterval);
        }

        if (this.autoRefreshEnabled) {
            this.refreshInterval = setInterval(() => {
                if (!this.isLoading) {
                    this.loadDatabases(true); // 静默刷新
                }
            }, 15000); // 每15秒刷新
        }
    }

    // 绑定事件
    bindEvents() {
        // 刷新按钮
        const refreshBtn = document.getElementById('refresh-btn');
        if (refreshBtn) {
            refreshBtn.addEventListener('click', () => {
                this.loadDatabases();
                // 添加旋转动画
                refreshBtn.querySelector('i').classList.add('fa-spin');
                setTimeout(() => {
                    refreshBtn.querySelector('i').classList.remove('fa-spin');
                }, 1000);
            });
        }

        // 自动刷新切换
        const autoRefreshToggle = document.getElementById('auto-refresh-toggle');
        if (autoRefreshToggle) {
            autoRefreshToggle.addEventListener('change', (e) => {
                this.autoRefreshEnabled = e.target.checked;
                this.setupAutoRefresh();
            });
        }

        document.getElementById('batch-ops-btn')?.addEventListener('click', () => this.openBatchModal());
        document.getElementById('apply-filter')?.addEventListener('click', () => this.applyFilters());
        document.getElementById('clear-filter')?.addEventListener('click', () => this.clearFilters());
        document.getElementById('select-all')?.addEventListener('change', (e) => this.toggleSelectAll(e.target.checked));
        document.getElementById('prev-page')?.addEventListener('click', () => this.changePage(-1));
        document.getElementById('next-page')?.addEventListener('click', () => this.changePage(1));

        // 过滤器变化
        document.getElementById('filter-module').addEventListener('change', (e) => {
            this.filters.module = e.target.value;
        });
        document.getElementById('filter-status').addEventListener('change', (e) => {
            this.filters.status = e.target.value;
        });
        document.getElementById('filter-needs-update').addEventListener('change', (e) => {
            this.filters.needs_update = e.target.checked;
        });
    }

    // 加载数据库列表
    async loadDatabases(silent = false) {
        if (this.isLoading) return;

        this.isLoading = true;
        if (!silent) {
            this.showLoadingState();
        }

        try {
            const params = new URLSearchParams({
                page: this.currentPage,
                page_size: this.pageSize,
                sort_by: this.sortField,
                order: this.sortOrder
            });

            if (this.filters.module) params.append('module', this.filters.module);
            if (this.filters.status) params.append('status', this.filters.status);
            if (this.filters.needs_update) params.append('needs_update', 'true');

            const response = await fetch(`/api/database/status?${params}`);

            if (!response.ok) {
                throw new Error(`HTTP ${response.status}: ${response.statusText}`);
            }

            const data = await response.json();

            if (data.success) {
                this.databases = data.databases || [];
                this.updateStatistics(data.statistics || {});
                this.updatePagination(data.pagination || {});
                this.renderTable();
                this.updateLastRefreshTime();
            } else {
                throw new Error(data.message || '加载数据失败');
            }
        } catch (error) {
            console.error('加载数据库状态失败:', error);
            if (!silent) {
                this.showNotification(`加载失败: ${error.message}`, 'error');
                this.showErrorState(error.message);
            }
        } finally {
            this.isLoading = false;
            this.hideLoadingState();
        }
    }

    // 显示加载状态
    showLoadingState() {
        const tbody = document.getElementById('database-tbody');
        if (tbody && tbody.children.length === 0) {
            tbody.innerHTML = `
                <tr>
                    <td colspan="11" class="text-center py-8">
                        <i class="fas fa-spinner fa-spin text-4xl text-gray-400"></i>
                        <p class="mt-2 text-gray-500">正在加载数据...</p>
                    </td>
                </tr>
            `;
        }
    }

    // 隐藏加载状态
    hideLoadingState() {
        // 加载状态会被 renderTable 自动替换
    }

    // 显示错误状态
    showErrorState(message) {
        const tbody = document.getElementById('database-tbody');
        if (tbody && tbody.children.length === 0) {
            tbody.innerHTML = `
                <tr>
                    <td colspan="11" class="text-center py-8">
                        <i class="fas fa-exclamation-circle text-4xl text-red-400"></i>
                        <p class="mt-2 text-red-500">${message}</p>
                        <button onclick="dbStatusManager.loadDatabases()"
                                class="mt-3 px-4 py-2 bg-blue-600 text-white rounded hover:bg-blue-700">
                            <i class="fas fa-redo mr-2"></i>重试
                        </button>
                    </td>
                </tr>
            `;
        }
    }

    // 更新最后刷新时间
    updateLastRefreshTime() {
        const element = document.getElementById('last-refresh-time');
        if (element) {
            const now = new Date();
            element.textContent = now.toLocaleTimeString('zh-CN');
        }
    }

    // 渲染表格
    renderTable() {
        const tbody = document.getElementById('database-tbody');
        if (!tbody) return;

        tbody.innerHTML = '';

        if (this.databases.length === 0) {
            tbody.innerHTML = `
                <tr>
                    <td colspan="11" class="text-center py-8 text-gray-500">
                        <i class="fas fa-database text-4xl text-gray-300 mb-3"></i>
                        <p>暂无数据库记录</p>
                    </td>
                </tr>
            `;
            return;
        }

        this.databases.forEach(db => {
            const row = this.createTableRow(db);
            tbody.appendChild(row);
        });

        // 更新任务状态
        this.updateRunningTasksDisplay();
    }

    // 创建表格行
    createTableRow(db) {
        const row = document.createElement('tr');
        row.className = 'hover:bg-gray-50';
        row.dataset.dbNum = db.db_num;

        row.innerHTML = `
            <td class="px-4 py-3">
                <input type="checkbox" class="db-checkbox rounded" value="${db.db_num}"
                       ${this.selectedDbs.has(db.db_num) ? 'checked' : ''}>
            </td>
            <td class="px-4 py-3 font-medium text-gray-900">${db.db_num}</td>
            <td class="px-4 py-3 text-gray-700">${db.db_name}</td>
            <td class="px-4 py-3">
                <span class="module-badge module-${db.module}">${db.module}</span>
            </td>
            <td class="px-4 py-3 text-center">
                ${this.getStatusBadge(db.parse_status)}
            </td>
            <td class="px-4 py-3 text-center">
                ${this.getStatusBadge(db.model_status)}
            </td>
            <td class="px-4 py-3 text-center">
                ${this.getStatusBadge(db.spatial_tree_status)}
            </td>
            <td class="px-4 py-3 text-center">
                ${db.needs_update
                    ? '<span class="text-amber-600"><i class="fas fa-exclamation-circle"></i></span>'
                    : '<span class="text-gray-400"><i class="fas fa-check-circle"></i></span>'
                }
            </td>
            <td class="px-4 py-3 text-right text-gray-700">${db.file_size.toFixed(1)}</td>
            <td class="px-4 py-3 text-right text-gray-700">${db.element_count.toLocaleString()}</td>
            <td class="px-4 py-3 text-center">
                <div class="flex justify-center space-x-1">
                    ${this.getActionButtons(db)}
                </div>
            </td>
        `;

        // 绑定复选框事件
        row.querySelector('.db-checkbox').addEventListener('change', (e) => {
            this.toggleSelection(db.db_num, e.target.checked);
        });

        return row;
    }

    // 获取状态徽章
    getStatusBadge(status) {
        const statusMap = {
            'notstarted': { text: '未开始', icon: 'fa-circle', class: 'status-notstarted' },
            'inprogress': { text: '处理中', icon: 'fa-spinner fa-spin', class: 'status-inprogress' },
            'completed': { text: '完成', icon: 'fa-check-circle', class: 'status-completed' },
            'failed': { text: '失败', icon: 'fa-times-circle', class: 'status-failed' },
            'outdated': { text: '过期', icon: 'fa-clock', class: 'status-outdated' }
        };

        const info = statusMap[status.toLowerCase()] || statusMap['notstarted'];
        return `
            <span class="inline-flex items-center px-2 py-1 rounded text-xs font-medium ${info.class}">
                <i class="fas ${info.icon} mr-1"></i>${info.text}
            </span>
        `;
    }

    // 获取操作按钮
    getActionButtons(db) {
        const buttons = [];

        // 查看详情
        buttons.push(`
            <button onclick="dbStatusManager.showDetails(${db.db_num})"
                    class="p-1 text-blue-600 hover:text-blue-800" title="查看详情">
                <i class="fas fa-info-circle"></i>
            </button>
        `);

        // 根据状态显示不同操作
        if (db.parse_status === 'notstarted' || db.parse_status === 'failed') {
            buttons.push(`
                <button onclick="dbStatusManager.parseDatabase(${db.db_num})"
                        class="p-1 text-green-600 hover:text-green-800" title="解析">
                    <i class="fas fa-file-import"></i>
                </button>
            `);
        }

        if (db.model_status !== 'completed' && db.parse_status === 'completed') {
            buttons.push(`
                <button onclick="dbStatusManager.generateModel(${db.db_num})"
                        class="p-1 text-blue-600 hover:text-blue-800" title="生成模型">
                    <i class="fas fa-cube"></i>
                </button>
            `);
        }

        if (db.needs_update) {
            buttons.push(`
                <button onclick="dbStatusManager.updateDatabase(${db.db_num})"
                        class="p-1 text-amber-600 hover:text-amber-800" title="更新">
                    <i class="fas fa-sync"></i>
                </button>
            `);
        }

        // 清理缓存
        buttons.push(`
            <button onclick="dbStatusManager.clearCache(${db.db_num})"
                    class="p-1 text-red-600 hover:text-red-800" title="清理缓存">
                <i class="fas fa-trash"></i>
            </button>
        `);

        return buttons.join('');
    }

    // 更新统计信息
    updateStatistics(stats) {
        // 安全更新统计信息
        const updateStat = (id, value) => {
            const element = document.getElementById(id);
            if (element) {
                const currentValue = parseInt(element.textContent) || 0;
                const newValue = value || 0;

                // 添加数字变化动画
                if (currentValue !== newValue) {
                    element.classList.add('stat-change');
                    element.textContent = newValue;
                    setTimeout(() => element.classList.remove('stat-change'), 500);
                } else {
                    element.textContent = newValue;
                }
            }
        };

        updateStat('stat-total', stats.total);
        updateStat('stat-parsed', stats.parsed);
        updateStat('stat-generated', stats.generated);
        updateStat('stat-needs-update', stats.needs_update);
        updateStat('stat-failed', stats.failed);
    }

    // 更新分页信息
    updatePagination(pagination) {
        const start = (pagination.page - 1) * pagination.page_size + 1;
        const end = Math.min(start + pagination.page_size - 1, pagination.total);

        document.getElementById('page-info').textContent = `${start}-${end}`;
        document.getElementById('total-count').textContent = pagination.total;
        document.getElementById('current-page').textContent = pagination.page;
        document.getElementById('total-pages').textContent = pagination.total_pages;

        // 更新按钮状态
        document.getElementById('prev-page').disabled = pagination.page <= 1;
        document.getElementById('next-page').disabled = pagination.page >= pagination.total_pages;
    }

    // 切换选择
    toggleSelection(dbNum, selected) {
        if (selected) {
            this.selectedDbs.add(dbNum);
        } else {
            this.selectedDbs.delete(dbNum);
        }
        this.updateSelectionUI();
    }

    // 全选/取消全选
    toggleSelectAll(selected) {
        const checkboxes = document.querySelectorAll('.db-checkbox');
        checkboxes.forEach(cb => {
            cb.checked = selected;
            const dbNum = parseInt(cb.value);
            if (selected) {
                this.selectedDbs.add(dbNum);
            } else {
                this.selectedDbs.delete(dbNum);
            }
        });
        this.updateSelectionUI();
    }

    // 更新选择UI
    updateSelectionUI() {
        document.getElementById('selected-count').textContent = this.selectedDbs.size;
    }

    // 排序
    sortBy(field) {
        if (this.sortField === field) {
            this.sortOrder = this.sortOrder === 'asc' ? 'desc' : 'asc';
        } else {
            this.sortField = field;
            this.sortOrder = 'asc';
        }
        this.loadDatabases();
    }

    // 应用过滤
    applyFilters() {
        this.currentPage = 1;
        this.loadDatabases();
    }

    // 清除过滤
    clearFilters() {
        this.filters = {
            module: '',
            status: '',
            needs_update: false
        };
        document.getElementById('filter-module').value = '';
        document.getElementById('filter-status').value = '';
        document.getElementById('filter-needs-update').checked = false;
        this.currentPage = 1;
        this.loadDatabases();
    }

    // 切换页面
    changePage(delta) {
        this.currentPage += delta;
        this.loadDatabases();
    }

    // 显示详情
    async showDetails(dbNum) {
        try {
            const response = await fetch(`/api/database/${dbNum}/details`);
            const data = await response.json();

            if (data.success) {
                this.renderDetailModal(data.database, data.history, data.files);
                document.getElementById('detail-modal').classList.remove('hidden');
            }
        } catch (error) {
            console.error('加载详情失败:', error);
            this.showNotification('加载详情失败', 'error');
        }
    }

    // 渲染详情模态框
    renderDetailModal(database, history, files) {
        const modalTitle = document.getElementById('detail-title');
        const modalContent = document.getElementById('detail-content');

        modalTitle.textContent = `数据库 ${database.db_num} - ${database.db_name}`;

        modalContent.innerHTML = `
            <div class="grid grid-cols-2 gap-6">
                <div>
                    <h4 class="font-semibold text-gray-900 mb-3">基本信息</h4>
                    <dl class="space-y-2 text-sm">
                        <div class="flex justify-between">
                            <dt class="text-gray-500">模块:</dt>
                            <dd class="font-medium">${database.module}</dd>
                        </div>
                        <div class="flex justify-between">
                            <dt class="text-gray-500">文件大小:</dt>
                            <dd class="font-medium">${database.file_size.toFixed(1)} MB</dd>
                        </div>
                        <div class="flex justify-between">
                            <dt class="text-gray-500">元素数量:</dt>
                            <dd class="font-medium">${database.element_count.toLocaleString()}</dd>
                        </div>
                        <div class="flex justify-between">
                            <dt class="text-gray-500">三角面数:</dt>
                            <dd class="font-medium">${database.triangle_count.toLocaleString()}</dd>
                        </div>
                    </dl>
                </div>
                <div>
                    <h4 class="font-semibold text-gray-900 mb-3">处理状态</h4>
                    <dl class="space-y-2 text-sm">
                        <div class="flex justify-between items-center">
                            <dt class="text-gray-500">解析状态:</dt>
                            <dd>${this.getStatusBadge(database.parse_status)}</dd>
                        </div>
                        <div class="flex justify-between items-center">
                            <dt class="text-gray-500">模型状态:</dt>
                            <dd>${this.getStatusBadge(database.model_status)}</dd>
                        </div>
                        <div class="flex justify-between items-center">
                            <dt class="text-gray-500">空间树:</dt>
                            <dd>${this.getStatusBadge(database.spatial_tree_status)}</dd>
                        </div>
                        <div class="flex justify-between items-center">
                            <dt class="text-gray-500">需要更新:</dt>
                            <dd>${database.needs_update ? '<span class="text-amber-600">是</span>' : '<span class="text-green-600">否</span>'}</dd>
                        </div>
                    </dl>
                </div>
            </div>

            ${database.error_message ? `
                <div class="mt-4 p-3 bg-red-50 border border-red-200 rounded">
                    <p class="text-sm text-red-700">
                        <i class="fas fa-exclamation-circle mr-2"></i>
                        ${database.error_message}
                    </p>
                </div>
            ` : ''}

            <div class="mt-6">
                <h4 class="font-semibold text-gray-900 mb-3">处理历史</h4>
                <div class="overflow-x-auto">
                    <table class="w-full text-sm">
                        <thead class="bg-gray-50">
                            <tr>
                                <th class="px-3 py-2 text-left">时间</th>
                                <th class="px-3 py-2 text-left">操作</th>
                                <th class="px-3 py-2 text-left">状态</th>
                                <th class="px-3 py-2 text-left">耗时</th>
                                <th class="px-3 py-2 text-left">说明</th>
                            </tr>
                        </thead>
                        <tbody class="divide-y">
                            ${history.map(record => `
                                <tr>
                                    <td class="px-3 py-2">${this.formatTime(record.time)}</td>
                                    <td class="px-3 py-2">${record.action}</td>
                                    <td class="px-3 py-2">
                                        <span class="px-2 py-1 text-xs rounded ${record.status === 'completed' ? 'bg-green-100 text-green-700' : 'bg-red-100 text-red-700'}">
                                            ${record.status}
                                        </span>
                                    </td>
                                    <td class="px-3 py-2">${record.duration}s</td>
                                    <td class="px-3 py-2 text-gray-600">${record.message}</td>
                                </tr>
                            `).join('')}
                        </tbody>
                    </table>
                </div>
            </div>

            <div class="mt-6 flex justify-end space-x-2">
                <button onclick="dbStatusManager.parseDatabase(${database.db_num})"
                        class="px-4 py-2 bg-green-600 text-white rounded hover:bg-green-700">
                    <i class="fas fa-file-import mr-2"></i>重新解析
                </button>
                <button onclick="dbStatusManager.generateModel(${database.db_num})"
                        class="px-4 py-2 bg-blue-600 text-white rounded hover:bg-blue-700">
                    <i class="fas fa-cube mr-2"></i>重新生成
                </button>
                <button onclick="dbStatusManager.clearCache(${database.db_num})"
                        class="px-4 py-2 bg-red-600 text-white rounded hover:bg-red-700">
                    <i class="fas fa-trash mr-2"></i>清理缓存
                </button>
            </div>
        `;
    }

    // 打开批量操作模态框
    openBatchModal() {
        if (this.selectedDbs.size === 0) {
            this.showNotification('请先选择要操作的数据库', 'warning');
            return;
        }
        document.getElementById('batch-modal').classList.remove('hidden');
        document.getElementById('selected-count').textContent = this.selectedDbs.size;
    }

    // 单个数据库操作
    async parseDatabase(dbNum) {
        if (!confirm(`确定要解析数据库 ${dbNum} 吗？`)) return;
        this.setTaskRunning(dbNum, 'parse');
        await this.callDatabaseApi(`/api/database/${dbNum}/parse`, 'POST', '解析', dbNum);
    }

    async generateModel(dbNum) {
        if (!confirm(`确定要生成数据库 ${dbNum} 的模型吗？`)) return;
        this.setTaskRunning(dbNum, 'generate');
        await this.callDatabaseApi(`/api/database/${dbNum}/generate`, 'POST', '模型生成', dbNum);
    }

    async updateDatabase(dbNum) {
        if (!confirm(`确定要更新数据库 ${dbNum} 吗？`)) return;
        this.setTaskRunning(dbNum, 'update');
        await this.callDatabaseApi(`/api/database/${dbNum}/update`, 'POST', '更新', dbNum);
    }

    async clearCache(dbNum) {
        if (!confirm(`确定要清理数据库 ${dbNum} 的缓存吗？`)) return;
        this.setTaskRunning(dbNum, 'clear');
        await this.callDatabaseApi(`/api/database/${dbNum}/clear-cache`, 'POST', '缓存清理', dbNum);
    }

    // 设置任务运行状态
    setTaskRunning(dbNum, taskType) {
        this.runningTasks.set(`${dbNum}-${taskType}`, true);
        this.updateRunningTasksDisplay();
    }

    // 清除任务运行状态
    clearTaskRunning(dbNum, taskType) {
        this.runningTasks.delete(`${dbNum}-${taskType}`);
        this.updateRunningTasksDisplay();
    }

    // 更新运行中任务的显示
    updateRunningTasksDisplay() {
        this.runningTasks.forEach((_, key) => {
            const [dbNum, taskType] = key.split('-');
            const row = document.querySelector(`tr[data-db-num="${dbNum}"]`);
            if (row) {
                const actionCell = row.querySelector('td:last-child');
                if (actionCell && !actionCell.querySelector('.task-spinner')) {
                    const spinner = document.createElement('span');
                    spinner.className = 'task-spinner ml-2';
                    spinner.innerHTML = '<i class="fas fa-spinner fa-spin text-blue-600"></i>';
                    actionCell.appendChild(spinner);
                }
            }
        });
    }

    // 调用数据库API
    async callDatabaseApi(url, method, action, dbNum = null) {
        try {
            const response = await fetch(url, {
                method,
                headers: {
                    'Content-Type': 'application/json'
                }
            });

            if (!response.ok) {
                throw new Error(`HTTP ${response.status}: ${response.statusText}`);
            }

            const data = await response.json();

            if (data.success) {
                this.showNotification(`${action}任务已启动`, 'success');
                // 立即刷新以显示最新状态
                setTimeout(() => this.loadDatabases(true), 1000);

                // 开始轮询任务状态
                if (data.task_id) {
                    this.pollTaskStatus(data.task_id, action, dbNum);
                }
            } else {
                throw new Error(data.message || `${action}失败`);
            }
        } catch (error) {
            console.error(`${action}失败:`, error);
            this.showNotification(`${action}失败: ${error.message}`, 'error');
        } finally {
            if (dbNum) {
                // 清除任务运行状态
                const taskType = this.getTaskTypeFromAction(action);
                if (taskType) {
                    setTimeout(() => this.clearTaskRunning(dbNum, taskType), 2000);
                }
            }
        }
    }

    // 轮询任务状态
    async pollTaskStatus(taskId, action, dbNum) {
        const maxAttempts = 60; // 最多轮询60次
        let attempts = 0;

        const poll = async () => {
            if (attempts >= maxAttempts) {
                this.showNotification(`${action}任务超时`, 'warning');
                return;
            }

            try {
                const response = await fetch(`/api/task/${taskId}/status`);
                if (response.ok) {
                    const data = await response.json();
                    if (data.status === 'completed') {
                        this.showNotification(`${action}任务完成`, 'success');
                        this.loadDatabases(true);
                        return;
                    } else if (data.status === 'failed') {
                        this.showNotification(`${action}任务失败: ${data.error}`, 'error');
                        this.loadDatabases(true);
                        return;
                    }
                }
            } catch (error) {
                console.error('轮询任务状态失败:', error);
            }

            attempts++;
            setTimeout(poll, 2000); // 2秒后再次轮询
        };

        poll();
    }

    // 根据操作获取任务类型
    getTaskTypeFromAction(action) {
        const actionMap = {
            '解析': 'parse',
            '模型生成': 'generate',
            '更新': 'update',
            '缓存清理': 'clear'
        };
        return actionMap[action] || null;
    }

    // 工具方法
    formatTime(timeStr) {
        if (!timeStr) return '--';
        const date = new Date(timeStr);
        return date.toLocaleDateString('zh-CN') + ' ' + date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
    }

    showNotification(message, type = 'info') {
        const colors = {
            'success': 'bg-green-500',
            'error': 'bg-red-500',
            'warning': 'bg-amber-500',
            'info': 'bg-blue-500'
        };

        const notification = document.createElement('div');
        notification.className = `fixed top-4 right-4 px-4 py-2 text-white rounded shadow-lg ${colors[type]} z-50`;
        notification.textContent = message;
        document.body.appendChild(notification);

        setTimeout(() => {
            notification.remove();
        }, 3000);
    }
}

// 全局方法
function closeDetailModal() {
    document.getElementById('detail-modal').classList.add('hidden');
}

function closeBatchModal() {
    document.getElementById('batch-modal').classList.add('hidden');
}

async function executeBatchOperation(operation) {
    if (!confirm(`确定要对选中的数据库执行批量${operation}操作吗？`)) return;

    try {
        const response = await fetch('/api/database/batch', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                db_nums: Array.from(dbStatusManager.selectedDbs),
                operation: operation
            })
        });

        const data = await response.json();
        if (data.success) {
            dbStatusManager.showNotification('批量操作已启动', 'success');
            closeBatchModal();
            dbStatusManager.selectedDbs.clear();
            await dbStatusManager.loadDatabases();
        }
    } catch (error) {
        console.error('批量操作失败:', error);
        dbStatusManager.showNotification('批量操作失败', 'error');
    }
}

// 初始化
const dbStatusManager = new DatabaseStatusManager();
document.addEventListener('DOMContentLoaded', () => {
    dbStatusManager.init();
});