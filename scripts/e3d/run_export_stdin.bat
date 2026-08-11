@echo off
rem Headless export: a direct des.exe launch never starts CAF (verified - the
rem process loads no Aveva.ApplicationFramework.dll), so addins and
rem AVEVA_DESIGN_ENTRYMACRO can never fire. What it does load is PMLNet.dll, i.e.
rem this is a classic PDMS-style session, so the export is driven by feeding PML
rem commands on stdin instead.
set E3DC=E:\reverse\e3d\shadow_e3d31_aps_all
cd /d "%E3DC%"
call evars.bat "%E3DC%\"
set AVEVA_PRODUCT=3D
set AVEVA_DESIGN_INSTALLED_DIR=%E3DC%\
call set_aveva_design.bat
set AVEVA_DESIGN_CONSOLE_WINDOW=ACTIVE
echo === STARTING des.exe ams %1 (stdin-driven) ===
des.exe ams SYSTEM/XXXXXX %1 < "D:\work\plant-code\old\gen-model\scripts\e3d\export_stdin.txt"
echo === EXIT CODE: %ERRORLEVEL% ===
