# 在真进程上核对「只认真孩子」「启动时间对不上就不停」。
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
