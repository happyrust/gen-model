<#
.SYNOPSIS
    Generate one self-contained EXE that patches an earlier aios-database.exe.

.DESCRIPTION
    The generated patch contains:
      - a small patch runner;
      - a JSON manifest naming source/target versions, build IDs, sizes and SHA-256;
      - a bsdiff payload.

    Applying is exact-hash gated and atomic:
      1. the installed EXE must match the recorded source SHA-256;
      2. the patched temporary EXE must match the recorded target SHA-256;
      3. only then is the old EXE renamed to a .bak-<hash> file and replaced.

    Double-clicking the generated EXE patches aios-database.exe beside it.
    The service must be stopped first because Windows locks a running image.

.EXAMPLE
    .\scripts\New-AiosDatabasePatch.ps1 `
      -PreviousExe .\releases\ams-local-0.1.21\aios-database.exe

.EXAMPLE
    .\scripts\New-AiosDatabasePatch.ps1 `
      -PreviousExe D:\release\old\aios-database.exe `
      -NewExe D:\release\new\aios-database.exe `
      -OutDir .\releases\patches -Force
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$PreviousExe,
    [string]$NewExe,
    [string]$RepoRoot,
    [string]$OutDir,
    [string]$Name,
    [string]$BuildId,
    [string]$BackendTargetDir,
    [ValidateRange(0, 9)]
    [int]$CompressionLevel = 1,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

function Resolve-ExistingFile([string]$Path, [string]$Label) {
    $resolved = Resolve-Path -LiteralPath $Path -ErrorAction SilentlyContinue
    if (-not $resolved -or -not (Test-Path -LiteralPath $resolved.Path -PathType Leaf)) {
        throw "$Label 不存在：$Path"
    }
    return $resolved.Path
}

function Get-AiosBuildInfo([string]$Exe) {
    $savedPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $jsonOutput = @(& $Exe version --json 2>&1)
        $jsonExit = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $savedPreference
    }
    if ($jsonExit -eq 0) {
        try {
            $parsed = ($jsonOutput -join "`n") | ConvertFrom-Json
            if ($parsed.product -eq 'aios-database' -and $parsed.version -and $parsed.build_id) {
                return [pscustomobject]@{
                    version = [string]$parsed.version
                    build_id = [string]$parsed.build_id
                    source = 'version --json'
                }
            }
        }
        catch {}
    }

    $plainOutput = @(& $Exe --version 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "$Exe 既不支持 'version --json'，也无法执行 '--version'：$($plainOutput -join ' ')"
    }
    $plain = ($plainOutput -join ' ').Trim()
    $match = [regex]::Match($plain, '(?i)\baios-database\s+([^\s]+)')
    if (-not $match.Success) {
        throw "$Exe 的 --version 输出无法识别：$plain"
    }
    $version = $match.Groups[1].Value
    return [pscustomobject]@{
        version = $version
        build_id = $version
        source = '--version fallback'
    }
}

