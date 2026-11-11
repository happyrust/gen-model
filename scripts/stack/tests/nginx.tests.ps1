# nginx 的配置：/api 一律转给后端（包括以 .json、.png 结尾的地址）；JSON 不长期缓存；带哈希的静态资源长期缓存。
# 前两条只看生成出来的配置文本，哪台机器都能跑。
# 后两条要真起 nginx：给了 -Nginx <nginx.exe> 才跑（它旁边要有 conf\mime.types）。端口现找两个空的，只听本机，用完就停；
# 只动这条测试自己起的进程（nginx 和一个假后端）。

$script:NginxSaid = New-Object System.Collections.ArrayList
function Say([string]$Text = '', [string]$Color = '') { [void]$script:NginxSaid.Add($Text) }

# 摆一个假的安装目录，返回解析好的配置。$NginxExe 为空时用一个不存在的路径（只生成配置够用）。
function New-NginxCfg([string]$NginxExe, [int]$FrontPort, [int]$BackPort, [bool]$Loopback = $true) {
    $root = New-TestDir 'ng'
    Write-TestFile (Join-Path $root 'www\index.html') '<html><body><div id="app"></div>INDEX</body></html>'
    Write-TestFile (Join-Path $root 'www\version.json') '{"version":"t"}'
    Write-TestFile (Join-Path $root 'www\assets\app-abc123.js') 'console.log("asset")'
    Write-TestFile (Join-Path $root 'www\config\model.config.json') '{"static":true}'
    $exe = 'bin/nginx/nginx.exe'
    if ($NginxExe) { $exe = $NginxExe.Replace('\', '/') }
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
port = $BackPort
config_template = "config/DbOption.factory.toml"
resource_dir = "resource"
schema_root = "schema"

[frontend]
kind = "nginx"
exe = "$exe"
root = "www"
port = $FrontPort

[listen]
loopback_only = $("$Loopback".ToLowerInvariant())
"@
    return (Get-StackConfig (Join-Path $root 'stack.toml'))
}

Test '生成的配置：/api 带 ^~；长缓存的规则里没有 json；JSON 单独一条不缓存' {
    $cfg = New-NginxCfg '' 51100 51022
    Initialize-NginxFiles $cfg
    $conf = [System.IO.File]::ReadAllText((Join-Path $cfg.Frontend.WorkDir 'conf\nginx.conf'), [System.Text.Encoding]::UTF8)
    Assert-Match $conf '(?m)^\s*location \^~ /api \{' '/api 是带 ^~ 的前缀规则'
    Assert-NotMatch $conf '(?m)^\s*location /api \{' '没有不带 ^~ 的 /api'
    Assert-Match $conf '(?m)^\s*location \^~ /files/ \{' '/files/ 也带 ^~'
    $longCache = [regex]::Match($conf, '(?m)^\s*location ~\* \\\.\(([a-z0-9|]+)\)\$ \{').Groups[1].Value
    Assert-True ($longCache.Length -gt 0) '找到了按扩展名长缓存的那条规则'
    Assert-True (@($longCache -split '\|') -notcontains 'json') "长缓存的扩展名里没有 json（现在是 $longCache）"
    Assert-True (@($longCache -split '\|') -contains 'js') '长缓存的扩展名里有 js'
    Assert-Match $conf '(?s)location ~\* \\\.json\$ \{[^}]*no-store' 'JSON 单独一条规则，不缓存'
    Assert-Match $conf '(?m)^\s*listen\s+127\.0\.0\.1:51100;' '只许本机时只听回环'
    Assert-Match $conf '(?m)^\s*server 127\.0\.0\.1:51022;' '上游是本机的后端端口'
    Assert-Match $conf ([regex]::Escape('root   "' + $cfg.Frontend.Root.Replace('\', '/') + '";')) '网页目录带引号、用正斜杠（路径里有空格也不怕）'
    Assert-NotMatch $conf '\{\{[A-Z_]+\}\}' '占位符都填上了'

    $cfg2 = New-NginxCfg '' 51100 51022 $false
    Initialize-NginxFiles $cfg2
    $conf2 = [System.IO.File]::ReadAllText((Join-Path $cfg2.Frontend.WorkDir 'conf\nginx.conf'), [System.Text.Encoding]::UTF8)
    Assert-Match $conf2 '(?m)^\s*listen\s+51100;' '缺省听所有网卡'
}

Test 'nginx -t：生成的配置 nginx 自己认' {
    if (-not $script:TestNginx) { Skip-Test '没给 -Nginx <nginx.exe>' }
    $cfg = New-NginxCfg $script:TestNginx (Get-FreeTestPort) (Get-FreeTestPort)
    Initialize-NginxFiles $cfg
    $r = Invoke-Capture $cfg $script:TestNginx ('-t -p "' + $cfg.Frontend.WorkDir.Replace('\', '/') + '/" -c conf/nginx.conf') $cfg.Frontend.WorkDir 30
    Assert-True $r.Ok "nginx -t 通过（它说：$($r.Text)）"
}

Test '真起 nginx：各种地址各自落到哪' {
    if (-not $script:TestNginx) { Skip-Test '没给 -Nginx <nginx.exe>' }
    $frontPort = Get-FreeTestPort
    $backPort = Get-FreeTestPort
    $cfg = New-NginxCfg $script:TestNginx $frontPort $backPort
    Initialize-NginxFiles $cfg
    $backend = $null
    $nginx = $null
    try {
        # 假后端：只听本机，收到什么路径就回「BACKEND <路径>」；60 秒后自己退。
        $hostExe = (Get-Process -Id $PID).Path
        $backend = Start-Process -FilePath $hostExe -ArgumentList ('-NoProfile -ExecutionPolicy Bypass -File "' + (Join-Path $script:TestsDir 'fake-backend.ps1') + '" -Port ' + $backPort + ' -Seconds 60') -WindowStyle Hidden -PassThru
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = $script:TestNginx
        $psi.Arguments = '-p "' + $cfg.Frontend.WorkDir.Replace('\', '/') + '/" -c conf/nginx.conf'
        $psi.WorkingDirectory = $cfg.Frontend.WorkDir
        $psi.UseShellExecute = $false
        $psi.CreateNoWindow = $true
        $nginx = [System.Diagnostics.Process]::Start($psi)
        $front = "http://127.0.0.1:$frontPort"
        $ready = $false
        $deadline = (Get-Date).AddSeconds(20)
        while ((Get-Date) -lt $deadline) {
            $a = Invoke-StackHttp 'GET' "$front/api/ping" '' 2
            if ($a.Ok -and $a.Body -match 'BACKEND') { $ready = $true; break }
            Start-Sleep -Milliseconds 300
        }
        Assert-True $ready 'nginx 和假后端都起来了'

        foreach ($path in '/api/v1/rvm-check/runs/x/files/compare.json', '/api/v1/some/picture.png', '/api/v1/a.js', '/api/v1/health', '/files/review/a.png', '/setup.html') {
            $r = Invoke-StackHttp 'GET' "$front$path" '' 5
            Assert-Equal "BACKEND $path" $r.Body.Trim() "$path 转给了后端"
        }
        $r = Invoke-StackHttp 'GET' "$front/" '' 5
        Assert-Match $r.Body 'INDEX' '/ 是首页'
        $r = Invoke-StackHttp 'GET' "$front/some/spa/route" '' 5
        Assert-Match $r.Body 'INDEX' '页面内的路由落到首页'
        $r = Invoke-StackHttp 'GET' "$front/assets/app-abc123.js" '' 5
        Assert-Match $r.Body 'asset' '静态脚本取得到'
        $r = Invoke-StackHttp 'GET' "$front/config/model.config.json" '' 5
        Assert-Match $r.Body '"static"' '静态 JSON 取得到（不转给后端）'
        $r = Invoke-StackHttp 'GET' "$front/config/no-such.json" '' 5
        Assert-Equal 404 $r.Status '找不到的 JSON 是 404，不落到首页'
        $r = Invoke-StackHttp 'GET' "$front/assets/no-such.js" '' 5
        Assert-Equal 404 $r.Status '找不到的静态脚本是 404'

        # 缓存头：JSON 不缓存，带哈希的脚本长期缓存
        foreach ($case in @(@{ Path = '/config/model.config.json'; Want = 'no-store' }, @{ Path = '/version.json'; Want = 'no-store' }, @{ Path = '/assets/app-abc123.js'; Want = 'immutable' }, @{ Path = '/'; Want = 'no-store' })) {
            $request = [System.Net.WebRequest]::Create("$front$($case.Path)")
            $request.Proxy = $null
            $request.Timeout = 5000
            $response = $request.GetResponse()
            try { $cache = [string]$response.Headers['Cache-Control'] } finally { $response.Close() }
            Assert-Match $cache $case.Want "$($case.Path) 的 Cache-Control（现在是「$cache」）"
        }
    } finally {
        # 收场：只动这里起的两个进程，和 nginx 主进程自己生的工作进程。
        if ($null -ne $nginx) {
            $info = Get-ProcInfo $nginx.Id
            $kids = @()
            if ($null -ne $info) { $kids = @(Get-ChildProcs $nginx.Id (Get-ProcTicks $info) | ForEach-Object { @{ pid = [int]$_.ProcessId; ticks = (Get-ProcTicks $_) } }) }
            try { if (-not $nginx.HasExited) { $nginx.Kill() } } catch { }
            foreach ($kid in $kids) { [void](Stop-VerifiedProcess $kid.pid $kid.ticks) }
        }
        if ($null -ne $backend) { try { if (-not $backend.HasExited) { $backend.Kill() } } catch { } }
    }
}
