/// XKT 生成器和查看器测试页面
pub fn render_xkt_generator_viewer_page() -> String {
    r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>XKT 生成器与查看器</title>
    <script src="https://cdn.jsdelivr.net/npm/@xeokit/xeokit-sdk@2.6.8/dist/xeokit-sdk.es.js" type="module"></script>
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }

        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            background: #0f172a;
            color: #e2e8f0;
            display: flex;
            height: 100vh;
        }

        .sidebar {
            width: 320px;
            background: rgba(15, 23, 42, 0.95);
            border-right: 1px solid rgba(148, 163, 184, 0.2);
            padding: 20px;
            overflow-y: auto;
            backdrop-filter: blur(8px);
        }

        .sidebar h2 {
            font-size: 18px;
            font-weight: 600;
            color: #94a3b8;
            margin-bottom: 20px;
        }

        .form-section {
            margin-bottom: 24px;
            padding: 16px;
            background: rgba(30, 41, 59, 0.5);
            border-radius: 8px;
        }

        .form-section h3 {
            font-size: 14px;
            font-weight: 500;
            color: #cbd5e1;
            margin-bottom: 12px;
        }

        .form-group {
            margin-bottom: 16px;
        }

        .form-group label {
            display: block;
            margin-bottom: 6px;
            font-size: 13px;
            color: #94a3b8;
        }

        .form-group input, .form-group select {
            width: 100%;
            padding: 10px 12px;
            border-radius: 6px;
            border: 1px solid rgba(148, 163, 184, 0.3);
            background: rgba(30, 41, 59, 0.8);
            color: #e2e8f0;
            font-size: 14px;
            outline: none;
            transition: all 0.2s;
        }

        .form-group input:focus, .form-group select:focus {
            border-color: #3b82f6;
            background: rgba(30, 41, 59, 1);
        }

        .checkbox-group {
            display: flex;
            align-items: center;
            gap: 8px;
        }

        .checkbox-group input[type="checkbox"] {
            width: auto;
        }

        .button-group {
            display: flex;
            gap: 10px;
        }

        .button {
            flex: 1;
            padding: 10px 16px;
            border-radius: 6px;
            border: none;
            background: #3b82f6;
            color: white;
            font-size: 14px;
            font-weight: 500;
            cursor: pointer;
            transition: all 0.2s;
        }

        .button:hover {
            background: #2563eb;
        }

        .button:disabled {
            background: #475569;
            cursor: not-allowed;
        }

        .button.secondary {
            background: #475569;
        }

        .button.secondary:hover {
            background: #64748b;
        }

        .viewer-container {
            flex: 1;
            position: relative;
            background: #0b1120;
        }

        #viewerCanvas {
            width: 100%;
            height: 100%;
            display: block;
        }

        .status-bar {
            position: absolute;
            bottom: 0;
            left: 0;
            right: 0;
            background: rgba(15, 23, 42, 0.95);
            border-top: 1px solid rgba(148, 163, 184, 0.2);
            padding: 12px 20px;
            backdrop-filter: blur(8px);
            display: flex;
            justify-content: space-between;
            align-items: center;
        }

        .status-text {
            font-size: 13px;
            color: #94a3b8;
        }

        .loading-overlay {
            position: absolute;
            top: 0;
            left: 0;
            right: 0;
            bottom: 0;
            background: rgba(0, 0, 0, 0.7);
            display: none;
            justify-content: center;
            align-items: center;
            z-index: 1000;
        }

        .loading-overlay.active {
            display: flex;
        }

        .loading-content {
            text-align: center;
        }

        .spinner {
            width: 50px;
            height: 50px;
            border: 3px solid rgba(59, 130, 246, 0.3);
            border-top-color: #3b82f6;
            border-radius: 50%;
            animation: spin 1s linear infinite;
            margin: 0 auto 16px;
        }

        @keyframes spin {
            to { transform: rotate(360deg); }
        }

        .loading-text {
            color: #e2e8f0;
            font-size: 14px;
        }

        .file-list {
            max-height: 200px;
            overflow-y: auto;
            border: 1px solid rgba(148, 163, 184, 0.2);
            border-radius: 6px;
            background: rgba(30, 41, 59, 0.5);
        }

        .file-item {
            padding: 8px 12px;
            border-bottom: 1px solid rgba(148, 163, 184, 0.1);
            cursor: pointer;
            font-size: 13px;
            transition: background 0.2s;
        }

        .file-item:hover {
            background: rgba(59, 130, 246, 0.1);
        }

        .file-item:last-child {
            border-bottom: none;
        }

        .quick-buttons {
            display: grid;
            grid-template-columns: 1fr 1fr;
            gap: 8px;
            margin-bottom: 16px;
        }

        .quick-button {
            padding: 8px 12px;
            border-radius: 6px;
            border: 1px solid rgba(148, 163, 184, 0.3);
            background: rgba(30, 41, 59, 0.8);
            color: #94a3b8;
            font-size: 12px;
            cursor: pointer;
            transition: all 0.2s;
        }

        .quick-button:hover {
            background: rgba(59, 130, 246, 0.2);
            border-color: #3b82f6;
            color: #3b82f6;
        }
    </style>
