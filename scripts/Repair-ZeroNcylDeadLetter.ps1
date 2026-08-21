<#
.SYNOPSIS
  Orchestrates the paired backup, guarded E3D edit, ADR-021 rebuild and acceptance
  for the empty NCYL 24381/38635. The default is a side-effect-free dry run.

.EXAMPLE
  powershell -File scripts\Repair-ZeroNcylDeadLetter.ps1 -Phase Backup -Execute
  powershell -File scripts\Repair-ZeroNcylDeadLetter.ps1 -Phase Macro -Execute
  powershell -File scripts\Repair-ZeroNcylDeadLetter.ps1 -Phase Rebuild -Execute
  powershell -File scripts\Repair-ZeroNcylDeadLetter.ps1 -Phase Verify -Execute
#>
[CmdletBinding()]
param(
    [ValidateSet('All', 'Backup', 'Macro', 'Rebuild', 'Verify')]
    [string]$Phase = 'All',
    [switch]$Execute,
    [string]$SourceFile = 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7997_0001',
    [string]$ProjectDir = 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample',
    [string]$EvidenceDir = 'docs\evidence\2026-08-21-zero-ncyl-dead-letter-recovery\live',
    [string]$HealthUri = 'http://127.0.0.1:9099/api/v1/health',
    [string]$SurrealEndpoint = 'ws://127.0.0.1:8009',
    [int]$ValidationPort = 18099,
    [string]$ServiceExe = 'D:\work\plant-code\old\test-worklspace\bin\aios-database.exe',
    [string]$ServiceWorkingDirectory = 'D:\work\plant-code\old\test-worklspace\bin',
    [string]$BuiltServiceExe = $(if ($env:CARGO_TARGET_DIR) { Join-Path $env:CARGO_TARGET_DIR 'release\aios-database.exe' } else { 'D:\work\plant-code\old\target\release\aios-database.exe' }),
    [string[]]$OriginalArguments = @()
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$evidence = Join-Path $repo $EvidenceDir
$macro = Join-Path $repo 'scripts\e3d\remove_zero_ncyl_24381_38635.mac'
$surreal = Join-Path $repo 'bin\surreal.exe'
$target = Join-Path (Split-Path -Parent $repo) 'target'
$l3 = Join-Path $target 'debug\l3_suite.exe'
$fixture = Join-Path $target 'debug\db_session_fixture.exe'
$namespace = '1516'
$database = 'AvevaMarineSample'
$user = 'root'
$password = 'root'

function Write-Step([string]$Text) {
    $prefix = if ($Execute) { 'EXEC' } else { 'DRY-RUN' }
    Write-Host "[$prefix] $Text"
}

function Assert-File([string]$Path, [string]$Label) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "$Label 不存在: $Path" }
}

function Get-AiosConsumers {
    @(Get-CimInstance Win32_Process -Filter "Name = 'aios-database.exe'" | Select-Object ProcessId, ExecutablePath, CommandLine)
}

function Get-Health {
    try { Invoke-RestMethod -Method Get -Uri $HealthUri -TimeoutSec 5 }
    catch { [ordered]@{ query_error = $_.Exception.Message } }
}

function Invoke-Sql([string]$Sql, [string]$Endpoint = 'http://127.0.0.1:8009/sql') {
    $pair = "${user}:${password}"
    $auth = [Convert]::ToBase64String([Text.Encoding]::ASCII.GetBytes($pair))
    $headers = @{ Accept = 'application/json'; Authorization = "Basic $auth"; 'Surreal-NS' = $namespace; 'Surreal-DB' = $database }
    (Invoke-WebRequest -Method Post -Uri $Endpoint -Headers $headers -ContentType 'application/surrealql' -UseBasicParsing -Body $Sql).Content
}

function Write-Json([string]$Path, $Value) {
    ConvertTo-Json -InputObject $Value -Depth 20 | Set-Content -LiteralPath $Path -Encoding utf8
}

function Start-CapturedConsumer($Consumer) {
    if (-not $Consumer -or -not $Consumer.ExecutablePath) { return }
    $commandLine = [string]$Consumer.CommandLine
    $prefix = '"' + $Consumer.ExecutablePath + '"'
    $arguments = if ($commandLine.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
        $commandLine.Substring($prefix.Length).Trim()
    } elseif ($commandLine.StartsWith($Consumer.ExecutablePath, [StringComparison]::OrdinalIgnoreCase)) {
        $commandLine.Substring($Consumer.ExecutablePath.Length).Trim()
    } else { '' }
    Start-Process -FilePath $Consumer.ExecutablePath -WorkingDirectory (Split-Path -Parent $Consumer.ExecutablePath) -ArgumentList $arguments -WindowStyle Hidden | Out-Null
}

