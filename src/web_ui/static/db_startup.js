// 数据库启动管理
class DbStartupManager {
    constructor() {
        this.statusCheckInterval = null;
        this.currentInstance = null;
    }

    // 启动数据库
    async startDatabase(config) {
        const { ip, port, user, password, dbFile } = config;
        const instanceKey = `${ip}:${port}`;
        
        // 先检查状态
        const status = await this.checkStatus(ip, port);
        if (status && (status.status === 'Starting' || status.status === 'Running')) {
            if (status.status === 'Starting') {
                alert('数据库正在启动中，请稍候...');
            } else {
                alert('数据库已经在运行中');
            }
            return false;
        }

        // 禁用启动按钮
        this.disableStartButton();
        this.showProgress('准备启动数据库...');

        try {
            // 发送启动请求
            const response = await fetch('/api/database/startup/start', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ ip, port, user, password, dbFile })
            });

            if (!response.ok) {
                const error = await response.text();
                throw new Error(error);
            }

            const result = await response.json();
            
            // 开始监控启动进度
            this.currentInstance = instanceKey;
            this.startStatusMonitoring(ip, port);
            
            return true;
        } catch (error) {
            console.error('启动失败:', error);
            this.showError(`启动失败: ${error.message}`);
            this.enableStartButton();
            return false;
        }
    }

    // 检查数据库状态
    async checkStatus(ip, port) {
        try {
            const response = await fetch(`/api/database/startup/status?ip=${ip}&port=${port}`);
            if (response.ok) {
                return await response.json();
            }
        } catch (error) {
            console.error('检查状态失败:', error);
        }
        return null;
    }

    // 开始监控状态
    startStatusMonitoring(ip, port) {
        // 清除之前的监控
        if (this.statusCheckInterval) {
            clearInterval(this.statusCheckInterval);
        }

        // 每秒检查一次状态
        this.statusCheckInterval = setInterval(async () => {
            const status = await this.checkStatus(ip, port);
            
            if (!status) {
                this.stopStatusMonitoring();
                return;
            }

            // 更新进度显示
            this.updateProgress(status);

            // 根据状态处理
            switch (status.status) {
                case 'Running':
                    this.onStartupSuccess(status);
                    this.stopStatusMonitoring();
                    break;
                    
                case 'Failed':
                    this.onStartupFailed(status);
                    this.stopStatusMonitoring();
                    break;
                    
                case 'Starting':
                    // 继续监控
                    break;
                    
                default:
                    this.stopStatusMonitoring();
                    break;
            }
        }, 1000);

        // 60秒后自动停止监控
        setTimeout(() => {
            if (this.statusCheckInterval) {
                this.stopStatusMonitoring();
                this.showError('启动超时');
                this.enableStartButton();
            }
        }, 60000);
    }

    // 停止监控
    stopStatusMonitoring() {
        if (this.statusCheckInterval) {
            clearInterval(this.statusCheckInterval);
            this.statusCheckInterval = null;
        }
    }

    // 启动成功回调
    onStartupSuccess(status) {
        console.log('数据库启动成功:', status);
        this.showSuccess('数据库启动成功！');
        this.enableStartButton();
        this.updateButtonState('running');
        
        // 触发自定义事件
        window.dispatchEvent(new CustomEvent('db-startup-success', { 
            detail: status 
        }));
    }

    // 启动失败回调
    onStartupFailed(status) {
        console.error('数据库启动失败:', status);
        this.showError(`启动失败: ${status.error_message || '未知错误'}`);
        this.enableStartButton();
        this.updateButtonState('stopped');
        
        // 触发自定义事件
        window.dispatchEvent(new CustomEvent('db-startup-failed', { 
            detail: status 
        }));
    }

    // 更新进度显示
    updateProgress(status) {
        const progressBar = document.getElementById('db-startup-progress');
        const progressText = document.getElementById('db-startup-progress-text');
        
        if (progressBar) {
            progressBar.style.width = `${status.progress}%`;
            progressBar.setAttribute('aria-valuenow', status.progress);
        }
        
        if (progressText) {
            progressText.textContent = status.progress_message || '启动中...';
        }
    }

    // 显示进度
    showProgress(message) {
        const progressContainer = document.getElementById('db-startup-progress-container');
        if (progressContainer) {
            progressContainer.style.display = 'block';
        }
        
        const progressText = document.getElementById('db-startup-progress-text');
        if (progressText) {
            progressText.textContent = message;
        }
    }

    // 显示成功消息
    showSuccess(message) {
        const progressContainer = document.getElementById('db-startup-progress-container');
        if (progressContainer) {
            progressContainer.style.display = 'none';
        }
        
        const messageEl = document.getElementById('db-startup-message');
        if (messageEl) {
            messageEl.className = 'alert alert-success';
            messageEl.textContent = message;
            messageEl.style.display = 'block';
            
            // 3秒后自动隐藏
            setTimeout(() => {
                messageEl.style.display = 'none';
            }, 3000);
        }
    }

    // 显示错误消息
    showError(message) {
        const progressContainer = document.getElementById('db-startup-progress-container');
        if (progressContainer) {
            progressContainer.style.display = 'none';
        }
        
        const messageEl = document.getElementById('db-startup-message');
        if (messageEl) {
            messageEl.className = 'alert alert-danger';
            messageEl.textContent = message;
            messageEl.style.display = 'block';
        }
    }

    // 禁用启动按钮
    disableStartButton() {
        const btn = document.getElementById('db-start-button');
        if (btn) {
            btn.disabled = true;
            btn.innerHTML = '<span class="spinner-border spinner-border-sm me-2"></span>启动中...';
        }
    }

    // 启用启动按钮
    enableStartButton() {
        const btn = document.getElementById('db-start-button');
        if (btn) {
            btn.disabled = false;
            btn.innerHTML = '启动';
        }
    }

    // 更新按钮状态
    updateButtonState(state) {
        const startBtn = document.getElementById('db-start-button');
        const stopBtn = document.getElementById('db-stop-button');
        const testBtn = document.getElementById('db-test-button');
        
        switch (state) {
            case 'running':
                if (startBtn) {
                    startBtn.disabled = true;
                    startBtn.innerHTML = '运行中';
                    startBtn.className = 'btn btn-success';
                }
                if (stopBtn) {
                    stopBtn.disabled = false;
                }
                if (testBtn) {
                    testBtn.disabled = false;
                }
                break;
                
            case 'stopped':
                if (startBtn) {
                    startBtn.disabled = false;
                    startBtn.innerHTML = '启动';
                    startBtn.className = 'btn btn-primary';
                }
                if (stopBtn) {
                    stopBtn.disabled = true;
                }
                if (testBtn) {
                    testBtn.disabled = true;
                }
                break;
                
            case 'starting':
                if (startBtn) {
                    startBtn.disabled = true;
                    startBtn.innerHTML = '<span class="spinner-border spinner-border-sm me-2"></span>启动中...';
                    startBtn.className = 'btn btn-warning';
                }
                if (stopBtn) {
                    stopBtn.disabled = true;
                }
                if (testBtn) {
                    testBtn.disabled = true;
                }
                break;
        }
    }

    // 初始化页面状态
    async initializePageState(ip, port) {
        const status = await this.checkStatus(ip, port);
        
        if (status) {
            switch (status.status) {
                case 'Running':
                    this.updateButtonState('running');
                    break;
                case 'Starting':
                    this.updateButtonState('starting');
                    // 继续监控
                    this.startStatusMonitoring(ip, port);
                    break;
                default:
                    this.updateButtonState('stopped');
                    break;
            }
        } else {
            this.updateButtonState('stopped');
        }
    }
}

// 创建全局实例
window.dbStartupManager = new DbStartupManager();

// 页面加载时初始化
document.addEventListener('DOMContentLoaded', () => {
    // 绑定启动按钮事件
    const startBtn = document.getElementById('db-start-button');
    if (startBtn) {
        startBtn.addEventListener('click', async () => {
            const config = {
                ip: document.getElementById('db-ip').value || 'localhost',
                port: parseInt(document.getElementById('db-port').value || '8009'),
                user: document.getElementById('db-user').value || 'root',
                password: document.getElementById('db-password').value || 'root',
                dbFile: document.getElementById('db-file').value || 'ams-8009-test.db'
            };
            
            await window.dbStartupManager.startDatabase(config);
        });
    }

    // 初始化页面状态
    const ip = document.getElementById('db-ip')?.value || 'localhost';
    const port = parseInt(document.getElementById('db-port')?.value || '8009');
    window.dbStartupManager.initializePageState(ip, port);
});