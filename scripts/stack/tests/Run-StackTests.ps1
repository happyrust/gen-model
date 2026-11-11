<#
入口（stack.ps1）的自动化测试。不依赖 Pester 或别的模块：Windows PowerShell 5.1 和 PowerShell 7 都能直接跑。

用法（在哪个目录跑都行）：
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\stack\tests\Run-StackTests.ps1     Windows PowerShell 5.1
  pwsh -NoProfile -File scripts\stack\tests\Run-StackTests.ps1                                    PowerShell 7
    -Filter <一段文字>     只跑名字里带这段文字的测试
    -Nginx <nginx.exe>     给了就加跑「真起一个 nginx」的路由测试；不给，那几条记成跳过
    -Keep                  测试用的临时目录留着不删（排查用）

它做什么、不做什么：
  · 测试文件是同目录下的 *.tests.ps1，一个文件一组，各自在独立的作用域里跑，互不影响。
  · 每组先把 stack.ps1 的函数点源进来（. stack.ps1 __lib —— 只定义函数，不执行任何命令）。
  · 进程管理那一组用假的进程表（把查进程、查端口、结束进程这几个函数换成假的），不碰真的进程；
    另有一条在真进程上做的，只动它自己起的一个 cmd 和它下面的一个 ping。
  · 路由那一组真起 nginx 和一个只在本机听的假后端，端口现找两个空的，用完就停。
  · 临时文件都在系统临时目录下的一个新建子目录里，跑完删掉。不改仓库里的任何文件。
退出码：全部通过 0；有没过的 1。
#>
[CmdletBinding()]
param(
    [string]$Filter = '',
    [string]$Nginx = '',
    [switch]$Keep
)
$ErrorActionPreference = 'Stop'
$script:TestsDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$script:StackDir = Split-Path -Parent $script:TestsDir
$script:StackScript = Join-Path $script:StackDir 'stack.ps1'
$script:TestNginx = $Nginx
$script:TestFilter = $Filter
$script:TestUtf8 = New-Object System.Text.UTF8Encoding -ArgumentList $false
$script:TestTemp = Join-Path ([System.IO.Path]::GetTempPath()) ('stack-tests-' + [Guid]::NewGuid().ToString('N').Substring(0, 8))
[void][System.IO.Directory]::CreateDirectory($script:TestTemp)
$script:TestResults = New-Object System.Collections.ArrayList
$script:TestGroup = ''
try { if ([Console]::IsOutputRedirected) { [Console]::OutputEncoding = $script:TestUtf8 } } catch { }

# ───────────── 写测试用的几个小函数 ─────────────

# 登记并马上跑一条测试。名字里带 -Filter 给的文字才跑。
function Test([string]$Name, [scriptblock]$Body) {
    $full = "$($script:TestGroup)：$Name"
    if ($script:TestFilter -and $full.IndexOf($script:TestFilter, [System.StringComparison]::OrdinalIgnoreCase) -lt 0) { return }
    $outcome = 'pass'
    $detail = ''
    try {
        & $Body | Out-Null
    } catch {
        if ($_.Exception.Data -and $_.Exception.Data.Contains('TestSkip')) {
            $outcome = 'skip'
            $detail = $_.Exception.Message
        } else {
            $outcome = 'fail'
            $detail = "$($_.Exception.Message)`n      $(($_.ScriptStackTrace -split "`n" | Select-Object -First 3) -join "`n      ")"
        }
    }
    [void]$script:TestResults.Add(@{ Name = $full; Outcome = $outcome; Detail = $detail })
    if ($outcome -eq 'pass') { Write-Host "  ✓ $full" -ForegroundColor Green }
    elseif ($outcome -eq 'skip') { Write-Host "  – $full（跳过：$detail）" -ForegroundColor Yellow }
    else { Write-Host "  ✗ $full" -ForegroundColor Red; Write-Host "      $detail" -ForegroundColor Red }
}

function Skip-Test([string]$Why) {
    $ex = New-Object System.Exception -ArgumentList $Why
    $ex.Data['TestSkip'] = $true
    throw $ex
}

function Assert-True($Condition, [string]$What) {
    if (-not $Condition) { throw "断言没过：$What" }
}

function Assert-Equal($Expected, $Actual, [string]$What = '') {
    $e = (@($Expected) | ForEach-Object { "$_" }) -join '|'
    $a = (@($Actual) | ForEach-Object { "$_" }) -join '|'
    if ($e -cne $a) { throw "断言没过：$What`n      期望「$e」`n      实际「$a」" }
}

