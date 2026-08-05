@echo off
rem 用启动入口宏跑 RVM 导出。
rem 导出宏只用原生 export/repre 命令，不依赖 appware 表单，所以不加载
rem PMLUI/DES/admin/start 也能跑完。
cd /d D:\AVEVA\Everything3D3.1
call evars.bat "D:\AVEVA\Everything3D3.1\"
set AVEVA_PRODUCT=3D
set AVEVA_DESIGN_INSTALLED_DIR=D:\AVEVA\Everything3D3.1\
call set_aveva_design.bat
set AVEVA_DESIGN_CONSOLE_WINDOW=ACTIVE
set AVEVA_DESIGN_ENTRYMACRO=$M "D:/work/plant-code/old/gen-model/scripts/e3d/rvm_export_c_iy_1r330_b.mac"
echo === STARTING des.exe ams SYSTEM/XXXXXX %1 (entrymacro=rvm export) ===
powershell -NoProfile -ExecutionPolicy Bypass -File "D:\work\plant-code\old\gen-model\scripts\e3d\launch_detached.ps1" -Arguments "ams SYSTEM/XXXXXX %1"
echo === LAUNCHER EXIT CODE: %ERRORLEVEL% ===
