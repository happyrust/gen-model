<#
.SYNOPSIS
    stack —— 读一份配置，把数据库、后端、rs-server、前端按顺序起起来，起一个查一个。

.DESCRIPTION
    用法见 `stack help`。

    这份脚本按 Windows PowerShell 5.1 能跑来写：不用三元运算、`??`、`?.`、`&&`、`||`。
    文件要存成带 BOM 的 UTF-8——5.1 把没有 BOM 的脚本当本地编码读，中文会乱。

    几条写死的纪律（改之前先想清楚）：
      * 停进程只按进程号，而且先核对「进程号 + 启动时间 + 程序路径（开发档的前端是命令行里的标记）」；
        从不按进程名停。连带收子进程时只认「创建时间不早于父进程」的（进程号会重用，光看父进程号会认错），
        占着受保护端口的进程一律不动。
      * 状态文件里没有记的进程当别人的——只有一种例外会被认领回来：它的程序就是运行目录里的那一个、
        又占着本套配置的端口（后端在向导页里自己重启、或者 up 跑到一半被关掉，就会出现这种进程）。
      * 数据库账号口令只在真正起数据库的那一小段里从后端配置读出来、经环境变量递下去；
        不打印、不上命令行、不进状态文件和日志。
      * 每个服务的环境变量由这里明确给出，不继承调用方的环境。
      * 缺省不初始化任何库、不生成模型：只有配置里点了名，或者运行 `stack init`，才去初始化。
#>
[CmdletBinding(PositionalBinding = $false)]
param(
    [Parameter(Position = 0)][string]$Command = 'help',
    [string]$Config = '',
    [string]$Service = '',
    [switch]$WhatIf,
    [switch]$DryRun,
    [switch]$NoInit,
    [int]$Seconds = 120,
    [int]$Level = 2,
    [string]$Node = '',
    [switch]$Force,
    # 命令后面不带名字的参数（`stack init 7997 8000` 里的库号）和不认识的开关都落在这里。
    [Parameter(ValueFromRemainingArguments = $true)][string[]]$Rest
)

$ErrorActionPreference = 'Stop'
$script:StackVersion = '0.3.0'
$script:LogFile = $null
$script:ExitCode = 0
$script:ListenCache = $null
$script:ListenCacheAt = [datetime]::MinValue
$script:LockStream = $null
$script:Utf8NoBom = New-Object System.Text.UTF8Encoding -ArgumentList $false

# 输出被接到管道或文件时按 UTF-8 写；直接显示在窗口里时不动控制台的代码页
# （老系统的控制台切到 UTF-8 之后中文会显示成方块）。
try {
    $redirected = $true
    try { $redirected = [Console]::IsOutputRedirected } catch { $redirected = $true }
    if ($redirected) { [Console]::OutputEncoding = $script:Utf8NoBom }
} catch { }

# ─────────────────────────── 输出 ───────────────────────────

function Write-StackLog([string]$Text) {
    if (-not $script:LogFile) { return }
    try {
        $stamp = (Get-Date).ToString('yyyy-MM-dd HH:mm:ss')
        [System.IO.File]::AppendAllText($script:LogFile, "$stamp  $Text`r`n", $script:Utf8NoBom)
    } catch { }
}

function Say([string]$Text = '', [string]$Color = '') {
    if ($Color) { Write-Host $Text -ForegroundColor $Color } else { Write-Host $Text }
    Write-StackLog $Text
}

function Get-DisplayWidth([string]$Text) {
    $width = 0
    foreach ($ch in $Text.ToCharArray()) {
        if ([int]$ch -gt 0x2E7F) { $width += 2 } else { $width += 1 }
    }
    return $width
}

function Format-Cell([string]$Text, [int]$Width) {
    $pad = $Width - (Get-DisplayWidth $Text)
    if ($pad -lt 1) { $pad = 1 }
    return $Text + (' ' * $pad)
}

function Limit-Text([string]$Text, [int]$Max) {
    if ($null -eq $Text) { return '' }
    if ($Text.Length -le $Max) { return $Text }
    return $Text.Substring(0, $Max) + ' …'
}

# 抛一个带退出码的错误。主流程末尾统一接住、打印、退出。
# -Quiet：该说的前面已经说过了，末尾不再重复打印这一句。
function Stop-Stack([string]$Message, [int]$Code = 1, [switch]$Quiet) {
    $ex = New-Object System.Exception -ArgumentList $Message
    $ex.Data['StackExitCode'] = $Code
    $ex.Data['StackQuiet'] = [bool]$Quiet
    throw $ex
}

