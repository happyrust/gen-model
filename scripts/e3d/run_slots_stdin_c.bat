@echo off
rem Headless slot-descriptor dump against the HEALTHY C: installation.
rem A direct des.exe launch never starts CAF, so no addin and no entry macro can
rem fire - but PMLNet.dll is loaded, so PML fed on stdin can import the assembly
rem and drive the dump. This avoids both the CAF startup race and having to
rem automate somebody else's live session.
set E3DC=C:\Program Files (x86)\AVEVA\Everything3D3.1
cd /d "%E3DC%"
call evars.bat "%E3DC%\"
set AVEVA_PRODUCT=3D
set AVEVA_DESIGN_INSTALLED_DIR=%E3DC%\
call set_aveva_design.bat
set AVEVA_DESIGN_CONSOLE_WINDOW=ACTIVE
echo === STARTING C: des.exe ams (stdin-driven) ===
des.exe ams SYSTEM/XXXXXX %1 < "D:\work\plant-code\old\gen-model\scripts\e3d\slots_stdin.txt"
echo === EXIT CODE: %ERRORLEVEL% ===
