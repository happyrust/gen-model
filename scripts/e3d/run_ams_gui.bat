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
set "E3D_TEMP_ROOT=%REPAIRED_ROOT%\temp\frida"
if not exist "%E3D_TEMP_ROOT%" mkdir "%E3D_TEMP_ROOT%"
set "TEMP=%E3D_TEMP_ROOT%"
set "TMP=%E3D_TEMP_ROOT%"
set "PROJECTS_ROOT=D:\AVEVA\Projects\E3D3.1"
set "PROJECT_ROOT=%PROJECTS_ROOT%\AvevaMarineSample"
set "PROJECT_EVARS=%PROJECT_ROOT%\evarsAvevaMarineSample.bat"
set "E3D_LAUNCHER=%REPAIRED_ROOT%\launch_e3d_sample_repaired.ps1"
set "AMS_GRAPHICS_FINISHER=%REPAIRED_ROOT%\finish_ams_graphics_document.ps1"
set "AMS_RUNTIME_VERIFIER=%REPAIRED_ROOT%\verify_ams_runtime_health.ps1"
set "SHADOW_INSTALL=%REPAIRED_ROOT%\shadow_e3d31_aps_all"
set "ACTIVE_PMLLIB=%REPAIRED_ROOT%\PMLLIB"
set "SHADOW_PMLLIB=%SHADOW_INSTALL%\PMLLIB"
set "SAFE_COPY_DLL=%REPAIRED_ROOT%\artifacts\design_edit_runtime_fix_20260811\Aveva.Core.Explorer.safe_copy.dll"
set "SELF_PASTE_GUARD_DLL=%REPAIRED_ROOT%\artifacts\copy_paste_self_guard_20260809\ExplorerControl.patched.dll"
set "DRAWLIST_MENU_FIX_DLL=%REPAIRED_ROOT%\artifacts\model_explorer_nested_add_remove_fix_20260815\DrawListAddin.active_empty_routing_fixed.dll"
set "STEELWORK_GLOBAL_REPAIR=%~dp0Repair-E3dSteelworkGlobals.ps1"
set "STEELWORK_GLOBAL_INIT=%~dp0Initialize-E3dSteelworkGlobals.ps1"
set "LIMITS_CE_REPAIR=%~dp0Repair-E3dLimitsCe.ps1"
set "LIMITS_CE_INIT=%~dp0Initialize-E3dLimitsCe.ps1"
set "LIMITS_CE_MACRO=%~dp0ams_limits_ce.pmlmac"
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
  "%ACTIVE_PMLLIB%\common\commands\designviewLimits.pmlcmd"
  "%SHADOW_PMLLIB%\common\commands\designviewLimits.pmlcmd"
  "%SAFE_COPY_DLL%"
  "%SELF_PASTE_GUARD_DLL%"
  "%DRAWLIST_MENU_FIX_DLL%"
  "%STEELWORK_GLOBAL_REPAIR%"
  "%STEELWORK_GLOBAL_INIT%"
  "%LIMITS_CE_REPAIR%"
  "%LIMITS_CE_INIT%"
  "%LIMITS_CE_MACRO%"
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

rem Steelwork grid-plane commands register CE/FORM refresh callbacks during
rem startup.  Their command manager must create the shared settings object
rem before registration, otherwise every selection change emits PML (2,751).
pwsh.exe -NoProfile -ExecutionPolicy Bypass -File "%STEELWORK_GLOBAL_REPAIR%" ^
  -PmlLibRoot "%ACTIVE_PMLLIB%"
if errorlevel 1 exit /b 1

rem Limits CE normally only changes the camera.  This direct AMS profile starts
rem with an empty local drawlist, so ensure CE is present before applying limits.
pwsh.exe -NoProfile -ExecutionPolicy Bypass -File "%LIMITS_CE_REPAIR%" ^
  -PmlLibRoot "%ACTIVE_PMLLIB%"
if errorlevel 1 exit /b 1
pwsh.exe -NoProfile -ExecutionPolicy Bypass -File "%LIMITS_CE_REPAIR%" ^
  -PmlLibRoot "%SHADOW_PMLLIB%"
if errorlevel 1 exit /b 1

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

pwsh.exe -NoProfile -ExecutionPolicy Bypass -File "%STEELWORK_GLOBAL_INIT%"
set "LAUNCH_EXIT=%ERRORLEVEL%"
if not "%LAUNCH_EXIT%"=="0" goto launch_failed

pwsh.exe -NoProfile -ExecutionPolicy Bypass -File "%AMS_GRAPHICS_FINISHER%"
set "LAUNCH_EXIT=%ERRORLEVEL%"
if not "%LAUNCH_EXIT%"=="0" goto launch_failed

rem Preserve the command instance registered by Design startup. Replacing that
rem global after registration leaves the Ribbon delegate pointing at a dead object.
pwsh.exe -NoProfile -ExecutionPolicy Bypass -File "%LIMITS_CE_INIT%"
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