</head>
<body>
    <div class="sidebar">
        <h2>XKT 生成器与查看器</h2>

        <div class="form-section">
            <h3>快速选择</h3>
            <div class="quick-buttons">
                <button class="quick-button" onclick="setQuickValues(1112, '')">DB 1112</button>
                <button class="quick-button" onclick="setQuickValues(17496, '')">DB 17496</button>
                <button class="quick-button" onclick="setQuickValues(17496, '266203/1')">17496/266203</button>
                <button class="quick-button" onclick="setQuickValues(25688, '71821/749')">25688/71821</button>
            </div>
        </div>

        <div class="form-section">
            <h3>生成设置</h3>
            <div class="form-group">
                <label for="dbnum">数据库编号 (DBNUM)</label>
                <input type="number" id="dbnum" placeholder="例如: 17496" value="17496">
            </div>

            <div class="form-group">
                <label for="refno">参考号 (REFNO) - 可选</label>
                <input type="text" id="refno" placeholder="例如: 266203/1 或留空生成整个数据库">
            </div>

            <div class="form-group checkbox-group">
                <input type="checkbox" id="compress" checked>
                <label for="compress">压缩输出</label>
            </div>

            <div class="button-group">
                <button class="button" onclick="generateXKT()">生成 XKT</button>
                <button class="button secondary" onclick="loadLatest()">加载最新</button>
            </div>
        </div>

        <div class="form-section">
            <h3>已有文件</h3>
            <div id="fileList" class="file-list">
                <div class="file-item">正在加载文件列表...</div>
            </div>
        </div>
    </div>

    <div class="viewer-container">
        <canvas id="viewerCanvas"></canvas>
        <div class="loading-overlay" id="loadingOverlay">
            <div class="loading-content">
                <div class="spinner"></div>
                <div class="loading-text" id="loadingText">正在生成...</div>
            </div>
        </div>
        <div class="status-bar">
            <div class="status-text" id="statusText">准备就绪</div>
            <div class="status-text" id="modelInfo">未加载模型</div>
        </div>
    </div>

    <script type="module">
        import {
            Viewer,
            XKTLoaderPlugin,
            NavCubePlugin,
            AxisGizmoPlugin
        } from 'https://cdn.jsdelivr.net/npm/@xeokit/xeokit-sdk@2.6.8/dist/xeokit-sdk.es.js';

        let viewer;
        let xktLoader;
        let currentModel = null;

        // 初始化查看器
        function initViewer() {
            viewer = new Viewer({
                canvasId: 'viewerCanvas',
                transparent: true,
                saoEnabled: true,
                pbrEnabled: false
            });

            // 设置相机
            viewer.camera.eye = [50, 50, 50];
            viewer.camera.look = [0, 0, 0];
            viewer.camera.up = [0, 1, 0];

            // 添加插件
            xktLoader = new XKTLoaderPlugin(viewer);

            new NavCubePlugin(viewer, {
                canvasId: "viewerCanvas",
                visible: true,
                size: 180,
                alignment: "topRight",
                topMargin: 60,
                rightMargin: 20
            });

            new AxisGizmoPlugin(viewer, {
                canvasId: "viewerCanvas",
                visible: true,
                size: [100, 100],
                alignment: "topRight",
                topMargin: 250,
                rightMargin: 45
            });

            // 控制器
            viewer.cameraControl.followPointer = true;
        }

        // 生成 XKT
        window.generateXKT = async function() {
            const dbnum = document.getElementById('dbnum').value;
            const refno = document.getElementById('refno').value;
            const compress = document.getElementById('compress').checked;

            if (!dbnum) {
                alert('请输入数据库编号');
                return;
            }

            const loadingOverlay = document.getElementById('loadingOverlay');
            const loadingText = document.getElementById('loadingText');
            const statusText = document.getElementById('statusText');

            loadingOverlay.classList.add('active');
            loadingText.textContent = '正在生成 XKT 文件...';
            statusText.textContent = '生成中...';

            try {
                const payload = {
                    dbno: parseInt(dbnum),
                    compress: compress
                };

                if (refno && refno.trim()) {
                    payload.refno = refno.trim();
                }

                const response = await fetch('/api/xkt/generate', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify(payload)
                });

                const result = await response.json();

                if (result.success) {
                    loadingText.textContent = '生成成功，正在加载...';
                    statusText.textContent = '加载模型中...';

                    // 加载生成的文件
                    const fileUrl = `/files/output/web_ui/${result.filename}`;
                    await loadXKTFile(fileUrl, result.filename);

                    // 刷新文件列表
                    await refreshFileList();

                    statusText.textContent = `已加载: ${result.filename}`;
                } else {
                    throw new Error(result.error || '生成失败');
                }
            } catch (error) {
                console.error('生成错误:', error);
                alert('生成失败: ' + error.message);
                statusText.textContent = '生成失败';
            } finally {
                loadingOverlay.classList.remove('active');
            }
        };

        // 加载 XKT 文件
        async function loadXKTFile(url, filename) {
            const loadingOverlay = document.getElementById('loadingOverlay');
            const loadingText = document.getElementById('loadingText');
            const modelInfo = document.getElementById('modelInfo');

            loadingOverlay.classList.add('active');
            loadingText.textContent = '正在加载模型...';

            try {
                // 清除当前模型
                if (currentModel) {
                    xktLoader.unload(currentModel);
                    currentModel = null;
                }

                // 加载新模型
                const model = xktLoader.load({
                    id: 'model_' + Date.now(),
                    src: url,
                    edges: true,
                    performance: true
                });

                currentModel = model.id;

                // 等待加载完成
                await new Promise((resolve, reject) => {
                    model.on("loaded", () => {
                        viewer.cameraFlight.flyTo(model);
                        modelInfo.textContent = `模型: ${filename}`;
                        resolve();
                    });
                    model.on("error", reject);
                });

            } catch (error) {
                console.error('加载错误:', error);
                alert('加载失败: ' + error.message);
                modelInfo.textContent = '加载失败';
            } finally {
                loadingOverlay.classList.remove('active');
            }
        }

        // 刷新文件列表
        async function refreshFileList() {
            try {
                const response = await fetch('/api/xkt/list-files');
                const files = await response.json();

                const fileListDiv = document.getElementById('fileList');
                if (files && files.length > 0) {
                    // 筛选 web_ui 目录下的文件，并按时间排序
                    const webUIFiles = files
                        .filter(f => f.path.includes('output/web_ui/'))
                        .sort((a, b) => b.mtime - a.mtime);

                    fileListDiv.innerHTML = webUIFiles
                        .slice(0, 20)
                        .map(file => {
                            const filename = file.path.split('/').pop();
                            const date = new Date(file.mtime * 1000);
                            const dateStr = date.toLocaleString('zh-CN', {
                                month: '2-digit',
                                day: '2-digit',
                                hour: '2-digit',
                                minute: '2-digit'
                            });
                            return `
                                <div class="file-item" onclick="loadFile('${file.path}')">
                                    <div>${filename}</div>
                                    <div style="font-size: 11px; color: #64748b;">${dateStr} | ${(file.size / 1024 / 1024).toFixed(2)} MB</div>
                                </div>
                            `;
                        })
                        .join('');

                    if (webUIFiles.length === 0) {
                        fileListDiv.innerHTML = '<div class="file-item">暂无生成的文件</div>';
                    }
                } else {
                    fileListDiv.innerHTML = '<div class="file-item">暂无文件</div>';
                }
            } catch (error) {
                console.error('获取文件列表失败:', error);
            }
        }

        // 加载文件
        window.loadFile = async function(path) {
            const filename = path.split('/').pop();
            const url = `/files/${path}`;
            await loadXKTFile(url, filename);
            document.getElementById('statusText').textContent = `已加载: ${filename}`;
        };

        // 加载最新文件
        window.loadLatest = async function() {
            try {
                const response = await fetch('/api/xkt/list-files');
                const files = await response.json();

                const webUIFiles = files
                    .filter(f => f.path.includes('output/web_ui/'))
                    .sort((a, b) => b.mtime - a.mtime);

                if (webUIFiles.length > 0) {
                    await loadFile(webUIFiles[0].path);
                } else {
                    alert('没有找到文件');
                }
            } catch (error) {
                alert('加载失败: ' + error.message);
            }
        };

        // 设置快速值
        window.setQuickValues = function(dbnum, refno) {
            document.getElementById('dbnum').value = dbnum;
            document.getElementById('refno').value = refno;
        };

        // 初始化
        document.addEventListener('DOMContentLoaded', function() {
            initViewer();
            refreshFileList();
        });

        // 定期刷新文件列表
        setInterval(refreshFileList, 30000);
    </script>
</body>
</html>
"##.to_string()
}
