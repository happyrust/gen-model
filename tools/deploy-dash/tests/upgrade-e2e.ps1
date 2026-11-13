# 自动升级 · 命令行端到端（U3）：起两个真代理，用 deploy-dash.exe upgrade 真升两台。
#
# 做什么：
#   1. 建两个沙盒根（各有假 stack.ps1 + 一份已装清单 + 一个旧文件）
#   2. 各起一个 deploy-dash-agent.exe（真代理，真跑假 stack.ps1）
#   3. 生成升级密钥、写 servers.toml（DPAPI 加密）到一个临时存档
#   4. 摆一个来源包（新文件 + release.json）
#   5. 跑 `deploy-dash.exe upgrade --all --yes --store <临时存档>`
#   6. 核对：两台的都换了、退出码 0、历史里有两条
#   7. 再来一次「第一台起不来」：核对换回、第二台没动、退出码 1
#
# 端口用 18681–18682（板上给 agent-server 留的 18631–18639 之外，避免撞上别人的）。
param(
    [string]$Root = 'D:\work\plant-code\old\deploy-dash-upgrade',
    [int]$PortA = 18681,
    [int]$PortB = 18682,
    # all = 两台都成；fail-first = 第一台起不来（up 回 4），验换回 + 第二台不动
    [ValidateSet('all', 'fail-first')]
    [string]$Scene = 'all'
)
$ErrorActionPreference = 'Stop'
$work = Join-Path $Root 'spike\tmp\up-e2e'
Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $work | Out-Null
$agentExe = Join-Path $Root 'spike\target\debug\deploy-dash-agent.exe'
$panelExe = Join-Path $Root 'spike\target\debug\deploy-dash.exe'
if (-not (Test-Path $agentExe)) { throw "缺代理 exe：$agentExe" }
if (-not (Test-Path $panelExe)) { throw "缺面板 exe：$panelExe" }
$out = @()
function Say($t) { $script:out += $t; Write-Host $t }

