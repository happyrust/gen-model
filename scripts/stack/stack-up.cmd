@echo off
rem stack-up.cmd - double-click to start everything. ASCII only on purpose.
rem Uses stack.toml next to this file; or drop a config file onto this file to use that one.
rem First run: stack.toml does not exist yet. stack.ps1 creates it from stack.default.toml and stops, so you can edit [project].
setlocal
set "STACK_CFG=%~dp0stack.toml"
if not "%~1"=="" set "STACK_CFG=%~f1"
call "%~dp0stack.cmd" up -Config "%STACK_CFG%"
set "STACK_RC=%ERRORLEVEL%"
echo.
if "%STACK_RC%"=="0" echo [stack] up finished. Everything is running. Exit code 0.
if "%STACK_RC%"=="2" echo [stack] Nothing was started: the configuration needs attention. Read the Chinese messages above. Exit code 2.
if not "%STACK_RC%"=="0" if not "%STACK_RC%"=="2" echo [stack] up ended with exit code %STACK_RC%: not everything is running. Read the Chinese messages above.
pause
exit /b %STACK_RC%
