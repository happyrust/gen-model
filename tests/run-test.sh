#!/bin/bash

echo "🚀 启动XKT测试环境..."

# 进入tests目录
cd "$(dirname "$0")"

# 检查是否安装了依赖
if [ ! -d "node_modules" ]; then
    echo "📦 安装测试依赖..."
    npm install
fi

# 检查Chrome是否已安装
if [ ! -d "node_modules/puppeteer/.local-chromium" ] && [ ! -d "$HOME/.cache/puppeteer" ]; then
    echo "🌐 安装Chrome浏览器..."
    npx puppeteer browsers install chrome
fi

# 启动HTTP服务器
echo "🌐 启动HTTP服务器 (端口8082)..."
python3 -m http.server 8082 > /dev/null 2>&1 &
HTTP_PID=$!

# 等待服务器启动
sleep 2

# 运行测试
echo "🧪 运行XKT自动化测试..."
npm test

# 保存测试结果
TEST_EXIT_CODE=$?

# 清理：停止HTTP服务器
echo "🧹 清理环境..."
kill $HTTP_PID 2>/dev/null

# 退出
if [ $TEST_EXIT_CODE -eq 0 ]; then
    echo "✅ 测试完成！"
else
    echo "❌ 测试失败！"
fi

exit $TEST_EXIT_CODE