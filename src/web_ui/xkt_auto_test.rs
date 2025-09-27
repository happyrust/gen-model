use axum::response::Html;

/// 自动化XKT测试页面
pub async fn serve_xkt_auto_test_page() -> Html<String> {
    Html(r##"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>XKT 自动化测试</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
            margin: 0;
            padding: 20px;
            background: #f5f5f5;
        }
        .container {
            max-width: 1200px;
            margin: 0 auto;
            background: white;
            border-radius: 8px;
            padding: 20px;
            box-shadow: 0 2px 4px rgba(0,0,0,0.1);
        }
        h1 {
            color: #333;
            border-bottom: 2px solid #4CAF50;
            padding-bottom: 10px;
        }
        .control-panel {
            background: #f9f9f9;
            padding: 15px;
            border-radius: 4px;
            margin-bottom: 20px;
        }
        .test-config {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
            gap: 15px;
            margin-bottom: 15px;
        }
        .config-group {
            display: flex;
            flex-direction: column;
        }
        label {
            font-weight: 500;
            margin-bottom: 5px;
            color: #666;
        }
        input, select, textarea {
            padding: 8px;
            border: 1px solid #ddd;
            border-radius: 4px;
            font-size: 14px;
        }
        textarea {
            width: 100%;
            min-height: 60px;
            font-family: monospace;
        }
        button {
            background: #4CAF50;
            color: white;
            border: none;
            padding: 10px 20px;
            border-radius: 4px;
            cursor: pointer;
            font-size: 16px;
            margin-right: 10px;
        }
        button:hover {
            background: #45a049;
        }
        button:disabled {
            background: #ccc;
            cursor: not-allowed;
        }
        .stop-button {
            background: #f44336;
        }
        .stop-button:hover {
            background: #da190b;
        }
        .test-status {
            margin-bottom: 20px;
            padding: 15px;
            border-radius: 4px;
            background: #e3f2fd;
        }
        .test-results {
            margin-top: 20px;
        }
        .test-result {
            margin-bottom: 10px;
            padding: 10px;
            border-radius: 4px;
            display: flex;
            justify-content: space-between;
            align-items: center;
        }
        .test-success {
            background: #c8e6c9;
            color: #2e7d32;
        }
        .test-failure {
            background: #ffcdd2;
            color: #c62828;
        }
        .test-pending {
            background: #fff3e0;
            color: #e65100;
        }
        .test-running {
            background: #e1f5fe;
            color: #0277bd;
        }
        .progress-bar {
            width: 100%;
            height: 20px;
            background: #e0e0e0;
            border-radius: 10px;
            overflow: hidden;
            margin: 10px 0;
        }
        .progress-fill {
            height: 100%;
            background: #4CAF50;
            transition: width 0.3s ease;
        }
        .stats {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
            gap: 10px;
            margin-top: 20px;
        }
        .stat-card {
            padding: 10px;
            text-align: center;
            border-radius: 4px;
            background: #f5f5f5;
        }
        .stat-value {
            font-size: 24px;
            font-weight: bold;
            color: #333;
        }
        .stat-label {
            font-size: 12px;
            color: #666;
            margin-top: 5px;
        }
        #viewer {
            width: 100%;
            height: 400px;
            border: 1px solid #ddd;
            border-radius: 4px;
            margin-top: 20px;
            display: none;
        }
        .log-container {
            background: #1e1e1e;
            color: #d4d4d4;
            padding: 10px;
            border-radius: 4px;
            font-family: 'Consolas', 'Monaco', monospace;
            font-size: 12px;
            max-height: 300px;
            overflow-y: auto;
            margin-top: 20px;
        }
        .log-entry {
            margin: 2px 0;
        }
        .log-time {
            color: #858585;
        }
        .log-success {
            color: #4ec9b0;
        }
        .log-error {
            color: #f48771;
        }
        .log-info {
            color: #4fc1ff;
        }
        .log-warning {
            color: #ce9178;
        }
    </style>
    <script src="https://cdn.jsdelivr.net/npm/xeokit-sdk@2.6.8/dist/xeokit-sdk.es.js"></script>
