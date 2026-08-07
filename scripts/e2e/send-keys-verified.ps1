# Activate a window, VERIFY it owns the foreground, then send keystrokes.
# Refuses to type if the foreground window doesn't match -- keystrokes must never
# land in whatever the user is currently using.
# Usage: powershell -File send-keys-verified.ps1 -Title "AVEVA E3D Design Design" -Keys '...{ENTER}'
param(
    [Parameter(Mandatory = $true)][string]$Title,
    [Parameter(Mandatory = $true)][string]$Keys,
    [int]$Retries = 5
)

Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class Win32Fg {
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int count);
}
"@

function Get-ForegroundTitle {
    $hwnd = [Win32Fg]::GetForegroundWindow()
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32Fg]::GetWindowText($hwnd, $sb, 512)
    $sb.ToString()
}

$shell = New-Object -ComObject WScript.Shell
for ($i = 1; $i -le $Retries; $i++) {
    # Priming with a harmless key press relaxes the foreground lock for AppActivate.
    [System.Windows.Forms.SendKeys]::SendWait("%")
    [void]$shell.AppActivate($Title)
    Start-Sleep -Milliseconds 900
    $fg = Get-ForegroundTitle
    if ($fg -like "$Title*") {
        [System.Windows.Forms.SendKeys]::SendWait($Keys)
        Write-Output "sent to [$fg]: $Keys"
        exit 0
    }
    Start-Sleep -Milliseconds 700
}
Write-Error "foreground is [$(Get-ForegroundTitle)], not [$Title]; refused to type"
exit 2
