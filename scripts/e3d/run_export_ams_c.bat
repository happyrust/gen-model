@echo off
rem Same export, but against the HEALTHY C: installation.
rem
rem The D: install never starts its appware - every assembly there is byte-identical
rem to C: except Startup.dll (770048 vs 774656), i.e. D:'s copy has been patched -
rem so its sessions come up with no CAF, no ribbon and no command loop at all.
rem C:'s custom_evars.bat already points projects_dir at D:\AVEVA\Projects\E3D3.1
rem and sources evarsAvevaMarineSample.bat, so the ams project is available here too.
rem
rem Launch is detached because des.exe is a GUI-subsystem binary: started from a
rem console it inherits one, and core.dll then skips spawning its console host.
set E3DC=C:\Program Files (x86)\AVEVA\Everything3D3.1
cd /d "%E3DC%"
call evars.bat "%E3DC%\"
set AVEVA_PRODUCT=3D
set AVEVA_DESIGN_INSTALLED_DIR=%E3DC%\
call set_aveva_design.bat
set PDMS_SHOWCONSOLE=1
set GENMODEL_NOUN_LAYOUT_OUT=D:\work\plant-code\old\gen-model\output\noun_layout.json
set GENMODEL_NOUN_LAYOUT_LOG=D:\work\plant-code\old\gen-model\output\noun_layout_export.log
set GENMODEL_NOUN_SLOTS_OUT=D:\work\plant-code\old\gen-model\output\noun_descriptor_slots.json
set GENMODEL_NOUN_SLOTS_BUDGET=300000
echo === STARTING (detached) C: des.exe ams SYSTEM/XXXXXX %1 ===
powershell -NoProfile -ExecutionPolicy Bypass -File "D:\work\plant-code\old\gen-model\scripts\e3d\launch_detached.ps1" -Exe "%E3DC%\des.exe" -WorkingDirectory "%E3DC%" -Arguments "ams SYSTEM/XXXXXX %1"
echo === LAUNCHER EXIT CODE: %ERRORLEVEL% ===
