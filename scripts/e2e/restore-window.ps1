# Restore (un-minimize) a process's main window without demanding focus.
param([Parameter(Mandatory = $true)][string]$ProcessName)

Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win32Show {
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hwnd, int cmd);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
}
"@

$proc = Get-Process -Name $ProcessName -ErrorAction Stop | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $proc) { Write-Error "no window for process $ProcessName"; exit 1 }
if ([Win32Show]::IsIconic($proc.MainWindowHandle)) {
    # 9 = SW_RESTORE
    [void][Win32Show]::ShowWindow($proc.MainWindowHandle, 9)
    Start-Sleep -Milliseconds 1200
}
Write-Output "window state ok: $ProcessName"
