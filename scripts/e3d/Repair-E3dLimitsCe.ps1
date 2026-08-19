param(
    [Parameter(Mandatory = $true)]
    [string]$PmlLibRoot
)

$ErrorActionPreference = 'Stop'
$target = Join-Path $PmlLibRoot 'common\commands\designviewLimits.pmlcmd'
if (-not (Test-Path -LiteralPath $target)) {
    throw "Design view limits command not found: $target"
}

$text = [IO.File]::ReadAllText($target)
$originalText = $text
$marker = '-- AIOS AMS: make Limits CE visible when the active local drawlist is empty.'
$viewMarker = '-- AIOS AMS: direct Design documents expose a G3D gadget, not GM3D.'
$newline = if ($text.Contains("`r`n")) { "`r`n" } else { "`n" }

if (-not $text.Contains($viewMarker)) {
    $viewNeedle = @(
        '    -- Check is a 3D form'
        "    if (not !graphicalView.type().eq('GM3D')) then"
        '      return'
        '    endif'
    ) -join $newline
    if (-not $text.Contains($viewNeedle)) {
        throw "Limits CE graphical-view guard changed; no file was modified: $target"
    }
    $viewReplacement = @(
        '    -- Check is a 3D form'
        "    $viewMarker"
        "    if (not (!graphicalView.type().eq('GM3D') or !graphicalView.type().eq('G3D'))) then"
        '      return'
        '    endif'
    ) -join $newline
    $text = $text.Replace($viewNeedle, $viewReplacement)
}

if (-not $text.Contains($marker)) {
    $needle = @(
        "    if (!action.eq('CE')) then"
        ''
        '        !!gphViews.limits(!graphicalView, !!ce)'
    ) -join $newline
    if (-not $text.Contains($needle)) {
        throw "Limits CE command body changed; no file was modified: $target"
    }

    $replacement = @(
        "    if (!action.eq('CE')) then"
        ''
        "        $marker"
        '        -- The repaired direct AMS startup intentionally begins with an empty'
        '        -- local drawlist.  Standard Limits only changes the camera volume; an'
        '        -- absent CE therefore leaves a grey view and appears to do nothing.'
        '        !drawlist = !!gphDrawlists.drawlist(!graphicalView)'
        '        !drawlist.add(!!ce)'
        '        !drawlist.update()'
        '        handle any'
        "          !!alert.error('Unable to display the current element before setting view limits: ' & !!error.text)"
        '          return'
        '        endhandle'
        ''
        '        !!gphViews.limits(!graphicalView, !!ce)'
        '        !graphicalView.refresh()'
    ) -join $newline
    $text = $text.Replace($needle, $replacement)
}

$backup = "$target.aios-original"
if (-not (Test-Path -LiteralPath $backup)) {
    [IO.File]::WriteAllText($backup, $originalText, [Text.Encoding]::UTF8)
}
if ($text -ne $originalText) {
    [IO.File]::WriteAllText($target, $text, [Text.Encoding]::UTF8)
}

$verify = [IO.File]::ReadAllText($target)
foreach ($required in @(
        $marker,
        $viewMarker,
        "!graphicalView.type().eq('G3D')",
        '!drawlist = !!gphDrawlists.drawlist(!graphicalView)',
        '!drawlist.add(!!ce)',
        '!drawlist.update()',
        '!graphicalView.refresh()'
    )) {
    if (-not $verify.Contains($required)) {
        throw "Limits CE repair verification failed for '$required': $target"
    }
}

Write-Host "[PML] Limits CE drawlist repair verified: $target"
