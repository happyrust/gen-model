<#
.SYNOPSIS
    Safely install an aios-database binary patch, verify startup, and roll back on failure.

.DESCRIPTION
    The installer verifies the patch sidecar, inspects its source/target identities, runs
    a dry-run while the old service is still available, stops only the process whose
    executable path exactly matches TargetExe, applies the patch, verifies the target
    SHA-256, starts the service, and requires /api/v1/health to report the target build_id.

    If anything after replacement fails, the generated .bak-<hash> file is atomically
    restored. When the old process was running before installation, the installer also
    attempts to restart it after rollback.

.EXAMPLE
    .\update.patch.exe.install.ps1

.EXAMPLE
    .\scripts\Install-AiosDatabasePatch.ps1 `
      -PatchExe D:\AMS\update.patch.exe `
      -TargetExe D:\AMS\aios-database.exe `
      -Workspace D:\AMS
#>
[CmdletBinding()]
param(
    [string]$PatchExe,
    [string]$TargetExe,
    [string]$Workspace,
    [string]$ConfigPath,
    [string]$HealthUrl,
    [ValidateRange(5, 1800)]
    [int]$StartupTimeoutSeconds = 120,
    [string[]]$ServiceArguments = @('serve'),
    [switch]$AllowMissingChecksum,
    [switch]$LeaveStoppedOnFailure
)

$ErrorActionPreference = 'Stop'

function Resolve-ExistingFile([string]$Path, [string]$Label) {
    $resolved = Resolve-Path -LiteralPath $Path -ErrorAction SilentlyContinue
    if (-not $resolved -or -not (Test-Path -LiteralPath $resolved.Path -PathType Leaf)) {
        throw "$Label 不存在：$Path"
    }
    return $resolved.Path
}

function Resolve-ExistingDirectory([string]$Path, [string]$Label) {
    $resolved = Resolve-Path -LiteralPath $Path -ErrorAction SilentlyContinue
    if (-not $resolved -or -not (Test-Path -LiteralPath $resolved.Path -PathType Container)) {
        throw "$Label 不存在：$Path"
    }
    return $resolved.Path
}

function Get-Sha256([string]$Path) {
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Invoke-JsonExe([string]$Exe, [string[]]$Arguments, [string]$Label) {
    $savedPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $output = @(& $Exe @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $savedPreference
    }
    if ($exitCode -ne 0) {
        throw "$Label 失败（exit $exitCode）：$($output -join "`n")"
    }
    try {
        return (($output -join "`n") | ConvertFrom-Json)
    }
    catch {
        throw "$Label 没有返回合法 JSON：$($output -join "`n")"
    }
}

function Get-ConfigValue([string]$Text, [string]$Name) {
    $pattern = '(?m)^\s*' + [regex]::Escape($Name) + '\s*=\s*["'']?([^"''#\r\n]+)'
    $match = [regex]::Match($Text, $pattern)
    if ($match.Success) { return $match.Groups[1].Value.Trim() }
    return ''
}

function Get-HealthUrl([string]$Config) {
    $text = [IO.File]::ReadAllText($Config)
    $address = Get-ConfigValue $text 'http_api_addr'
    if (-not $address) { $address = Get-ConfigValue $text 'server_release_ip' }
    if (-not $address) { $address = '127.0.0.1:9099' }
    if ($address -notmatch '^https?://') { $address = "http://$address" }
    try {
        $builder = New-Object System.UriBuilder -ArgumentList $address
    }
    catch {
        throw "配置中的 HTTP 地址无效：$address"
    }
    if ($builder.Host -eq '0.0.0.0' -or $builder.Host -eq '::' -or $builder.Host -eq '*') {
        $builder.Host = '127.0.0.1'
    }
    $builder.Path = '/api/v1/health'
    $builder.Query = ''
    $builder.Fragment = ''
    return $builder.Uri.AbsoluteUri
}

function Get-TargetProcesses([string]$Executable) {
    $full = [IO.Path]::GetFullPath($Executable)
    return @(
        Get-CimInstance Win32_Process -Filter "Name='aios-database.exe'" -ErrorAction SilentlyContinue |
            Where-Object {
                $_.ExecutablePath -and
                [string]::Equals(
                    [IO.Path]::GetFullPath($_.ExecutablePath),
                    $full,
                    [StringComparison]::OrdinalIgnoreCase
                )
            }
    )
}

function Stop-TargetProcesses([string]$Executable) {
    $processes = @(Get-TargetProcesses $Executable)
    foreach ($process in $processes) {
        Stop-Process -Id $process.ProcessId -Force -ErrorAction Stop
    }
    $deadline = (Get-Date).AddSeconds(15)
    do {
        $remaining = @(Get-TargetProcesses $Executable)
        if ($remaining.Count -eq 0) { return $processes }
        Start-Sleep -Milliseconds 200
    } while ((Get-Date) -lt $deadline)
    throw "无法停止目标进程：$($remaining.ProcessId -join ', ')"
}

function Start-AiosDatabase(
    [string]$Executable,
    [string]$WorkingDirectory,
    [string]$Configuration,
    [string]$PackageRoot,
    [string[]]$Arguments,
    [string]$LogPrefix
) {
    $overrides = [ordered]@{
        DB_OPTION_FILE = $Configuration
        PLANT_UI_WEB_ROOT = Join-Path $PackageRoot 'web'
        PLANT_ASSET_ROOT = Join-Path $WorkingDirectory 'assets'
        AIOS_E3D_SCHEMA_ROOT = Join-Path $PackageRoot 'schema'
        AIOS_OPEN_BROWSER = '0'
        RUST_MIN_STACK = '134217728'
    }
    $surrealCandidates = @(
        (Join-Path $WorkingDirectory 'bin\surreal.exe'),
        (Join-Path $PackageRoot 'bin\surreal.exe')
    )
    $surreal = $surrealCandidates |
        Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
        Select-Object -First 1
    if ($surreal) { $overrides['AIOS_SURREAL_EXE'] = $surreal }

    $previous = @{}
    foreach ($name in $overrides.Keys) {
        $previous[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
        [Environment]::SetEnvironmentVariable($name, [string]$overrides[$name], 'Process')
    }
    $logDir = Join-Path $PackageRoot 'logs'
    New-Item -ItemType Directory -Path $logDir -Force | Out-Null
    $stdout = Join-Path $logDir "$LogPrefix.stdout.log"
    $stderr = Join-Path $logDir "$LogPrefix.stderr.log"
    try {
        $process = Start-Process `
            -FilePath $Executable `
            -ArgumentList $Arguments `
            -WorkingDirectory $WorkingDirectory `
            -WindowStyle Hidden `
            -PassThru `
            -RedirectStandardOutput $stdout `
            -RedirectStandardError $stderr
    }
    finally {
        foreach ($name in $overrides.Keys) {
            [Environment]::SetEnvironmentVariable($name, $previous[$name], 'Process')
        }
    }
    return [pscustomobject]@{
        Process = $process
        Stdout = $stdout
        Stderr = $stderr
    }
}

