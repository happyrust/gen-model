param(
    [string]$Exe = 'D:\Rust\target\debug\aios-database.exe',
    [int]$Port = 18084
)

$ErrorActionPreference = 'Stop'
$evidenceDir = Split-Path -Parent $PSCommandPath
$repo = (Resolve-Path (Join-Path $evidenceDir '..\..')).Path
$runDir = Join-Path $env:TEMP ('gen-model-mbd-live-' + [guid]::NewGuid().ToString('N'))
$configPath = Join-Path $evidenceDir 'isolated-18084-config.toml'
$stdoutPath = Join-Path $evidenceDir 'isolated-18084-stdout.log'
$stderrPath = Join-Path $evidenceDir 'isolated-18084-stderr.log'
$summaryPath = Join-Path $evidenceDir 'summary.json'
$process = $null
$summary = [ordered]@{
    captured_at = (Get-Date).ToString('o')
    repo = $repo
    git = [ordered]@{}
    executable = [ordered]@{}
    current_8022 = [ordered]@{}
    isolated_18084 = [ordered]@{
        port = $Port
        storage = 'embedded-mem'
        startup_autorun = $false
        room_build_skipped = $true
        responses = [ordered]@{}
    }
}

function Invoke-RawHttp {
    param(
        [string]$Url,
        [string]$OutFile,
        [int]$TimeoutSeconds = 120
    )

    $status = & curl.exe -sS --max-time $TimeoutSeconds -i -o $OutFile -w '%{http_code}' $Url
    if ($LASTEXITCODE -ne 0) {
        throw "curl failed ($LASTEXITCODE): $Url"
    }
    return [int]$status
}

function Get-HttpJson {
    param([string]$Path)

    $raw = [System.IO.File]::ReadAllText($Path)
    $separator = if ($raw.Contains("`r`n`r`n")) { "`r`n`r`n" } else { "`n`n" }
    $index = $raw.LastIndexOf($separator, [System.StringComparison]::Ordinal)
    if ($index -lt 0) {
        throw "HTTP body separator not found: $Path"
    }
    $body = $raw.Substring($index + $separator.Length)
    return $body | ConvertFrom-Json
}

function Set-TomlScalar {
    param(
        [string]$Text,
        [string]$Name,
        [string]$Literal
    )

    $pattern = "(?m)^\s*$([regex]::Escape($Name))\s*=.*$"
    if ([regex]::IsMatch($Text, $pattern)) {
        return [regex]::Replace($Text, $pattern, "$Name = $Literal", 1)
    }
    return $Text.TrimEnd() + "`r`n$Name = $Literal`r`n"
}

