@echo off
rem 无图形会话跑 RVM 导出。
rem core.dll 只认 PDMS_SHOWCONSOLE / PDMS_HIDECONSOLE / PDMS_GRAPHICS / PDMS_NOGRAPHICS
rem 这四个开关（见 run_export_console.bat 的考据）。带图形的 GUI 会话会让 core.dll
rem 另起 pdmsconsole.exe 并把自己的 std 句柄重定向过去，所以文件喂 stdin 被无视；
rem 关掉图形后本进程自己读 stdin，宏就能直接灌进去。
set E3DC=C:\Program Files (x86)\AVEVA\Everything3D3.1
cd /d "%E3DC%"
call evars.bat "%E3DC%\"
set AVEVA_PRODUCT=3D
set AVEVA_DESIGN_INSTALLED_DIR=%E3DC%\
call set_aveva_design.bat
set PDMS_NOGRAPHICS=1
set PDMS_SHOWCONSOLE=1
echo === STARTING des.exe ams SYSTEM/XXXXXX %1 (NOGRAPHICS, stdin from file) ===
des.exe ams SYSTEM/XXXXXX %1 < "D:\work\plant-code\old\gen-model\scripts\e3d\rvm_export_stdin_nogfx.txt"
echo === EXIT CODE: %ERRORLEVEL% ===
