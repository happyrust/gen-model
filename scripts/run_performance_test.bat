@echo off
setlocal enabledelayedexpansion

echo === AIOS 模型生成性能测试 ===
echo.

REM 检查是否在正确的目录
if not exist "Cargo.toml" (
    echo 错误: 请在项目根目录运行此脚本
    pause
    exit /b 1
)

REM 创建输出目录
if not exist "performance_results" mkdir performance_results

echo 1. 准备测试环境...

REM 设置环境变量
set RUST_LOG=info
set RUST_BACKTRACE=1

echo 2. 编译项目...
cargo build --release --bin performance_test
if errorlevel 1 (
    echo 编译失败
    pause
    exit /b 1
)

echo 3. 运行性能测试...

REM 选择测试模式
echo 请选择测试模式:
echo 1) 快速测试 (24383-24385, 3个数据库)
echo 2) 中等测试 (24383-24400, 18个数据库)
echo 3) 完整测试 (24383-66456, 所有数据库)
echo 4) 自定义范围
echo.
set /p choice="请输入选择 (1-4): "

if "%choice%"=="1" (
    set START_DB=24383
    set END_DB=24385
    set OUTPUT_FILE=quick_test_report.txt
    echo 执行快速测试...
) else if "%choice%"=="2" (
    set START_DB=24383
    set END_DB=24400
    set OUTPUT_FILE=medium_test_report.txt
    echo 执行中等测试...
) else if "%choice%"=="3" (
    set START_DB=24383
    set END_DB=66456
    set OUTPUT_FILE=full_test_report.txt
    echo 执行完整测试（这可能需要很长时间）...
) else if "%choice%"=="4" (
    set /p START_DB="请输入起始数据库号: "
    set /p END_DB="请输入结束数据库号: "
    set OUTPUT_FILE=custom_test_report.txt
    echo 执行自定义测试 (!START_DB! - !END_DB!)...
) else (
    echo 无效选择，使用快速测试
    set START_DB=24383
    set END_DB=24385
    set OUTPUT_FILE=quick_test_report.txt
)

echo.
echo 测试参数:
echo - 起始数据库: !START_DB!
echo - 结束数据库: !END_DB!
echo - 输出文件: performance_results\!OUTPUT_FILE!
echo.

REM 询问是否启用追踪
set /p enable_trace="是否启用性能追踪? (y/n): "

set TRACE_FLAG=
if /i "%enable_trace%"=="y" (
    set TRACE_FLAG=--trace
    echo 性能追踪已启用
)

echo.
echo 开始测试...
echo %date% %time%: 测试开始 > performance_results\test.log

REM 运行测试
cd performance_results
..\target\release\performance_test.exe --start !START_DB! --end !END_DB! --output !OUTPUT_FILE! !TRACE_FLAG! 2>&1 | tee -a test.log

echo.
echo %date% %time%: 测试完成 >> test.log

echo 4. 生成测试总结...

REM 创建测试总结
(
echo AIOS 模型生成性能测试总结
echo ============================
echo.
echo 测试时间: %date% %time%
echo 测试范围: !START_DB! - !END_DB!
echo 输出文件: !OUTPUT_FILE!
if defined TRACE_FLAG (
    echo 追踪启用: 是
) else (
    echo 追踪启用: 否
)
echo.
echo 文件说明:
echo - !OUTPUT_FILE!: 详细的性能测试报告
echo - test.log: 测试过程日志
if defined TRACE_FLAG (
    echo - performance_trace.json: 性能追踪文件
)
echo.
echo 查看建议:
echo 1. 查看 !OUTPUT_FILE! 了解详细统计信息
if defined TRACE_FLAG (
    echo 2. 如果启用了追踪，在Chrome浏览器中打开 chrome://tracing/ 加载 performance_trace.json
)
echo 3. 查看 test.log 了解测试过程中的详细信息
) > test_summary.txt

cd ..

echo 5. 测试完成!
echo.
echo 结果文件位置: performance_results\
echo - 测试报告: !OUTPUT_FILE!
echo - 测试日志: test.log
echo - 测试总结: test_summary.txt

if defined TRACE_FLAG (
    echo - 性能追踪: performance_trace.json
    echo.
    echo 要查看性能追踪:
    echo 1. 打开Chrome浏览器
    echo 2. 访问 chrome://tracing/
    echo 3. 点击 'Load' 按钮
    echo 4. 选择 performance_results\performance_trace.json 文件
)

echo.
echo 要查看测试报告，运行:
echo type performance_results\!OUTPUT_FILE!

pause