function Wait-AiosHealth(
    [System.Diagnostics.Process]$Process,
    [string]$Url,
    [string]$ExpectedBuildId,
    [int]$TimeoutSeconds
) {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $lastError = '尚未收到 health 响应'
    do {
        $Process.Refresh()
        if ($Process.HasExited) {
            throw "服务进程提前退出（exit $($Process.ExitCode)）"
        }
        $health = $null
        try {
            $health = Invoke-RestMethod -Uri $Url -TimeoutSec 3
        }
        catch {
            $lastError = $_.Exception.Message
        }
        if ($health) {
            $observedBuildId = [string]$health.build_id
            if (-not $observedBuildId -and [string]$health.mode -eq 'setup') {
                $setupBuilder = New-Object System.UriBuilder -ArgumentList $Url
                $setupBuilder.Path = '/api/v1/setup/state'
                $setupBuilder.Query = ''
                try {
                    $setupState = Invoke-RestMethod -Uri $setupBuilder.Uri.AbsoluteUri -TimeoutSec 3
                    $observedBuildId = [string]$setupState.build_id
                    $health | Add-Member -NotePropertyName build_id -NotePropertyValue $observedBuildId -Force
                    $health | Add-Member -NotePropertyName setup_state -NotePropertyValue $setupState -Force
                }
                catch {
                    $lastError = "setup health 无 build_id，且 setup/state 读取失败：$($_.Exception.Message)"
                    $health = $null
                }
            }
            if ($health -and $observedBuildId -ne $ExpectedBuildId) {
                throw "health 来自错误构建：期望 $ExpectedBuildId，实际 $observedBuildId"
            }
            if ($health) {
                return $health
            }
        }
        Start-Sleep -Seconds 1
    } while ((Get-Date) -lt $deadline)
    throw "服务在 ${TimeoutSeconds}s 内未通过 health：$lastError"
}

