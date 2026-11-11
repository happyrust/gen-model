@echo off
rem stack-status.cmd - double-click to see what is running. ASCII only on purpose.
rem Uses stack.toml next to this file; or drop a config file onto this file to use that one.
setlocal
set "STACK_CFG=%~dp0stack.toml"
if not "%~1"=="" set "STACK_CFG=%~f1"
call "%~dp0stack.cmd" status -Config "%STACK_CFG%"
set "STACK_RC=%ERRORLEVEL%"
echo.
echo [stack] status finished. Exit code %STACK_RC%.
pause
exit /b %STACK_RC%
