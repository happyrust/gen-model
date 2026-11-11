# 后端在不在忙（down 靠它决定停不停）：按健康检查、队列、任务表判断。
# 全用假的应答——把入口里发请求的那个函数换成「按地址给一段写好的 JSON」，一个请求都不真发。

$script:BusyReplies = @{}
$script:BusyAsked = New-Object System.Collections.ArrayList
function Invoke-StackHttp([string]$Method, [string]$Url, [string]$BodyJson = '', [int]$TimeoutSec = 5) {
    [void]$script:BusyAsked.Add($Url)
    foreach ($key in $script:BusyReplies.Keys) {
        if ($Url.IndexOf($key, [System.StringComparison]::OrdinalIgnoreCase) -lt 0) { continue }
        $body = $script:BusyReplies[$key]
        if ($null -eq $body) { return @{ Ok = $false; Status = 0; Body = ''; Bytes = 0; Error = '假的：没有应答' } }
        return @{ Ok = $true; Status = 200; Body = $body; Bytes = $body.Length; Error = '' }
    }
    return @{ Ok = $false; Status = 0; Body = ''; Bytes = 0; Error = '假的：没有这个地址' }
}
$busyCfg = @{ Backend = @{ Port = 51022 } }
$healthIdle = '{"status":"ok","storage_version":{"expected":1,"stored":1,"caught_up":true,"upgrade":{"state":"idle"}},"initialization":{"status":"model_ready","model_ready":true},"model_cache":{"write_behind":{"pending_unique_roots":0,"in_flight_roots":0,"retrying_roots":0,"blocked_roots":3}}}'

# 摆好三处的应答再问一次。给 $null = 那一处没有应答。（参数不标类型：标成字符串的话 $null 会变成空串。）
function Get-BusyWith($Health, $Queue, $Tasks) {
    $script:BusyReplies = @{ '/api/v1/health' = $Health; '/api/v1/queue' = $Queue; '/api/v1/tasks' = $Tasks }
    $script:BusyAsked.Clear()
    return (Get-BackendBusy $busyCfg)
}

Test '闲着：队列空、没有任务、写后台清空了——不算忙（判死的根不算）' {
    $b = Get-BusyWith $healthIdle '{"rows":[]}' '{"tasks":[],"has_more":false}'
    Assert-True $b.Known '问得到'
    Assert-True (-not $b.Busy) '不忙'
    Assert-Equal 0 @($b.Reasons).Count '没有理由'
}

Test '队列里只有积压（held）的行：没人触发不会跑，不算忙' {
    $b = Get-BusyWith $healthIdle '{"rows":[{"task_id":"t1","dbnum":7997,"intent":"apply_window","phase":"parse","state":"held"}]}' '{"tasks":[]}'
    Assert-True ($b.Known -and -not $b.Busy) '问得到、不忙'
}

Test '有库正在初始化 / 做增量：算忙，说出是哪个库；同一个任务不报两遍' {
    $b = Get-BusyWith $healthIdle '{"rows":[{"task_id":"t1","dbnum":7352,"intent":"apply_window","phase":"parse","state":"running"},{"task_id":"t2","dbnum":8000,"intent":"reinitialize","phase":"model","state":"queued"}]}' '{"tasks":[{"task_id":"t1","kind":"data_batch","state":"running","dbnum":7352}]}'
    Assert-True ($b.Known -and $b.Busy) '问得到、在忙'
    $text = (@($b.Reasons) -join "`n")
    Assert-Match $text '队列里正在跑：库 7352（apply_window，parse）' '在跑的'
    Assert-Match $text '队列里排着、还没跑：库 8000（reinitialize，model）' '在排的'
    Assert-NotMatch $text '有任务正在跑' '队列里那一行在任务表里的那一条不重复报'
}