function Restore-PatchBackup(
    [string]$Backup,
    [string]$Target,
    [string]$ExpectedSourceHash
) {
    if (-not (Test-Path -LiteralPath $Backup -PathType Leaf)) {
        throw "补丁备份不存在：$Backup"
    }
    $failedTarget = "$Target.failed-$((Get-Date).ToUniversalTime().ToString('yyyyMMddHHmmss'))"
    if (Test-Path -LiteralPath $Target -PathType Leaf) {
        [IO.File]::Replace($Backup, $Target, $failedTarget, $true)
    }
    else {
        Move-Item -LiteralPath $Backup -Destination $Target
        $failedTarget = $null
    }
    $restoredHash = Get-Sha256 $Target
    if ($restoredHash -ne $ExpectedSourceHash) {
        throw "回滚后哈希不对：期望 $ExpectedSourceHash，实际 $restoredHash"
    }
    return [pscustomobject]@{
        RestoredSha256 = $restoredHash
        FailedTarget = $failedTarget
    }
}

function Write-InstallReceipt(
    [string]$Root,
    [System.Collections.IDictionary]$Receipt
) {
    $logDir = Join-Path $Root 'logs'
    New-Item -ItemType Directory -Path $logDir -Force | Out-Null
    $path = Join-Path $logDir "patch-install-$((Get-Date).ToString('yyyyMMdd-HHmmss')).json"
    [IO.File]::WriteAllText(
        $path,
        ($Receipt | ConvertTo-Json -Depth 8),
        [Text.UTF8Encoding]::new($false)
    )
    return $path
}

if (-not $PatchExe) {
    $suffix = '.install.ps1'
    if ($PSCommandPath -and $PSCommandPath.EndsWith($suffix, [StringComparison]::OrdinalIgnoreCase)) {
        $candidate = $PSCommandPath.Substring(0, $PSCommandPath.Length - $suffix.Length)
        if (Test-Path -LiteralPath $candidate -PathType Leaf) { $PatchExe = $candidate }
    }
}
if (-not $PatchExe) {
    $nearby = @(Get-ChildItem -LiteralPath $PSScriptRoot -Filter '*.patch.exe' -File)
    if ($nearby.Count -eq 1) { $PatchExe = $nearby[0].FullName }
}
if (-not $PatchExe) {
    throw '请用 -PatchExe 指定补丁；只有同目录恰好一份 *.patch.exe 时才能省略'
}
$PatchExe = Resolve-ExistingFile $PatchExe '补丁 EXE'

if (-not $TargetExe) {
    $TargetExe = Join-Path (Split-Path -Parent $PatchExe) 'aios-database.exe'
}
$TargetExe = Resolve-ExistingFile $TargetExe '目标 aios-database.exe'
$packageRoot = Split-Path -Parent $TargetExe

