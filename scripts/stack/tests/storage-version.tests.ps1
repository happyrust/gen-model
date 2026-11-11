# 库版本这一步：归类对不对、该等的等、不该等的不白等、话里不劝人删数据库。

$script:SvSaid = New-Object System.Collections.ArrayList
$script:SvIdle = $true
function Say([string]$Text = '', [string]$Color = '') { [void]$script:SvSaid.Add($Text) }
function Start-Sleep { param([double]$Milliseconds = 0, [double]$Seconds = 0) }
function Test-Guards($Cfg, $State) { return '' }
function Test-BackendIdle($Cfg) { return $script:SvIdle }

function Health([string]$Json) { return (ConvertFrom-JsonSafe $Json) }

Test '归类：一致 / 新库在启动 / 等初始化 / 有数据没记录 / 旧 / 新 / 升级中 / 升级失败 / 设置模式 / 没应答' {
    $cases = @(
        @{ Want = 'unknown'; Json = '' }
        @{ Want = 'setup'; Json = '{"mode":"setup"}' }
        @{ Want = 'unknown'; Json = '{"status":"ok","storage_version":null}' }
        @{ Want = 'ok'; Json = '{"storage_version":{"expected":1,"stored":1,"caught_up":true,"upgrade":{"state":"idle"}},"initialization":{"status":"model_ready","model_ready":true}}' }
        @{ Want = 'starting'; Json = '{"storage_version":{"expected":1,"stored":null,"caught_up":false,"upgrade":{"state":"idle"}},"initialization":{"status":"discovering","model_ready":false}}' }
        @{ Want = 'starting'; Json = '{"storage_version":{"expected":1,"stored":null,"caught_up":false,"upgrade":{"state":"idle"}}}' }
        @{ Want = 'awaiting-model'; Json = '{"storage_version":{"expected":1,"stored":null,"caught_up":false,"upgrade":{"state":"idle"}},"initialization":{"status":"model_running","data_ready":true,"model_ready":false},"watch_dbnums":[7352]}' }
        @{ Want = 'awaiting-model'; Json = '{"storage_version":{"expected":1,"stored":null,"caught_up":false,"upgrade":{"state":"idle"}},"initialization":{"status":"running","data_ready":false,"model_ready":false}}' }
        @{ Want = 'no-record'; Json = '{"storage_version":{"expected":1,"stored":null,"caught_up":false,"upgrade":{"state":"idle"}},"initialization":{"status":"model_ready","model_ready":true}}' }
        @{ Want = 'no-record'; Json = '{"storage_version":{"expected":1,"stored":null,"caught_up":false,"upgrade":{"state":"done"}},"initialization":{"status":"model_ready","model_ready":true}}' }
        @{ Want = 'old'; Json = '{"storage_version":{"expected":2,"stored":1,"caught_up":false,"upgrade":{"state":"idle"}},"initialization":{"status":"model_ready","model_ready":true}}' }
        @{ Want = 'newer'; Json = '{"storage_version":{"expected":2,"stored":3,"caught_up":false,"upgrade":{"state":"idle"}},"initialization":{"status":"model_ready","model_ready":true}}' }
        @{ Want = 'upgrading'; Json = '{"storage_version":{"expected":2,"stored":1,"caught_up":false,"upgrade":{"state":"running","requested":[7352,7997,8000],"done":1}},"initialization":{"status":"running","model_ready":false}}' }
        @{ Want = 'upgrade-failed'; Json = '{"storage_version":{"expected":2,"stored":null,"caught_up":false,"upgrade":{"state":"failed","requested":[7352],"done":0}},"initialization":{"status":"model_ready","model_ready":true}}' }
    )
    foreach ($c in $cases) {
        $sv = Get-StorageVersionKind (Health $c.Json)
        Assert-Equal $c.Want $sv.Kind "健康检查 $($c.Json)"
    }
    $sv = Get-StorageVersionKind (Health '{"storage_version":{"expected":2,"stored":1,"caught_up":false,"upgrade":{"state":"running","requested":[7352,7997,8000],"done":1}}}')
    Assert-Equal '1/3' "$($sv.UpgradeDone)/$($sv.UpgradeTotal)" '升级进度读对了'
    $sv = Get-StorageVersionKind (Health '{"storage_version":{"expected":1,"stored":null,"caught_up":false},"watch_dbnums":[7352,8000]}')
    Assert-Match $sv.Text '还没有' '没有版本记录时说「还没有」'
    Assert-Equal @(7352, 8000) @($sv.Watch) '读出了只管哪几个库'
    $sv = Get-StorageVersionKind (Health '{"storage_version":{"expected":1,"stored":1,"caught_up":true},"watch_dbnums":null}')
    Assert-Equal 0 @($sv.Watch).Count '没配 watch_dbnums 就是空的'
}

