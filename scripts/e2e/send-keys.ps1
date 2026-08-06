# Activate a window by title and send keystrokes (drives E3D command window / rs-plant search).
# Usage: powershell -File send-keys.ps1 -Title "AVEVA E3D Design Design" -Keys '$M "D:/path/macro.mac"{ENTER}' [-SettleMs 1500]
param(
    [Parameter(Mandatory = $true)][string]$Title,
    [Parameter(Mandatory = $true)][string]$Keys,
    [int]$SettleMs = 1500
)

Add-Type -AssemblyName System.Windows.Forms
$shell = New-Object -ComObject WScript.Shell
if (-not $shell.AppActivate($Title)) {
    Write-Error "window not found: $Title"
    exit 1
}
Start-Sleep -Milliseconds $SettleMs
[System.Windows.Forms.SendKeys]::SendWait($Keys)
Write-Output "sent to [$Title]: $Keys"
