// 增量更新检测页面 JavaScript
class IncrementalUpdateManager {
    constructor() {
        this.sites = [];
        this.refreshInterval = null;
        this.activeDetections = new Map();
    }

    // 初始化
    async init() {
        await this.loadSites();
        await this.loadConfig();
        this.bindEvents();
        this.startAutoRefresh();
    }

    // 绑定事件
    bindEvents() {
        document.getElementById('refresh-all').addEventListener('click', () => this.loadSites());
        document.getElementById('config-btn').addEventListener('click', () => this.openConfigModal());
    }

    // 加载站点状态
    async loadSites() {
        try {
            const response = await fetch('/api/incremental/status');
            const data = await response.json();

            if (data.success) {
                this.sites = data.sites;
                this.updateStats(data);
                this.renderSites();
            }
        } catch (error) {
            console.error('加载站点状态失败:', error);
            this.showNotification('加载失败', 'error');
        }
    }

    // 更新统计信息
    updateStats(data) {
        document.getElementById('total-sites').textContent = data.sites.length;
        document.getElementById('total-pending').textContent = data.total_pending;
        document.getElementById('total-synced').textContent = data.total_synced;
        document.getElementById('last-check').textContent = this.formatTime(data.last_check);
    }

    // 渲染站点列表
    renderSites() {
        const container = document.getElementById('sites-container');
        container.innerHTML = '';

        this.sites.forEach(site => {
            const card = this.createSiteCard(site);
            container.appendChild(card);
        });
    }

    // 创建站点卡片
    createSiteCard(site) {
        const card = document.createElement('div');
        card.className = 'border rounded-lg p-4 hover:shadow-lg transition-shadow';

        const statusClass = this.getStatusClass(site.detection_status);
        const statusIcon = this.getStatusIcon(site.detection_status);
        const isScanning = site.detection_status === 'Scanning';

        card.innerHTML = `
            <div class="flex justify-between items-start">
                <div class="flex-1">
                    <div class="flex items-center mb-2">
                        <h3 class="text-lg font-semibold text-gray-900">${site.site_name}</h3>
                        <span class="ml-3 px-2 py-1 text-xs rounded-full ${statusClass} ${isScanning ? 'scanning-animation' : ''}">
                            <i class="${statusIcon} mr-1"></i>
                            ${this.getStatusText(site.detection_status)}
                        </span>
                    </div>
                    <div class="grid grid-cols-2 md:grid-cols-4 gap-4 text-sm">
                        <div>
                            <span class="text-gray-500">上次同步:</span>
                            <span class="ml-1 text-gray-700">${this.formatTime(site.last_sync_time)}</span>
                        </div>
                        <div>
                            <span class="text-gray-500">待同步:</span>
                            <span class="ml-1 font-medium text-amber-600">${site.pending_items}</span>
                        </div>
                        <div>
                            <span class="text-gray-500">已同步:</span>
                            <span class="ml-1 font-medium text-green-600">${site.synced_items}</span>
                        </div>
                        <div>
                            <span class="text-gray-500">增量大小:</span>
                            <span class="ml-1 text-gray-700">${this.formatSize(site.increment_size)}</span>
                        </div>
                    </div>
                    ${site.changed_files.length > 0 ? this.renderChangedFiles(site.changed_files) : ''}
                </div>
                <div class="flex flex-col space-y-2 ml-4">
                    ${this.renderActionButtons(site)}
                </div>
            </div>
            ${site.detection_status === 'Syncing' ? this.renderProgressBar(site) : ''}
        `;

        return card;
    }

    // 渲染变更文件列表
    renderChangedFiles(files) {
        const displayFiles = files.slice(0, 3);
        const moreCount = files.length - 3;

        return `
            <div class="mt-3 pt-3 border-t">
                <div class="text-sm text-gray-600 mb-2">最近变更:</div>
                <div class="space-y-1">
                    ${displayFiles.map(file => `
                        <div class="flex items-center text-xs">
                            <span class="px-2 py-0.5 rounded ${this.getChangeTypeClass(file.change_type)}">
                                ${this.getChangeTypeIcon(file.change_type)}
                            </span>
                            <span class="ml-2 text-gray-700 truncate">${file.path}</span>
                            <span class="ml-auto text-gray-500">${this.formatSize(file.size)}</span>
                        </div>
                    `).join('')}
                    ${moreCount > 0 ? `<div class="text-xs text-gray-500">还有 ${moreCount} 个文件...</div>` : ''}
                </div>
            </div>
        `;
    }

