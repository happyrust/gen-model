@echo off
rem 带完整 appware 起 E3D DESIGN（分离式），这样 Command Window 面板存在，
rem 可用 UIAutomation 把 $M 宏调用打进去。
rem 与 run_ams_gui.bat 的区别：这里走 launch_detached.ps1，不阻塞调用方。
set E3DC=E:\reverse\e3d\shadow_e3d31_aps_all
cd /d "%E3DC%"
call evars.bat "%E3DC%\"
set AVEVA_PRODUCT=3D
set AVEVA_DESIGN_INSTALLED_DIR=%E3DC%\
call set_aveva_design.bat
set AVEVA_DESIGN_CONSOLE_WINDOW=ACTIVE
rem 短路径：$M 原样吃字符串，安装路径里只要有空格就会把宏路径截断。
for %%I in ("%E3DC%") do set E3DSHORT=%%~sI
set E3DMAC=%E3DSHORT:\=/%
set AVEVA_DESIGN_ENTRYMACRO=$M "%E3DMAC%/PMLUI/DES/admin/start"
rem 同时开控制台命令通道：单独用 PDMS_SHOWCONSOLE（无 appware）时注入的按键不被消费，
rem 这里让 appware 与控制台通道并存，命令窗口就绪后再注入。
set PDMS_SHOWCONSOLE=1
echo === STARTING des.exe ams SYSTEM/XXXXXX %1 (full appware, detached) ===
powershell -NoProfile -ExecutionPolicy Bypass -File "D:\work\plant-code\old\gen-model\scripts\e3d\launch_detached.ps1" -Exe "%E3DC%\des.exe" -WorkingDirectory "%E3DC%" -Arguments "ams SYSTEM/XXXXXX %1"
echo === LAUNCHER EXIT CODE: %ERRORLEVEL% ===