function Assert-Match([string]$Text, [string]$Pattern, [string]$What = '') {
    if ($Text -notmatch $Pattern) { throw "断言没过：$What`n      期望里面有「$Pattern」`n      实际「$(if ($Text.Length -gt 400) { $Text.Substring(0, 400) + ' …' } else { $Text })」" }
}

function Assert-NotMatch([string]$Text, [string]$Pattern, [string]$What = '') {
    if ($Text -match $Pattern) { throw "断言没过：$What`n      不该有「$Pattern」，却出现了「$($Matches[0])」" }
}

# 跑一段会用 Stop-Stack 报错的代码，核对退出码和报错里的字样。返回报错的全文。
function Assert-StackError([scriptblock]$Body, [int]$Code, [string]$Pattern = '', [string]$What = '') {
    $message = $null
    $actual = -1
    try { & $Body | Out-Null } catch {
        $message = "$($_.Exception.Message)"
        if ($_.Exception.Data -and $_.Exception.Data.Contains('StackExitCode')) { $actual = [int]$_.Exception.Data['StackExitCode'] }
    }
    if ($null -eq $message) { throw "断言没过：$What —— 应当报错（退出码 $Code），却没有报错" }
    if ($actual -ne $Code) { throw "断言没过：$What —— 退出码应当是 $Code，实际 $actual（$message）" }
    if ($Pattern -and $message -notmatch $Pattern) { throw "断言没过：$What —— 报错里应当有「$Pattern」，实际「$message」" }
    return $message
}

# 在测试的临时目录下新建一个子目录。
function New-TestDir([string]$Name) {
    $dir = Join-Path $script:TestTemp ($Name + '-' + [Guid]::NewGuid().ToString('N').Substring(0, 6))
    [void][System.IO.Directory]::CreateDirectory($dir)
    return $dir
}

function Write-TestFile([string]$Path, [string]$Text) {
    [void][System.IO.Directory]::CreateDirectory((Split-Path -Parent $Path))
    [System.IO.File]::WriteAllText($Path, $Text, $script:TestUtf8)
}

# 找一个此刻没人听的本机端口（让系统挑）。
function Get-FreeTestPort {
    $listener = New-Object System.Net.Sockets.TcpListener -ArgumentList ([System.Net.IPAddress]::Loopback), 0
    $listener.Start()
    $port = ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
    $listener.Stop()
    return $port
}

# ───────────── 逐个文件跑 ─────────────
Write-Host "stack 的自动化测试 —— PowerShell $($PSVersionTable.PSVersion)；入口 $script:StackScript"
$files = @(Get-ChildItem -LiteralPath $script:TestsDir -Filter '*.tests.ps1' -File | Sort-Object Name)
foreach ($file in $files) {
    $script:TestGroup = $file.BaseName -replace '\.tests$', ''
    Write-Host "[$($script:TestGroup)]"
    try {
        # 每个文件一个独立的作用域：里面点源进来的函数、换掉的假函数，出了这个花括号就没了。
        & {
            . $script:StackScript __lib
            . $file.FullName
        }
    } catch {
        [void]$script:TestResults.Add(@{ Name = "$($script:TestGroup)：（整个文件）"; Outcome = 'fail'; Detail = "$($_.Exception.Message)" })
        Write-Host "  ✗ 这个文件没跑完：$($_.Exception.Message)" -ForegroundColor Red
        Write-Host "      $(($_.ScriptStackTrace -split "`n" | Select-Object -First 3) -join "`n      ")" -ForegroundColor Red
    }
}

if (-not $Keep) { try { Remove-Item -LiteralPath $script:TestTemp -Recurse -Force -ErrorAction SilentlyContinue } catch { } }
else { Write-Host "临时目录留着：$script:TestTemp" }

$passed = @($script:TestResults | Where-Object { $_.Outcome -eq 'pass' }).Count
$skipped = @($script:TestResults | Where-Object { $_.Outcome -eq 'skip' }).Count
$failed = @($script:TestResults | Where-Object { $_.Outcome -eq 'fail' }).Count
Write-Host ''
if ($failed -gt 0) {
    Write-Host "没过：$failed 条（通过 $passed 条，跳过 $skipped 条）。" -ForegroundColor Red
    foreach ($r in @($script:TestResults | Where-Object { $_.Outcome -eq 'fail' })) { Write-Host "  ✗ $($r.Name)" -ForegroundColor Red }
    exit 1
}
Write-Host "全部通过：$passed 条$(if ($skipped -gt 0) { "（另有 $skipped 条跳过）" })。" -ForegroundColor Green
exit 0