    // 渲染操作按钮
    renderActionButtons(site) {
        const buttons = [];

        switch (site.detection_status) {
            case 'Idle':
                buttons.push(`
                    <button onclick="incrementalManager.startDetection('${site.site_id}')"
                            class="px-3 py-1 text-sm bg-blue-600 text-white rounded hover:bg-blue-700">
                        <i class="fas fa-search mr-1"></i>检测
                    </button>
                `);
                break;
            case 'ChangesDetected':
                buttons.push(`
                    <button onclick="incrementalManager.startSync('${site.site_id}')"
                            class="px-3 py-1 text-sm bg-green-600 text-white rounded hover:bg-green-700">
                        <i class="fas fa-sync mr-1"></i>同步
                    </button>
                `);
                buttons.push(`
                    <button onclick="incrementalManager.showDetails('${site.site_id}')"
                            class="px-3 py-1 text-sm bg-gray-600 text-white rounded hover:bg-gray-700">
                        <i class="fas fa-info-circle mr-1"></i>详情
                    </button>
                `);
                break;
            case 'Scanning':
            case 'Syncing':
                buttons.push(`
                    <button onclick="incrementalManager.cancelTask('${site.site_id}')"
                            class="px-3 py-1 text-sm bg-red-600 text-white rounded hover:bg-red-700">
                        <i class="fas fa-stop mr-1"></i>停止
                    </button>
                `);
                break;
            case 'Completed':
                buttons.push(`
                    <button onclick="incrementalManager.showDetails('${site.site_id}')"
                            class="px-3 py-1 text-sm bg-gray-600 text-white rounded hover:bg-gray-700">
                        <i class="fas fa-check-circle mr-1"></i>查看
                    </button>
                `);
                break;
        }

        return buttons.join('');
    }

    // 渲染进度条
    renderProgressBar(site) {
        const progress = Math.round((site.synced_items / (site.synced_items + site.pending_items)) * 100);
        return `
            <div class="mt-4">
                <div class="flex justify-between text-sm text-gray-600 mb-1">
                    <span>同步进度</span>
                    <span>${progress}%</span>
                </div>
                <div class="w-full bg-gray-200 rounded-full h-2">
                    <div class="bg-green-600 h-2 rounded-full transition-all duration-300" style="width: ${progress}%"></div>
                </div>
                <div class="mt-1 text-xs text-gray-500">
                    预计剩余时间: ${this.formatDuration(site.estimated_sync_time)}
                </div>
            </div>
        `;
    }

    // 启动检测
    async startDetection(siteId) {
        try {
            const response = await fetch(`/api/incremental/detect/${siteId}`, { method: 'POST' });
            const data = await response.json();

            if (data.success) {
                this.showNotification('已启动检测', 'success');
                this.activeDetections.set(siteId, data.task_id);
                await this.loadSites();
            }
        } catch (error) {
            console.error('启动检测失败:', error);
            this.showNotification('启动检测失败', 'error');
        }
    }

    // 启动同步
    async startSync(siteId) {
        try {
            const response = await fetch(`/api/incremental/sync/${siteId}`, { method: 'POST' });
            const data = await response.json();

            if (data.success) {
                this.showNotification('已启动同步', 'success');
                await this.loadSites();
            }
        } catch (error) {
            console.error('启动同步失败:', error);
            this.showNotification('启动同步失败', 'error');
        }
    }

    // 取消任务
    async cancelTask(siteId) {
        if (!confirm('确定要取消当前任务吗？')) return;

        const taskId = this.activeDetections.get(siteId);
        if (!taskId) return;

        try {
            const response = await fetch(`/api/incremental/task/${taskId}/cancel`, { method: 'POST' });
            const data = await response.json();

            if (data.success) {
                this.showNotification('任务已取消', 'info');
                this.activeDetections.delete(siteId);
                await this.loadSites();
            }
        } catch (error) {
            console.error('取消任务失败:', error);
            this.showNotification('取消任务失败', 'error');
        }
    }

