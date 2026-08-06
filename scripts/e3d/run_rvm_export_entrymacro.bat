@echo off
rem 用启动入口宏跑 RVM 导出。
rem 导出宏只用原生 export/repre 命令，不依赖 appware 表单，所以不加载
rem PMLUI/DES/admin/start 也能跑完。
set E3DC=C:\Program Files (x86)\AVEVA\Everything3D3.1
cd /d "%E3DC%"
call evars.bat "%E3DC%\"
set AVEVA_PRODUCT=3D
set AVEVA_DESIGN_INSTALLED_DIR=%E3DC%\
call set_aveva_design.bat
set AVEVA_DESIGN_CONSOLE_WINDOW=ACTIVE
set AVEVA_DESIGN_ENTRYMACRO=$M "D:/work/plant-code/old/gen-model/scripts/e3d/rvm_export_c_iy_1r330_b.mac"
echo === STARTING des.exe ams SYSTEM/XXXXXX %1 (entrymacro=rvm export) ===
powershell -NoProfile -ExecutionPolicy Bypass -File "D:\work\plant-code\old\gen-model\scripts\e3d\launch_detached.ps1" -Exe "%E3DC%\des.exe" -WorkingDirectory "%E3DC%" -Arguments "ams SYSTEM/XXXXXX %1"
echo === LAUNCHER EXIT CODE: %ERRORLEVEL% ===
