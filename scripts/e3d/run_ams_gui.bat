@echo off
rem Launch E3D DESIGN on AvevaMarineSample with the STANDARD appware start macro.
rem
rem   run_ams_gui.bat [<mdb>]      default mdb: /ALL
rem
rem AVEVA_DESIGN_ENTRYMACRO is what brings the appware up. Without it the session
rem still gets a ribbon - CAF builds that on the .NET side - but PMLUI's
rem DES/admin/vars never runs, so not one synonym is ever defined, and every
rem ribbon button that goes through one dies in the command processor. Save Work
rem is the loudest: it runs !!runSynonym('CALLG MSAVEW'), runsynonym.pmlmac line
rem 44 is a bare $1, so CALLG reaches the parser undefined and the session
rem answers "(47,15) CP: Syntax error".
rem
rem The E: shadow install is the one to use. C:'s install no longer has a des.exe
rem at its root, and D:\AVEVA\Everything3D3.1 is an IDA workspace - a copy of
rem core.dll plus its .i64/.id0/.id1 databases, no des.exe and no evars.bat - so
rem the entry macro this script used to point at never existed. projects_dir comes
rem from custom_evars.bat under D:\AVEVA\Projects\E3D3.1.
rem
rem The entry macro goes through the 8.3 short path: $M takes the string as-is, so
rem any space in the install path would cut the macro path in half.
rem
rem Detached because des.exe is GUI-subsystem: started from a console it inherits
rem one and core.dll then skips spawning its own console host.

setlocal

set MDB=%1
if "%MDB%"=="" set MDB=/ALL

set E3DC=E:\reverse\e3d\shadow_e3d31_aps_all
cd /d "%E3DC%"
call evars.bat "%E3DC%\"
set AVEVA_PRODUCT=3D
set AVEVA_DESIGN_INSTALLED_DIR=%E3DC%\
call set_aveva_design.bat
set AVEVA_DESIGN_CONSOLE_WINDOW=ACTIVE
set PDMS_SHOWCONSOLE=1

for %%I in ("%E3DC%") do set E3DSHORT=%%~sI
set E3DMAC=%E3DSHORT:\=/%
set AVEVA_DESIGN_ENTRYMACRO=$M "%E3DMAC%/PMLUI/DES/admin/start"

echo === ENTRYMACRO: %AVEVA_DESIGN_ENTRYMACRO% ===
echo === STARTING (detached) des.exe ams SYSTEM/XXXXXX %MDB% ===
powershell -NoProfile -ExecutionPolicy Bypass -File "D:\work\plant-code\old\gen-model\scripts\e3d\launch_detached.ps1" -Exe "%E3DC%\des.exe" -WorkingDirectory "%E3DC%" -Arguments "ams SYSTEM/XXXXXX %MDB%"
echo === LAUNCHER EXIT CODE: %ERRORLEVEL% ===

endlocal