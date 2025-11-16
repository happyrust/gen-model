#!/bin/bash

# 启动房间计算API服务器脚本
# 使用真实的aios-core查询方法

echo "🏠 启动房间计算API服务器..."
echo "📍 项目路径: $(pwd)"
echo "🔧 特性: web_server,sqlite-index"

# 检查配置文件
if [ ! -f "DbOption.toml" ]; then
    echo "❌ 错误: 找不到 DbOption.toml 配置文件"
    echo "请确保配置文件存在并包含正确的数据库连接信息"
    exit 1
fi

echo "✅ 找到配置文件: DbOption.toml"

# 检查几何文件目录
if [ ! -d "assets/meshes" ]; then
    echo "⚠️  警告: assets/meshes 目录不存在"
    echo "创建目录..."
    mkdir -p assets/meshes
fi

echo "✅ 几何文件目录: assets/meshes"

# 启动服务器
echo ""
echo "🚀 启动Web服务器..."
echo "📡 监听地址: http://localhost:8080"
echo "🔗 房间API基础路径: /api/room"
echo ""
echo "可用的API端点:"
echo "  GET  /api/room/status          - 系统状态"
echo "  GET  /api/room/query           - 单点房间查询"
echo "  POST /api/room/batch-query     - 批量房间查询"
echo "  POST /api/room/process-codes   - 房间代码处理"
echo "  POST /api/room/tasks           - 创建房间任务"
echo "  GET  /api/room/tasks/{id}      - 查询任务状态"
echo "  POST /api/room/snapshot        - 创建数据快照"
echo ""
echo "💡 测试命令:"
echo "  npm run test-real-room-api     - 运行真实API测试"
echo ""
echo "按 Ctrl+C 停止服务器"
echo "================================"

# 运行服务器
cargo run --bin web_server --features "web_server,sqlite-index"