# ── 沙盒 ──
function New-Sandbox([string]$tag, [bool]$UpFails) {
    $r = Join-Path $work $tag
    New-Item -ItemType Directory -Force -Path (Join-Path $r 'bin') | Out-Null
    [IO.File]::WriteAllText((Join-Path $r 'bin\aios-database.exe'), "old-$tag")
    $oldSha = (Get-FileHash (Join-Path $r 'bin\aios-database.exe') -Algorithm SHA256).Hash.ToLower()
    $rel = @"
{"name":"pkg","version":"v1","channel":"rc","created_at":"2026-11-12T09:37:05+08:00",
 "components":[{"id":"backend","title":"后端","version":"0.1.44","path":"bin/aios-database.exe","sha256":"$oldSha","dirty":false,"bytes":8}],
 "files":[{"path":"bin/aios-database.exe","bytes":8,"sha256":"$oldSha"}]}
"@
    [IO.File]::WriteAllText((Join-Path $r 'release.json'), $rel)
    $upExit = if ($UpFails) { 4 } else { 0 }
    $script = @"
param([Parameter(Position=0)][string]`$Command = 'help', [string]`$Config = '', [switch]`$NoInit)
Write-Output "fake stack: `$Command"
if (`$Command -eq 'up') { Write-Output 'fake: starting'; exit $upExit }
if (`$Command -eq 'down') { Write-Output 'fake: stopping'; exit 0 }
exit 0
"@
    [IO.File]::WriteAllText((Join-Path $r 'stack.ps1'), $script)
    [IO.File]::WriteAllText((Join-Path $r 'stack.toml'), "[project]`nname = `"$tag`"`n")
    return $r
}

# ── 起代理 ──
function Start-Agent([string]$root, [int]$port) {
    $p = Start-Process -FilePath $agentExe -ArgumentList @('--root', $root, '--listen', "127.0.0.1:$port") -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $root 'agent.out.log') -RedirectStandardError (Join-Path $root 'agent.err.log')
    $deadline = (Get-Date).AddSeconds(20)
    while ((Get-Date) -lt $deadline) {
        try {
            $r = Invoke-WebRequest "http://127.0.0.1:$port/v1/ping" -UseBasicParsing -TimeoutSec 2
            if ($r.StatusCode -eq 200) { return $p }
        } catch { Start-Sleep -Milliseconds 200 }
    }
    throw "代理没起来（端口 $port）：$((Get-Content (Join-Path $root 'agent.err.log') -Raw -ErrorAction SilentlyContinue))"
}

Say "== 建沙盒（场景 $Scene） =="
$failA = ($Scene -eq 'fail-first')
$rootA = New-Sandbox 'sandbox-a' $false
$rootB = New-Sandbox 'sandbox-b' $false
Say "  A=$rootA"
Say "  B=$rootB"
# 失败场景：把 A 的目标文件独占打开，代理「挪旧文件进备份」那一步会失败（重试 5 次 × 2 秒后放弃），
# 走的是「换文件中途失败 → 照日志换回」这条路（设计 5.6）。沙盒里没有在跑的服务，用 up 失败注入不到。
$lockA = $null
if ($failA) {
    $lockA = [IO.File]::Open((Join-Path $rootA 'bin\aios-database.exe'), 'Open', 'ReadWrite', 'None')
    Say "  已独占打开 A 的 bin\aios-database.exe（换文件会失败）"
}

Say "== 起两个代理（$PortA / $PortB） =="
$pa = Start-Agent $rootA $PortA
$pb = Start-Agent $rootB $PortB
Say "  A pid=$($pa.Id)  B pid=$($pb.Id)"

try {
    Say "== 生成升级密钥 =="
    $keyA = (& $agentExe upgrade-key --root $rootA 2>&1 | Select-String -Pattern '^[0-9a-f]{64}$').ToString().Trim()
    $keyB = (& $agentExe upgrade-key --root $rootB 2>&1 | Select-String -Pattern '^[0-9a-f]{64}$').ToString().Trim()
    if ($keyA.Length -ne 64 -or $keyB.Length -ne 64) { throw "密钥没生成：A=$($keyA.Length) B=$($keyB.Length)" }
    Say "  A key_id=$($keyA.Substring(0,8))  B key_id=$($keyB.Substring(0,8))（只显示前 8 位）"

    Say "== 令牌 =="
    $tokA = (& $agentExe show-token --root $rootA).Trim()
    $tokB = (& $agentExe show-token --root $rootB).Trim()
    Say "  A 令牌 $($tokA.Length) 字  B 令牌 $($tokB.Length) 字"

    Say "== 摆来源包 =="
    $src = Join-Path $work 'pkg-v2'
    New-Item -ItemType Directory -Force -Path (Join-Path $src 'bin') | Out-Null
    $newBytes = [Text.Encoding]::UTF8.GetBytes('new-backend-v2')
    [IO.File]::WriteAllBytes((Join-Path $src 'bin\aios-database.exe'), $newBytes)
    $newSha = (Get-FileHash (Join-Path $src 'bin\aios-database.exe') -Algorithm SHA256).Hash.ToLower()
    $srcRel = @"
{"name":"pkg","version":"v2","channel":"rc","created_at":"2026-11-13T09:00:00+08:00",
 "components":[{"id":"backend","title":"后端","version":"0.1.45","path":"bin/aios-database.exe","sha256":"$newSha","dirty":false,"bytes":$($newBytes.Length)}],
 "files":[{"path":"bin/aios-database.exe","bytes":$($newBytes.Length),"sha256":"$newSha"}]}
"@
    [IO.File]::WriteAllText((Join-Path $src 'release.json'), $srcRel)
    Say "  来源包 $src（$($newBytes.Length) 字节）"

    Say "== 写 servers.toml（DPAPI 加密，用面板自己的加密） =="
    $store = Join-Path $work 'store'
    New-Item -ItemType Directory -Force -Path $store | Out-Null
    # 直接在脚本里做 DPAPI（不生成子脚本，省掉一层引号地狱）
    Add-Type -AssemblyName System.Security
    $prot = {
        param([string]$s, [string]$ent)
        $e = [Text.Encoding]::UTF8.GetBytes($ent)
        $c = [Security.Cryptography.ProtectedData]::Protect([Text.Encoding]::UTF8.GetBytes($s), $e, [Security.Cryptography.DataProtectionScope]::CurrentUser)
        'dpapi:' + [Convert]::ToBase64String($c)
    }
    $tokEnt = 'AiosDeployDash servers.toml token v1'
    $keyEnt = 'AiosDeployDash servers.toml upgrade-key v1'
    $nl = [Environment]::NewLine
    $lines = [System.Collections.Generic.List[string]]::new()
    $lines.Add('# 自动升级端到端测试用的存档（这个文件是本脚本造的）')
    $lines.Add('schema = 1')
    foreach ($pair in @(@('甲', $PortA, $tokA, $keyA, 1), @('乙', $PortB, $tokB, $keyB, 2))) {
        $name, $port, $tok, $key, $order = $pair
        $lines.Add('')
        $lines.Add('[[server]]')
        $lines.Add('name = "' + $name + '"')
        $lines.Add('address = "127.0.0.1"')
        $lines.Add('port = ' + $port)
        $lines.Add('token = "' + (& $prot $tok $tokEnt) + '"')
        $lines.Add('upgrade_key = "' + (& $prot $key $keyEnt) + '"')
        $lines.Add('note = "e2e"')
        $lines.Add('order = ' + $order)
    }
    [IO.File]::WriteAllText((Join-Path $store 'servers.toml'), ($lines -join $nl) + $nl, [Text.UTF8Encoding]::new($false))
    Say "  存档 $store（$($lines.Count) 行）"

    Say "== 跑 upgrade --all --yes --json =="
    $runOut = Join-Path $work 'cli.out.log'
    $runErr = Join-Path $work 'cli.err.log'
    $cli = Start-Process -FilePath $panelExe -ArgumentList @('upgrade', '--source', $src, '--all', '--yes', '--json', '--no-agent', '--store', $store, '--wait-idle', '5') -Wait -PassThru -NoNewWindow -RedirectStandardOutput $runOut -RedirectStandardError $runErr
    $code = $cli.ExitCode
    $json = @(Get-Content $runOut -Encoding utf8 -ErrorAction SilentlyContinue)
    $errText = @(Get-Content $runErr -Encoding utf8 -ErrorAction SilentlyContinue)
    Say "  退出码=$code"
    Say "  stdout：$($json -join ' | ')"
    if ($errText.Count -gt 0) { Say "  stderr：$($errText -join ' | ')" }
    $parsed = $json | Select-String -Pattern '^\{' | Select-Object -First 1
    if ($parsed) { $o = $parsed.ToString() | ConvertFrom-Json; Say "  解析：$($o.servers.Count) 台" ; foreach ($s in $o.servers) { Say "    $($s.name): skipped=$($s.skipped) outcome='$($s.outcome)'" } }

    Say "== 核对 =="
    if ($lockA) { $lockA.Dispose(); $lockA = $null; Say "  已松开 A 的文件锁" }
    $aNow = [IO.File]::ReadAllText((Join-Path $rootA 'bin\aios-database.exe'))
    $bNow = [IO.File]::ReadAllText((Join-Path $rootB 'bin\aios-database.exe'))
    $aHist = Join-Path $rootA 'run\gui\upgrade\history.jsonl'
    $bHist = Join-Path $rootB 'run\gui\upgrade\history.jsonl'
    Say "  A 文件：$aNow"
    Say "  B 文件：$bNow"
    Say "  A 历史：$(Test-Path $aHist)"
    Say "  B 历史：$(Test-Path $bHist)"
    $bad = @()
    if ($Scene -eq 'all') {
        if ($code -ne 0) { $bad += "期望退出码 0，实得 $code" }
        if ($aNow -ne 'new-backend-v2') { $bad += "A 应换成新内容，实得「$aNow」" }
        if ($bNow -ne 'new-backend-v2') { $bad += "B 应换成新内容，实得「$bNow」" }
        if (-not (Test-Path $aHist) -or -not (Test-Path $bHist)) { $bad += '两台都该有历史' }
    } else {
        if ($code -ne 1) { $bad += "期望退出码 1，实得 $code" }
        if ($aNow -ne 'old-sandbox-a') { $bad += "A 应还是旧内容，实得「$aNow」" }
        if ($bNow -ne 'old-sandbox-b') { $bad += "B 不该被动，实得「$bNow」" }
        # A 应留一份换文件日志，最后一行是 rolled_back
        $aJrn = Get-ChildItem (Join-Path $rootA 'run\gui\upgrade\applied') -Directory -ErrorAction SilentlyContinue
        Say "  A 应用目录：$($aJrn.Count) 个"
        if ($aJrn.Count -gt 0) {
            $jf = Join-Path $aJrn[0].FullName 'journal.jsonl'
            if (Test-Path $jf) {
                $ops = (Get-Content $jf -Encoding utf8) | ForEach-Object { ($_ | ConvertFrom-Json).op }
                Say "  A 日志 ops：$($ops -join ' → ')"
                if ($ops[-1] -ne 'rolled_back') { $bad += "A 日志最后一行该是 rolled_back，实得 $($ops[-1])" }
            } else { $bad += 'A 缺 journal.jsonl' }
        }
        # A 的历史应有一条不是 ok 的
        if (Test-Path $aHist) {
            $lastA = (Get-Content $aHist -Tail 1 -Encoding utf8) | ConvertFrom-Json
            Say "  A 最后一条：outcome=$($lastA.outcome) failed_step=$($lastA.failed_step) message=$($lastA.message)"
            if ($lastA.outcome -eq 'ok') { $bad += 'A 不该报 ok' }
        }
        # B 一条都不该有（连传都没传）
        Say "  B 有历史：$(Test-Path $bHist)"
        if (Test-Path $bHist) { $bad += 'B 不该有历史（一台没成就该停下，不该碰第二台）' }
    }
    if ($bad.Count -eq 0) { Say "  通过" } else { foreach ($m in $bad) { Say "  !! $m" } }
}
finally {
    Say "== 收尾 =="
    if ($lockA) { try { $lockA.Dispose() } catch {} }
    foreach ($p in @($pa, $pb)) {
        if ($p -and -not $p.HasExited) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
    }
    Say "  两个代理已停"
    $out | Set-Content (Join-Path $work 'run.log') -Encoding utf8
}