function Get-Sha256([string]$Path) {
    $stream = [IO.File]::OpenRead($Path)
    try {
        $sha = [Security.Cryptography.SHA256]::Create()
        try { ([BitConverter]::ToString($sha.ComputeHash($stream))).Replace('-', '').ToLowerInvariant() }
        finally { $sha.Dispose() }
    } finally { $stream.Dispose() }
}

function Get-FileRecord([string]$Path) {
    $item = Get-Item -LiteralPath $Path
    [ordered]@{ path = $item.FullName; length = $item.Length; last_write_utc = $item.LastWriteTimeUtc.ToString('o'); sha256 = (Get-Sha256 $Path) }
}

function Assert-MacroDiscipline {
    Assert-File $macro 'E3D 宏'
    $bare = Get-Content -LiteralPath $macro | ForEach-Object { $_.Trim() } | Where-Object { $_ -and -not $_.StartsWith('--') }
    if (@($bare | Where-Object { $_ -match '^SAVEWORK\b' }).Count -ne 1) { throw '守卫宏必须恰好一个 SAVEWORK' }
    if ($bare -match '^(QUIT|FINISH|MERGE|PURGE|COMPACT)\b') { throw '守卫宏包含禁用命令' }
    foreach ($token in @('24381/38635', '24381/38614', "!!ce.type.neq('NCYL')", '!!ce.diam.neq(0)', '!!ce.heig.neq(0)', 'DELETE NCYL', 'CODEX-ZERO-NCYL-GUARD-PASS')) {
        if (-not (($bare -join "`n").Contains($token))) { throw "守卫宏缺少断言/动作: $token" }
    }
}

