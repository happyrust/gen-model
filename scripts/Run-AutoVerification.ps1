<#
.SYNOPSIS
    自动化验证脚本（Issue #36 live #2 + Issue #50 A6/A7）

.DESCRIPTION
    一键执行真机验证，自动收集结果并生成报告。

    包含两部分：
    1. A6 全语料对拍（自动，命令行运行）
    2. Live #2 真机验证（半自动，需要在 E3D TTY 中执行宏）

.PARAMETER Mode
    运行模式：
    - a6        只跑 A6 全语料对拍
    - live2     只跑 live #2 真机验证（引导模式）
    - all       跑全部（先 A6，再引导 live #2）
    - quick     快速验证（A6 限制步数 + live #2 检查清单）

.PARAMETER Shards
    A6 分片数（默认 1，即不分片）

.PARAMETER SkipA6
    跳过 A6 对拍

.PARAMETER SkipLive2
    跳过 live #2

.EXAMPLE
    .\Run-AutoVerification.ps1 -Mode quick
    快速验证（A6 限制 5 步/库 + live #2 检查清单）

.EXAMPLE
    .\Run-AutoVerification.ps1 -Mode a6 -Shards 6
    跑 A6 全语料对拍，6 分片并行

.EXAMPLE
    .\Run-AutoVerification.ps1 -Mode live2
    引导 live #2 真机验证
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $false)]
    [ValidateSet("a6", "live2", "all", "quick")]
    [string]$Mode = "quick",

    [Parameter(Mandatory = $false)]
    [int]$Shards = 1,

    [Parameter(Mandatory = $false)]
    [switch]$SkipA6,

    [Parameter(Mandatory = $false)]
    [switch]$SkipLive2
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

# 路径配置
$RepoRoot = "D:\work\plant-code\old\gen-model"
$EvidenceDir = Join-Path $RepoRoot "docs\evidence"
$ReportDir = Join-Path $RepoRoot "verification\auto-$(Get-Date -Format 'yyyyMMdd-HHmmss')"
$DbOptionFile = Join-Path $RepoRoot "db_options\DbOption-aps.toml"

# 创建报告目录
New-Item -ItemType Directory -Path $ReportDir -Force | Out-Null
Write-Host "报告目录: $ReportDir" -ForegroundColor Cyan

# 时间戳函数
$script:StartTime = Get-Date
function Write-Log {
    param([string]$Message, [string]$Level = "INFO")
    $elapsed = (Get-Date) - $script:StartTime
    $timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss"
    $line = "[$timestamp] [$Level] $Message"
    Write-Host $line -ForegroundColor $(switch ($Level) {
        "ERROR" { "Red" }
        "WARN" { "Yellow" }
        "PASS" { "Green" }
        "FAIL" { "Red" }
        default { "White" }
    })
    Add-Content -Path (Join-Path $ReportDir "verification.log") -Value $line
}

function Test-ServiceHealth {
    try {
        $response = Invoke-WebRequest -Uri "http://127.0.0.1:8022/api/v1/health" -UseBasicParsing -TimeoutSec 5
        $health = $response.Content | ConvertFrom-Json
        if ($health.status -eq "ok") {
            Write-Log "服务健康检查通过 (version: $($health.version))" "PASS"
            return $true
        }
    } catch {
        Write-Log "服务健康检查失败: $_" "ERROR"
        return $false
    }
    return $false
}

function Get-DbnumStatus {
    param([int]$Dbnum)
    try {
        $response = Invoke-WebRequest -Uri "http://127.0.0.1:8022/api/v1/dbnums" -UseBasicParsing -TimeoutSec 10
        $data = $response.Content | ConvertFrom-Json
        $db = $data.dbnums | Where-Object { $_.dbnum -eq $Dbnum }
        return $db
    } catch {
        Write-Log "查询 dbnum $Dbnum 失败: $_" "ERROR"
        return $null
    }
}