if (-not $Workspace) {
    if (Test-Path -LiteralPath (Join-Path $packageRoot 'DbOption.toml') -PathType Leaf) {
        $Workspace = $packageRoot
    }
    elseif (Test-Path -LiteralPath (Join-Path $packageRoot 'workspace\DbOption.toml') -PathType Leaf) {
        $Workspace = Join-Path $packageRoot 'workspace'
    }
    else {
        $Workspace = $packageRoot
    }
}
$Workspace = Resolve-ExistingDirectory $Workspace '运行 workspace'

if (-not $ConfigPath) {
    foreach ($candidate in @(
        (Join-Path $packageRoot 'DbOption.toml'),
        (Join-Path $Workspace 'DbOption.toml')
    )) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            $ConfigPath = $candidate
            break
        }
    }
}
if (-not $ConfigPath) {
    throw "找不到 DbOption.toml：检查 $packageRoot 与 $Workspace，或用 -ConfigPath 指定"
}
$ConfigPath = Resolve-ExistingFile $ConfigPath 'DbOption 配置'
if (-not $HealthUrl) { $HealthUrl = Get-HealthUrl $ConfigPath }

$patchInfo = Invoke-JsonExe $PatchExe @('inspect', '--json') '读取补丁清单'
$patchHash = Get-Sha256 $PatchExe
$checksumPath = "$PatchExe.sha256"
if (Test-Path -LiteralPath $checksumPath -PathType Leaf) {
    $recorded = [regex]::Match(
        [IO.File]::ReadAllText($checksumPath),
        'sha256=([0-9a-fA-F]{64})'
    ).Groups[1].Value.ToLowerInvariant()
    if (-not $recorded -or $recorded -ne $patchHash) {
        throw "补丁旁置校验不匹配：实际 $patchHash，记录 $recorded"
    }
}
elseif (-not $AllowMissingChecksum) {
    throw "缺补丁旁置校验：$checksumPath（确知来源可信时可显式加 -AllowMissingChecksum）"
}

$installedHash = Get-Sha256 $TargetExe
if ($installedHash -ne [string]$patchInfo.source.sha256 -and
    $installedHash -ne [string]$patchInfo.target.sha256) {
    throw "目标既不是补丁源也不是目标：实际 $installedHash；源 $($patchInfo.source.sha256)；目标 $($patchInfo.target.sha256)"
}

$dryRun = Invoke-JsonExe $PatchExe @(
    'apply', '--target', $TargetExe, '--dry-run', '--json'
) '补丁 dry-run'
if ($dryRun.status -ne 'verified' -and $dryRun.status -ne 'already-current') {
    throw "dry-run 返回未知状态：$($dryRun.status)"
}

$runningBefore = @(Get-TargetProcesses $TargetExe)
$wasRunning = $runningBefore.Count -gt 0
$stopped = @(Stop-TargetProcesses $TargetExe)
$backupsBefore = @(
    Get-ChildItem -LiteralPath $packageRoot -Filter 'aios-database.exe.bak-*' -File -ErrorAction SilentlyContinue |
        Select-Object -ExpandProperty FullName
)

$applyReceipt = $null
$started = $null
$receipt = [ordered]@{
    status = 'starting'
    patch = $PatchExe
    patch_sha256 = $patchHash
    target = $TargetExe
    source = $patchInfo.source
    expected_target = $patchInfo.target
    dry_run = $dryRun.status
    stopped_process_ids = @($stopped | Select-Object -ExpandProperty ProcessId)
    health_url = $HealthUrl
    applied = $null
    health = $null
    rollback = $null
    logs = $null
    target_sha256 = $null
    process_id = $null
    failure = $null
    receipt = $null
}

