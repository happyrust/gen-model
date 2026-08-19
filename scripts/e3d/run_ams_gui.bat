@echo off
chcp 65001 >nul 2>&1
setlocal

rem Launch the D: AvevaMarineSample project through the verified repaired AMS
rem startup chain.  This project copy is the source consumed by aios-database;
rem E:\reverse\e3d\launch_ams.bat intentionally targets the separate E: copy.
rem
rem Usage:
rem   run_ams_gui.bat [<mdb>]      default: /ALL
rem   set E3D_LOGIN=USER/PASSWORD  optional login override

set "MDB=%~1"
if not defined MDB if defined E3D_MDB set "MDB=%E3D_MDB%"
if not defined MDB set "MDB=/ALL"
if not defined E3D_LOGIN set "E3D_LOGIN=SYSTEM/XXXXXX"

set "REPAIRED_ROOT=E:\reverse\e3d"
set "PROJECTS_ROOT=D:\AVEVA\Projects\E3D3.1"
set "PROJECT_ROOT=%PROJECTS_ROOT%\AvevaMarineSample"
set "PROJECT_EVARS=%PROJECT_ROOT%\evarsAvevaMarineSample.bat"
set "E3D_LAUNCHER=%REPAIRED_ROOT%\launch_e3d_sample_repaired.ps1"
set "AMS_GRAPHICS_FINISHER=%REPAIRED_ROOT%\finish_ams_graphics_document.ps1"
set "AMS_RUNTIME_VERIFIER=%REPAIRED_ROOT%\verify_ams_runtime_health.ps1"
set "SHADOW_INSTALL=%REPAIRED_ROOT%\shadow_e3d31_aps_all"
set "SAFE_COPY_DLL=%REPAIRED_ROOT%\artifacts\design_edit_runtime_fix_20260811\Aveva.Core.Explorer.safe_copy.dll"
set "SELF_PASTE_GUARD_DLL=%REPAIRED_ROOT%\artifacts\copy_paste_self_guard_20260809\ExplorerControl.patched.dll"
set "DRAWLIST_MENU_FIX_DLL=%REPAIRED_ROOT%\artifacts\model_explorer_nested_add_remove_fix_20260815\DrawListAddin.active_empty_routing_fixed.dll"
set "AMS_USER_DIR=%REPAIRED_ROOT%\aveva_user_ams_d_project"
set "AMS_WORK_DIR=%REPAIRED_ROOT%\aveva_work_ams_d_project"

rem The generic NativeFixup repair macro points at YCYK and can block the AMS UI.
rem The AMS graphics finisher below owns document creation and drawlist filling.
set "E3D_SKIP_REPAIR_MACRO=1"

for %%F in (
  "%E3D_LAUNCHER%"
  "%AMS_GRAPHICS_FINISHER%"
  "%AMS_RUNTIME_VERIFIER%"
  "%PROJECT_EVARS%"
  "%SHADOW_INSTALL%\des.exe"
  "%SAFE_COPY_DLL%"
  "%SELF_PASTE_GUARD_DLL%"
  "%DRAWLIST_MENU_FIX_DLL%"
) do if not exist "%%~F" (
  echo [FAIL] Missing required file: %%~F
  exit /b 1
)

where pwsh.exe >nul 2>&1
if errorlevel 1 (
  echo [FAIL] pwsh.exe was not found in PATH.
  exit /b 1
)

echo [PROJECT] AMS ^(AvevaMarineSample, D: project copy^)
echo [EVARS]   %PROJECT_EVARS%
echo [MODULE]  Design
echo [MDB]     %MDB%
echo [INSTALL] %SHADOW_INSTALL%

rem Keep the verified Model Explorer and hierarchy editing fixes byte-identical
rem to the repaired AMS launcher before starting Design.
fc /b "%SELF_PASTE_GUARD_DLL%" "%SHADOW_INSTALL%\ExplorerControl.dll" >nul 2>&1
if errorlevel 1 copy /y "%SELF_PASTE_GUARD_DLL%" "%SHADOW_INSTALL%\ExplorerControl.dll" >nul || exit /b 1
fc /b "%SAFE_COPY_DLL%" "%SHADOW_INSTALL%\Aveva.Core.Explorer.dll" >nul 2>&1
if errorlevel 1 copy /y "%SAFE_COPY_DLL%" "%SHADOW_INSTALL%\Aveva.Core.Explorer.dll" >nul || exit /b 1
fc /b "%DRAWLIST_MENU_FIX_DLL%" "%SHADOW_INSTALL%\DrawListAddin.dll" >nul 2>&1
if errorlevel 1 copy /y "%DRAWLIST_MENU_FIX_DLL%" "%SHADOW_INSTALL%\DrawListAddin.dll" >nul || exit /b 1
echo [EDIT] Copy/paste/delete and Model Explorer DrawList fixes ready.

pwsh.exe -NoProfile -ExecutionPolicy Bypass -File "%E3D_LAUNCHER%" ^
  -UseShadowInstall ^
  -NoCleanup ^
  -NoDllDeploy ^
  -NoSensitiveBinaryRefresh ^
  -NoCorePatch ^
  -NoPostRepair ^
  -NoSuspendedLaunch ^
  -UseInjectorWatcher ^
  -MinimalRuntimePatches ^
  -PostInjectionDelaySeconds 55 ^
  -UserDir "%AMS_USER_DIR%" ^
  -WorkDir "%AMS_WORK_DIR%" ^
  -ProjectCode ams ^
  -ProjectRoot "%PROJECTS_ROOT%" ^
  -ProjectDirectory AvevaMarineSample ^
  -ProjectEnvPrefix AMS ^
  -ProjectEvarsBatch "%PROJECT_EVARS%" ^
  -Login "%E3D_LOGIN%" ^
  -Mdb "%MDB%"

set "LAUNCH_EXIT=%ERRORLEVEL%"
if not "%LAUNCH_EXIT%"=="0" goto launch_failed

pwsh.exe -NoProfile -ExecutionPolicy Bypass -File "%AMS_GRAPHICS_FINISHER%"
set "LAUNCH_EXIT=%ERRORLEVEL%"
if not "%LAUNCH_EXIT%"=="0" goto launch_failed

pwsh.exe -NoProfile -ExecutionPolicy Bypass -File "%AMS_RUNTIME_VERIFIER%"
set "LAUNCH_EXIT=%ERRORLEVEL%"
if "%LAUNCH_EXIT%"=="0" (
  echo [OK] D: AMS MDB, model tree, 3D drawlist, and edit runtime verified.
  exit /b 0
)

:launch_failed
echo [FAIL] AMS launcher exited with code %LAUNCH_EXIT%.
exit /b %LAUNCH_EXIT%
