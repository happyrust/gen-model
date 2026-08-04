@echo off
rem core.dll spawns pdmsconsole.exe itself (CreateProcess + stdin/stdout/stderr
rem pipes) and re-redirects its own std handles onto it - which is why feeding
rem des.exe's stdin from a file is ignored. The switches core.dll actually reads
rem are PDMS_SHOWCONSOLE / PDMS_HIDECONSOLE / PDMS_GRAPHICS / PDMS_NOGRAPHICS;
rem AVEVA_DESIGN_CONSOLE_WINDOW only becomes pdms_console_window, which appears
rem in no shipped binary.
cd /d D:\AVEVA\Everything3D3.1
call evars.bat "D:\AVEVA\Everything3D3.1\"
set AVEVA_PRODUCT=3D
set AVEVA_DESIGN_INSTALLED_DIR=D:\AVEVA\Everything3D3.1\
call set_aveva_design.bat
set PDMS_SHOWCONSOLE=1
set PDMS_ACTIVATE=1
set GENMODEL_NOUN_LAYOUT_OUT=D:\work\plant-code\old\gen-model\output\noun_layout.json
set GENMODEL_NOUN_LAYOUT_LOG=D:\work\plant-code\old\gen-model\output\noun_layout_export.log
echo === STARTING des.exe ams SYSTEM/XXXXXX %1 (detached, PDMS_SHOWCONSOLE) ===
powershell -NoProfile -ExecutionPolicy Bypass -File "D:\work\plant-code\old\gen-model\scripts\e3d\launch_detached.ps1" -Arguments "ams SYSTEM/XXXXXX %1"
echo === LAUNCHER EXIT CODE: %ERRORLEVEL% ===
