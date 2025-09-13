#!/bin/bash

# XTK 生成测试运行脚本

echo "🚀 开始 XTK 生成测试"
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

# 运行单元测试
echo "🧪 运行单元测试..."
cargo test xeokit_xtk_generator --lib
if [ $? -ne 0 ]; then
    echo "⚠️  单元测试有问题，但继续执行集成测试"
fi
echo ""

# 运行集成测试演示
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

# 运行验证测试
echo "🔍 运行验证测试..."
cargo test test_validate_generated_xtk --test validation_tests
echo ""

# 性能测试
echo "⚡ 运行性能测试..."
cargo test test_performance_benchmark --release
echo ""

echo "🎉 测试完成!"
echo "============"

# 显示测试总结
echo "📋 测试总结:"
echo "  - 编译检查: ✅"
echo "  - 单元测试: 运行完成"
echo "  - 集成测试: 运行完成"
echo "  - 验证测试: 运行完成"
echo "  - 性能测试: 运行完成"
echo ""

echo "📁 生成的文件保存在 ./test_output/ 目录中"
echo "💡 您可以使用 xeokit 查看器打开生成的 .xkt 文件"
echo ""

# 提供下一步建议
echo "🔗 下一步建议:"
echo "  1. 检查 test_output 目录中的 XTK 文件"
echo "  2. 使用 xeokit 查看器验证文件内容"
echo "  3. 根据需要调整生成器配置"
echo "  4. 测试更多参考号"
