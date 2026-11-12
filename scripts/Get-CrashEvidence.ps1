#requires -Version 7.0
<#
.SYNOPSIS
  后端进程崩了之后，一步把现场拼起来（GitHub #78）。只读：不碰服务、不改任何文件。

.DESCRIPTION
  `0xC0000409` 在 Windows 上只说明「进程是自己 abort 的」。出错偏移只说明是哪一种 abort：同一个构建里，
  不管在分配什么，分配失败都死在同一条指令上；直接 abort() 是另一个偏移；C 运行库的 abort()（本机 C/C++
  代码，或 Rust 展开途中再 panic）落在 ucrtbase.dll 里。哪一个都不指向出事时在跑的代码。
  要知道当时出了什么事，看三样东西，这个脚本把它们按时间对到一起：

    1. 系统事件日志里的应用崩溃（Application Error，事件 1000）：哪个构建、什么异常码、什么偏移。
    2. 同一时刻 Windows 自己有没有诊断出「虚拟内存不足」（Resource-Exhaustion-Detector，事件 2004）：
       当时整机的提交量 / 提交上限，以及提交量最大的几个进程。崩溃前后几分钟内有这一条，
       又不是本进程占得最多，就是整机可提交内存被别的进程用尽，本进程是被牵连的。
    3. 后端 logs 目录里的 crash-notes.log，以及崩溃那一轮 stderr 的最后几行
       （"memory allocation of N bytes failed" = 分配失败）。

  事件日志里没有不等于没崩：从 IDE / 代理的终端起的进程继承了「崩了不上报」的错误模式，崩了系统既不记
  事件也不留转储，只有 crash-notes.log 和 stderr 里有。

.PARAMETER Days
  往回看多少天，默认 30。

.PARAMETER Process
  崩溃进程名的正则，默认匹配后端与它的测试程序。

.PARAMETER LogsDir
  后端的 logs 目录（放 crash-notes.log 与每一轮 <启动时刻>.stderr.log 的那个）。不给就只看事件日志。

.PARAMETER WindowMinutes
  崩溃前后多少分钟内的内存诊断算「同时」，默认 5（系统隔几分钟才诊断一次）。

.EXAMPLE
  pwsh -NoProfile -File scripts\Get-CrashEvidence.ps1 -LogsDir .\logs -Days 60
#>
[CmdletBinding()]
param(
    [int] $Days = 30,
    [string] $Process = '^aios[-_]database',
    [string] $LogsDir,
    [int] $WindowMinutes = 5
)
$ErrorActionPreference = 'Stop'
$since = (Get-Date).AddDays(-$Days)

function Limit-Line([string] $text, [int] $max) {
    # 日志行里可能带着访问地址：打码，再截短。
    $clean = $text -replace '\b\d{1,3}(\.\d{1,3}){3}(:\d+)?\b', '<addr>'
    if ($clean.Length -gt $max) { $clean.Substring(0, $max) + ' …' } else { $clean }
}

$crashes = @(Get-WinEvent -FilterHashtable @{ LogName = 'Application'; ProviderName = 'Application Error'; Id = 1000; StartTime = $since } -ErrorAction SilentlyContinue |
    ForEach-Object {
        $p = $_.Properties
        [pscustomobject]@{
            Time   = $_.TimeCreated
            App    = [string]$p[0].Value
            # PE 时间戳：同一个构建的偏移才可比。
            Build  = [string]$p[2].Value
            Module = [string]$p[3].Value
            Code   = [string]$p[6].Value
            Offset = ([string]$p[7].Value).TrimStart('0')
        }
    } | Where-Object { $_.App -match $Process } | Sort-Object Time)

$lowMemory = @(Get-WinEvent -FilterHashtable @{ LogName = 'System'; ProviderName = 'Microsoft-Windows-Resource-Exhaustion-Detector'; Id = 2004; StartTime = $since.AddMinutes(-$WindowMinutes) } -ErrorAction SilentlyContinue |
    ForEach-Object {
        $info = ([xml]$_.ToXml()).Event.UserData.MemoryExhaustionInfo
        [pscustomobject]@{
            Time    = $_.TimeCreated
            LimitGb = [double]$info.SystemInfo.SystemCommitLimit / 1GB
            UsedGb  = [double]$info.SystemInfo.SystemCommitCharge / 1GB
            Top     = (@($info.ProcessInfo.ChildNodes | Where-Object { $_.Name } | ForEach-Object { '{0} {1:N1} GB' -f $_.Name, ([double]$_.CommitCharge / 1GB) }) -join '、')
        }
    } | Sort-Object Time)

