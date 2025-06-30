#!/bin/bash

# AIOS 模型生成性能测试脚本
# 用于快速运行性能测试并生成报告

set -e

echo "=== AIOS 模型生成性能测试 ==="
echo ""

# 检查是否在正确的目录
if [ ! -f "Cargo.toml" ]; then
    echo "错误: 请在项目根目录运行此脚本"
    exit 1
fi

# 创建输出目录
mkdir -p performance_results
cd performance_results

echo "1. 准备测试环境..."

# 设置环境变量
export RUST_LOG=info
export RUST_BACKTRACE=1

echo "2. 编译项目..."
cd ..
cargo build --release --bin performance_test

echo "3. 运行性能测试..."

# 选择测试模式
echo "请选择测试模式:"
echo "1) 快速测试 (24383-24385, 3个数据库)"
echo "2) 中等测试 (24383-24400, 18个数据库)"
echo "3) 完整测试 (24383-66456, 所有数据库)"
echo "4) 自定义范围"
echo ""
read -p "请输入选择 (1-4): " choice

case $choice in
    1)
        START_DB=24383
        END_DB=24385
        OUTPUT_FILE="quick_test_report.txt"
        echo "执行快速测试..."
        ;;
    2)
        START_DB=24383
        END_DB=24400
        OUTPUT_FILE="medium_test_report.txt"
        echo "执行中等测试..."
        ;;
    3)
        START_DB=24383
        END_DB=66456
        OUTPUT_FILE="full_test_report.txt"
        echo "执行完整测试（这可能需要很长时间）..."
        ;;
    4)
        read -p "请输入起始数据库号: " START_DB
        read -p "请输入结束数据库号: " END_DB
        OUTPUT_FILE="custom_test_report.txt"
        echo "执行自定义测试 ($START_DB - $END_DB)..."
        ;;
    *)
        echo "无效选择，使用快速测试"
        START_DB=24383
        END_DB=24385
        OUTPUT_FILE="quick_test_report.txt"
        ;;
esac

echo ""
echo "测试参数:"
echo "- 起始数据库: $START_DB"
echo "- 结束数据库: $END_DB"
echo "- 输出文件: performance_results/$OUTPUT_FILE"
echo ""

# 询问是否启用追踪
read -p "是否启用性能追踪? (y/n): " enable_trace

TRACE_FLAG=""
if [ "$enable_trace" = "y" ] || [ "$enable_trace" = "Y" ]; then
    TRACE_FLAG="--trace"
    echo "性能追踪已启用"
fi

echo ""
echo "开始测试..."
echo "$(date): 测试开始" > performance_results/test.log

# 运行测试
cd performance_results
../target/release/performance_test \
    --start $START_DB \
    --end $END_DB \
    --output $OUTPUT_FILE \
    $TRACE_FLAG \
    2>&1 | tee -a test.log

echo ""
echo "$(date): 测试完成" >> test.log

echo "4. 生成测试总结..."

# 创建测试总结
cat > test_summary.txt << EOF
AIOS 模型生成性能测试总结
============================

测试时间: $(date)
测试范围: $START_DB - $END_DB
输出文件: $OUTPUT_FILE
追踪启用: $([ -n "$TRACE_FLAG" ] && echo "是" || echo "否")

文件说明:
- $OUTPUT_FILE: 详细的性能测试报告
- test.log: 测试过程日志
- performance_trace.json: 性能追踪文件 (如果启用)

查看建议:
1. 查看 $OUTPUT_FILE 了解详细统计信息
2. 如果启用了追踪，在Chrome浏览器中打开 chrome://tracing/ 加载 performance_trace.json
3. 查看 test.log 了解测试过程中的详细信息

EOF

echo "5. 测试完成!"
echo ""
echo "结果文件位置: performance_results/"
echo "- 测试报告: $OUTPUT_FILE"
echo "- 测试日志: test.log"
echo "- 测试总结: test_summary.txt"

if [ -n "$TRACE_FLAG" ]; then
    echo "- 性能追踪: performance_trace.json"
    echo ""
    echo "要查看性能追踪:"
    echo "1. 打开Chrome浏览器"
    echo "2. 访问 chrome://tracing/"
    echo "3. 点击 'Load' 按钮"
    echo "4. 选择 performance_results/performance_trace.json 文件"
fi

echo ""
echo "要查看测试报告，运行:"
echo "cat performance_results/$OUTPUT_FILE"
