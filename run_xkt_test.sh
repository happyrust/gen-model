#!/bin/bash

# XKT 1112 自动化测试脚本

echo "🚀 开始 XKT 1112 自动化测试"
echo "=================================="

# 检查服务器是否运行
if ! curl -s http://localhost:8080/api/status > /dev/null; then
    echo "❌ 服务器未运行，请先启动服务器:"
    echo "   cargo run --features=\"web_ui\" --bin web_ui"
    exit 1
fi

echo "✅ 服务器正在运行"

# 运行基础测试
echo ""
echo "📊 运行基础格式测试..."
node test_xkt_simple.js

# 检查测试是否成功
if [ $? -eq 0 ]; then
    echo ""
    echo "✅ 基础测试通过!"

    # 获取最新生成的文件名
    LATEST_FILE=$(ls -t output/web_ui/db1112_compressed_*.xkt | head -1 | xargs basename)

    echo ""
    echo "🌐 浏览器测试信息:"
    echo "   访问: http://localhost:8080/xeokit-viewer"
    echo "   文件: $LATEST_FILE"
    echo ""
    echo "🤖 自动化测试页面:"
    echo "   访问: http://localhost:8080/xkt-auto-test"
    echo ""
    echo "📁 测试报告:"
    ls -la xkt_test_report_1112_*.json | tail -1

    echo ""
    echo "✅ 所有测试完成!"

else
    echo "❌ 测试失败"
    exit 1
fi