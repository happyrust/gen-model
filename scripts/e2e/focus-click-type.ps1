# Activate a window (verified), click at window-relative coordinates, then type keys.
# Aborts without clicking/typing if the window refuses to come to the foreground.
param(
    [Parameter(Mandatory = $true)][string]$Title,
    [int]$RelX = -1, [int]$RelY = -1,
    [string]$Keys = "",
    [int]$Retries = 5,
    [int]$PostClickMs = 600,
    [int]$Clicks = 1,
    [string]$Button = "left"
)

Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class Win32Input {
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int count);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extra);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

function Get-Fg {
    $hwnd = [Win32Input]::GetForegroundWindow()
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32Input]::GetWindowText($hwnd, $sb, 512)
    @{ Hwnd = $hwnd; Title = $sb.ToString() }
}

$shell = New-Object -ComObject WScript.Shell
$fg = $null
for ($i = 1; $i -le $Retries; $i++) {
    [System.Windows.Forms.SendKeys]::SendWait("%")
    [void]$shell.AppActivate($Title)
    Start-Sleep -Milliseconds 900
    $fg = Get-Fg
    if ($fg.Title -like "$Title*") { break }
    $fg = $null
    Start-Sleep -Milliseconds 700
}
if (-not $fg) {
    Write-Error "foreground is [$((Get-Fg).Title)], not [$Title]; refused to interact"
    exit 2
}

if ($RelX -ge 0 -and $RelY -ge 0) {
    $rect = New-Object Win32Input+RECT
    [void][Win32Input]::GetWindowRect($fg.Hwnd, [ref]$rect)
    $x = $rect.Left + $RelX
    $y = $rect.Top + $RelY
    [void][Win32Input]::SetCursorPos($x, $y)
    Start-Sleep -Milliseconds 200
    $down = 0x0002; $up = 0x0004
    if ($Button -eq "right") { $down = 0x0008; $up = 0x0010 }
    for ($c = 1; $c -le $Clicks; $c++) {
        [Win32Input]::mouse_event($down, 0, 0, 0, [UIntPtr]::Zero)
        [Win32Input]::mouse_event($up, 0, 0, 0, [UIntPtr]::Zero)
        if ($c -lt $Clicks) { Start-Sleep -Milliseconds 120 }
    }
    Start-Sleep -Milliseconds $PostClickMs
    Write-Output "$Button-clicked x$Clicks at screen ($x,$y)"
}

if ($Keys -ne "") {
    $check = Get-Fg
    if ($check.Title -notlike "$Title*") {
        Write-Error "foreground changed to [$($check.Title)] before typing; refused"
        exit 2
    }
    [System.Windows.Forms.SendKeys]::SendWait($Keys)
    Write-Output "typed: $Keys"
}