    // 显示站点详情
    async showDetails(siteId) {
        try {
            const response = await fetch(`/api/incremental/site/${siteId}`);
            const data = await response.json();

            if (data.success) {
                this.renderDetailModal(data.site, data.sync_history);
                document.getElementById('detail-modal').classList.remove('hidden');
            }
        } catch (error) {
            console.error('加载详情失败:', error);
            this.showNotification('加载详情失败', 'error');
        }
    }

    // 渲染详情模态框
    renderDetailModal(site, history) {
        const modalTitle = document.getElementById('modal-title');
        const modalContent = document.getElementById('modal-content');

        modalTitle.textContent = `${site.site_name} - 增量更新详情`;

        modalContent.innerHTML = `
            <div class="space-y-4">
                <!-- 变更文件列表 -->
                <div>
                    <h4 class="font-semibold text-gray-900 mb-2">变更文件 (${site.changed_files.length})</h4>
                    <div class="max-h-60 overflow-y-auto border rounded p-2">
                        <table class="w-full text-sm">
                            <thead>
                                <tr class="border-b">
                                    <th class="text-left py-1">类型</th>
                                    <th class="text-left py-1">路径</th>
                                    <th class="text-right py-1">大小</th>
                                    <th class="text-right py-1">修改时间</th>
                                </tr>
                            </thead>
                            <tbody>
                                ${site.changed_files.map(file => `
                                    <tr class="border-b hover:bg-gray-50">
                                        <td class="py-1">
                                            <span class="px-2 py-0.5 text-xs rounded ${this.getChangeTypeClass(file.change_type)}">
                                                ${file.change_type}
                                            </span>
                                        </td>
                                        <td class="py-1 text-gray-700">${file.path}</td>
                                        <td class="py-1 text-right text-gray-600">${this.formatSize(file.size)}</td>
                                        <td class="py-1 text-right text-gray-600">${this.formatTime(file.modified_time)}</td>
                                    </tr>
                                `).join('')}
                            </tbody>
                        </table>
                    </div>
                </div>

                <!-- 同步历史 -->
                <div>
                    <h4 class="font-semibold text-gray-900 mb-2">同步历史</h4>
                    <div class="border rounded p-2">
                        <table class="w-full text-sm">
                            <thead>
                                <tr class="border-b">
                                    <th class="text-left py-1">时间</th>
                                    <th class="text-right py-1">同步项</th>
                                    <th class="text-right py-1">数据量</th>
                                    <th class="text-right py-1">耗时</th>
                                    <th class="text-center py-1">状态</th>
                                </tr>
                            </thead>
                            <tbody>
                                ${history.map(record => `
                                    <tr class="border-b hover:bg-gray-50">
                                        <td class="py-1 text-gray-700">${this.formatTime(record.time)}</td>
                                        <td class="py-1 text-right">${record.items_synced}</td>
                                        <td class="py-1 text-right">${this.formatSize(record.size)}</td>
                                        <td class="py-1 text-right">${this.formatDuration(record.duration)}</td>
                                        <td class="py-1 text-center">
                                            <span class="px-2 py-0.5 text-xs rounded bg-green-100 text-green-800">
                                                ${record.status}
                                            </span>
                                        </td>
                                    </tr>
                                `).join('')}
                            </tbody>
                        </table>
                    </div>
                </div>
            </div>
        `;
    }

    // 加载配置
    async loadConfig() {
        try {
            const response = await fetch('/api/incremental/config');
            const data = await response.json();

            if (data.success) {
                const config = data.config;
                document.getElementById('auto-detect').checked = config.auto_detect;
                document.getElementById('detect-interval').value = config.detect_interval;
                document.getElementById('auto-sync').checked = config.auto_sync;
                document.getElementById('sync-batch-size').value = config.sync_batch_size;
                document.getElementById('notification-enabled').checked = config.notification_enabled;
            }
        } catch (error) {
            console.error('加载配置失败:', error);
        }
    }

    // 开启自动刷新
    startAutoRefresh() {
        // 每30秒刷新一次
        this.refreshInterval = setInterval(() => {
            this.loadSites();
        }, 30000);
    }

    // 停止自动刷新
    stopAutoRefresh() {
        if (this.refreshInterval) {
            clearInterval(this.refreshInterval);
            this.refreshInterval = null;
        }
    }

