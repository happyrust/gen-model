@echo off
rem Run one macro in a fresh, single-purpose AvevaMarineSample DESIGN session.
rem
rem   run_ams_c_entrymacro.bat <forward-slash-macro-path>
rem
rem Why this exists: neither of the other two channels can drive a session
rem unattended. Redirecting des.exe's stdin is ignored, because core.dll spawns
rem pdmsconsole.exe and re-points its own std handles at that pipe; and
rem console_inject.ps1 has to AttachConsole to a console it does not own, which
rem has failed with ERROR_INVALID_HANDLE. AVEVA_DESIGN_ENTRYMACRO is read by
rem Startup.dll, which queues the macro once the event loop is up - so the macro
rem is part of session startup rather than something injected into it.
rem
rem The C: install is the one to use: its assemblies are stock, while D:'s
rem Startup.dll has been patched and its sessions come up with no command loop.
rem C:'s custom_evars.bat already points projects_dir at D:\AVEVA\Projects\E3D3.1.
rem
rem The launch is detached because des.exe is a GUI-subsystem binary: started
rem from a console it inherits one, and core.dll then skips spawning its own
rem console host.

if "%~1"=="" (
    echo usage: run_ams_c_entrymacro.bat "D:/path/to/macro.mac"
    exit /b 1
)

set E3DC=C:\Program Files (x86)\AVEVA\Everything3D3.1
cd /d "%E3DC%"
call evars.bat "%E3DC%\"
set AVEVA_PRODUCT=3D
set AVEVA_DESIGN_INSTALLED_DIR=%E3DC%\
call set_aveva_design.bat
set PDMS_SHOWCONSOLE=1
set AVEVA_DESIGN_ENTRYMACRO=$M "%~1"
echo === ENTRYMACRO: %AVEVA_DESIGN_ENTRYMACRO% ===
echo === STARTING (detached) C: des.exe ams SYSTEM/XXXXXX /ALL ===
powershell -NoProfile -ExecutionPolicy Bypass -File "D:\work\plant-code\old\gen-model\scripts\e3d\launch_detached.ps1" -Exe "%E3DC%\des.exe" -WorkingDirectory "%E3DC%" -Arguments "ams SYSTEM/XXXXXX /ALL"
echo === LAUNCHER EXIT CODE: %ERRORLEVEL% ===