try {
    $applyReceipt = Invoke-JsonExe $PatchExe @(
        'apply', '--target', $TargetExe, '--json'
    ) '应用补丁'
    $receipt.applied = $applyReceipt
    $targetHash = Get-Sha256 $TargetExe
    if ($targetHash -ne [string]$patchInfo.target.sha256) {
        throw "补丁后目标哈希不对：期望 $($patchInfo.target.sha256)，实际 $targetHash"
    }

    $stamp = 'patch-start-' + (Get-Date -Format 'yyyyMMdd-HHmmss')
    $started = Start-AiosDatabase `
        $TargetExe `
        $Workspace `
        $ConfigPath `
        $packageRoot `
        $ServiceArguments `
        $stamp
    $receipt.logs = [ordered]@{
        stdout = $started.Stdout
        stderr = $started.Stderr
    }
    $health = Wait-AiosHealth `
        $started.Process `
        $HealthUrl `
        ([string]$patchInfo.target.build_id) `
        $StartupTimeoutSeconds
    $receipt.health = $health
    $receipt.status = 'succeeded'
    $receipt.target_sha256 = $targetHash
    $receipt.process_id = $started.Process.Id
    $receiptPath = Write-InstallReceipt $packageRoot $receipt
    $receipt.receipt = $receiptPath
    $receipt | ConvertTo-Json -Depth 8
}
catch {
    $failure = $_.Exception.Message
    if ($started -and -not $started.Process.HasExited) {
        Stop-Process -Id $started.Process.Id -Force -ErrorAction SilentlyContinue
        Wait-Process -Id $started.Process.Id -Timeout 10 -ErrorAction SilentlyContinue
    }
    $stopFailure = $null
    try {
        Stop-TargetProcesses $TargetExe | Out-Null
    }
    catch {
        $stopFailure = $_.Exception.Message
    }

    $backup = $null
    if ($applyReceipt -and $applyReceipt.backup) {
        $backup = [string]$applyReceipt.backup
    }
    elseif ($applyReceipt -and $applyReceipt.status -eq 'applied') {
        $backup = Get-ChildItem `
            -LiteralPath $packageRoot `
            -Filter 'aios-database.exe.bak-*' `
            -File `
            -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -notin $backupsBefore } |
            Select-Object -ExpandProperty FullName -First 1
    }

    $rollback = [ordered]@{
        status = 'not-needed'
        backup = $backup
        restored_sha256 = $null
        failed_target = $null
        restart = 'not-requested'
        restart_process_id = $null
        restart_health = $null
        stop_failure = $stopFailure
    }
    if ($backup) {
        try {
            $restored = Restore-PatchBackup `
                $backup `
                $TargetExe `
                ([string]$patchInfo.source.sha256)
            $rollback.status = 'restored'
            $rollback.restored_sha256 = $restored.RestoredSha256
            $rollback.failed_target = $restored.FailedTarget

            if ($wasRunning -and -not $LeaveStoppedOnFailure) {
                try {
                    $restartStamp = 'patch-rollback-' + (Get-Date -Format 'yyyyMMdd-HHmmss')
                    $oldStart = Start-AiosDatabase `
                        $TargetExe `
                        $Workspace `
                        $ConfigPath `
                        $packageRoot `
                        $ServiceArguments `
                        $restartStamp
                    $oldHealth = Wait-AiosHealth `
                        $oldStart.Process `
                        $HealthUrl `
                        ([string]$patchInfo.source.build_id) `
                        $StartupTimeoutSeconds
                    $rollback.restart = 'healthy'
                    $rollback.restart_process_id = $oldStart.Process.Id
                    $rollback.restart_health = $oldHealth
                }
                catch {
                    $rollback.restart = "failed: $($_.Exception.Message)"
                }
            }
        }
        catch {
            $rollback.status = "failed: $($_.Exception.Message)"
        }
    }
    elseif ($applyReceipt -and $applyReceipt.status -eq 'already-current') {
        $rollback.status = 'unavailable-already-current'
    }

    $receipt.status = 'failed'
    $receipt.failure = $failure
    $receipt.applied = $applyReceipt
    $receipt.rollback = $rollback
    $receiptPath = Write-InstallReceipt $packageRoot $receipt
    $receipt.receipt = $receiptPath
    Write-Output ($receipt | ConvertTo-Json -Depth 8)
    throw "补丁安装失败：$failure；回滚=$($rollback.status)；回执=$receiptPath"
}
