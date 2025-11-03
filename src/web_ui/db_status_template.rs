pub fn db_status_page() -> String {
    r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>数据库状态管理</title>
    <style>
        * {
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }
        
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            padding: 20px;
        }
        
        .container {
            max-width: 1400px;
            margin: 0 auto;
        }
        
        .header {
            background: white;
            border-radius: 10px;
            padding: 30px;
            margin-bottom: 20px;
            box-shadow: 0 4px 6px rgba(0, 0, 0, 0.1);
        }
        
        .header h1 {
            color: #333;
            margin-bottom: 10px;
        }
        
        .header p {
            color: #666;
        }
        
        .nav-tabs {
            display: flex;
            gap: 10px;
            margin-bottom: 20px;
        }
        
        .nav-tab {
            padding: 10px 20px;
            background: white;
            border: none;
            border-radius: 5px;
            cursor: pointer;
            transition: all 0.3s;
            font-size: 14px;
            font-weight: 500;
        }
        
        .nav-tab:hover {
            background: #f0f0f0;
        }
        
        .nav-tab.active {
            background: #667eea;
            color: white;
        }
        
        .filters {
            background: white;
            border-radius: 10px;
            padding: 20px;
            margin-bottom: 20px;
            box-shadow: 0 2px 4px rgba(0, 0, 0, 0.1);
        }
        
        .filter-row {
            display: flex;
            gap: 15px;
            align-items: center;
            flex-wrap: wrap;
        }
        
        .filter-group {
            display: flex;
            flex-direction: column;
            gap: 5px;
        }
        
        .filter-group label {
            font-size: 12px;
            color: #666;
            font-weight: 500;
        }
        
        .filter-group select,
        .filter-group input {
            padding: 8px 12px;
            border: 1px solid #ddd;
            border-radius: 5px;
            font-size: 14px;
        }
        
        .btn {
            padding: 8px 16px;
            border: none;
            border-radius: 5px;
            cursor: pointer;
            font-size: 14px;
            font-weight: 500;
            transition: all 0.3s;
        }
        
        .btn-primary {
            background: #667eea;
            color: white;
        }
        
        .btn-primary:hover {
            background: #5a67d8;
        }
        
        .btn-success {
            background: #48bb78;
            color: white;
        }
        
        .btn-success:hover {
            background: #38a169;
        }
        
        .btn-warning {
            background: #ed8936;
            color: white;
        }
        
        .btn-warning:hover {
            background: #dd6b20;
        }
        
        .db-grid {
            display: grid;
            grid-template-columns: repeat(auto-fill, minmax(350px, 1fr));
            gap: 20px;
        }
        
        .db-card {
            background: white;
            border-radius: 10px;
            padding: 20px;
            box-shadow: 0 2px 4px rgba(0, 0, 0, 0.1);
            transition: transform 0.3s, box-shadow 0.3s;
        }
        
        .db-card:hover {
            transform: translateY(-2px);
            box-shadow: 0 4px 8px rgba(0, 0, 0, 0.15);
        }
        
        .db-card-header {
            display: flex;
            justify-content: space-between;
            align-items: center;
            margin-bottom: 15px;
            padding-bottom: 10px;
            border-bottom: 2px solid #f0f0f0;
        }
        
        .db-number {
            font-size: 20px;
            font-weight: bold;
            color: #333;
        }
        
        .db-type {
            background: #e2e8f0;
            color: #4a5568;
            padding: 4px 8px;
            border-radius: 4px;
            font-size: 12px;
            font-weight: 500;
        }
        
        .db-info {
            display: grid;
            grid-template-columns: 1fr 1fr;
            gap: 10px;
            margin-bottom: 15px;
        }
        
        .info-item {
            display: flex;
            flex-direction: column;
        }
        
        .info-label {
            font-size: 11px;
            color: #999;
            margin-bottom: 2px;
        }
        
        .info-value {
            font-size: 14px;
            color: #333;
            font-weight: 500;
        }
        
        .status-row {
            display: flex;
            gap: 10px;
            margin-bottom: 15px;
        }
        
        .status-badge {
            flex: 1;
            padding: 8px;
            border-radius: 5px;
            text-align: center;
            font-size: 12px;
            font-weight: 500;
        }
        
        .status-parsed {
            background: #c6f6d5;
            color: #22543d;
        }
        
        .status-not-parsed {
            background: #fed7d7;
            color: #742a2a;
        }
        
        .status-generating {
            background: #fef5e7;
            color: #744210;
        }
        
        .status-generated {
            background: #c6f6d5;
            color: #22543d;
        }
        
        .update-indicator {
            background: #fff5f5;
            border: 1px solid #feb2b2;
            border-radius: 5px;
            padding: 10px;
            margin-bottom: 15px;
            display: flex;
            align-items: center;
            gap: 10px;
        }
        
        .update-icon {
            color: #e53e3e;
            font-size: 20px;
        }
        
        .update-text {
            flex: 1;
            font-size: 13px;
            color: #742a2a;
        }
        
        .card-actions {
            display: flex;
            gap: 10px;
        }
        
        .card-actions button {
            flex: 1;
            padding: 8px;
            font-size: 13px;
        }
        
        .modal {
            display: none;
            position: fixed;
            top: 0;
            left: 0;
            width: 100%;
            height: 100%;
            background: rgba(0, 0, 0, 0.5);
            z-index: 1000;
            align-items: center;
            justify-content: center;
        }
        
        .modal.show {
            display: flex;
        }
        
        .modal-content {
            background: white;
            border-radius: 10px;
            padding: 30px;
            max-width: 600px;
            width: 90%;
            max-height: 80vh;
            overflow-y: auto;
        }
        
        .modal-header {
            margin-bottom: 20px;
        }
        
        .modal-title {
            font-size: 24px;
            color: #333;
            margin-bottom: 10px;
        }
        
        .modal-body {
            margin-bottom: 20px;
        }
        
        .detail-section {
            margin-bottom: 20px;
        }
        
        .detail-section h3 {
            font-size: 16px;
            color: #666;
            margin-bottom: 10px;
        }
        
        .detail-table {
            width: 100%;
            border-collapse: collapse;
        }
        
        .detail-table td {
            padding: 8px;
            border-bottom: 1px solid #f0f0f0;
        }
        
        .detail-table td:first-child {
            font-weight: 500;
            color: #666;
            width: 40%;
        }
        
        .version-compare {
            display: flex;
            gap: 20px;
            margin: 15px 0;
        }
        
        .version-box {
            flex: 1;
            padding: 15px;
            border: 1px solid #e2e8f0;
            border-radius: 5px;
        }
        
        .version-box h4 {
            font-size: 14px;
            color: #666;
            margin-bottom: 10px;
        }
        
        .version-number {
            font-size: 24px;
            font-weight: bold;
            color: #333;
        }
        
        .change-list {
            list-style: none;
            padding: 0;
        }
        
        .change-item {
            padding: 8px;
            margin-bottom: 5px;
            background: #f7fafc;
            border-left: 3px solid #667eea;
            font-size: 13px;
        }
        
        .loading {
            text-align: center;
            padding: 40px;
            color: #666;
        }
        
        .spinner {
            border: 3px solid #f3f3f3;
            border-top: 3px solid #667eea;
            border-radius: 50%;
            width: 40px;
            height: 40px;
            animation: spin 1s linear infinite;
            margin: 0 auto 20px;
        }
        
        @keyframes spin {
            0% { transform: rotate(0deg); }
            100% { transform: rotate(360deg); }
        }
        
        .empty-state {
            text-align: center;
            padding: 60px 20px;
            color: #999;
        }
        
        .empty-icon {
            font-size: 48px;
            margin-bottom: 20px;
        }
        
        .batch-actions {
            background: white;
            border-radius: 10px;
            padding: 15px;
            margin-bottom: 20px;
            display: flex;
            justify-content: space-between;
            align-items: center;
            box-shadow: 0 2px 4px rgba(0, 0, 0, 0.1);
        }
        
        .selected-count {
            font-size: 14px;
            color: #666;
        }
        
        .checkbox-wrapper {
            display: flex;
            align-items: center;
            gap: 8px;
            margin-bottom: 10px;
        }
        
        input[type="checkbox"] {
            width: 16px;
            height: 16px;
            cursor: pointer;
        }
    </style>
