#!/bin/bash

# 创建多区域演示数据
# 使用已有的zone_001.xkt文件创建多个区域副本

echo "🏗️ 创建多区域演示数据..."

cd output/zones/db1112

# 检查zone_001.xkt是否存在
if [ ! -f "zone_001.xkt" ]; then
    echo "❌ 错误：zone_001.xkt 不存在"
    exit 1
fi

# 复制zone_001.xkt为多个区域文件
echo "📦 复制区域文件..."
cp zone_001.xkt zone_002.xkt
cp zone_001.xkt zone_003.xkt
cp zone_001.xkt zone_004.xkt
cp zone_001.xkt zone_005.xkt

echo "✅ 创建了5个区域文件"

# 创建新的zone_manifest.json
cat > zone_manifest_multi.json << 'EOF'
{
  "database": 1112,
  "totalZones": 5,
  "generatedAt": "2025-09-29T13:30:00.000Z",
  "globalBoundingBox": {
    "min": [-500, -500, 0],
    "max": [500, 500, 300]
  },
  "zones": [
    {
      "id": "zone_001",
      "name": "工艺区 A",
      "refno": "17496/266203",
      "description": "主要工艺设备区",
      "xktFile": "zone_001.xkt",
      "fileSize": 2466,
      "compressed": true,
      "hasGeometry": true,
      "boundingBox": {
        "min": [-250, -250, 0],
        "max": [-50, -50, 100]
      },
      "center": [-150, -150, 50],
      "radius": 141.42,
      "adjacentZones": ["zone_002", "zone_003"]
    },
    {
      "id": "zone_002",
      "name": "工艺区 B",
      "refno": "17496/266203",
      "description": "次要工艺设备区",
      "xktFile": "zone_002.xkt",
      "fileSize": 2466,
      "compressed": true,
      "hasGeometry": true,
      "boundingBox": {
        "min": [50, -250, 0],
        "max": [250, -50, 100]
      },
      "center": [150, -150, 50],
      "radius": 141.42,
      "adjacentZones": ["zone_001", "zone_004"]
    },
    {
      "id": "zone_003",
      "name": "储罐区",
      "refno": "17496/266203",
      "description": "储罐和容器区",
      "xktFile": "zone_003.xkt",
      "fileSize": 2466,
      "compressed": true,
      "hasGeometry": true,
      "boundingBox": {
        "min": [-250, 50, 0],
        "max": [-50, 250, 100]
      },
      "center": [-150, 150, 50],
      "radius": 141.42,
      "adjacentZones": ["zone_001", "zone_005"]
    },
    {
      "id": "zone_004",
      "name": "管廊区",
      "refno": "17496/266203",
      "description": "主管廊结构",
      "xktFile": "zone_004.xkt",
      "fileSize": 2466,
      "compressed": true,
      "hasGeometry": true,
      "boundingBox": {
        "min": [50, 50, 0],
        "max": [250, 250, 100]
      },
      "center": [150, 150, 50],
      "radius": 141.42,
      "adjacentZones": ["zone_002", "zone_005"]
    },
    {
      "id": "zone_005",
      "name": "公用工程区",
      "refno": "17496/266203",
      "description": "公用工程系统",
      "xktFile": "zone_005.xkt",
      "fileSize": 2466,
      "compressed": true,
      "hasGeometry": true,
      "boundingBox": {
        "min": [-100, -100, 100],
        "max": [100, 100, 200]
      },
      "center": [0, 0, 150],
      "radius": 141.42,
      "adjacentZones": ["zone_003", "zone_004"]
    }
  ]
}
EOF

echo "📝 创建了多区域清单文件"