Test '不走队列的任务（「核对」这类）在跑：算忙' {
    $b = Get-BusyWith $healthIdle '{"rows":[]}' '{"tasks":[{"task_id":"r9","kind":"rvm_check","state":"running"},{"task_id":"m3","kind":"model_rebuild","state":"running","dbnum":7997}]}'
    Assert-True $b.Busy '在忙'
    Assert-Match (@($b.Reasons) -join "`n") '有任务正在跑：rvm_check、model_rebuild（库 7997）' '说出是什么任务'
}

Test '写后台没清空：算忙，三格加起来报' {
    $health = '{"status":"ok","storage_version":{"expected":1,"stored":1,"caught_up":true,"upgrade":{"state":"idle"}},"model_cache":{"write_behind":{"pending_unique_roots":3,"in_flight_roots":2,"retrying_roots":1,"blocked_roots":9}}}'
    $b = Get-BusyWith $health '{"rows":[]}' '{"tasks":[]}'
    Assert-True $b.Busy '在忙'
    Assert-Match (@($b.Reasons) -join "`n") '还有 6 个根没写进数据库' '3 + 2 + 1'
}

Test '库的升级正在做：算忙' {
    $health = '{"status":"ok","storage_version":{"expected":2,"stored":1,"caught_up":false,"upgrade":{"state":"running","requested":[7352,7997,8000],"done":1}},"initialization":{"status":"running","model_ready":false}}'
    $b = Get-BusyWith $health '{"rows":[]}' '{"tasks":[]}'
    Assert-True $b.Busy '在忙'
    Assert-Match (@($b.Reasons) -join "`n") '库的升级正在做（做完 1 个，共 3 个）' '说了进度'
}

Test '设置模式：什么活都没在干，不算忙，也不用再问队列' {
    $b = Get-BusyWith '{"mode":"setup"}' $null $null
    Assert-True ($b.Known -and -not $b.Busy) '问得到、不忙'
    Assert-Equal 1 $script:BusyAsked.Count '只问了健康检查'
}

Test '问不到：健康检查没答，或者队列、任务表有一处没答——都算「判断不了」' {
    $b = Get-BusyWith $null '{"rows":[]}' '{"tasks":[]}'
    Assert-True (-not $b.Known) '健康检查没答'
    $b = Get-BusyWith 'not json' '{"rows":[]}' '{"tasks":[]}'
    Assert-True (-not $b.Known) '健康检查答的不是 JSON'
    $b = Get-BusyWith $healthIdle $null '{"tasks":[]}'
    Assert-True (-not $b.Known) '队列没答'
    $b = Get-BusyWith $healthIdle '{"rows":[]}' $null
    Assert-True (-not $b.Known) '任务表没答'
    Assert-True (-not $b.Busy) '判断不了的时候不说它忙，由调用方按「问不到」处理'
}

Test '库里的版本记录写没写上：跟着报出来（down 在后端忙的时候拿它多提醒一句）' {
    $b = Get-BusyWith $healthIdle '{"rows":[]}' '{"tasks":[]}'
    Assert-True (-not $b.NoRecord) '版本记录在：不标'
    $fresh = '{"status":"ok","storage_version":{"expected":1,"stored":null,"caught_up":false,"upgrade":{"state":"idle"}},"initialization":{"status":"running","model_ready":false},"watch_dbnums":[7352]}'
    $b = Get-BusyWith $fresh '{"rows":[{"task_id":"t1","dbnum":8190,"intent":"apply_window","phase":"meta","state":"running"}]}' '{"tasks":[]}'
    Assert-True ($b.Busy -and $b.NoRecord) '在忙、版本记录还没有：标出来'
    $b = Get-BusyWith '{"mode":"setup"}' $null $null
    Assert-True (-not $b.NoRecord) '设置模式：不标'
    $b = Get-BusyWith '{"status":"ok"}' '{"rows":[]}' '{"tasks":[]}'
    Assert-True (-not $b.NoRecord) '后端还没核对过库版本（健康检查里没有这一节）：不标'
}