@echo off
rem Probe: does the "graphics" keyword (the only argument launch.bat passes for a
rem Design start) bring up the CAF layer? Without CAF there is no addin host, and
rem the GenModelNounLayout addin registered in DesignAddins.xml cannot fire.
rem Pass the extra keyword as %2, e.g.  run_export_caf.bat /ALL graphics
cd /d D:\AVEVA\Everything3D3.1
call evars.bat "D:\AVEVA\Everything3D3.1\"
set AVEVA_PRODUCT=3D
set AVEVA_DESIGN_INSTALLED_DIR=D:\AVEVA\Everything3D3.1\
call set_aveva_design.bat
set AVEVA_DESIGN_CONSOLE_WINDOW=ACTIVE
set GENMODEL_NOUN_LAYOUT_OUT=D:\work\plant-code\old\gen-model\output\noun_layout.json
set GENMODEL_NOUN_LAYOUT_LOG=D:\work\plant-code\old\gen-model\output\noun_layout_export.log
echo === STARTING des.exe ams SYSTEM/XXXXXX %1 %2 ===
des.exe ams SYSTEM/XXXXXX %1 %2
echo === EXIT CODE: %ERRORLEVEL% ===
