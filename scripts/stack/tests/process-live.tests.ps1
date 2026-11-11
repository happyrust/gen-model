# 在真进程上核对「只认真孩子」「启动时间对不上就不停」「发 Ctrl+C 请它自己退出」。
# 只动这条测试自己起的两个进程：一个 cmd，和它下面的一个 ping（对着本机回环地址，不出网）。

Test '真进程：只认自己起的 cmd 下面的 ping；启动时间对不上不停，对得上才停' {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = Join-Path ([Environment]::GetEnvironmentVariable('SystemRoot')) 'System32\cmd.exe'
    $psi.Arguments = '/d /c "ping -n 120 127.0.0.1 >NUL"'
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $parent = [System.Diagnostics.Process]::Start($psi)
    $childPid = 0
    try {
        $parentInfo = $null
        $kids = @()
        $deadline = (Get-Date).AddSeconds(15)
        while ((Get-Date) -lt $deadline) {
            $parentInfo = Get-ProcInfo $parent.Id
            if ($null -ne $parentInfo) { $kids = @(Get-ChildProcs $parent.Id (Get-ProcTicks $parentInfo) | Where-Object { $_.Name -ieq 'PING.EXE' }) }
            if ($kids.Count -gt 0) { break }
            Start-Sleep -Milliseconds 200
        }
        Assert-Equal 1 $kids.Count '找到了它下面的 ping'
        $childPid = [int]$kids[0].ProcessId
        $parentTicks = Get-ProcTicks $parentInfo

        Assert-Equal 0 @(Get-ChildProcs $parent.Id '630000000000000000').Count '给一个对不上的启动时间：一个孩子都不认'
        Assert-True (-not (Stop-VerifiedProcess $parent.Id '630000000000000000')) '启动时间对不上：不停'
        Assert-True (-not $parent.HasExited) '它还在跑'

        Assert-True (Stop-VerifiedProcess $parent.Id $parentTicks) '启动时间对得上：停了'
        [void]$parent.WaitForExit(5000)
        Assert-True $parent.HasExited 'cmd 已经退出'

        # 父进程退了之后，孩子还记着那个父进程号——这正是「光看父进程号会认错」的来由。这时一个都不该认。
        Assert-Equal 0 @(Get-ChildProcs $parent.Id $parentTicks).Count '父进程不在了：一个都不认'
    } finally {
        if ($childPid -gt 0) {
            $child = Get-ProcInfo $childPid
            if ($null -ne $child -and $child.Name -ieq 'PING.EXE' -and [int]$child.ParentProcessId -eq $parent.Id) { [void](Stop-VerifiedProcess $childPid (Get-ProcTicks $child)) }
        }
        try { if (-not $parent.HasExited) { $parent.Kill() } } catch { }
    }
}

# 给没有窗口的进程发 Ctrl+C（down 请数据库自己退出用的那条路），在真进程上走一遍。
# 同样只动这条测试自己起的两个进程：一个没有窗口的 cmd，和它下面的一个 ping（ping 收到 Ctrl+C 会自己退出）。
Test '真进程：发 Ctrl+C——名单不全不发、启动时间对不上不发；名单全了它自己退出' {
    $root = New-TestDir 'ctrl'
    Write-TestFile (Join-Path $root 'stack.toml') @"
[project]
e3d_root = "e3d"
name = "Sample"
code = 1516
e3d_version = "e3d31"

[database]
exe = "bin/surreal.exe"
port = 51009

[backend]
exe = "bin/aios-database.exe"
port = 51022
config_template = "config/DbOption.factory.toml"
resource_dir = "resource"
schema_root = "schema"

[frontend]
enabled = false
"@
    $liveCfg = Get-StackConfig (Join-Path $root 'stack.toml')
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = Join-Path ([Environment]::GetEnvironmentVariable('SystemRoot')) 'System32\cmd.exe'
    $psi.Arguments = '/d /c "ping -n 120 127.0.0.1 <NUL >NUL"'
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $parent = [System.Diagnostics.Process]::Start($psi)
    $childPid = 0
    try {
        $kids = @()
        $deadline = (Get-Date).AddSeconds(15)
        while ((Get-Date) -lt $deadline) {
            $parentInfo = Get-ProcInfo $parent.Id
            if ($null -ne $parentInfo) { $kids = @(Get-ChildProcs $parent.Id (Get-ProcTicks $parentInfo) | Where-Object { $_.Name -ieq 'PING.EXE' }) }
            if ($kids.Count -gt 0) { break }
            Start-Sleep -Milliseconds 200
        }
        Assert-Equal 1 $kids.Count '找到了它下面的 ping'
        $childPid = [int]$kids[0].ProcessId
        $ticks = Get-ProcTicks $kids[0]

        $r = Send-ConsoleCtrl $liveCfg $childPid $ticks @($childPid)
        Assert-True (-not $r.Sent) '名单里没有它上面的 cmd：不发'
        Assert-Match $r.Why "还有别的进程（进程号 $($parent.Id)）" '说出控制台上还有谁'
        Start-Sleep -Milliseconds 500
        Assert-True ($null -ne (Get-ProcInfo $childPid)) 'ping 还在'

        $r = Send-ConsoleCtrl $liveCfg $childPid '630000000000000000' @($childPid, $parent.Id)
        Assert-True (-not $r.Sent) '启动时间对不上：不发'
        Assert-Match $r.Why '已经不在了，或者这个进程号换了主人' '说的是进程换了主人'
        Assert-True ($null -ne (Get-ProcInfo $childPid)) 'ping 还在'

        $r = Send-ConsoleCtrl $liveCfg $childPid $ticks @($childPid, $parent.Id)
        Assert-True $r.Sent "名单全了：发出去了（$($r.Why)）"
        $deadline = (Get-Date).AddSeconds(15)
        while ($null -ne (Get-ProcInfo $childPid) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 200 }
        Assert-True ($null -eq (Get-ProcInfo $childPid)) 'ping 自己退出了'
        [void]$parent.WaitForExit(5000)
        Assert-True $parent.HasExited '它上面的 cmd 也退了'

        $r = Send-ConsoleCtrl $liveCfg $childPid $ticks @($childPid, $parent.Id)
        Assert-True (-not $r.Sent) '进程已经不在：不发'
    } finally {
        if ($childPid -gt 0) {
            $child = Get-ProcInfo $childPid
            if ($null -ne $child -and $child.Name -ieq 'PING.EXE' -and [int]$child.ParentProcessId -eq $parent.Id) { [void](Stop-VerifiedProcess $childPid (Get-ProcTicks $child)) }
        }
        try { if (-not $parent.HasExited) { $parent.Kill() } } catch { }
    }
}