# 让 Wait-StorageVersion 依次看到给定的几种情况（最后一种一直重复），数它问了几次。
function Invoke-WaitWith([string[]]$Kinds, [int]$TimeoutSec = 180) {
    $script:SvQueue = New-Object System.Collections.Queue
    foreach ($k in $Kinds) { $script:SvQueue.Enqueue($k) }
    $script:SvAsked = 0
    function Get-StorageVersionState($Cfg) {
        $script:SvAsked += 1
        $kind = $script:SvQueue.Peek()
        if ($script:SvQueue.Count -gt 1) { $kind = $script:SvQueue.Dequeue() }
        return @{ Kind = $kind; Known = $true; CaughtUp = ($kind -eq 'ok'); Text = "假的：$kind"; Stored = $null; Expected = 1; ModelReady = $false; InitStatus = ''; Watch = @(); Idle = $false; UpgradeDone = 0; UpgradeTotal = 0 }
    }
    $fakeCfg = @{ Timeouts = @{ StorageVersionSec = $TimeoutSec }; Guard = @{ AbortCommitFreeGb = 0; MaxBackendGb = 0 } }
    $sv = Wait-StorageVersion $fakeCfg @{}
    return @{ Kind = $sv.Kind; Asked = $script:SvAsked; Idle = $sv.Idle }
}

Test '等待：新库写上了就往下走；有数据没记录的不白等；旧库、设置模式一问就回' {
    $script:SvIdle = $true
    $r = Invoke-WaitWith @('starting', 'starting', 'ok')
    Assert-Equal 'ok/3' "$($r.Kind)/$($r.Asked)" '新库：等到写上为止'

    $r = Invoke-WaitWith @('no-record')
    Assert-Equal 'no-record/5' "$($r.Kind)/$($r.Asked)" '有数据没记录：连看 5 次（约 5 秒）就回，不等满 180 秒'

    $r = Invoke-WaitWith @('starting', 'no-record', 'no-record', 'ok')
    Assert-Equal 'ok/4' "$($r.Kind)/$($r.Asked)" '「就绪」和「写版本记录」是前后脚：没满 5 次之前写上了，照常往下走'

    $r = Invoke-WaitWith @('no-record', 'no-record', 'starting', 'no-record')
    Assert-Equal 'no-record/8' "$($r.Kind)/$($r.Asked)" '中间变过，就重新数 5 次'

    foreach ($kind in 'old', 'newer', 'setup', 'upgrade-failed') {
        $r = Invoke-WaitWith @($kind)
        Assert-Equal "$kind/1" "$($r.Kind)/$($r.Asked)" "${kind}：等多久都不会变，一问就回"
    }
}

Test '等待：启动做完了、后端闲着却没有版本记录（等库被初始化）——不白等；后端在忙就接着等' {
    $script:SvIdle = $true
    $r = Invoke-WaitWith @('starting', 'awaiting-model')
    Assert-Equal 'awaiting-model/11/True' "$($r.Kind)/$($r.Asked)/$($r.Idle)" '闲着：连看 10 次就回，并且标出「闲着」'

    $script:SvIdle = $false
    $r = Invoke-WaitWith @('awaiting-model') 1
    Assert-Equal 'awaiting-model/False' "$($r.Kind)/$($r.Idle)" '在忙：等到点，没有标「闲着」'
    Assert-True ($r.Asked -gt 10) "在忙：是等到点的（问了 $($r.Asked) 次）"

    $r = Invoke-WaitWith @('awaiting-model', 'awaiting-model', 'ok')
    Assert-Equal 'ok/3' "$($r.Kind)/$($r.Asked)" '在忙：初始化做完、版本记录写上了，就往下走'
    $script:SvIdle = $true
}

