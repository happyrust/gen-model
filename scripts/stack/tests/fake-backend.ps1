# 测试用的假后端：只听本机的一个端口，收到什么路径就回「BACKEND <路径>」。到点自己退出。
# 由 nginx.tests.ps1 起，不单独用。
param([Parameter(Mandatory = $true)][int]$Port, [int]$Seconds = 60)
$ErrorActionPreference = 'Stop'
$listener = New-Object System.Net.Sockets.TcpListener -ArgumentList ([System.Net.IPAddress]::Loopback), $Port
$listener.Start()
$deadline = (Get-Date).AddSeconds($Seconds)
try {
    while ((Get-Date) -lt $deadline) {
        if (-not $listener.Pending()) { Start-Sleep -Milliseconds 20; continue }
        $client = $listener.AcceptTcpClient()
        try {
            $client.ReceiveTimeout = 3000
            $stream = $client.GetStream()
            $reader = New-Object System.IO.StreamReader -ArgumentList $stream, ([System.Text.Encoding]::ASCII)
            $requestLine = $reader.ReadLine()
            # 把请求头读完（读到空行）
            while ($true) { $line = $reader.ReadLine(); if ($null -eq $line -or $line -eq '') { break } }
            $path = '/'
            if ($requestLine -match '^\S+\s+(\S+)') { $path = $Matches[1] }
            $body = [System.Text.Encoding]::UTF8.GetBytes("BACKEND $path")
            $head = [System.Text.Encoding]::ASCII.GetBytes("HTTP/1.1 200 OK`r`nContent-Type: text/plain; charset=utf-8`r`nContent-Length: $($body.Length)`r`nConnection: close`r`n`r`n")
            $stream.Write($head, 0, $head.Length)
            $stream.Write($body, 0, $body.Length)
            $stream.Flush()
        } catch { } finally { $client.Close() }
    }
} finally { $listener.Stop() }
