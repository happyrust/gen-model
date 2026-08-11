@echo off
rem 无人值守 RVM 基准导出：靠 GenModelRvmExport CAF addin 自己在启动后触发。
rem 不加载 appware（addin 不需要命令窗口），会话更轻、起得更快。
rem 用法：run_rvm_export_addin.bat <MDB>   例如 run_rvm_export_addin.bat /ALL
set E3DC=E:\reverse\e3d\shadow_e3d31_aps_all
cd /d "%E3DC%"
call evars.bat "%E3DC%\"
set AVEVA_PRODUCT=3D
set AVEVA_DESIGN_INSTALLED_DIR=%E3DC%\
call set_aveva_design.bat

set GENMODEL_RVM_ELEMENT=/C-IY-1R330-B
set GENMODEL_RVM_OUT=D:\work\plant-code\old\gen-model\test_data\rvm\C-IY-1R330-B.rvm
set GENMODEL_RVM_LOG=D:\work\plant-code\old\gen-model\output\rvm_export_addin.log
set GENMODEL_RVM_DELAY_MS=20000
rem 置 1 则导出后自动退出会话；首轮先留着会话便于排查。
set GENMODEL_RVM_QUIT=0

echo === STARTING des.exe ams SYSTEM/XXXXXX %1 (GenModelRvmExport addin) ===
powershell -NoProfile -ExecutionPolicy Bypass -File "D:\work\plant-code\old\gen-model\scripts\e3d\launch_detached.ps1" -Exe "%E3DC%\des.exe" -WorkingDirectory "%E3DC%" -Arguments "ams SYSTEM/XXXXXX %1"
echo === LAUNCHER EXIT CODE: %ERRORLEVEL% ===
