#!/bin/bash

# 简单的 XTK 生成测试脚本

echo "🚀 简单 XTK 生成测试"
echo "===================="

# 创建测试输出目录
mkdir -p test_output

# 设置环境变量
export RUST_LOG=info
export RUST_BACKTRACE=1

echo "📋 测试参考号: 24383/92720"
echo "📁 输出目录: ./test_output"
echo ""

# 运行基本编译检查
echo "🔧 检查编译..."
cargo check
if [ $? -ne 0 ]; then
    echo "❌ 编译检查失败"
    exit 1
fi
echo "✅ 编译检查通过"
echo ""

# 运行简化的 XTK 生成演示
echo "🎯 运行 XTK 生成演示..."
cargo run --example xtk_test_demo
if [ $? -ne 0 ]; then
    echo "❌ XTK 生成演示失败"
    exit 1
fi
echo ""

# 检查生成的文件
echo "📁 检查生成的文件..."
if [ -d "test_output" ]; then
    echo "生成的 XTK 文件:"
    ls -la test_output/*.xkt 2>/dev/null || echo "没有找到 XTK 文件"
    echo ""
    
    # 显示文件大小统计
    if ls test_output/*.xkt 1> /dev/null 2>&1; then
        echo "📊 文件大小统计:"
        for file in test_output/*.xkt; do
            size=$(stat -f%z "$file" 2>/dev/null || stat -c%s "$file" 2>/dev/null)
            size_kb=$((size / 1024))
            echo "  $(basename "$file"): ${size_kb} KB"
        done
        echo ""
    fi
else
    echo "⚠️  测试输出目录不存在"
fi

echo "🎉 测试完成!"
echo "============"

# 显示测试总结
echo "📋 测试总结:"
echo "  - 编译检查: ✅"
echo "  - XTK 生成演示: 运行完成"
echo ""

echo "📁 生成的文件保存在 ./test_output/ 目录中"
echo "💡 您可以使用 xeokit 查看器打开生成的 .xkt 文件"
echo "🌐 或者打开 test_output/xtk_viewer.html 来查看文件信息"
echo ""

# 提供下一步建议
echo "🔗 下一步建议:"
echo "  1. 检查 test_output 目录中的 XTK 文件"
echo "  2. 打开 test_output/xtk_viewer.html 验证文件"
echo "  3. 根据需要调整生成器配置"
echo "  4. 测试更多参考号"