try {
    if (-not (Test-Path -LiteralPath $Exe -PathType Leaf)) {
        throw "backend executable not found: $Exe"
    }
    if (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue) {
        throw "isolated port $Port is already occupied"
    }

    $summary.git.branch = (& git -C $repo branch --show-current).Trim()
    $summary.git.revision = (& git -C $repo rev-parse HEAD).Trim()
    $summary.git.dirty = [bool]((& git -C $repo status --porcelain).Length)
    $exeItem = Get-Item -LiteralPath $Exe
    $summary.executable.path = $exeItem.FullName
    $summary.executable.length = $exeItem.Length
    $summary.executable.last_write_time = $exeItem.LastWriteTime.ToString('o')
    $summary.executable.product_version = $exeItem.VersionInfo.ProductVersion
    $summary.executable.sha256 = (Get-FileHash -LiteralPath $Exe -Algorithm SHA256).Hash

    $currentListener = Get-NetTCPConnection -LocalPort 8022 -State Listen -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($currentListener) {
        $currentProcess = Get-CimInstance Win32_Process -Filter "ProcessId=$($currentListener.OwningProcess)"
        $summary.current_8022.pid = $currentListener.OwningProcess
        $summary.current_8022.executable = $currentProcess.ExecutablePath
        $summary.current_8022.command_line = $currentProcess.CommandLine
        if (Test-Path -LiteralPath $currentProcess.ExecutablePath -PathType Leaf) {
            $currentExe = Get-Item -LiteralPath $currentProcess.ExecutablePath
            $summary.current_8022.executable_length = $currentExe.Length
            $summary.current_8022.executable_product_version = $currentExe.VersionInfo.ProductVersion
            $summary.current_8022.executable_sha256 = (
                Get-FileHash -LiteralPath $currentProcess.ExecutablePath -Algorithm SHA256
            ).Hash
        }
        $currentHealthPath = Join-Path $evidenceDir 'current-8022-health.json'
        $currentHealthStatus = & curl.exe -sS --max-time 30 -o $currentHealthPath -w '%{http_code}' `
            'http://127.0.0.1:8022/api/v1/health'
        if ($LASTEXITCODE -ne 0) {
            throw "current 8022 health capture failed: curl=$LASTEXITCODE"
        }
        $summary.current_8022.health_status = [int]$currentHealthStatus
        $currentHealth = Get-Content -LiteralPath $currentHealthPath -Raw | ConvertFrom-Json
        $summary.current_8022.service_status = $currentHealth.status
        $summary.current_8022.version = $currentHealth.version
        $summary.current_8022.build_id = $currentHealth.build_id
        $summary.current_8022.data_read_mode = $currentHealth.data_read_mode
        $summary.current_8022.data_face = $currentHealth.data_face
        $summary.current_8022.in_memory_db = $currentHealth.in_memory_db
        $summary.current_8022.core_status = $currentHealth.core.status
        $summary.current_8022.core_stale_reason = $currentHealth.core.stale_reason
        $summary.current_8022.store_endpoint = $currentHealth.sul_db.endpoint
        $summary.current_8022.store_medium = $currentHealth.sul_db.medium
        $summary.current_8022.mbd_status = Invoke-RawHttp `
            -Url 'http://127.0.0.1:8022/api/mbd/v2/pipe/24383_99996' `
            -OutFile (Join-Path $evidenceDir 'current-8022-24383_99996.http.txt')
        $currentError = Get-HttpJson (Join-Path $evidenceDir 'current-8022-24383_99996.http.txt')
        $summary.current_8022.mbd_error = $currentError.message
    }

    New-Item -ItemType Directory -Path $runDir | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $runDir 'assets') | Out-Null
    if (Test-Path -LiteralPath (Join-Path $repo 'resource')) {
        Copy-Item -LiteralPath (Join-Path $repo 'resource') -Destination $runDir -Recurse
    }

    $config = [System.IO.File]::ReadAllText((Join-Path $repo 'DbOption.toml'))
    $config = Set-TomlScalar $config 'http_api_addr' "`"127.0.0.1:$Port`""
    $config = Set-TomlScalar $config 'server_release_ip' "`"127.0.0.1:$Port`""
    $config = Set-TomlScalar $config 'startup_autorun' 'false'
    $config = Set-TomlScalar $config 'auto_initialize_new_dbnums' 'false'
    $config = Set-TomlScalar $config 'room_membership' 'false'
    $config = Set-TomlScalar $config 'store_mode' '"embedded-mem"'
    $config = Set-TomlScalar $config 'data_face' '"read-through"'
    [System.IO.File]::WriteAllText($configPath, $config, [System.Text.UTF8Encoding]::new($false))

    $env:DB_OPTION_FILE = $configPath
    $env:AIOS_STORE_MODE = 'embedded-mem'
    $env:AIOS_DATA_FACE = 'read-through'
    $env:AIOS_STARTUP_AUTORUN = '0'
    $env:AIOS_AUTO_INITIALIZE_NEW_DBNUMS = '0'
    $env:AIOS_SKIP_STARTUP_ROOM_BUILD = '1'
    $env:AIOS_ROOM_MEMBERSHIP = '0'
    $env:PLANT_ASSET_ROOT = Join-Path $runDir 'assets'
    $env:RUST_MIN_STACK = '134217728'

    $process = Start-Process `
        -FilePath $Exe `
        -ArgumentList 'serve' `
        -WorkingDirectory $runDir `
        -RedirectStandardOutput $stdoutPath `
        -RedirectStandardError $stderrPath `
        -WindowStyle Hidden `
        -PassThru
    $summary.isolated_18084.pid = $process.Id

    $healthUrl = "http://127.0.0.1:$Port/api/v1/health"
    $healthReady = $false
    $deadline = (Get-Date).AddSeconds(120)
    while ((Get-Date) -lt $deadline) {
        if ($process.HasExited) {
            throw "isolated backend exited early with code $($process.ExitCode)"
        }
        try {
            $probe = Invoke-WebRequest -UseBasicParsing -Uri $healthUrl -TimeoutSec 3
            if ($probe.StatusCode -eq 200) {
                $healthReady = $true
                break
            }
        } catch {
            Start-Sleep -Milliseconds 500
        }
    }
    if (-not $healthReady) {
        throw 'isolated backend did not become healthy within 120 seconds'
    }

    $healthPath = Join-Path $evidenceDir 'isolated-18084-health.json'
    $healthStatus = & curl.exe -sS --max-time 30 -o $healthPath -w '%{http_code}' $healthUrl
    if ($LASTEXITCODE -ne 0 -or [int]$healthStatus -ne 200) {
        throw "isolated health capture failed: curl=$LASTEXITCODE status=$healthStatus"
    }
    $health = Get-Content -LiteralPath $healthPath -Raw | ConvertFrom-Json
    $summary.isolated_18084.health = [ordered]@{
        status = 200
        service_status = $health.status
        project = $health.project
        mdb = $health.mdb
        namespace = $health.namespace
        build_id = $health.build_id
        version = $health.version
        data_read_mode = $health.data_read_mode
        data_face = $health.data_face
        in_memory_db = $health.in_memory_db
        session_vector = $health.core.session_vector
    }

    $cases = @(
        [pscustomobject]@{ Name = '24383_99996'; Path = '/api/mbd/v2/pipe/24383_99996'; File = 'isolated-18084-24383_99996.http.txt'; Expected = 200 },
        [pscustomobject]@{ Name = '24381_145018'; Path = '/api/mbd/v2/pipe/24381_145018'; File = 'isolated-18084-24381_145018.http.txt'; Expected = 200 },
        [pscustomobject]@{ Name = '24384_26229'; Path = '/api/mbd/v2/pipe/24384_26229'; File = 'isolated-18084-24384_26229.http.txt'; Expected = 200 },
        [pscustomobject]@{ Name = 'bad_refno'; Path = '/api/mbd/v2/pipe/not-a-refno'; File = 'isolated-18084-bad-refno.http.txt'; Expected = 400 },
        [pscustomobject]@{ Name = 'missing'; Path = '/api/mbd/v2/pipe/24383_999999999'; File = 'isolated-18084-missing.http.txt'; Expected = 404 },
        [pscustomobject]@{ Name = 'non_route'; Path = '/api/mbd/v2/pipe/24383_99997'; File = 'isolated-18084-non-route.http.txt'; Expected = 422 }
    )

    foreach ($case in $cases) {
        $outFile = Join-Path $evidenceDir $case.File
        $status = Invoke-RawHttp -Url ("http://127.0.0.1:$Port" + $case.Path) -OutFile $outFile
        $body = Get-HttpJson $outFile
        $entry = [ordered]@{
            status = $status
            expected_status = $case.Expected
            matches_expected = ($status -eq $case.Expected)
            raw = $case.File
        }
        if ($status -eq 200) {
            $kindCounts = [ordered]@{}
            foreach ($primitive in $body.primitives) {
                $kind = [string]$primitive.kind
                if (-not $kindCounts.Contains($kind)) { $kindCounts[$kind] = 0 }
                $kindCounts[$kind]++
            }
            $entry.version = $body.version
            $entry.input_refno = $body.input_refno
            $entry.branch_refno = $body.branch_refno
            $entry.primitive_count = @($body.primitives).Count
            $entry.primitive_kinds = $kindCounts
            $entry.issue_count = @($body.issues).Count
            $entry.error_issue_count = @($body.issues | Where-Object severity -eq 'error').Count
            $entry.layout_mode = $body.meta.layout_mode
            $entry.geometry_space = $body.meta.geometry_space
            $entry.source_to_design_length = @($body.meta.source_to_design).Count
            $entry.notes = $body.meta.notes
        } else {
            $entry.code = $body.code
            $entry.message = $body.message
            $entry.detail = $body.detail
        }
        $summary.isolated_18084.responses[$case.Name] = $entry
    }
} catch {
    $summary.failure = $_.Exception.Message
    throw
} finally {
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
        Wait-Process -Id $process.Id -ErrorAction SilentlyContinue
    }
    if (Test-Path -LiteralPath $runDir) {
        Remove-Item -LiteralPath $runDir -Recurse -Force -ErrorAction SilentlyContinue
    }
    [System.IO.File]::WriteAllText(
        $summaryPath,
        ($summary | ConvertTo-Json -Depth 12),
        [System.Text.UTF8Encoding]::new($false)
    )
}

$hashFiles = Get-ChildItem -LiteralPath $evidenceDir -File |
    Where-Object Name -ne 'SHA256SUMS.txt' |
    Sort-Object Name
$hashLines = foreach ($file in $hashFiles) {
    '{0}  {1}' -f (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash, $file.Name
}
[System.IO.File]::WriteAllLines(
    (Join-Path $evidenceDir 'SHA256SUMS.txt'),
    $hashLines,
    [System.Text.UTF8Encoding]::new($false)
)

if ($summary.failure) {
    exit 1
}

$summary | ConvertTo-Json -Depth 12
