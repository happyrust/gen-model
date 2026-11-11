# 初始化点名的库：后端没有入队的库不白等。
# 全用假的应答——库列表、初始化请求的回执、后端闲不闲都换成假的，一个请求都不真发。

$script:InitSaid = New-Object System.Collections.ArrayList
$script:InitSleeps = 0
$script:InitPosted = New-Object System.Collections.ArrayList
$script:InitRequested = $false
$script:InitReply = '{"enqueued":[],"warnings":[]}'
$script:InitDbnums = @()          # 库列表里有哪些库
$script:InitBecomeReady = @()     # 请求发出之后，这些库变成「已初始化」
function Say([string]$Text = '', [string]$Color = '') { [void]$script:InitSaid.Add($Text) }
function Start-Sleep { param([double]$Milliseconds = 0, [double]$Seconds = 0) $script:InitSleeps += 1 }
function Test-ServiceHealth($Cfg, [string]$Name) { return @{ Ok = $true; Json = (ConvertFrom-JsonSafe '{"status":"ok"}') } }
function Test-BackendIdle($Cfg) { return $true }
function Get-FailedInitTasks($Cfg, [int[]]$Dbnums, [datetime]$Since) { return @() }
function Test-RecordedProcess($Cfg, [string]$Name, $Record) { return @{ Alive = $true; Ours = $true; Unreadable = $false; Why = ''; Proc = $null } }
function Test-Guards($Cfg, $State) { return '' }
function Get-ServiceNames($Cfg) { return , @('database', 'backend') }
function Get-DbnumRows($Cfg) {
    $rows = @()
    foreach ($db in $script:InitDbnums) {
        $ready = ($script:InitRequested -and $script:InitBecomeReady -contains $db)
        $rows += [pscustomobject]@{ dbnum = $db; db_type = 'DESI'; initialized = $ready; applied_sesno = 57; file_latest_sesno = 57; model_verdict = 'in_sync'; model_source = 'database' }
    }
    return $rows
}
function Invoke-StackHttp([string]$Method, [string]$Url, [string]$BodyJson = '', [int]$TimeoutSec = 5) {
    if ($Method -eq 'POST') { [void]$script:InitPosted.Add($BodyJson); $script:InitRequested = $true }
    return @{ Ok = $true; Status = 200; Body = $script:InitReply; Bytes = $script:InitReply.Length; Error = '' }
}
$initCfg = @{ Guard = @{ ProtectPorts = @() }; Backend = @{ Port = 51022 }; Project = @{ Name = 'Sample' }; Timeouts = @{ InitSec = 600 } }
$initState = @{ backend = @{ pid = 400; log = 'backend.log' } }
function Reset-Init {
    $script:InitSaid.Clear(); $script:InitPosted.Clear()
    $script:InitSleeps = 0; $script:InitRequested = $false
}
function Get-InitSaid { return ($script:InitSaid -join "`n") }
$noFile = 'dbnum=6003: 在执行范围内，但本次扫描没有找到它的候选文件，没有可入队的批次'

Test '从后端的提示里认出没有入队的库：只认点了名的、只认这两种说法' {
    $warnings = @(
        $noFile
        'dbnum=9999: 不在当前 MDB 声明的执行范围内，已拒绝（ADR-020：勾选是范围内的子集选择，不是第二条范围门）'
        'dbnum=7009：在执行范围内，但本次扫描没有找到它的候选文件，没有可入队的批次'
        'dbnum=8000: 在执行范围内，但本次扫描没有找到它的候选文件，没有可入队的批次'
        '目录版本核对已随本次执行另起任务（目录库不进数据批次）：结果见 /health.catalogue_reconcile'
        'dbnum=7352: 别的什么提示'
        $null
        ''
    )
    $found = @(Get-NotEnqueuedDbnums $warnings @(7352, 6003, 9999, 7009))
    Assert-Equal @(6003, 9999, 7009) @($found | ForEach-Object { $_.Dbnum }) '认出了三个（8000 没点名，不算；7352 那句不是这两种说法）'
    Assert-Match $found[0].Reason '没有找到它的候选文件' '带着后端给的原因'
    Assert-Equal 0 @(Get-NotEnqueuedDbnums @() @(7352)).Count '没有提示'
    Assert-Equal 0 @(Get-NotEnqueuedDbnums $null @(7352)).Count '提示是空的'
    Assert-Equal 0 @(Get-NotEnqueuedDbnums @('后端换了一种说法：库 6003 没有文件') @(6003)).Count '写法变了认不出来：返回空（那就和以前一样等）'
}

Test '点名的库里有一个后端没入队：马上说出来，不等它；其余的照常等到做完' {
    Reset-Init
    $script:InitDbnums = @(7352, 6003)
    $script:InitBecomeReady = @(7352)
    $script:InitReply = '{"enqueued":[{"dbnum":7352}],"warnings":["' + $noFile + '"]}'
    Invoke-Init $initCfg $initState @(7352, 6003)
    Assert-Equal @(6003) @($script:InitSkipped) '记下了没入队的库'
    Assert-Match $script:InitPosted[0] '"dbnums":\[7352,6003\]' '请求里两个库都带了'
    Assert-Match (Get-InitSaid) '库 6003 后端没有入队，这一次不会初始化' '说了'
    Assert-Match (Get-InitSaid) '其余点名的库（7352）照常等它们做完' '说了还等谁'
    Assert-Match (Get-InitSaid) '✓ 初始化完成' '7352 做完了'
    Assert-NotMatch (Get-InitSaid) '库 6003：数据到会话' '收尾的清单里没有 6003'
    Assert-True ($script:InitSleeps -lt 5) "没有为 6003 一直等下去（等了 $($script:InitSleeps) 圈）"
}

Test '点名的库后端一个都没入队：马上回来，不等' {
    Reset-Init
    $script:InitDbnums = @(6003)
    $script:InitBecomeReady = @()
    $script:InitReply = '{"enqueued":[],"warnings":["' + $noFile + '"]}'
    Invoke-Init $initCfg $initState @(6003)
    Assert-Equal @(6003) @($script:InitSkipped) '记下了'
    Assert-Equal 0 $script:InitSleeps '一圈都没等'
    Assert-NotMatch (Get-InitSaid) '初始化完成' '没有说做完了'
}

Test '都入队了：照常等到做完，没有「没入队」这回事；上一次的记录不带到这一次' {
    Reset-Init
    $script:InitSkipped = @(6003)
    $script:InitDbnums = @(7352)
    $script:InitBecomeReady = @(7352)
    $script:InitReply = '{"enqueued":[{"dbnum":7352}],"warnings":["生成器身份核对已随本次执行另起任务：结果见 /health.generator_reconcile"]}'
    Invoke-Init $initCfg $initState @(7352)
    Assert-Equal 0 @($script:InitSkipped).Count '没有没入队的'
    Assert-Match (Get-InitSaid) '✓ 初始化完成' '做完了'
    Assert-Match (Get-InitSaid) '后端提示：生成器身份核对' '别的提示照常显示'
}
