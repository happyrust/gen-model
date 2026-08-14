@echo off
rem 无图形会话导出 1112 CWALL /1RS-WF03-W-C-RR001 的窄口径 RVM。
rem 用法同 run_rvm_export_nogfx.bat，只换 stdin 宏。
set E3DC=E:\reverse\e3d\shadow_e3d31_aps_all
cd /d "%E3DC%"
call evars.bat "%E3DC%\"
set AVEVA_PRODUCT=3D
set AVEVA_DESIGN_INSTALLED_DIR=%E3DC%\
call set_aveva_design.bat
set PDMS_NOGRAPHICS=1
set PDMS_SHOWCONSOLE=1
echo === STARTING des.exe ams SYSTEM/XXXXXX %1 (NOGRAPHICS, stdin 1112 CWALL) ===
des.exe ams SYSTEM/XXXXXX %1 < "D:\work\plant-code\old\gen-model\scripts\e3d\rvm_export_stdin_1112.txt"
echo === EXIT CODE: %ERRORLEVEL% ===
