#!/bin/bash

echo "========================================"
echo "    gen_geos_data 函数性能测试工具"
echo "========================================"
echo

echo "请选择测试模式:"
echo "1. 快速测试 (从数据库查询少量参考号)"
echo "2. 标准测试 (从数据库查询中等数量参考号)"
echo "3. 完整测试 (从数据库查询大量参考号)"
echo "4. 批量测试 (测试不同规模的参考号组)"
echo "5. 手动测试 (手动指定参考号)"
echo "6. 运行示例程序"
echo "0. 退出"
echo

read -p "请输入选择 (0-6): " choice

case $choice in
    0)
        echo "退出程序"
        exit 0
        ;;
    1)
        echo
        echo "🚀 执行快速测试..."
        echo "配置: 数据库24383, PRIM类型, 最多10个参考号"
        echo
        cargo run --release --bin test_gen_geos_data -- --mode database --dbno 24383 --types PRIM --max-refnos 10 --trace --output quick_test_report.txt
        show_results "quick_test_report.txt"
        ;;
    2)
        echo
        echo "📊 执行标准测试..."
        echo "配置: 数据库24383, PRIM+LOOP类型, 最多50个参考号"
        echo
        cargo run --release --bin test_gen_geos_data -- --mode database --dbno 24383 --types PRIM LOOP --max-refnos 50 --trace --output standard_test_report.txt
        show_results "standard_test_report.txt"
        ;;
    3)
        echo
        echo "🔍 执行完整测试..."
        echo "配置: 数据库24383, PRIM+LOOP+CATA类型, 最多200个参考号"
        echo
        cargo run --release --bin test_gen_geos_data -- --mode database --dbno 24383 --types PRIM LOOP CATA --max-refnos 200 --trace --output full_test_report.txt
        show_results "full_test_report.txt"
        ;;
    4)
        echo
        echo "📦 执行批量测试..."
        echo "配置: 3组测试, 每组20个参考号"
        echo
        cargo run --release --bin test_gen_geos_data -- --mode batch --dbno 24383 --types PRIM --batch-count 3 --batch-size 20 --trace --output batch_test_report.txt
        show_results "batch_test_report.txt"
        ;;
    5)
        echo
        echo "🔧 手动测试模式"
        read -p "请输入要测试的参考号 (用逗号分隔，例如: 24383_123456,24383_123457): " manual_refnos
        
        if [ -z "$manual_refnos" ]; then
            echo "错误: 请输入至少一个参考号"
            exit 1
        fi
        
        echo
        echo "执行手动测试..."
        echo "参考号: $manual_refnos"
        echo
        cargo run --release --bin test_gen_geos_data -- --mode manual --refnos "$manual_refnos" --trace --output manual_test_report.txt
        show_results "manual_test_report.txt"
        ;;
    6)
        echo
        echo "📚 运行示例程序..."
        echo
        cargo run --release --example test_gen_geos_data_example
        show_example_results
        ;;
    *)
        echo "无效选择，请重新运行脚本"
        exit 1
        ;;
esac

function show_results() {
    local report_file=$1
    
    echo
    echo "========================================"
    echo "           测试完成"
    echo "========================================"
    echo
    echo "📄 生成的文件:"
    echo "  - 性能测试报告: $(pwd)/$report_file"
    echo "  - 性能追踪文件: $(pwd)/performance_trace.json"
    echo
    echo "💡 建议:"
    echo "  1. 查看测试报告了解详细的性能数据"
    echo "  2. 在 Chrome 浏览器中打开 chrome://tracing/"
    echo "  3. 加载 performance_trace.json 文件查看详细的函数调用分析"
    echo "  4. 根据报告中的优化建议改进代码性能"
    echo

    read -p "是否打开测试报告? (y/n): " open_report
    if [[ $open_report =~ ^[Yy]$ ]]; then
        if command -v code &> /dev/null; then
            echo "使用 VS Code 打开报告..."
            code "$report_file"
        elif command -v gedit &> /dev/null; then
            echo "使用 gedit 打开报告..."
            gedit "$report_file" &
        elif command -v nano &> /dev/null; then
            echo "使用 nano 打开报告..."
            nano "$report_file"
        else
            echo "使用 cat 显示报告内容:"
            cat "$report_file"
        fi
    fi

    read -p "是否在浏览器中打开性能追踪? (y/n): " open_trace
    if [[ $open_trace =~ ^[Yy]$ ]]; then
        echo "打开 Chrome 追踪工具..."
        if command -v google-chrome &> /dev/null; then
            google-chrome "chrome://tracing/" &
        elif command -v chromium-browser &> /dev/null; then
            chromium-browser "chrome://tracing/" &
        elif command -v firefox &> /dev/null; then
            echo "注意: Firefox 不支持 Chrome 追踪格式，建议使用 Chrome"
            firefox "chrome://tracing/" &
        else
            echo "请手动在 Chrome 浏览器中打开 chrome://tracing/"
        fi
        echo "请在打开的页面中加载 performance_trace.json 文件"
    fi
}

function show_example_results() {
    echo
    echo "========================================"
    echo "         示例程序完成"
    echo "========================================"
    echo
    echo "📄 生成的文件:"
    echo "  - 示例测试报告: $(pwd)/gen_geos_data_example_report.txt"
    echo "  - 性能追踪文件: $(pwd)/performance_trace.json"
    echo

    read -p "是否打开示例报告? (y/n): " open_example
    if [[ $open_example =~ ^[Yy]$ ]]; then
        if command -v code &> /dev/null; then
            code "gen_geos_data_example_report.txt"
        elif command -v gedit &> /dev/null; then
            gedit "gen_geos_data_example_report.txt" &
        else
            cat "gen_geos_data_example_report.txt"
        fi
    fi
}

echo
echo "感谢使用 gen_geos_data 性能测试工具！"
