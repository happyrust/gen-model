@echo off
rem 带完整 appware 起 E3D DESIGN（分离式），这样 Command Window 面板存在，
rem 可用 UIAutomation 把 $M 宏调用打进去。
rem 与 run_ams_gui.bat 的区别：这里走 launch_detached.ps1，不阻塞调用方。
cd /d D:\AVEVA\Everything3D3.1
call evars.bat "D:\AVEVA\Everything3D3.1\"
set AVEVA_PRODUCT=3D
set AVEVA_DESIGN_INSTALLED_DIR=D:\AVEVA\Everything3D3.1\
call set_aveva_design.bat
set AVEVA_DESIGN_CONSOLE_WINDOW=ACTIVE
set AVEVA_DESIGN_ENTRYMACRO=$M "D:/AVEVA/Everything3D3.1/PMLUI/DES/admin/start"
rem 同时开控制台命令通道：单独用 PDMS_SHOWCONSOLE（无 appware）时注入的按键不被消费，
rem 这里让 appware 与控制台通道并存，命令窗口就绪后再注入。
set PDMS_SHOWCONSOLE=1
echo === STARTING des.exe ams SYSTEM/XXXXXX %1 (full appware, detached) ===
powershell -NoProfile -ExecutionPolicy Bypass -File "D:\work\plant-code\old\gen-model\scripts\e3d\launch_detached.ps1" -Arguments "ams SYSTEM/XXXXXX %1"
echo === LAUNCHER EXIT CODE: %ERRORLEVEL% ===