function Invoke-Backup {
    Write-Step '记录进程、health、7997 水位、死信、队列计数、源文件 header/hash'
    if (-not $Execute) { return }
    New-Item -ItemType Directory -Force $evidence | Out-Null
    Assert-File $SourceFile 'E3D 源文件'; Assert-File $surreal 'SurrealDB 2.1'
    $version = (& $surreal version) -join ' '
    if ($version -notmatch '\b2\.1\.') { throw "SurrealDB 必须为 2.1.x，实得: $version" }
    $consumers = Get-AiosConsumers
    if ($consumers.Count -gt 1) { throw "发现 $($consumers.Count) 个 aios-database 消费者" }
    Write-Json (Join-Path $evidence 'consumer-before.json') $consumers
    Write-Json (Join-Path $evidence 'health-before.json') (Get-Health)
    $sql = @'
SELECT applied_sesno, file_latest_sesno, updated_at FROM dbnum_watermark:7997;
SELECT action, target_refno, noun, attempts, last_error, revision, updated_at FROM model_update_pending WHERE attempts >= 5 ORDER BY updated_at DESC;
SELECT action, attempts < 5 AS retryable, count() AS count FROM model_update_pending GROUP BY action, retryable ORDER BY action;
'@
    Invoke-Sql $sql | Set-Content -LiteralPath (Join-Path $evidence 'surreal-before.json') -Encoding utf8
    if ($consumers.Count -eq 1) {
        Write-Step "停止唯一消费者 PID $($consumers[0].ProcessId)"
        Stop-Process -Id $consumers[0].ProcessId -Force
        Wait-Process -Id $consumers[0].ProcessId -ErrorAction SilentlyContinue
    }
    try { $sourceBefore = Get-FileRecord $SourceFile }
    catch {
        if ($consumers.Count -eq 1) {
            Write-Step '源文件仍被 E3D 锁定；恢复原 aios-database 启动命令'
            Start-CapturedConsumer $consumers[0]
        }
        throw
    }
    Write-Json (Join-Path $evidence 'source-before.json') $sourceBefore
    $header = New-Object byte[] 512
    $stream = [IO.File]::OpenRead($SourceFile)
    try { $read = $stream.Read($header, 0, $header.Length) } finally { $stream.Dispose() }
    [Convert]::ToHexString($header[0..($read - 1)]) | Set-Content -LiteralPath (Join-Path $evidence 'source-header-before.hex')

    $export = Join-Path $evidence 'AvevaMarineSample-1516.surql'
    Write-Step "导出 1516/AvevaMarineSample 到 $export"
    & $surreal export --endpoint $SurrealEndpoint --username $user --password $password --namespace $namespace --database $database $export
    if ($LASTEXITCODE) { throw "Surreal export 失败: $LASTEXITCODE" }
    $exportHash = Get-FileRecord $export

    Write-Step "在 memory://127.0.0.1:$ValidationPort 导入验证"
    $stdout = Join-Path $evidence 'validation-surreal.stdout.log'; $stderr = Join-Path $evidence 'validation-surreal.stderr.log'
    $validation = Start-Process -FilePath $surreal -ArgumentList @('start','--user',$user,'--pass',$password,'--bind',"127.0.0.1:$ValidationPort",'memory') -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    try {
        $ready = $false
        for ($i = 0; $i -lt 40; $i++) {
            try { Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:$ValidationPort/health" -TimeoutSec 1 | Out-Null; $ready = $true; break } catch { Start-Sleep -Milliseconds 250 }
        }
        if (-not $ready) { throw '隔离 SurrealDB 未就绪' }
        & $surreal import --endpoint "ws://127.0.0.1:$ValidationPort" --username $user --password $password --namespace $namespace --database $database $export
        if ($LASTEXITCODE) { throw "Surreal import 验证失败: $LASTEXITCODE" }
        Invoke-Sql $sql "http://127.0.0.1:$ValidationPort/sql" | Set-Content -LiteralPath (Join-Path $evidence 'surreal-import-verified.json') -Encoding utf8
    } finally { Stop-Process -Id $validation.Id -Force -ErrorAction SilentlyContinue }

    $sourceBackup = Join-Path $evidence 'ams7997_0001.before'
    Copy-Item -LiteralPath $SourceFile -Destination $sourceBackup -Force
    $sourceRecord = Get-FileRecord $SourceFile; $backupRecord = Get-FileRecord $sourceBackup
    if ($sourceRecord.sha256 -ne $backupRecord.sha256) { throw 'E3D 文件备份哈希不一致' }
    Write-Json (Join-Path $evidence 'paired-baseline.json') ([ordered]@{ surreal_version = $version; export = $exportHash; source = $sourceRecord; source_backup = $backupRecord; verified_at = (Get-Date).ToUniversalTime().ToString('o') })
    if ((Test-Path -LiteralPath $BuiltServiceExe -PathType Leaf) -and
        -not [IO.Path]::GetFullPath($BuiltServiceExe).Equals([IO.Path]::GetFullPath($ServiceExe), [StringComparison]::OrdinalIgnoreCase)) {
        $deployedBackup = Join-Path $evidence 'aios-database.before.exe'
        Copy-Item -LiteralPath $ServiceExe -Destination $deployedBackup -Force
        Copy-Item -LiteralPath $BuiltServiceExe -Destination $ServiceExe -Force
        Write-Json (Join-Path $evidence 'service-deployment.json') ([ordered]@{
            before = (Get-FileRecord $deployedBackup)
            built = (Get-FileRecord $BuiltServiceExe)
            deployed = (Get-FileRecord $ServiceExe)
        })
        if ((Get-Sha256 $BuiltServiceExe) -ne (Get-Sha256 $ServiceExe)) { throw '部署后的 aios-database 哈希不一致' }
    }
}

function Invoke-Macro {
    Write-Step "通过 l3_suite 单用途会话运行守卫宏 $macro"
    Assert-MacroDiscipline
    if (-not $Execute) { return }
    if ((Get-AiosConsumers).Count) { throw '运行 E3D 宏前必须保持 aios-database 已停止' }
    Assert-File (Join-Path $evidence 'paired-baseline.json') '成对基线清单'
    Assert-File $l3 'l3_suite'
    $before = Get-FileRecord $SourceFile
    & $l3 --check-driver $macro --project-dir $ProjectDir --target-db-file $SourceFile --target-dbnum 7997 --e3d-project AMS --e3d-login SYSTEM/XXXXXX --e3d-mdb /ALL --output (Join-Path $evidence 'e3d-driver')
    if ($LASTEXITCODE) { throw "E3D driver 失败: $LASTEXITCODE" }
    $log = [IO.Path]::ChangeExtension($macro, '.log')
    $text = Get-Content -LiteralPath $log -Raw
    if ($text -notmatch 'CODEX-ZERO-NCYL-GUARD-PASS' -or $text -notmatch 'CODEX-ZERO-NCYL-DELETE-DONE' -or $text -match 'GUARD-FAIL|ABORTED-NO-SAVEWORK') { throw 'E3D 宏守卫/完成哨兵不符合预期' }
    $after = Get-FileRecord $SourceFile
    if ($before.sha256 -eq $after.sha256) { throw 'SAVEWORK 后源文件哈希未变化' }
    Write-Json (Join-Path $evidence 'source-after.json') $after
    Copy-Item -LiteralPath $log -Destination (Join-Path $evidence 'e3d-macro.log') -Force
    if (Test-Path -LiteralPath $fixture) { & $fixture inspect --source $SourceFile | Set-Content -LiteralPath (Join-Path $evidence 'source-session-after.json') -Encoding utf8 }
}

function Invoke-Rebuild {
    Write-Step '以命令行 watch 覆盖 7997,8000 启动，等待 ADR-021 重建后仅复活一次目标模型工作'
    if (-not $Execute) { return }
    if ((Get-AiosConsumers).Count) { throw '重建启动前检测到其他 aios-database 消费者' }
    Assert-File $ServiceExe 'aios-database release binary'
    $stdout = Join-Path $evidence 'rebuild.stdout.log'; $stderr = Join-Path $evidence 'rebuild.stderr.log'
    $proc = Start-Process -FilePath $ServiceExe -WorkingDirectory $ServiceWorkingDirectory -ArgumentList @('serve','--watch-dbnum','7997,8000') -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    Write-Json (Join-Path $evidence 'rebuild-process.json') ([ordered]@{ pid = $proc.Id; exe = $ServiceExe; args = @('serve','--watch-dbnum','7997,8000') })
    $rebuilt = $false
    for ($i = 0; $i -lt 360; $i++) {
        Start-Sleep -Seconds 5
        try {
            $h = Get-Health
            $wm = (Invoke-Sql 'SELECT applied_sesno, file_latest_sesno FROM dbnum_watermark:7997;') | ConvertFrom-Json
            $row = @($wm[-1].result)[0]
            if ($row -and [int]$row.applied_sesno -eq [int]$row.file_latest_sesno -and [int]$row.applied_sesno -gt 0) { $rebuilt = $true; break }
        } catch { }
    }
    if (-not $rebuilt) { throw '7997 在 30 分钟内未完成 ADR-021 重建' }
    $dead = (Invoke-Sql "SELECT action, target_refno, attempts FROM model_update_pending WHERE action = 'regen_root' AND target_refno = '24381/38436' AND attempts >= 5;") | ConvertFrom-Json
    if (@($dead[-1].result).Count) {
        $body = @{ action = 'regen_root'; target_refno = '24381/38436' } | ConvertTo-Json
        Invoke-WebRequest -Method Post -Uri 'http://127.0.0.1:9099/api/v1/update/pending-units/retry' -ContentType 'application/json' -Body $body -UseBasicParsing | Set-Content -LiteralPath (Join-Path $evidence 'retry-once.json') -Encoding utf8
    }
}

function Invoke-Verify {
    Write-Step '等待 model_ready、房间队列归零、health ok；观察 10 分钟后恢复原始启动参数'
    if (-not $Execute) { return }
    $deadline = (Get-Date).AddMinutes(45)
    do {
        $health = Get-Health
        $pending = (Invoke-Sql 'SELECT action, count() AS count FROM model_update_pending GROUP BY action ORDER BY action;') | ConvertFrom-Json
        $rows = @($pending[-1].result)
        $room = @($rows | Where-Object { $_.action -like 'room_recalc_*' } | Measure-Object -Property count -Sum).Sum
        if ($health.status -eq 'ok' -and $health.model_ready -eq $true -and [int]$room -eq 0) { break }
        Start-Sleep -Seconds 10
    } while ((Get-Date) -lt $deadline)
    if ($health.status -ne 'ok' -or $health.model_ready -ne $true -or [int]$room -ne 0) { throw '模型门或房间任务在 45 分钟内未收敛' }
    $start = Get-Date
    $samples = @()
    while ((Get-Date) -lt $start.AddMinutes(10)) {
        $sample = Get-Health
        if ($sample.status -ne 'ok' -or @($sample.blocking_conditions).Count) { throw '10 分钟观察期内 health 再次降级' }
        $samples += $sample
        Start-Sleep -Seconds 30
    }
    Write-Json (Join-Path $evidence 'health-observation.json') $samples
    Invoke-Sql @'
SELECT applied_sesno, file_latest_sesno FROM dbnum_watermark:7997;
SELECT action, target_refno, attempts, last_error FROM model_update_pending WHERE attempts >= 5;
SELECT action, count() AS count FROM model_update_pending GROUP BY action ORDER BY action;
'@ | Set-Content -LiteralPath (Join-Path $evidence 'surreal-after.json') -Encoding utf8
    Write-Json (Join-Path $evidence 'health-after.json') (Get-Health)
    $current = Get-AiosConsumers
    foreach ($p in $current) { Stop-Process -Id $p.ProcessId -Force }
    Start-Process -FilePath $ServiceExe -WorkingDirectory $ServiceWorkingDirectory -ArgumentList $OriginalArguments -WindowStyle Hidden | Out-Null
}

Assert-File $macro '守卫宏'
$selected = if ($Phase -eq 'All') { @('Backup','Macro','Rebuild','Verify') } else { @($Phase) }
foreach ($step in $selected) {
    switch ($step) { 'Backup' { Invoke-Backup }; 'Macro' { Invoke-Macro }; 'Rebuild' { Invoke-Rebuild }; 'Verify' { Invoke-Verify } }
}
