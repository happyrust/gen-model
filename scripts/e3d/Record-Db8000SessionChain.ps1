<#
.SYNOPSIS
    阶段二录制：在真实 E3D 上按案例清单逐条造变更，产出 recording.json 供
    `db_session_fixture pack` 打成便携夹具。

.DESCRIPTION
    方案 docs/plans/2026-08-12-db8000-session-snapshot-fixture-test-plan.md §2。

    录制是一次性的、要占生产空窗，所以每一步都当场验，不把错误留到事后：

      * 触碰 E3D 之前先审全部宏（恰好一个 SAVEWORK、无 QUIT/FINISH、Q REF 与
        Q NAME 成对），一条不合规就整轮拒绝——宏写错的代价是重录一轮；
      * 每执行一条腿就读一次会话链，要求 sesno **恰好 +1**。多一个 SAVEWORK
        会让后续所有案例的窗口错位，事后从 sesno 反推是猜；
      * refno 从宏日志里的 `Ref =` / `Name /` 相邻对回读，不靠人抄。

    投递通道走 ADR-019 采纳的 `l3_suite --check-driver`：driver 自己拥有会话
    wrapper（L3-ALIVE / 场景宏 / L3-DONE / QUIT）与定向清理，本脚本不直接起
    des.exe。

    录制期间禁止对目标库做 MERGE / PURGE / 数据库压缩——会话链的 append-only
    假设是整套切割机制的地基，压缩过的文件切不出历史。

.EXAMPLE
    powershell -File scripts\e3d\Record-Db8000SessionChain.ps1 `
      -TargetDbFile 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001' `
      -ProjectDir  'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample'
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$TargetDbFile,
    [Parameter(Mandatory)][string]$ProjectDir,
    [int]$Dbnum = 8000,
    [string]$CaseManifest = 'scripts/e3d/db8000_recording_cases.json',
    [string]$Output = "output/db8000-recording/$(Get-Date -Format yyyyMMdd-HHmmss)",
    [string]$E3dProject = 'AMS',
    [string]$E3dLogin = 'SYSTEM/XXXXXX',
    [string]$E3dMdb = '/ALL',
    # 只审宏与前置校验，不登录 E3D、不改库。开生产空窗之前先跑这一档。
    [switch]$CheckOnly
)