# 创建可视化HTML文件
cat > multi_zone_demo.html << 'EOF'
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>多区域XKT加载演示 - DB1112</title>
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', 'Microsoft YaHei', Arial;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            color: #fff;
            display: flex;
            justify-content: center;
            align-items: center;
            min-height: 100vh;
            padding: 20px;
        }
        .container {
            background: rgba(255, 255, 255, 0.1);
            backdrop-filter: blur(10px);
            border-radius: 20px;
            padding: 40px;
            max-width: 1200px;
            width: 100%;
            box-shadow: 0 20px 40px rgba(0,0,0,0.2);
        }
        h1 {
            text-align: center;
            margin-bottom: 30px;
            font-size: 2.5em;
            text-shadow: 2px 2px 4px rgba(0,0,0,0.2);
        }
        .zone-grid {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
            gap: 20px;
            margin-bottom: 30px;
        }
        .zone-card {
            background: rgba(255, 255, 255, 0.2);
            padding: 20px;
            border-radius: 15px;
            text-align: center;
            transition: all 0.3s;
            cursor: pointer;
            border: 2px solid transparent;
        }
        .zone-card:hover {
            transform: translateY(-5px);
            background: rgba(255, 255, 255, 0.3);
            box-shadow: 0 10px 20px rgba(0,0,0,0.2);
        }
        .zone-card.loaded {
            background: rgba(76, 175, 80, 0.3);
            border-color: #4caf50;
        }
        .zone-icon {
            font-size: 3em;
            margin-bottom: 10px;
        }
        .zone-name {
            font-weight: bold;
            margin-bottom: 5px;
        }
        .zone-info {
            font-size: 0.9em;
            opacity: 0.8;
        }
        .stats {
            background: rgba(0, 0, 0, 0.3);
            padding: 20px;
            border-radius: 15px;
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
            gap: 15px;
            text-align: center;
        }
        .stat-item {
            padding: 10px;
        }
        .stat-value {
            font-size: 2em;
            font-weight: bold;
            margin-bottom: 5px;
        }
        .stat-label {
            opacity: 0.8;
            font-size: 0.9em;
        }
        .controls {
            display: flex;
            justify-content: center;
            gap: 15px;
            margin-top: 30px;
        }
        button {
            background: rgba(255, 255, 255, 0.2);
            color: white;
            border: 2px solid rgba(255, 255, 255, 0.3);
            padding: 12px 30px;
            border-radius: 25px;
            cursor: pointer;
            font-size: 1em;
            transition: all 0.3s;
        }
        button:hover {
            background: rgba(255, 255, 255, 0.3);
            transform: scale(1.05);
        }
        .spatial-map {
            background: rgba(0, 0, 0, 0.3);
            border-radius: 15px;
            padding: 20px;
            margin: 20px 0;
            height: 300px;
            position: relative;
        }
        .map-zone {
            position: absolute;
            width: 60px;
            height: 60px;
            background: rgba(255, 255, 255, 0.2);
            border: 2px solid rgba(255, 255, 255, 0.5);
            border-radius: 10px;
            display: flex;
            align-items: center;
            justify-content: center;
            cursor: pointer;
            transition: all 0.3s;
        }
        .map-zone:hover {
            background: rgba(255, 255, 255, 0.3);
            transform: scale(1.1);
        }
        .map-zone.loaded {
            background: rgba(76, 175, 80, 0.3);
            border-color: #4caf50;
        }
    </style>
