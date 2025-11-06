// 同步控制中心前端脚本

// 全局状态
let eventSource = null;
let currentState = null;

// 初始化
document.addEventListener('DOMContentLoaded', () => {
    initializeControls();
    loadInitialState();
    startEventStream();
    startPolling();
});

// 初始化控制按钮
function initializeControls() {
    // 启动按钮
    document.getElementById('btn-start')?.addEventListener('click', async () => {
        const envId = await promptForEnvId();
        if (!envId) return;

        const response = await fetch('/api/sync/start', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ env_id: envId })
        });
        const data = await response.json();
        showMessage(data.message, data.status);
    });

    // 停止按钮
    document.getElementById('btn-stop')?.addEventListener('click', async () => {
        if (!confirm('确定要停止同步服务吗？')) return;

        const response = await fetch('/api/sync/stop', { method: 'POST' });
        const data = await response.json();
        showMessage(data.message, data.status);
    });

    // 重启按钮
    document.getElementById('btn-restart')?.addEventListener('click', async () => {
        if (!confirm('确定要重启同步服务吗？')) return;

        const response = await fetch('/api/sync/restart', { method: 'POST' });
        const data = await response.json();
        showMessage(data.message, data.status);
    });

    // 暂停按钮
    document.getElementById('btn-pause')?.addEventListener('click', async () => {
        const response = await fetch('/api/sync/pause', { method: 'POST' });
        const data = await response.json();
        showMessage(data.message, data.status);
        updateControlButtons();
    });

    // 恢复按钮
    document.getElementById('btn-resume')?.addEventListener('click', async () => {
        const response = await fetch('/api/sync/resume', { method: 'POST' });
        const data = await response.json();
        showMessage(data.message, data.status);
        updateControlButtons();
    });

    // 清空队列按钮
    document.getElementById('btn-clear-queue')?.addEventListener('click', async () => {
        if (!confirm('确定要清空同步队列吗？')) return;

        const response = await fetch('/api/sync/queue/clear', { method: 'POST' });
        const data = await response.json();
        showMessage(data.message, data.status);
    });
}

// 加载初始状态
async function loadInitialState() {
    try {
        const response = await fetch('/api/sync/status');
        const data = await response.json();

        if (data.status === 'success') {
            currentState = data.state;
            updateUI(data.state);
        }
    } catch (error) {
        console.error('加载状态失败:', error);
    }
}

// 启动事件轮询（替代 SSE）
function startEventStream() {
    // 使用轮询替代 SSE
    setInterval(async () => {
        try {
            const response = await fetch('/api/sync/events');
            const data = await response.json();

            if (data.status === 'success' && data.events) {
                data.events.forEach(event => {
                    handleSyncEvent(event);
                });
            }
        } catch (error) {
            console.error('获取事件失败:', error);
        }
    }, 1000); // 每秒轮询一次
}

// 处理同步事件
function handleSyncEvent(event) {
    console.log('收到事件:', event);

    // 添加到日志
    addLogEntry(event);

    // 根据事件类型更新 UI
    switch (event.type) {
        case 'Started':
            updateServiceStatus(true);
            showMessage(`服务已启动: ${event.data.env_id}`, 'success');
            break;

        case 'Stopped':
            updateServiceStatus(false);
            showMessage(`服务已停止: ${event.data.reason}`, 'warning');
            break;

        case 'ConnectionChanged':
            updateConnectionStatus(event.data);
            break;

        case 'ProgressUpdate':
            updateProgress(event.data);
            break;

        case 'SyncCompleted':
            addLogEntry({
                level: 'info',
                message: `文件同步完成: ${event.data.file_path} (${event.data.duration_ms}ms)`
            });
            break;

        case 'SyncFailed':
            addLogEntry({
                level: 'error',
                message: `同步失败: ${event.data.file_path} - ${event.data.error}`
            });
            break;

        case 'Alert':
            handleAlert(event.data);
            break;

        case 'MetricsUpdate':
            updateMetrics(event.data);
            break;
    }
}

// 更新 UI
function updateUI(state) {
    // 更新服务状态
    updateServiceStatus(state.is_running, state.is_paused);

    // 更新连接状态
    updateConnectionStatus({
        mqtt_connected: state.mqtt_connected,
        watcher_active: state.watcher_active
    });

    // 更新队列长度
    document.getElementById('queue-length').textContent = state.queue_size || 0;

    // 更新性能指标
    document.getElementById('metric-sync-rate').textContent =
        (state.sync_rate_mbps || 0).toFixed(2);
    document.getElementById('metric-total-synced').textContent =
        state.total_synced || 0;

    const successRate = state.total_synced && state.total_synced + state.total_failed > 0
        ? (state.total_synced / (state.total_synced + state.total_failed) * 100).toFixed(1)
        : 0;
    document.getElementById('metric-success-rate').textContent = `${successRate}%`;

    document.getElementById('metric-uptime').textContent =
        formatUptime(state.uptime_seconds || 0);

    // 更新控制按钮状态
    updateControlButtons();
}

// 更新服务状态显示
function updateServiceStatus(isRunning, isPaused) {
    const statusEl = document.getElementById('service-status');
    const indicator = statusEl.querySelector('.status-indicator');
    const text = statusEl.querySelector('span:last-child');

    if (isRunning) {
        if (isPaused) {
            indicator.className = 'status-indicator status-warning';
            text.textContent = '已暂停';
        } else {
            indicator.className = 'status-indicator status-running';
            text.textContent = '运行中';
        }
    } else {
        indicator.className = 'status-indicator status-stopped';
        text.textContent = '已停止';
    }
}

