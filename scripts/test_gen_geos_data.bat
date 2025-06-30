@echo off
chcp 65001 >nul
echo ========================================
echo    gen_geos_data 函数性能测试工具
echo ========================================
echo.

echo 请选择测试模式:
echo 1. 快速测试 (从数据库查询少量参考号)
echo 2. 标准测试 (从数据库查询中等数量参考号)
echo 3. 完整测试 (从数据库查询大量参考号)
echo 4. 批量测试 (测试不同规模的参考号组)
echo 5. 手动测试 (手动指定参考号)
echo 6. 运行示例程序
echo 0. 退出
echo.

set /p choice=请输入选择 (0-6): 

if "%choice%"=="0" goto :end
if "%choice%"=="1" goto :quick_test
if "%choice%"=="2" goto :standard_test
if "%choice%"=="3" goto :full_test
if "%choice%"=="4" goto :batch_test
if "%choice%"=="5" goto :manual_test
if "%choice%"=="6" goto :example_test

echo 无效选择，请重新运行脚本
pause
goto :end

:quick_test
echo.
echo 🚀 执行快速测试...
echo 配置: 数据库24383, PRIM类型, 最多10个参考号
echo.
cargo run --release --bin test_gen_geos_data -- --mode database --dbno 24383 --types PRIM --max-refnos 10 --trace --output quick_test_report.txt
goto :show_results

:standard_test
echo.
echo 📊 执行标准测试...
echo 配置: 数据库24383, PRIM+LOOP类型, 最多50个参考号
echo.
cargo run --release --bin test_gen_geos_data -- --mode database --dbno 24383 --types PRIM LOOP --max-refnos 50 --trace --output standard_test_report.txt
goto :show_results

:full_test
echo.
echo 🔍 执行完整测试...
echo 配置: 数据库24383, PRIM+LOOP+CATA类型, 最多200个参考号
echo.
cargo run --release --bin test_gen_geos_data -- --mode database --dbno 24383 --types PRIM LOOP CATA --max-refnos 200 --trace --output full_test_report.txt
goto :show_results

:batch_test
echo.
echo 📦 执行批量测试...
echo 配置: 3组测试, 每组20个参考号
echo.
cargo run --release --bin test_gen_geos_data -- --mode batch --dbno 24383 --types PRIM --batch-count 3 --batch-size 20 --trace --output batch_test_report.txt
goto :show_results

:manual_test
echo.
echo 🔧 手动测试模式
echo 请输入要测试的参考号 (用逗号分隔，例如: 24383_123456,24383_123457):
set /p manual_refnos=参考号列表: 

if "%manual_refnos%"=="" (
    echo 错误: 请输入至少一个参考号
    pause
    goto :end
)

echo.
echo 执行手动测试...
echo 参考号: %manual_refnos%
echo.
cargo run --release --bin test_gen_geos_data -- --mode manual --refnos "%manual_refnos%" --trace --output manual_test_report.txt
goto :show_results

:example_test
echo.
echo 📚 运行示例程序...
echo.
cargo run --release --example test_gen_geos_data_example
goto :show_example_results

:show_results
echo.
echo ========================================
echo           测试完成
echo ========================================
echo.
echo 📄 生成的文件:
echo   - 性能测试报告: %cd%\*_test_report.txt
echo   - 性能追踪文件: %cd%\performance_trace.json
echo.
echo 💡 建议:
echo   1. 查看测试报告了解详细的性能数据
echo   2. 在 Chrome 浏览器中打开 chrome://tracing/ 
echo   3. 加载 performance_trace.json 文件查看详细的函数调用分析
echo   4. 根据报告中的优化建议改进代码性能
echo.

set /p open_report=是否打开测试报告? (y/n): 
if /i "%open_report%"=="y" (
    for %%f in (*_test_report.txt) do (
        echo 打开报告: %%f
        start notepad "%%f"
    )
)

set /p open_trace=是否在浏览器中打开性能追踪? (y/n): 
if /i "%open_trace%"=="y" (
    echo 打开 Chrome 追踪工具...
    start chrome chrome://tracing/
    echo 请在打开的页面中加载 performance_trace.json 文件
)

goto :end

:show_example_results
echo.
echo ========================================
echo         示例程序完成
echo ========================================
echo.
echo 📄 生成的文件:
echo   - 示例测试报告: %cd%\gen_geos_data_example_report.txt
echo   - 性能追踪文件: %cd%\performance_trace.json
echo.

set /p open_example=是否打开示例报告? (y/n): 
if /i "%open_example%"=="y" (
    start notepad "gen_geos_data_example_report.txt"
)

goto :end

:end
echo.
echo 感谢使用 gen_geos_data 性能测试工具！
pause