</head>
<body>
    <div class="container">
        <h1>🤖 XKT 自动化测试工具</h1>

        <div class="control-panel">
            <div class="test-config">
                <div class="config-group">
                    <label>测试模式:</label>
                    <select id="testMode">
                        <option value="single">单个测试</option>
                        <option value="batch">批量测试</option>
                        <option value="random">随机测试</option>
                    </select>
                </div>
                <div class="config-group">
                    <label>DBNUM (单个/起始):</label>
                    <input type="text" id="dbnumStart" value="1112" />
                </div>
                <div class="config-group">
                    <label>REFNO (单个/起始):</label>
                    <input type="text" id="refnoStart" value="17496/266203" />
                </div>
                <div class="config-group">
                    <label>测试数量:</label>
                    <input type="number" id="testCount" value="5" min="1" max="100" />
                </div>
                <div class="config-group">
                    <label>延迟(ms):</label>
                    <input type="number" id="delay" value="1000" min="100" max="10000" />
                </div>
                <div class="config-group">
                    <label>压缩:</label>
                    <select id="compress">
                        <option value="true">启用</option>
                        <option value="false">禁用</option>
                        <option value="both">都测试</option>
                    </select>
                </div>
            </div>

            <div style="margin-top: 15px;">
                <label>批量测试列表 (DBNUM/REFNO，每行一个):</label>
                <textarea id="batchList" placeholder="1112/17496/266203
