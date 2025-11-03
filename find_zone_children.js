#!/usr/bin/env node

import fetch from 'node-fetch';
import fs from 'fs';
import path from 'path';

/**
 * 查找已知区域的子节点并生成XKT
 */
class ZoneChildrenFinder {
    constructor(baseUrl = 'http://localhost:8080') {
        this.baseUrl = baseUrl;
        this.dbno = 1112;
        this.outputDir = path.join('output', 'zones', `db${this.dbno}`);
        this.zones = [];
    }

    /**
     * 查找refno的直接子节点
     */
    async findChildren(parentRefno) {
        console.log(`🔍 查找 ${parentRefno} 的子节点...`);

        // 基于已知的refno格式，尝试查找子节点
        const [mainNo, subNo] = parentRefno.split('/').map(Number);
        const children = [];

        // 尝试相邻的子号
        for (let offset = 1; offset <= 20; offset++) {
            const childRefno = `${mainNo}/${subNo + offset}`;

            try {
                const response = await fetch(`${this.baseUrl}/api/xkt/generate`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        dbno: this.dbno,
                        refno: childRefno,
                        compress: true
                    }),
                    timeout: 3000
                });

                if (response.ok) {
                    const result = await response.json();
                    if (result.success && result.filename) {
                        // 下载并检查文件大小
                        const xktResponse = await fetch(`${this.baseUrl}${result.url}`);
                        const data = await xktResponse.arrayBuffer();

                        if (data.byteLength > 500) {
                            console.log(`  ✅ 找到子节点: ${childRefno} (${data.byteLength} 字节)`);
                            children.push({
                                refno: childRefno,
                                size: data.byteLength,
                                filename: result.filename
                            });
                        }
                    }
                }
            } catch (error) {
                // 忽略错误，继续下一个
            }
        }

        return children;
    }

    /**
     * 生成模拟的多区域数据
     */
    async generateMultipleZones() {
        console.log('🏗️ 生成多区域演示数据\n');

        // 使用已知的有效refno作为基础
        const baseRefno = '17496/266203';
        const [mainNo, subNo] = baseRefno.split('/').map(Number);

        // 创建多个虚拟区域（使用相同的数据但不同的ID）
        const zones = [
            {
                id: 'zone_001',
                name: '工艺区 A',
                refno: baseRefno,
                description: '主要工艺设备区'
            },
            {
                id: 'zone_002',
                name: '工艺区 B',
                refno: baseRefno,  // 使用相同数据模拟
                description: '次要工艺设备区'
            },
            {
                id: 'zone_003',
                name: '储罐区',
                refno: baseRefno,  // 使用相同数据模拟
                description: '储罐和容器区'
            },
            {
                id: 'zone_004',
                name: '管廊区',
                refno: baseRefno,  // 使用相同数据模拟
                description: '主管廊结构'
            },
            {
                id: 'zone_005',
                name: '公用工程区',
                refno: baseRefno,  // 使用相同数据模拟
                description: '公用工程系统'
            }
        ];

        const generatedFiles = [];

        for (const zone of zones) {
            console.log(`生成 ${zone.name}...`);

            try {
                // 生成XKT
                const response = await fetch(`${this.baseUrl}/api/xkt/generate`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        dbno: this.dbno,
                        refno: zone.refno,
                        compress: true
                    })
                });

                if (response.ok) {
                    const result = await response.json();

                    // 下载文件
                    const xktResponse = await fetch(`${this.baseUrl}${result.url}`);
                    const xktData = await xktResponse.arrayBuffer();
                    const buffer = Buffer.from(xktData);

                    // 保存为不同的文件名
                    const zonePath = path.join(this.outputDir, `${zone.id}.xkt`);
                    fs.writeFileSync(zonePath, buffer);

                    console.log(`  ✅ 已保存: ${zone.id}.xkt (${buffer.length} 字节)`);

                    generatedFiles.push({
                        zone: zone,
                        filename: `${zone.id}.xkt`,
                        size: buffer.length,
                        path: zonePath
                    });
                }
            } catch (error) {
                console.error(`  ❌ 生成失败: ${error.message}`);
            }

            await new Promise(resolve => setTimeout(resolve, 500));
        }

        return generatedFiles;
    }

    /**
     * 生成带空间分布的清单
     */
    generateSpatialManifest(generatedFiles) {
        const manifest = {
            database: this.dbno,
            totalZones: generatedFiles.length,
            generatedAt: new Date().toISOString(),
            globalBoundingBox: {
                min: [-500, -500, 0],
                max: [500, 500, 300]
            },
            zones: []
        };

        // 为每个区域分配不同的空间位置
        const gridSize = Math.ceil(Math.sqrt(generatedFiles.length));
        let index = 0;

        for (const file of generatedFiles) {
            const row = Math.floor(index / gridSize);
            const col = index % gridSize;

            // 计算区域位置（网格布局）
            const centerX = (col - gridSize / 2) * 200;
            const centerY = (row - gridSize / 2) * 200;
            const centerZ = 50;

            manifest.zones.push({
                id: file.zone.id,
                name: file.zone.name,
                refno: file.zone.refno,
                description: file.zone.description,
                xktFile: file.filename,
                fileSize: file.size,
                compressed: true,
                hasGeometry: true,
                // 为每个区域分配不同的包围盒
                boundingBox: {
                    min: [centerX - 50, centerY - 50, 0],
                    max: [centerX + 50, centerY + 50, 100]
                },
                center: [centerX, centerY, centerZ],
                radius: 86.6,
                // 添加邻接信息用于预加载
                adjacentZones: this.getAdjacentZones(index, gridSize, generatedFiles.length)
            });

            index++;
        }

        const manifestPath = path.join(this.outputDir, 'zone_manifest.json');
        fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));

        console.log(`\n📝 空间清单已保存到: ${manifestPath}`);
        return manifest;
    }

    /**
     * 获取相邻区域
     */
    getAdjacentZones(index, gridSize, total) {
        const adjacent = [];
        const row = Math.floor(index / gridSize);
        const col = index % gridSize;

        // 上
        if (row > 0) {
            const adjIndex = (row - 1) * gridSize + col;
            if (adjIndex < total) {
                adjacent.push(`zone_${String(adjIndex + 1).padStart(3, '0')}`);
            }
        }

        // 下
        if (row < gridSize - 1) {
            const adjIndex = (row + 1) * gridSize + col;
            if (adjIndex < total) {
                adjacent.push(`zone_${String(adjIndex + 1).padStart(3, '0')}`);
            }
        }

        // 左
        if (col > 0) {
            const adjIndex = row * gridSize + (col - 1);
            if (adjIndex < total) {
                adjacent.push(`zone_${String(adjIndex + 1).padStart(3, '0')}`);
            }
        }

        // 右
        if (col < gridSize - 1) {
            const adjIndex = row * gridSize + (col + 1);
            if (adjIndex < total) {
                adjacent.push(`zone_${String(adjIndex + 1).padStart(3, '0')}`);
            }
        }

        return adjacent;
    }

    /**
     * 生成增强的查看器
     */
    generateEnhancedViewer() {
        const viewerHtml = `<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>多区域XKT加载演示</title>
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }
        body { font-family: 'Microsoft YaHei', Arial; background: #1a1a1a; color: #fff; }

        #container {
            display: flex;
            height: 100vh;
        }

        #sidebar {
            width: 300px;
            background: #2a2a2a;
            padding: 20px;
            overflow-y: auto;
        }

        #viewer {
            flex: 1;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            position: relative;
        }

        .zone-grid {
            display: grid;
            grid-template-columns: repeat(3, 1fr);
            gap: 10px;
            margin-top: 20px;
        }

        .zone-card {
            background: #3a3a3a;
            padding: 10px;
            border-radius: 8px;
            cursor: pointer;
            transition: all 0.3s;
            text-align: center;
        }

        .zone-card:hover {
            background: #4a4a4a;
            transform: scale(1.05);
        }

        .zone-card.loaded {
            background: #2d5a2d;
            border: 2px solid #4caf50;
        }

        .zone-card.loading {
            background: #5a5a2d;
            animation: pulse 1s infinite;
        }

        @keyframes pulse {
            0%, 100% { opacity: 1; }
            50% { opacity: 0.5; }
        }

        .stats {
            position: absolute;
            top: 20px;
            right: 20px;
            background: rgba(0,0,0,0.8);
            padding: 15px;
            border-radius: 10px;
            min-width: 200px;
        }

        .stat-item {
            display: flex;
            justify-content: space-between;
            margin: 10px 0;
        }

        .controls {
            margin: 20px 0;
        }

        button {
            background: #667eea;
            color: white;
            border: none;
            padding: 10px 20px;
            border-radius: 5px;
            cursor: pointer;
            margin: 5px;
            transition: background 0.3s;
        }

        button:hover {
            background: #5a67d8;
        }

        input[type="range"] {
            width: 100%;
            margin: 10px 0;
        }

        h2 {
            color: #667eea;
            margin-bottom: 15px;
        }

        .zone-map {
            width: 100%;
            height: 200px;
            background: #1a1a1a;
            border-radius: 8px;
            margin: 20px 0;
            position: relative;
            overflow: hidden;
        }

        .zone-dot {
            position: absolute;
            width: 20px;
            height: 20px;
            background: #667eea;
            border-radius: 50%;
            transform: translate(-50%, -50%);
            cursor: pointer;
        }

        .zone-dot.loaded {
            background: #4caf50;
        }
    </style>
</head>
<body>
    <div id="container">
        <div id="sidebar">
            <h2>🏗️ 区域控制</h2>

            <div class="controls">
                <button onclick="loadAllZones()">加载全部</button>
                <button onclick="unloadAllZones()">卸载全部</button>
                <button onclick="testSequentialLoad()">顺序加载测试</button>
            </div>

            <div>
                <label>视距: <span id="viewDistanceValue">500</span>m</label>
                <input type="range" id="viewDistance" min="100" max="1000" value="500">
            </div>

            <div>
                <label>
                    <input type="checkbox" id="autoLoad" checked> 自动加载
                </label>
            </div>

            <h3 style="margin-top: 20px;">📍 区域地图</h3>
            <div class="zone-map" id="zoneMap"></div>

            <h3>📦 区域列表</h3>
            <div class="zone-grid" id="zoneGrid"></div>
        </div>

        <div id="viewer">
            <div class="stats">
                <h3>📊 性能统计</h3>
                <div class="stat-item">
                    <span>已加载区域:</span>
                    <span id="loadedCount">0</span>
                </div>
                <div class="stat-item">
                    <span>总内存:</span>
                    <span id="memoryUsage">0 MB</span>
                </div>
                <div class="stat-item">
                    <span>加载时间:</span>
                    <span id="loadTime">0 ms</span>
                </div>
                <div class="stat-item">
                    <span>FPS:</span>
                    <span id="fps">60</span>
                </div>
            </div>
        </div>
    </div>

    <script type="module">
        class MultiZoneManager {
            constructor() {
                this.manifest = null;
                this.loadedZones = new Map();
                this.loadingQueue = [];
                this.stats = {
                    loadTime: 0,
                    totalMemory: 0
                };
            }

            async init() {
                const response = await fetch('/zones/db1112/zone_manifest.json');
                this.manifest = await response.json();

                this.renderZoneGrid();
                this.renderZoneMap();

                console.log('多区域管理器已初始化', this.manifest);
            }

            renderZoneGrid() {
                const grid = document.getElementById('zoneGrid');
                grid.innerHTML = '';

                this.manifest.zones.forEach(zone => {
                    const card = document.createElement('div');
                    card.className = 'zone-card';
                    card.id = 'card-' + zone.id;
                    card.innerHTML = \`
                        <div style="font-size: 24px;">📦</div>
                        <div style="font-weight: bold;">\${zone.name}</div>
                        <div style="font-size: 12px; color: #888;">
                            \${(zone.fileSize / 1024).toFixed(1)} KB
                        </div>
                    \`;
                    card.onclick = () => this.toggleZone(zone.id);
                    grid.appendChild(card);
                });
            }

            renderZoneMap() {
                const map = document.getElementById('zoneMap');
                map.innerHTML = '';

                this.manifest.zones.forEach(zone => {
                    const dot = document.createElement('div');
                    dot.className = 'zone-dot';
                    dot.id = 'dot-' + zone.id;

                    // 将世界坐标映射到地图坐标
                    const mapX = ((zone.center[0] + 500) / 1000) * 100;
                    const mapY = ((zone.center[1] + 500) / 1000) * 100;

                    dot.style.left = mapX + '%';
                    dot.style.top = mapY + '%';
                    dot.title = zone.name;
                    dot.onclick = () => this.toggleZone(zone.id);

                    map.appendChild(dot);
                });
            }

            async loadZone(zoneId) {
                if (this.loadedZones.has(zoneId)) return;

                const zone = this.manifest.zones.find(z => z.id === zoneId);
                if (!zone) return;

                const startTime = performance.now();

                // 更新UI状态
                document.getElementById('card-' + zoneId)?.classList.add('loading');
                document.getElementById('dot-' + zoneId)?.classList.add('loading');

                try {
                    const response = await fetch(\`/zones/db1112/\${zone.xktFile}\`);
                    const data = await response.arrayBuffer();

                    this.loadedZones.set(zoneId, {
                        zone: zone,
                        data: data,
                        size: data.byteLength
                    });

                    // 更新UI
                    document.getElementById('card-' + zoneId)?.classList.remove('loading');
                    document.getElementById('card-' + zoneId)?.classList.add('loaded');
                    document.getElementById('dot-' + zoneId)?.classList.add('loaded');

                    // 更新统计
                    this.stats.loadTime = performance.now() - startTime;
                    this.updateStats();

                    console.log(\`✅ 已加载区域: \${zone.name}\`);

                    // 预加载相邻区域
                    if (zone.adjacentZones) {
                        zone.adjacentZones.forEach(adjId => {
                            setTimeout(() => this.preloadZone(adjId), 100);
                        });
                    }
                } catch (error) {
                    console.error(\`❌ 加载失败: \${zone.name}\`, error);
                    document.getElementById('card-' + zoneId)?.classList.remove('loading');
                }
            }

            async preloadZone(zoneId) {
                if (this.loadedZones.has(zoneId)) return;

                const zone = this.manifest.zones.find(z => z.id === zoneId);
                if (!zone) return;

                // 低优先级预加载
                console.log(\`📥 预加载区域: \${zone.name}\`);
            }

            unloadZone(zoneId) {
                if (!this.loadedZones.has(zoneId)) return;

                this.loadedZones.delete(zoneId);

                // 更新UI
                document.getElementById('card-' + zoneId)?.classList.remove('loaded');
                document.getElementById('dot-' + zoneId)?.classList.remove('loaded');

                this.updateStats();
                console.log(\`📤 已卸载区域: \${zoneId}\`);
            }

            toggleZone(zoneId) {
                if (this.loadedZones.has(zoneId)) {
                    this.unloadZone(zoneId);
                } else {
                    this.loadZone(zoneId);
                }
            }

            updateStats() {
                document.getElementById('loadedCount').textContent = this.loadedZones.size;

                let totalMemory = 0;
                this.loadedZones.forEach(zone => {
                    totalMemory += zone.size;
                });

                document.getElementById('memoryUsage').textContent =
                    (totalMemory / (1024 * 1024)).toFixed(2) + ' MB';

                document.getElementById('loadTime').textContent =
                    this.stats.loadTime.toFixed(0) + ' ms';
            }
        }

        // 全局函数
        window.manager = new MultiZoneManager();

        window.loadAllZones = async () => {
            for (const zone of manager.manifest.zones) {
                await manager.loadZone(zone.id);
                await new Promise(r => setTimeout(r, 100));
            }
        };

        window.unloadAllZones = () => {
            manager.manifest.zones.forEach(zone => {
                manager.unloadZone(zone.id);
            });
        };

        window.testSequentialLoad = async () => {
            await unloadAllZones();
            for (const zone of manager.manifest.zones) {
                await manager.loadZone(zone.id);
                await new Promise(r => setTimeout(r, 500));
            }
        };

        // 初始化
        manager.init().then(() => {
            console.log('系统就绪');
            // 默认加载第一个区域
            if (manager.manifest.zones.length > 0) {
                manager.loadZone(manager.manifest.zones[0].id);
            }
        });

        // FPS计数器
        let fps = 60;
        let lastTime = performance.now();
        function updateFPS() {
            const now = performance.now();
            fps = Math.round(1000 / (now - lastTime));
            lastTime = now;
            document.getElementById('fps').textContent = fps;
            requestAnimationFrame(updateFPS);
        }
        updateFPS();

        // 视距控制
        document.getElementById('viewDistance').addEventListener('input', (e) => {
            document.getElementById('viewDistanceValue').textContent = e.target.value;
        });
    </script>
</body>
</html>`;

        const viewerPath = path.join(this.outputDir, 'multi_zone_viewer.html');
        fs.writeFileSync(viewerPath, viewerHtml);

        console.log(`🌐 增强查看器已保存到: ${viewerPath}`);
    }

    /**
     * 主执行函数
     */
    async run() {
        console.log('='.repeat(60));
        console.log('🏗️ 多区域XKT生成和空间布局');
        console.log('='.repeat(60));

        // 确保输出目录存在
        if (!fs.existsSync(this.outputDir)) {
            fs.mkdirSync(this.outputDir, { recursive: true });
        }

        // 生成多个区域文件
        const generatedFiles = await this.generateMultipleZones();

        if (generatedFiles.length > 0) {
            // 生成空间清单
            const manifest = this.generateSpatialManifest(generatedFiles);

            // 生成增强查看器
            this.generateEnhancedViewer();

            console.log('\n' + '='.repeat(60));
            console.log('✅ 多区域系统生成完成！');
            console.log('='.repeat(60));
            console.log(`📊 生成统计:`);
            console.log(`  - 区域数量: ${generatedFiles.length}`);
            console.log(`  - 总文件大小: ${generatedFiles.reduce((sum, f) => sum + f.size, 0)} 字节`);
            console.log(`  - 空间范围: 1000x1000x300 米`);
            console.log(`\n📁 文件位置: ${this.outputDir}`);
            console.log(`🌐 打开 multi_zone_viewer.html 查看演示`);
        }
    }
}

// 执行
if (import.meta.url === `file://${process.argv[1]}`) {
    const finder = new ZoneChildrenFinder();
    finder.run().catch(console.error);
}

export { ZoneChildrenFinder };