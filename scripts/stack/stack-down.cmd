@echo off
rem stack-down.cmd - double-click to stop what stack-up started. ASCII only on purpose.
rem Uses stack.toml next to this file; or drop a config file onto this file to use that one.
setlocal
set "STACK_CFG=%~dp0stack.toml"
if not "%~1"=="" set "STACK_CFG=%~f1"
call "%~dp0stack.cmd" down -Config "%STACK_CFG%"
set "STACK_RC=%ERRORLEVEL%"
echo.
echo [stack] down finished. Exit code %STACK_RC%.
pause
exit /b %STACK_RC%