    // 工具方法
    getStatusClass(status) {
        const classes = {
            'Idle': 'bg-gray-100 text-gray-700',
            'Scanning': 'bg-blue-100 text-blue-700',
            'ChangesDetected': 'bg-amber-100 text-amber-700',
            'Syncing': 'bg-green-100 text-green-700',
            'Completed': 'bg-green-100 text-green-700',
            'Error': 'bg-red-100 text-red-700'
        };
        return classes[status] || classes['Idle'];
    }

    getStatusIcon(status) {
        const icons = {
            'Idle': 'fas fa-circle',
            'Scanning': 'fas fa-spinner fa-spin',
            'ChangesDetected': 'fas fa-exclamation-circle',
            'Syncing': 'fas fa-sync fa-spin',
            'Completed': 'fas fa-check-circle',
            'Error': 'fas fa-times-circle'
        };
        return icons[status] || icons['Idle'];
    }

    getStatusText(status) {
        const texts = {
            'Idle': '空闲',
            'Scanning': '扫描中',
            'ChangesDetected': '发现变更',
            'Syncing': '同步中',
            'Completed': '已完成',
            'Error': '错误'
        };
        return texts[status] || status;
    }

    getChangeTypeClass(type) {
        const classes = {
            'added': 'change-added',
            'modified': 'change-modified',
            'deleted': 'change-deleted'
        };
        return classes[type.toLowerCase()] || '';
    }

    getChangeTypeIcon(type) {
        const icons = {
            'added': '+',
            'modified': '~',
            'deleted': '-'
        };
        return icons[type.toLowerCase()] || '?';
    }

    formatTime(timeStr) {
        if (!timeStr) return '--';
        const date = new Date(timeStr);
        const now = new Date();
        const diff = now - date;

        if (diff < 60000) return '刚刚';
        if (diff < 3600000) return `${Math.floor(diff / 60000)} 分钟前`;
        if (diff < 86400000) return `${Math.floor(diff / 3600000)} 小时前`;

        return date.toLocaleDateString('zh-CN') + ' ' + date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
    }

    formatSize(bytes) {
        if (!bytes) return '0 B';
        const k = 1024;
        const sizes = ['B', 'KB', 'MB', 'GB'];
        const i = Math.floor(Math.log(bytes) / Math.log(k));
        return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
    }

    formatDuration(seconds) {
        if (!seconds) return '--';
        if (seconds < 60) return `${seconds} 秒`;
        if (seconds < 3600) return `${Math.floor(seconds / 60)} 分钟`;
        return `${Math.floor(seconds / 3600)} 小时 ${Math.floor((seconds % 3600) / 60)} 分钟`;
    }

    showNotification(message, type = 'info') {
        // 简单的通知实现
        const colors = {
            'success': 'bg-green-500',
            'error': 'bg-red-500',
            'info': 'bg-blue-500',
            'warning': 'bg-amber-500'
        };

        const notification = document.createElement('div');
        notification.className = `fixed top-4 right-4 px-4 py-2 text-white rounded shadow-lg ${colors[type]} z-900`;
        notification.textContent = message;
        document.body.appendChild(notification);

        setTimeout(() => {
            notification.remove();
        }, 3000);
    }

    openConfigModal() {
        document.getElementById('config-modal').classList.remove('hidden');
    }
}

// 全局方法
function closeDetailModal() {
    document.getElementById('detail-modal').classList.add('hidden');
}

function closeConfigModal() {
    document.getElementById('config-modal').classList.add('hidden');
}

async function saveConfig() {
    const config = {
        auto_detect: document.getElementById('auto-detect').checked,
        detect_interval: parseInt(document.getElementById('detect-interval').value),
        auto_sync: document.getElementById('auto-sync').checked,
        sync_batch_size: parseInt(document.getElementById('sync-batch-size').value),
        notification_enabled: document.getElementById('notification-enabled').checked
    };

    try {
        const response = await fetch('/api/incremental/config', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(config)
        });

        const data = await response.json();
        if (data.success) {
            incrementalManager.showNotification('配置已保存', 'success');
            closeConfigModal();
        }
    } catch (error) {
        console.error('保存配置失败:', error);
        incrementalManager.showNotification('保存配置失败', 'error');
    }
}

// 初始化
const incrementalManager = new IncrementalUpdateManager();
document.addEventListener('DOMContentLoaded', () => {
    incrementalManager.init();
});