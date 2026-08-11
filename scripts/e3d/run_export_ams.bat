@echo off
rem One-shot: launch E3D DESIGN on the AvevaMarineSample project so the
rem GenModelNounLayout CAF addin fires and writes noun_layout.json.
rem
rem AVEVA_DESIGN_ENTRYMACRO is deliberately NOT set: the literal appears in no
rem shipped E3D binary (only Startup.dll), and a direct des.exe launch ignores it,
rem which is why the entry-macro route silently produced nothing. Registration is
rem instead a "GenModelNounLayout" line in DesignAddins.xml.
set E3DC=E:\reverse\e3d\shadow_e3d31_aps_all
cd /d "%E3DC%"
call evars.bat "%E3DC%\"
set AVEVA_PRODUCT=3D
set AVEVA_DESIGN_INSTALLED_DIR=%E3DC%\
call set_aveva_design.bat
set AVEVA_DESIGN_CONSOLE_WINDOW=ACTIVE
set GENMODEL_NOUN_LAYOUT_OUT=D:\work\plant-code\old\gen-model\output\noun_layout.json
set GENMODEL_NOUN_LAYOUT_LOG=D:\work\plant-code\old\gen-model\output\noun_layout_export.log
echo === STARTING des.exe ams %1 ===
des.exe ams SYSTEM/XXXXXX %1
echo === EXIT CODE: %ERRORLEVEL% ===
