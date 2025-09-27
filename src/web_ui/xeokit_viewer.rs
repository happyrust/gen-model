use axum::{
    extract::{Path, Query},
    http::HeaderMap,
    response::{Html, IntoResponse, Json},
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Deserialize)]
pub struct ViewerParams {
    pub file: Option<String>,
}

pub async fn xeokit_viewer_page(Query(params): Query<ViewerParams>) -> impl IntoResponse {
    let default_file = "test_output/model_17496_266203_with_mesh_check.xkt".to_string();
    let xkt_file = params.file.unwrap_or(default_file);

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Xeokit XKT 模型查看器</title>
    <script src="https://cdn.jsdelivr.net/npm/@xeokit/xeokit-sdk@2.6.8/dist/xeokit-sdk.es.js" type="module"></script>
    <style>
        body {{
            margin: 0;
            padding: 0;
            font-family: Arial, sans-serif;
            background: #2a2a2a;
            color: white;
        }}

        #viewer-container {{
            position: relative;
            width: 100vw;
            height: 100vh;
            background: linear-gradient(to bottom, #87CEEB, #98FB98);
        }}

        #myCanvas {{
            width: 100%;
            height: 100%;
            position: absolute;
            top: 0;
            left: 0;
        }}

        .controls {{
            position: absolute;
            top: 20px;
            left: 20px;
            z-index: 1000;
            background: rgba(0,0,0,0.8);
            padding: 15px;
            border-radius: 8px;
            max-width: 300px;
        }}

        .controls h3 {{
            margin: 0 0 10px 0;
            color: #ffffff;
        }}

        .control-group {{
            margin-bottom: 15px;
        }}

        .control-group label {{
            display: block;
            margin-bottom: 5px;
            color: #cccccc;
        }}

        .control-group input, .control-group select, .control-group button {{
            width: 100%;
            padding: 5px;
            margin-bottom: 5px;
            border: 1px solid #555;
            border-radius: 4px;
            background: #333;
            color: white;
        }}

        .control-group button {{
            background: #007acc;
            border: none;
            cursor: pointer;
            padding: 8px;
        }}

        .control-group button:hover {{
            background: #005a9e;
        }}

        .status {{
            position: absolute;
            bottom: 20px;
            left: 20px;
            z-index: 1000;
            background: rgba(0,0,0,0.8);
            padding: 10px;
            border-radius: 5px;
            color: #cccccc;
            font-size: 12px;
            max-width: 400px;
        }}

        .error {{
            color: #ff6b6b;
        }}

        .success {{
            color: #51cf66;
        }}

        .info {{
            color: #339af0;
        }}

        /* 文件浏览器样式 */
        .file-browser {{
            position: absolute;
            top: 20px;
            right: 20px;
            z-index: 1000;
            background: rgba(0,0,0,0.9);
            padding: 15px;
            border-radius: 8px;
            width: 300px;
            max-height: 500px;
            display: none;
        }}

        .file-browser.active {{
            display: block;
        }}

        .file-browser h3 {{
            margin: 0 0 10px 0;
            color: #ffffff;
        }}

        .file-list {{
            max-height: 400px;
            overflow-y: auto;
        }}

        .file-item {{
            padding: 8px;
            cursor: pointer;
            border-radius: 4px;
            margin-bottom: 2px;
            display: flex;
            align-items: center;
        }}

        .file-item:hover {{
            background: rgba(255,255,255,0.1);
        }}

        .file-item.directory {{
            font-weight: bold;
            color: #339af0;
        }}

        .file-item.file {{
            color: #51cf66;
        }}

        .file-icon {{
            margin-right: 8px;
        }}

        .toggle-browser {{
            position: absolute;
            top: 20px;
            right: 340px;
            z-index: 1000;
            background: #007acc;
            color: white;
            border: none;
            padding: 10px 20px;
            border-radius: 5px;
            cursor: pointer;
        }}

        .toggle-browser:hover {{
            background: #005a9e;
        }}

        .breadcrumb {{
            margin-bottom: 10px;
            padding: 5px;
            background: rgba(255,255,255,0.1);
            border-radius: 4px;
            font-size: 12px;
        }}
    </style>
