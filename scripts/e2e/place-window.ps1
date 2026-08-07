# Find a top-level window by exact title (optionally scoped to a process) and
# move/resize/show it. Never demands keyboard focus.
param(
    [Parameter(Mandatory = $true)][string]$Title,
    [int]$X = 60, [int]$Y = 60, [int]$Width = 1720, [int]$Height = 1040
)

Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class Win32Place {
    public delegate bool EnumProc(IntPtr hwnd, IntPtr lparam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lparam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int count);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hwnd, int cmd);
    [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr hwnd, int x, int y, int w, int h, bool repaint);
    public static IntPtr Find(string title) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((hwnd, lp) => {
            var sb = new StringBuilder(512); GetWindowText(hwnd, sb, 512);
            if (sb.ToString() == title) { found = hwnd; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
"@

$hwnd = [Win32Place]::Find($Title)
if ($hwnd -eq [IntPtr]::Zero) { Write-Error "window not found: $Title"; exit 1 }
# 8 = SW_SHOWNA (show without activating)
[void][Win32Place]::ShowWindow($hwnd, 8)
[void][Win32Place]::MoveWindow($hwnd, $X, $Y, $Width, $Height, $true)
Start-Sleep -Milliseconds 1500
Write-Output ("placed [{0}] hwnd=0x{1:X} at {2},{3} {4}x{5}" -f $Title, $hwnd.ToInt64(), $X, $Y, $Width, $Height)
