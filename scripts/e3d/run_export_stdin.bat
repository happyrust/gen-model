@echo off
rem Headless export: a direct des.exe launch never starts CAF (verified - the
rem process loads no Aveva.ApplicationFramework.dll), so addins and
rem AVEVA_DESIGN_ENTRYMACRO can never fire. What it does load is PMLNet.dll, i.e.
rem this is a classic PDMS-style session, so the export is driven by feeding PML
rem commands on stdin instead.
cd /d D:\AVEVA\Everything3D3.1
call evars.bat "D:\AVEVA\Everything3D3.1\"
set AVEVA_PRODUCT=3D
set AVEVA_DESIGN_INSTALLED_DIR=D:\AVEVA\Everything3D3.1\
call set_aveva_design.bat
set AVEVA_DESIGN_CONSOLE_WINDOW=ACTIVE
echo === STARTING des.exe ams %1 (stdin-driven) ===
des.exe ams SYSTEM/XXXXXX %1 < "D:\work\plant-code\old\gen-model\scripts\e3d\export_stdin.txt"
echo === EXIT CODE: %ERRORLEVEL% ===
