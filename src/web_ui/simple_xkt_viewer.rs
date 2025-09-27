/// 简化版 XKT 查看器页面
pub fn render_simple_xkt_viewer_page() -> String {
    r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>XKT 模型查看器</title>
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            background: #0f172a;
            color: #e2e8f0;
            display: flex;
            flex-direction: column;
            height: 100vh;
        }

        .header {
            background: rgba(15, 23, 42, 0.95);
            padding: 16px 24px;
            border-bottom: 1px solid rgba(148, 163, 184, 0.2);
            backdrop-filter: blur(8px);
        }

        .header h1 {
            font-size: 20px;
            font-weight: 600;
            color: #94a3b8;
            margin-bottom: 12px;
        }

        .input-group {
            display: flex;
            gap: 12px;
            align-items: center;
            max-width: 600px;
        }

        .input-group input {
            flex: 1;
            padding: 10px 16px;
            border-radius: 8px;
            border: 1px solid rgba(148, 163, 184, 0.3);
            background: rgba(30, 41, 59, 0.8);
            color: #e2e8f0;
            font-size: 14px;
            outline: none;
            transition: all 0.2s;
        }

        .input-group input:focus {
            border-color: #3b82f6;
            background: rgba(30, 41, 59, 1);
        }

        .input-group button {
            padding: 10px 24px;
            border-radius: 8px;
            border: none;
            background: #3b82f6;
            color: white;
            font-size: 14px;
            font-weight: 500;
            cursor: pointer;
            transition: all 0.2s;
        }

        .input-group button:hover {
            background: #2563eb;
            transform: translateY(-1px);
        }

        .input-group button:disabled {
            background: #475569;
            cursor: not-allowed;
            transform: none;
        }

        #viewerCanvas {
            flex: 1;
            width: 100%;
            display: block;
            background: #0b1120;
        }

        .status {
            position: fixed;
            bottom: 20px;
            right: 20px;
            padding: 12px 20px;
            background: rgba(15, 23, 42, 0.95);
            border: 1px solid rgba(148, 163, 184, 0.2);
            border-radius: 8px;
            font-size: 14px;
            backdrop-filter: blur(8px);
            display: none;
        }

        .status.show {
            display: block;
        }

        .status.success {
            border-color: #10b981;
            color: #10b981;
        }

        .status.error {
            border-color: #ef4444;
            color: #ef4444;
        }

        .status.loading {
            border-color: #f59e0b;
            color: #f59e0b;
        }

        .loading-spinner {
            display: inline-block;
            width: 14px;
            height: 14px;
            border: 2px solid rgba(59, 130, 246, 0.3);
            border-top-color: #3b82f6;
            border-radius: 50%;
            animation: spin 0.8s linear infinite;
            margin-right: 8px;
            vertical-align: middle;
        }

        @keyframes spin {
            to { transform: rotate(360deg); }
        }
    </style>
    <script src="https://cdn.jsdelivr.net/npm/@xeokit/xeokit-sdk/dist/xeokit-sdk.min.js"></script>
</head>
<body>
    <div class="header">
        <h1>XKT 模型查看器</h1>
        <div class="input-group">
            <input type="text" id="refnoInput" placeholder="输入参考号 (例如: 17496_266203)" />
            <button id="generateBtn" onclick="generateAndLoadModel()">生成并查看</button>
        </div>
    </div>

    <canvas id="viewerCanvas"></canvas>

    <div id="status" class="status"></div>

    <script>
        let viewer = null;
        let xktLoader = null;
        let currentModel = null;

        // 初始化查看器
        function initViewer() {
            if (!window.xeokit) {
                showStatus('XeoKit SDK 加载失败', 'error');
                return;
            }

            viewer = new xeokit.Viewer({
                canvasId: 'viewerCanvas',
                transparent: true
            });

            // 相机控制
            new xeokit.CameraControl(viewer, {
                doublePickFlyTo: true,
                followPointer: true
            });

            // 设置初始相机位置
            viewer.camera.eye = [100, 50, 100];
            viewer.camera.look = [0, 0, 0];
            viewer.camera.up = [0, 1, 0];

            // XKT 加载器
            xktLoader = new xeokit.XKTLoaderPlugin(viewer, {
                id: "XKTLoaderPlugin"
            });
        }

        // 显示状态信息
        function showStatus(message, type = 'info') {
            const statusEl = document.getElementById('status');
            statusEl.className = 'status show ' + type;

            if (type === 'loading') {
                statusEl.innerHTML = '<span class="loading-spinner"></span>' + message;
            } else {
                statusEl.innerHTML = message;
            }

            if (type !== 'loading') {
                setTimeout(() => {
                    statusEl.classList.remove('show');
                }, 3000);
            }
        }

        // 生成并加载模型
        async function generateAndLoadModel() {
            const refno = document.getElementById('refnoInput').value.trim();

            if (!refno) {
                showStatus('请输入参考号', 'error');
                return;
            }

            // 解析参考号格式 (可能是 17496_266203 或 17496/266203)
            let dbno, refnoOnly;
            if (refno.includes('_')) {
                [dbno, refnoOnly] = refno.split('_');
            } else if (refno.includes('/')) {
                [dbno, refnoOnly] = refno.split('/');
            } else {
                // 默认使用 17496 作为数据库号
                dbno = '17496';
                refnoOnly = refno;
            }

            const btn = document.getElementById('generateBtn');
            btn.disabled = true;
            btn.textContent = '生成中...';

            showStatus('正在生成 XKT 文件...', 'loading');

            try {
                // 调用 API 生成 XKT
                const response = await fetch('/api/xkt/generate', {
                    method: 'POST',
                    headers: {
                        'Content-Type': 'application/json'
                    },
                    body: JSON.stringify({
                        dbno: parseInt(dbno),
                        refno: dbno + '_' + refnoOnly,
                        compress: true
                    })
                });

                if (!response.ok) {
                    throw new Error('生成失败: ' + response.statusText);
                }

                const result = await response.json();

                if (result.success) {
                    showStatus('XKT 生成成功，正在加载模型...', 'loading');

                    // 清除之前的模型
                    if (currentModel) {
                        xktLoader.destroyModel(currentModel.id);
                        currentModel = null;
                    }

                    // 加载新模型
                    const modelId = 'model_' + Date.now();
                    currentModel = await xktLoader.load({
                        id: modelId,
                        src: result.url,
                        edges: true,
                        performance: true
                    });

                    // 自动调整相机到模型
                    viewer.cameraFlight.flyTo(modelId, { duration: 1.0 });

                    showStatus('模型加载成功！', 'success');
                } else {
                    throw new Error(result.error || '生成失败');
                }
            } catch (error) {
                console.error('Error:', error);
                showStatus('错误: ' + error.message, 'error');
            } finally {
                btn.disabled = false;
                btn.textContent = '生成并查看';
            }
        }

        // 监听回车键
        document.getElementById('refnoInput').addEventListener('keypress', function(e) {
            if (e.key === 'Enter') {
                generateAndLoadModel();
            }
        });

        // 页面加载完成后初始化
        window.addEventListener('DOMContentLoaded', initViewer);
    </script>
</body>
</html>"##
        .to_string()
}