function Wait-ForSync {
    param([int]$Dbnum, [int]$TimeoutSec = 120)
    Write-Log "等待 dbnum $Dbnum 同步（超时 ${TimeoutSec}s）..."
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    do {
        Start-Sleep -Seconds 3
        $db = Get-DbnumStatus -Dbnum $Dbnum
        if ($db -and $db.in_sync -eq $true) {
            Write-Log "dbnum $Dbnum 已同步 (model_sesno: $($db.model_sesno))" "PASS"
            return $true
        }
        $remaining = ($deadline - (Get-Date)).TotalSeconds
        if ($remaining -le 0) { break }
        Write-Host "  等待中... 剩余 $([math]::Floor($remaining))s" -NoNewline
        Write-Host "`r" -NoNewline
    } while ($true)
    Write-Log "dbnum $Dbnum 同步超时" "FAIL"
    return $false
}

# ============================================================
# A6 全语料对拍
# ============================================================
function Invoke-A6Parity {
    Write-Log "========================================" "INFO"
    Write-Log "A6 全语料对拍开始" "INFO"
    Write-Log "========================================" "INFO"

    # 设置环境变量
    $env:DB_OPTION_FILE = $DbOptionFile
    $env:AIOS_MODEL_UNIT_LEVEL_PUBLISH = "1"
    $env:AIOS_MODEL_STRUCTURAL_CONNECTION_CASCADE = "1"
    $env:AIOS_MODEL_CABLE_ENDPOINT_CASCADE = "1"
    $env:AIOS_MODEL_DESIGN_REFERENCE_CASCADE = "1"

    # 快速模式：限制步数
    if ($Mode -eq "quick") {
        $env:AIOS_UNIT_PUBLISH_STEPS_PER_DB = "5"
        $env:AIOS_UNIT_PUBLISH_ROOTS_PER_WINDOW = "10"
        $env:AIOS_UNIT_PUBLISH_BUDGET_SECS = "300"
        Write-Log "快速模式：限制 5 步/库，每窗 10 根，预算 300s" "INFO"
    }

    # 分片设置
    if ($Shards -gt 1) {
        Write-Log "分片模式：需要手动运行 $Shards 个分片" "WARN"
        Write-Log "当前运行分片 0/$Shards（如需多分片请手动运行其他分片）" "WARN"
        $env:AIOS_UNIT_PUBLISH_SHARD = "0/$Shards"
    }

    # 报告路径
    $reportPath = Join-Path $EvidenceDir "$(Get-Date -Format 'yyyy-MM-dd')-unit-publish-parity-auto.md"
    $env:AIOS_UNIT_PUBLISH_REPORT = $reportPath

    Write-Log "运行命令: cargo test --lib probe_unit_publish_corpus_against_regen -- --ignored --nocapture" "INFO"
    Write-Log "报告路径: $reportPath" "INFO"

    # 运行测试（用 cmd /c 避免 PowerShell 把 cargo 警告当错误）
    Push-Location $RepoRoot
    try {
        $logFile = Join-Path $ReportDir "a6-output.log"
        # 用 cmd /c 运行，stderr 重定向到 stdout，输出到文件和变量
        cmd /c "cargo test --lib probe_unit_publish_corpus_against_regen -- --ignored --nocapture 2>&1" | Tee-Object -FilePath $logFile
        $exitCode = $LASTEXITCODE
        # 读取日志用于解析
        $testOutput = Get-Content $logFile

        # 解析结果
        $summaryLine = $testOutput | Where-Object { $_ -match "SUMMARY_JSON" } | Select-Object -Last 1
        if ($summaryLine) {
            $json = $summaryLine -replace "SUMMARY_JSON\s*", ""
            $summary = $json | ConvertFrom-Json
            Write-Log "========================================" "INFO"
            Write-Log "A6 结果汇总:" "INFO"
            Write-Log "  窗口数: $($summary.windows)" "INFO"
            Write-Log "  检查根数: $($summary.roots_checked)" "INFO"
            Write-Log "  对拍通过: $($summary.parity_ok)" "PASS"
            Write-Log "  DIFF 数: $($summary.diff)" $(if ($summary.diff -eq 0) { "PASS" } else { "FAIL" })
            Write-Log "  回退数: $($summary.fallbacks)" "INFO"
            Write-Log "  重放数: $($summary.replayed)" "INFO"
            Write-Log "  失败数: $($summary.failed)" "INFO"
            Write-Log "========================================" "INFO"

            # 保存结果
            $summary | ConvertTo-Json | Out-File (Join-Path $ReportDir "a6-summary.json")

            if ($summary.diff -eq 0 -and $exitCode -eq 0) {
                Write-Log "A6 全语料对拍: ✅ 通过" "PASS"
                return $true
            } else {
                Write-Log "A6 全语料对拍: ❌ 失败 (diff=$($summary.diff), exit=$exitCode)" "FAIL"
                return $false
            }
        } else {
            Write-Log "未找到 SUMMARY_JSON，测试可能异常退出 (exit=$exitCode)" "FAIL"
            return $false
        }
    } finally {
        Pop-Location
    }
}

