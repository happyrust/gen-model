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
$routeMarker = '-- AIOS AMS: route direct G3D commands to the newest registered drawlist.'
$visibleViewMarker = '-- AIOS AMS: apply limits to the view attached to that drawlist.'
$preLimitRefreshMarker = '-- AIOS AMS: materialise newly-added CE graphics before reading its limits.'
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

# The direct AMS document is created after the managed Core3D bootstrap. Its
# visible drawlist is therefore the newest registered list, while resolving
# the PML G3D gadget can return stale list 0. Keep Limits CE on the same route
# as Model Explorer Add/Remove.
if (-not $text.Contains($routeMarker)) {
    $routeNeedle = '        !drawlist = !!gphDrawlists.drawlist(!graphicalView)'
    if (-not $text.Contains($routeNeedle)) {
        throw "Limits CE drawlist route changed; no file was modified: $target"
    }
    $routeReplacement = @(
        "        $routeMarker"
        '        !drawlistToken = 0'
        '        do !index indices !!gphDrawlists.drawlists'
        '          if (!index gt !drawlistToken) then'
        '            !drawlistToken = !index'
        '          endif'
        '        enddo'
        '        if (!drawlistToken eq 0) then'
        "          !!alert.error('No drawlist is registered for the current graphical document')"
        '          return'
        '        endif'
        '        !drawlist = !!gphDrawlists.drawlists[!drawlistToken]'
    ) -join $newline
    $text = $text.Replace($routeNeedle, $routeReplacement)
}

# Migrate the first implementation, which treated DRAWLISTS as a method. In
# this PML object it is a sparse member array indexed by native drawlist token.
$methodRoute = @(
    "        $routeMarker"
    '        !drawlistTokens = !!gphDrawlists.drawlists()'
    '        if (!drawlistTokens.size() eq 0) then'
    "          !!alert.error('No drawlist is registered for the current graphical document')"
    '          return'
    '        endif'
    '        !drawlistToken = !drawlistTokens[!drawlistTokens.size()]'
    '        !drawlist = !!gphDrawlists.drawlist(!drawlistToken)'
) -join $newline
if ($text.Contains($methodRoute)) {
    $arrayRoute = @(
        "        $routeMarker"
        '        !drawlistToken = 0'
        '        do !index indices !!gphDrawlists.drawlists'
        '          if (!index gt !drawlistToken) then'
        '            !drawlistToken = !index'
        '          endif'
        '        enddo'
        '        if (!drawlistToken eq 0) then'
        "          !!alert.error('No drawlist is registered for the current graphical document')"
        '          return'
        '        endif'
        '        !drawlist = !!gphDrawlists.drawlists[!drawlistToken]'
    ) -join $newline
    $text = $text.Replace($methodRoute, $arrayRoute)
}

# The command's form argument can still resolve to the startup document's stale
# G3D gadget.  Adding CE to the newest drawlist then succeeds, but applying the
# camera limits to that stale gadget leaves the visible view unchanged.  Route
# both operations through the same drawlist/view attachment.
if (-not $text.Contains($visibleViewMarker)) {
    $visibleViewNeedle = '        !drawlist = !!gphDrawlists.drawlists[!drawlistToken]'
    if (-not $text.Contains($visibleViewNeedle)) {
        throw "Limits CE visible-view route changed; no file was modified: $target"
    }
    $visibleViewReplacement = @(
        $visibleViewNeedle
        "        $visibleViewMarker"
        '        !attachedViews = !!gphDrawlists.views(!drawlistToken)'
        '        if (!attachedViews.empty()) then'
        "          !!alert.error('The active drawlist has no attached graphical view')"
        '          return'
        '        endif'
        '        !graphicalView = !attachedViews[1]'
    ) -join $newline
    $text = $text.Replace($visibleViewNeedle, $visibleViewReplacement)
}

# A newly-added hierarchy is materialised by the first view refresh.  Asking
# Core3D for its limits before that refresh leaves the first invocation on an
# empty camera volume; a second click then appears to fix it.  Build the CE
# graphics before applying the camera limits so one click is deterministic.
if (-not $text.Contains($preLimitRefreshMarker)) {
    $refreshNeedle = '        !drawlist.update()'
    if (-not $text.Contains($refreshNeedle)) {
        throw "Limits CE drawlist update changed; no file was modified: $target"
    }
    $refreshReplacement = @(
        $refreshNeedle
        "        $preLimitRefreshMarker"
        '        !graphicalView.refresh()'
    ) -join $newline
    $text = $text.Replace($refreshNeedle, $refreshReplacement)
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
        $routeMarker,
        $visibleViewMarker,
        $preLimitRefreshMarker,
        $viewMarker,
        "!graphicalView.type().eq('G3D')",
        'do !index indices !!gphDrawlists.drawlists',
        '!drawlist = !!gphDrawlists.drawlists[!drawlistToken]',
        '!attachedViews = !!gphDrawlists.views(!drawlistToken)',
        '!graphicalView = !attachedViews[1]',
        '!drawlist.add(!!ce)',
        '!drawlist.update()',
        '!graphicalView.refresh()'
    )) {
    if (-not $verify.Contains($required)) {
        throw "Limits CE repair verification failed for '$required': $target"
    }
}

Write-Host "[PML] Limits CE drawlist repair verified: $target"
