# Capture the full virtual screen to a PNG (evidence screenshots for E2E runs).
# Usage: powershell -File capture-screen.ps1 -OutFile "path\to\shot.png" [-ActivateTitle "window title"] [-SettleMs 1200]
param(
    [Parameter(Mandatory = $true)][string]$OutFile,
    [string]$ActivateTitle = "",
    [int]$SettleMs = 1200
)

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

if ($ActivateTitle -ne "") {
    $shell = New-Object -ComObject WScript.Shell
    [void]$shell.AppActivate($ActivateTitle)
    Start-Sleep -Milliseconds $SettleMs
}

$bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bitmap = New-Object System.Drawing.Bitmap $bounds.Width, $bounds.Height
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
$graphics.Dispose()

$dir = Split-Path -Parent $OutFile
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
$bitmap.Save($OutFile, [System.Drawing.Imaging.ImageFormat]::Png)
$bitmap.Dispose()
Write-Output "saved: $OutFile"
