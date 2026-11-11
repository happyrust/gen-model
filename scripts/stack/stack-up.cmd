@echo off
rem stack-up.cmd - double-click to start everything. ASCII only on purpose.
rem Uses stack.toml next to this file; or drop a config file onto this file to use that one.
setlocal
set "STACK_CFG=%~dp0stack.toml"
if not "%~1"=="" set "STACK_CFG=%~f1"
call "%~dp0stack.cmd" up -Config "%STACK_CFG%"
set "STACK_RC=%ERRORLEVEL%"
echo.
if "%STACK_RC%"=="0" (
  echo [stack] up finished. Exit code 0.
) else (
  echo [stack] up did NOT finish. Exit code %STACK_RC%. Read the messages above.
)
pause
exit /b %STACK_RC%
