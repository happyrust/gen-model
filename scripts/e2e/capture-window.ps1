# Capture ONE window's content via PrintWindow (no focus stealing).
# Usage: powershell -File capture-window.ps1 -ProcessName "plant-ui-app" -OutFile "shot.png"
#        powershell -File capture-window.ps1 -WindowTitle "AVEVA E3D Design Design" -OutFile "shot.png"
param(
    [string]$ProcessName = "",
    [string]$WindowTitle = "",
    [Parameter(Mandatory = $true)][string]$OutFile
)

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class Win32Cap {
    public delegate bool EnumProc(IntPtr hwnd, IntPtr lparam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lparam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int count);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    public static IntPtr FindByTitle(string title) {
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

$hwnd = [IntPtr]::Zero
if ($WindowTitle -ne "") {
    $hwnd = [Win32Cap]::FindByTitle($WindowTitle)
    if ($hwnd -eq [IntPtr]::Zero) { Write-Error "no window titled $WindowTitle"; exit 1 }
} else {
    $proc = Get-Process -Name $ProcessName -ErrorAction Stop | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
    if (-not $proc) { Write-Error "no window for process $ProcessName"; exit 1 }
    $hwnd = $proc.MainWindowHandle
}

$rect = New-Object Win32Cap+RECT
[void][Win32Cap]::GetWindowRect($hwnd, [ref]$rect)
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top
if ($width -le 0 -or $height -le 0) { Write-Error "window has empty rect"; exit 1 }

$bitmap = New-Object System.Drawing.Bitmap $width, $height
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$hdc = $graphics.GetHdc()
# 2 = PW_RENDERFULLCONTENT: needed for DirectX / WebView surfaces.
[void][Win32Cap]::PrintWindow($hwnd, $hdc, 2)
$graphics.ReleaseHdc($hdc)
$graphics.Dispose()

$dir = Split-Path -Parent $OutFile
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
$bitmap.Save($OutFile, [System.Drawing.Imaging.ImageFormat]::Png)
$bitmap.Dispose()
Write-Output "saved: $OutFile ($width x $height)"
