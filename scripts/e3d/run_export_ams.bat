@echo off
rem One-shot: launch E3D DESIGN on the AvevaMarineSample project so the
rem GenModelNounLayout CAF addin fires and writes noun_layout.json.
rem
rem AVEVA_DESIGN_ENTRYMACRO is deliberately NOT set: the literal appears in no
rem shipped E3D binary (only Startup.dll), and a direct des.exe launch ignores it,
rem which is why the entry-macro route silently produced nothing. Registration is
rem instead a "GenModelNounLayout" line in DesignAddins.xml.
cd /d D:\AVEVA\Everything3D3.1
call evars.bat "D:\AVEVA\Everything3D3.1\"
set AVEVA_PRODUCT=3D
set AVEVA_DESIGN_INSTALLED_DIR=D:\AVEVA\Everything3D3.1\
call set_aveva_design.bat
set AVEVA_DESIGN_CONSOLE_WINDOW=ACTIVE
set GENMODEL_NOUN_LAYOUT_OUT=D:\work\plant-code\old\gen-model\output\noun_layout.json
set GENMODEL_NOUN_LAYOUT_LOG=D:\work\plant-code\old\gen-model\output\noun_layout_export.log
echo === STARTING des.exe ams %1 ===
des.exe ams SYSTEM/XXXXXX %1
echo === EXIT CODE: %ERRORLEVEL% ===