</head>
<body>
    <div id="viewer-container">
        <canvas id="myCanvas"></canvas>

        <button class="toggle-browser" onclick="toggleFileBrowser()">📁 浏览文件</button>

        <div class="controls">
            <h3>XKT 模型查看器</h3>

            <div class="control-group">
                <label>XKT 文件选择:</label>
                <input type="file" id="file-input" accept=".xkt" style="display: none;" onchange="handleFileSelect(event)">
                <button onclick="document.getElementById('file-input').click()">📁 选择本地文件</button>
                <input type="text" id="file-path" value="{}" placeholder="或输入XKT文件路径">
                <button onclick="loadModel()">加载模型</button>
            </div>

            <div class="control-group">
                <label>预设模型:</label>
                <select id="preset-models" onchange="loadPresetModel()">
                    <option value="">选择预设模型</option>
                    <option value="output/web_ui/db1112_compressed_20250926075802.xkt">17496/256215 (最新生成)</option>
                    <option value="test_output/model_17496_266203_with_mesh_check.xkt">17496_266203 (新生成)</option>
                    <option value="test_output/force_mesh_regenerate.xkt">强制生成mesh模型</option>
                    <option value="test_with_mesh.xkt">测试mesh模型</option>
                    <option value="test_debug_mesh.xkt">调试mesh模型</option>
                </select>
            </div>

            <div class="control-group">
                <label>视图控制:</label>
                <button onclick="viewer.cameraFlight.flyTo(viewer.scene)">适应视图</button>
                <button onclick="viewer.cameraControl.planView()">平面视图</button>
                <button onclick="resetCamera()">重置相机</button>
            </div>

            <div class="control-group">
                <label>显示模式:</label>
                <button onclick="toggleWireframe()">切换线框</button>
                <button onclick="toggleXray()">切换X光</button>
                <button onclick="showStats()">显示统计</button>
            </div>
        </div>

        <div class="file-browser" id="fileBrowser">
            <h3>文件浏览器</h3>
            <div class="breadcrumb" id="breadcrumb">当前路径: /</div>
            <div class="file-list" id="fileList">
                <!-- 文件列表将动态加载 -->
            </div>
        </div>

        <div class="status" id="status">
            准备就绪 - 请加载XKT模型
        </div>
    </div>

    <script type="module">
        import {{ Viewer, XKTLoaderPlugin }} from "https://cdn.jsdelivr.net/npm/@xeokit/xeokit-sdk@2.6.8/dist/xeokit-sdk.es.js";

        // 全局变量
        let viewer;
        let xktLoader;
        let currentModel;
        let wireframeMode = false;
        let xrayMode = false;
        let currentPath = "test_output";

        // 初始化查看器
        function initViewer() {{
            try {{
                viewer = new Viewer({{
                    canvasId: "myCanvas",
                    transparent: true,
                    alphaDepthMask: true
                }});

                viewer.scene.camera.eye = [10, 10, 10];
                viewer.scene.camera.look = [0, 0, 0];
                viewer.scene.camera.up = [0, 1, 0];

                xktLoader = new XKTLoaderPlugin(viewer);

                updateStatus("查看器初始化成功", "success");

                // 自动加载默认模型
                if ("{}" !== "") {{
                    loadModel();
                }}

                // 初始化文件浏览器
                loadFileList(currentPath);
            }} catch (error) {{
                updateStatus(`初始化失败: ${{error.message}}`, "error");
                console.error("初始化错误:", error);
            }}
        }}

        // 处理本地文件选择
        function handleFileSelect(event) {{
            const file = event.target.files[0];
            if (!file) return;

            if (!file.name.toLowerCase().endsWith('.xkt')) {{
                updateStatus("请选择 .xkt 格式的文件", "error");
                return;
            }}

            updateStatus(`正在加载本地文件: ${{file.name}}`, "info");

            // 清除之前的模型
            if (currentModel) {{
                currentModel.destroy();
            }}

            try {{
                // 读取文件并转换为 Blob URL
                const reader = new FileReader();
                reader.onload = function(e) {{
                    const arrayBuffer = e.target.result;
                    const blob = new Blob([arrayBuffer], {{ type: 'application/octet-stream' }});
                    const url = URL.createObjectURL(blob);

                    currentModel = xktLoader.load({{
                        id: "localModel",
                        src: url,
                        performanceModel: true,
                        edges: true,
                        saoEnabled: true,
                        dtxEnabled: true
                    }});

                    currentModel.on("loaded", () => {{
                        updateStatus(`本地文件加载成功: ${{file.name}}`, "success");
                        viewer.cameraFlight.flyTo(viewer.scene);
                        showModelStats();
                        // 清理 URL
                        URL.revokeObjectURL(url);
                    }});

                    currentModel.on("error", (error) => {{
                        updateStatus(`加载失败: ${{error}}`, "error");
                        console.error("加载错误:", error);
                        URL.revokeObjectURL(url);
                    }});
                }};

                reader.onerror = function() {{
                    updateStatus("文件读取失败", "error");
                }};

                reader.readAsArrayBuffer(file);

            }} catch (error) {{
                updateStatus(`加载异常: ${{error.message}}`, "error");
                console.error("加载异常:", error);
            }}
        }}

        // 加载模型 (服务器路径)
        function loadModel() {{
            const filePath = document.getElementById('file-path').value;
            if (!filePath) {{
                updateStatus("请输入XKT文件路径或选择本地文件", "error");
                return;
            }}

            updateStatus(`正在加载模型: ${{filePath}}`, "info");

            // 清除之前的模型
            if (currentModel) {{
                currentModel.destroy();
            }}

            try {{
                currentModel = xktLoader.load({{
                    id: "myModel",
                    src: `/files/${{filePath}}`,

                    performanceModel: true,

                    edges: true,

                    saoEnabled: true,

                    dtxEnabled: true
                }});

                currentModel.on("loaded", () => {{
                    updateStatus("模型加载成功", "success");
                    viewer.cameraFlight.flyTo(viewer.scene);
                    showModelStats();
                }});

                currentModel.on("error", (error) => {{
                    updateStatus(`加载失败: ${{error}}`, "error");
                    console.error("加载错误:", error);
                }});

            }} catch (error) {{
                updateStatus(`加载异常: ${{error.message}}`, "error");
                console.error("加载异常:", error);
            }}
        }}

        // 加载预设模型
        function loadPresetModel() {{
            const select = document.getElementById('preset-models');
            const filePath = select.value;
            if (filePath) {{
                document.getElementById('file-path').value = filePath;
                loadModel();
            }}
        }}

        // 重置相机
        function resetCamera() {{
            if (viewer) {{
                viewer.scene.camera.eye = [10, 10, 10];
                viewer.scene.camera.look = [0, 0, 0];
                viewer.scene.camera.up = [0, 1, 0];
                updateStatus("相机已重置", "info");
            }}
        }}

        // 切换线框模式
        function toggleWireframe() {{
            if (viewer && currentModel) {{
                wireframeMode = !wireframeMode;
                currentModel.edges = wireframeMode;
                updateStatus(`线框模式: ${{wireframeMode ? '开启' : '关闭'}}`, "info");
            }}
        }}

        // 切换X光模式
        function toggleXray() {{
            if (viewer && currentModel) {{
                xrayMode = !xrayMode;
                currentModel.xrayed = xrayMode;
                updateStatus(`X光模式: ${{xrayMode ? '开启' : '关闭'}}`, "info");
            }}
        }}

        // 显示统计信息
        function showStats() {{
            if (currentModel) {{
                showModelStats();
            }} else {{
                updateStatus("未加载模型", "error");
            }}
        }}

        // 显示模型统计
        function showModelStats() {{
            if (currentModel) {{
                const numObjects = currentModel.numObjects;
                const numGeometries = currentModel.numGeometries;
                const numTriangles = currentModel.numTriangles;
                const numVertices = currentModel.numVertices;

                const stats = `模型统计: 对象=${{numObjects}}, 几何体=${{numGeometries}}, 三角形=${{numTriangles}}, 顶点=${{numVertices}}`;
                updateStatus(stats, "info");
            }}
        }}

        // 更新状态显示
        function updateStatus(message, type = "info") {{
            const statusDiv = document.getElementById('status');
            statusDiv.className = `status ${{type}}`;
            statusDiv.textContent = `[${{new Date().toLocaleTimeString()}}] ${{message}}`;
        }}

        // 切换文件浏览器
        function toggleFileBrowser() {{
            const browser = document.getElementById('fileBrowser');
            browser.classList.toggle('active');
        }}

        // 加载文件列表
        async function loadFileList(path) {{
            try {{
                const response = await fetch(`/api/xkt/list-files?path=${{encodeURIComponent(path)}}`);
                const files = await response.json();

                currentPath = path;
                document.getElementById('breadcrumb').textContent = `当前路径: /${{path}}`;

                const fileList = document.getElementById('fileList');
                fileList.innerHTML = '';

                // 添加上级目录
                if (path && path !== '') {{
                    const parentItem = document.createElement('div');
                    parentItem.className = 'file-item directory';
                    parentItem.innerHTML = '<span class="file-icon">📁</span><span>..</span>';
                    parentItem.onclick = () => {{
                        const parentPath = path.split('/').slice(0, -1).join('/');
                        loadFileList(parentPath);
                    }};
                    fileList.appendChild(parentItem);
                }}

                // 添加文件和目录
                files.forEach(file => {{
                    const item = document.createElement('div');
                    item.className = `file-item ${{file.is_dir ? 'directory' : 'file'}}`;

                    const icon = file.is_dir ? '📁' : '📄';
                    const sizeText = file.is_dir ? '' : ` (${{(file.size / 1024).toFixed(1)}} KB)`;

                    item.innerHTML = `<span class="file-icon">${{icon}}</span><span>${{file.name}}${{sizeText}}</span>`;

                    item.onclick = () => {{
                        if (file.is_dir) {{
                            loadFileList(file.path);
                        }} else {{
                            document.getElementById('file-path').value = file.path;
                            loadModel();
                            toggleFileBrowser();
                        }}
                    }};

                    fileList.appendChild(item);
                }});
            }} catch (error) {{
                console.error('加载文件列表失败:', error);
                updateStatus('加载文件列表失败', 'error');
            }}
        }}

        // 将所有函数暴露到全局作用域
        window.handleFileSelect = handleFileSelect;
        window.loadModel = loadModel;
        window.loadPresetModel = loadPresetModel;
        window.resetCamera = resetCamera;
        window.toggleWireframe = toggleWireframe;
        window.toggleXray = toggleXray;
        window.showStats = showStats;
        window.toggleFileBrowser = toggleFileBrowser;

        // 页面加载完成后初始化
        document.addEventListener('DOMContentLoaded', initViewer);

        // 将viewer暴露到全局作用域，方便调试
        window.viewer = viewer;
    </script>
