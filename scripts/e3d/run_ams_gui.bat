@echo off
rem Launch E3D DESIGN on AvevaMarineSample with the STANDARD appware start macro,
rem so the Command Window is available to run the noun-layout export by hand.
cd /d D:\AVEVA\Everything3D3.1
call evars.bat "D:\AVEVA\Everything3D3.1\"
set AVEVA_PRODUCT=3D
set AVEVA_DESIGN_INSTALLED_DIR=D:\AVEVA\Everything3D3.1\
call set_aveva_design.bat
set AVEVA_DESIGN_CONSOLE_WINDOW=ACTIVE
set AVEVA_DESIGN_ENTRYMACRO=$M "D:/AVEVA/Everything3D3.1/PMLUI/DES/admin/start"
echo === STARTING des.exe ams %1 ===
des.exe ams SYSTEM/XXXXXX %1
echo === EXIT CODE: %ERRORLEVEL% ===