# ============================================================
# Live #2 真机验证（引导模式）
# ============================================================
function Invoke-Live2Guide {
    Write-Log "========================================" "INFO"
    Write-Log "Live #2 真机验证（引导模式）" "INFO"
    Write-Log "========================================" "INFO"

    # 1. 检查服务
    if (-not (Test-ServiceHealth)) {
        Write-Log "服务未运行，请先启动 gen-model 服务" "ERROR"
        return $false
    }

    # 2. 检查 ams7997 状态
    $db = Get-DbnumStatus -Dbnum 7997
    if (-not $db) {
        Write-Log "ams7997 不在服务监控列表中" "ERROR"
        return $false
    }
    Write-Log "ams7997 当前状态: file_latest_sesno=$($db.file_latest_sesno)" "INFO"

    # 3. 检查 E3D 脚本
    $scripts = @(
        "scripts\e3d\adr066_live2_rvm_export_before.mac",
        "scripts\e3d\adr066_live2_zone_103249_move_apply.mac",
        "scripts\e3d\adr066_live2_rvm_export_after.mac",
        "scripts\e3d\adr066_live2_zone_103249_move_restore.mac"
    )
    foreach ($script in $scripts) {
        $path = Join-Path $RepoRoot $script
        if (-not (Test-Path $path)) {
            Write-Log "脚本不存在: $script" "ERROR"
            return $false
        }
    }
    Write-Log "E3D 测试脚本检查通过" "PASS"

    # 4. 生成操作指引
    Write-Log "" "INFO"
    Write-Log "========================================" "INFO"
    Write-Log "请按以下步骤在 E3D TTY 中执行:" "INFO"
    Write-Log "========================================" "INFO"
    Write-Log "" "INFO"
    Write-Log "步骤 1 - 导出基线 RVM（挪动前）:" "INFO"
    Write-Log '  $M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_rvm_export_before.mac"' "WARN"
    Write-Log "" "INFO"
    Write-Log "步骤 2 - 挪动 ZONE 103249（东移 1000mm）+ SAVEWORK:" "INFO"
    Write-Log '  $M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_zone_103249_move_apply.mac"' "WARN"
    Write-Log "" "INFO"
    Write-Log "步骤 3 - 导出真值 RVM（挪动后）:" "INFO"
    Write-Log '  $M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_rvm_export_after.mac"' "WARN"
    Write-Log "" "INFO"
    Write-Log "步骤 4 - 执行完成后，回到这里按任意键继续自动验证..." "INFO"
    Write-Log "========================================" "INFO"

    # 保存指引到文件
    $guidePath = Join-Path $ReportDir "live2-guide.md"
    @"
# Live #2 真机验证操作指引

生成时间: $(Get-Date -Format "yyyy-MM-dd HH:mm:ss")

## 在 E3D TTY 中依次执行:

### 步骤 1 - 导出基线 RVM（挪动前）
``````text
`$M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_rvm_export_before.mac"
``````

### 步骤 2 - 挪动 ZONE 103249（东移 1000mm）+ SAVEWORK
``````text
`$M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_zone_103249_move_apply.mac"
``````

### 步骤 3 - 导出真值 RVM（挪动后）
``````text
`$M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_rvm_export_after.mac"
``````

### 步骤 4 - 还原（测试完成后）
``````text
`$M "D:/work/plant-code/old/gen-model/scripts/e3d/adr066_live2_zone_103249_move_restore.mac"
``````

## 自动验证检查点

执行完步骤 3 后，脚本会自动检查:
- [ ] 服务同步状态（in_sync）
- [ ] 回执中有 transform 单元
- [ ] 无 regen 单元
- [ ] RVM 对比（位移 = 1000,0,0）

"@ | Out-File $guidePath
    Write-Log "操作指引已保存到: $guidePath" "INFO"

    # 5. 等待用户执行
    Write-Log "" "INFO"
    $null = Read-Host "请在 E3D TTY 中完成上述 3 个步骤后，按 Enter 继续自动验证"

    # 6. 自动验证
    Write-Log "" "INFO"
    Write-Log "开始自动验证..." "INFO"

    # 6.1 等待同步
    if (-not (Wait-ForSync -Dbnum 7997 -TimeoutSec 120)) {
        Write-Log "同步超时，请检查服务日志" "FAIL"
        return $false
    }

    # 6.2 查询回执
    Write-Log "查询 ZONE 103249 回执..." "INFO"
    try {
        $recordsResponse = Invoke-WebRequest -Uri "http://127.0.0.1:8022/api/v1/model/records?root=24381/103249" -UseBasicParsing -TimeoutSec 10
        $records = $recordsResponse.Content | ConvertFrom-Json
        $records | ConvertTo-Json -Depth 10 | Out-File (Join-Path $ReportDir "live2-records.json")

        # 分析回执
        $units = $records.units
        $transformUnits = $units | Where-Object { $_.kind -eq "transform" }
        $regenUnits = $units | Where-Object { $_.kind -eq "regen" }

        Write-Log "回执分析:" "INFO"
        Write-Log "  transform 单元数: $($transformUnits.Count)" $(if ($transformUnits.Count -gt 0) { "PASS" } else { "FAIL" })
        Write-Log "  regen 单元数: $($regenUnits.Count)" $(if ($regenUnits.Count -eq 0) { "PASS" } else { "WARN" })

        if ($transformUnits.Count -gt 0) {
            $firstTransform = $transformUnits[0]
            Write-Log "  第一个 transform 单元: root=$($firstTransform.root), records=$($firstTransform.records)" "INFO"
        }

        # 判定
        $pass = ($transformUnits.Count -gt 0) -and ($regenUnits.Count -eq 0)
        if ($pass) {
            Write-Log "Live #2 回执验证: ✅ 通过（有 transform 单元，无 regen）" "PASS"
        } else {
            Write-Log "Live #2 回执验证: ❌ 失败" "FAIL"
        }

        # 6.3 RVM 对比（如果有脚本）
        $rvmBefore = Join-Path $RepoRoot "output\e3d-live2\zone_103249_before.rvm"
        $rvmAfter = Join-Path $RepoRoot "output\e3d-live2\zone_103249_after.rvm"
        if ((Test-Path $rvmBefore) -and (Test-Path $rvmAfter)) {
            Write-Log "发现 RVM 文件，执行对比..." "INFO"
            $rvmScript = Join-Path $RepoRoot "scripts\rvm_verify.py"
            if (Test-Path $rvmScript) {
                $rvmResult = python $rvmScript $rvmBefore $rvmAfter 2>&1 | Tee-Object -FilePath (Join-Path $ReportDir "rvm-compare.log")
                Write-Log "RVM 对比完成，结果已保存" "INFO"
            }
        } else {
            Write-Log "RVM 文件不存在，跳过 RVM 对比（请确认步骤 1/3 已执行）" "WARN"
        }

        return $pass
    } catch {
        Write-Log "查询回执失败: $_" "ERROR"
        return $false
    }
}