</body>
</html>"#,
        xkt_file, xkt_file
    );

    Html(html)
}

pub async fn serve_xkt_file(Path(filepath): Path<String>) -> impl IntoResponse {
    use axum::http::StatusCode;
    use std::fs;

    // 处理嵌套路径，如 test_output/file.xkt
    let full_path = format!("/Volumes/DPC/work/gen-model/{}", filepath);

    println!("尝试访问文件: {}", full_path); // 调试信息

    match fs::read(&full_path) {
        Ok(content) => {
            let mut headers = HeaderMap::new();
            headers.insert("Content-Type", "application/octet-stream".parse().unwrap());
            headers.insert("Access-Control-Allow-Origin", "*".parse().unwrap());
            println!("成功读取文件，大小: {} bytes", content.len());
            (StatusCode::OK, headers, content)
        }
        Err(e) => {
            println!("文件读取失败: {}", e);
            (StatusCode::NOT_FOUND, HeaderMap::new(), Vec::new())
        }
    }
}

#[derive(Serialize)]
pub struct FileInfo {
    name: String,
    path: String,
    size: u64,
    is_dir: bool,
}

#[derive(Deserialize)]
pub struct ListFilesParams {
    pub path: Option<String>,
}

pub async fn list_xkt_files(Query(params): Query<ListFilesParams>) -> impl IntoResponse {
    use axum::http::StatusCode;

    let base_path = "/Volumes/DPC/work/gen-model";
    let search_path = params.path.unwrap_or_else(|| "test_output".to_string());
    let full_path = format!("{}/{}", base_path, search_path);

    println!("列出目录: {}", full_path);

    let path = PathBuf::from(&full_path);

    if !path.exists() {
        return (StatusCode::NOT_FOUND, Json(Vec::<FileInfo>::new())).into_response();
    }

    let mut files = Vec::new();

    // 读取目录内容
    match fs::read_dir(&path) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let metadata = entry.metadata().ok();
                let file_name = entry.file_name().to_string_lossy().to_string();

                // 如果是目录或者是.xkt文件，添加到列表
                if let Some(meta) = metadata {
                    if meta.is_dir() || file_name.ends_with(".xkt") || file_name.ends_with(".mesh")
                    {
                        let relative_path = if search_path.is_empty() {
                            file_name.clone()
                        } else {
                            format!("{}/{}", search_path, file_name)
                        };
                        files.push(FileInfo {
                            name: file_name,
                            path: relative_path,
                            size: meta.len(),
                            is_dir: meta.is_dir(),
                        });
                    }
                }
            }
        }
        Err(e) => {
            println!("读取目录失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(Vec::<FileInfo>::new()),
            )
                .into_response();
        }
    }

    // 排序：目录在前，文件在后
    files.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });

    (StatusCode::OK, Json(files)).into_response()
}