// 更新连接状态
function updateConnectionStatus(data) {
    // MQTT 状态
    const mqttEl = document.getElementById('mqtt-status');
    const mqttIndicator = mqttEl.querySelector('.status-indicator');
    const mqttText = mqttEl.querySelector('span:last-child');

    if (data.mqtt_connected) {
        mqttIndicator.className = 'status-indicator status-running';
        mqttText.textContent = '已连接';
    } else {
        mqttIndicator.className = 'status-indicator status-stopped';
        mqttText.textContent = '未连接';
    }

    // Watcher 状态
    const watcherEl = document.getElementById('watcher-status');
    const watcherIndicator = watcherEl.querySelector('.status-indicator');
    const watcherText = watcherEl.querySelector('span:last-child');

    if (data.watcher_active) {
        watcherIndicator.className = 'status-indicator status-running';
        watcherText.textContent = '监听中';
    } else {
        watcherIndicator.className = 'status-indicator status-stopped';
        watcherText.textContent = '未激活';
    }
}

// 更新进度
function updateProgress(data) {
    document.getElementById('queue-length').textContent = data.pending || 0;
    // 可以添加进度条显示
}

// 更新性能指标
function updateMetrics(data) {
    if (data.sync_rate_mbps !== undefined) {
        document.getElementById('metric-sync-rate').textContent =
            data.sync_rate_mbps.toFixed(2);
    }
}

// 添加日志条目
function addLogEntry(event) {
    const container = document.getElementById('log-container');
    const entry = document.createElement('div');

    const timestamp = new Date().toLocaleTimeString();
    let level = 'info';
    let message = '';

    if (event.level) {
        level = event.level.toLowerCase();
        message = event.message;
    } else if (event.type) {
        message = formatEventMessage(event);
    }

    entry.className = `log-entry ${level}`;
    entry.innerHTML = `
        <span class="text-gray-500">[${timestamp}]</span>
        <span>${message}</span>
    `;

    container.insertBefore(entry, container.firstChild);

    // 保持最多 100 条日志
    while (container.children.length > 100) {
        container.removeChild(container.lastChild);
    }
}

// 格式化事件消息
function formatEventMessage(event) {
    switch (event.type) {
        case 'Started':
            return `✅ 服务启动 - 环境: ${event.data.env_id}`;
        case 'Stopped':
            return `⛔ 服务停止 - ${event.data.reason}`;
        case 'ConnectionChanged':
            return `🔄 连接状态变更 - MQTT: ${event.data.mqtt_connected ? '✅' : '❌'}, Watcher: ${event.data.watcher_active ? '✅' : '❌'}`;
        case 'SyncStarted':
            return `📤 开始同步: ${event.data.file_path}`;
        case 'SyncCompleted':
            return `✅ 同步完成: ${event.data.file_path} (${event.data.duration_ms}ms)`;
        case 'SyncFailed':
            return `❌ 同步失败: ${event.data.file_path} - ${event.data.error}`;
        default:
            return JSON.stringify(event);
    }
}

// 处理告警
function handleAlert(data) {
    const levelColors = {
        'Info': 'info',
        'Warning': 'warning',
        'Error': 'error',
        'Critical': 'error'
    };

    addLogEntry({
        level: levelColors[data.level] || 'info',
        message: `[${data.level}] ${data.message}`
    });

    // 对于严重告警，显示弹窗
    if (data.level === 'Critical' || data.level === 'Error') {
        showMessage(data.message, 'error');
    }
}

// 更新控制按钮状态
function updateControlButtons() {
    const isRunning = currentState?.is_running || false;
    const isPaused = currentState?.is_paused || false;

    document.getElementById('btn-start').disabled = isRunning;
    document.getElementById('btn-stop').disabled = !isRunning;
    document.getElementById('btn-restart').disabled = !isRunning;
    document.getElementById('btn-pause').disabled = !isRunning || isPaused;
    document.getElementById('btn-resume').disabled = !isRunning || !isPaused;
}

// 定期轮询状态（作为 SSE 的备份）
function startPolling() {
    setInterval(async () => {
        try {
            const response = await fetch('/api/sync/status');
            const data = await response.json();
            if (data.status === 'success') {
                currentState = data.state;
                updateUI(data.state);
            }
        } catch (error) {
            console.error('轮询失败:', error);
        }
    }, 5000); // 每5秒轮询一次
}

// 提示输入环境 ID
async function promptForEnvId() {
    // 先获取可用的环境列表
    try {
        const response = await fetch('/api/remote-sync/envs');
        const data = await response.json();

        if (data.items && data.items.length > 0) {
            // 如果有环境，显示选择对话框
            const envList = data.items.map(env =>
                `${env.name} (${env.id})`
            ).join('\n');

            const selectedEnv = prompt(
                `请选择要启动的环境:\n${envList}\n\n请输入环境ID:`,
                data.items[0].id
            );

            return selectedEnv;
        } else {
            alert('没有可用的环境配置，请先创建环境');
            return null;
        }
    } catch (error) {
        console.error('获取环境列表失败:', error);
        return prompt('请输入环境ID:');
    }
}

// 显示消息
function showMessage(message, type = 'info') {
    // 添加到日志
    addLogEntry({
        level: type === 'success' ? 'info' : type,
        message: message
    });

    // TODO: 可以添加更美观的通知组件
    if (type === 'error') {
        console.error(message);
    } else {
        console.log(message);
    }
}

// 格式化运行时间
function formatUptime(seconds) {
    if (seconds < 60) {
        return `${seconds}s`;
    } else if (seconds < 3600) {
        const minutes = Math.floor(seconds / 60);
        return `${minutes}m ${seconds % 60}s`;
    } else {
        const hours = Math.floor(seconds / 3600);
        const minutes = Math.floor((seconds % 3600) / 60);
        return `${hours}h ${minutes}m`;
    }
}