function Get-TargetDirectory([string]$Root) {
    Push-Location $Root
    try {
        $metadata = @(& cargo metadata --format-version 1 --no-deps 2>&1)
        if ($LASTEXITCODE -ne 0) {
            throw "cargo metadata 失败：$($metadata -join "`n")"
        }
        return [string](($metadata -join "`n") | ConvertFrom-Json).target_directory
    }
    finally {
        Pop-Location
    }
}

function ConvertTo-FileToken([string]$Value) {
    $token = ($Value -replace '[^A-Za-z0-9._-]', '_').Trim('_')
    if ($token.Length -gt 64) { $token = $token.Substring(0, 64) }
    if (-not $token) { return 'unknown' }
    return $token
}

$scriptDir = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
if (-not $RepoRoot) { $RepoRoot = Split-Path -Parent $scriptDir }
$RepoRoot = (Resolve-Path -LiteralPath $RepoRoot).Path
if (-not $OutDir) { $OutDir = Join-Path $RepoRoot 'releases\patches' }
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
$OutDir = (Resolve-Path -LiteralPath $OutDir).Path

$PreviousExe = Resolve-ExistingFile $PreviousExe '上一版 aios-database.exe'
$previousInfo = Get-AiosBuildInfo $PreviousExe

$previousTargetDir = $env:CARGO_TARGET_DIR
try {
    if ($BackendTargetDir) {
        New-Item -ItemType Directory -Path $BackendTargetDir -Force | Out-Null
        $env:CARGO_TARGET_DIR = (Resolve-Path -LiteralPath $BackendTargetDir).Path
    }

    if (-not $NewExe) {
        $cargoVersion = [regex]::Match(
            (Get-Content -LiteralPath (Join-Path $RepoRoot 'Cargo.toml') -Raw),
            '(?m)^\s*version\s*=\s*"([^"]+)"'
        ).Groups[1].Value
        if (-not $cargoVersion) { throw 'Cargo.toml 中没有读到 aios-database 版本' }
        if (-not $BuildId) {
            $BuildId = "$cargoVersion+patch.$((Get-Date).ToUniversalTime().ToString('yyyyMMddHHmmss'))"
        }
        $previousBuildId = $env:AIOS_BUILD_ID
        try {
            $env:AIOS_BUILD_ID = $BuildId
            Push-Location $RepoRoot
            try {
                & cargo build --release --locked --features http_api --bin aios-database
                if ($LASTEXITCODE -ne 0) { throw "aios-database release 构建失败（exit $LASTEXITCODE）" }
            }
            finally {
                Pop-Location
            }
        }
        finally {
            if ($null -eq $previousBuildId) { Remove-Item Env:AIOS_BUILD_ID -ErrorAction SilentlyContinue }
            else { $env:AIOS_BUILD_ID = $previousBuildId }
        }
        $targetDir = Get-TargetDirectory $RepoRoot
        $NewExe = Join-Path $targetDir 'release\aios-database.exe'
    }
    $NewExe = Resolve-ExistingFile $NewExe '新版 aios-database.exe'
    $newInfo = Get-AiosBuildInfo $NewExe

    $oldHash = (Get-FileHash -LiteralPath $PreviousExe -Algorithm SHA256).Hash.ToLowerInvariant()
    $newHash = (Get-FileHash -LiteralPath $NewExe -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($oldHash -eq $newHash) { throw '上一版与新版 SHA-256 相同，不需要生成 patch' }
    if (-not $Name) {
        $Name = 'aios-database-{0}-to-{1}-{2}.patch.exe' -f `
            (ConvertTo-FileToken $previousInfo.build_id), `
            (ConvertTo-FileToken $newInfo.build_id), `
            $newHash.Substring(0, 12)
    }
    if (-not $Name.EndsWith('.exe', [StringComparison]::OrdinalIgnoreCase)) { $Name += '.exe' }
    $output = Join-Path $OutDir $Name
    if ((Test-Path -LiteralPath $output) -and -not $Force) {
        throw "patch 已存在：$output（覆盖请加 -Force）"
    }

    Push-Location $RepoRoot
    try {
        & cargo build --release --locked -p aios-patch --bin aios-database-patch
        if ($LASTEXITCODE -ne 0) { throw "patch 工具构建失败（exit $LASTEXITCODE）" }
    }
    finally {
        Pop-Location
    }
    $targetDir = Get-TargetDirectory $RepoRoot
    $patchTool = Resolve-ExistingFile (Join-Path $targetDir 'release\aios-database-patch.exe') 'patch 工具'
    $arguments = @(
        'create',
        '--source', $PreviousExe,
        '--target', $NewExe,
        '--output', $output,
        '--source-version', $previousInfo.version,
        '--source-build-id', $previousInfo.build_id,
        '--target-version', $newInfo.version,
        '--target-build-id', $newInfo.build_id,
        '--compression-level', $CompressionLevel
    )
    if ($Force) { $arguments += '--force' }
    & $patchTool @arguments
    if ($LASTEXITCODE -ne 0) { throw "patch 生成失败（exit $LASTEXITCODE）" }

    $inspect = @(& $output inspect --json 2>&1)
    if ($LASTEXITCODE -ne 0) { throw "生成物无法自检：$($inspect -join "`n")" }
    $manifest = ($inspect -join "`n") | ConvertFrom-Json
    $patchHash = (Get-FileHash -LiteralPath $output -Algorithm SHA256).Hash.ToLowerInvariant()
    $sidecar = Get-Content -LiteralPath "$output.sha256" -Raw
    $recorded = ([regex]::Match($sidecar, 'sha256=([0-9a-fA-F]{64})')).Groups[1].Value.ToLowerInvariant()
    if ($patchHash -ne $recorded) { throw 'patch EXE 与 .sha256 旁置校验不一致' }
    if ($manifest.source.sha256 -ne $oldHash -or $manifest.target.sha256 -ne $newHash) {
        throw 'patch 自检清单与输入 EXE 哈希不一致'
    }

    # 每一份交付物都在隔离目录里走一遍真实升级，而不只检查 manifest：
    # dry-run 不写盘；双击语义（无参数）完成替换并留备份；第二次应用幂等。
    $verifyDir = Join-Path $env:TEMP ('aios-patch-verify-' + [guid]::NewGuid().ToString('N'))
    $verification = $null
    New-Item -ItemType Directory -Path $verifyDir | Out-Null
    try {
        $installed = Join-Path $verifyDir 'aios-database.exe'
        $runner = Join-Path $verifyDir ([IO.Path]::GetFileName($output))
        Copy-Item -LiteralPath $PreviousExe -Destination $installed
        Copy-Item -LiteralPath $output -Destination $runner

        $dryOutput = @(& $runner apply --dry-run --json 2>&1)
        if ($LASTEXITCODE -ne 0) {
            throw "patch dry-run 验证失败：$($dryOutput -join "`n")"
        }
        $dryReceipt = ($dryOutput -join "`n") | ConvertFrom-Json
        if ($dryReceipt.status -ne 'verified' -or
            (Get-FileHash -LiteralPath $installed -Algorithm SHA256).Hash.ToLowerInvariant() -ne $oldHash) {
            throw 'patch dry-run 回执不对或写动了源 EXE'
        }

        # 无参数就是用户双击时走的路径：patch 同目录 aios-database.exe。
        $applyOutput = @(& $runner 2>&1)
        if ($LASTEXITCODE -ne 0) {
            throw "patch 默认应用验证失败：$($applyOutput -join "`n")"
        }
        $applyReceipt = ($applyOutput -join "`n") | ConvertFrom-Json
        $appliedHash = (Get-FileHash -LiteralPath $installed -Algorithm SHA256).Hash.ToLowerInvariant()
        $backups = @(Get-ChildItem -LiteralPath $verifyDir -Filter 'aios-database.exe.bak-*' -File)
        if ($applyReceipt.status -ne 'applied' -or $appliedHash -ne $newHash -or $backups.Count -ne 1) {
            throw 'patch 默认应用后状态、目标哈希或备份数量不对'
        }
        $backupHash = (Get-FileHash -LiteralPath $backups[0].FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($backupHash -ne $oldHash) { throw 'patch 留下的备份与上一版 EXE 不一致' }

        $secondOutput = @(& $runner apply --json 2>&1)
        if ($LASTEXITCODE -ne 0) {
            throw "patch 二次幂等验证失败：$($secondOutput -join "`n")"
        }
        $secondReceipt = ($secondOutput -join "`n") | ConvertFrom-Json
        if ($secondReceipt.status -ne 'already-current' -or
            @(Get-ChildItem -LiteralPath $verifyDir -Filter 'aios-database.exe.bak-*' -File).Count -ne 1) {
            throw 'patch 二次应用没有保持 already-current 或重复生成了备份'
        }

        $verification = [ordered]@{
            dry_run = $dryReceipt.status
            first_apply = $applyReceipt.status
            second_apply = $secondReceipt.status
            target_hash_match = $true
            backup_hash_match = $true
        }
    }
    finally {
        Remove-Item -LiteralPath $verifyDir -Recurse -Force -ErrorAction SilentlyContinue
    }

    $installerSource = Resolve-ExistingFile `
        (Join-Path $scriptDir 'Install-AiosDatabasePatch.ps1') `
        '自动安装/失败回滚脚本'
    $installer = "$output.install.ps1"
    Copy-Item -LiteralPath $installerSource -Destination $installer -Force
    $installerHash = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()
    $installerChecksum = "$installer.sha256"
    "$([IO.Path]::GetFileName($installer))  sha256=$installerHash  bytes=$((Get-Item -LiteralPath $installer).Length)" |
        Set-Content -LiteralPath $installerChecksum -Encoding ascii

    [pscustomobject]@{
        patch = $output
        bytes = (Get-Item -LiteralPath $output).Length
        sha256 = $patchHash
        source = $manifest.source
        target = $manifest.target
        payload_bytes = $manifest.patch_bytes
        checksum = "$output.sha256"
        manifest = "$output.manifest.json"
        installer = $installer
        installer_sha256 = $installerHash
        installer_checksum = $installerChecksum
        apply = ".\$([IO.Path]::GetFileName($output))"
        verification = $verification
    } | ConvertTo-Json -Depth 6
}
finally {
    if ($null -eq $previousTargetDir) { Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue }
    else { $env:CARGO_TARGET_DIR = $previousTargetDir }
}
