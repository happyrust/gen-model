@echo off
rem stack.cmd - thin wrapper around stack.ps1. ASCII only on purpose (cmd reads this file in the OEM code page).
rem Usage:  stack <command> [-Config <file>] ...      Run "stack help" for the full text.
rem Always uses Windows PowerShell 5.1, which every supported target machine has.
setlocal
set "STACK_PS=%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"
"%STACK_PS%" -NoProfile -ExecutionPolicy Bypass -File "%~dp0stack.ps1" %*
exit /b %ERRORLEVEL%