$ErrorActionPreference = 'Stop'
# 本脚本在 scripts/e3d/ 下，比 scripts/ 里的编排脚本多一层。
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Push-Location $repo
try {
    $out = Join-Path $repo $Output
    New-Item -ItemType Directory -Force $out | Out-Null
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
    $fixtureExe = Join-Path $target 'debug/db_session_fixture.exe'
    $l3Exe = Join-Path $target 'debug/l3_suite.exe'

    function Assert-Ok([string]$what) {
        if ($LASTEXITCODE) { throw "$what failed (exit $LASTEXITCODE)" }
    }

    # 会话链探针：与阶段一切割用的是同一份解析，所以这里读到的 sesno 就是
    # 将来 pack 能切出来的那个。
    function Get-Sesno {
        $json = & $fixtureExe inspect --source $TargetDbFile
        Assert-Ok 'db_session_fixture inspect'
        return ([int]($json | ConvertFrom-Json).latest_sesno)
    }

    # 宏纪律审查。E3D 侧的错误极难事后归因（日志要 ALPHA LOG END 才落盘），
    # 所以能静态判的一律静态判。
    function Test-MacroDiscipline([string]$relative) {
        $path = Join-Path $repo $relative
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "宏不存在: $relative" }
        $lines = Get-Content -LiteralPath $path
        $bare = $lines | ForEach-Object { $_.Trim() } | Where-Object { $_ -and -not $_.StartsWith('--') }

        $saves = @($bare | Where-Object { $_ -match '^SAVEWORK\b' -or $_ -match '^SAVE\s+WORK\b' })
        if ($saves.Count -ne 1) {
            throw "$relative 必须恰好有一个 SAVEWORK（实得 $($saves.Count)）——多一个就让后续案例窗口整体错位"
        }
        $fatal = @($bare | Where-Object { $_ -match '^(QUIT|FINISH)\b' })
        if ($fatal.Count) { throw "$relative 不得自带 QUIT/FINISH：会话由 driver 拥有" }
        $forbidden = @($bare | Where-Object { $_ -match '^(MERGE|PURGE|COMPACT)\b' })
        if ($forbidden.Count) { throw "$relative 含 MERGE/PURGE/COMPACT：会破坏会话链的 append-only 假设" }

        $opens = @($bare | Where-Object { $_ -match '^ALPHA\s+LOG\s+"' }).Count
        $closes = @($bare | Where-Object { $_ -match '^ALPHA\s+LOG\s+END\b' }).Count
        if ($opens -lt 1 -or $opens -ne $closes) {
            throw "$relative 的 ALPHA LOG 未成对（open=$opens close=$closes）：不闭合则整段日志不落盘，refno 回读不到"
        }
        if (-not ($bare | Where-Object { $_ -match '^Q\s+REF\b' })) {
            throw "$relative 缺 Q REF：脚本靠 Ref/Name 相邻对把名字解析成 refno"
        }
        if (-not ($bare | Where-Object { $_ -match '^Q\s+NAME\b' })) {
            throw "$relative 缺 Q NAME：同上"
        }
        return $path
    }

    # 从宏日志里回读 name -> refno。宏的 `Q REF` / `Q NAME` 是相邻输出，
    # 形如 `Ref =24384/26194` 紧跟 `Name /CODEX_DB8000_EQ_ADD_BOX`。
    function Read-RefnoMap([string]$macroPath) {
        $log = [IO.Path]::ChangeExtension($macroPath, '.log')
        if (-not (Test-Path -LiteralPath $log)) { throw "宏日志缺失: $log（ALPHA LOG 没闭合？）" }
        $map = @{}
        $pending = $null
        foreach ($line in Get-Content -LiteralPath $log) {
            $text = $line.Trim()
            if ($text -match '^Ref\s+=?(\d+/\d+)$') { $pending = $Matches[1]; continue }
            if ($pending -and $text -match '^Name\s+(/\S+)$') { $map[$Matches[1]] = $pending }
            $pending = $null
        }
        return $map
    }

    function Invoke-Leg([string]$caseId, [string]$leg, [string]$macroPath) {
        $before = Get-Sesno
        $legOut = Join-Path $out "$caseId-$leg"
        & $l3Exe --check-driver $macroPath --project-dir $ProjectDir `
            --e3d-project $E3dProject --e3d-login $E3dLogin --e3d-mdb $E3dMdb `
            --output $legOut
        Assert-Ok "E3D driver ($caseId/$leg)"
        $after = Get-Sesno
        if ($after -ne $before + 1) {
            throw "$caseId/$leg 后 sesno $before -> $after，期望恰好 +1。宏里 SAVEWORK 数量不对，或有并发会话在写这个库——本轮作废，别继续录"
        }
        Write-Host ("  {0,-8} sesno {1} -> {2}" -f $leg, $before, $after)
        return $after
    }

    # ── 前置校验 ────────────────────────────────────────────────────────────
    Write-Host '== 前置校验 =='
    if (-not (Test-Path -LiteralPath $TargetDbFile -PathType Leaf)) {
        throw "目标库文件不存在: $TargetDbFile"
    }
    if ((Split-Path -Leaf $TargetDbFile) -notmatch [string]$Dbnum) {
        throw "目标文件名 $(Split-Path -Leaf $TargetDbFile) 与 dbnum $Dbnum 不符——录错库要重录"
    }
    if ((Get-Item -LiteralPath $TargetDbFile).IsReadOnly) {
        throw "目标库只读，E3D 存不进去: $TargetDbFile"
    }
    if (-not (Test-Path -LiteralPath $ProjectDir -PathType Container)) {
        throw "E3D 项目目录不存在: $ProjectDir"
    }

    $manifestPath = Join-Path $repo $CaseManifest
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    if ($manifest.dbnum -ne $Dbnum) {
        throw "清单 dbnum $($manifest.dbnum) 与 -Dbnum $Dbnum 不符"
    }
    if (-not $manifest.cases) { throw '清单里没有案例' }

    $macroPaths = @{}
    foreach ($case in $manifest.cases) {
        if (-not $case.id) { throw '案例缺 id' }
        $macroPaths["$($case.id)/apply"] = Test-MacroDiscipline $case.apply_macro
        if ($case.restore_macro) {
            $macroPaths["$($case.id)/restore"] = Test-MacroDiscipline $case.restore_macro
        }
    }
    Write-Host "  宏纪律通过：$($macroPaths.Count) 条腿 / $($manifest.cases.Count) 个案例"

    cargo build --bin l3_suite --bin db_session_fixture `
--no-default-features --features "ws,gen_model,manifold,project_hd,http_api"
    Assert-Ok 'cargo build'

    $baseline = Get-Sesno
    Write-Host "  baseline_sesno = $baseline"

    if ($CheckOnly) {
        Write-Host "`n-CheckOnly：前置与宏审查全过，未登录 E3D、未改库。"
        return
    }

    # ── 逐案例录制 ──────────────────────────────────────────────────────────
    Write-Host "`n== 录制 =="
    Write-Warning '录制期间禁止对该库做 MERGE / PURGE / 压缩，也不要让别的会话写它。'
    $recordedCases = @()
    foreach ($case in $manifest.cases) {
        Write-Host "[$($case.id)]"
        $applyMacro = $macroPaths["$($case.id)/apply"]
        $applySesno = Invoke-Leg $case.id 'apply' $applyMacro
        $refnos = Read-RefnoMap $applyMacro

        $restoreSesno = $null
        if ($case.restore_macro) {
            $restoreSesno = Invoke-Leg $case.id 'restore' $macroPaths["$($case.id)/restore"]
        }

        $elements = @()
        $refs = @{}
        foreach ($element in $case.elements) {
            $refno = $refnos[$element.name]
            if (-not $refno) {
                throw "案例 $($case.id) 的元素 $($element.name) 在宏日志里没回读到 refno——宏是不是漏了 Q REF/Q NAME，或者名字拼错了"
            }
            $refs[$element.name] = $refno
            $state = [ordered]@{
                refno        = $refno
                noun         = $element.noun
                before_apply = [bool]$element.before_apply
                after_apply  = [bool]$element.after_apply
            }
            if ($null -ne $restoreSesno -and $null -ne $element.after_restore) {
                $state['after_restore'] = [bool]$element.after_restore
            }
            $elements += [pscustomobject]$state
        }

        $expected = @()
        foreach ($net in $case.expected_net) {
            $expected += [pscustomobject]@{ refno = $refs[$net.element]; net = $net.net }
        }

        $recorded = [ordered]@{
            id          = $case.id
            apply_sesno = $applySesno
        }
        if ($null -ne $restoreSesno) { $recorded['restore_sesno'] = $restoreSesno }
        $recorded['refs'] = $refs
        $recorded['elements'] = $elements
        if ($expected.Count) { $recorded['expected'] = [pscustomobject]@{ net_window = $expected } }
        $recordedCases += [pscustomobject]$recorded
    }

    # ── 产出 ────────────────────────────────────────────────────────────────
    $recordingPath = Join-Path $out 'recording.json'
    [pscustomobject][ordered]@{
        dbnum          = $Dbnum
        baseline_sesno = $baseline
        source         = $TargetDbFile
        cases          = $recordedCases
    } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $recordingPath -Encoding UTF8

    Write-Host "`n== 完成 =="
    Write-Host "recording=$recordingPath"
    Write-Host "窗口 $baseline..$(Get-Sesno)（$($recordedCases.Count) 个案例）"
    Write-Host "`n下一步（打包，无需 E3D）："
    Write-Host "  $fixtureExe pack --recording `"$recordingPath`" --dbnum $Dbnum ``"
    Write-Host "    --out tests/fixtures/issues/issue-021-db8000-session-pair-suite"
}
finally {
    Pop-Location
}
