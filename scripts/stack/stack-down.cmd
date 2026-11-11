@echo off
rem stack-down.cmd - double-click to stop what stack-up started. ASCII only on purpose.
rem Uses stack.toml next to this file; or drop a config file onto this file to use that one.
rem Exit code 6 means nothing was stopped because the backend is busy; this file then asks whether to stop anyway.
rem stack.cmd gets NUL as its input so that it can never swallow the answer typed for the question below.
setlocal
set "STACK_CFG=%~dp0stack.toml"
if not "%~1"=="" set "STACK_CFG=%~f1"
call "%~dp0stack.cmd" down -Config "%STACK_CFG%" <NUL
set "STACK_RC=%ERRORLEVEL%"
echo.
if "%STACK_RC%"=="6" goto busy
echo [stack] down finished. Exit code %STACK_RC%.
goto done
:busy
echo [stack] Nothing was stopped: the backend is busy, or it did not answer. The Chinese messages above say why. Exit code 6.
echo [stack] Stopping it now interrupts what it is doing; that work is redone the next time it starts.
set "STACK_ANSWER="
set /p "STACK_ANSWER=[stack] Stop everything anyway? Type Y and press Enter. Anything else leaves it all running: "
if /i not "%STACK_ANSWER%"=="Y" goto done
call "%~dp0stack.cmd" down -Config "%STACK_CFG%" -Force <NUL
set "STACK_RC=%ERRORLEVEL%"
echo.
echo [stack] down -Force finished. Exit code %STACK_RC%.
:done
pause
exit /b %STACK_RC%