</head>
<body>
    <div class="container">
        <h1>🏗️ 多区域XKT加载演示</h1>

        <div class="spatial-map" id="spatialMap">
            <div class="map-zone" style="left: 15%; top: 15%;" data-zone="zone_001">Z1</div>
            <div class="map-zone" style="left: 55%; top: 15%;" data-zone="zone_002">Z2</div>
            <div class="map-zone" style="left: 15%; top: 55%;" data-zone="zone_003">Z3</div>
            <div class="map-zone" style="left: 55%; top: 55%;" data-zone="zone_004">Z4</div>
            <div class="map-zone" style="left: 35%; top: 35%;" data-zone="zone_005">Z5</div>
        </div>

        <div class="zone-grid" id="zoneGrid">
            <div class="zone-card" data-zone="zone_001">
                <div class="zone-icon">⚙️</div>
                <div class="zone-name">工艺区 A</div>
                <div class="zone-info">2.4 KB</div>
            </div>
            <div class="zone-card" data-zone="zone_002">
                <div class="zone-icon">🏭</div>
                <div class="zone-name">工艺区 B</div>
                <div class="zone-info">2.4 KB</div>
            </div>
            <div class="zone-card" data-zone="zone_003">
                <div class="zone-icon">🛢️</div>
                <div class="zone-name">储罐区</div>
                <div class="zone-info">2.4 KB</div>
            </div>
            <div class="zone-card" data-zone="zone_004">
                <div class="zone-icon">🔧</div>
                <div class="zone-name">管廊区</div>
                <div class="zone-info">2.4 KB</div>
            </div>
            <div class="zone-card" data-zone="zone_005">
                <div class="zone-icon">⚡</div>
                <div class="zone-name">公用工程区</div>
                <div class="zone-info">2.4 KB</div>
            </div>
        </div>

        <div class="stats">
            <div class="stat-item">
                <div class="stat-value" id="loadedCount">0</div>
                <div class="stat-label">已加载区域</div>
            </div>
            <div class="stat-item">
                <div class="stat-value" id="totalMemory">0</div>
                <div class="stat-label">内存使用 (KB)</div>
            </div>
            <div class="stat-item">
                <div class="stat-value" id="loadTime">0</div>
                <div class="stat-label">加载时间 (ms)</div>
            </div>
            <div class="stat-item">
                <div class="stat-value">60</div>
                <div class="stat-label">FPS</div>
            </div>
        </div>

        <div class="controls">
            <button onclick="loadAllZones()">加载全部区域</button>
            <button onclick="unloadAllZones()">卸载全部区域</button>
            <button onclick="testSequential()">顺序加载测试</button>
        </div>
    </div>

    <script>
        const loadedZones = new Set();
        let totalMemory = 0;

        function toggleZone(zoneId) {
            if (loadedZones.has(zoneId)) {
                unloadZone(zoneId);
            } else {
                loadZone(zoneId);
            }
        }

        function loadZone(zoneId) {
            if (loadedZones.has(zoneId)) return;

            const startTime = performance.now();

            // 模拟加载
            setTimeout(() => {
                loadedZones.add(zoneId);
                totalMemory += 2.4;

                // 更新UI
                document.querySelectorAll(`[data-zone="${zoneId}"]`).forEach(el => {
                    el.classList.add('loaded');
                });

                updateStats();
                document.getElementById('loadTime').textContent =
                    Math.round(performance.now() - startTime);

                console.log(`✅ 加载区域: ${zoneId}`);
            }, Math.random() * 500 + 200);
        }

        function unloadZone(zoneId) {
            if (!loadedZones.has(zoneId)) return;

            loadedZones.delete(zoneId);
            totalMemory -= 2.4;

            document.querySelectorAll(`[data-zone="${zoneId}"]`).forEach(el => {
                el.classList.remove('loaded');
            });

            updateStats();
            console.log(`📤 卸载区域: ${zoneId}`);
        }

        function updateStats() {
            document.getElementById('loadedCount').textContent = loadedZones.size;
            document.getElementById('totalMemory').textContent = totalMemory.toFixed(1);
        }

        async function loadAllZones() {
            const zones = ['zone_001', 'zone_002', 'zone_003', 'zone_004', 'zone_005'];
            for (const zone of zones) {
                loadZone(zone);
                await new Promise(r => setTimeout(r, 200));
            }
        }

        function unloadAllZones() {
            const zones = ['zone_001', 'zone_002', 'zone_003', 'zone_004', 'zone_005'];
            zones.forEach(zone => unloadZone(zone));
        }

        async function testSequential() {
            unloadAllZones();
            await new Promise(r => setTimeout(r, 500));

            const zones = ['zone_001', 'zone_002', 'zone_003', 'zone_004', 'zone_005'];
            for (const zone of zones) {
                loadZone(zone);
                await new Promise(r => setTimeout(r, 1000));
            }
        }

        // 点击事件
        document.querySelectorAll('.zone-card, .map-zone').forEach(el => {
            el.addEventListener('click', () => {
                const zoneId = el.getAttribute('data-zone');
                toggleZone(zoneId);
            });
        });

        // 初始加载第一个区域
        setTimeout(() => loadZone('zone_001'), 500);
    </script>
</body>
</html>
EOF

echo "🌐 创建了演示HTML文件"

# 输出结果
echo ""
echo "============================================"
echo "✅ 多区域演示环境创建完成！"
echo "============================================"
echo "📁 文件位置：output/zones/db1112/"
echo "📦 区域文件："
ls -lh zone_*.xkt
echo ""
echo "📋 清单文件：zone_manifest_multi.json"
echo "🌐 演示页面：multi_zone_demo.html"
echo ""
echo "使用方法："
echo "1. 在浏览器中打开 multi_zone_demo.html"
echo "2. 点击区域卡片或地图上的区域进行加载/卸载"
echo "3. 使用底部按钮测试批量加载功能"