1113/17497/266204
1114/17498/266205"></textarea>
            </div>

            <div style="margin-top: 15px;">
                <button onclick="startAutoTest()" id="startBtn">🚀 开始测试</button>
                <button onclick="stopTest()" id="stopBtn" class="stop-button" disabled>⏹ 停止测试</button>
                <button onclick="clearResults()">🗑️ 清空结果</button>
                <button onclick="exportResults()">📊 导出报告</button>
            </div>
        </div>

        <div class="test-status">
            <div style="display: flex; justify-content: space-between; align-items: center;">
                <span id="statusText">⏳ 等待开始测试...</span>
                <span id="timeElapsed">耗时: 0s</span>
            </div>
            <div class="progress-bar">
                <div class="progress-fill" id="progressBar" style="width: 0%"></div>
            </div>
            <div class="stats">
                <div class="stat-card">
                    <div class="stat-value" id="totalTests">0</div>
                    <div class="stat-label">总测试</div>
                </div>
                <div class="stat-card" style="background: #c8e6c9;">
                    <div class="stat-value" id="successTests">0</div>
                    <div class="stat-label">成功</div>
                </div>
                <div class="stat-card" style="background: #ffcdd2;">
                    <div class="stat-value" id="failedTests">0</div>
                    <div class="stat-label">失败</div>
                </div>
                <div class="stat-card" style="background: #e1f5fe;">
                    <div class="stat-value" id="pendingTests">0</div>
                    <div class="stat-label">待处理</div>
                </div>
            </div>
        </div>

        <div class="test-results">
            <h3>测试结果:</h3>
            <div id="resultsList"></div>
        </div>

        <div class="log-container" id="logContainer">
            <div id="logEntries"></div>
        </div>

        <canvas id="viewer"></canvas>
    </div>

    <script>
        let isRunning = false;
        let shouldStop = false;
        let startTime = null;
        let timerInterval = null;
        let viewer = null;
        let testResults = [];

        // 测试统计
        let stats = {
            total: 0,
            success: 0,
            failed: 0,
            pending: 0
        };

        // 初始化 Xeokit viewer
        function initViewer() {
            const canvas = document.getElementById('viewer');
            viewer = new xeokit.Viewer({
                canvasId: "viewer",
                transparent: true
            });

            viewer.camera.eye = [-3.93, 2.85, 27.01];
            viewer.camera.look = [4.40, 3.72, 8.89];
            viewer.camera.up = [0.00, 1.00, 0.00];

            new xeokit.AxisGizmoPlugin(viewer, {
                id: "axisGizmo"
            });

            return viewer;
        }

        // 日志功能
        function log(message, type = 'info') {
            const logEntries = document.getElementById('logEntries');
            const time = new Date().toLocaleTimeString();
            const entry = document.createElement('div');
            entry.className = 'log-entry';

            const colorClass = `log-${type}`;
            entry.innerHTML = `<span class="log-time">[${time}]</span> <span class="${colorClass}">${message}</span>`;

            logEntries.appendChild(entry);
            logEntries.scrollTop = logEntries.scrollHeight;
        }

        // 开始自动测试
        async function startAutoTest() {
            if (isRunning) return;

            isRunning = true;
            shouldStop = false;
            testResults = [];
            stats = { total: 0, success: 0, failed: 0, pending: 0 };

            document.getElementById('startBtn').disabled = true;
            document.getElementById('stopBtn').disabled = false;
            document.getElementById('resultsList').innerHTML = '';

            startTime = Date.now();
            startTimer();

            const mode = document.getElementById('testMode').value;
            const delay = parseInt(document.getElementById('delay').value);

            log('🚀 开始自动化测试...', 'info');

            try {
                if (mode === 'single') {
                    await runSingleTest();
                } else if (mode === 'batch') {
                    await runBatchTests(delay);
                } else if (mode === 'random') {
                    await runRandomTests(delay);
                }

                log('✅ 测试完成!', 'success');
            } catch (error) {
                log(`❌ 测试出错: ${error.message}`, 'error');
            } finally {
                isRunning = false;
                document.getElementById('startBtn').disabled = false;
                document.getElementById('stopBtn').disabled = true;
                stopTimer();
                updateStatus('✅ 测试完成');
            }
        }

        // 运行单个测试
        async function runSingleTest() {
            const dbnum = document.getElementById('dbnumStart').value;
            const refno = document.getElementById('refnoStart').value;
            const compress = document.getElementById('compress').value;

            if (compress === 'both') {
                await testXKT(dbnum, refno, true);
                if (!shouldStop) {
                    await testXKT(dbnum, refno, false);
                }
            } else {
                await testXKT(dbnum, refno, compress === 'true');
            }
        }

        // 运行批量测试
        async function runBatchTests(delay) {
            const batchList = document.getElementById('batchList').value.trim();
            const compress = document.getElementById('compress').value;

            let testCases = [];

            if (batchList) {
                // 从文本框解析测试用例
                const lines = batchList.split('\n');
                for (const line of lines) {
                    const parts = line.trim().split('/');
                    if (parts.length >= 2) {
                        const dbnum = parts[0];
                        const refno = parts.slice(1).join('/');
                        testCases.push({ dbnum, refno });
                    }
                }
            } else {
                // 使用默认测试用例
                const dbnumStart = parseInt(document.getElementById('dbnumStart').value);
                const refnoStart = document.getElementById('refnoStart').value;
                const count = parseInt(document.getElementById('testCount').value);

                for (let i = 0; i < count; i++) {
                    testCases.push({
                        dbnum: (dbnumStart + i).toString(),
                        refno: refnoStart
                    });
                }
            }

            stats.total = compress === 'both' ? testCases.length * 2 : testCases.length;
            stats.pending = stats.total;
            updateStats();

            for (let i = 0; i < testCases.length; i++) {
                if (shouldStop) break;

                const { dbnum, refno } = testCases[i];

                if (compress === 'both') {
                    await testXKT(dbnum, refno, true);
                    if (!shouldStop && i < testCases.length) {
                        await new Promise(resolve => setTimeout(resolve, delay));
                        await testXKT(dbnum, refno, false);
                    }
                } else {
                    await testXKT(dbnum, refno, compress === 'true');
                }

                if (i < testCases.length - 1 && !shouldStop) {
                    await new Promise(resolve => setTimeout(resolve, delay));
                }
            }
        }

        // 运行随机测试
        async function runRandomTests(delay) {
            const count = parseInt(document.getElementById('testCount').value);
            const compress = document.getElementById('compress').value;

            stats.total = compress === 'both' ? count * 2 : count;
            stats.pending = stats.total;
            updateStats();

            for (let i = 0; i < count; i++) {
                if (shouldStop) break;

                const dbnum = Math.floor(Math.random() * 9000 + 1000).toString();
                const refno = `${Math.floor(Math.random() * 90000 + 10000)}/${Math.floor(Math.random() * 900000 + 100000)}`;

                if (compress === 'both') {
                    await testXKT(dbnum, refno, true);
                    if (!shouldStop && i < count) {
                        await new Promise(resolve => setTimeout(resolve, delay));
                        await testXKT(dbnum, refno, false);
                    }
                } else {
                    await testXKT(dbnum, refno, compress === 'true');
                }

                if (i < count - 1 && !shouldStop) {
                    await new Promise(resolve => setTimeout(resolve, delay));
                }
            }
        }

        // 测试单个XKT文件
        async function testXKT(dbnum, refno, compress) {
            const testId = `${dbnum}_${refno.replace(/\//g, '_')}_${compress ? 'compressed' : 'uncompressed'}`;
            const testName = `DBNUM: ${dbnum}, REFNO: ${refno}, 压缩: ${compress ? '是' : '否'}`;

            updateStatus(`🔄 测试中: ${testName}`);
            log(`🔄 开始测试: ${testName}`, 'info');

            const resultDiv = addTestResult(testId, testName, 'running');

            try {
                // 1. 生成XKT文件
                const generateResponse = await fetch('/api/xkt/generate', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ dbnum, refno, compress })
                });

                if (!generateResponse.ok) {
                    throw new Error(`生成失败: ${generateResponse.statusText}`);
                }

                const generateResult = await generateResponse.json();
                const filename = generateResult.filename;
                log(`📦 生成文件: ${filename}`, 'info');

                // 2. 尝试加载XKT文件
                const loadResult = await loadXKTFile(filename);

                if (loadResult.success) {
                    log(`✅ 加载成功: ${testName}`, 'success');
                    updateTestResult(testId, 'success', `✅ 成功 - ${loadResult.info}`);
                    stats.success++;
                } else {
                    log(`❌ 加载失败: ${testName} - ${loadResult.error}`, 'error');
                    updateTestResult(testId, 'failure', `❌ 失败 - ${loadResult.error}`);
                    stats.failed++;
                }

                stats.pending--;
                updateStats();

                testResults.push({
                    dbnum,
                    refno,
                    compress,
                    success: loadResult.success,
                    filename,
                    error: loadResult.error,
                    info: loadResult.info,
                    timestamp: new Date().toISOString()
                });

            } catch (error) {
                log(`❌ 测试失败: ${testName} - ${error.message}`, 'error');
                updateTestResult(testId, 'failure', `❌ 错误 - ${error.message}`);
                stats.failed++;
                stats.pending--;
                updateStats();

                testResults.push({
                    dbnum,
                    refno,
                    compress,
                    success: false,
                    error: error.message,
                    timestamp: new Date().toISOString()
                });
            }

            updateProgress();
        }

        // 加载XKT文件
        async function loadXKTFile(filename) {
            return new Promise((resolve) => {
                try {
                    if (!viewer) {
                        initViewer();
                    }

                    // 清除之前的模型
                    viewer.scene.models.forEach(model => {
                        model.destroy();
                    });

                    const xktLoader = new xeokit.XKTLoaderPlugin(viewer);

                    const startLoadTime = Date.now();

                    const model = xktLoader.load({
                        id: "testModel",
                        src: `/output/web_ui/${filename}`,
                        edges: true,
                        performance: true
                    });

                    model.on("loaded", () => {
                        const loadTime = Date.now() - startLoadTime;
                        const entityCount = Object.keys(model.entities).length;
                        const triangleCount = model.numTriangles || 0;

                        resolve({
                            success: true,
                            info: `实体: ${entityCount}, 三角形: ${triangleCount}, 加载时间: ${loadTime}ms`
                        });
                    });

                    model.on("error", (error) => {
                        resolve({
                            success: false,
                            error: error.message || "未知加载错误"
                        });
                    });

                    // 设置超时
                    setTimeout(() => {
                        resolve({
                            success: false,
                            error: "加载超时 (10秒)"
                        });
                    }, 10000);

                } catch (error) {
                    resolve({
                        success: false,
                        error: error.message || "加载异常"
                    });
                }
            });
        }

        // UI更新函数
        function addTestResult(id, name, status) {
            const resultsList = document.getElementById('resultsList');
            const div = document.createElement('div');
            div.id = `test-${id}`;
            div.className = `test-result test-${status}`;
            div.innerHTML = `
                <span>${name}</span>
                <span id="status-${id}">⏳ 运行中...</span>
            `;
            resultsList.appendChild(div);
            return div;
        }

        function updateTestResult(id, status, message) {
            const div = document.getElementById(`test-${id}`);
            if (div) {
                div.className = `test-result test-${status === 'success' ? 'success' : 'failure'}`;
                document.getElementById(`status-${id}`).textContent = message;
            }
        }

        function updateStatus(text) {
            document.getElementById('statusText').textContent = text;
        }

        function updateStats() {
            document.getElementById('totalTests').textContent = stats.total;
            document.getElementById('successTests').textContent = stats.success;
            document.getElementById('failedTests').textContent = stats.failed;
            document.getElementById('pendingTests').textContent = stats.pending;
        }

        function updateProgress() {
            const completed = stats.success + stats.failed;
            const progress = stats.total > 0 ? (completed / stats.total) * 100 : 0;
            document.getElementById('progressBar').style.width = `${progress}%`;
        }

        function startTimer() {
            timerInterval = setInterval(() => {
                const elapsed = Math.floor((Date.now() - startTime) / 1000);
                document.getElementById('timeElapsed').textContent = `耗时: ${elapsed}s`;
            }, 1000);
        }

        function stopTimer() {
            if (timerInterval) {
                clearInterval(timerInterval);
                timerInterval = null;
            }
        }

        function stopTest() {
            shouldStop = true;
            log('⏹ 停止测试...', 'warning');
        }

        function clearResults() {
            document.getElementById('resultsList').innerHTML = '';
            document.getElementById('logEntries').innerHTML = '';
            testResults = [];
            stats = { total: 0, success: 0, failed: 0, pending: 0 };
            updateStats();
            updateProgress();
            updateStatus('⏳ 等待开始测试...');
            document.getElementById('timeElapsed').textContent = '耗时: 0s';
        }

        function exportResults() {
            if (testResults.length === 0) {
                alert('没有测试结果可导出');
                return;
            }

            const report = {
                timestamp: new Date().toISOString(),
                stats: stats,
                results: testResults
            };

            const blob = new Blob([JSON.stringify(report, null, 2)], { type: 'application/json' });
            const url = URL.createObjectURL(blob);
            const a = document.createElement('a');
            a.href = url;
            a.download = `xkt-test-report-${Date.now()}.json`;
            a.click();
            URL.revokeObjectURL(url);

            log('📊 测试报告已导出', 'success');
        }

        // 页面加载时初始化
        document.addEventListener('DOMContentLoaded', () => {
            log('🤖 XKT自动化测试工具已就绪', 'info');
        });
    </script>
</body>
</html>
    "##.to_string())
}