function New-Dir([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { New-Item -ItemType Directory -Force -Path $Path | Out-Null }
}

# 同一个运行目录上，同一时刻只许一个会起停进程的 stack 命令（up / down / prepare）。
# 用「独占打开一个文件」来占位：进程一结束系统就放开，不会留下过期的锁。
function Enter-StackLock($Cfg) {
    New-Dir $Cfg.RunDir
    $file = Join-Path $Cfg.RunDir 'stack.lock'
    try {
        $script:LockStream = New-Object System.IO.FileStream -ArgumentList $file, ([System.IO.FileMode]::OpenOrCreate), ([System.IO.FileAccess]::ReadWrite), ([System.IO.FileShare]::None)
    } catch {
        Stop-Stack "另一个 stack 命令（up、down 或 prepare）正在这个运行目录上跑（$file 被占着）。等它结束再来" 3
    }
}

# ─────────────────────────── 配置解析（TOML 的一个子集） ───────────────────────────
# 认：注释、[表] 和 [表.子表]、键 = 值；值可以是 "字符串"、'字符串'、整数、true / false、写在一行里的数组。
# 不认的写法一律报错并指出行号，不悄悄忽略。

function Read-TomlValue([string]$Text, [int]$LineNo) {
    $s = $Text.TrimStart()
    if ($s.Length -eq 0) { Stop-Stack "配置第 $LineNo 行：等号后面没有值" 2 }
    $first = $s[0]
    if ($first -eq '"') {
        if ($s.StartsWith('"""')) { Stop-Stack "配置第 $LineNo 行：不支持多行字符串（三个引号）" 2 }
        $sb = New-Object System.Text.StringBuilder
        $i = 1
        while ($true) {
            if ($i -ge $s.Length) { Stop-Stack "配置第 $LineNo 行：双引号没有闭合" 2 }
            $ch = $s[$i]
            if ($ch -eq '"') { break }
            if ($ch -eq '\') {
                if ($i + 1 -ge $s.Length) { Stop-Stack "配置第 $LineNo 行：双引号字符串以反斜杠结尾" 2 }
                $next = $s[$i + 1]
                if ($next -eq '\') { [void]$sb.Append('\') }
                elseif ($next -eq '"') { [void]$sb.Append('"') }
                elseif ($next -eq 'n') { [void]$sb.Append("`n") }
                elseif ($next -eq 't') { [void]$sb.Append("`t") }
                elseif ($next -eq 'r') { [void]$sb.Append("`r") }
                else { Stop-Stack "配置第 $LineNo 行：双引号字符串里有不认识的转义 \$next。Windows 路径请写成正斜杠（D:/a/b），或者用单引号（'D:\a\b'）" 2 }
                $i += 2
                continue
            }
            [void]$sb.Append($ch)
            $i += 1
        }
        return @{ Value = $sb.ToString(); Rest = $s.Substring($i + 1) }
    }
    if ($first -eq "'") {
        if ($s.StartsWith("'''")) { Stop-Stack "配置第 $LineNo 行：不支持多行字符串（三个引号）" 2 }
        $end = $s.IndexOf("'", 1)
        if ($end -lt 0) { Stop-Stack "配置第 $LineNo 行：单引号没有闭合" 2 }
        return @{ Value = $s.Substring(1, $end - 1); Rest = $s.Substring($end + 1) }
    }
    if ($first -eq '[') {
        $items = New-Object System.Collections.ArrayList
        $rest = $s.Substring(1).TrimStart()
        while ($true) {
            if ($rest.Length -eq 0) { Stop-Stack "配置第 $LineNo 行：数组要写在一行里，这一行没有看到右方括号" 2 }
            if ($rest[0] -eq ']') { $rest = $rest.Substring(1); break }
            $item = Read-TomlValue $rest $LineNo
            if ($item.Value -is [System.Array]) { Stop-Stack "配置第 $LineNo 行：不支持数组套数组" 2 }
            [void]$items.Add($item.Value)
            $rest = $item.Rest.TrimStart()
            if ($rest.Length -gt 0 -and $rest[0] -eq ',') { $rest = $rest.Substring(1).TrimStart(); continue }
            if ($rest.Length -gt 0 -and $rest[0] -eq ']') { $rest = $rest.Substring(1); break }
            Stop-Stack "配置第 $LineNo 行：数组元素之间要用逗号隔开，并且整个数组写在一行里" 2
        }
        return @{ Value = $items.ToArray(); Rest = $rest }
    }
    if ($first -eq '{') { Stop-Stack "配置第 $LineNo 行：不支持行内表（花括号）。请改成单独的一节 [表名]" 2 }
    $m = [regex]::Match($s, '^(true|false)(?![A-Za-z0-9_])')
    if ($m.Success) { return @{ Value = ($m.Groups[1].Value -eq 'true'); Rest = $s.Substring($m.Length) } }
    $m = [regex]::Match($s, '^[+-]?[0-9][0-9_]*(?![A-Za-z0-9_.:\-])')
    if ($m.Success) {
        $number = [long]($m.Value.Replace('_', ''))
        if ($number -ge [int]::MinValue -and $number -le [int]::MaxValue) { $number = [int]$number }
        return @{ Value = $number; Rest = $s.Substring($m.Length) }
    }
    $shown = ($s -split '\s+#')[0].Trim()
    Stop-Stack "配置第 $LineNo 行：不认识的值「$(Limit-Text $shown 60)」。字符串要加引号；只支持字符串、整数、true / false 和一行写完的数组" 2
}

function Read-StackToml([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { Stop-Stack "找不到配置文件：$Path。从 stack.example.toml 拷一份改，或用 -Config 指明" 2 }
    $root = [ordered]@{}
    $current = $root
    $currentName = ''
    $lineNo = 0
    # 每个键、每一节写在第几行，记下来；后面报「不认识的键」「类型不对」时好指出行号。
    $script:TomlLines = @{}
    foreach ($raw in [System.IO.File]::ReadAllLines($Path, [System.Text.Encoding]::UTF8)) {
        $lineNo += 1
        $line = $raw
        if ($lineNo -eq 1 -and $line.Length -gt 0 -and [int]$line[0] -eq 0xFEFF) { $line = $line.Substring(1) }
        $line = $line.Trim()
        if ($line.Length -eq 0 -or $line.StartsWith('#')) { continue }
        if ($line.StartsWith('[[')) { Stop-Stack "配置第 $lineNo 行：不支持表数组（双方括号）" 2 }
        if ($line.StartsWith('[')) {
            $m = [regex]::Match($line, '^\[\s*([A-Za-z0-9_\-]+(?:\.[A-Za-z0-9_\-]+)*)\s*\]\s*(#.*)?$')
            if (-not $m.Success) { Stop-Stack "配置第 $lineNo 行：表头写法不对「$line」。应当像 [backend] 或 [backend.env]" 2 }
            $currentName = $m.Groups[1].Value
            $current = $root
            $prefix = ''
            foreach ($part in $currentName.Split('.')) {
                if (-not $script:TomlLines.ContainsKey("$prefix|$part")) { $script:TomlLines["$prefix|$part"] = $lineNo }
                if (-not $current.Contains($part)) { $current[$part] = [ordered]@{} }
                elseif (-not ($current[$part] -is [System.Collections.IDictionary])) { Stop-Stack "配置第 $lineNo 行：「$part」前面已经是一个键，不能再当表名" 2 }
                $current = $current[$part]
                if ($prefix) { $prefix = "$prefix.$part" } else { $prefix = $part }
            }
            continue
        }
        $m = [regex]::Match($line, '^([A-Za-z0-9_\-]+)\s*=(.*)$')
        if (-not $m.Success) {
            if ($line -match '^[A-Za-z0-9_\-]+(\.[A-Za-z0-9_\-]+)+\s*=') { Stop-Stack "配置第 $lineNo 行：不支持带点的键「$(Limit-Text $line 60)」。请写成单独的一节" 2 }
            Stop-Stack "配置第 $lineNo 行：看不懂「$(Limit-Text $line 60)」。应当是 键 = 值、[表名]，或以 # 开头的注释" 2
        }
        $key = $m.Groups[1].Value
        if ($current.Contains($key)) {
            $where = '顶层'
            if ($currentName) { $where = "[$currentName]" }
            Stop-Stack "配置第 $lineNo 行：$where 里「$key」写了两次" 2
        }
        $parsed = Read-TomlValue $m.Groups[2].Value $lineNo
        $rest = $parsed.Rest.Trim()
        if ($rest.Length -gt 0 -and -not $rest.StartsWith('#')) { Stop-Stack "配置第 $lineNo 行：值后面多出了「$(Limit-Text $rest 60)」" 2 }
        $current[$key] = $parsed.Value
        $script:TomlLines["$currentName|$key"] = $lineNo
    }
    return $root
}

# 「第 N 行：」——某一节里某个键（或某一节本身）写在配置的第几行；查不到就返回空。
function Get-LineText([string]$TableName, [string]$Key) {
    $name = $TableName
    if ($name -eq '顶层') { $name = '' }
    $lookup = "$name|$Key"
    if ($null -ne $script:TomlLines -and $script:TomlLines.ContainsKey($lookup)) { return "第 $($script:TomlLines[$lookup]) 行：" }
    return ''
}

function Get-Opt($Table, [string]$Key, $Default = $null) {
    if ($null -ne $Table -and ($Table -is [System.Collections.IDictionary]) -and $Table.Contains($Key)) { return $Table[$Key] }
    return $Default
}

function Resolve-StackPath([string]$Value, [string]$Base) {
    if (-not $Value) { return '' }
    $v = $Value.Replace('/', '\')
    if (-not [System.IO.Path]::IsPathRooted($v)) { $v = Join-Path $Base $v }
    $full = [System.IO.Path]::GetFullPath($v)
    if ($full.Length -gt 3) { $full = $full.TrimEnd('\') }
    return $full
}

function Test-UnderPath([string]$Path, [string]$Root) {
    $p = [System.IO.Path]::GetFullPath($Path).TrimEnd('\') + '\'
    $r = [System.IO.Path]::GetFullPath($Root).TrimEnd('\') + '\'
    return $p.StartsWith($r, [System.StringComparison]::OrdinalIgnoreCase)
}

function Assert-KnownKeys($Table, [string]$TableName, [string[]]$Known) {
    if ($null -eq $Table) { return }
    if (-not ($Table -is [System.Collections.IDictionary])) { Stop-Stack "配置里「$TableName」应当是一节（[$TableName]），不是一个值" 2 }
    foreach ($key in $Table.Keys) {
        if ($Known -contains $key) { continue }
        $at = Get-LineText $TableName $key
        if ($TableName -eq '顶层') { Stop-Stack "配置${at}不认识「$key」这一节（或顶层的键）。认识的节有：$($Known -join '、')" 2 }
        Stop-Stack "配置${at}[$TableName] 里有不认识的键「$key」。认识的有：$($Known -join '、')" 2
    }
}

function ConvertTo-IntArray($Value, [string]$What) {
    $out = @()
    if ($null -eq $Value) { return ,$out }
    foreach ($item in @($Value)) {
        if (-not ($item -is [int])) { Stop-Stack "配置 $What 里要写整数，看到的是「$item」" 2 }
        $out += [int]$item
    }
    return ,$out
}

function ConvertTo-EnvTable($Table, [string]$TableName) {
    $out = [ordered]@{}
    if ($null -eq $Table) { return $out }
    # 写成了「env = "…"」这样的一个值：不能悄悄当成没写。
    if (-not ($Table -is [System.Collections.IDictionary])) { Stop-Stack "配置里 $TableName 应当是单独的一节（[$TableName]，里面一行一个「变量名 = `"值`"」），不是一个值" 2 }
    foreach ($key in $Table.Keys) {
        if ($key -notmatch '^[A-Za-z_][A-Za-z0-9_]*$') { Stop-Stack "配置 [$TableName] 里的环境变量名「$key」不合法" 2 }
        if ($key -match '(?i)PASS|PWD|TOKEN|SECRET|_KEY$') { Stop-Stack "配置 [$TableName] 里不要放口令类的环境变量（$key）。数据库口令只从后端配置里读" 2 }
        if ($Table[$key] -is [System.Array]) { Stop-Stack "配置 [$TableName] 里 $key 的值要写成字符串" 2 }
        $out[$key] = [string]$Table[$key]
    }
    return $out
}

function Assert-Type($Table, [string]$TableName, [string]$Key, [string]$Kind) {
    $value = Get-Opt $Table $Key
    if ($null -eq $value) { return }
    $at = Get-LineText $TableName $Key
    $what = "${at}[$TableName] $Key"
    if ($Kind -eq 'int' -and -not ($value -is [int])) { Stop-Stack "配置$what 要写整数（不带引号），现在是「$value」" 2 }
    if ($Kind -eq 'bool' -and -not ($value -is [bool])) { Stop-Stack "配置$what 要写 true 或 false（不带引号），现在是「$value」" 2 }
    if ($Kind -eq 'string' -and -not ($value -is [string])) { Stop-Stack "配置$what 要写成带引号的字符串，现在是「$value」" 2 }
}

function Assert-Required($Table, [string]$TableName, [string[]]$Keys, [string]$Hint = '') {
    foreach ($key in $Keys) {
        if ($null -eq (Get-Opt $Table $key)) { Stop-Stack "配置 [$TableName] 缺必填项「$key」$Hint" 2 }
    }
}

function Get-StackConfig([string]$Path) {
    $configPath = [System.IO.Path]::GetFullPath($Path)
    $raw = Read-StackToml $configPath
    $configDir = Split-Path -Parent $configPath
    Assert-KnownKeys $raw '顶层' @('stack', 'project', 'database', 'backend', 'rs_server', 'frontend', 'listen', 'init', 'guard', 'timeouts')

    $stack = Get-Opt $raw 'stack'
    Assert-KnownKeys $stack 'stack' @('name', 'root', 'private_temp')
    foreach ($k in 'name', 'root') { Assert-Type $stack 'stack' $k 'string' }
    Assert-Type $stack 'stack' 'private_temp' 'bool'
    # 相对路径的规矩：root 相对配置文件所在的目录；其余所有路径相对 root。
    $root = Resolve-StackPath ([string](Get-Opt $stack 'root' '.')) $configDir

    $project = Get-Opt $raw 'project'
    Assert-KnownKeys $project 'project' @('e3d_root', 'name', 'code', 'mdb', 'e3d_version')
    Assert-Required $project 'project' @('e3d_root', 'name', 'code', 'e3d_version')
    foreach ($k in 'e3d_root', 'name', 'mdb', 'e3d_version') { Assert-Type $project 'project' $k 'string' }
    Assert-Type $project 'project' 'code' 'int'
    $e3dVersion = [string]$project['e3d_version']
    if ('e3d31', 'e3d210' -notcontains $e3dVersion) { Stop-Stack "配置$(Get-LineText 'project' 'e3d_version')[project] e3d_version 只能是 e3d31 或 e3d210，现在是「$e3dVersion」" 2 }

    $listen = Get-Opt $raw 'listen'
    Assert-KnownKeys $listen 'listen' @('loopback_only')
    Assert-Type $listen 'listen' 'loopback_only' 'bool'

    $database = Get-Opt $raw 'database'
    Assert-KnownKeys $database 'database' @('exe', 'host', 'port', 'data_dir', 'env')
    Assert-Required $database 'database' @('exe', 'port')
    foreach ($k in 'exe', 'host', 'data_dir') { Assert-Type $database 'database' $k 'string' }
    Assert-Type $database 'database' 'port' 'int'

    $backend = Get-Opt $raw 'backend'
    Assert-KnownKeys $backend 'backend' @('exe', 'port', 'work_dir', 'config_template', 'resource_dir', 'schema_root', 'plant_ui_web_root', 'rvm_check_dir', 'watch_dbnums', 'env', 'config_set')
    Assert-Required $backend 'backend' @('exe', 'port', 'config_template', 'resource_dir', 'schema_root')
    foreach ($k in 'exe', 'work_dir', 'config_template', 'resource_dir', 'schema_root', 'plant_ui_web_root', 'rvm_check_dir') { Assert-Type $backend 'backend' $k 'string' }
    Assert-Type $backend 'backend' 'port' 'int'

    # rs-server：可选。有 [rs_server] 这一节就算启用，除非写了 enabled = false。
    $rs = Get-Opt $raw 'rs_server'
    Assert-KnownKeys $rs 'rs_server' @('enabled', 'exe', 'port', 'template_dir', 'work_dir', 'wait_storage_version', 'public_url', 'plant_ui_config', 'cors_origins', 'env')
    foreach ($k in 'enabled', 'wait_storage_version', 'plant_ui_config') { Assert-Type $rs 'rs_server' $k 'bool' }
    $rsEnabled = [bool](Get-Opt $rs 'enabled' ($null -ne $rs))
    if ($rsEnabled) {
        Assert-Required $rs 'rs_server' @('exe', 'port', 'template_dir') '（不想带 rs-server 就写 enabled = false）'
        foreach ($k in 'exe', 'template_dir', 'work_dir', 'public_url') { Assert-Type $rs 'rs_server' $k 'string' }
        Assert-Type $rs 'rs_server' 'port' 'int'
    }
    $rsCors = @()
    foreach ($origin in @(Get-Opt $rs 'cors_origins' @())) {
        if (-not ($origin -is [string]) -or $origin -notmatch '^https?://[^/\s]+$') { Stop-Stack "配置$(Get-LineText 'rs_server' 'cors_origins')[rs_server] cors_origins 里每一项要写成「http://主机:端口」这样（不带路径），看到的是「$origin」" 2 }
        $rsCors += [string]$origin
    }

    # 前端两种形态：nginx（包里的构建产物 + nginx）和 vite-dev（开发机上的前端开发服务器）。
    $frontend = Get-Opt $raw 'frontend'
    Assert-KnownKeys $frontend 'frontend' @('enabled', 'kind', 'root', 'node', 'port', 'exe', 'conf_template', 'env')
    Assert-Type $frontend 'frontend' 'enabled' 'bool'
    $frontendEnabled = [bool](Get-Opt $frontend 'enabled' ($null -ne $frontend))
    $frontendKind = [string](Get-Opt $frontend 'kind' 'vite-dev')
    if ($frontendEnabled) {
        Assert-Required $frontend 'frontend' @('root', 'port') '（不想带前端就写 enabled = false）'
        foreach ($k in 'kind', 'root', 'node', 'exe', 'conf_template') { Assert-Type $frontend 'frontend' $k 'string' }
        Assert-Type $frontend 'frontend' 'port' 'int'
        if ('vite-dev', 'nginx' -notcontains $frontendKind) { Stop-Stack "配置$(Get-LineText 'frontend' 'kind')[frontend] kind 只能是 nginx（构建好的前端产物 + nginx）或 vite-dev（前端开发服务器），现在是「$frontendKind」" 2 }
        if ($frontendKind -eq 'nginx') { Assert-Required $frontend 'frontend' @('exe') '（kind = "nginx" 时要写明 nginx.exe 的位置）' }
    }

    $init = Get-Opt $raw 'init'
    Assert-KnownKeys $init 'init' @('dbnums', 'auto_new')
    Assert-Type $init 'init' 'auto_new' 'bool'

    $guard = Get-Opt $raw 'guard'
    Assert-KnownKeys $guard 'guard' @('min_commit_free_gb', 'abort_commit_free_gb', 'max_backend_gb', 'auto_down', 'wait_lock_file', 'wait_lock_timeout_sec', 'protect_ports', 'require_exe_under_root', 'min_disk_free_gb')
    foreach ($k in 'min_commit_free_gb', 'abort_commit_free_gb', 'max_backend_gb', 'wait_lock_timeout_sec', 'min_disk_free_gb') { Assert-Type $guard 'guard' $k 'int' }
    foreach ($k in 'auto_down', 'require_exe_under_root') { Assert-Type $guard 'guard' $k 'bool' }
    Assert-Type $guard 'guard' 'wait_lock_file' 'string'

    $timeouts = Get-Opt $raw 'timeouts'
    Assert-KnownKeys $timeouts 'timeouts' @('database_sec', 'backend_sec', 'rs_server_sec', 'storage_version_sec', 'frontend_sec', 'init_sec')
    foreach ($k in 'database_sec', 'backend_sec', 'rs_server_sec', 'storage_version_sec', 'frontend_sec', 'init_sec') {
        Assert-Type $timeouts 'timeouts' $k 'int'
        $seconds = Get-Opt $timeouts $k
        if ($null -ne $seconds -and $seconds -lt 1) { Stop-Stack "配置$(Get-LineText 'timeouts' $k)[timeouts] $k 要写成不小于 1 的整数（单位是秒），现在是「$seconds」" 2 }
    }

    $backendWork = Resolve-StackPath ([string](Get-Opt $backend 'work_dir' 'work/backend')) $root
    $frontendWork = Join-Path $root 'work\frontend'
    if ($frontendKind -eq 'nginx') { $frontendWork = Join-Path $root 'work\nginx' }

    $nodeExe = ''
    if ($frontendEnabled -and $frontendKind -eq 'vite-dev') {
        $nodeValue = [string](Get-Opt $frontend 'node' 'node')
        if ($nodeValue -eq 'node' -or $nodeValue -eq '') {
            $found = Get-Command node -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
            if ($found) { $nodeExe = $found.Path }
        } else {
            $nodeExe = Resolve-StackPath $nodeValue $root
        }
    }

    $watch = $null
    if ($null -ne (Get-Opt $backend 'watch_dbnums')) { $watch = ConvertTo-IntArray $backend['watch_dbnums'] '[backend] watch_dbnums' }

    $configSet = [ordered]@{}
    $rawSet = Get-Opt $backend 'config_set'
    if ($null -ne $rawSet) {
        if (-not ($rawSet -is [System.Collections.IDictionary])) { Stop-Stack '配置里 backend.config_set 应当是一节（[backend.config_set]）' 2 }
        foreach ($key in $rawSet.Keys) {
            if ($key -match '(?i)pass|user|secret|token') { Stop-Stack "配置 [backend.config_set] 里不要放账号口令类的键（$key）。入口不经手后端配置里的账号口令" 2 }
            $configSet[$key] = $rawSet[$key]
        }
    }

    $cfg = @{
        ConfigPath = $configPath
        Name = [string](Get-Opt $stack 'name' 'stack')
        Root = $root
        PrivateTemp = [bool](Get-Opt $stack 'private_temp' $true)
        RunDir = Join-Path $root 'run'
        LogsDir = Join-Path $root 'logs'
        TmpDir = Join-Path $root 'tmp'
        StateFile = Join-Path $root 'run\state.json'
        Project = @{
            E3dRoot = Resolve-StackPath ([string]$project['e3d_root']) $root
            Name = [string]$project['name']
            Code = [int]$project['code']
            Mdb = [string](Get-Opt $project 'mdb' 'ALL')
            E3dVersion = $e3dVersion
        }
        LoopbackOnly = [bool](Get-Opt $listen 'loopback_only' $false)
        Database = @{
            Exe = Resolve-StackPath ([string]$database['exe']) $root
            Host = [string](Get-Opt $database 'host' '127.0.0.1')
            Port = [int]$database['port']
            DataDir = Resolve-StackPath ([string](Get-Opt $database 'data_dir' 'data/surreal')) $root
            Env = ConvertTo-EnvTable (Get-Opt $database 'env') 'database.env'
        }
        Backend = @{
            Exe = Resolve-StackPath ([string]$backend['exe']) $root
            Port = [int]$backend['port']
            WorkDir = $backendWork
            ConfigFile = Join-Path $backendWork 'DbOption.toml'
            ConfigTemplate = Resolve-StackPath ([string]$backend['config_template']) $root
            ResourceDir = Resolve-StackPath ([string]$backend['resource_dir']) $root
            SchemaRoot = Resolve-StackPath ([string]$backend['schema_root']) $root
            PlantUiWebRoot = Resolve-StackPath ([string](Get-Opt $backend 'plant_ui_web_root' '')) $root
            # 「核对」（RVM 比对）的产物放哪。缺省在数据目录下面；写成空串 = 入口不管这一项。
            RvmCheckDir = Resolve-StackPath ([string](Get-Opt $backend 'rvm_check_dir' 'data/rvm-check')) $root
            WatchDbnums = $watch
            Env = ConvertTo-EnvTable (Get-Opt $backend 'env') 'backend.env'
            ConfigSet = $configSet
        }
        RsServer = @{
            Enabled = $rsEnabled
            Exe = Resolve-StackPath ([string](Get-Opt $rs 'exe' '')) $root
            Port = [int](Get-Opt $rs 'port' 0)
            WorkDir = Resolve-StackPath ([string](Get-Opt $rs 'work_dir' 'work/rs-server')) $root
            TemplateDir = Resolve-StackPath ([string](Get-Opt $rs 'template_dir' '')) $root
            WaitStorageVersion = [bool](Get-Opt $rs 'wait_storage_version' $true)
            PublicUrl = ([string](Get-Opt $rs 'public_url' '')).TrimEnd('/')
            PlantUiConfig = [bool](Get-Opt $rs 'plant_ui_config' $true)
            CorsOrigins = $rsCors
            Env = ConvertTo-EnvTable (Get-Opt $rs 'env') 'rs_server.env'
        }
        Frontend = @{
            Enabled = $frontendEnabled
            Kind = $frontendKind
            Root = Resolve-StackPath ([string](Get-Opt $frontend 'root' '')) $root
            Node = $nodeExe
            Exe = Resolve-StackPath ([string](Get-Opt $frontend 'exe' '')) $root
            ConfTemplate = Resolve-StackPath ([string](Get-Opt $frontend 'conf_template' '')) $root
            Port = [int](Get-Opt $frontend 'port' 0)
            WorkDir = $frontendWork
            WrapperConfig = Join-Path $frontendWork 'vite.stack.config.mjs'
            CacheDir = Join-Path $frontendWork 'vite-cache'
            Env = ConvertTo-EnvTable (Get-Opt $frontend 'env') 'frontend.env'
        }
        Init = @{
            Dbnums = ConvertTo-IntArray (Get-Opt $init 'dbnums') '[init] dbnums'
            AutoNew = [bool](Get-Opt $init 'auto_new' $false)
        }
        Guard = @{
            MinCommitFreeGb = [int](Get-Opt $guard 'min_commit_free_gb' 0)
            AbortCommitFreeGb = [int](Get-Opt $guard 'abort_commit_free_gb' 0)
            MaxBackendGb = [int](Get-Opt $guard 'max_backend_gb' 0)
            AutoDown = [bool](Get-Opt $guard 'auto_down' $false)
            WaitLockFile = Resolve-StackPath ([string](Get-Opt $guard 'wait_lock_file' '')) $root
            WaitLockTimeoutSec = [int](Get-Opt $guard 'wait_lock_timeout_sec' 600)
            ProtectPorts = ConvertTo-IntArray (Get-Opt $guard 'protect_ports') '[guard] protect_ports'
            RequireExeUnderRoot = [bool](Get-Opt $guard 'require_exe_under_root' $true)
            MinDiskFreeGb = [int](Get-Opt $guard 'min_disk_free_gb' 2)
        }
        Timeouts = @{
            DatabaseSec = [int](Get-Opt $timeouts 'database_sec' 30)
            BackendSec = [int](Get-Opt $timeouts 'backend_sec' 180)
            RsServerSec = [int](Get-Opt $timeouts 'rs_server_sec' 120)
            StorageVersionSec = [int](Get-Opt $timeouts 'storage_version_sec' 180)
            FrontendSec = [int](Get-Opt $timeouts 'frontend_sec' 90)
            InitSec = [int](Get-Opt $timeouts 'init_sec' 600)
        }
    }
    return $cfg
}

# 全部可能的服务名，按起的顺序。停的时候倒过来。
$script:AllServiceNames = @('database', 'backend', 'rs_server', 'frontend')

function Get-ServiceNames($Cfg) {
    $names = @('database', 'backend')
    if ($Cfg.RsServer.Enabled) { $names += 'rs_server' }
    if ($Cfg.Frontend.Enabled) { $names += 'frontend' }
    return ,$names
}

function Get-ServiceTitle([string]$Name) {
    if ($Name -eq 'database') { return '数据库' }
    if ($Name -eq 'backend') { return '后端' }
    if ($Name -eq 'rs_server') { return 'rs-server' }
    if ($Name -eq 'frontend') { return '前端' }
    return $Name
}

function Get-ServicePort($Cfg, [string]$Name) {
    if ($Name -eq 'database') { return $Cfg.Database.Port }
    if ($Name -eq 'backend') { return $Cfg.Backend.Port }
    if ($Name -eq 'rs_server') { return $Cfg.RsServer.Port }
    return $Cfg.Frontend.Port
}

function Get-ServiceTimeout($Cfg, [string]$Name) {
    if ($Name -eq 'database') { return $Cfg.Timeouts.DatabaseSec }
    if ($Name -eq 'backend') { return $Cfg.Timeouts.BackendSec }
    if ($Name -eq 'rs_server') { return $Cfg.Timeouts.RsServerSec }
    return $Cfg.Timeouts.FrontendSec
}

function Get-BackendBindHost($Cfg) {
    if ($Cfg.LoopbackOnly) { return '127.0.0.1' }
    return '0.0.0.0'
}

function Get-BackendUrl($Cfg) { return "http://127.0.0.1:$($Cfg.Backend.Port)" }
function Get-FrontendUrl($Cfg) { return "http://127.0.0.1:$($Cfg.Frontend.Port)" }
function Get-RsServerUrl($Cfg) { return "http://127.0.0.1:$($Cfg.RsServer.Port)" }

function Get-MachineName {
    try { return ([Environment]::MachineName).ToLowerInvariant() } catch { return 'localhost' }
}

# 浏览器访问 rs-server 用的地址（写进 plant-ui 网页版的配置）。配置里给了就用配置的；
# 只许本机时是回环地址；否则用本机的机器名——同一个网段里别的机器一般也能按名字找到它。
function Get-RsServerPublicUrl($Cfg) {
    if ($Cfg.RsServer.PublicUrl) { return $Cfg.RsServer.PublicUrl }
    if ($Cfg.LoopbackOnly) { return "http://127.0.0.1:$($Cfg.RsServer.Port)" }
    return "http://$(Get-MachineName):$($Cfg.RsServer.Port)"
}

# rs-server 的跨域白名单：从后端或前端的页面里调它，浏览器地址栏里可能出现的各种写法都放进去
# （回环、机器名、本机的各个 IPv4 地址 × 后端和前端两个端口），再加配置里额外给的。
function Get-RsServerCorsOrigins($Cfg) {
    $hosts = @('127.0.0.1', 'localhost', (Get-MachineName))
    try {
        foreach ($address in [System.Net.Dns]::GetHostAddresses([Environment]::MachineName)) {
            if ($address.AddressFamily -ne [System.Net.Sockets.AddressFamily]::InterNetwork) { continue }
            $text = $address.ToString()
            if ($text -like '127.*' -or $text -like '169.254.*') { continue }
            if ($hosts -notcontains $text) { $hosts += $text }
        }
    } catch { }
    $ports = @($Cfg.Backend.Port)
    if ($Cfg.Frontend.Enabled) { $ports += $Cfg.Frontend.Port }
    $origins = @()
    foreach ($h in $hosts) {
        foreach ($port in $ports) {
            if ($port -eq 80) { $origins += "http://$h" } else { $origins += "http://${h}:$port" }
        }
    }
    foreach ($origin in $Cfg.RsServer.CorsOrigins) { if ($origins -notcontains $origin) { $origins += $origin } }
    return ,$origins
}

function Get-DatabaseProbeHost($Cfg) {
    $h = $Cfg.Database.Host
    if ($h -eq '0.0.0.0' -or $h -eq '::' -or $h -eq 'localhost') { return '127.0.0.1' }
    return $h
}

# ─────────────────────────── 进程、端口、内存 ───────────────────────────

function Get-ProcInfo([int]$ProcessId) {
    if ($ProcessId -le 0) { return $null }
    return Get-CimInstance Win32_Process -Filter "ProcessId=$ProcessId" -ErrorAction SilentlyContinue
}

# 从一批「父进程号 = 某个进程」的进程里，挑出真是它生的那些。
# Windows 不会在父进程退出之后改子进程记着的父进程号，而进程号是会重用的：一个早就在跑的、不相干的进程，
# 只要它那个已经退出的父进程的进程号后来恰好分给了本套的某个服务，光看父进程号，它就成了这个服务的「孩子」。
# 孩子不可能比父亲更早创建——所以只认创建时间不早于父进程的；创建时间读不到的也不认。
# 这个函数不碰系统（测试直接喂假数据）：$Candidates 里每个元素要有 ProcessId、ParentProcessId、CreationDate。
# （返回的是一个个进程，不是包起来的数组：调用的地方用 @(…) 接，或者直接 foreach。下面几个新函数都是这个规矩。）
function Select-TrueChildren($Candidates, [int]$ParentPid, [long]$ParentStartTicks) {
    $out = @()
    if ($ParentPid -le 0 -or $ParentStartTicks -le 0) { return $out }
    foreach ($c in @($Candidates)) {
        if ($null -eq $c) { continue }
        if ([int]$c.ParentProcessId -ne $ParentPid) { continue }
        if ([int]$c.ProcessId -eq $ParentPid) { continue }
        if ($null -eq $c.CreationDate) { continue }
        if (([datetime]$c.CreationDate).ToUniversalTime().Ticks -lt $ParentStartTicks) { continue }
        $out += $c
    }
    return $out
}

# 一个进程名下真正的子进程。父进程已经不在、或者这个进程号已经换了主人（给了 $ParentTicks 而对不上），一个都不认。
function Get-ChildProcs([int]$ProcessId, [string]$ParentTicks = '') {
    $parent = Get-ProcInfo $ProcessId
    if ($null -eq $parent -or $null -eq $parent.CreationDate) { return @() }
    if ($ParentTicks -and -not (Test-SameStart $ParentTicks $parent)) { return @() }
    $candidates = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$ProcessId" -ErrorAction SilentlyContinue)
    return (Select-TrueChildren $candidates $ProcessId (([datetime]$parent.CreationDate).ToUniversalTime().Ticks))
}

# 启动时间存成 UTC 的 ticks（字符串）。不存成日期文本：PowerShell 7 读 JSON 时会把像日期的字符串
# 自动变成日期对象，两个版本读出来的东西不一样。
function Get-ProcTicks($Proc) {
    if ($null -eq $Proc -or $null -eq $Proc.CreationDate) { return '' }
    return [string](([datetime]$Proc.CreationDate).ToUniversalTime().Ticks)
}

function Format-ProcClock($Proc) {
    if ($null -eq $Proc -or $null -eq $Proc.CreationDate) { return '' }
    return ([datetime]$Proc.CreationDate).ToString('MM-dd HH:mm:ss')
}

function Test-SameStart([string]$RecordedTicks, $Proc) {
    if (-not $RecordedTicks -or $null -eq $Proc -or $null -eq $Proc.CreationDate) { return $false }
    $recorded = [long]0
    if (-not [long]::TryParse($RecordedTicks, [ref]$recorded)) { return $false }
    $actual = ([datetime]$Proc.CreationDate).ToUniversalTime().Ticks
    return ([math]::Abs($recorded - $actual) -lt 20000000)
}

# 本机所有在听的 TCP 端口 → 进程号。先用系统自带的命令；那个模块在少数机器上没有，就退到 netstat。
# 结果留不到一秒：等端口变化的循环里反复问，不必每次都重新取。
# 取一次要大半秒（Get-NetTCPConnection 慢）。一口气要问好几个端口的地方（预检、认领、down 收尾）：
# 先 -Fresh 取一次，后面带 -MaxAgeMs 用同一份，不必每个端口都重新取。
function Get-ListenTable([switch]$Fresh, [int]$MaxAgeMs = 800) {
    if (-not $Fresh -and $null -ne $script:ListenCache -and ((Get-Date) - $script:ListenCacheAt).TotalMilliseconds -lt $MaxAgeMs) { return $script:ListenCache }
    $table = @{}
    $ok = $false
    try {
        foreach ($c in @(Get-NetTCPConnection -State Listen -ErrorAction Stop)) {
            $port = [int]$c.LocalPort
            $owner = [int]$c.OwningProcess
            if (-not $table.ContainsKey($port)) { $table[$port] = @() }
            if ($table[$port] -notcontains $owner) { $table[$port] += $owner }
        }
        $ok = $true
    } catch { $ok = $false }
    if (-not $ok) {
        $table = @{}
        foreach ($proto in 'TCP', 'TCPv6') {
            foreach ($line in @(& netstat.exe -ano -p $proto 2>$null)) {
                $m = [regex]::Match("$line", '^\s*TCP\s+\S+:(\d+)\s+\S+\s+LISTENING\s+(\d+)\s*$')
                if (-not $m.Success) { continue }
                $port = [int]$m.Groups[1].Value
                $owner = [int]$m.Groups[2].Value
                if (-not $table.ContainsKey($port)) { $table[$port] = @() }
                if ($table[$port] -notcontains $owner) { $table[$port] += $owner }
            }
        }
    }
    $script:ListenCache = $table
    $script:ListenCacheAt = Get-Date
    return $table
}

function Get-PortOwners([int]$Port, [switch]$Fresh, [int]$MaxAgeMs = 800) {
    $table = Get-ListenTable -Fresh:$Fresh -MaxAgeMs $MaxAgeMs
    if ($table.ContainsKey($Port)) { return ,@($table[$Port]) }
    return ,@()
}

# 这个进程是不是占着某个受保护的端口（[guard] protect_ports）。是就返回那个端口号，不是返回 0。
# 占着受保护端口的进程，入口既不认作本套的，也绝不去停它——不管它是怎么出现在名单里的。
function Get-ProtectedPortOf($Cfg, [int]$ProcessId, [switch]$Fresh, [int]$MaxAgeMs = 800) {
    $first = [bool]$Fresh
    foreach ($port in $Cfg.Guard.ProtectPorts) {
        if ((Get-PortOwners $port -Fresh:$first -MaxAgeMs $MaxAgeMs) -contains $ProcessId) { return [int]$port }
        $first = $false
    }
    return 0
}

function Format-ProcBrief([int]$ProcessId) {
    $p = Get-ProcInfo $ProcessId
    if ($null -eq $p) { return "进程 $ProcessId（已经不在）" }
    $path = $p.ExecutablePath
    if (-not $path) { $path = '程序路径读不到' }
    return "进程 $ProcessId（$($p.Name)，$path，起于 $(Format-ProcClock $p)）"
}

function Get-CommitFreeGb {
    $os = Get-CimInstance Win32_OperatingSystem
    return [math]::Round($os.FreeVirtualMemory / 1MB, 1)
}

function Get-ProcMemGb([int]$ProcessId) {
    $p = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
    if ($null -eq $p) { return 0 }
    $bytes = $p.WorkingSet64
    if ($p.PrivateMemorySize64 -gt $bytes) { $bytes = $p.PrivateMemorySize64 }
    return [math]::Round($bytes / 1GB, 2)
}

# 按进程号停一个进程；动手之前再核对一次启动时间，对不上就不动。
function Stop-VerifiedProcess([int]$ProcessId, [string]$Ticks) {
    $p = Get-ProcInfo $ProcessId
    if ($null -eq $p) { return $true }
    if (-not (Test-SameStart $Ticks $p)) { return $false }
    Stop-Process -Id $ProcessId -Force -ErrorAction SilentlyContinue
    $deadline = (Get-Date).AddSeconds(20)
    while ($null -ne (Get-ProcInfo $ProcessId) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 200 }
    return ($null -eq (Get-ProcInfo $ProcessId))
}

# ─────────────────────────── HTTP ───────────────────────────

function Invoke-StackHttp([string]$Method, [string]$Url, [string]$BodyJson = '', [int]$TimeoutSec = 5) {
    $result = @{ Ok = $false; Status = 0; Body = ''; Bytes = 0; Error = '' }
    $response = $null
    try {
        $request = [System.Net.WebRequest]::Create($Url)
        $request.Method = $Method
        $request.Proxy = $null
        $request.Timeout = $TimeoutSec * 1000
        $request.ReadWriteTimeout = $TimeoutSec * 1000
        # 老的 .NET 发 POST 时会先问一句「可以发吗」再等回话，白等几百毫秒；关掉。
        try { $request.ServicePoint.Expect100Continue = $false } catch { }
        if ($BodyJson) {
            $bytes = [System.Text.Encoding]::UTF8.GetBytes($BodyJson)
            $request.ContentType = 'application/json'
            $request.ContentLength = $bytes.Length
            $stream = $request.GetRequestStream()
            try { $stream.Write($bytes, 0, $bytes.Length) } finally { $stream.Dispose() }
        }
        $response = $request.GetResponse()
    } catch [System.Net.WebException] {
        $response = $_.Exception.Response
        if ($null -eq $response) { $result.Error = $_.Exception.Message; return $result }
    } catch {
        $result.Error = $_.Exception.Message
        return $result
    }
    try {
        $result.Status = [int]$response.StatusCode
        $memory = New-Object System.IO.MemoryStream
        try {
            $response.GetResponseStream().CopyTo($memory)
            $data = $memory.ToArray()
        } finally { $memory.Dispose() }
        $result.Bytes = $data.Length
        $result.Body = [System.Text.Encoding]::UTF8.GetString($data)
        $result.Ok = ($result.Status -ge 200 -and $result.Status -lt 300)
        if (-not $result.Ok) { $result.Error = "HTTP $($result.Status)" }
    } catch {
        $result.Error = $_.Exception.Message
    } finally {
        if ($null -ne $response) { $response.Close() }
    }
    return $result
}

function ConvertFrom-JsonSafe([string]$Text) {
    if (-not $Text) { return $null }
    try { return ($Text | ConvertFrom-Json) } catch { }
    # Windows PowerShell 5.1 的 ConvertFrom-Json 吃不下 2 MB 以上的应答（大项目的库列表可能超过）；
    # 退到同一个解析器、把长度上限放开。这时得到的是字典而不是对象，Get-Prop 两种都认。
    try {
        Add-Type -AssemblyName System.Web.Extensions -ErrorAction Stop
        $serializer = New-Object System.Web.Script.Serialization.JavaScriptSerializer
        $serializer.MaxJsonLength = [int]::MaxValue
        $serializer.RecursionLimit = 200
        return ,$serializer.DeserializeObject($Text)
    } catch { return $null }
}

function Get-Prop($Object, [string]$Name) {
    if ($null -eq $Object) { return $null }
    if ($Object -is [System.Collections.IDictionary]) {
        foreach ($key in $Object.Keys) { if ($key -ceq $Name) { return $Object[$key] } }
        return $null
    }
    $p = $Object.PSObject.Properties[$Name]
    if ($null -eq $p) { return $null }
    return $p.Value
}

# ─────────────────────────── 状态文件 ───────────────────────────

function Read-StackState($Cfg) {
    $state = @{}
    if (-not (Test-Path -LiteralPath $Cfg.StateFile)) { return $state }
    $json = ConvertFrom-JsonSafe ([System.IO.File]::ReadAllText($Cfg.StateFile, [System.Text.Encoding]::UTF8))
    if ($null -eq $json) {
        # 读不出来（多半是写到一半被打断）：不装作「什么都没起」，说一声；在跑的服务靠认领规则找回来。
        Say "  注意：状态文件 $($Cfg.StateFile) 读不出来，按没有记录处理。在跑的服务会按「程序在运行目录里、又占着本套的端口」重新认领。" 'Yellow'
        return $state
    }
    $services = Get-Prop $json 'services'
    if ($null -eq $services) { return $state }
    foreach ($p in $services.PSObject.Properties) {
        $v = $p.Value
        $state[$p.Name] = @{
            pid = [int](Get-Prop $v 'pid')
            startTicks = [string](Get-Prop $v 'startTicks')
            startedAt = [string](Get-Prop $v 'startedAt')
            exe = [string](Get-Prop $v 'exe')
            marker = [string](Get-Prop $v 'marker')
            port = [int](Get-Prop $v 'port')
            wrapperPid = [int](Get-Prop $v 'wrapperPid')
            wrapperTicks = [string](Get-Prop $v 'wrapperTicks')
            log = [string](Get-Prop $v 'log')
            # 认领回来的进程才有这一格（认领的时刻）；入口自己起的是空的。
            adopted = [string](Get-Prop $v 'adopted')
        }
    }
    return $state
}

function Save-StackState($Cfg, $State) {
    New-Dir $Cfg.RunDir
    $services = [ordered]@{}
    foreach ($name in $script:AllServiceNames) {
        if ($State.ContainsKey($name)) { $services[$name] = $State[$name] }
    }
    $doc = [ordered]@{
        note = '由 stack.ps1 维护：记着本套起了哪些进程。里面没有账号口令。'
        stack = $Cfg.Name
        config = $Cfg.ConfigPath
        savedAt = (Get-Date).ToString('MM-dd HH:mm:ss')
        services = $services
    }
    $tmp = $Cfg.StateFile + '.tmp'
    [System.IO.File]::WriteAllText($tmp, ($doc | ConvertTo-Json -Depth 6), $script:Utf8NoBom)
    if (Test-Path -LiteralPath $Cfg.StateFile) {
        # 一步换掉：不留「旧的删了、新的还没改名」的空档（那一刻被打断，记录就没了）。
        try { [System.IO.File]::Replace($tmp, $Cfg.StateFile, [NullString]::Value) }
        catch {
            Remove-Item -LiteralPath $Cfg.StateFile -Force
            Move-Item -LiteralPath $tmp -Destination $Cfg.StateFile
        }
    } else {
        Move-Item -LiteralPath $tmp -Destination $Cfg.StateFile
    }
}

# 这条记录里的进程还在不在、是不是本套自己起的那一个。返回 @{ Alive; Ours; Unreadable; Why; Proc }
# Unreadable：进程号和启动时间都对得上，但读不到它的程序路径或命令行——要么它正在退出，
# 要么这个窗口的权限比起它的那个窗口低。这时不认它是本套的（不会去停它），也不把记录丢掉。
function Test-RecordedProcess($Cfg, [string]$Name, $Record) {
    $out = @{ Alive = $false; Ours = $false; Unreadable = $false; Why = ''; Proc = $null }
    if ($null -eq $Record -or $Record.pid -le 0) { $out.Why = '没有记录'; return $out }
    $proc = Get-ProcInfo $Record.pid
    if ($null -eq $proc) { $out.Why = "进程 $($Record.pid) 已经不在了"; return $out }
    $out.Proc = $proc
    if (-not (Test-SameStart $Record.startTicks $proc)) {
        $out.Why = "本套起的那个进程（$($Record.pid)）已经不在了；这个进程号现在是另一个进程（$($proc.Name)，起于 $(Format-ProcClock $proc)）"
        return $out
    }
    $out.Alive = $true
    if ($Record.marker) {
        # 开发档的前端是 node.exe，程序路径和别的 node 进程一样，靠命令行里的标记认。
        if (-not $proc.CommandLine) {
            $out.Unreadable = $true
            $out.Why = "进程 $($Record.pid) 读不到命令行：它正在退出，或者这个窗口的权限比起它的那个窗口低（请用同样的方式打开窗口再试）"
            return $out
        }
        if ("$($proc.CommandLine)".IndexOf($Record.marker, [System.StringComparison]::OrdinalIgnoreCase) -lt 0) {
            $out.Why = "进程 $($Record.pid) 的命令行里没有本套的标记，不认它是本套的"
            return $out
        }
    } else {
        if (-not $proc.ExecutablePath) {
            $out.Unreadable = $true
            $out.Why = "进程 $($Record.pid) 读不到程序路径：它正在退出，或者这个窗口的权限比起它的那个窗口低（请用同样的方式打开窗口再试）"
            return $out
        }
        if ($proc.ExecutablePath -ine $Record.exe) {
            $out.Why = "进程 $($Record.pid) 的程序路径是 $($proc.ExecutablePath)，不是本套记录的 $($Record.exe)，不认它是本套的"
            return $out
        }
    }
    # 「谁在听哪个端口」取一次要大半秒；这里几秒之内的都算数（真要动手停进程的地方会重新取）。
    $guarded = Get-ProtectedPortOf $Cfg ([int]$Record.pid) -MaxAgeMs 3000
    if ($guarded -gt 0) {
        $out.Why = "进程 $($Record.pid) 占着受保护的端口 $guarded，不认它是本套的"
        return $out
    }
    $out.Ours = $true
    return $out
}

# ─────────────────────────── 认领：在跑、却没记在状态文件里的本套进程 ───────────────────────────
# 两种情况会出现这种进程：
#   · 在后端的配置向导页里点了「重启」——后端拉起一个新进程再自己退出，进程号变了；
#   · stack up 跑到一半窗口被关——服务进程已经起了，还没来得及记。
# 认领的条件（都要满足）：
#   1. 它的程序路径就是配置里这个服务的程序，而且这个程序在运行目录（[stack] root）里面——
#      安装文件夹里的那一份程序只有本套会去运行。程序放在运行目录之外的不认领。
#      开发档的前端（node）改看命令行里有没有本套的标记（包装配置的路径，它在运行目录里）。
#   2. 它（或它真正的子进程）占着配置里这个服务的端口。
#   3. 它没有占着受保护的端口。
# 读不到程序路径的进程（多半是权限更高的窗口起的）不认领。

# 一个进程是不是「这个服务的程序」：'yes' / 'no' / 'unreadable'（读不到路径或命令行，判断不了）。
function Test-ServiceProgram($Cfg, $Spec, $Proc) {
    if ($null -eq $Proc) { return 'no' }
    if ($Spec.Marker) {
        if (-not $Proc.CommandLine) { return 'unreadable' }
        if ("$($Proc.CommandLine)".IndexOf($Spec.Marker, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) { return 'yes' }
        return 'no'
    }
    if (-not $Proc.ExecutablePath) { return 'unreadable' }
    if (-not $Spec.Exe -or $Proc.ExecutablePath -ine $Spec.Exe) { return 'no' }
    if (-not (Test-UnderPath $Spec.Exe $Cfg.Root)) { return 'no' }
    return 'yes'
}

# 运行着的、程序就是这个服务的程序的进程（不管占没占端口）。比对用完整路径；名字只用来缩小查询范围。
function Get-ServiceProgramProcs($Cfg, [string]$Name) {
    $found = @()
    $spec = Get-ServiceSpec $Cfg $Name
    if (-not $spec.Exe) { return $found }
    $leaf = Split-Path -Leaf $spec.Exe
    if (-not $leaf -or $leaf -match "['\\]") { return $found }
    foreach ($p in @(Get-CimInstance Win32_Process -Filter "Name='$leaf'" -ErrorAction SilentlyContinue)) {
        if ((Test-ServiceProgram $Cfg $spec $p) -eq 'yes') { $found += $p }
    }
    return $found
}

# 找一个可以认领的进程：@{ Proc = 进程或 $null; UnreadablePid = 占着端口、但读不到路径的进程号（0 = 没有） }
# 「谁在听哪个端口」用调用方刚取的那一份（几秒之内的都算数），不为每个服务重新取一遍。
function Find-AdoptableProcess($Cfg, [string]$Name) {
    $out = @{ Proc = $null; UnreadablePid = 0 }
    $spec = Get-ServiceSpec $Cfg $Name
    $port = Get-ServicePort $Cfg $Name
    foreach ($owner in (Get-PortOwners $port -MaxAgeMs 6000)) {
        $proc = Get-ProcInfo ([int]$owner)
        $kind = Test-ServiceProgram $Cfg $spec $proc
        if ($kind -eq 'unreadable') { $out.UnreadablePid = [int]$owner; continue }
        if ($kind -ne 'yes') { continue }
        # 监听挂在子进程名下的服务（nginx 的工作进程）：往上找到同一个程序的主进程，认领主进程。
        for ($i = 0; $i -lt 4; $i++) {
            $parent = Get-ProcInfo ([int]$proc.ParentProcessId)
            if ($null -eq $parent -or $null -eq $parent.CreationDate -or $null -eq $proc.CreationDate) { break }
            if ((Test-ServiceProgram $Cfg $spec $parent) -ne 'yes') { break }
            # 「父进程」比它还晚创建：那个进程号已经被重用，不是真的父进程。
            if (([datetime]$parent.CreationDate) -gt ([datetime]$proc.CreationDate)) { break }
            $proc = $parent
        }
        if ((Get-ProtectedPortOf $Cfg ([int]$proc.ProcessId) -MaxAgeMs 6000) -gt 0) { continue }
        $out.Proc = $proc
        return $out
    }
    return $out
}

# 对点到名的服务（缺省是配置里启用的全部）：记录缺了，或者记录里的进程已经不在、不是原来那个，
# 就看有没有可以认领的进程，有就记回来。记录还有效、或者只是读不到路径（权限）的，不动。
# $Persist：写回状态文件——只有占着运行目录锁的命令（up、down）才写；别的命令只在这一次运行里当它是本套的。
# 返回认领了的服务名。
function Invoke-Adopt($Cfg, $State, [bool]$Persist, [string[]]$Names = @(), [bool]$Quiet = $false) {
    $adopted = @()
    # 这一趟里「没有有效记录、也没认领到」的服务名：调用方要不要再等一等（程序在跑、只是还没占上端口），看它。
    $script:AdoptMissing = @()
    $tableFresh = $false
    if (@($Names).Count -eq 0) { $Names = Get-ServiceNames $Cfg }
    foreach ($name in $Names) {
        $old = $null
        if ($State.ContainsKey($name)) {
            $old = $State[$name]
            $check = Test-RecordedProcess $Cfg $name $old
            if ($check.Ours -or $check.Unreadable) { continue }
        }
        if (-not $tableFresh) { [void](Get-ListenTable -Fresh); $tableFresh = $true }
        $found = Find-AdoptableProcess $Cfg $name
        if ($null -eq $found.Proc) { $script:AdoptMissing += $name; continue }
        $spec = Get-ServiceSpec $Cfg $name
        # 日志：后端自己重启时，新进程接着往原来那个文件里写；没有旧记录就取这个服务最新的那个日志文件。
        $log = ''
        if ($null -ne $old -and $old.log) { $log = $old.log }
        if (-not $log) {
            $latest = Get-ChildItem -LiteralPath (Join-Path $Cfg.LogsDir $name) -Filter "$name-*.log" -File -ErrorAction SilentlyContinue | Sort-Object LastWriteTime | Select-Object -Last 1
            if ($latest) { $log = $latest.FullName }
        }
        $State[$name] = @{
            pid = [int]$found.Proc.ProcessId
            startTicks = (Get-ProcTicks $found.Proc)
            startedAt = (Format-ProcClock $found.Proc)
            exe = [string]$found.Proc.ExecutablePath
            marker = $spec.Marker
            port = $spec.Port
            wrapperPid = 0
            wrapperTicks = ''
            log = $log
            adopted = (Get-Date).ToString('MM-dd HH:mm:ss')
        }
        $adopted += $name
        if ($Quiet) { continue }
        $why = '状态文件里没有它的记录（上一次 stack up 没跑完就会这样）'
        if ($null -ne $old) { $why = "状态文件里记的是进程 $($old.pid)，它已经不在了（在后端的向导页里点过「重启」就会这样）" }
        $tail = '这一次按本套的服务看待；下次运行 stack up 或 stack down 时会记进状态文件'
        if ($Persist) { $tail = '已经记回状态文件' }
        Say "  认领了$(Get-ServiceTitle $name)的进程 $($found.Proc.ProcessId)（$($found.Proc.Name)，起于 $(Format-ProcClock $found.Proc)）：它的程序就是运行目录里的那一个，又占着本套的端口 $($spec.Port)。$why。$tail。" 'Yellow'
    }
    if ($Persist -and $adopted.Count -gt 0) { Save-StackState $Cfg $State }
    return $adopted
}

# 端口空着、但运行目录里这个服务的程序已经有进程在跑：多半是刚被重启（或刚起）、还没占上端口。
# 等它一会儿：占上端口就认领，返回 'adopted'；它自己退了，返回 'gone'；到点了还在、还没占端口，返回 'stuck'。
function Wait-StrayService($Cfg, $State, [string]$Name, [bool]$Persist, [int]$Seconds) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ($true) {
        if (@(Invoke-Adopt $Cfg $State $Persist @($Name)).Count -gt 0) { return 'adopted' }
        if (@(Get-ServiceProgramProcs $Cfg $Name).Count -eq 0) { return 'gone' }
        if ((Get-Date) -ge $deadline) { return 'stuck' }
        Start-Sleep -Milliseconds 700
    }
}

# ─────────────────────────── 后端配置：从模板生成，只改指定的键 ───────────────────────────

function Format-TomlLiteral($Value, [bool]$QuoteNumbers) {
    if ($Value -is [bool]) { if ($Value) { return 'true' } else { return 'false' } }
    if ($Value -is [int] -or $Value -is [long]) {
        if ($QuoteNumbers) { return '"' + $Value + '"' }
        return "$Value"
    }
    if ($Value -is [System.Array]) {
        $parts = @()
        foreach ($item in $Value) { $parts += (Format-TomlLiteral $item $false) }
        return '[' + ($parts -join ', ') + ']'
    }
    return '"' + ([string]$Value).Replace('\', '\\').Replace('"', '\"') + '"'
}

# 在「第一个表头之前」找这个键：有就换值（原来带引号的数字继续带引号，行尾注释留着），没有就插进去。
function Set-TomlTopLevelKey($Lines, [string]$Key, $Value) {
    $topEnd = $Lines.Count
    for ($i = 0; $i -lt $Lines.Count; $i++) {
        if ($Lines[$i] -match '^\s*\[') { $topEnd = $i; break }
    }
    $pattern = '^(\s*)' + [regex]::Escape($Key) + '(\s*=\s*)(.*)$'
    for ($i = 0; $i -lt $topEnd; $i++) {
        $m = [regex]::Match($Lines[$i], $pattern)
        if (-not $m.Success) { continue }
        $old = $m.Groups[3].Value
        $oldTrim = $old.TrimStart()
        $quoteNumbers = ($oldTrim.StartsWith('"') -or $oldTrim.StartsWith("'"))
        $tail = ''
        try {
            $parsed = Read-TomlValue $old ($i + 1)
            if ($parsed.Rest.Trim().StartsWith('#')) { $tail = $parsed.Rest }
        } catch { $tail = '' }
        $Lines[$i] = $m.Groups[1].Value + $Key + $m.Groups[2].Value + (Format-TomlLiteral $Value $quoteNumbers) + $tail
        return
    }
    $Lines.Insert($topEnd, "$Key = " + (Format-TomlLiteral $Value $false))
}

# 只取顶层的一个键，去掉引号后返回；没有就返回 $null。整份文件的内容不显示。
function Get-TomlTopLevelText([string]$Path, [string]$Key) {
    foreach ($line in [System.IO.File]::ReadAllLines($Path, [System.Text.Encoding]::UTF8)) {
        if ($line -match '^\s*\[') { break }
        $m = [regex]::Match($line, '^\s*' + [regex]::Escape($Key) + '\s*=\s*(.*)$')
        if (-not $m.Success) { continue }
        try { return [string]((Read-TomlValue $m.Groups[1].Value 0).Value) } catch { return $m.Groups[1].Value.Trim() }
    }
    return $null
}

# Windows 路径写进 TOML：用单引号（原样字符串），反斜杠不用转义，人看得懂、也好改。
# 路径里万一有单引号，才退回双引号加转义。
function Format-TomlPath([string]$Path) {
    if ($Path.IndexOf("'") -lt 0) { return "'" + $Path + "'" }
    return '"' + $Path.Replace('\', '\\').Replace('"', '\"') + '"'
}

# 在 [表名] 这一节里找这个键：有就换值（行尾注释留着），没有就加在这一节的末尾；
# 整份文件里没有这一节，就在文件末尾加上这一节。别的行一概不动。
# $Literal 是已经写成 TOML 的值（带引号）。
function Set-TomlTableKey($Lines, [string]$Table, [string]$Key, [string]$Literal) {
    # 这一节如果被写成了「表名.键 = 值」或行内表，再加一节会让整份配置解析失败，这种情况直接报错。
    for ($i = 0; $i -lt $Lines.Count; $i++) {
        if ($Lines[$i] -match ('^\s*' + [regex]::Escape($Table) + '\s*(\.|=)')) { Stop-Stack "后端配置模板第 $($i + 1) 行把 $Table 写成了「$Table.键 = 值」或行内表，入口不会改这种写法。请改成单独的一节 [$Table]" 2 }
    }
    $start = -1
    for ($i = 0; $i -lt $Lines.Count; $i++) {
        if ($Lines[$i] -match ('^\s*\[\s*' + [regex]::Escape($Table) + '\s*\]\s*(#.*)?$')) { $start = $i; break }
    }
    if ($start -lt 0) {
        while ($Lines.Count -gt 0 -and $Lines[$Lines.Count - 1].Trim() -eq '') { $Lines.RemoveAt($Lines.Count - 1) }
        $Lines.Add('')
        $Lines.Add("[$Table]")
        $Lines.Add("$Key = $Literal")
        # 末尾留一个空元素：按换行拼回去之后，文件以换行结尾。
        $Lines.Add('')
        return
    }
    $end = $Lines.Count
    for ($i = $start + 1; $i -lt $Lines.Count; $i++) {
        if ($Lines[$i] -match '^\s*\[') { $end = $i; break }
    }
    $pattern = '^(\s*)' + [regex]::Escape($Key) + '(\s*=\s*)(.*)$'
    for ($i = $start + 1; $i -lt $end; $i++) {
        $m = [regex]::Match($Lines[$i], $pattern)
        if (-not $m.Success) { continue }
        $tail = ''
        try {
            $parsed = Read-TomlValue $m.Groups[3].Value ($i + 1)
            if ($parsed.Rest.Trim().StartsWith('#')) { $tail = $parsed.Rest }
        } catch { $tail = '' }
        $Lines[$i] = $m.Groups[1].Value + $Key + $m.Groups[2].Value + $Literal + $tail
        return
    }
    $insert = $end
    while ($insert -gt $start + 1 -and $Lines[$insert - 1].Trim() -eq '') { $insert -= 1 }
    $Lines.Insert($insert, "$Key = $Literal")
}

# 只取 [表名] 这一节里的一个键，去掉引号后返回；没有就返回 $null。整份文件的内容不显示。
function Get-TomlTableText([string]$Path, [string]$Table, [string]$Key) {
    $inside = $false
    foreach ($line in [System.IO.File]::ReadAllLines($Path, [System.Text.Encoding]::UTF8)) {
        if ($line -match '^\s*\[') {
            $inside = ($line -match ('^\s*\[\s*' + [regex]::Escape($Table) + '\s*\]\s*(#.*)?$'))
            continue
        }
        if (-not $inside) { continue }
        $m = [regex]::Match($line, '^\s*' + [regex]::Escape($Key) + '\s*=\s*(.*)$')
        if (-not $m.Success) { continue }
        try { return [string]((Read-TomlValue $m.Groups[1].Value 0).Value) } catch { return $m.Groups[1].Value.Trim() }
    }
    return $null
}

# 两个路径是不是同一个地方：不分大小写，不分正反斜杠，不计末尾的斜杠。
function Test-SamePath([string]$A, [string]$B) {
    $pair = @()
    foreach ($raw in $A, $B) {
        $p = ([string]$raw).Trim().Replace('/', '\')
        try { if ($p -and [System.IO.Path]::IsPathRooted($p)) { $p = [System.IO.Path]::GetFullPath($p) } } catch { }
        if ($p.Length -gt 3) { $p = $p.TrimEnd('\') }
        $pair += $p
    }
    return ($pair[0] -ieq $pair[1])
}

# 入口管的那些后端配置键（都不是账号口令）。
function Get-BackendConfigKeys($Cfg) {
    $bind = (Get-BackendBindHost $Cfg) + ':' + $Cfg.Backend.Port
    $keys = [ordered]@{
        project_path = $Cfg.Project.E3dRoot.Replace('\', '/')
        project_name = $Cfg.Project.Name
        project_code = $Cfg.Project.Code
        surreal_ns = $Cfg.Project.Code
        mdb_name = $Cfg.Project.Mdb
        e3d_version = $Cfg.Project.E3dVersion
        v_ip = $Cfg.Database.Host
        v_port = $Cfg.Database.Port
        store_mode = 'rocksdb'
        http_api_addr = $bind
        server_release_ip = $bind
        # 后端代码自己的缺省是「启动时自动初始化没初始化过的库」。入口明确写成不自动，
        # 除非配置里 [init] auto_new = true。
        auto_initialize_new_dbnums = $Cfg.Init.AutoNew
    }
    if ($null -ne $Cfg.Backend.WatchDbnums) { $keys['watch_dbnums'] = $Cfg.Backend.WatchDbnums }
    foreach ($key in $Cfg.Backend.ConfigSet.Keys) { $keys[$key] = $Cfg.Backend.ConfigSet[$key] }
    return $keys
}

function Initialize-BackendFiles($Cfg) {
    $work = $Cfg.Backend.WorkDir
    foreach ($dir in $work, (Join-Path $work 'assets'), (Join-Path $work 'direct-index')) { New-Dir $dir }
    # 数据库函数脚本：后端按「当前目录\resource\surreal」找，缺了它连上库之后那一步就退出。
    $source = Join-Path $Cfg.Backend.ResourceDir 'surreal'
    $target = Join-Path $work 'resource\surreal'
    if ($source -ine $target) {
        New-Dir $target
        foreach ($file in @(Get-ChildItem -LiteralPath $source -Filter '*.surql' -File)) {
            $dest = Join-Path $target $file.Name
            $same = $false
            if (Test-Path -LiteralPath $dest) {
                $existing = Get-Item -LiteralPath $dest
                $same = ($existing.Length -eq $file.Length -and $existing.LastWriteTimeUtc -eq $file.LastWriteTimeUtc)
            }
            if (-not $same) { Copy-Item -LiteralPath $file.FullName -Destination $dest -Force }
        }
    }

    $configFile = $Cfg.Backend.ConfigFile
    if (-not (Test-Path -LiteralPath $configFile)) {
        $text = [System.IO.File]::ReadAllText($Cfg.Backend.ConfigTemplate, [System.Text.Encoding]::UTF8)
        $newline = "`n"
        if ($text.Contains("`r`n")) { $newline = "`r`n" }
        $lines = New-Object System.Collections.Generic.List[string]
        foreach ($line in ($text -split "`r?`n")) { $lines.Add($line) }
        $keys = Get-BackendConfigKeys $Cfg
        foreach ($key in $keys.Keys) { Set-TomlTopLevelKey $lines $key $keys[$key] }
        $changed = @($keys.Keys)
        # 「核对」（RVM 比对）的产物目录：明确写成数据目录下面的那一个。不写的话后端按自己程序所在的位置定
        # （程序目录\output\rvm-check），换一版程序目录之后，以前的产物在界面上就下载不到了。
        if ($Cfg.Backend.RvmCheckDir) {
            Set-TomlTableKey $lines 'rvm_check' 'work_dir' (Format-TomlPath $Cfg.Backend.RvmCheckDir)
            $changed += '[rvm_check] work_dir'
        }
        $lines.Insert(0, "# 由 stack.ps1 从模板生成（$((Get-Date).ToString('yyyy-MM-dd HH:mm:ss'))）。入口改过的键：$($changed -join '、')。其余照模板。")
        [System.IO.File]::WriteAllText($configFile, ($lines -join $newline), $script:Utf8NoBom)
        Say "  后端配置已从模板生成：$configFile（入口改了 $($changed.Count) 个键，其余照模板）"
    }
    if ($Cfg.Backend.RvmCheckDir) { New-Dir $Cfg.Backend.RvmCheckDir }
    # 不管是刚生成的还是早就在的：要连哪个库、听哪个端口必须和 stack 的配置一致，否则不起。
    $vPort = Get-TomlTopLevelText $configFile 'v_port'
    $vIp = Get-TomlTopLevelText $configFile 'v_ip'
    $addr = Get-TomlTopLevelText $configFile 'http_api_addr'
    if ("$vPort" -ne "$($Cfg.Database.Port)" -or "$vIp" -ne $Cfg.Database.Host) {
        Stop-Stack "后端配置 $configFile 里要连的数据库是 ${vIp}:$vPort，而 stack 配置里是 $($Cfg.Database.Host):$($Cfg.Database.Port)。两边不一致，不起后端。想按 stack 配置重新生成，就先把那份后端配置挪走" 2
    }
    if (-not "$addr".EndsWith(':' + $Cfg.Backend.Port)) {
        Stop-Stack "后端配置 $configFile 里的监听地址是 $addr，而 stack 配置里后端端口是 $($Cfg.Backend.Port)。两边不一致，不起后端" 2
    }
    # 后端配置只生成一次，以后以它为准（向导页里改的东西要留得住）。所以 stack.toml 里入口管的键
    # 后来又改了的话，这里只提醒，不替人覆盖。
    $keys = Get-BackendConfigKeys $Cfg
    $drift = @()
    foreach ($key in $keys.Keys) {
        if ($key -eq 'v_ip' -or $key -eq 'v_port') { continue }
        $want = [string]$keys[$key]
        if ($keys[$key] -is [System.Array]) { $want = (@($keys[$key]) -join ' ') }
        $have = Get-TomlTopLevelText $configFile $key
        if ($null -eq $have) { $have = '（没有这个键）' }
        if ("$have" -ine "$want") { $drift += "$key：stack.toml 要的是「$want」，后端配置里是「$have」" }
    }
    $rvmDrift = $false
    if ($Cfg.Backend.RvmCheckDir) {
        $rvmHave = Get-TomlTableText $configFile 'rvm_check' 'work_dir'
        if (-not $rvmHave) {
            $drift += "[rvm_check] work_dir：后端配置里没有写。这样「核对」的产物会落在 $(Get-RvmCheckFallbackDir $Cfg)，换一版程序、挪了程序的位置之后，以前的产物在界面上就下载不到了。stack.toml 要的是「$($Cfg.Backend.RvmCheckDir)」"
            $rvmDrift = $true
        } elseif (-not (Test-SamePath $rvmHave $Cfg.Backend.RvmCheckDir)) {
            $drift += "[rvm_check] work_dir：stack.toml 要的是「$($Cfg.Backend.RvmCheckDir)」，后端配置里是「$rvmHave」（整个文件夹挪过位置的话就会这样）"
            $rvmDrift = $true
        }
    }
    if ($drift.Count -gt 0) {
        Say "  注意：后端配置只在第一次生成，之后以它为准。下面这些键和 stack.toml 现在的写法不一样，**没有**替你改：" 'Yellow'
        foreach ($d in $drift) { Say "    · $d" 'Yellow' }
        Say "    要按 stack.toml 重新生成：先 stack down，把 $configFile 挪走，再 stack up。" 'Yellow'
        if ($rvmDrift) {
            Say "    只想改「核对」产物目录这一项：先 stack down；在那份后端配置里把 [rvm_check] 一节的 work_dir 改成 $(Format-TomlPath $Cfg.Backend.RvmCheckDir)" 'Yellow'
            Say "    （没有这一节，就在文件末尾加两行：[rvm_check] 和 work_dir = $(Format-TomlPath $Cfg.Backend.RvmCheckDir)）；原来那个目录里已有的产物要留的话，把里面的子文件夹挪到新目录下；再 stack up。" 'Yellow'
        }
    }
    # 这份配置只生成一次，新一版模板里新加的键到不了它。把缺的键名说出来（只说名字，不碰值），抄不抄由人定。
    $have = @(Get-TomlKeyNames $configFile)
    $missing = @(Get-TomlKeyNames $Cfg.Backend.ConfigTemplate | Where-Object { $have -notcontains $_ })
    if ($missing.Count -gt 0) {
        $shown = (@($missing | Select-Object -First 12) -join '、')
        if ($missing.Count -gt 12) { $shown += ' …' }
        Say "  注意：后端配置模板（现在这一版的）里有 $($missing.Count) 个键，现有的后端配置里没有：$shown。" 'Yellow'
        Say "    后端配置只在第一次生成，新模板里新加的键到不了它；这些键现在按后端自己的缺省值走。要照模板来：从 $($Cfg.Backend.ConfigTemplate) 里把这几行抄进 $configFile ，再 stack down、stack up。" 'Yellow'
    }
}

# 一份 TOML 里所有的键名：顶层的就是键名，某一节里的写成「[节] 键」。只取名字，不取值。
function Get-TomlKeyNames([string]$Path) {
    $names = @()
    if (-not $Path -or -not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $names }
    $table = ''
    foreach ($line in [System.IO.File]::ReadAllLines($Path, [System.Text.Encoding]::UTF8)) {
        $m = [regex]::Match($line, '^\s*\[\s*([A-Za-z0-9_\-\.]+)\s*\]')
        if ($m.Success) { $table = $m.Groups[1].Value; continue }
        $m = [regex]::Match($line, '^\s*([A-Za-z0-9_\-]+)\s*=')
        if (-not $m.Success) { continue }
        if ($table) { $names += "[$table] $($m.Groups[1].Value)" } else { $names += $m.Groups[1].Value }
    }
    return $names
}

# 后端配置里没写 [rvm_check] work_dir 时，后端自己把「核对」的产物放哪（照它的规矩算出来，只用来给人看）：
# 程序所在的目录里有 aios-database.exe 或 tools\ 就用「程序目录\output\rvm-check」，否则用「工作目录\output\rvm-check」。
function Get-RvmCheckFallbackDir($Cfg) {
    $exeDir = Split-Path -Parent $Cfg.Backend.Exe
    if ((Test-Path -LiteralPath (Join-Path $exeDir 'aios-database.exe') -PathType Leaf) -or (Test-Path -LiteralPath (Join-Path $exeDir 'tools') -PathType Container)) {
        return (Join-Path $exeDir 'output\rvm-check')
    }
    return (Join-Path $Cfg.Backend.WorkDir 'output\rvm-check')
}

# 把一棵目录里的文件拷到另一处：缺的、大小或时间不一样的才拷；目标那边多出来的文件不删
# （那是服务运行时自己写的）。
function Copy-TreeIfChanged([string]$Source, [string]$Target) {
    if (-not (Test-Path -LiteralPath $Source -PathType Container)) { return 0 }
    $copied = 0
    $prefix = $Source.TrimEnd('\').Length + 1
    foreach ($file in @(Get-ChildItem -LiteralPath $Source -Recurse -File -Force)) {
        $dest = Join-Path $Target $file.FullName.Substring($prefix)
        $same = $false
        if (Test-Path -LiteralPath $dest -PathType Leaf) {
            $existing = Get-Item -LiteralPath $dest -Force
            $same = ($existing.Length -eq $file.Length -and $existing.LastWriteTimeUtc -eq $file.LastWriteTimeUtc)
        }
        if (-not $same) {
            New-Dir (Split-Path -Parent $dest)
            Copy-Item -LiteralPath $file.FullName -Destination $dest -Force
            $copied += 1
        }
    }
    return $copied
}

# rs-server 的运行目录。它的配置、资源、静态目录全按「当前目录」找，而且会把当前目录下的
# assets\ 和 target\ 整个对外提供——所以给它一个干净的目录，里面只有它要的东西。
function Initialize-RsServerFiles($Cfg) {
    $work = $Cfg.RsServer.WorkDir
    foreach ($dir in $work, (Join-Path $work 'target'), (Join-Path $work 'assets')) { New-Dir $dir }
    [void](Copy-TreeIfChanged (Join-Path $Cfg.RsServer.TemplateDir 'resource') (Join-Path $work 'resource'))
    [void](Copy-TreeIfChanged (Join-Path $Cfg.RsServer.TemplateDir 'assets') (Join-Path $work 'assets'))

    $configFile = Join-Path $work 'DbOption.toml'
    $backendConfig = $Cfg.Backend.ConfigFile
    if (-not (Test-Path -LiteralPath $backendConfig)) { Stop-Stack "后端配置还没有生成（$backendConfig），没法给 rs-server 生成配置" 2 }
    $bind = (Get-BackendBindHost $Cfg) + ':' + $Cfg.RsServer.Port
    if (-not (Test-Path -LiteralPath $configFile)) {
        # 以后端那份配置为底：共用的键（库地址、账号、命名空间、项目）原样照抄，保证两边连的是同一个库。
        # 只属于 rs-server 的键另写；MySQL 和数据中心平台那几项一律留空——没配时相应功能报「未配置」。
        $text = [System.IO.File]::ReadAllText($backendConfig, [System.Text.Encoding]::UTF8)
        $newline = "`n"
        if ($text.Contains("`r`n")) { $newline = "`r`n" }
        $lines = New-Object System.Collections.Generic.List[string]
        foreach ($line in ($text -split "`r?`n")) { $lines.Add($line) }
        if ($lines.Count -gt 0 -and $lines[0].StartsWith('# 由 stack.ps1 从模板生成')) { $lines.RemoveAt(0) }
        # [rvm_check] 一节只和后端有关（「核对」是后端的功能），不带进 rs-server 的配置。
        $sectionStart = -1
        for ($i = 0; $i -lt $lines.Count; $i++) { if ($lines[$i] -match '^\s*\[\s*rvm_check\s*\]\s*(#.*)?$') { $sectionStart = $i; break } }
        if ($sectionStart -ge 0) {
            $sectionEnd = $lines.Count
            for ($i = $sectionStart + 1; $i -lt $lines.Count; $i++) { if ($lines[$i] -match '^\s*\[') { $sectionEnd = $i; break } }
            $lines.RemoveRange($sectionStart, $sectionEnd - $sectionStart)
            while ($lines.Count -gt 0 -and $lines[$lines.Count - 1].Trim() -eq '') { $lines.RemoveAt($lines.Count - 1) }
            $lines.Add('')
        }
        $keys = [ordered]@{
            server_release_ip = $bind
            aios_database_url = (Get-BackendUrl $Cfg)
            ip = ''
            user = ''
            password = ''
            puhua_database_ip = ''
            puhua_database_user = ''
            puhua_database_password = ''
            plat_url = ''
        }
        foreach ($key in $keys.Keys) { Set-TomlTopLevelKey $lines $key $keys[$key] }
        $lines.Insert(0, '# 要用登录、工艺 / 仪控提资查材料、项目地址表：填 ip / port / user / password 和 puhua_database_* 三项（MySQL）。要往数据中心推送：填 plat_url，再加一行 plat_ukey = "平台令牌"。改完 stack down 再 stack up。')
        $lines.Insert(0, "# rs-server 的配置。由 stack.ps1 以后端的配置为底生成（$((Get-Date).ToString('yyyy-MM-dd HH:mm:ss'))），只生成一次，以后不覆盖。入口另写的键：$(@($keys.Keys) -join '、')。")
        [System.IO.File]::WriteAllText($configFile, ($lines -join $newline), $script:Utf8NoBom)
        Say "  rs-server 的配置已生成：$configFile（库地址、账号、项目照后端那份；MySQL 和数据中心平台几项留空）"
    }
    # 两边必须连同一个库；监听端口必须和 stack 配置一致。
    foreach ($key in 'v_ip', 'v_port', 'surreal_ns', 'project_name') {
        $a = Get-TomlTopLevelText $configFile $key
        $b = Get-TomlTopLevelText $backendConfig $key
        if ("$a" -ne "$b") { Stop-Stack "rs-server 的配置 $configFile 里 $key 是「$a」，后端配置里是「$b」。两边连的不是同一个库，不起 rs-server。想重新生成，就先把 rs-server 那份配置挪走" 2 }
    }
    $addr = Get-TomlTopLevelText $configFile 'server_release_ip'
    if (-not "$addr".EndsWith(':' + $Cfg.RsServer.Port)) {
        Stop-Stack "rs-server 的配置 $configFile 里的监听地址是 $addr，而 stack 配置里它的端口是 $($Cfg.RsServer.Port)。两边不一致，不起 rs-server" 2
    }
}

# plant-ui 网页版的配置（网页根目录下的 config.json）：让网页版知道 rs-server 在哪。
# 后端自己现算的那份里「数据服务地址」固定是空的，页面会把它当成后端自己，「提资」就找不到 rs-server。
# 做法：向后端要一份它现算的，把数据服务地址填上、把数据库账号口令两项清空，存到网页根目录。
# 磁盘上这份会原样发给所有打开网页的人，所以不能带口令——网页版这时用它自带的缺省账号连库。
function Update-PlantUiWebConfig($Cfg) {
    if (-not $Cfg.Backend.PlantUiWebRoot) { return }
    $file = Join-Path $Cfg.Backend.PlantUiWebRoot 'config.json'
    $marker = Join-Path $Cfg.RunDir 'plant-ui-web-config.generated'
    if (-not $Cfg.RsServer.Enabled -or -not $Cfg.RsServer.PlantUiConfig) {
        if ((Test-Path -LiteralPath $file) -and (Test-Path -LiteralPath $marker)) {
            Remove-Item -LiteralPath $file -Force
            Remove-Item -LiteralPath $marker -Force
            Say '  去掉了入口以前生成的 plant-ui 网页版配置（恢复成后端现算的那份；网页版里的数据服务地址要自己在界面的设置里填）'
        }
        return
    }
    if ((Test-Path -LiteralPath $file) -and -not (Test-Path -LiteralPath $marker)) {
        Say "  注意：$file 已经在，而且不是入口生成的，没有动它。plant-ui 网页版找 rs-server 的地址以它里面的 data_api_url 为准。" 'Yellow'
        return
    }
    if (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file -Force }
    $r = Invoke-StackHttp 'GET' ((Get-BackendUrl $Cfg) + '/config.json') '' 10
    $j = ConvertFrom-JsonSafe $r.Body
    $db = Get-Prop $j 'db'
    if (-not $r.Ok -or $null -eq $db) {
        Say '  注意：没能从后端取到 plant-ui 网页版的配置（后端可能在设置模式），这次没有写数据服务地址。' 'Yellow'
        return
    }
    $public = Get-RsServerPublicUrl $Cfg
    $doc = [ordered]@{
        db = [ordered]@{
            host = (Get-Prop $db 'host')
            port = (Get-Prop $db 'port')
            secure = (Get-Prop $db 'secure')
            namespace = (Get-Prop $db 'namespace')
            database = (Get-Prop $db 'database')
            mdb = (Get-Prop $db 'mdb')
            username = ''
            password = ''
        }
        model_api_url = ''
        data_api_url = $public
    }
    [System.IO.File]::WriteAllText($file, ($doc | ConvertTo-Json -Depth 5), $script:Utf8NoBom)
    [System.IO.File]::WriteAllText($marker, "由 stack.ps1 生成于 $((Get-Date).ToString('yyyy-MM-dd HH:mm:ss'))：$file", $script:Utf8NoBom)
    Say "  plant-ui 网页版的配置已写好：数据服务（rs-server）地址 $public（$file）"
}

# nginx 的配置。缺省用下面这份；要自己改就在 [frontend] conf_template 里给一份带同样占位符的模板。
$script:NginxTemplate = @'
# 由 stack.ps1 生成，每次 up 都会重写，不要手改。
# 要改端口、目录：改 stack.toml。要改别的：在 stack.toml 的 [frontend] conf_template 里给一份自己的模板。
worker_processes  1;

error_log  logs/error.log warn;

events {
    worker_connections  1024;
}

http {
    include       "{{MIME}}";
    default_type  application/octet-stream;

    map $http_upgrade $connection_upgrade {
        default upgrade;
        ''      close;
    }

    log_format  main  '$remote_addr - $remote_user [$time_local] "$request" '
                      '$status $body_bytes_sent "$http_referer" "$http_user_agent"';
    access_log  logs/access.log  main;

    sendfile        on;
    keepalive_timeout  65;

    # 后端（aios-database）：和前端在同一台机器上
    upstream stack_backend {
        server 127.0.0.1:{{BACKEND_PORT}};
        keepalive 32;
    }

    server {
        listen       {{LISTEN}};
        server_name  localhost;

        root   "{{WWW}}";
        index  index.html;
        charset utf-8;

        # 入口页每次都重新取，免得旧的 index.html 继续引用旧的脚本
        location = /index.html {
            try_files /index.html =404;
            add_header Cache-Control "no-store, no-cache, must-revalidate" always;
        }

        location / {
            try_files $uri $uri/ /index.html;
            add_header Cache-Control "no-store, no-cache, must-revalidate" always;
        }

        # 文件名带哈希的静态资源可以长期缓存
        location ~* \.(js|css|png|jpg|jpeg|gif|ico|svg|woff|woff2|ttf|eot|wasm)$ {
            try_files $uri =404;
            expires 1y;
            add_header Cache-Control "public, immutable" always;
        }

        # JSON（配置、清单一类）文件名不带哈希：每次都重新取，换新一版之后浏览器不会接着用旧的。
        # 找不到就是 404，不落到首页上。
        location ~* \.json$ {
            try_files $uri =404;
            add_header Cache-Control "no-store, no-cache, must-revalidate" always;
        }

        location = /version.json {
            try_files /version.json =404;
            add_header Cache-Control "no-store, no-cache, must-revalidate" always;
        }

        # 接口、校审附件、配置向导页都转给后端
        location = /setup.html {
            proxy_pass http://stack_backend;
            proxy_set_header Host $host;
            add_header Cache-Control "no-store" always;
        }

        # ^~ ：地址以 /api 开头就归这一条，不再去比上面那些按扩展名匹配的规则。
        # 少了它，/api/…/compare.json 这类以 .json、.png 结尾的接口地址会被当成静态文件，回 404。
        location ^~ /api {
            proxy_pass http://stack_backend;
            proxy_http_version 1.1;
            proxy_set_header Host $host;
            proxy_set_header X-Real-IP $remote_addr;
            proxy_set_header X-Forwarded-Host $host;
            proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
            proxy_set_header X-Forwarded-Proto $scheme;
            proxy_set_header Upgrade $http_upgrade;
            proxy_set_header Connection $connection_upgrade;
            proxy_read_timeout 300s;
            proxy_buffering off;
            client_max_body_size 50m;
        }

        location ^~ /files/ {
            proxy_pass http://stack_backend;
            proxy_http_version 1.1;
            proxy_set_header Host $host;
            proxy_set_header X-Real-IP $remote_addr;
            proxy_set_header X-Forwarded-Proto $scheme;
            proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
            client_max_body_size 50m;
        }

        gzip on;
        gzip_vary on;
        gzip_proxied any;
        gzip_comp_level 6;
        gzip_types text/plain text/css application/json application/javascript text/xml application/xml application/xml+rss text/javascript application/wasm;
        gzip_min_length 1000;
    }
}
'@

function Initialize-NginxFiles($Cfg) {
    $work = $Cfg.Frontend.WorkDir
    foreach ($dir in $work, (Join-Path $work 'conf'), (Join-Path $work 'logs'), (Join-Path $work 'temp')) { New-Dir $dir }
    $template = $script:NginxTemplate
    if ($Cfg.Frontend.ConfTemplate) { $template = [System.IO.File]::ReadAllText($Cfg.Frontend.ConfTemplate, [System.Text.Encoding]::UTF8) }
    $listen = "$($Cfg.Frontend.Port)"
    if ($Cfg.LoopbackOnly) { $listen = "127.0.0.1:$($Cfg.Frontend.Port)" }
    $mime = (Join-Path (Split-Path -Parent $Cfg.Frontend.Exe) 'conf\mime.types').Replace('\', '/')
    $text = $template.Replace('{{MIME}}', $mime).Replace('{{BACKEND_PORT}}', "$($Cfg.Backend.Port)").Replace('{{LISTEN}}', $listen).Replace('{{WWW}}', $Cfg.Frontend.Root.Replace('\', '/'))
    [System.IO.File]::WriteAllText((Join-Path $work 'conf\nginx.conf'), $text, $script:Utf8NoBom)
}

function Initialize-FrontendFiles($Cfg) {
    if ($Cfg.Frontend.Kind -eq 'nginx') { Initialize-NginxFiles $Cfg; return }
    foreach ($dir in $Cfg.Frontend.WorkDir, $Cfg.Frontend.CacheDir) { New-Dir $dir }
    $base = (Join-Path $Cfg.Frontend.Root 'vite.config.ts').Replace('\', '/')
    $rootDir = $Cfg.Frontend.Root.Replace('\', '/')
    $cache = $Cfg.Frontend.CacheDir.Replace('\', '/')
    $hostLine = ''
    if ($Cfg.LoopbackOnly) { $hostLine = ', host: "127.0.0.1"' }
    $text = @"
// 由 stack.ps1 生成，每次 up 都会重写，不要手改。
// 作用：照搬前端仓库自己的 vite 配置，只换掉端口和依赖缓存目录，让这一个开发服务器
// 不去碰同一份源码上另一个开发服务器的 node_modules/.vite。
// 这里故意不 import 任何包名：本文件不在前端仓库里，包名在这个位置解析不到；
// 原配置里的包名由 vite 按原文件的位置解析。
import base from "$base";

export default async function (env) {
  const resolved = typeof base === "function" ? await base(env) : await base;
  const server = Object.assign({}, resolved.server, { port: $($Cfg.Frontend.Port), strictPort: true$hostLine });
  return Object.assign({}, resolved, { root: "$rootDir", cacheDir: "$cache", server: server });
}
"@
    [System.IO.File]::WriteAllText($Cfg.Frontend.WrapperConfig, $text, $script:Utf8NoBom)
}

# ─────────────────────────── 每个服务怎么起：程序、参数、工作目录、环境变量 ───────────────────────────

# 从本机系统照搬的变量（只按名字逐个取，不整批列举）。别的一概不带。
$script:SystemEnvNames = @(
    'SystemRoot', 'SystemDrive', 'windir', 'ComSpec', 'PATHEXT', 'OS', 'COMPUTERNAME',
    'USERPROFILE', 'USERNAME', 'USERDOMAIN', 'HOMEDRIVE', 'HOMEPATH', 'APPDATA', 'LOCALAPPDATA',
    'ProgramData', 'ALLUSERSPROFILE', 'PUBLIC', 'ProgramFiles', 'ProgramFiles(x86)', 'ProgramW6432',
    'CommonProgramFiles', 'CommonProgramFiles(x86)', 'CommonProgramW6432',
    'NUMBER_OF_PROCESSORS', 'PROCESSOR_ARCHITECTURE', 'PROCESSOR_IDENTIFIER', 'PROCESSOR_LEVEL', 'PROCESSOR_REVISION'
)

function New-BaseEnv($Cfg, [string[]]$ExtraPath) {
    $envTable = [ordered]@{}
    foreach ($name in $script:SystemEnvNames) {
        $value = [Environment]::GetEnvironmentVariable($name)
        if ($null -ne $value -and $value -ne '') { $envTable[$name] = $value }
    }
    $sysRoot = [Environment]::GetEnvironmentVariable('SystemRoot')
    $paths = @()
    foreach ($p in $ExtraPath) { if ($p) { $paths += $p } }
    $paths += (Join-Path $sysRoot 'System32')
    $paths += $sysRoot
    $paths += (Join-Path $sysRoot 'System32\Wbem')
    $paths += (Join-Path $sysRoot 'System32\WindowsPowerShell\v1.0')
    $envTable['PATH'] = ($paths -join ';')
    if ($Cfg.PrivateTemp) {
        $envTable['TEMP'] = $Cfg.TmpDir
        $envTable['TMP'] = $Cfg.TmpDir
    } else {
        foreach ($name in 'TEMP', 'TMP') {
            $value = [Environment]::GetEnvironmentVariable($name)
            if ($value) { $envTable[$name] = $value }
        }
    }
    return $envTable
}

# 数据库账号口令：只从后端配置里取这两个键，谁也不打印。只有真正起数据库的那一段调用它。
function Get-DatabaseCredential($Cfg) {
    $file = $Cfg.Backend.ConfigFile
    if (-not (Test-Path -LiteralPath $file)) { throw "后端配置还没有生成：$file" }
    $user = Get-TomlTopLevelText $file 'v_user'
    $pass = Get-TomlTopLevelText $file 'v_password'
    if (-not $user -or -not $pass) { throw "后端配置 $file 里没有数据库账号（v_user / v_password 两个键）" }
    return @{ User = $user; Pass = $pass }
}

# 返回一个服务的启动说明。Env 是完整的环境变量表；SetNames 是其中由入口自己定的那些名字
# （其余是照搬系统的）；SecretNames 是起的时候才从后端配置读进来的（这里不带值，除非 -WithSecrets）。
function Get-ServiceSpec($Cfg, [string]$Name, [switch]$WithSecrets) {
    $stamp = (Get-Date).ToString('yyyyMMdd-HHmmss')
    $spec = @{
        Name = $Name
        Title = (Get-ServiceTitle $Name)
        Port = (Get-ServicePort $Cfg $Name)
        Log = (Join-Path (Join-Path $Cfg.LogsDir $Name) "$Name-$stamp.log")
        Marker = ''
        SecretNames = @()
        Late = [ordered]@{}
        # Hidden：值由入口定、但 config 命令里不逐项显示的变量（名字 → 一句说明）。
        Hidden = @{}
        # ExtraLog：服务自己另写的日志（nginx 的 error.log）。报错时主日志没有内容就看它。
        ExtraLog = ''
    }
    $set = [ordered]@{}
    if ($Name -eq 'database') {
        $spec.Exe = $Cfg.Database.Exe
        # 数据目录用相对写法（工作目录设成它的上一级）：不让盘符里的冒号混进 rocksdb: 这个前缀里。
        $spec.Cwd = Split-Path -Parent $Cfg.Database.DataDir
        $spec.Args = @('start', '--no-banner', '--bind', "$($Cfg.Database.Host):$($Cfg.Database.Port)", ('rocksdb:' + (Split-Path -Leaf $Cfg.Database.DataDir)))
        $envTable = New-BaseEnv $Cfg @()
        # 自家分支缺省的响应缓冲在 Windows 上给每次顺序请求加十几毫秒；现有的各个起库脚本都设成 0。
        $set['SURREAL_WEBSOCKET_RESPONSE_BUFFER_SIZE'] = '0'
        foreach ($key in $Cfg.Database.Env.Keys) { $set[$key] = $Cfg.Database.Env[$key] }
        $spec.SecretNames = @('SURREAL_USER', 'SURREAL_PASS')
        foreach ($key in $spec.SecretNames) { $spec.Late[$key] = '起的时候从后端配置读，不显示' }
        if ($WithSecrets) {
            $cred = Get-DatabaseCredential $Cfg
            $envTable['SURREAL_USER'] = $cred.User
            $envTable['SURREAL_PASS'] = $cred.Pass
        }
    } elseif ($Name -eq 'backend') {
        $spec.Exe = $Cfg.Backend.Exe
        $spec.Args = @()
        $spec.Cwd = $Cfg.Backend.WorkDir
        $envTable = New-BaseEnv $Cfg @()
        $set['DB_OPTION_FILE'] = $Cfg.Backend.ConfigFile
        $set['AIOS_STORE_MODE'] = 'rocksdb'
        $set['PLANT_ASSET_ROOT'] = (Join-Path $Cfg.Backend.WorkDir 'assets')
        $set['GEN_MODEL_RUN_DIR'] = $Cfg.Backend.WorkDir
        $set['AIOS_DIRECT_INDEX_DIR'] = (Join-Path $Cfg.Backend.WorkDir 'direct-index')
        $set['AIOS_E3D_SCHEMA_ROOT'] = $Cfg.Backend.SchemaRoot
        if ($Cfg.Backend.PlantUiWebRoot) { $set['PLANT_UI_WEB_ROOT'] = $Cfg.Backend.PlantUiWebRoot }
        $set['AIOS_OPEN_BROWSER'] = '0'
        $set['RUST_MIN_STACK'] = '134217728'
        $set['RUST_BACKTRACE'] = '1'
        foreach ($key in $Cfg.Backend.Env.Keys) { $set[$key] = $Cfg.Backend.Env[$key] }
    } elseif ($Name -eq 'rs_server') {
        $spec.Exe = $Cfg.RsServer.Exe
        $spec.Args = @()
        # 它的配置（DbOption.toml）、resource\、assets\、target\ 全按当前目录找。
        $spec.Cwd = $Cfg.RsServer.WorkDir
        $envTable = New-BaseEnv $Cfg @()
        $set['AIOS_STORE_MODE'] = 'rocksdb'
        $set['AIOS_DATABASE_URL'] = (Get-BackendUrl $Cfg)
        # 关掉它的自动升级：缺省它一发现库版本记录缺失或落后，就会自己去调后端的升级接口，
        # 那个接口逐库清空、重新解析。关掉之后它只等着，库版本对上了自己恢复。
        $set['AIOS_STORAGE_VERSION_UPGRADE'] = 'off'
        # 内网是纯 HTTP：登录 cookie 不能带 Secure 标记，否则浏览器不存。
        $set['SESSION_COOKIE_SECURE'] = 'false'
        $set['RUST_BACKTRACE'] = '1'
        $origins = Get-RsServerCorsOrigins $Cfg
        $set['CORS_ALLOWED_ORIGINS'] = ($origins -join ',')
        $spec.Hidden['CORS_ALLOWED_ORIGINS'] = "允许从这些页面里调它：回环、机器名、本机各个地址 × 后端和前端的端口，共 $($origins.Count) 项；不逐项显示"
        foreach ($key in $Cfg.RsServer.Env.Keys) { $set[$key] = $Cfg.RsServer.Env[$key] }
    } elseif ($Name -eq 'frontend' -and $Cfg.Frontend.Kind -eq 'nginx') {
        $spec.Exe = $Cfg.Frontend.Exe
        # -p 给的是它的运行目录（配置、日志、临时文件都在里面）；程序本身在包里，不动。
        $spec.Args = @('-p', ($Cfg.Frontend.WorkDir.Replace('\', '/') + '/'), '-c', 'conf/nginx.conf')
        $spec.Cwd = $Cfg.Frontend.WorkDir
        $spec.ExtraLog = Join-Path $Cfg.Frontend.WorkDir 'logs\error.log'
        $envTable = New-BaseEnv $Cfg @()
        foreach ($key in $Cfg.Frontend.Env.Keys) { $set[$key] = $Cfg.Frontend.Env[$key] }
    } elseif ($Name -eq 'frontend') {
        $spec.Exe = $Cfg.Frontend.Node
        $spec.Args = @((Join-Path $Cfg.Frontend.Root 'node_modules\vite\bin\vite.js'), '--config', $Cfg.Frontend.WrapperConfig)
        $spec.Cwd = $Cfg.Frontend.Root
        $spec.Marker = $Cfg.Frontend.WrapperConfig
        $nodeDir = ''
        if ($Cfg.Frontend.Node) { $nodeDir = Split-Path -Parent $Cfg.Frontend.Node }
        $envTable = New-BaseEnv $Cfg @($nodeDir)
        $backendUrl = Get-BackendUrl $Cfg
        # 前端连哪个后端，全部在这里定死（进程环境变量压过前端仓库里的 .env* 文件）：
        #   两条开发代理（/api 和 /gm）的上游 → 本套的后端；
        #   页面里的接口地址 → 同源的 /gm 代理，或本套后端的地址。
        $set['VITE_BACKEND_URL'] = $backendUrl
        $set['VITE_GEN_MODEL_V1_PROXY_TARGET'] = $backendUrl
        $set['VITE_GEN_MODEL_API_BASE_URL'] = $backendUrl
        $set['VITE_GEN_MODEL_V1_BASE_URL'] = '/gm'
        $set['NO_COLOR'] = '1'
        # 前端的 vite 配置启动时要一个提交号（界面「关于」里显示），没有就自己去调 git；
        # 入口给它的 PATH 里没有 git，所以由入口先取好再递进去。
        $spec.Late['GIT_COMMIT_FULL'] = '起的时候用 git 读前端仓库当前的提交号（只读）；本机没有 git 就不给'
        if ($WithSecrets) {
            $commit = Get-GitCommit $Cfg $Cfg.Frontend.Root
            if ($commit) { $set['GIT_COMMIT_FULL'] = $commit }
        }
        foreach ($key in $Cfg.Frontend.Env.Keys) { $set[$key] = $Cfg.Frontend.Env[$key] }
    } else {
        Stop-Stack "没有叫「$Name」的服务" 2
    }
    $inherited = @($envTable.Keys | Where-Object { $_ -ne 'PATH' -and $_ -ne 'TEMP' -and $_ -ne 'TMP' -and $spec.SecretNames -notcontains $_ })
    foreach ($key in $set.Keys) { $envTable[$key] = $set[$key] }
    $spec.Env = $envTable
    $spec.SetEnv = $set
    $spec.InheritedNames = $inherited
    return $spec
}

# 拼进 cmd 命令行的东西里不能有双引号和百分号（别的字符在引号里都是原样）。
function Test-CmdSafe([string]$Text) {
    return ($Text -notmatch '["%]')
}

function Format-CmdArg([string]$Text) {
    return '"' + $Text + '"'
}

function Set-StartInfoEnv($StartInfo, $EnvTable) {
    # .NET 4.6 起有保留大小写的 Environment；更老的只有 EnvironmentVariables（会把名字变成小写）。
    $envDict = $null
    if ($StartInfo.PSObject.Properties['Environment']) { $envDict = $StartInfo.Environment } else { $envDict = $StartInfo.EnvironmentVariables }
    $envDict.Clear()
    foreach ($key in $EnvTable.Keys) { $envDict[$key] = [string]$EnvTable[$key] }
}

# 跑一个很快就结束的程序（问版本用），拿回它的输出。同样给明确的环境变量和工作目录。
function Invoke-Capture($Cfg, [string]$Exe, [string]$ArgLine, [string]$Cwd, [int]$TimeoutSec = 20) {
    $out = @{ Ok = $false; Text = ''; TimedOut = $false }
    try {
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = $Exe
        $psi.Arguments = $ArgLine
        $psi.WorkingDirectory = $Cwd
        $psi.UseShellExecute = $false
        $psi.CreateNoWindow = $true
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        Set-StartInfoEnv $psi (New-BaseEnv $Cfg @())
        $process = [System.Diagnostics.Process]::Start($psi)
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit($TimeoutSec * 1000)) {
            # 这个进程是这里刚起的（手里拿着它的进程对象），不是按进程号或名字找来的。
            try { $process.Kill() } catch { }
            $out.Text = "等了 $TimeoutSec 秒没有结束"
            $out.TimedOut = $true
            return $out
        }
        $text = "$($stdout.Result)".Trim()
        if (-not $text) { $text = "$($stderr.Result)".Trim() }
        $out.Text = (($text -split "`r?`n") | Select-Object -First 1)
        $out.Ok = ($process.ExitCode -eq 0)
    } catch {
        $out.Text = $_.Exception.Message
    }
    return $out
}

# 一个 git 仓库当前的提交号（只读地问一句 rev-parse HEAD）。本机没有 git、或那不是仓库，就返回空。
function Get-GitCommit($Cfg, [string]$Dir) {
    $git = Get-Command git -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $git -or -not (Test-Path -LiteralPath $Dir -PathType Container)) { return '' }
    $r = Invoke-Capture $Cfg $git.Path '--no-optional-locks rev-parse HEAD' $Dir 15
    if ($r.Ok -and $r.Text -match '^[0-9a-f]{40}$') { return $r.Text }
    return ''
}

# 内部命令：真正把一个服务起起来。up 通过「另开一个隐藏的 PowerShell」来调它，
# 这样长命的服务进程不会继承调用方的输出管道（否则调用方永远等不到输出结束）。
# 这里再经 cmd 把服务的输出接到日志文件，并且给一份明确列出的环境变量。
function Invoke-Spawn($Cfg, [string]$Name) {
    $resultFile = Join-Path $Cfg.RunDir "$Name.spawn.json"
    $errorFile = Join-Path $Cfg.RunDir "$Name.spawn.err.txt"
    try {
        $spec = Get-ServiceSpec $Cfg $Name -WithSecrets
        New-Dir (Split-Path -Parent $spec.Log)
        if ($Cfg.PrivateTemp) { New-Dir $Cfg.TmpDir }
        $parts = @((Format-CmdArg $spec.Exe))
        foreach ($arg in $spec.Args) { $parts += (Format-CmdArg $arg) }
        $inner = ($parts -join ' ') + ' <NUL 1>>' + (Format-CmdArg $spec.Log) + ' 2>&1'
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = Join-Path ([Environment]::GetEnvironmentVariable('SystemRoot')) 'System32\cmd.exe'
        $psi.Arguments = '/d /s /c "' + $inner + '"'
        $psi.WorkingDirectory = $spec.Cwd
        $psi.UseShellExecute = $false
        $psi.CreateNoWindow = $true
        Set-StartInfoEnv $psi $spec.Env
        $process = [System.Diagnostics.Process]::Start($psi)
        $wrapper = Get-ProcInfo $process.Id
        $doc = [ordered]@{ wrapperPid = $process.Id; wrapperTicks = (Get-ProcTicks $wrapper); log = $spec.Log }
        [System.IO.File]::WriteAllText($resultFile, ($doc | ConvertTo-Json), $script:Utf8NoBom)
    } catch {
        try { [System.IO.File]::WriteAllText($errorFile, $_.Exception.Message, $script:Utf8NoBom) } catch { }
    }
}

# 服务输出的最后几行。可能带账号信息的行（数据库首次建库时会提到账号名）不回显。
function Get-LogTail([string]$Path, [int]$Count = 20) {
    if (-not $Path -or -not (Test-Path -LiteralPath $Path)) { return @() }
    try {
        $stream = New-Object System.IO.FileStream -ArgumentList $Path, ([System.IO.FileMode]::Open), ([System.IO.FileAccess]::Read), ([System.IO.FileShare]::ReadWrite)
        try {
            $reader = New-Object System.IO.StreamReader -ArgumentList $stream, ([System.Text.Encoding]::UTF8)
            $all = $reader.ReadToEnd()
        } finally { $stream.Dispose() }
        $lines = @($all -split "`r?`n" | Where-Object { $_.Trim().Length -gt 0 })
        if ($lines.Count -gt $Count) { $lines = $lines[($lines.Count - $Count)..($lines.Count - 1)] }
        $out = @()
        foreach ($line in $lines) {
            $text = "$line" -replace "$([char]27)\[[0-9;]*m", ''
            if ($text -match '(?i)root user|credential|password|passwd|secret|token') { $text = '（这一行可能带账号信息，不回显；要看请直接打开日志文件）' }
            $out += (Limit-Text $text 300)
        }
        return $out
    } catch { return @("（读不了日志：$($_.Exception.Message)）") }
}

# ─────────────────────────── 检查：这个服务算不算起好了 ───────────────────────────

function Test-ServiceHealth($Cfg, [string]$Name) {
    $out = @{ Ok = $false; Version = ''; Detail = ''; Reason = ''; Checked = ''; Json = $null }
    if ($Name -eq 'database') {
        $url = "http://$(Get-DatabaseProbeHost $Cfg):$($Cfg.Database.Port)/version"
        $out.Checked = "GET $url 要回 2.1.x 的版本号"
        $r = Invoke-StackHttp 'GET' $url '' 3
        if (-not $r.Ok) { $out.Reason = "没有应答（$($r.Error)）"; return $out }
        $body = $r.Body.Trim()
        if ($body -notmatch '(?<!\d)2\.1\.\d') { $out.Reason = "这个端口上答话的不是 2.1.x 的数据库，而是「$(Limit-Text $body 80)」。本系统只能用自带的 2.1.4；不要按 3.x 的提示去迁移数据"; return $out }
        $out.Ok = $true
        $out.Version = $body
        return $out
    }
    if ($Name -eq 'backend') {
        $url = (Get-BackendUrl $Cfg) + '/api/v1/health'
        $out.Checked = "GET $url 要有应答、应答来自本套的后端、并且它已经连上数据库"
        $r = Invoke-StackHttp 'GET' $url '' 8
        if ($r.Status -eq 0) { $out.Reason = "没有应答（$($r.Error)）"; return $out }
        $j = ConvertFrom-JsonSafe $r.Body
        if ($null -eq $j) { $out.Reason = "应答不是健康检查的格式（HTTP $($r.Status)）"; return $out }
        $out.Json = $j
        $out.Version = [string](Get-Prop $j 'build_id')
        if (-not $out.Version) { $out.Version = [string](Get-Prop $j 'version') }
        if ([string](Get-Prop $j 'mode') -eq 'setup') {
            $out.Ok = $true
            $out.Detail = '设置模式：配置还不完整，等着在向导页里填'
            return $out
        }
        $bind = [string](Get-Prop (Get-Prop $j 'http') 'bind_addr')
        if ($bind -and -not $bind.EndsWith(':' + $Cfg.Backend.Port)) { $out.Reason = "应答来自另一个后端（它监听 $bind），不是本套的"; return $out }
        $connected = Get-Prop (Get-Prop $j 'sul_db') 'connected'
        if ($connected -ne $true) { $out.Reason = '后端有应答，但它还没连上数据库（sul_db.connected 不是 true）'; return $out }
        $status = [string](Get-Prop $j 'status')
        $out.Ok = $true
        $out.Detail = "已连数据库；自报 status=$status"
        if ($status -ne 'ok') {
            $degraded = @(Get-Prop $j 'degraded_sections') -join ','
            $blocking = @(Get-Prop $j 'blocking_conditions') -join ','
            $out.Detail += "（降级项：$degraded；阻断项：$blocking）"
        }
        return $out
    }
    if ($Name -eq 'rs_server') {
        $url = (Get-RsServerUrl $Cfg) + '/api/metadata/meta'
        $out.Checked = "GET $url 要有应答，并且它连的是本套的数据库"
        $r = Invoke-StackHttp 'GET' $url '' 8
        if (-not $r.Ok) { $out.Reason = "没有应答（$($r.Error)）"; return $out }
        $j = ConvertFrom-JsonSafe $r.Body
        if ($null -eq $j) { $out.Reason = '应答不是预期的格式'; return $out }
        $out.Json = $j
        $out.Version = 'rs-server ' + [string](Get-Prop $j 'version')
        $address = [string](Get-Prop (Get-Prop $j 'database') 'address')
        if ($address -and -not $address.TrimEnd('/').EndsWith(':' + $Cfg.Database.Port)) { $out.Reason = "应答来自另一个 rs-server（它连的数据库是 $address），不是本套的"; return $out }
        $out.Ok = $true
        $out.Detail = "连的是本套的数据库；项目 $([string](Get-Prop (Get-Prop $j 'project') 'name'))"
        return $out
    }
    $url = (Get-FrontendUrl $Cfg) + '/'
    $healthPath = '/api/v1/health'
    if ($Cfg.Frontend.Kind -eq 'vite-dev') { $healthPath = '/gm/api/v1/health' }
    $out.Checked = "GET $url 要回前端首页，并且经它转发的 /api/v1/health 来自本套的后端"
    $r = Invoke-StackHttp 'GET' $url '' 8
    if (-not $r.Ok) { $out.Reason = "没有应答（$($r.Error)）"; return $out }
    if ($r.Body -notmatch 'id="app"') { $out.Reason = '应答不是前端首页（里面没有 id="app"）'; return $out }
    $proxied = Invoke-StackHttp 'GET' ((Get-FrontendUrl $Cfg) + $healthPath) '' 8
    if ($proxied.Status -eq 0) { $out.Reason = "首页有了，但经前端转发的后端健康检查没通（$($proxied.Error)）"; return $out }
    $pj = ConvertFrom-JsonSafe $proxied.Body
    if ($null -eq $pj) { $out.Reason = "首页有了，但经前端转发的后端健康检查没有拿到正常应答（HTTP $($proxied.Status)）。多半是后端没在跑"; return $out }
    $bind = [string](Get-Prop (Get-Prop $pj 'http') 'bind_addr')
    if ([string](Get-Prop $pj 'mode') -ne 'setup' -and -not $bind.EndsWith(':' + $Cfg.Backend.Port)) {
        $out.Reason = "前端把接口请求转给了别的后端（那一个监听「$bind」），不是本套的 $($Cfg.Backend.Port)"
        return $out
    }
    $out.Ok = $true
    if ($Cfg.Frontend.Kind -eq 'nginx') {
        $version = ''
        try { $version = [string]((ConvertFrom-JsonSafe ([System.IO.File]::ReadAllText((Join-Path $Cfg.Frontend.Root 'version.json'), [System.Text.Encoding]::UTF8))).version) } catch { }
        $out.Version = "plant3d-web $version（nginx）"
    } else {
        $version = ''
        try { $version = [string]((ConvertFrom-JsonSafe ([System.IO.File]::ReadAllText((Join-Path $Cfg.Frontend.Root 'package.json'), [System.Text.Encoding]::UTF8))).version) } catch { }
        $out.Version = "plant3d-web $version（开发服务器）"
    }
    $out.Detail = '接口请求转给本套的后端'
    return $out
}

# 把后端健康检查里和库版本有关的几格归成一种情况。这个函数不碰系统（测试直接喂假数据）。
# $Health 是 /api/v1/health 解析出来的对象（$null = 后端没有应答）。Kind：
#   ok             库里的版本记录和程序要的一致
#   starting       还没有版本记录，后端还在做启动（initialization.status = discovering）——全新的库，
#                  缺省配置下后端做完启动时自己会写上（样例工程十来秒）
#   awaiting-model 还没有版本记录，后端的启动做完了，但它管的库的模型面还没就绪（initialization.model_ready 不是 true）：
#                  要么有库正在初始化（做完会写），要么在等库被初始化——配了 watch_dbnums 的新库就停在这里，
#                  第一次初始化完成才写版本记录
#   no-record      没有版本记录，而模型面已经就绪——它不会自己补写了（库里已经有数据、却没有记录）
#   old / newer    有版本记录，比程序要的旧 / 新
#   upgrading      后端正在做库的升级
#   upgrade-failed 上一次升级没有全部成功
#   setup          后端在设置模式（配置还没填完）
#   unknown        后端没有应答，或者它还没核对过库版本
# 等得到的只有：starting、upgrading、unknown，和 awaiting-model 里「后端还在忙」的那一半。别的等多久都不会变。
# 这些是 2026-11-12 在沙箱里对着后端 0.1.44（r4、r5 两个构建）一种一种看出来的，见 output\stack-fix-20261112。
function Get-StorageVersionKind($Health) {
    $out = @{ Kind = 'unknown'; Known = $false; CaughtUp = $false; Text = '后端没有应答'; Stored = $null; Expected = $null; ModelReady = $false; InitStatus = ''; Watch = @(); Idle = $false; UpgradeDone = 0; UpgradeTotal = 0 }
    if ($null -eq $Health) { return $out }
    if ([string](Get-Prop $Health 'mode') -eq 'setup') { $out.Kind = 'setup'; $out.Text = '后端在设置模式（配置还没填完）'; return $out }
    $sv = Get-Prop $Health 'storage_version'
    if ($null -eq $sv) { $out.Text = '后端还没核对过库版本'; return $out }
    $out.Known = $true
    $out.CaughtUp = ((Get-Prop $sv 'caught_up') -eq $true)
    $out.Stored = Get-Prop $sv 'stored'
    $out.Expected = Get-Prop $sv 'expected'
    $init = Get-Prop $Health 'initialization'
    $out.ModelReady = ((Get-Prop $init 'model_ready') -eq $true)
    $out.InitStatus = [string](Get-Prop $init 'status')
    $out.Watch = @(Get-Prop $Health 'watch_dbnums' | Where-Object { $null -ne $_ })
    $shown = $out.Stored
    if ($null -eq $shown) { $shown = '还没有' }
    $out.Text = "库里的版本记录：$shown；程序要的：$($out.Expected)"
    $upgrade = Get-Prop $sv 'upgrade'
    $upgradeState = [string](Get-Prop $upgrade 'state')
    $out.UpgradeDone = [int](Get-Prop $upgrade 'done')
    $out.UpgradeTotal = @(Get-Prop $upgrade 'requested' | Where-Object { $null -ne $_ }).Count
    if ($out.CaughtUp) { $out.Kind = 'ok' }
    elseif ($upgradeState -eq 'running') { $out.Kind = 'upgrading' }
    elseif ($upgradeState -eq 'failed') { $out.Kind = 'upgrade-failed' }
    elseif ($null -eq $out.Stored) {
        if ($out.ModelReady) { $out.Kind = 'no-record' }
        elseif ($out.InitStatus -eq '' -or $out.InitStatus -eq 'discovering') { $out.Kind = 'starting' }
        else { $out.Kind = 'awaiting-model' }
    }
    elseif ([long]$out.Stored -lt [long]$out.Expected) { $out.Kind = 'old' }
    else { $out.Kind = 'newer' }
    return $out
}

# 问一次后端的健康检查，返回上面那个归类。
function Get-StorageVersionState($Cfg) {
    $r = Invoke-StackHttp 'GET' ((Get-BackendUrl $Cfg) + '/api/v1/health') '' 8
    return (Get-StorageVersionKind (ConvertFrom-JsonSafe $r.Body))
}

# 库版本没对上的时候，一句话的「下一步」（状态表里用）。
function Get-StorageVersionHint($Sv) {
    if ($Sv.Kind -eq 'starting') { return '库是新的，后端做完启动时自己会写版本记录：过一会儿再运行 stack up，它只把 rs-server 补起来' }
    if ($Sv.Kind -eq 'awaiting-model') { return '库里还没有版本记录，后端要等第一次初始化完成才写：先初始化要用的库（stack init <库号>），做完再运行 stack up' }
    if ($Sv.Kind -eq 'upgrading') { return '后端正在做库的升级：等它做完再运行 stack up' }
    if ($Sv.Kind -eq 'setup') { return '先在后端的向导页里把配置填完，再运行 stack up' }
    if ($Sv.Kind -eq 'unknown') { return '运行 stack status 看后端是不是正常' }
    return '后端不会自己把它对上，再等也没有用：照下面的说明选一种办法'
}

# 库版本没对上、所以先不起 rs-server 的时候，把话说具体：是哪种情况、等不等得到、在哪做什么、做了会怎样。
function Show-StorageVersionAdvice($Cfg, $Sv) {
    $ops = (Get-BackendUrl $Cfg) + '/ops.html'
    $c = 'Yellow'
    if ($Sv.Kind -eq 'setup') {
        Say '  这是哪种情况：后端在设置模式（它的配置还不完整），这时候没有库版本可核对。' $c
        Say "  下一步：浏览器打开 $(Get-BackendUrl $Cfg)/ ，按向导把配置填完；然后再运行 stack up，它会把 rs-server 补起来。" $c
        return
    }
    if ($Sv.Kind -eq 'unknown') {
        Say "  这是哪种情况：$($Sv.Text)。" $c
        Say '  下一步：运行 stack status 看后端是不是正常；正常之后再运行 stack up。' $c
        return
    }
    if ($Sv.Kind -eq 'starting') {
        Say '  这是哪种情况：库是新的，还没有版本记录；后端的启动还没做完——做完的时候它自己会写上（样例工程十来秒，大的工程要久一些）。' $c
        Say '  下一步：过一会儿再运行 stack up（双击「开始」），它只把 rs-server 补起来，别的服务不动。' $c
        Say '          每次都等不到：在 stack.toml 里加一节 [timeouts]，写 storage_version_sec = 600，让它多等一会儿。' $c
        return
    }
    $watch = @($Sv.Watch | Where-Object { $null -ne $_ })
    if ($Sv.Kind -eq 'awaiting-model') {
        if (-not $Sv.Idle) {
            Say '  这是哪种情况：库里还没有版本记录；后端的启动做完了，现在还在忙（有库在初始化或生成模型），做完的时候会写上。' $c
            Say "  下一步：等它做完（进度看运维页 $ops ）再运行 stack up，它只把 rs-server 补起来。" $c
            return
        }
        Say '  这是哪种情况：库里还没有版本记录。后端的启动已经做完、现在闲着，但它管的库的模型面还没有就绪——' $c
        Say '          版本记录要等第一次初始化完成的时候才写，光等不会有（所以没有等满）。' $c
        $example = '<库号>'
        if ($watch.Count -gt 0) {
            Say "          配置里写了只让后端管库 $($watch -join '、')（watch_dbnums）：这种配置下，这几个库初始化完之前都是这样。" $c
            $example = (@($watch | Select-Object -First 6) -join ' ')
        }
        Say "  下一步：先初始化要用的库——stack init $example（或者在 plant3d-web 里初始化）；做完再运行 stack up，它会把 rs-server 补起来。" $c
        Say '          初始化完如果还是对不上（这份数据目录是别的版本的后端用过的，就会这样），入口会再说明怎么办。' $c
        Say '          用不着 rs-server（元数据网页、提资这些接口）：在 stack.toml 的 [rs_server] 一节里写 enabled = false。' $c
        return
    }
    if ($Sv.Kind -eq 'upgrading') {
        Say "  这是哪种情况：后端正在做库的升级（逐个库清空、重新解析、重新生成模型；已完成 $($Sv.UpgradeDone) / $($Sv.UpgradeTotal) 个库）。" $c
        Say "  下一步：等它做完再运行 stack up。进度看运维页 $ops 。" $c
        return
    }
    if ($Sv.Kind -eq 'upgrade-failed') {
        Say '  这是哪种情况：上一次库升级没有全部成功，版本记录因此没有写上。' $c
        Say "  下一步：打开运维页 $ops 看是哪几个库、为什么没成；处理之后在那里重新点「升级数据库」。也可以照下面第 2 条另起一个新库。" $c
    } elseif ($Sv.Kind -eq 'no-record') {
        Say '  这是哪种情况：库里已经有数据，却没有版本记录，而后端的启动已经做完——它不会自己补写，再等也没有用。' $c
        Say '          这样的库多半是别的版本的后端用过的数据目录（比如换新一版时接过来的旧 data\surreal），' $c
        Say '          或者版本记录还没写上的时候初始化失败过。' $c
    } elseif ($Sv.Kind -eq 'old') {
        Say "  这是哪种情况：库里的版本记录（$($Sv.Stored)）比程序要的（$($Sv.Expected)）旧——这份数据是老一版的后端建的，现在的后端读不懂里面的模型。它不会自己变，再等也没有用。" $c
    } else {
        Say "  这是哪种情况：库里的版本记录（$($Sv.Stored)）比程序要的（$($Sv.Expected)）新——这份数据是更新的后端建的，这一版读不了它。" $c
        Say '  下一步：换回建这份数据的那一版（或更新的）安装包；或者照下面第 2 条另起一个新库。' $c
    }
    Say '  怎么办（选一种）：' $c
    Say '    1. 用不着 rs-server（元数据网页、提资这些接口）：什么都不用做，plant3d-web、后端、plant-ui 网页版照常能用。' $c
    Say '       不想每次「开始」都看到这段话：在 stack.toml 的 [rs_server] 一节里写 enabled = false。' $c
    Say "    2. 库里的东西可以重来（初始化过的库可以再初始化）：stack down → 把文件夹 $($Cfg.Database.DataDir) 改个名留作备份 → stack up。" $c
    Say '       全新的库后端自己会写版本记录；然后再初始化要的库。备份确认用不着了再删。' $c
    if ($Sv.Kind -ne 'newer') {
        if ($watch.Count -gt 0) {
            # 在沙箱里试过：只管几个库时做升级，升级本身成功，但版本记录不写，rs-server 还是起不来。
            Say "    3. 在后端做「升级」这条路眼下走不通：配置里写了只让后端管库 $($watch -join '、')（watch_dbnums），" $c
            Say '       而后端要把这个 MDB 的全部设计库都升级过才写版本记录。要走这条路，先把 watch_dbnums 去掉' $c
            Say '       （stack.toml 的 [backend] 一节和 work\backend\DbOption.toml 里各有一处），stack down、stack up，再照下面做：' $c
            Say "       浏览器打开运维页 $ops ，点页面顶上数据库架构版本提示里的「升级数据库」——它会把全部设计库逐个清空、重新解析、重新生成模型。" $c
        } else {
            Say "    3. 要留着这个库：在后端做「升级」——浏览器打开运维页 $ops ，页面顶上有一条数据库架构版本的提示，点里面的「升级数据库」。" $c
            Say '       它会把这个 MDB 里的全部设计库（不只是初始化过的那几个）逐个清空、重新解析、重新生成模型；库多、库大的工程要很久、很吃内存。' $c
            Say '       全部做完后端才写版本记录；做完再运行 stack up，它会把 rs-server 补起来。' $c
        }
    }
}

# 这些占着端口的进程里，有没有本套记下的那个进程或它真正的子进程（nginx 的监听可能挂在子进程名下）。
function Test-PortOwnedBy($Owners, [int]$ProcessId, [string]$Ticks = '') {
    if (@($Owners).Count -eq 0) { return $false }
    if (@($Owners) -contains $ProcessId) { return $true }
    foreach ($kid in (Get-ChildProcs $ProcessId $Ticks)) { if (@($Owners) -contains [int]$kid.ProcessId) { return $true } }
    return $false
}

# 一个服务此刻的状态。State：ok 正常 / stopped 没起 / waiting 在等前面的条件 / bad 异常
function Get-ServiceState($Cfg, $State, [string]$Name) {
    $port = Get-ServicePort $Cfg $Name
    $info = @{ Name = $Name; Title = (Get-ServiceTitle $Name); State = 'stopped'; Reason = ''; Pid = 0; Port = $port; Version = ''; Start = ''; MemGb = 0; Log = ''; Detail = ''; Checked = ''; Next = ''; Ours = $false; Unreadable = $false; Sv = $null }
    $record = $null
    if ($State.ContainsKey($Name)) { $record = $State[$Name] }
    $owners = Get-PortOwners $port
    if ($null -eq $record) {
        if ($owners.Count -gt 0) {
            $info.State = 'bad'
            $info.Checked = "本套有没有起它的记录；端口 $port 上有没有进程在听"
            $ownerProc = Get-ProcInfo ([int]$owners[0])
            if ($null -ne $ownerProc -and -not $ownerProc.ExecutablePath) {
                # 读不到占用者的程序路径：不能一口咬定它是别人的——用管理员窗口起的本套服务，从普通窗口看就是这样。
                $info.Reason = "本套没有起它的记录，而端口 $port 被进程 $($owners[0])（$($ownerProc.Name)）占着；这个窗口读不到它的程序路径，判断不了它是不是本套的"
                $info.Next = '如果它是本套的（比如当时是用「以管理员身份运行」起的），请用同样的方式打开窗口再运行；不是的话，停掉它，或把配置里这个服务的 port 换一个，再运行 stack up'
            } else {
                $info.Reason = "本套没起它，但端口 $port 被别的进程占着：$(Format-ProcBrief $owners[0])"
                $info.Next = "那个进程不是本套起的，入口不会动它。停掉它，或把配置里这个服务的 port 换一个，再运行 stack up"
            }
        } elseif ($Name -eq 'rs_server' -and $Cfg.RsServer.WaitStorageVersion -and $State.ContainsKey('backend')) {
            # 后端在跑而 rs-server 没起：多半是库版本还没对上。
            $sv = Get-StorageVersionState $Cfg
            # 「启动做完了、模型面没就绪」这一种：后端此刻闲不闲，决定了话怎么说（闲着 = 在等库被初始化；在忙 = 做完会写）。
            if ($sv.Kind -eq 'awaiting-model') { $sv.Idle = [bool](Test-BackendIdle $Cfg) }
            if ($sv.Kind -ne 'ok' -and $sv.Kind -ne 'unknown') {
                $info.State = 'waiting'
                $info.Checked = '后端健康检查里库版本是不是已经一致（一致之后才起 rs-server）'
                $info.Reason = "库版本还没对上，所以没起它（$($sv.Text)）"
                $info.Next = Get-StorageVersionHint $sv
                $info.Sv = $sv
            }
        }
        return $info
    }
    $info.Log = $record.log
    $check = Test-RecordedProcess $Cfg $Name $record
    if (-not $check.Ours) {
        $info.State = 'bad'
        $info.Checked = "本套记下的那个进程（进程号 $($record.pid)，起于 $($record.startedAt)）还在不在、是不是原来那一个"
        $info.Reason = "$($check.Why)。本套上次起它是在 $($record.startedAt)"
        $info.Next = '运行 stack up：它会把这一个重新起起来，别的服务不动'
        if ($check.Unreadable) {
            $info.Pid = $record.pid
            $info.Unreadable = $true
            $info.Next = '用起它时同样的方式打开窗口（比如都用管理员、或都不用）再运行 stack status / stack up / stack down；如果它其实正在退出，过几秒再看'
        } elseif ($owners.Count -gt 0 -and -not (Test-PortOwnedBy $owners $record.pid $record.startTicks)) {
            $info.Reason += "；端口 $port 现在被 $(Format-ProcBrief $owners[0]) 占着"
            $info.Next = "端口 $port 现在被别的进程占着，入口不会动它。先停掉那个进程（或换端口），再运行 stack up"
        } elseif ($owners.Count -eq 0) {
            # 记录里的进程不在了、端口也空着，但运行目录里这个程序另有进程在跑：多半是刚被重启、还没占上端口。
            $strays = @(Get-ServiceProgramProcs $Cfg $Name)
            if ($strays.Count -gt 0) {
                $info.Reason += "；运行目录里这个程序另有进程在跑（进程 $(@($strays | ForEach-Object { $_.ProcessId }) -join '、')），只是还没占上本套的端口 $port"
                $info.Next = '如果刚在后端的向导页里点过「重启」，过几秒再运行 stack status，入口会把它认领回来；它一直不占这个端口的话，运行 stack up，照它说的做'
            }
        }
        return $info
    }
    $info.Ours = $true
    $info.Pid = $record.pid
    $info.Start = ([datetime]$check.Proc.CreationDate).ToString('HH:mm:ss')
    $info.MemGb = Get-ProcMemGb $record.pid
    $health = Test-ServiceHealth $Cfg $Name
    $info.Version = $health.Version
    $info.Detail = $health.Detail
    $info.Checked = $health.Checked
    if ($health.Ok) { $info.State = 'ok' }
    else {
        $info.State = 'bad'
        $info.Reason = "进程在，但检查没通过：$($health.Reason)"
        $info.Next = '如果是刚起来，过一会儿再运行 stack status；如果它依赖的服务（前端依赖后端，后端依赖数据库）不正常，先运行 stack up 把那个补起来；一直不好就 stack down 之后再 stack up'
    }
    return $info
}

function Show-StatusTable($Rows) {
    Say ((Format-Cell '服务' 11) + (Format-Cell '状态' 6) + (Format-Cell '端口' 7) + (Format-Cell '进程号' 8) + (Format-Cell '起于' 10) + (Format-Cell '内存' 9) + '版本 / 说明')
    foreach ($row in $Rows) {
        $stateText = '没起'
        $color = 'DarkGray'
        if ($row.State -eq 'ok') { $stateText = '正常'; $color = 'Green' }
        elseif ($row.State -eq 'bad') { $stateText = '异常'; $color = 'Red' }
        elseif ($row.State -eq 'waiting') { $stateText = '等待'; $color = 'Yellow' }
        $pidText = '-'
        if ($row.Pid -gt 0) { $pidText = "$($row.Pid)" }
        $memText = '-'
        if ($row.MemGb -gt 0) { $memText = "$($row.MemGb) GB" }
        $startText = '-'
        if ($row.Start) { $startText = $row.Start }
        $tail = $row.Version
        if ($row.State -eq 'ok' -and $row.Detail) { $tail = "$tail；$($row.Detail)" }
        if ($row.State -ne 'ok' -and $row.Reason) { $tail = $row.Reason }
        Say ((Format-Cell $row.Title 11) + (Format-Cell $stateText 6) + (Format-Cell "$($row.Port)" 7) + (Format-Cell $pidText 8) + (Format-Cell $startText 10) + (Format-Cell $memText 9) + $tail) $color
    }
}

# ─────────────────────────── 预检 ───────────────────────────
# 每一处问题都说四样：是什么、查的是什么、结果、下一步。

function Invoke-Preflight($Cfg, $State) {
    $problems = New-Object System.Collections.ArrayList
    $notes = New-Object System.Collections.ArrayList
    $names = Get-ServiceNames $Cfg

    function Add-Problem([string]$What, [string]$Checked, [string]$Result, [string]$Next) {
        [void]$problems.Add(@{ What = $What; Checked = $Checked; Result = $Result; Next = $Next })
    }
    function Add-Note([string]$Text) { [void]$notes.Add($Text) }

    # 程序
    $exeChecks = @(@{ What = '数据库程序'; Path = $Cfg.Database.Exe; Key = '[database] exe' }, @{ What = '后端程序'; Path = $Cfg.Backend.Exe; Key = '[backend] exe' })
    if ($Cfg.RsServer.Enabled) { $exeChecks += @{ What = 'rs-server 程序'; Path = $Cfg.RsServer.Exe; Key = '[rs_server] exe' } }
    if ($Cfg.Frontend.Enabled -and $Cfg.Frontend.Kind -eq 'nginx') { $exeChecks += @{ What = 'nginx 程序'; Path = $Cfg.Frontend.Exe; Key = '[frontend] exe' } }
    foreach ($c in $exeChecks) {
        if (-not (Test-Path -LiteralPath $c.Path -PathType Leaf)) {
            Add-Problem "$($c.What)不在" "配置 $($c.Key) 指的文件在不在" "没有这个文件：$($c.Path)" "把程序放到这个位置，或改配置里的 $($c.Key)"
            continue
        }
        if ($Cfg.Guard.RequireExeUnderRoot -and -not (Test-UnderPath $c.Path $Cfg.Root)) {
            Add-Problem "$($c.What)不在本套的目录里" "配置 $($c.Key) 是不是在运行目录 $($Cfg.Root) 下面" "它在 $($c.Path)" "入口靠程序路径认进程：请把程序拷一份到运行目录里（比如 bin\）再指过去，免得和别的实例分不清。确实只有这一个实例时，可以把 [guard] require_exe_under_root 设成 false"
        }
    }
    # 工程目录
    if (-not (Test-Path -LiteralPath $Cfg.Project.E3dRoot -PathType Container)) {
        Add-Problem 'E3D 工程根目录不在' '配置 [project] e3d_root 指的目录在不在' "没有这个目录：$($Cfg.Project.E3dRoot)" '核对路径；如果是共享盘，确认这台机器、这个账号能访问'
    } else {
        $projectDir = Join-Path $Cfg.Project.E3dRoot $Cfg.Project.Name
        if (-not (Test-Path -LiteralPath $projectDir -PathType Container)) {
            Add-Problem '工程根目录下没有这个项目' "配置 [project] name（$($Cfg.Project.Name)）在工程根目录下有没有同名文件夹" "没有：$projectDir" '核对 [project] name 和 e3d_root'
        } elseif (@(Get-ChildItem -LiteralPath $projectDir -Directory | Where-Object { $_.Name -match '^[A-Za-z0-9]{2,4}000$' }).Count -eq 0) {
            Add-Problem '项目文件夹不像一个 E3D 项目' "$projectDir 下面有没有形如 xxx000 的库目录" '一个都没有' '核对 [project] name 和 e3d_root 指的是不是 E3D 的项目目录'
        }
    }
    # 后端要用的三样东西
    if (-not (Test-Path -LiteralPath $Cfg.Backend.ConfigTemplate -PathType Leaf)) { Add-Problem '后端配置模板不在' '配置 [backend] config_template 指的文件在不在' "没有：$($Cfg.Backend.ConfigTemplate)" '改成后端配置模板（DbOption.factory.toml）的实际位置' }
    if (@(Get-ChildItem -LiteralPath (Join-Path $Cfg.Backend.ResourceDir 'surreal') -Filter '*.surql' -File -ErrorAction SilentlyContinue).Count -eq 0) { Add-Problem '数据库函数脚本不在' '配置 [backend] resource_dir 下面有没有 surreal\*.surql' "没有：$(Join-Path $Cfg.Backend.ResourceDir 'surreal')" '改成后端 resource 目录的实际位置（没有这些脚本，后端连上库之后就退出）' }
    if (-not (Test-Path -LiteralPath (Join-Path (Join-Path $Cfg.Backend.SchemaRoot $Cfg.Project.E3dVersion) 'manifest.json'))) { Add-Problem 'E3D 库结构定义不在' "配置 [backend] schema_root 下面有没有 $($Cfg.Project.E3dVersion)\manifest.json" "没有：$(Join-Path $Cfg.Backend.SchemaRoot $Cfg.Project.E3dVersion)" '改成库结构定义（schema）目录的实际位置；e3d_version 要和里面的子目录同名' }
    if ($Cfg.Backend.PlantUiWebRoot -and -not (Test-Path -LiteralPath (Join-Path $Cfg.Backend.PlantUiWebRoot 'index.html'))) { Add-Problem 'plant-ui 网页目录不对' '配置 [backend] plant_ui_web_root 下面有没有 index.html' "没有：$($Cfg.Backend.PlantUiWebRoot)" '改成 plant-ui 网页产物所在的目录；不用后端自带的界面可以把这一项删掉' }
    # rs-server 要的运行文件
    if ($Cfg.RsServer.Enabled) {
        if (@(Get-ChildItem -LiteralPath (Join-Path $Cfg.RsServer.TemplateDir 'resource\surreal') -Filter '*.surql' -File -ErrorAction SilentlyContinue).Count -eq 0) {
            Add-Problem 'rs-server 的运行文件不在' '配置 [rs_server] template_dir 下面有没有 resource\surreal\*.surql' "没有：$(Join-Path $Cfg.RsServer.TemplateDir 'resource\surreal')" '改成放 rs-server 运行文件（resource、assets）的目录；没有这些脚本它起不来。不用 rs-server 就把 [rs_server] enabled 设成 false'
        }
    }
    # 前端
    if ($Cfg.Frontend.Enabled -and $Cfg.Frontend.Kind -eq 'nginx') {
        if (-not (Test-Path -LiteralPath (Join-Path $Cfg.Frontend.Root 'index.html'))) { Add-Problem '前端产物目录不对' '配置 [frontend] root 下面有没有 index.html' "没有：$($Cfg.Frontend.Root)" '改成前端构建产物所在的目录' }
        if ($Cfg.Frontend.Exe -and (Test-Path -LiteralPath $Cfg.Frontend.Exe -PathType Leaf) -and -not (Test-Path -LiteralPath (Join-Path (Split-Path -Parent $Cfg.Frontend.Exe) 'conf\mime.types'))) { Add-Problem 'nginx 缺文件' 'nginx.exe 旁边有没有 conf\mime.types' "没有：$(Join-Path (Split-Path -Parent $Cfg.Frontend.Exe) 'conf\mime.types')" '把 nginx 自带的 conf\mime.types 放回去' }
        if ($Cfg.Frontend.ConfTemplate -and -not (Test-Path -LiteralPath $Cfg.Frontend.ConfTemplate -PathType Leaf)) { Add-Problem 'nginx 配置模板不在' '配置 [frontend] conf_template 指的文件在不在' "没有：$($Cfg.Frontend.ConfTemplate)" '改成模板的实际位置，或去掉这一项用入口自带的模板' }
    } elseif ($Cfg.Frontend.Enabled) {
        if (-not $Cfg.Frontend.Node -or -not (Test-Path -LiteralPath $Cfg.Frontend.Node -PathType Leaf)) { Add-Problem '找不到 node' '配置 [frontend] node（缺省是从 PATH 里找 node）' '没有找到 node.exe' '装 Node.js 20 以上，或在 [frontend] node 里写明 node.exe 的完整路径' }
        if (-not (Test-Path -LiteralPath (Join-Path $Cfg.Frontend.Root 'vite.config.ts'))) { Add-Problem '前端目录不对' '配置 [frontend] root 下面有没有 vite.config.ts' "没有：$($Cfg.Frontend.Root)" '改成前端仓库的实际位置' }
        elseif (-not (Test-Path -LiteralPath (Join-Path $Cfg.Frontend.Root 'node_modules\vite\bin\vite.js'))) { Add-Problem '前端的依赖没装' '前端目录下有没有 node_modules\vite' "没有：$(Join-Path $Cfg.Frontend.Root 'node_modules\vite')" '在前端目录里装好依赖再来（入口不替人装依赖）' }
    }
    # 拼进命令行的路径
    $cmdTexts = @($Cfg.Database.Exe, $Cfg.Database.DataDir, $Cfg.Backend.Exe, $Cfg.Root)
    if ($Cfg.RsServer.Enabled) { $cmdTexts += @($Cfg.RsServer.Exe) }
    if ($Cfg.Frontend.Enabled -and $Cfg.Frontend.Kind -eq 'nginx') { $cmdTexts += @($Cfg.Frontend.Exe) }
    elseif ($Cfg.Frontend.Enabled) { $cmdTexts += @($Cfg.Frontend.Node, $Cfg.Frontend.Root) }
    foreach ($t in $cmdTexts) { if ($t -and -not (Test-CmdSafe $t)) { Add-Problem '路径里有入口处理不了的字符' '路径里有没有双引号或百分号' "有：$t" '换一个不带这两种字符的目录' } }

    # 端口（谁在听哪个端口：这里取一次，下面几个服务共用）
    $seen = @{}
    [void](Get-ListenTable -Fresh)
    foreach ($name in $names) {
        $port = Get-ServicePort $Cfg $name
        $title = Get-ServiceTitle $name
        if ($port -lt 1 -or $port -gt 65535) { Add-Problem "$title 的端口不合法" "配置 [$name] port" "$port 不在 1–65535 之间" "改配置里 [$name] 的 port"; continue }
        if ($seen.ContainsKey($port)) { Add-Problem "$title 和 $($seen[$port]) 用了同一个端口" '各服务的端口有没有重复' "都是 $port" '给它们各配一个端口' } else { $seen[$port] = $title }
        if ($Cfg.Guard.ProtectPorts -contains $port) { Add-Problem "$title 的端口在受保护名单里" "配置 [$name] port 是不是在 [guard] protect_ports 里" "$port 在名单里，本套不许用" "给 [$name] 换一个端口"; continue }
        $owners = Get-PortOwners $port -MaxAgeMs 6000
        if ($owners.Count -eq 0) { continue }
        $ours = $false
        $unreadable = ''
        if ($State.ContainsKey($name)) {
            $check = Test-RecordedProcess $Cfg $name $State[$name]
            if ($check.Ours -and (Test-PortOwnedBy $owners $State[$name].pid $State[$name].startTicks)) { $ours = $true }
            elseif ($check.Unreadable) { $unreadable = $check.Why }
        }
        if ($ours) { Add-Note "$title 已经在跑（进程 $($State[$name].pid)，端口 $port）"; continue }
        # 读不到程序路径的两种情形，都不能说成「不是本套起的」：
        #   · 本套记着这个进程，进程号和启动时间都对得上，只是这个窗口读不到它的路径（起它的窗口权限更高）；
        #   · 没有记录，占着端口的那个进程也读不到路径——判断不了它是谁的。
        if ($unreadable) {
            Add-Problem "$title 在跑，但这个窗口核对不了它" "本套记下的进程（$($State[$name].pid)）的程序路径读不读得到" "$unreadable" '用起它时同样的方式打开窗口（都用「以管理员身份运行」，或都不用）再运行 stack up / stack status / stack down。记录没有丢，换回原来那种窗口照常能停'
            continue
        }
        $ownerProc = Get-ProcInfo ([int]$owners[0])
        if ($null -ne $ownerProc -and -not $ownerProc.ExecutablePath) {
            Add-Problem "$title 要用的端口 $port 被占着，而且看不出是谁的" "本机有没有别的进程在听 $port；它的程序路径读不读得到" "进程 $($owners[0])（$($ownerProc.Name)，起于 $(Format-ProcClock $ownerProc)）在听；这个窗口读不到它的程序路径（它多半是权限更高的窗口起的），判断不了它是不是本套的。我没有动它" "如果它是本套的服务（比如当时用「以管理员身份运行」起的），用同样的方式打开窗口再运行；不是的话，停掉那个进程，或把配置里 [$name] 的 port 换一个，然后再运行 stack up"
            continue
        }
        Add-Problem "$title 要用的端口 $port 被占着" "本机有没有别的进程在听 $port" "$(Format-ProcBrief $owners[0])在听。它不是本套起的，我没有动它" "停掉那个进程，或把配置里 [$name] 的 port 换一个，然后再运行 stack up"
    }

    # 程序版本。问版本要执行程序；后端一启动就会在当前目录下建 logs\，所以工作目录放到本套的 run\ 里。
    $probeDir = ''
    if (Test-Path -LiteralPath $Cfg.Root -PathType Container) { New-Dir $Cfg.RunDir; $probeDir = $Cfg.RunDir }
    # 限时 60 秒：干净机器上第一次执行这两个程序，杀毒软件要先把它们扫一遍（后端两百多 MB），平时一两秒的事会变成几十秒。
    # 超时和「跑不起来」分开说，免得把人带到「程序不对」那条路上去。
    $slowNext = '第一次运行这个程序时，杀毒软件要先把它扫一遍，会慢：再运行一次 stack up（再双击一次「开始」）一般就过了。每次都这样，把安装文件夹加进杀毒软件的排除项'
    if ($probeDir -and (Test-Path -LiteralPath $Cfg.Database.Exe -PathType Leaf)) {
        $v = Invoke-Capture $Cfg $Cfg.Database.Exe 'version' $probeDir 60
        if ($v.TimedOut) { Add-Problem '数据库程序问版本没有答完' "$($Cfg.Database.Exe) version 能不能在 60 秒内结束" '等了 60 秒没有结束（这一次已经停掉）' $slowNext }
        elseif ($v.Text -notmatch '(?<!\d)2\.1\.\d+') { Add-Problem '数据库程序的版本不对' "$($Cfg.Database.Exe) version 的输出里有没有 2.1.x" "输出是「$(Limit-Text $v.Text 100)」" '换成本系统自带的那一份（自家分支的 2.1.4）。装成 3.x 会要求做不可逆的数据迁移，不要照做' }
        else { Add-Note "数据库程序：$(Limit-Text $v.Text 100)" }
    }
    if ($probeDir -and (Test-Path -LiteralPath $Cfg.Backend.Exe -PathType Leaf)) {
        $v = Invoke-Capture $Cfg $Cfg.Backend.Exe 'version' $probeDir 60
        if ($v.TimedOut) { Add-Problem '后端程序问版本没有答完' "$($Cfg.Backend.Exe) version 能不能在 60 秒内结束" '等了 60 秒没有结束（这一次已经停掉）' $slowNext }
        elseif (-not $v.Ok) { Add-Problem '后端程序跑不起来' "$($Cfg.Backend.Exe) version 能不能正常结束" "不能：$(Limit-Text $v.Text 160)" '确认这是 64 位 Windows 上能运行的那一份后端程序' }
        else { Add-Note "后端程序：$(Limit-Text $v.Text 140)" }
    }

    # 内存、磁盘、让路文件
    $free = Get-CommitFreeGb
    # 开工线只在「还有服务要起」的时候管；三个都已经在跑，再跑一遍 up 不该被它挡下。
    $needStart = $false
    foreach ($name in $names) { if (-not ($State.ContainsKey($name) -and (Test-RecordedProcess $Cfg $name $State[$name]).Ours)) { $needStart = $true } }
    if ($Cfg.Guard.MinCommitFreeGb -gt 0 -and $free -lt $Cfg.Guard.MinCommitFreeGb -and $needStart) {
        Add-Problem '内存余量不够开工' "可提交内存余量是不是不少于 [guard] min_commit_free_gb（$($Cfg.Guard.MinCommitFreeGb) GB）" "现在只有 $free GB" '等别的程序放出内存再来；或确认这台机器够用后调低这条线'
    } else { Add-Note "可提交内存余量 $free GB" }
    try {
        $drive = New-Object System.IO.DriveInfo -ArgumentList ([System.IO.Path]::GetPathRoot($Cfg.Root))
        $diskFree = [math]::Round($drive.AvailableFreeSpace / 1GB, 1)
        if ($diskFree -lt $Cfg.Guard.MinDiskFreeGb) { Add-Problem '磁盘余量不够' "运行目录所在的盘是不是还剩 [guard] min_disk_free_gb（$($Cfg.Guard.MinDiskFreeGb) GB）以上" "$($drive.Name) 只剩 $diskFree GB" '清出空间，或把 [stack] root 挪到别的盘' }
        else { Add-Note "$($drive.Name) 盘剩余 $diskFree GB" }
    } catch { }
    if ($Cfg.Guard.WaitLockFile -and (Test-Path -LiteralPath $Cfg.Guard.WaitLockFile)) { Add-Note "让路文件在：$($Cfg.Guard.WaitLockFile)（up 会等它消失再起）" }

    $hasEnvironment = $false
    try { $hasEnvironment = [bool]((New-Object System.Diagnostics.ProcessStartInfo).PSObject.Properties['Environment']) } catch { }
    if (-not $hasEnvironment -and $Cfg.Frontend.Enabled -and $Cfg.Frontend.Kind -eq 'vite-dev') { Add-Problem '这台机器的 .NET 太老' '.NET 是不是 4.6 以上（传给服务的环境变量名要保留大小写）' '不是：变量名会被变成小写，前端开发服务器认不出 VITE_ 开头的变量' '装 .NET Framework 4.6 以上（或 PowerShell 5.1），或者把 [frontend] enabled 设成 false' }

    foreach ($n in $notes) { Say "  · $n" }
    foreach ($p in $problems) {
        Say "  ✗ $($p.What)" 'Red'
        Say "      查的是：$($p.Checked)"
        Say "      结果：$($p.Result)"
        Say "      下一步：$($p.Next)" 'Yellow'
    }
    return @{ Problems = $problems; Notes = $notes }
}

# ─────────────────────────── 起一个服务并等它起好 ───────────────────────────

function Test-Guards($Cfg, $State) {
    if ($Cfg.Guard.AbortCommitFreeGb -gt 0) {
        $free = Get-CommitFreeGb
        if ($free -lt $Cfg.Guard.AbortCommitFreeGb) { return "可提交内存余量只剩 $free GB，低于 $($Cfg.Guard.AbortCommitFreeGb) GB（[guard] abort_commit_free_gb）" }
    }
    if ($Cfg.Guard.MaxBackendGb -gt 0 -and $State.ContainsKey('backend')) {
        $mem = Get-ProcMemGb $State['backend'].pid
        if ($mem -gt $Cfg.Guard.MaxBackendGb) { return "后端占用内存 $mem GB，超过 $($Cfg.Guard.MaxBackendGb) GB（[guard] max_backend_gb）" }
    }
    return ''
}

function Stop-OnGuard($Cfg, [string]$Why) {
    Say ''
    Say "✗ 越线了：$Why" 'Red'
    if ($Cfg.Guard.AutoDown) {
        # watch 平时不占运行目录的锁；要动手停进程了，先占上。占不上说明另有 up / down 在跑，由它处理。
        $canStop = $true
        if ($null -eq $script:LockStream) { try { Enter-StackLock $Cfg } catch { $canStop = $false } }
        if ($canStop) {
            Say '  按配置（[guard] auto_down = true）把本套已经起来的服务收掉。' 'Yellow'
            Invoke-Down $Cfg $false | Out-Null
        } else {
            Say '  另一个 stack 命令正在这个运行目录上跑，由它来处理；这里不动任何进程。' 'Yellow'
        }
    } else {
        Say '  本套已经起来的服务还在。要收掉请运行 stack down。' 'Yellow'
    }
    Stop-Stack $Why 3 -Quiet
}

function Show-Failure($Cfg, [string]$Name, [int]$Index, [int]$Total, [string]$Result, [string]$Log, [string]$Next) {
    $title = Get-ServiceTitle $Name
    $health = Test-ServiceHealth $Cfg $Name
    Say ''
    Say "✗ $title 没有起好。" 'Red'
    Say "  停在：$title（第 $Index 个服务，共 $Total 个）"
    Say "  查的是：$($health.Checked)"
    Say "  结果：$Result"
    $tail = Get-LogTail $Log 20
    $shownLog = $Log
    if (@($tail).Count -eq 0) {
        # 有的服务（nginx）把出错的原因写在它自己的日志里，不往标准输出写。
        $extra = ''
        try { $extra = (Get-ServiceSpec $Cfg $Name).ExtraLog } catch { $extra = '' }
        if ($extra) {
            $extraTail = Get-LogTail $extra 20
            if (@($extraTail).Count -gt 0) { $tail = $extraTail; $shownLog = $extra }
        }
    }
    if (@($tail).Count -gt 0) {
        Say "  它输出的最后 $(@($tail).Count) 行（$shownLog）："
        foreach ($line in $tail) { Say "    | $line" 'DarkGray' }
    } elseif ($Log) {
        Say "  它还没有任何输出（$Log）"
    }
    Say "  下一步：$Next" 'Yellow'
}

function Start-StackService($Cfg, $State, [string]$Name, [int]$Index, [int]$Total) {
    $title = Get-ServiceTitle $Name
    $spec = Get-ServiceSpec $Cfg $Name
    $resultFile = Join-Path $Cfg.RunDir "$Name.spawn.json"
    $errorFile = Join-Path $Cfg.RunDir "$Name.spawn.err.txt"
    foreach ($f in $resultFile, $errorFile) { if (Test-Path -LiteralPath $f) { Remove-Item -LiteralPath $f -Force } }

    $hostExe = (Get-Process -Id $PID).Path
    $argLine = '-NoProfile -ExecutionPolicy Bypass -File "' + $PSCommandPath + '" __spawn -Config "' + $Cfg.ConfigPath + '" -Service ' + $Name
    Start-Process -FilePath $hostExe -ArgumentList $argLine -WindowStyle Hidden | Out-Null

    $deadline = (Get-Date).AddSeconds(40)
    while (-not (Test-Path -LiteralPath $resultFile) -and -not (Test-Path -LiteralPath $errorFile) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 200 }
    if (Test-Path -LiteralPath $errorFile) {
        $message = [System.IO.File]::ReadAllText($errorFile, [System.Text.Encoding]::UTF8)
        Stop-Stack "$title 没能起来：$message" 4
    }
    if (-not (Test-Path -LiteralPath $resultFile)) { Stop-Stack "$title 没能起来：40 秒内没有等到启动结果（$resultFile）" 4 }
    Start-Sleep -Milliseconds 200
    $spawn = ConvertFrom-JsonSafe ([System.IO.File]::ReadAllText($resultFile, [System.Text.Encoding]::UTF8))
    $wrapperPid = [int](Get-Prop $spawn 'wrapperPid')
    $wrapperTicks = [string](Get-Prop $spawn 'wrapperTicks')
    $log = [string](Get-Prop $spawn 'log')

    # 在 cmd 那一层下面找到真正的服务进程（只认 cmd 起来之后才创建的子进程）
    $found = $null
    $deadline = (Get-Date).AddSeconds(15)
    while ($null -eq $found -and (Get-Date) -lt $deadline) {
        foreach ($kid in (Get-ChildProcs $wrapperPid $wrapperTicks)) {
            if ($kid.Name -ieq 'conhost.exe') { continue }
            if ($spec.Marker) {
                if ("$($kid.CommandLine)".IndexOf($spec.Marker, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) { $found = $kid }
            } elseif ($kid.ExecutablePath -and ($kid.ExecutablePath -ieq $spec.Exe)) { $found = $kid }
        }
        if ($null -eq $found) {
            if ($null -eq (Get-ProcInfo $wrapperPid)) { break }
            Start-Sleep -Milliseconds 200
        }
    }
    if ($null -eq $found) {
        Show-Failure $Cfg $Name $Index $Total '进程一起来就退出了' $log '看上面它自己报的原因；改好之后再运行 stack up'
        Stop-Stack "$title 一起来就退出了" 4
    }
    $State[$Name] = @{
        pid = [int]$found.ProcessId
        startTicks = (Get-ProcTicks $found)
        startedAt = (Format-ProcClock $found)
        exe = [string]$found.ExecutablePath
        marker = $spec.Marker
        port = $spec.Port
        wrapperPid = $wrapperPid
        wrapperTicks = $wrapperTicks
        log = $log
        adopted = ''
    }
    Save-StackState $Cfg $State
    Say "  已起：进程 $($found.ProcessId)，输出写到 $log"
}

function Wait-ServiceReady($Cfg, $State, [string]$Name, [int]$Index, [int]$Total) {
    $title = Get-ServiceTitle $Name
    $timeoutSec = Get-ServiceTimeout $Cfg $Name
    $record = $State[$Name]
    $started = Get-Date
    $deadline = $started.AddSeconds($timeoutSec)
    $unreadable = 0
    while ($true) {
        $check = Test-RecordedProcess $Cfg $Name $record
        # 进程正在退出的那一瞬间：进程号还在、程序路径已经读不到。等它退干净再下结论，免得报出一句让人看不懂的话。
        if ($check.Unreadable -and $unreadable -lt 8) { $unreadable += 1; Start-Sleep -Milliseconds 400; continue }
        if (-not $check.Ours) {
            Show-Failure $Cfg $Name $Index $Total "进程已经退出（$($check.Why)）" $record.log '看上面它自己报的原因；改好之后再运行 stack up'
            Stop-Stack "$title 起来之后又退出了" 4
        }
        $unreadable = 0
        # 护栏每一圈都看，而且先于「起好了没有」：服务一下子就起好的时候也不能漏掉这一眼。
        $guard = Test-Guards $Cfg $State
        if ($guard) { Stop-OnGuard $Cfg $guard }
        $health = Test-ServiceHealth $Cfg $Name
        if ($health.Ok) {
            $seconds = [int]((Get-Date) - $started).TotalSeconds
            $extra = ''
            if ($health.Detail) { $extra = "；$($health.Detail)" }
            Say "  ✓ $title 起好了（$seconds 秒）：$($health.Version)$extra" 'Green'
            return
        }
        if ((Get-Date) -ge $deadline) {
            Show-Failure $Cfg $Name $Index $Total "等了 $timeoutSec 秒还没通过：$($health.Reason)" $record.log "进程还在（$($record.pid)）。可以过一会儿运行 stack status 再看；确认起不来就运行 stack down，按上面的输出排查后再 stack up"
            Stop-Stack "$title 在 $timeoutSec 秒内没有起好" 4
        }
        Start-Sleep -Milliseconds 700
    }
}

# ─────────────────────────── 初始化配置里点名的库 ───────────────────────────

function Get-DbnumRows($Cfg) {
    $r = Invoke-StackHttp 'GET' ((Get-BackendUrl $Cfg) + '/api/v1/dbnums') '' 60
    if (-not $r.Ok) { return $null }
    return @(Get-Prop (ConvertFrom-JsonSafe $r.Body) 'dbnums')
}

function Get-DbnumRow($Rows, [int]$Dbnum) {
    foreach ($row in @($Rows)) { if ([int](Get-Prop $row 'dbnum') -eq $Dbnum) { return $row } }
    return $null
}

# 后端此刻闲不闲：队列里没有在排或在跑的行，也没有在跑的任务。
function Test-BackendIdle($Cfg) {
    $queue = Invoke-StackHttp 'GET' ((Get-BackendUrl $Cfg) + '/api/v1/queue') '' 15
    if (-not $queue.Ok) { return $false }
    foreach ($row in @(Get-Prop (ConvertFrom-JsonSafe $queue.Body) 'rows')) {
        if ($null -ne $row -and [string](Get-Prop $row 'state') -ne 'held') { return $false }
    }
    $tasks = Invoke-StackHttp 'GET' ((Get-BackendUrl $Cfg) + '/api/v1/tasks?state=running&limit=5') '' 15
    if (-not $tasks.Ok) { return $false }
    foreach ($task in @(Get-Prop (ConvertFrom-JsonSafe $tasks.Body) 'tasks')) { if ($null -ne $task) { return $false } }
    return $true
}

# 点名的库里，有没有「这次请求之后」以失败收场的数据批次。每个返回 @{ Dbnum; Message; TaskId }。
function Get-FailedInitTasks($Cfg, [int[]]$Dbnums, [datetime]$Since) {
    $r = Invoke-StackHttp 'GET' ((Get-BackendUrl $Cfg) + '/api/v1/tasks?state=failed&limit=100') '' 15
    if (-not $r.Ok) { return @() }
    $found = @()
    foreach ($task in @(Get-Prop (ConvertFrom-JsonSafe $r.Body) 'tasks')) {
        if ($null -eq $task) { continue }
        $db = Get-Prop $task 'dbnum'
        if ($null -eq $db -or $Dbnums -notcontains [int]$db) { continue }
        # 时间这一格：PowerShell 7 读 JSON 时已经变成时间对象，5.1 还是字符串（小数位可能多于 7 位，先截掉）。
        $raw = Get-Prop $task 'finished_at'
        $when = [datetime]::MinValue
        if ($raw -is [datetime]) { $when = $raw.ToLocalTime() }
        elseif (-not [datetime]::TryParse((([string]$raw) -replace '(\.\d{1,7})\d*', '$1'), [ref]$when)) { continue }
        if ($when -lt $Since) { continue }
        $message = [string](Get-Prop (Get-Prop (Get-Prop $task 'result') 'batch') 'message')
        if (-not $message) { $message = '后端没有给出原因' }
        $found += @{ Dbnum = [int]$db; Message = $message; TaskId = [string](Get-Prop $task 'task_id') }
    }
    return $found
}

# 初始化没成的时候怎么说：后端给的原因原样带出来；认得出来的那一种（库里留着以前按需生成的模型行）再多说几句。
# 这里不写死哪一版后端有什么毛病，也不劝人删数据库目录：话要对任何一版后端都成立。
function Show-InitFailure($Cfg, $Failures) {
    Say ''
    $residue = @()
    foreach ($f in @($Failures)) {
        $task = ''
        if ($f.TaskId) { $task = "（后端任务 $($f.TaskId)）" }
        Say "  ✗ 库 $($f.Dbnum) 没有初始化成$task。后端给的原因：$(Limit-Text $f.Message 400)" 'Red'
        if ($f.Message -match '模型面残留') { $residue += $f.Dbnum }
    }
    if ($residue.Count -gt 0) {
        Say '  后端说的「模型面残留」是这么回事：这个库还没初始化的时候，在它的节点上按需生成过模型，生成的结果留在了数据库里；' 'Yellow'
        Say '  初始化之前要先把它们清掉，这一次没清成。' 'Yellow'
        Say '    · 2026-11-11 晚上之后构建的后端（带 gen-model #146 的修复，test3 起的安装包里就是）会在初始化前自己清；' 'Yellow'
        Say '      在这样的后端上还看到这句话，说明这次清理本身出了问题。' 'Yellow'
        Say '    · 更早的后端没有这个修复，也清不掉。先后要反过来：先初始化，再到节点上生成。' 'Yellow'
        Say "  下一步（按这个顺序试，都不丢别的库的数据）：" 'Yellow'
        Say "    1. 再运行一次 stack init $($residue -join ' ')。" 'Yellow'
        Say "    2. 还不行：在 plant3d-web 里对这个库做一次整库的「清理模型」，再 stack init。" 'Yellow'
        Say "    3. 还不行：换成带这个修复的安装包（原地覆盖，数据不用动，见说明「换新一版」）；或者把 logs\ 和 work\backend\logs\ 带给开发看。" 'Yellow'
    } else {
        Say "  下一步：看后端的日志（stack status 里有位置），或打开运维页 $(Get-BackendUrl $Cfg)/ops.html 看这个任务的详情。改好之后再运行 stack init。" 'Yellow'
    }
    Say '  同一次点名的其余库（如果有）后端还在接着做，不受影响；stack status 看得到后端还在不在忙。' 'Yellow'
}

# 初始化点名的库：解析这些库的全部元素进数据库，并生成它们的模型（后端里这是一件事，分不开）。
# 系统库和字典库按后端的设计会一并处理。已经初始化过的库不重复做。
function Invoke-Init($Cfg, $State, [int[]]$Dbnums) {
    if (@($Dbnums).Count -eq 0) { return }
    if ($Cfg.Guard.ProtectPorts -contains $Cfg.Backend.Port) { Stop-Stack '后端端口在受保护名单里，不向它发初始化请求' 2 }
    $health = Test-ServiceHealth $Cfg 'backend'
    if ($null -ne $health.Json -and [string](Get-Prop $health.Json 'mode') -eq 'setup') {
        Say '后端在设置模式（配置还不完整），先不初始化库。在向导页里填完之后再运行 stack up。' 'Yellow'
        return
    }
    $list = @($Dbnums) -join '、'
    $total = (Get-ServiceNames $Cfg).Count
    Say "初始化库 $list（解析数据并生成这些库的模型）"
    $rows = Get-DbnumRows $Cfg
    if ($null -eq $rows) { Stop-Stack '后端的库列表（GET /api/v1/dbnums）读不出来，没法初始化。运行 stack status 看后端是否正常' 4 }
    $need = @()
    foreach ($db in $Dbnums) {
        $row = Get-DbnumRow $rows $db
        if ($null -eq $row) {
            $known = @($rows | ForEach-Object { [int](Get-Prop $_ 'dbnum') } | Sort-Object)
            $shown = ($known | Select-Object -First 40) -join '、'
            if ($known.Count -gt 40) { $shown += ' …' }
            Stop-Stack "库 $db 不在后端的库列表里：这个 MDB 里没有它，或者它的库文件不在。后端列表里有 $($known.Count) 个库：$shown。请核对库号和 [project] mdb" 4
        }
        if ((Get-Prop $row 'initialized') -ne $true) { $need += $db }
    }
    # 留 5 秒的余地：后端记的完成时间和这里的钟可能差一点。
    $requestedAt = (Get-Date).AddSeconds(-5)
    if ($need.Count -eq 0) {
        Say '  都已经初始化过了，不重复做。'
    } else {
        $body = '{"project":"' + $Cfg.Project.Name + '","dbnums":[' + ($need -join ',') + ']}'
        $r = Invoke-StackHttp 'POST' ((Get-BackendUrl $Cfg) + '/api/v1/update/execute') $body 120
        if (-not $r.Ok) { Stop-Stack "后端没有接受初始化请求（$($r.Error)）：$(Limit-Text $r.Body 300)" 4 }
        $j = ConvertFrom-JsonSafe $r.Body
        $enqueued = @(Get-Prop $j 'enqueued' | Where-Object { $null -ne $_ }).Count
        Say "  已请求初始化库 $($need -join '、')：入队 $enqueued 个批次（系统库和字典库按后端的设计一并处理）"
        $blocked = @()
        foreach ($b in @(Get-Prop $j 'blocked')) {
            if ($null -eq $b) { continue }
            Say "  ✗ 库 $(Get-Prop $b 'dbnum') 被后端挡下：$(Limit-Text ([string](Get-Prop $b 'reason')) 200)" 'Red'
            if ($need -contains [int](Get-Prop $b 'dbnum')) { $blocked += [int](Get-Prop $b 'dbnum') }
        }
        foreach ($w in @(Get-Prop $j 'warnings')) { if ($w) { Say "  后端提示：$(Limit-Text ([string]$w) 200)" 'Yellow' } }
        if ($blocked.Count -gt 0) { Stop-Stack "库 $($blocked -join '、') 被后端挡下，没有初始化（原因见上）" 4 }
    }
    $started = Get-Date
    $deadline = $started.AddSeconds($Cfg.Timeouts.InitSec)
    $lastNote = Get-Date
    while ($true) {
        $rows = Get-DbnumRows $Cfg
        $done = ($null -ne $rows)
        if ($done) {
            foreach ($db in $Dbnums) {
                $row = Get-DbnumRow $rows $db
                if ($null -eq $row -or (Get-Prop $row 'initialized') -ne $true) { $done = $false }
            }
        }
        if ($done -and (Test-BackendIdle $Cfg)) {
            Say "  ✓ 初始化完成（$([int]((Get-Date) - $started).TotalSeconds) 秒）" 'Green'
            foreach ($db in $Dbnums) {
                $row = Get-DbnumRow $rows $db
                Say "    库 $db：数据到会话 $(Get-Prop $row 'applied_sesno')（库文件最新 $(Get-Prop $row 'file_latest_sesno')）；模型 $(Get-Prop $row 'model_verdict')；现在从 $(Get-Prop $row 'model_source') 取模型"
            }
            return
        }
        # 点名的库里有一个以失败收场：不用等到超时，马上把后端给的原因说出来。
        if ($need.Count -gt 0) {
            $failures = @(Get-FailedInitTasks $Cfg $need $requestedAt)
            if ($failures.Count -gt 0) {
                Show-InitFailure $Cfg $failures
                Stop-Stack "库 $(@($failures | ForEach-Object { $_.Dbnum }) -join '、') 没有初始化成" 4 -Quiet
            }
        }
        $check = Test-RecordedProcess $Cfg 'backend' $State['backend']
        if (-not $check.Ours) {
            Show-Failure $Cfg 'backend' 2 $total "初始化过程中后端退出了（$($check.Why)）" $State['backend'].log '看上面的输出；再运行 stack up 会把后端补起来并接着初始化'
            Stop-Stack '初始化过程中后端退出了' 4
        }
        $guard = Test-Guards $Cfg $State
        if ($guard) { Stop-OnGuard $Cfg $guard }
        if ((Get-Date) -ge $deadline) {
            Show-Failure $Cfg 'backend' 2 $total "等了 $($Cfg.Timeouts.InitSec) 秒，库 $list 还没初始化完" $State['backend'].log "初始化还在后端里跑着，没有被打断。过一会儿运行 stack smoke 再看，或打开运维页 $(Get-BackendUrl $Cfg)/ops.html 看进度；库大的话把 [timeouts] init_sec 调大"
            Stop-Stack "初始化在 $($Cfg.Timeouts.InitSec) 秒内没有完成" 4
        }
        if (((Get-Date) - $lastNote).TotalSeconds -ge 20) {
            Say "  … 还在初始化（$([int]((Get-Date) - $started).TotalSeconds) 秒；后端内存 $(Get-ProcMemGb $State['backend'].pid) GB，可提交余量 $(Get-CommitFreeGb) GB）"
            $lastNote = Get-Date
        }
        Start-Sleep -Seconds 2
    }
}

# ─────────────────────────── 命令：up ───────────────────────────

function Invoke-Up($Cfg) {
    Say "stack up —— $($Cfg.Name)（配置 $($Cfg.ConfigPath)）"
    $state = Read-StackState $Cfg

    if ($Cfg.Guard.WaitLockFile -and (Test-Path -LiteralPath $Cfg.Guard.WaitLockFile)) {
        Say "让路文件在：$($Cfg.Guard.WaitLockFile)。等它消失再起（最多等 $($Cfg.Guard.WaitLockTimeoutSec) 秒）…" 'Yellow'
        $waitStart = Get-Date
        while (Test-Path -LiteralPath $Cfg.Guard.WaitLockFile) {
            if (((Get-Date) - $waitStart).TotalSeconds -ge $Cfg.Guard.WaitLockTimeoutSec) { Stop-Stack "等了 $($Cfg.Guard.WaitLockTimeoutSec) 秒，让路文件还在：$($Cfg.Guard.WaitLockFile)。什么都没起，过一会儿再运行 stack up" 3 }
            Start-Sleep -Seconds 3
        }
        Say "  让路文件消失了，等了 $([int]((Get-Date) - $waitStart).TotalSeconds) 秒"
    }

    # 在跑、却没记在状态文件里的本套进程（后端在向导页里重启过，或者上一次 up 没跑完）：先认领回来，再预检。
    [void](Invoke-Adopt $Cfg $state $true)

    Say '预检'
    $pre = Invoke-Preflight $Cfg $state
    if ($pre.Problems.Count -gt 0) {
        Say ''
        Say "✗ 预检有 $($pre.Problems.Count) 处没过，什么都没起。按上面的「下一步」改好之后再运行 stack up。" 'Red'
        Stop-Stack '预检没过' 4 -Quiet
    }

    $names = Get-ServiceNames $Cfg
    $total = $names.Count
    $index = 0
    $pending = @()
    foreach ($name in $names) {
        $index += 1
        $title = Get-ServiceTitle $name
        $port = Get-ServicePort $Cfg $name
        Say "[$index/$total] $title（端口 $port）"
        $info = Get-ServiceState $Cfg $state $name
        # 进程正在退出的那一瞬间也读不到程序路径：等一等再下结论。
        $tries = 0
        while ($info.Unreadable -and $tries -lt 8) { Start-Sleep -Milliseconds 400; $tries += 1; $info = Get-ServiceState $Cfg $state $name }
        if ($info.State -eq 'ok') {
            Say "  已经在跑（进程 $($info.Pid)），不动它" 'Green'
            continue
        }
        if ($info.Ours) {
            Say "  进程 $($info.Pid) 在，但检查还没通过（$($info.Reason)）。等它…"
            Wait-ServiceReady $Cfg $state $name $index $total
            continue
        }
        if ($info.Unreadable) {
            # 记录在、进程号和启动时间都对得上，只是这个窗口读不到它的程序路径：不丢记录、不重起，把话说对。
            Say ''
            Say "✗ $title 在跑，但这个窗口核对不了它。" 'Red'
            Say "  查的是：$($info.Checked)"
            Say "  结果：$($info.Reason)"
            Say "  下一步：$($info.Next)" 'Yellow'
            Stop-Stack "$title 的进程读不到程序路径" 4 -Quiet
        }
        # 没有记录，或者记录里的进程已经不在了：先看有没有可以认领的（程序在运行目录里、又占着本套的端口）。
        if (@(Invoke-Adopt $Cfg $state $true @($name)).Count -gt 0) {
            Wait-ServiceReady $Cfg $state $name $index $total
            continue
        }
        if ($state.ContainsKey($name)) {
            Say "  上次起的那个不在了（$($info.Reason)）。重新起。" 'Yellow'
            $state.Remove($name)
            Save-StackState $Cfg $state
        }
        $owners = Get-PortOwners $port -Fresh
        if ($owners.Count -gt 0) {
            $ownerProc = Get-ProcInfo ([int]$owners[0])
            if ($null -ne $ownerProc -and -not $ownerProc.ExecutablePath) {
                Show-Failure $Cfg $name $index $total "端口 $port 被进程 $($owners[0])（$($ownerProc.Name)）占着；这个窗口读不到它的程序路径，判断不了它是不是本套的。我没有动它" '' "如果它是本套的服务（比如当时用「以管理员身份运行」起的），用同样的方式打开窗口再运行；不是的话，停掉那个进程，或把配置里 [$name] 的 port 换一个，然后再运行 stack up"
            } else {
                Show-Failure $Cfg $name $index $total "端口 $port 被占着：$(Format-ProcBrief $owners[0])。它不是本套起的，我没有动它" '' "停掉那个进程，或把配置里 [$name] 的 port 换一个，然后再运行 stack up"
            }
            Stop-Stack "$title 的端口被占" 4
        }
        # 端口空着，但运行目录里这个服务的程序已经有进程在跑：多半是刚被重启、还没占上端口。等它一会儿，不起第二个。
        $strays = @(Get-ServiceProgramProcs $Cfg $name)
        if ($strays.Count -gt 0) {
            $strayText = (@($strays | ForEach-Object { $_.ProcessId }) -join '、')
            Say "  运行目录里的 $title 程序已经有进程在跑（进程 $strayText），只是还没占上端口 $port。先不起第二个，最多等它 30 秒…" 'Yellow'
            $outcome = Wait-StrayService $Cfg $state $name $true 30
            if ($outcome -eq 'adopted') {
                Wait-ServiceReady $Cfg $state $name $index $total
                continue
            }
            if ($outcome -eq 'stuck') {
                $strays = @(Get-ServiceProgramProcs $Cfg $name)
                $spec = Get-ServiceSpec $Cfg $name
                Say ''
                Say "✗ $title 没有起：运行目录里的程序已经有进程在跑，但它没占本套的端口。" 'Red'
                Say "  查的是：有没有进程的程序路径就是 $($spec.Exe)；端口 $port 上有没有进程在听"
                Say "  结果：$(@($strays | ForEach-Object { Format-ProcBrief ([int]$_.ProcessId) }) -join '；')在跑；端口 $port 上没有进程在听。我没有起第二个，也没有动它"
                Say "  下一步：如果刚在配置向导页里点过「重启」，过半分钟再运行 stack up；如果在向导页里改过监听端口，把 stack.toml 里 [$name] 的 port 改成一样的。它一直不占这个端口的话，到任务管理器里结束这个进程（先核对它的路径是上面那一个），再运行 stack up" 'Yellow'
                Stop-Stack "$title 的程序已经有进程在跑，但没占本套的端口" 4 -Quiet
            }
            Say '  那个进程自己退了。照常起。'
        }
        if ($name -eq 'database') {
            Initialize-BackendFiles $Cfg
            New-Dir $Cfg.Database.DataDir
        }
        if ($name -eq 'backend') {
            Initialize-BackendFiles $Cfg
            # 后端要连的那个端口上，必须是本套自己起的那台数据库。
            $dbState = Get-ServiceState $Cfg $state 'database'
            if ($dbState.State -ne 'ok') { Stop-Stack "数据库不是正常状态（$($dbState.Reason)），不起后端" 4 }
        }
        if ($name -eq 'rs_server') {
            # 后端健康、并且库版本一致之后才起它：它一启动就要读库、往库里灌函数；库版本没对上时
            # 它读库的接口一律不可用，缺省还会自己去触发后端的升级（清库重建）——那个已经在它的环境变量里关掉了。
            $backendState = Get-ServiceState $Cfg $state 'backend'
            if ($backendState.State -ne 'ok') { Stop-Stack "后端不是正常状态（$($backendState.Reason)），不起 rs-server" 4 }
            if ($Cfg.RsServer.WaitStorageVersion) {
                $sv = Wait-StorageVersion $Cfg $state
                if ($sv.Kind -ne 'ok') {
                    $waited = ''
                    if ($sv.WaitedSec -ge 1) { $waited = "等了 $($sv.WaitedSec) 秒，" }
                    Say "  先不起 rs-server：${waited}后端的库版本还没对上（$($sv.Text)）。plant3d-web、后端和 plant-ui 网页版不受影响，照常能用。" 'Yellow'
                    Show-StorageVersionAdvice $Cfg $sv
                    if (-not $NoInit -and $Cfg.Init.Dbnums.Count -gt 0 -and $sv.Kind -eq 'awaiting-model') {
                        Say "  配置里 [init] dbnums 点了名（$($Cfg.Init.Dbnums -join '、')）：下面会先初始化；初始化完再看一次，库版本对上了就把 rs-server 补起来。" 'Yellow'
                    }
                    $pending += $title
                    continue
                }
                Say "  后端的库版本一致（$($sv.Text)），可以起"
            }
            Initialize-RsServerFiles $Cfg
        }
        if ($name -eq 'frontend') { Initialize-FrontendFiles $Cfg }
        Start-StackService $Cfg $state $name $index $total
        Wait-ServiceReady $Cfg $state $name $index $total
    }

    # plant-ui 网页版要知道 rs-server 在哪（见 Update-PlantUiWebConfig 上面的说明）。
    if ((Get-ServiceState $Cfg $state 'backend').State -eq 'ok') { Update-PlantUiWebConfig $Cfg }

    # 缺省不初始化任何库。只有配置里 [init] dbnums 点了名才做；也可以起好之后用 stack init <库号…>。
    if ($NoInit) {
        if ($Cfg.Init.Dbnums.Count -gt 0) { Say "按 -NoInit 不初始化（配置里点名的库 $($Cfg.Init.Dbnums -join '、') 这次不做）" }
    } elseif ($Cfg.Init.Dbnums.Count -gt 0) {
        Invoke-Init $Cfg $state $Cfg.Init.Dbnums
        # rs-server 刚才因为库版本没对上而没起：点了名的库初始化完之后，版本记录可能已经写上了
        # （配了 watch_dbnums 的新库就是这样）。再看一眼，对上了就把它补起来，省得再运行一次。
        $rsTitle = Get-ServiceTitle 'rs_server'
        if ($pending -contains $rsTitle -and -not $state.ContainsKey('rs_server')) {
            $sv = Get-StorageVersionState $Cfg
            $rsOwners = Get-PortOwners $Cfg.RsServer.Port -Fresh
            if ($sv.Kind -eq 'ok' -and $rsOwners.Count -eq 0) {
                $rsIndex = [array]::IndexOf($names, 'rs_server') + 1
                Say "[$rsIndex/$total] rs-server（端口 $($Cfg.RsServer.Port)）：初始化之后库版本一致了（$($sv.Text)），现在把它补起来"
                Initialize-RsServerFiles $Cfg
                Start-StackService $Cfg $state 'rs_server' $rsIndex $total
                Wait-ServiceReady $Cfg $state 'rs_server' $rsIndex $total
                $pending = @($pending | Where-Object { $_ -ne $rsTitle })
            }
        }
    } else {
        Say '没有点名要初始化的库：不初始化、不生成模型。要初始化某个库，在界面里做，或运行 stack init <库号…>。'
    }

    Say ''
    $rows = @()
    foreach ($name in $names) { $rows += (Get-ServiceState $Cfg $state $name) }
    Show-StatusTable $rows
    Say ''
    Show-Urls $Cfg
    if ($pending.Count -gt 0) {
        Say "还没起的：$($pending -join '、')（原因和下一步见上面「先不起」那一段）。其余已经起好，可以照常用。" 'Yellow'
        Say '（因为还有服务没起，这次的退出码是 4；双击「开始」的窗口最后一行会用英文说 not everything is running，指的就是这件事。）' 'Yellow'
        Stop-Stack "还有服务没起：$($pending -join '、')" 4 -Quiet
    }
    Say '全部起好。看状态：stack status；检查：stack smoke；停：stack down。' 'Green'
}

function Show-Urls($Cfg) {
    if ($Cfg.Frontend.Enabled) { Say "  plant3d-web      $(Get-FrontendUrl $Cfg)/" }
    Say "  plant-ui 网页版  $(Get-BackendUrl $Cfg)/        （后端自带；运维页 $(Get-BackendUrl $Cfg)/ops.html）"
    if ($Cfg.RsServer.Enabled) { Say "  rs-server        $(Get-RsServerUrl $Cfg)/metadata   （元数据网页；接口说明 $(Get-RsServerUrl $Cfg)/api/docs）" }
    if (-not $Cfg.LoopbackOnly) { Say '  （以上是在这台机器上打开用的地址；从别的机器打开，把 127.0.0.1 换成这台机器的名字或地址）' }
}

# 等后端的库版本一致。返回 Get-StorageVersionState 的那个归类，另加 WaitedSec（等了几秒）。
# 只在「等得到」的时候等：
#   · 全新的库、后端还在做启动：缺省配置下后端做完启动时自己写，样例工程十来秒；
#   · 后端的启动做完了、还在忙（有库在初始化）：做完会写；
#   · 后端正在做库的升级。
# 等不到的情形马上回去，不白等：
#   · 后端在设置模式；版本记录比程序要的旧或新；上一次升级失败；
#   · 没有版本记录而模型面已经就绪（连看到 5 次才算数——版本记录和「就绪」是前后脚写的，别抢在它中间下结论）；
#   · 没有版本记录、后端的启动做完了、却闲着（连着 10 次都没有在排在跑的任务）：它在等库被初始化，光等不会有。
function Wait-StorageVersion($Cfg, $State) {
    $started = Get-Date
    $deadline = $started.AddSeconds($Cfg.Timeouts.StorageVersionSec)
    $said = $false
    $settled = 0
    $lastKind = ''
    while ($true) {
        $sv = Get-StorageVersionState $Cfg
        $sv.WaitedSec = [int]((Get-Date) - $started).TotalSeconds
        if ($sv.Kind -eq 'ok') { return $sv }
        if ($sv.Kind -eq 'setup' -or $sv.Kind -eq 'old' -or $sv.Kind -eq 'newer' -or $sv.Kind -eq 'upgrade-failed') { return $sv }
        if ($sv.Kind -ne $lastKind) { $settled = 0; $lastKind = $sv.Kind }
        if ($sv.Kind -eq 'no-record') {
            $settled += 1
            if ($settled -ge 5) { return $sv }
        } elseif ($sv.Kind -eq 'awaiting-model') {
            if (Test-BackendIdle $Cfg) { $settled += 1 } else { $settled = 0 }
            if ($settled -ge 10) { $sv.Idle = $true; return $sv }
        } else {
            $settled = 0
        }
        if ((Get-Date) -ge $deadline) { return $sv }
        if (-not $said) { Say "  等后端的库版本一致再起 rs-server（现在：$($sv.Text)；最多等 $($Cfg.Timeouts.StorageVersionSec) 秒）…"; $said = $true }
        $guard = Test-Guards $Cfg $State
        if ($guard) { Stop-OnGuard $Cfg $guard }
        Start-Sleep -Seconds 1
    }
}

# ─────────────────────────── 命令：down ───────────────────────────

function Invoke-Down($Cfg, [bool]$OnlyTell) {
    $state = Read-StackState $Cfg
    $stopped = 0
    $failed = 0
    if ($OnlyTell) { Say "stack down -WhatIf —— 只说不做（$($Cfg.Name)）" } else { Say "stack down —— $($Cfg.Name)" }
    # 在跑、却没记在状态文件里的本套进程（后端在向导页里重启过，或者上一次 up 没跑完）：先认领回来，下面照常停。
    [void](Invoke-Adopt $Cfg $state (-not $OnlyTell))
    if (-not $OnlyTell) {
        # 程序在跑、只是还没占上端口的（刚在向导页里点了「重启」的后端就是这样）：等它几秒，占上了就认领、一起停。
        foreach ($name in @($script:AdoptMissing)) {
            if (@(Get-ServiceProgramProcs $Cfg $name).Count -eq 0) { continue }
            Say "  $(Get-ServiceTitle $name)：运行目录里的程序有进程在跑，只是还没占上端口 $(Get-ServicePort $Cfg $name)。等它最多 15 秒…" 'Yellow'
            [void](Wait-StrayService $Cfg $state $name $true 15)
        }
    }
    $order = @($script:AllServiceNames)
    [array]::Reverse($order)
    foreach ($name in $order) {
        $title = Get-ServiceTitle $name
        if (-not $state.ContainsKey($name)) {
            # 配置里没启用、状态文件里也没有的服务不用提。
            if ((Get-ServiceNames $Cfg) -contains $name) { Say "  $title：本套没有起它的记录，不用停" }
            continue
        }
        $record = $state[$name]
        # 要动手停进程了：配了受保护端口的话，先重新取一次「谁在听哪个端口」，不用几秒前的旧账。
        if (-not $OnlyTell -and $Cfg.Guard.ProtectPorts.Count -gt 0) { [void](Get-ListenTable -Fresh) }
        $check = Test-RecordedProcess $Cfg $name $record
        if ($check.Unreadable) {
            # 进程号和启动时间都对得上，只是读不到它的程序路径：多半是这个窗口的权限比起它的那个低。
            # 不停它，也不把记录丢掉——换个同样权限的窗口再来还能停。
            Say "  ✗ $title：$($check.Why)。没有停它，记录留着。" 'Red'
            $failed += 1
            continue
        }
        if (-not $check.Ours) {
            Say "  $title：$($check.Why)。不停任何进程。" 'Yellow'
            if (-not $OnlyTell) { $state.Remove($name); Save-StackState $Cfg $state }
            continue
        }
        $what = "$title：进程 $($record.pid)（$($check.Proc.Name)，起于 $(Format-ProcClock $check.Proc)，端口 $($record.port)"
        if ($record.marker) { $what += "，命令行带本套的标记 $($record.marker)）" } else { $what += "，程序 $($record.exe)）" }
        if ($OnlyTell) { Say "  会停 $what"; continue }
        # 有的服务带子进程（nginx 的工作进程、前端开发服务器的打包进程）；先记下来，
        # 主进程停了之后核对它们是否跟着退了，没退的按「进程号 + 启动时间」核对后收掉。
        # 名单里只收真是它生的：创建时间不早于这个服务的（见 Select-TrueChildren 上面的说明）。
        $kids = @()
        foreach ($kid in (Get-ChildProcs $record.pid $record.startTicks)) { $kids += @{ pid = [int]$kid.ProcessId; ticks = (Get-ProcTicks $kid); name = $kid.Name } }
        if (-not (Stop-VerifiedProcess $record.pid $record.startTicks)) {
            Say "  ✗ $title：进程 $($record.pid) 没有停下来" 'Red'
            $failed += 1
            continue
        }
        # 下面要核对「占没占受保护端口」：先重新取一次谁在听哪个端口，这个服务的子进程和外层 cmd 共用这一份。
        if ($Cfg.Guard.ProtectPorts.Count -gt 0 -and ($kids.Count -gt 0 -or $record.wrapperPid -gt 0)) { [void](Get-ListenTable -Fresh) }
        foreach ($kid in $kids) {
            if ($kid.name -ieq 'conhost.exe') { continue }
            Start-Sleep -Milliseconds 300
            $p = Get-ProcInfo $kid.pid
            if ($null -eq $p -or -not (Test-SameStart $kid.ticks $p)) { continue }
            # 占着受保护端口的进程一律不动，不管它是怎么进到这份名单里的。
            $guarded = Get-ProtectedPortOf $Cfg $kid.pid -MaxAgeMs 6000
            if ($guarded -gt 0) {
                Say "    没有动子进程 $($kid.pid)（$($kid.name)）：它占着受保护的端口 $guarded" 'Yellow'
                continue
            }
            if (Stop-VerifiedProcess $kid.pid $kid.ticks) { Say "    连带停了它的子进程 $($kid.pid)（$($kid.name)）" }
        }
        # 外面包的那层 cmd 会在服务退出后自己结束；没结束才按「进程号 + 启动时间」核对后收掉。
        if ($record.wrapperPid -gt 0) {
            Start-Sleep -Milliseconds 300
            $w = Get-ProcInfo $record.wrapperPid
            if ($null -ne $w -and $w.Name -ieq 'cmd.exe' -and (Get-ProtectedPortOf $Cfg $record.wrapperPid -MaxAgeMs 6000) -eq 0) { Stop-VerifiedProcess $record.wrapperPid $record.wrapperTicks | Out-Null }
        }
        Say "  已停 $what" 'Green'
        $stopped += 1
        $state.Remove($name)
        Save-StackState $Cfg $state
    }
    if ($OnlyTell) {
        if ($Cfg.Guard.ProtectPorts.Count -gt 0) {
            Say '  受保护的端口此刻的占用者（不在上面「会停」的名单里，不会动）：'
            foreach ($port in $Cfg.Guard.ProtectPorts) {
                $owners = Get-PortOwners $port
                if ($owners.Count -gt 0) { Say "    $port → $(Format-ProcBrief $owners[0])" } else { Say "    $port → 没有进程在听" }
            }
        }
        return 0
    }
    $left = @()
    [void](Get-ListenTable -Fresh)
    foreach ($name in (Get-ServiceNames $Cfg)) {
        $port = Get-ServicePort $Cfg $name
        $owners = Get-PortOwners $port -MaxAgeMs 6000
        if ($owners.Count -gt 0) { $left += "$port（$(Format-ProcBrief $owners[0])）" }
    }
    if ($left.Count -gt 0) { Say "  注意：这些端口上还有进程在听，不在本套的记录里，没有动：$($left -join '；')" 'Yellow' }
    # 运行目录里的程序还有没有进程在跑（没占本套的端口，所以认领不了）。
    $strays = @()
    foreach ($name in (Get-ServiceNames $Cfg)) {
        foreach ($p in @(Get-ServiceProgramProcs $Cfg $name)) { $strays += "$(Get-ServiceTitle $name)：$(Format-ProcBrief ([int]$p.ProcessId))" }
    }
    if ($strays.Count -gt 0) {
        Say "  ✗ 运行目录里的程序还有进程在跑；它没占本套的端口，所以没有认领、也没有动它：$($strays -join '；')" 'Red'
        Say '    下一步：如果刚在后端的向导页里点过「重启」，过半分钟再运行一次 stack down；否则到任务管理器里核对它的路径之后手工结束。' 'Yellow'
        $failed += 1
    }
    Say "停了 $stopped 个。"
    if ($failed -gt 0) { return 1 }
    return 0
}

# ─────────────────────────── 命令：status / doctor / smoke / watch / config ───────────────────────────

function Invoke-Status($Cfg) {
    $state = Read-StackState $Cfg
    Say "stack status —— $($Cfg.Name)（$((Get-Date).ToString('HH:mm:ss'))；可提交内存余量 $(Get-CommitFreeGb) GB）"
    # status 不占运行目录的锁，所以认领只在这一次运行里算数，不写状态文件。
    [void](Invoke-Adopt $Cfg $state $false)
    # 程序在跑、只是还没占上端口的（刚在向导页里点了「重启」的后端）：等它几秒，占上了就认领，免得白报一次「不在了」。
    foreach ($name in @($script:AdoptMissing)) {
        if (@(Get-ServiceProgramProcs $Cfg $name).Count -eq 0) { continue }
        [void](Wait-StrayService $Cfg $state $name $false 8)
    }
    $rows = @()
    foreach ($name in (Get-ServiceNames $Cfg)) { $rows += (Get-ServiceState $Cfg $state $name) }
    Show-StatusTable $rows
    $bad = @($rows | Where-Object { $_.State -ne 'ok' })
    # 不正常的逐个说清楚：是哪个、查的是什么、结果、它自己最后输出了什么、下一步。
    foreach ($row in $bad) {
        if ($row.State -ne 'bad') { continue }
        Say ''
        Say "✗ $($row.Title) 不正常。" 'Red'
        Say "  查的是：$($row.Checked)"
        Say "  结果：$($row.Reason)"
        $tail = @()
        if ($row.Log) { $tail = Get-LogTail $row.Log 8 }
        if ($tail.Count -gt 0) {
            Say "  它输出的最后 $($tail.Count) 行（$($row.Log)）："
            foreach ($line in $tail) { Say "    | $line" 'DarkGray' }
        }
        Say "  下一步：$($row.Next)" 'Yellow'
    }
    foreach ($row in $bad) {
        if ($row.State -ne 'waiting') { continue }
        Say ''
        Say "$($row.Title) 还没起：$($row.Reason)" 'Yellow'
        if ($null -ne $row.Sv) { Show-StorageVersionAdvice $Cfg $row.Sv } else { Say "  下一步：$($row.Next)" 'Yellow' }
    }
    if ($bad.Count -gt 0) {
        if (@($rows | Where-Object { $_.State -ne 'stopped' }).Count -eq 0) {
            Say ''
            Say '都没起。下一步：运行 stack up。' 'Yellow'
        } elseif (@($bad | Where-Object { $_.State -eq 'stopped' }).Count -gt 0) {
            Say ''
            Say '「没起」的那些：运行 stack up 会把它们起起来，已经在跑的不动。' 'Yellow'
        }
        return 1
    }
    return 0
}

function Invoke-Doctor($Cfg) {
    Say "stack doctor —— 只检查，不起任何服务（$($Cfg.Name)）"
    Say "  配置：$($Cfg.ConfigPath)"
    Say "  运行目录：$($Cfg.Root)"
    Say "  PowerShell $($PSVersionTable.PSVersion)"
    $state = Read-StackState $Cfg
    [void](Invoke-Adopt $Cfg $state $false)
    $pre = Invoke-Preflight $Cfg $state
    Say ''
    if ($pre.Problems.Count -gt 0) { Say "✗ $($pre.Problems.Count) 处要处理。" 'Red'; return 4 }
    Say '✓ 可以运行 stack up。' 'Green'
    return 0
}

# 带着 Origin 头请求一次，看对方回不回「允许这个来源」。浏览器的跨域检查看的就是这个响应头。
function Test-CorsAllowed([string]$Url, [string]$Origin) {
    $out = @{ Ok = $false; Allow = '' }
    $response = $null
    try {
        $request = [System.Net.WebRequest]::Create($Url)
        $request.Method = 'GET'
        $request.Proxy = $null
        $request.Timeout = 8000
        $request.Headers.Add('Origin', $Origin)
        try { $response = $request.GetResponse() } catch [System.Net.WebException] { $response = $_.Exception.Response }
        if ($null -ne $response) {
            $out.Allow = [string]$response.Headers['Access-Control-Allow-Origin']
            $out.Ok = ($out.Allow -eq $Origin -or $out.Allow -eq '*')
        }
    } catch { } finally { if ($null -ne $response) { $response.Close() } }
    return $out
}

# 从一批模型记录里找第一个带网格的，返回网格的键；没有就返回空。
function Get-FirstGeoHash($Records) {
    foreach ($item in @(Get-Prop $Records 'items')) {
        foreach ($inst in @(Get-Prop $item 'insts')) {
            $h = [string](Get-Prop $inst 'geo_hash')
            if ($h.Length -gt 3) { return $h }
        }
    }
    return ''
}

# 冒烟。三层：
#   第 0 层  各个服务在跑并通过检查；后端自报健康、读得到工程目录、库版本一致；plant-ui 首页；rs-server 的几个接口
#   第 1 层  模型树有根（直接读 E3D 库文件，不用初始化任何库）；配置里点名的库已初始化
#   第 2 层  取到一个模型（记录 + 网格）：缺省只在已经初始化过的库里取，不触发任何生成；
#            给了 -Node <节点号> 就在那个节点上按需生成一次再取（界面在节点上点「生成」做的就是这件事）
# 结果三种：✓ 过；✗ 没过；– 跳过（条件不具备，不算没过）。
function Invoke-Smoke($Cfg, [int]$MaxLevel, [string]$NodeRef, [bool]$ForceNode = $false) {
    $state = Read-StackState $Cfg
    $results = New-Object System.Collections.ArrayList
    function Add-Result([int]$Lv, [string]$What, [bool]$Ok, [string]$Detail) { [void]$results.Add(@{ Level = $Lv; What = $What; Ok = $Ok; Skip = $false; Detail = $Detail }) }
    function Add-Skip([int]$Lv, [string]$What, [string]$Detail) { [void]$results.Add(@{ Level = $Lv; What = $What; Ok = $true; Skip = $true; Detail = $Detail }) }
    Say "stack smoke —— $($Cfg.Name)（做到第 $MaxLevel 层）"
    if ($Cfg.Guard.ProtectPorts -contains $Cfg.Backend.Port) { Stop-Stack '后端端口在受保护名单里，不对它做冒烟' 2 }
    [void](Invoke-Adopt $Cfg $state $false)

    # 第 0 层：各个服务在跑并通过检查
    $allUp = $true
    $backendUp = $false
    $rsUp = $false
    foreach ($name in (Get-ServiceNames $Cfg)) {
        $info = Get-ServiceState $Cfg $state $name
        $detail = $info.Version
        if ($info.Detail) { $detail = "$detail；$($info.Detail)" }
        if ($info.State -ne 'ok') { $detail = $info.Reason; if (-not $detail) { $detail = '没起' }; $allUp = $false }
        if ($name -eq 'backend' -and $info.State -eq 'ok') { $backendUp = $true }
        if ($name -eq 'rs_server' -and $info.State -eq 'ok') { $rsUp = $true }
        $who = $info.Title
        if ($who -match '[A-Za-z0-9]$') { $who += ' ' }
        if ($info.State -eq 'waiting') {
            # rs-server 因为库版本没对上而没起：原因只记一次，记在下面「库版本一致」那一项上。
            Add-Skip 0 "${who}在跑并通过检查" "还没起：$($info.Reason)。见下面「库版本一致」那一项"
            continue
        }
        Add-Result 0 "${who}在跑并通过检查" ($info.State -eq 'ok') $detail
    }
    $setupMode = $false
    if ($backendUp) {
        # 后端自报的 status 会短暂变成 degraded（有一项每 30 秒重算一次，重算的那一下就这样），所以多看几次。
        $j = $null
        $status = ''
        for ($try = 0; $try -lt 6; $try++) {
            $health = Test-ServiceHealth $Cfg 'backend'
            $j = $health.Json
            $status = [string](Get-Prop $j 'status')
            if ($status -eq 'ok' -or [string](Get-Prop $j 'mode') -eq 'setup') { break }
            Start-Sleep -Seconds 2
        }
        if ([string](Get-Prop $j 'mode') -eq 'setup') {
            $setupMode = $true
            Add-Result 0 '后端已经配置好' $false "后端在设置模式：打开 $(Get-BackendUrl $Cfg)/ 按向导填完配置"
        } else {
            Add-Result 0 '后端自报健康' ($status -eq 'ok') "status=$status；降级项 [$(@(Get-Prop $j 'degraded_sections') -join ',')]；阻断项 [$(@(Get-Prop $j 'blocking_conditions') -join ',')]"
            $closure = [string](Get-Prop (Get-Prop $j 'mdb_closure') 'status')
            Add-Result 0 '后端读得到 E3D 工程目录' ($closure -eq 'ok') "工程目录检查（mdb_closure.status）=$closure；项目 $(Get-Prop $j 'project')，MDB $(Get-Prop $j 'mdb')"
            # 库版本：对上了是 ✓；全新的库还没写（后端的启动没做完）或正在升级，是「–」——等得到，不算没过；
            # 等不到的几种（有数据却没有记录、版本旧了或新了、升级失败）才是 ✗。
            $svs = Get-StorageVersionKind $j
            if ($svs.Kind -eq 'ok') {
                Add-Result 0 '库版本一致' $true "库里的版本记录 $($svs.Stored)，程序要的 $($svs.Expected)"
            } elseif ($svs.Kind -eq 'starting' -or $svs.Kind -eq 'unknown') {
                Add-Skip 0 '库版本一致' "新库的版本记录还没写（$($svs.Text)）：后端做完启动时自己会写，样例工程十来秒。过一会儿再冒烟"
            } elseif ($svs.Kind -eq 'awaiting-model') {
                Add-Skip 0 '库版本一致' "还没有版本记录（$($svs.Text)）：后端要等第一次初始化完成才写。先初始化要用的库（stack init <库号>）再冒烟；rs-server 也要等它"
            } elseif ($svs.Kind -eq 'upgrading') {
                Add-Skip 0 '库版本一致' "后端正在做库的升级（已完成 $($svs.UpgradeDone) / $($svs.UpgradeTotal) 个库），做完才对得上"
            } else {
                $svWhy = '库里已经有数据、却没有版本记录，后端不会自己补写'
                if ($svs.Kind -eq 'old') { $svWhy = '这份数据是老一版的后端建的' }
                elseif ($svs.Kind -eq 'newer') { $svWhy = '这份数据是更新的后端建的' }
                elseif ($svs.Kind -eq 'upgrade-failed') { $svWhy = '上一次库升级没有全部成功' }
                Add-Result 0 '库版本一致' $false "$($svs.Text)。$svWhy；rs-server 因此不起。怎么办：运行 stack status，照它说的选一种办法"
            }
            $auto = Get-Prop $j 'auto_initialize_new_dbnums'
            Add-Result 0 '启动时不自动初始化新库' ($auto -eq $Cfg.Init.AutoNew) "后端此刻的开关 auto_initialize_new_dbnums=$auto（配置 [init] auto_new=$($Cfg.Init.AutoNew)）"
            # 「核对」（RVM 比对）：产物目录是不是入口定的那个；E3D 启动脚本找不找得到。
            $rv = Invoke-StackHttp 'GET' ((Get-BackendUrl $Cfg) + '/api/v1/rvm-check/defaults') '' 15
            $rvj = ConvertFrom-JsonSafe $rv.Body
            if ($Cfg.Backend.RvmCheckDir) {
                $rvDir = [string](Get-Prop $rvj 'work_dir')
                $rvOk = ($rv.Ok -and $rvDir -and (Test-SamePath $rvDir $Cfg.Backend.RvmCheckDir))
                $rvDetail = "后端用的是 $rvDir"
                if (-not $rv.Ok) { $rvDetail = "GET /api/v1/rvm-check/defaults 请求失败（$($rv.Error)）" }
                elseif (-not $rvOk) { $rvDetail += "，而 stack.toml 要的是 $($Cfg.Backend.RvmCheckDir)。后端配置里 [rvm_check] work_dir 没写或写的是别处：运行 stack down 再 stack up，照它给的提示改" }
                Add-Result 0 '「核对」的产物放在定好的目录里' $rvOk $rvDetail
            }
            if ($rv.Ok -and (Get-Prop $rvj 'launcher_found') -ne $true) {
                Add-Skip 0 '「核对」调得起 E3D' "没找到 E3D 启动脚本（后端试的是 $(Limit-Text ([string](Get-Prop $rvj 'e3d_launcher')) 120)）。不影响别的功能；要用「核对」，在后端配置的 [rvm_check] 一节里写 e3d_launcher = '启动脚本的完整路径'，再 stack down、stack up"
            }
        }
        # plant-ui 网页版：后端自己提供的首页
        $page = Invoke-StackHttp 'GET' ((Get-BackendUrl $Cfg) + '/') '' 15
        $isPlantUi = ($page.Ok -and $page.Body -match 'rs-plant')
        $pageDetail = "GET $(Get-BackendUrl $Cfg)/ → HTTP $($page.Status)，$($page.Bytes) 字节"
        if ($page.Ok -and -not $isPlantUi) { $pageDetail += '；不是 plant-ui 的首页（里面没有 rs-plant）' }
        Add-Result 0 'plant-ui 网页版首页打得开' $isPlantUi $pageDetail
        if ($Cfg.RsServer.Enabled -and $Cfg.Backend.PlantUiWebRoot) {
            $c = Invoke-StackHttp 'GET' ((Get-BackendUrl $Cfg) + '/config.json') '' 10
            $cj = ConvertFrom-JsonSafe $c.Body
            $dataUrl = [string](Get-Prop $cj 'data_api_url')
            $want = Get-RsServerPublicUrl $Cfg
            $db = Get-Prop $cj 'db'
            $noSecret = ([string](Get-Prop $db 'username') -eq '' -and [string](Get-Prop $db 'password') -eq '')
            $cDetail = "数据服务地址是「$dataUrl」（应当是 $want）"
            if (-not $noSecret) { $cDetail += '；注意：这份配置里带着数据库账号口令' }
            Add-Result 0 'plant-ui 网页版知道 rs-server 在哪' ($c.Ok -and $dataUrl -eq $want -and $noSecret) $cDetail
        }
    }
    if ($rsUp) {
        # rs-server：它自己看到的库版本；元数据网页；从后端页面里调它时浏览器放不放行（跨域）
        $d = Invoke-StackHttp 'GET' ((Get-RsServerUrl $Cfg) + '/api/metadata/database') '' 60
        $dj = ConvertFrom-JsonSafe $d.Body
        $needUpgrade = Get-Prop $dj 'upgradeRequired'
        Add-Result 0 'rs-server 看到的库版本一致' ($d.Ok -and $needUpgrade -eq $false) "库里 $(Get-Prop $dj 'stored')，它要的 $(Get-Prop $dj 'expected')，要不要升级 $needUpgrade；它看到 $(@(Get-Prop $dj 'databases').Count) 个库文件"
        $m = Invoke-StackHttp 'GET' ((Get-RsServerUrl $Cfg) + '/metadata') '' 15
        Add-Result 0 'rs-server 的元数据网页打得开' $m.Ok "GET $(Get-RsServerUrl $Cfg)/metadata → HTTP $($m.Status)，$($m.Bytes) 字节"
        $origin = "http://127.0.0.1:$($Cfg.Backend.Port)"
        $cors = Test-CorsAllowed ((Get-RsServerUrl $Cfg) + '/hello') $origin
        Add-Result 0 '从后端的页面里调 rs-server 浏览器会放行' $cors.Ok "带 Origin: $origin 请求它，响应头 Access-Control-Allow-Origin 是「$($cors.Allow)」"
    }

    # 第 1 层：模型树有根
    $nodes = @()
    $rows = @()
    $initialized = @()
    if ($backendUp -and -not $setupMode -and $MaxLevel -ge 1) {
        $r = Invoke-StackHttp 'GET' ((Get-BackendUrl $Cfg) + '/api/v1/tree/roots') '' 120
        $nodes = @(Get-Prop (ConvertFrom-JsonSafe $r.Body) 'nodes' | Where-Object { $null -ne $_ })
        $detail = "根 $($nodes.Count) 个（直接读 E3D 库文件，不用初始化）"
        if (-not $r.Ok) { $detail = "请求失败（$($r.Error)）：$(Limit-Text $r.Body 160)" }
        Add-Result 1 '模型树有根' ($r.Ok -and $nodes.Count -gt 0) $detail
        $rows = @(Get-DbnumRows $Cfg | Where-Object { $null -ne $_ })
        $initialized = @($rows | Where-Object { (Get-Prop $_ 'initialized') -eq $true } | ForEach-Object { [int](Get-Prop $_ 'dbnum') })
        $shown = ($initialized | Select-Object -First 12) -join '、'
        if ($initialized.Count -gt 12) { $shown += ' …' }
        if ($initialized.Count -eq 0) { $shown = '没有' }
        Add-Result 1 '后端的库列表读得出来' ($rows.Count -gt 0) "$($rows.Count) 个库；已经初始化过的 $($initialized.Count) 个：$shown"
        foreach ($db in $Cfg.Init.Dbnums) {
            $row = Get-DbnumRow $rows $db
            $ok = ($null -ne $row -and (Get-Prop $row 'initialized') -eq $true)
            $detail = '后端的库列表里没有它'
            if ($null -ne $row) { $detail = "initialized=$(Get-Prop $row 'initialized')；数据到会话 $(Get-Prop $row 'applied_sesno')，文件最新会话 $(Get-Prop $row 'file_latest_sesno')；模型判定 $(Get-Prop $row 'model_verdict')" }
            Add-Result 1 "配置里点名的库 $db 已初始化" $ok $detail
        }
    }

    # 第 2 层：取到一个模型——取到记录，再取到其中一个网格
    if ($backendUp -and -not $setupMode -and $MaxLevel -ge 2) {
        if ($NodeRef) {
            # 给了节点号：在这个节点上按需生成一次，再取记录和网格。
            # 先看这个节点在哪个库、那个库初始化过没有。没初始化的库缺省不做、要加 -Force：在没初始化的库上按需生成，
            # 结果会留在数据库里。带 gen-model #146 修复的后端（2026-11-11 晚之后的构建）之后初始化这个库时会自己清掉；
            # 更早的后端清不掉，初始化会被挡下（「模型面残留」）。入口从后端的版本号上分不出这两种，所以让人自己拿主意。
            $refno = $NodeRef.Trim().Replace('_', '/')
            $ownerDb = 0
            $ownerInitialized = $false
            if ($refno -match '^(\d+)/\d+$') {
                $ref0 = [long]$Matches[1]
                foreach ($row in $rows) {
                    if (@(Get-Prop $row 'ref0s' | Where-Object { $null -ne $_ } | ForEach-Object { [long]$_ }) -contains $ref0) {
                        $ownerDb = [int](Get-Prop $row 'dbnum')
                        $ownerInitialized = ((Get-Prop $row 'initialized') -eq $true)
                        break
                    }
                }
            }
            $allowed = ($ownerInitialized -or $ForceNode)
            if (-not $allowed) {
                $where = '没查到这个节点在哪个库（节点号要写成 23736/1 这样）'
                if ($ownerDb -gt 0) { $where = "节点所在的库 $ownerDb 还没有初始化" }
                Add-Result 2 "在节点 $refno 上按需生成" $false "没有做：$where。在没初始化的库上按需生成，结果会留在数据库里：带 #146 修复的后端（test3 起的安装包里就是）以后初始化这个库时会自己清掉；更早的后端清不掉，那个库就初始化不了。要做就加 -Force；想稳妥，先 stack init $ownerDb 再冒烟"
            } else {
            $g = Invoke-StackHttp 'POST' ((Get-BackendUrl $Cfg) + '/api/v1/model/generate') ('{"refno":"' + $refno + '"}') 150
            $gj = ConvertFrom-JsonSafe $g.Body
            $gDetail = "POST /model/generate → HTTP $($g.Status)"
            if ($g.Ok) { $gDetail = "库 $ownerDb；状态 $(Get-Prop $gj 'status')；生成根 $(@(Get-Prop $gj 'generation_roots').Count) 个，其中失败 $([int](Get-Prop $gj 'failed_root_count')) 个；画得出来的实例 $(Get-Prop $gj 'model_instance_count') 个；取数源 $(Get-Prop $gj 'model_source')（落库 = $(Get-Prop $gj 'durable')）" }
            else { $gDetail += "：$(Limit-Text $g.Body 200)" }
            if ($g.Ok -and -not $ownerInitialized) { $gDetail += "。库 $ownerDb 还没初始化，按 -Force 做了" }
            Add-Result 2 "在节点 $refno 上按需生成" $g.Ok $gDetail
            if ($g.Ok) {
                $r = Invoke-StackHttp 'POST' ((Get-BackendUrl $Cfg) + '/api/v1/model/records') ('{"generation_root":"' + $refno + '","limit":500}') 120
                $j = ConvertFrom-JsonSafe $r.Body
                $hash = Get-FirstGeoHash $j
                $got = $false
                $detail = "记录 $(Get-Prop $j 'total') 条（来源 $(Get-Prop $j 'source')）"
                if ($hash) {
                    $mesh = Invoke-StackHttp 'GET' ((Get-BackendUrl $Cfg) + "/api/v1/meshes/$hash.mesh") '' 60
                    $got = ($mesh.Ok -and $mesh.Bytes -gt 0)
                    $detail += "；网格 $(Limit-Text $hash 28) 取到 $($mesh.Bytes) 字节"
                } else { $detail += '；前 500 条里没有带网格的（这个节点下面可能没有画得出来的东西）' }
                Add-Result 2 "取到节点 $refno 的模型（记录 + 网格）" $got $detail
            }
            }
        } else {
            # 没给节点号：只在已经初始化过的库里取，不触发任何生成。
            $candidates = @($nodes | Where-Object { $initialized -contains [int](Get-Prop $_ 'dbnum') })
            if ($candidates.Count -eq 0) {
                Add-Skip 2 '取到一个模型（记录 + 网格）' '还没有初始化过的库，这一层跳过（冒烟不替你生成模型）。要验证：stack init <库号> 之后再冒烟；或者 stack smoke -Node <节点号>，在那个节点上按需生成一次'
            } else {
                $got = $false
                $detail = ''
                foreach ($node in ($candidates | Select-Object -First 8)) {
                    $refno = ([string](Get-Prop $node 'refno')).Replace('_', '/')
                    $label = "根 $refno（$(Get-Prop $node 'name')，库 $(Get-Prop $node 'dbnum')）"
                    $r = Invoke-StackHttp 'POST' ((Get-BackendUrl $Cfg) + '/api/v1/model/records') ('{"generation_root":"' + $refno + '","limit":500}') 120
                    if (-not $r.Ok) { $detail = "$label 取记录失败（$($r.Error)）：$(Limit-Text $r.Body 160)"; continue }
                    $j = ConvertFrom-JsonSafe $r.Body
                    $hash = Get-FirstGeoHash $j
                    if (-not $hash) { $detail = "$label 有 $(Get-Prop $j 'total') 条记录，但前 500 条里没有带网格的"; continue }
                    $mesh = Invoke-StackHttp 'GET' ((Get-BackendUrl $Cfg) + "/api/v1/meshes/$hash.mesh") '' 60
                    if ($mesh.Ok -and $mesh.Bytes -gt 0) {
                        $got = $true
                        $detail = "$label：记录 $(Get-Prop $j 'total') 条（来源 $(Get-Prop $j 'source')，complete=$(Get-Prop $j 'complete')）；网格 $(Limit-Text $hash 28) 取到 $($mesh.Bytes) 字节"
                        break
                    }
                    $detail = "$label 的网格 $(Limit-Text $hash 28) 没取到（$($mesh.Error)）"
                }
                Add-Result 2 '已初始化的库里取到一个模型（记录 + 网格）' $got $detail
            }
        }
    }

    $failed = 0
    $skipped = 0
    foreach ($r in $results) {
        $mark = '✓'
        $color = 'Green'
        if ($r.Skip) { $mark = '–'; $color = 'Yellow'; $skipped += 1 }
        elseif (-not $r.Ok) { $mark = '✗'; $color = 'Red'; $failed += 1 }
        Say ("  第 $($r.Level) 层  $mark " + (Format-Cell $r.What 40) + $r.Detail) $color
    }
    Say ''
    if ($failed -gt 0) {
        Say "冒烟没过：$($results.Count) 项里 $failed 项不通过。下一步：先运行 stack status 看服务是否都正常，再对照上面每一项后面的说明。" 'Red'
        return 5
    }
    if ($skipped -gt 0) { Say "有 $skipped 项因为条件不具备跳过了（上面标「–」的），不算没过。" 'Yellow' }
    Say "冒烟通过：$($results.Count - $skipped) 项都过了。" 'Green'
    return 0
}

function Invoke-Watch($Cfg, [int]$WatchSeconds) {
    Say "stack watch —— 盯 $WatchSeconds 秒（$($Cfg.Name)）"
    $deadline = (Get-Date).AddSeconds($WatchSeconds)
    $peak = 0
    while ($true) {
        $state = Read-StackState $Cfg
        [void](Invoke-Adopt $Cfg $state $false @() $true)
        $parts = @()
        foreach ($name in (Get-ServiceNames $Cfg)) {
            $title = Get-ServiceTitle $name
            if (-not $state.ContainsKey($name)) { $parts += "$title 没起"; continue }
            if (-not (Test-RecordedProcess $Cfg $name $state[$name]).Ours) { $parts += "$title 不在了"; continue }
            $mem = Get-ProcMemGb $state[$name].pid
            if ($name -eq 'backend' -and $mem -gt $peak) { $peak = $mem }
            $parts += "$title $mem GB"
        }
        Say "  $((Get-Date).ToString('HH:mm:ss'))  可提交余量 $(Get-CommitFreeGb) GB；$($parts -join '；')"
        $guard = Test-Guards $Cfg $state
        if ($guard) { Stop-OnGuard $Cfg $guard }
        if ((Get-Date) -ge $deadline) { break }
        Start-Sleep -Seconds 5
    }
    Say "盯完了。后端内存最高 $peak GB。"
    return 0
}

# 命令 init：初始化点名的库。服务要先起好（数据库和后端是本套的、而且正常）。
function Invoke-InitCommand($Cfg, [int[]]$Dbnums) {
    Say "stack init —— $($Cfg.Name)：库 $(@($Dbnums) -join '、')"
    $state = Read-StackState $Cfg
    [void](Invoke-Adopt $Cfg $state $false)
    foreach ($name in 'database', 'backend') {
        $info = Get-ServiceState $Cfg $state $name
        if ($info.State -ne 'ok') {
            $why = $info.Reason
            if (-not $why) { $why = '没起' }
            Say "✗ $($info.Title) 不在正常状态（$why），没法初始化。" 'Red'
            Say '  下一步：先运行 stack up 把服务起好，再运行 stack init。' 'Yellow'
            return 4
        }
    }
    Invoke-Init $Cfg $state $Dbnums
    Say '初始化做完了。在 plant3d-web 里刷新一下就能看到这些库的模型；stack smoke 可以再核一遍。' 'Green'
    # rs-server 之前因为库版本没对上而没起的：初始化完可能已经对上了（配了 watch_dbnums 的新库就是这样）。提醒一句。
    if ($Cfg.RsServer.Enabled -and $Cfg.RsServer.WaitStorageVersion -and -not $state.ContainsKey('rs_server')) {
        $sv = Get-StorageVersionState $Cfg
        if ($sv.Kind -eq 'ok') { Say "rs-server 还没起，而库版本现在一致了（$($sv.Text)）：运行 stack up（双击「开始」），它会把 rs-server 补起来。" 'Yellow' }
        elseif ($sv.Kind -ne 'unknown') { Say "rs-server 还没起：库版本还没对上（$($sv.Text)）。运行 stack status 看是哪种情况、怎么办。" 'Yellow' }
    }
    return 0
}

# 只生成各服务的配置文件（和 up 里生成的一模一样），不起任何服务。给想先看一眼的人用。
function Invoke-Prepare($Cfg) {
    Say "stack prepare —— 只生成配置文件，不起服务（$($Cfg.Name)）"
    Initialize-BackendFiles $Cfg
    Say "  后端配置：$($Cfg.Backend.ConfigFile)"
    Say "  数据库函数脚本：$(Join-Path $Cfg.Backend.WorkDir 'resource\surreal')"
    if ($Cfg.RsServer.Enabled) {
        Initialize-RsServerFiles $Cfg
        Say "  rs-server 的运行目录：$($Cfg.RsServer.WorkDir)（配置 DbOption.toml、resource\、assets\）"
    }
    if ($Cfg.Frontend.Enabled) {
        Initialize-FrontendFiles $Cfg
        if ($Cfg.Frontend.Kind -eq 'nginx') { Say "  nginx 的配置：$(Join-Path $Cfg.Frontend.WorkDir 'conf\nginx.conf')" }
        else { Say "  前端的包装配置：$($Cfg.Frontend.WrapperConfig)" }
    }
    Say '生成好了。后端和 rs-server 的配置只在不存在时生成，已经在的不覆盖；前端的配置每次都重写。'
    return 0
}

# 只把配置解析出来给人看：不查端口、不碰进程、不执行任何程序。
function Invoke-ShowConfig($Cfg) {
    Say "stack config —— 解析结果（$($Cfg.Name)）。只读配置，不查端口、不碰进程。"
    Say "  配置文件：$($Cfg.ConfigPath)"
    Say "  运行目录：$($Cfg.Root)"
    Say "  项目：$($Cfg.Project.Name)（编码 $($Cfg.Project.Code)，MDB $($Cfg.Project.Mdb)，$($Cfg.Project.E3dVersion)），工程根目录 $($Cfg.Project.E3dRoot)"
    $listenText = '维持现状：后端、rs-server 和前端监听所有网卡'
    if ($Cfg.LoopbackOnly) { $listenText = '只许本机访问：后端、rs-server 和前端都只听 127.0.0.1' }
    Say "  对谁开放：$listenText；数据库听 $($Cfg.Database.Host)"
    $initText = '不点名初始化任何库（缺省）：不初始化、不生成模型'
    if ($Cfg.Init.Dbnums.Count -gt 0) { $initText = "起好之后初始化库 $($Cfg.Init.Dbnums -join '、')（解析数据并生成这些库的模型）" }
    Say "  初始化：$initText；后端启动时自动初始化新库 = $($Cfg.Init.AutoNew)"
    if ($Cfg.RsServer.Enabled) {
        $gateText = '后端健康、库版本一致之后才起'
        if (-not $Cfg.RsServer.WaitStorageVersion) { $gateText = '后端健康之后就起（不等库版本）' }
        Say "  rs-server：启用；$gateText；自动升级关着；浏览器访问它用的地址 $(Get-RsServerPublicUrl $Cfg)"
    } else {
        Say '  rs-server：没启用'
    }
    Say "  护栏：开工线 $($Cfg.Guard.MinCommitFreeGb) GB，收摊线 $($Cfg.Guard.AbortCommitFreeGb) GB，后端上限 $($Cfg.Guard.MaxBackendGb) GB（0 = 不设）；越线自动收摊 = $($Cfg.Guard.AutoDown)；受保护端口 [$($Cfg.Guard.ProtectPorts -join ', ')]"
    $keys = Get-BackendConfigKeys $Cfg
    Say ''
    Say "  后端配置 $($Cfg.Backend.ConfigFile)"
    Say "    模板：$($Cfg.Backend.ConfigTemplate)（文件不存在时从它生成一次，以后不覆盖）"
    Say '    入口写进去的键：'
    foreach ($key in $keys.Keys) { Say "      $key = $(Format-TomlLiteral $keys[$key] $false)" }
    if ($Cfg.Backend.RvmCheckDir) { Say "      [rvm_check] work_dir = $(Format-TomlPath $Cfg.Backend.RvmCheckDir)      （「核对」的产物放这里，不跟着程序的位置走）" }
    Say '    （写进文件时保持模板里原来的写法：模板里某个数字带着引号，就继续带引号。）'
    Say '    账号口令两个键（v_user / v_password）照模板，入口不改也不显示。'
    if ($Cfg.RsServer.Enabled) {
        Say ''
        Say "  rs-server 的配置 $(Join-Path $Cfg.RsServer.WorkDir 'DbOption.toml')"
        Say '    以后端那份配置为底生成一次（库地址、账号、命名空间、项目照抄），以后不覆盖。入口另写的键：'
        Say "      server_release_ip = `"$((Get-BackendBindHost $Cfg) + ':' + $Cfg.RsServer.Port)`"      （它的监听地址）"
        Say "      aios_database_url = `"$(Get-BackendUrl $Cfg)`"      （后端的地址）"
        Say '      ip、user、password、puhua_database_ip、puhua_database_user、puhua_database_password、plat_url = ""      （MySQL 和数据中心平台，留空）'
        Say "    运行文件从 $($Cfg.RsServer.TemplateDir) 拷进它的运行目录（resource\、assets\）。"
    }
    foreach ($name in (Get-ServiceNames $Cfg)) {
        $spec = Get-ServiceSpec $Cfg $name
        Say ''
        Say "  $($spec.Title)（端口 $($spec.Port)）"
        Say "    程序：$($spec.Exe)"
        $argText = $spec.Args -join ' '
        if (-not $argText) { $argText = '（不带参数）' }
        Say "    参数：$argText"
        Say "    工作目录：$($spec.Cwd)"
        Say '    环境变量——入口定的：'
        Say "      PATH = $($spec.Env['PATH'])"
        Say "      TEMP / TMP = $($spec.Env['TEMP'])"
        foreach ($key in $spec.SetEnv.Keys) {
            if ($spec.Hidden.ContainsKey($key)) { Say "      $key = （$($spec.Hidden[$key])）" }
            else { Say "      $key = $($spec.SetEnv[$key])" }
        }
        foreach ($key in $spec.Late.Keys) { Say "      $key = （$($spec.Late[$key])）" }
        Say "    环境变量——照搬本机系统的（只这些名字，值不显示）：$($spec.InheritedNames -join '、')"
        Say '    除此之外不带任何环境变量。'
    }
    return 0
}

function Show-Help {
    $text = @"
stack $script:StackVersion —— 读一份配置，把数据库、后端、rs-server、前端按顺序起起来，起一个查一个

用法
  stack <命令> [-Config <配置文件>]
      不给 -Config 时用当前目录下的 stack.toml。
  不经 stack.cmd、直接跑脚本：
      powershell -NoProfile -ExecutionPolicy Bypass -File stack.ps1 <命令> -Config <配置文件>
  双击：安装包里是「开始.cmd」「停止.cmd」「状态.cmd」（英文名的 start.cmd / stop.cmd / status.cmd 是同一个东西）。

命令
  up        预检 → 生成各服务的配置 → 按 数据库、后端、rs-server、前端 的顺序起没起的，起一个查一个。
            已经在跑的不动；再跑一遍不会多起进程。
            缺省不初始化任何库、不生成模型；配置里 [init] dbnums 点了名才初始化那几个库。
              -NoInit      配置里点了名也不初始化，只起服务
  down      倒着停：前端、rs-server、后端、数据库。只停本套自己起的。
              -WhatIf      只列出会停哪些进程，不真停（也可以写 -DryRun）
  status    一张表：每个服务的状态、端口、进程号、起的时间、内存、版本；不正常的说出原因和下一步
  init      初始化点名的库：stack init 7997 8000（库号用空格或逗号隔开）。
            初始化 = 把这个库的全部元素解析进数据库，并生成这个库的模型；后端会把系统库和字典库
            一并处理。已经初始化过的库不重复做。服务要先起好。
  smoke     冒烟，三层：
              第 0 层  各个服务在跑并通过检查；后端健康、读得到工程目录、库版本一致；
                       plant-ui 首页；rs-server 的接口和元数据网页
              第 1 层  模型树有根（直接读 E3D 库文件，不用初始化）
              第 2 层  取到一个模型（记录 + 网格）。缺省只在已经初始化过的库里取，不触发任何生成；
                       一个库都没初始化时这一层跳过。
              -Level n     只做到第 n 层（缺省 2）
              -Node 节点号 第 2 层改成：在这个节点上按需生成一次再取（像界面里在节点上点「生成」）。例：-Node 23736/1
                           节点所在的库还没初始化时缺省不做，要再加 -Force：在没初始化的库上按需生成，结果会留在
                           数据库里；带 gen-model #146 修复的后端以后初始化这个库时会自己清掉，更早的后端清不掉。
  doctor    只检查、不起任何服务：路径、端口、程序版本、内存和磁盘余量
  watch     盯一段时间：每 5 秒报一次各服务的内存；越过配置的线就按配置处理
              -Seconds n   盯多久（缺省 120）
  config    把配置解析出来给人看：每个服务用什么程序、参数、工作目录、环境变量。
            不查端口、不碰进程
  prepare   只生成各服务的配置文件（和 up 里生成的一样），不起服务。想先看一眼再起时用
  help      这一页

配置
  安装包里带的是 stack.default.toml（缺省配置，包内的路径和端口都填好了）。第一次运行 stack up 时，
  入口照它生成 stack.toml 然后停下，请你把 [project]（E3D 工程根目录、项目名、项目编码、MDB、E3D 版本）
  改好再运行一次。stack.toml 是你自己的那一份，不在安装包里，换新一版时解压不会盖掉它。
  开发机上从 stack.example.toml 拷一份改。
  配置里有不认识的写法或不认识的键，会报错并指出行号，不会悄悄忽略。
  相对路径：[stack] root 相对配置文件所在的目录；其余路径都相对 root。

它替人做了什么
  · 后端的配置（DbOption.toml）从模板生成一次，只改入口管的那十几个键，其余照模板；以后不覆盖，
    在向导页里改的东西留得住。它要连的数据库、监听的端口和 stack 的配置不一致时不起。
    stack.toml 里入口管的键后来又改了的话，它只提醒，不替你覆盖。
  · 后端启动时不自动初始化库（明确写进后端配置；后端代码自己的缺省是「自动」）。
  · 「核对」（RVM 比对）的产物目录明确写进后端配置（[rvm_check] work_dir），缺省是 data\rvm-check，
    不跟着后端程序的位置走——换一版程序之后，以前的产物在界面上照样下载得到。
  · rs-server：后端健康、库版本一致之后才起；关掉它的自动升级；给它一个干净的运行目录；
    它的配置以后端那份为底生成，MySQL 和数据中心平台几项留空；
    再给 plant-ui 网页版写一份配置，让网页版找得到它。
    库版本只在等得到的时候等（全新的库后端做完启动时自己写；或者后端正在升级）。等不到的——
    库里有数据却没有版本记录、版本记录比程序旧——马上说出是哪种情况、在哪做什么，不白等。
  · 每个服务的环境变量由入口明确给出（stack config 能看到全部），不带调用方环境里的东西。
  · 数据库账号口令沿用后端配置模板里现有的那一对，经环境变量递给数据库；不打印、不上命令行、
    不写进状态文件和日志。
  · 认进程靠「进程号 + 启动时间 + 程序路径」（开发档的前端靠命令行里的标记），从不按进程名。
    所以各个程序要用放在运行目录里的那一份（[guard] require_exe_under_root）。起和停要用同样权限的窗口。
    状态文件里没有记的进程当别人的，只有一种会被认领回来：它的程序就是运行目录里的那一个、
    又占着本套的端口——在后端的向导页里点了「重启」、或者 stack up 跑到一半窗口被关，就会有这种进程。
    连带收子进程（nginx 的工作进程）时只认创建时间不早于它的；占着受保护端口的进程一律不动。
  · 缺省不改对谁开放：后端、rs-server、前端监听所有网卡。只想让本机访问，把 [listen] loopback_only 设成 true。

文件都在哪（都在 [stack] root 下面）
  run\state.json        记着本套起了哪些进程（没有口令）
  run\stack.lock        up / down / prepare 正在跑的时候占着它，防止两个同时跑
  logs\<服务>\          每个服务自己的输出，每起一次一个文件
  logs\stack\           入口自己打印过的内容，按天一个文件
  work\backend\         后端的工作目录：生成的 DbOption.toml、网格、日志、锁
  work\rs-server\       rs-server 的运行目录：生成的 DbOption.toml、resource、assets
  work\nginx\           nginx 的配置、日志、临时文件（开发档是 work\frontend\）
  data\surreal\         数据库的数据
  data\rvm-check\       「核对」（RVM 比对）的产物，每次核对一个子文件夹
  tmp\                  给服务用的临时目录

退出码
  0 正常              2 配置或用法有问题      3 运行中越过内存线、等让路文件超时、或另一个 stack 正在跑
  4 预检没过（包括开工时内存余量不够）、有服务没起好、或点名的库没初始化成        5 冒烟没过        1 其它
"@
    Write-Host $text
}

# ─────────────────────────── 主流程 ───────────────────────────

# 测试用：点源本文件并带上 __lib（ . stack.ps1 __lib ），只把上面的函数定义进调用方，不执行任何命令。
if ($Command -eq '__lib') { return }

try {
    $commandName = $Command.ToLowerInvariant()
    if ($commandName -eq 'help' -or $commandName -eq '-h' -or $commandName -eq '--help' -or $commandName -eq '/?' -or $commandName -eq '-?') {
        Show-Help
        exit 0
    }
    if ('up', 'down', 'status', 'init', 'smoke', 'doctor', 'watch', 'config', 'prepare', '__spawn' -notcontains $commandName) {
        Write-Host "不认识的命令「$Command」。可用的命令：up、down、status、init、smoke、doctor、watch、config、prepare、help。" -ForegroundColor Red
        exit 2
    }
    # 命令后面不带名字的参数：只有 init 收（库号）。别的命令多出来的、还有不认识的开关，都报错，不悄悄吞掉。
    $extra = @()
    $initDbnums = @()
    foreach ($a in @($Rest)) {
        if ($null -eq $a -or "$a" -eq '') { continue }
        if ($commandName -eq 'init' -and "$a" -match '^[0-9]+([,\s]+[0-9]+)*$') {
            foreach ($piece in ("$a" -split '[,\s]+')) { if ($piece) { $initDbnums += [int]$piece } }
        } else { $extra += "$a" }
    }
    if ($commandName -ne '__spawn' -and $Service) { $extra += "-Service $Service" }
    if ($extra.Count -gt 0) {
        Write-Host "不认识的参数：$($extra -join ' ')。认识的有 -Config、-WhatIf（-DryRun）、-NoInit、-Level、-Node、-Force、-Seconds；init 后面跟库号。用法见 stack help。" -ForegroundColor Red
        exit 2
    }
    if ($commandName -eq 'init' -and $initDbnums.Count -eq 0) {
        Write-Host '要初始化哪几个库？用法：stack init <库号> [<库号> …]，例如 stack init 7997 8000。库号在界面的模型树上、或 stack smoke 的输出里能看到。' -ForegroundColor Red
        exit 2
    }
    $configFile = $Config
    if (-not $configFile) { $configFile = Join-Path (Get-Location).Path 'stack.toml' }
    # 安装包里带的是 stack.default.toml（缺省配置）；stack.toml 是用户自己的那一份，包里没有——
    # 这样换新一版时，解压覆盖不会把用户改过的配置盖掉。
    # 第一次：stack.toml 不在、旁边有 stack.default.toml，就照它拷一份出来，然后停下，请人先把 [project] 改好。
    $configFull = [System.IO.Path]::GetFullPath($configFile)
    if (-not (Test-Path -LiteralPath $configFull -PathType Leaf) -and (Split-Path -Leaf $configFull) -ieq 'stack.toml') {
        $defaultFile = Join-Path (Split-Path -Parent $configFull) 'stack.default.toml'
        if (Test-Path -LiteralPath $defaultFile -PathType Leaf) {
            if ('up', 'doctor', 'config', 'prepare' -contains $commandName) {
                Copy-Item -LiteralPath $defaultFile -Destination $configFull
                Write-Host "第一次运行：这里还没有 stack.toml，已经照 stack.default.toml 生成了一份：$configFull" -ForegroundColor Yellow
                Write-Host '请先用记事本打开它，把最上面 [project] 一节改成你的工程（E3D 工程根目录、项目名、项目编码、E3D 版本、MDB），保存；' -ForegroundColor Yellow
                Write-Host '然后再运行一次（双击「开始」）。这一次什么都没有起。' -ForegroundColor Yellow
                Write-Host '这份 stack.toml 是你自己的，不在安装包里：以后换新一版，解压不会动它。' -ForegroundColor Yellow
                exit 2
            }
            Write-Host "这里还没有 stack.toml（$configFull）。先运行 stack up（双击「开始」）：它会照 stack.default.toml 生成一份，请你改好 [project] 再运行。" -ForegroundColor Red
            exit 2
        }
    }
    $cfg = Get-StackConfig $configFile

    if ($commandName -eq '__spawn') {
        New-Dir $cfg.RunDir
        Invoke-Spawn $cfg $Service
        exit 0
    }

    # 入口自己的日志：up 会把运行目录建出来；其余命令只在运行目录已经存在时才记，不为了记日志去建目录。
    if ($commandName -eq 'up' -or $commandName -eq 'prepare') { foreach ($dir in $cfg.Root, $cfg.RunDir, $cfg.LogsDir) { New-Dir $dir } }
    if ((Test-Path -LiteralPath $cfg.Root -PathType Container) -and $commandName -ne 'config') {
        $stackLogDir = Join-Path $cfg.LogsDir 'stack'
        New-Dir $stackLogDir
        $script:LogFile = Join-Path $stackLogDir ('stack-' + (Get-Date).ToString('yyyyMMdd') + '.log')
        Write-StackLog "──── stack $commandName（PowerShell $($PSVersionTable.PSVersion)，进程 $PID）────"
    }

    $onlyTell = [bool]($WhatIf -or $DryRun)
    $takesLock = ($commandName -eq 'up' -or $commandName -eq 'prepare' -or ($commandName -eq 'down' -and -not $onlyTell))
    if ($takesLock -and (Test-Path -LiteralPath $cfg.Root -PathType Container)) { Enter-StackLock $cfg }

    $outcome = 0
    if ($commandName -eq 'up') { Invoke-Up $cfg | Out-Null }
    elseif ($commandName -eq 'down') { $outcome = Invoke-Down $cfg $onlyTell }
    elseif ($commandName -eq 'status') { $outcome = Invoke-Status $cfg }
    elseif ($commandName -eq 'doctor') { $outcome = Invoke-Doctor $cfg }
    elseif ($commandName -eq 'init') { $outcome = Invoke-InitCommand $cfg $initDbnums }
    elseif ($commandName -eq 'smoke') { $outcome = Invoke-Smoke $cfg $Level $Node ([bool]$Force) }
    elseif ($commandName -eq 'watch') { $outcome = Invoke-Watch $cfg $Seconds }
    elseif ($commandName -eq 'config') { $outcome = Invoke-ShowConfig $cfg }
    elseif ($commandName -eq 'prepare') { $outcome = Invoke-Prepare $cfg }
    # 函数里万一有东西漏进了输出流，这里只认最后那个数。
    $script:ExitCode = [int](@($outcome) | Select-Object -Last 1)
} catch {
    $code = 1
    $ex = $_.Exception
    if ($null -ne $ex -and $null -ne $ex.Data -and $ex.Data.Contains('StackExitCode')) { $code = [int]$ex.Data['StackExitCode'] }
    $quiet = ($null -ne $ex -and $null -ne $ex.Data -and $ex.Data.Contains('StackQuiet') -and $ex.Data['StackQuiet'])
    $message = "$($ex.Message)"
    if ($code -eq 1) { $message = "入口自己出错了：$message`n$($_.ScriptStackTrace)" }
    if (-not $quiet) { Say "✗ $message" 'Red' } else { Write-StackLog "✗ $message" }
    $script:ExitCode = $code
}
Write-StackLog "退出码 $script:ExitCode"
exit $script:ExitCode