Test '等待：等得到的情形（新库在启动、升级中、后端还没应答）到点才放弃' {
    # 真的钟：把上限设成 1 秒，问到超时为止
    foreach ($kind in 'starting', 'upgrading', 'unknown') {
        $r = Invoke-WaitWith @($kind) 1
        Assert-Equal $kind $r.Kind "$kind 到点返回原样"
        Assert-True ($r.Asked -gt 5) "$kind 是等到点的（问了 $($r.Asked) 次），不是一问就回"
    }
}

Test '提示：每种情况都说得出下一步；不劝人删数据库目录；升级说清楚会清空重建' {
    $fakeCfg = @{ Backend = @{ Port = 51022 }; Database = @{ DataDir = 'D:\x\data\surreal' } }
    foreach ($kind in 'starting', 'awaiting-model', 'no-record', 'old', 'newer', 'upgrading', 'upgrade-failed', 'setup', 'unknown') {
        foreach ($idle in $true, $false) {
            $script:SvSaid.Clear()
            Show-StorageVersionAdvice $fakeCfg @{ Kind = $kind; Text = '库里的版本记录：还没有；程序要的：1'; Stored = 1; Expected = 2; UpgradeDone = 1; UpgradeTotal = 3; Idle = $idle; Watch = @() }
            $text = ($script:SvSaid -join "`n")
            Assert-True ($script:SvSaid.Count -ge 2) "$kind 有话说"
            Assert-Match $text '下一步|怎么办' "$kind 说了下一步"
            Assert-NotMatch $text '删掉文件夹|删除文件夹|删掉 data|删掉整个' "$kind 没有劝人删数据库目录"
        }
        Assert-Match (Get-StorageVersionHint @{ Kind = $kind }) '\S' "$kind 有一句话的提示"
    }
    $script:SvSaid.Clear()
    Show-StorageVersionAdvice $fakeCfg @{ Kind = 'no-record'; Text = 'x'; Stored = $null; Expected = 1; UpgradeDone = 0; UpgradeTotal = 0; Watch = @() }
    $text = ($script:SvSaid -join "`n")
    Assert-Match $text 'ops\.html' '说了在哪做升级'
    Assert-Match $text '全部设计库' '说了升级会动全部设计库'
    Assert-Match $text '清空' '说了会清空重建'
    Assert-Match $text 'enabled = false' '说了不要 rs-server 时怎么办'
    Assert-Match $text '改个名' '从头来的办法是改名留底，不是删'

    $script:SvSaid.Clear()
    Show-StorageVersionAdvice $fakeCfg @{ Kind = 'no-record'; Text = 'x'; Stored = $null; Expected = 1; UpgradeDone = 0; UpgradeTotal = 0; Watch = @(7352) }
    $text = ($script:SvSaid -join "`n")
    Assert-Match $text '走不通' '配了 watch_dbnums：说了升级这条路走不通'
    Assert-Match $text 'watch_dbnums' '点了 watch_dbnums 的名'

    $script:SvSaid.Clear()
    Show-StorageVersionAdvice $fakeCfg @{ Kind = 'awaiting-model'; Text = 'x'; Stored = $null; Expected = 1; UpgradeDone = 0; UpgradeTotal = 0; Idle = $true; Watch = @(7352, 8000) }
    $text = ($script:SvSaid -join "`n")
    Assert-Match $text 'stack init 7352 8000' '等初始化：下一步是具体的命令，带着配置里的库号'
    Assert-Match $text '没有等满' '说了为什么不等'
}

Test '初始化失败的提示：不写死后端版本、不劝人删数据库目录' {
    $script:SvSaid.Clear()
    $fakeCfg = @{ Backend = @{ Port = 51022 }; Database = @{ DataDir = 'D:\x\data\surreal' } }
    Show-InitFailure $fakeCfg @(@{ Dbnum = 7352; Message = '首次按需初始化失败: dbnum=7352 仍有 24 行模型面残留'; TaskId = 't1' })
    $text = ($script:SvSaid -join "`n")
    Assert-Match $text '库 7352 没有初始化成' '带出了库号'
    Assert-Match $text '模型面残留' '原样带出了后端给的原因'
    Assert-NotMatch $text '0\.1\.44' '没有写死后端的版本号'
    Assert-NotMatch $text '删掉文件夹|删除文件夹|删掉 data' '没有劝人删数据库目录'
    Assert-Match $text 'stack init 7352' '下一步是具体的命令'
}