'最近 {0} 天：进程名匹配 {1} 的崩溃 {2} 次；系统诊断出虚拟内存不足 {3} 次' -f $Days, $Process, $crashes.Count, $lowMemory.Count
$together = 0
foreach ($crash in $crashes) {
    ''
    '{0}  {1}  {2}  {3}+0x{4}  构建 {5}' -f $crash.Time.ToString('yyyy-MM-dd HH:mm:ss'), $crash.App, $crash.Code, $crash.Module, $crash.Offset, $crash.Build
    $near = $lowMemory | Where-Object { [math]::Abs(($_.Time - $crash.Time).TotalMinutes) -le $WindowMinutes } |
        Sort-Object { [math]::Abs(($_.Time - $crash.Time).TotalSeconds) } | Select-Object -First 1
    if ($near) {
        $together++
        $gap = [int]($near.Time - $crash.Time).TotalSeconds
        '    整机虚拟内存不足（诊断在崩溃{0} {1} 秒）：提交量 {2:N1} / 上限 {3:N1} GB；占用最多：{4}' -f $(if ($gap -ge 0) { '后' } else { '前' }), [math]::Abs($gap), $near.UsedGb, $near.LimitGb, $near.Top
    } else {
        '    前后 {0} 分钟内没有虚拟内存不足的诊断' -f $WindowMinutes
    }
    if ($LogsDir -and (Test-Path -LiteralPath $LogsDir)) {
        Get-ChildItem -LiteralPath $LogsDir -File -Filter '*.stderr.log' |
            Where-Object { $_.Length -gt 0 -and $_.LastWriteTime -ge $crash.Time.AddMinutes(-2) -and $_.LastWriteTime -le $crash.Time.AddMinutes(1) } |
            ForEach-Object {
                '    {0} 的最后两行：' -f $_.Name
                Get-Content -LiteralPath $_.FullName -Encoding UTF8 -Tail 2 | ForEach-Object { '      ' + (Limit-Line $_ 220) }
            }
    }
}

if ($LogsDir) {
    $notes = Join-Path $LogsDir 'crash-notes.log'
    ''
    if (Test-Path -LiteralPath $notes) {
        $epoch = [datetime]::new(1970, 1, 1, 0, 0, 0, [DateTimeKind]::Utc)
        $lines = @(Get-Content -LiteralPath $notes -Encoding UTF8 | Where-Object { $_ -match 'unix_ms=(\d+)' -and $epoch.AddMilliseconds([double]$Matches[1]).ToLocalTime() -ge $since })
        'crash-notes.log：最近 {0} 天 {1} 条' -f $Days, $lines.Count
        foreach ($line in $lines) {
            [void]($line -match 'unix_ms=(\d+)')
            '  {0}  {1}' -f $epoch.AddMilliseconds([double]$Matches[1]).ToLocalTime().ToString('yyyy-MM-dd HH:mm:ss'), (Limit-Line ($line -replace '^\[crash\] unix_ms=\d+ ', '') 420)
        }
    } else {
        'crash-notes.log：{0} 里没有' -f $LogsDir
    }
}

''
'小结：{0} 次崩溃里有 {1} 次与「整机虚拟内存不足」同时。' -f $crashes.Count, $together
'怎么读：c0000409 = 进程自己 abort，偏移只分得出是哪一种（同一构建里分配失败都是同一个偏移；模块是'
'        ucrtbase.dll 的是 C 运行库的 abort()），不指向出事的代码；c00000fd = 栈溢出；c0000005 = 访问违例。'
'        crash-notes.log 里 sys_commit_free_mb 是整机还剩的可提交内存，commit_mb 是本进程占的：'
'        前者见底而后者不大，进程是被牵连的。事件日志里没有不等于没崩（从 IDE / 代理终端起的进程崩了不上报）。'