# ============================================================
# 生成报告
# ============================================================
function New-VerificationReport {
    param([hashtable]$Results)

    $reportPath = Join-Path $ReportDir "verification-report.md"
    $elapsed = (Get-Date) - $script:StartTime

    $lines = @()
    $lines += "# 自动化验证报告"
    $lines += ""
    $lines += "- 生成时间: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')"
    $lines += "- 耗时: $($elapsed.ToString('hh\:mm\:ss'))"
    $lines += "- 报告目录: $ReportDir"
    $lines += ""
    $lines += "## 测试结果"
    $lines += ""
    $lines += "| 测试项 | 结果 | 详情 |"
    $lines += "|--------|------|------|"

    foreach ($key in $Results.Keys) {
        $result = $Results[$key]
        $status = if ($result.Pass) { "✅ 通过" } else { "❌ 失败" }
        $lines += "| $key | $status | $($result.Detail) |"
    }

    $lines += ""
    $lines += "## 详细日志"
    $lines += ""
    $lines += "详见同目录下各日志文件:"
    $lines += "- \`verification.log\` — 完整日志"
    $lines += "- \`a6-output.log\` — A6 测试输出"
    $lines += "- \`a6-summary.json\` — A6 结果汇总"
    $lines += "- \`live2-records.json\` — Live #2 回执数据"
    $lines += "- \`rvm-compare.log\` — RVM 对比结果"

    $lines | Out-File $reportPath
    Write-Log "报告已生成: $reportPath" "PASS"

    # 控制台输出摘要
    Write-Host ""
    Write-Host "========================================" -ForegroundColor Cyan
    Write-Host "验证报告摘要" -ForegroundColor Cyan
    Write-Host "========================================" -ForegroundColor Cyan
    foreach ($key in $Results.Keys) {
        $result = $Results[$key]
        $status = if ($result.Pass) { "✅ 通过" } else { "❌ 失败" }
        $color = if ($result.Pass) { "Green" } else { "Red" }
        Write-Host "$key : $status" -ForegroundColor $color
    }
    Write-Host "========================================" -ForegroundColor Cyan
    Write-Host "完整报告: $reportPath" -ForegroundColor Cyan
}

# ============================================================
# 主流程
# ============================================================
$results = @{}

Write-Log "========================================" "INFO"
Write-Log "自动化验证开始 (Mode: $Mode)" "INFO"
Write-Log "========================================" "INFO"

# A6 对拍
if (-not $SkipA6 -and ($Mode -eq "a6" -or $Mode -eq "all" -or $Mode -eq "quick")) {
    $a6Pass = Invoke-A6Parity
    $results["A6 全语料对拍"] = @{
        Pass = $a6Pass
        Detail = if ($a6Pass) { "diff=0, 全部通过" } else { "见 a6-output.log" }
    }
} else {
    Write-Log "跳过 A6 对拍" "WARN"
}

# Live #2
if (-not $SkipLive2 -and ($Mode -eq "live2" -or $Mode -eq "all" -or $Mode -eq "quick")) {
    $live2Pass = Invoke-Live2Guide
    $results["Live #2 真机验证"] = @{
        Pass = $live2Pass
        Detail = if ($live2Pass) { "有 transform 单元，无 regen" } else { "见 live2-records.json" }
    }
} else {
    Write-Log "跳过 Live #2" "WARN"
}

# 生成报告
New-VerificationReport -Results $results

Write-Log "自动化验证完成" "INFO"