</head>
<body>
    <div class="container">
        <div class="header">
            <h1>🗄️ 数据库状态管理</h1>
            <p>监控和管理数据库解析、模型生成状态，执行增量更新</p>
        </div>
        
        <div class="nav-tabs">
            <button class="nav-tab active" onclick="switchTab('overview')">总览</button>
            <button class="nav-tab" onclick="switchTab('needsUpdate')">待更新</button>
            <button class="nav-tab" onclick="switchTab('history')">历史记录</button>
            <button class="nav-tab" onclick="window.location.href='/dashboard'">返回仪表板</button>
        </div>
        
        <div class="filters">
            <div class="filter-row">
                <div class="filter-group">
                    <label>项目名称</label>
                    <select id="projectFilter">
                        <option value="">全部项目</option>
                        <option value="AvevaMarineSample">AvevaMarineSample</option>
                    </select>
                </div>
                <div class="filter-group">
                    <label>数据库类型</label>
                    <select id="dbTypeFilter">
                        <option value="">全部类型</option>
                        <option value="CATA">CATA</option>
                        <option value="DESI">DESI</option>
                        <option value="DICT">DICT</option>
                    </select>
                </div>
                <div class="filter-group">
                    <label>状态筛选</label>
                    <select id="statusFilter">
                        <option value="">全部状态</option>
                        <option value="parsed">已解析</option>
                        <option value="not_parsed">未解析</option>
                        <option value="generated">已生成</option>
                        <option value="needs_update">需要更新</option>
                    </select>
                </div>
                <button class="btn btn-primary" onclick="applyFilters()">应用筛选</button>
                <button class="btn btn-success" onclick="checkAllVersions()">检查版本</button>
                <button class="btn btn-primary" onclick="scanLocal()">扫描本地</button>
                <button class="btn btn-success" onclick="syncFileMeta()">同步文件版本→SurrealDB</button>
                <button class="btn btn-warning" onclick="rescanAndCache()">写入本地缓存（redb）</button>
            </div>
        </div>
        
        <div class="batch-actions" id="batchActions" style="display: none;">
            <div class="selected-count">
                已选择 <span id="selectedCount">0</span> 个数据库
            </div>
            <div>
                <button class="btn btn-warning" onclick="batchUpdate()">批量增量更新</button>
                <button class="btn btn-primary" onclick="clearSelection()">清除选择</button>
            </div>
        </div>
        
        <div id="dbGrid" class="db-grid">
            <div class="loading">
                <div class="spinner"></div>
                <p>正在加载数据库状态...</p>
            </div>
        </div>
    </div>
    
    <div id="detailModal" class="modal">
        <div class="modal-content">
            <div class="modal-header">
                <h2 class="modal-title">数据库详情</h2>
            </div>
            <div class="modal-body" id="modalBody">
            </div>
            <div class="card-actions">
                <button class="btn btn-primary" onclick="closeModal()">关闭</button>
            </div>
        </div>
    </div>
    
    <script>
        let selectedDbs = new Set();
        let currentTab = 'overview';
        let dbStatusData = [];
        
        async function loadDbStatus() {
            try {
                const response = await fetch('/api/db-status');
                const result = await response.json();
                
                if (result.status === 'success') {
                    dbStatusData = result.data;
                    renderDbCards(result.data);
                }
            } catch (error) {
                console.error('加载数据库状态失败:', error);
                document.getElementById('dbGrid').innerHTML = '<div class="empty-state"><div class="empty-icon">❌</div><p>加载失败，请重试</p></div>';
            }
        }
        
        function renderDbCards(data) {
            const grid = document.getElementById('dbGrid');
            
            if (data.length === 0) {
                grid.innerHTML = '<div class="empty-state"><div class="empty-icon">📭</div><p>没有找到匹配的数据库</p></div>';
                return;
            }
            
            grid.innerHTML = data.map(db => `
                <div class="db-card" data-dbnum="${db.dbnum}">
                    <div class="checkbox-wrapper">
                        <input type="checkbox" id="select-${db.dbnum}" onchange="toggleSelection(${db.dbnum})">
                        <label for="select-${db.dbnum}">选择此数据库</label>
                    </div>
                    <div class="db-card-header">
                        <span class="db-number">DB ${db.dbnum}</span>
                        <span class="db-type">${db.db_type}</span>
                    </div>
                    
                    <div class="db-info">
                        <div class="info-item">
                            <span class="info-label">文件名</span>
                            <span class="info-value">${db.file_name}</span>
                        </div>
                        <div class="info-item">
                            <span class="info-label">项目</span>
                            <span class="info-value">${db.project}</span>
                        </div>
                        <div class="info-item">
                            <span class="info-label">记录数</span>
                            <span class="info-value">${db.count.toLocaleString()}</span>
                        </div>
                        <div class="info-item">
                            <span class="info-label">会话号</span>
                            <span class="info-value">${db.sesno}</span>
                        </div>
                        <div class="info-item">
                            <span class="info-label">本地sesno</span>
                            <span class="info-value">${db.cached_sesno ?? '-'}</span>
                        </div>
                        <div class="info-item">
                            <span class="info-label">文件sesno</span>
                            <span class="info-value">${db.latest_file_sesno ?? '-'}</span>
                        </div>
                    </div>
                    
                    <div class="status-row">
                        <div class="status-badge ${getParseStatusClass(db.parse_status)}">
                            ${getParseStatusText(db.parse_status)}
                        </div>
                        <div class="status-badge ${getModelStatusClass(db.model_status)}">
                            ${getModelStatusText(db.model_status)}
                        </div>
                        <div class="status-badge ${getMeshStatusClass(db.mesh_status)}">
                            ${getMeshStatusText(db.mesh_status)}
                        </div>
                    </div>
                    
                    ${db.needs_update ? `
                        <div class="update-indicator">
                            <span class="update-icon">${db.updating ? '⏳' : '⚠️'}</span>
                            <span class="update-text">${db.updating ? '增量更新进行中...' : '检测到文件版本更新，需要重新处理'}</span>
                        </div>
                    ` : ''}
                    
                    <div class="card-actions">
                        <button class="btn btn-primary" onclick="showDetail(${db.dbnum})">查看详情</button>
                        ${db.needs_update ? 
                            `<button class="btn btn-warning" ${db.updating ? 'disabled' : ''} onclick="incrementalUpdate(${db.dbnum})">${db.updating ? '更新中' : '增量更新'}</button>` : 
                            `<button class="btn btn-success" onclick="checkVersion(${db.dbnum})">检查版本</button>`
                        }
                        <label style="margin-left:8px;font-size:12px;">
                            <input type="checkbox" ${db.auto_update ? 'checked' : ''} onchange="toggleAutoUpdate(${db.dbnum}, this.checked)"> 自动更新
                        </label>
                        <select style="margin-left:6px;font-size:12px;" onchange="setAutoUpdateType(${db.dbnum}, this.value)">
                            <option value="ParseOnly" ${db.auto_update_type === 'ParseOnly' ? 'selected' : ''}>仅解析</option>
                            <option value="ParseAndModel" ${db.auto_update_type === 'ParseAndModel' ? 'selected' : ''}>解析+建模</option>
                            <option value="Full" ${db.auto_update_type === 'Full' ? 'selected' : ''}>完整</option>
                        </select>
                    </div>
                    ${db.last_update_result ? `<div style=\"margin-top:8px;font-size:12px;color:#666;\">上次更新: ${db.last_update_result}</div>` : ''}
                </div>
            `).join('');
        }
        
        function getParseStatusClass(status) {
            switch(status) {
                case 'Parsed': return 'status-parsed';
                case 'Parsing': return 'status-generating';
                default: return 'status-not-parsed';
            }
        }
        
        function getParseStatusText(status) {
            switch(status) {
                case 'Parsed': return '✓ 已解析';
                case 'Parsing': return '⏳ 解析中';
                case 'ParseFailed': return '✗ 解析失败';
                default: return '○ 未解析';
            }
        }
        
        function getModelStatusClass(status) {
            switch(status) {
                case 'Generated': return 'status-generated';
                case 'Generating': return 'status-generating';
                default: return 'status-not-parsed';
            }
        }
        
        function getModelStatusText(status) {
            switch(status) {
                case 'Generated': return '✓ 模型已生成';
                case 'Generating': return '⏳ 生成中';
                case 'GenerationFailed': return '✗ 生成失败';
                default: return '○ 未生成';
            }
        }
        
        function getMeshStatusClass(status) {
            switch(status) {
                case 'Generated': return 'status-generated';
                case 'Generating': return 'status-generating';
                default: return 'status-not-parsed';
            }
        }
        
        function getMeshStatusText(status) {
            switch(status) {
                case 'Generated': return '✓ 网格已生成';
                case 'Generating': return '⏳ 生成中';
                case 'GenerationFailed': return '✗ 生成失败';
                default: return '○ 未生成';
            }
        }
        
        function toggleSelection(dbnum) {
            if (selectedDbs.has(dbnum)) {
                selectedDbs.delete(dbnum);
            } else {
                selectedDbs.add(dbnum);
            }
            updateBatchActions();
        }
        
        function updateBatchActions() {
            const batchActions = document.getElementById('batchActions');
            const selectedCount = document.getElementById('selectedCount');
            
            if (selectedDbs.size > 0) {
                batchActions.style.display = 'flex';
                selectedCount.textContent = selectedDbs.size;
            } else {
                batchActions.style.display = 'none';
            }
        }
        
        function clearSelection() {
            selectedDbs.clear();
            document.querySelectorAll('input[type="checkbox"]').forEach(cb => cb.checked = false);
            updateBatchActions();
        }
        
        async function batchUpdate() {
            if (selectedDbs.size === 0) return;
            
            const dbnums = Array.from(selectedDbs);
            
            if (!confirm(`确定要对 ${dbnums.length} 个数据库执行增量更新吗？`)) {
                return;
            }
            
            try {
                // 可选目标 sesno 提示
                let ses = prompt('可选: 输入目标 sesno（留空则按最新）');
                let targetValue = null;
                if (ses !== null && ses.trim() !== '' && !Number.isNaN(Number(ses))) {
                    targetValue = Number(ses);
                }
                const response = await fetch('/api/db-status/update', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        dbnums: dbnums,
                        force_update: false,
                        update_type: 'ParseAndModel',
                        ...(targetValue !== null ? { target_sesno: targetValue } : {})
                    })
                });
                
                const result = await response.json();
                if (result.status === 'success') {
                    alert(result.message);
                    clearSelection();
                    loadDbStatus();
                }
            } catch (error) {
                alert('批量更新失败: ' + error.message);
            }
        }
        
        async function incrementalUpdate(dbnum) {
            if (!confirm(`确定要对数据库 ${dbnum} 执行增量更新吗？`)) {
                return;
            }
            
            try {
                // 可选目标 sesno 提示
                let ses = prompt('可选: 输入目标 sesno（留空则按最新）');
                let targetValue = null;
                if (ses !== null && ses.trim() !== '' && !Number.isNaN(Number(ses))) {
                    targetValue = Number(ses);
                }
                const response = await fetch('/api/db-status/update', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        dbnums: [dbnum],
                        force_update: false,
                        update_type: 'Full',
                        ...(targetValue !== null ? { target_sesno: targetValue } : {})
                    })
                });
                
                const result = await response.json();
                if (result.status === 'success') {
                    alert('增量更新任务已创建');
                    loadDbStatus();
                }
            } catch (error) {
                alert('更新失败: ' + error.message);
            }
        }
        
        async function checkVersion(dbnum) {
            try {
                const response = await fetch(`/api/db-status/check-versions?dbnum=${dbnum}`);
                const result = await response.json();
                
                if (result.status === 'success') {
                    const versionInfo = result.data[0];
                    if (versionInfo && versionInfo.needs_update) {
                        if (confirm('检测到新版本，是否立即更新？')) {
                            incrementalUpdate(dbnum);
                        }
                    } else {
                        alert('当前版本已是最新');
                    }
                }
            } catch (error) {
                alert('版本检查失败: ' + error.message);
            }
        }
        
        async function checkAllVersions() {
            try {
                const response = await fetch('/api/db-status/check-versions');
                const result = await response.json();
                
                if (result.status === 'success') {
                    const needsUpdate = result.total_needs_update;
                    if (needsUpdate > 0) {
                        alert(`检测到 ${needsUpdate} 个数据库需要更新`);
                        loadDbStatus();
                    } else {
                        alert('所有数据库都是最新版本');
                    }
                }
            } catch (error) {
                alert('版本检查失败: ' + error.message);
            }
        }

        async function scanLocal() {
            try {
                const res = await fetch('/api/db-sync/scan');
                const data = await res.json();
                if (data.status === 'success') {
                    alert(`扫描完成，需更新: ${data.data.needs_update_count} / ${data.data.total}`);
                    loadDbStatus();
                }
            } catch (e) { alert('扫描失败: ' + e.message); }
        }

        async function syncFileMeta() {
            try {
                const res = await fetch('/api/db-sync/sync', {
                    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({})
                });
                const data = await res.json();
                if (data.status === 'success') {
                    alert('同步完成');
                    loadDbStatus();
                }
            } catch (e) { alert('同步失败: ' + e.message); }
        }

        async function rescanAndCache() {
            try {
                const res = await fetch('/api/db-sync/rescan', {
                    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({})
                });
                const data = await res.json();
                if (data.status === 'success') {
                    alert(`已写入本地缓存: ${data.updated}`);
                    loadDbStatus();
                }
            } catch (e) { alert('写入失败: ' + e.message); }
        }
        
        async function toggleAutoUpdate(dbnum, enabled) {
            try {
                const response = await fetch(`/api/db-status/${dbnum}/auto-update`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ auto_update: enabled })
                });
                const result = await response.json();
                if (result.status !== 'success') {
                    alert('设置失败');
                }
            } catch (error) {
                alert('设置失败: ' + error.message);
            }
        }

        async function setAutoUpdateType(dbnum, t) {
            try {
                const response = await fetch(`/api/db-status/${dbnum}/auto-update-type`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ auto_update_type: t })
                });
                const result = await response.json();
                if (result.status !== 'success') {
                    alert('设置自动更新类型失败');
                }
            } catch (error) {
                alert('设置失败: ' + error.message);
            }
        }
        
        async function showDetail(dbnum) {
            try {
                const response = await fetch(`/api/db-status/${dbnum}`);
                const result = await response.json();
                
                if (result.status === 'success') {
                    const data = result.data;
                    const modalBody = document.getElementById('modalBody');
                    
                    modalBody.innerHTML = `
                        <div class="detail-section">
                            <h3>基本信息</h3>
                            <table class="detail-table">
                                <tr><td>数据库编号</td><td>${data.basic_info.dbnum}</td></tr>
                                <tr><td>文件名</td><td>${data.basic_info.file_name}</td></tr>
                                <tr><td>项目</td><td>${data.basic_info.project}</td></tr>
                                <tr><td>记录数</td><td>${data.basic_info.count.toLocaleString()}</td></tr>
                                <tr><td>会话号</td><td>${data.basic_info.sesno}</td></tr>
                                <tr><td>最大REF1</td><td>${data.basic_info.max_ref1}</td></tr>
                            </table>
                        </div>
                        
                        <div class="detail-section">
                            <h3>版本对比</h3>
                            <div class="version-compare">
                                <div class="version-box">
                                    <h4>数据库版本</h4>
                                    <div class="version-number">${data.basic_info.sesno}</div>
                                </div>
                                <div class="version-box">
                                    <h4>文件版本</h4>
                                    <div class="version-number">${data.basic_info.file_version?.file_version || 'N/A'}</div>
                                </div>
                            </div>
                        </div>
                        
                        <div class="detail-section">
                            <h3>变更历史</h3>
                            <ul class="change-list">
                                ${data.change_log.map(log => `
                                    <li class="change-item">
                                        <strong>版本 ${log.version}</strong> (${log.date})<br>
                                        ${log.changes} - ${log.records_changed} 条记录变更
                                    </li>
                                `).join('')}
                            </ul>
                        </div>
                        
                        <div class="detail-section">
                            <h3>相关文件</h3>
                            <table class="detail-table">
                                ${data.related_files.map(file => `
                                    <tr>
                                        <td>${file.file_type}</td>
                                        <td>${file.exists ? '✓ 存在' : '✗ 不存在'} - ${(file.size / 1024 / 1024).toFixed(2)} MB</td>
                                    </tr>
                                `).join('')}
                            </table>
                        </div>
                    `;
                    
                    document.getElementById('detailModal').classList.add('show');
                }
            } catch (error) {
                alert('加载详情失败: ' + error.message);
            }
        }
        
        function closeModal() {
            document.getElementById('detailModal').classList.remove('show');
        }
        
        function switchTab(tab) {
            currentTab = tab;
            document.querySelectorAll('.nav-tab').forEach(t => t.classList.remove('active'));
            event.target.classList.add('active');
            
            let filteredData = dbStatusData;
            
            if (tab === 'needsUpdate') {
                filteredData = dbStatusData.filter(db => db.needs_update);
            } else if (tab === 'history') {
                // 可以添加历史记录的筛选逻辑
            }
            
            renderDbCards(filteredData);
        }
        
        function applyFilters() {
            const project = document.getElementById('projectFilter').value;
            const dbType = document.getElementById('dbTypeFilter').value;
            const status = document.getElementById('statusFilter').value;
            
            let filteredData = dbStatusData;
            
            if (project) {
                filteredData = filteredData.filter(db => db.project === project);
            }
            
            if (dbType) {
                filteredData = filteredData.filter(db => db.db_type === dbType);
            }
            
            if (status === 'needs_update') {
                filteredData = filteredData.filter(db => db.needs_update);
            } else if (status === 'parsed') {
                filteredData = filteredData.filter(db => db.parse_status === 'Parsed');
            } else if (status === 'not_parsed') {
                filteredData = filteredData.filter(db => db.parse_status === 'NotParsed');
            } else if (status === 'generated') {
                filteredData = filteredData.filter(db => db.model_status === 'Generated');
            }
            
            renderDbCards(filteredData);
        }
        
        // 页面加载时初始化
        document.addEventListener('DOMContentLoaded', () => {
            loadDbStatus();
            
            // 每30秒自动刷新
            setInterval(loadDbStatus, 30000);
        });
        
        // 点击模态框外部关闭
        document.getElementById('detailModal').addEventListener('click', (e) => {
            if (e.target.id === 'detailModal') {
                closeModal();
            }
        });
    </script>
</body>
</html>
    "#.to_string()
}
