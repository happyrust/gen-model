@echo off
setlocal EnableExtensions DisableDelayedExpansion
rem Run one macro in a fresh, single-purpose E3D DESIGN TTY session.
rem
rem   run_ams_c_entrymacro.bat <forward-slash-macro-path>
rem
rem Optional environment:
rem   L3_E3D_PROJECTS_DIR  parent directory of the selected E3D project
rem   L3_E3D_PROJECT       project code (default: AMS)
rem   L3_E3D_LOGIN         USER/PASSWORD (default: SYSTEM/XXXXXX)
rem   L3_E3D_MDB           MDB name (default: /ALL)
rem   L3_E3D_INSTALL_DIR   E3D install (default: C: stock 3.1)
rem   L3_E3D_PROJECT_EVAR  project evars batch (default: AMS under project root)
rem   L3_E3D_TIMEOUT_SECONDS / L3_E3D_PID_FILE
rem
rem Why this exists: redirected stdin is ignored, because core.dll spawns
rem pdmsconsole.exe and re-points its own std handles at that pipe; and
rem console_inject.ps1 has to AttachConsole to a console it does not own, which
rem has failed with ERROR_INVALID_HANDLE. AVEVA_DESIGN_ENTRYMACRO is read by
rem Startup.dll, which queues the macro once the event loop is up - so the macro
rem is part of TTY session startup rather than something injected into it.
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

for %%I in ("%~1") do set "E3D_MACRO=%%~fI"
if exist "%E3D_MACRO%" goto macro_ok
echo E3D macro does not exist: %E3D_MACRO% 1>&2
exit /b 2
:macro_ok

if not defined L3_E3D_INSTALL_DIR set "L3_E3D_INSTALL_DIR=C:\Program Files (x86)\AVEVA\Everything3D3.1"
if not defined L3_E3D_PROJECTS_DIR set "L3_E3D_PROJECTS_DIR=D:\AVEVA\Projects\E3D3.1"
if not defined L3_E3D_PROJECT set "L3_E3D_PROJECT=AMS"
if not defined L3_E3D_LOGIN set "L3_E3D_LOGIN=SYSTEM/XXXXXX"
if not defined L3_E3D_MDB set "L3_E3D_MDB=/ALL"
if not defined L3_E3D_TIMEOUT_SECONDS set "L3_E3D_TIMEOUT_SECONDS=1200"

set "E3DC=%L3_E3D_INSTALL_DIR%"
set "projects_dir=%L3_E3D_PROJECTS_DIR%"
if not "%projects_dir:~-1%"=="\" set "projects_dir=%projects_dir%\"
if not defined L3_E3D_PROJECT_EVAR set "L3_E3D_PROJECT_EVAR=%projects_dir%AvevaMarineSample\evarsAvevaMarineSample.bat"
if exist "%L3_E3D_PROJECT_EVAR%" goto project_evar_ok
echo E3D project evars do not exist: %L3_E3D_PROJECT_EVAR% 1>&2
exit /b 3
:project_evar_ok
if exist "%E3DC%\des.exe" goto des_ok
echo E3D des.exe does not exist: %E3DC%\des.exe 1>&2
exit /b 4
:des_ok

cd /d "%E3DC%"
call evars.bat "%E3DC%\"
if errorlevel 1 exit /b %ERRORLEVEL%
rem Rebind the selected project after stock evars/custom_evars so the session and
rem the model service always point at the same L3 work copy.
call "%L3_E3D_PROJECT_EVAR%"
if errorlevel 1 exit /b %ERRORLEVEL%
set AVEVA_PRODUCT=3D
set AVEVA_DESIGN_INSTALLED_DIR=%E3DC%\
call set_aveva_design.bat
if errorlevel 1 exit /b %ERRORLEVEL%
set PDMS_SHOWCONSOLE=
set PDMS_HIDECONSOLE=1
set "E3D_MACRO=%E3D_MACRO:\=/%"
set AVEVA_DESIGN_ENTRYMACRO=$M "%E3D_MACRO%"
set "E3D_LAUNCHER=%~dp0launch_detached.ps1"

echo === E3D TTY project=%L3_E3D_PROJECT% mdb=%L3_E3D_MDB% macro=%E3D_MACRO% ===
if not defined L3_E3D_PID_FILE goto launch_without_pid_file
powershell -NoProfile -ExecutionPolicy Bypass -File "%E3D_LAUNCHER%" -Exe "%E3DC%\des.exe" -WorkingDirectory "%E3DC%" -Arguments "-tty %L3_E3D_PROJECT% %L3_E3D_LOGIN% %L3_E3D_MDB%" -Wait -TimeoutSeconds %L3_E3D_TIMEOUT_SECONDS% -PidFile "%L3_E3D_PID_FILE%"
goto launch_finished
:launch_without_pid_file
powershell -NoProfile -ExecutionPolicy Bypass -File "%E3D_LAUNCHER%" -Exe "%E3DC%\des.exe" -WorkingDirectory "%E3DC%" -Arguments "-tty %L3_E3D_PROJECT% %L3_E3D_LOGIN% %L3_E3D_MDB%" -Wait -TimeoutSeconds %L3_E3D_TIMEOUT_SECONDS%
:launch_finished
set "E3D_EXIT=%ERRORLEVEL%"
echo === E3D TTY EXIT CODE: %E3D_EXIT% ===
exit /b %E3D_EXIT%
