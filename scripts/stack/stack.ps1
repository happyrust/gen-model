<#
.SYNOPSIS
    stack —— 读一份配置，把数据库、后端、前端按顺序起起来，起一个查一个。

.DESCRIPTION
    用法见 `stack help`。

    这份脚本按 Windows PowerShell 5.1 能跑来写：不用三元运算、`??`、`?.`、`&&`、`||`。
    文件要存成带 BOM 的 UTF-8——5.1 把没有 BOM 的脚本当本地编码读，中文会乱。

    几条写死的纪律（改之前先想清楚）：
      * 停进程只按进程号，而且先核对「进程号 + 启动时间 + 程序路径（前端是命令行里的标记）」；
        从不按进程名停。状态文件里没有记的进程一律当别人的，不认领。
      * 数据库账号口令只在真正起数据库的那一小段里从后端配置读出来、经环境变量递下去；
        不打印、不上命令行、不进状态文件和日志。
      * 每个服务的环境变量由这里明确给出，不继承调用方的环境。
#>
param(
    [string]$Command = 'help',
    [string]$Config = '',
    [string]$Service = '',
    [switch]$WhatIf,
    [switch]$DryRun,
    [switch]$NoInit,
    [int]$Seconds = 120,
    [int]$Level = 2
)

$ErrorActionPreference = 'Stop'
$script:StackVersion = '0.1.0'
$script:LogFile = $null
$script:ExitCode = 0
$script:ListenCache = $null
$script:ListenCacheAt = [datetime]::MinValue
$script:LockStream = $null
$script:Utf8NoBom = New-Object System.Text.UTF8Encoding -ArgumentList $false

try { [Console]::OutputEncoding = $script:Utf8NoBom } catch { }

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
    Assert-KnownKeys $raw '顶层' @('stack', 'project', 'database', 'backend', 'frontend', 'listen', 'init', 'guard', 'timeouts')

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
    Assert-KnownKeys $backend 'backend' @('exe', 'port', 'work_dir', 'config_template', 'resource_dir', 'schema_root', 'plant_ui_web_root', 'watch_dbnums', 'env', 'config_set')
    Assert-Required $backend 'backend' @('exe', 'port', 'config_template', 'resource_dir', 'schema_root')
    foreach ($k in 'exe', 'work_dir', 'config_template', 'resource_dir', 'schema_root', 'plant_ui_web_root') { Assert-Type $backend 'backend' $k 'string' }
    Assert-Type $backend 'backend' 'port' 'int'

    $frontend = Get-Opt $raw 'frontend'
    Assert-KnownKeys $frontend 'frontend' @('enabled', 'kind', 'root', 'node', 'port', 'env')
    Assert-Type $frontend 'frontend' 'enabled' 'bool'
    $frontendEnabled = [bool](Get-Opt $frontend 'enabled' ($null -ne $frontend))
    if ($frontendEnabled) {
        Assert-Required $frontend 'frontend' @('root', 'port') '（不想带前端就写 enabled = false）'
        foreach ($k in 'kind', 'root', 'node') { Assert-Type $frontend 'frontend' $k 'string' }
        Assert-Type $frontend 'frontend' 'port' 'int'
        $kind = [string](Get-Opt $frontend 'kind' 'vite-dev')
        if ($kind -ne 'vite-dev') { Stop-Stack "配置$(Get-LineText 'frontend' 'kind')[frontend] kind 目前只支持 vite-dev（前端开发服务器），现在是「$kind」" 2 }
    }

    $init = Get-Opt $raw 'init'
    Assert-KnownKeys $init 'init' @('dbnums', 'mode', 'auto_new')
    Assert-Type $init 'init' 'auto_new' 'bool'
    Assert-Type $init 'init' 'mode' 'string'
    $initMode = [string](Get-Opt $init 'mode' 'data+model')
    if ('data', 'data+model' -notcontains $initMode) { Stop-Stack "配置$(Get-LineText 'init' 'mode')[init] mode 只能是 data 或 data+model，现在是「$initMode」" 2 }

    $guard = Get-Opt $raw 'guard'
    Assert-KnownKeys $guard 'guard' @('min_commit_free_gb', 'abort_commit_free_gb', 'max_backend_gb', 'auto_down', 'wait_lock_file', 'wait_lock_timeout_sec', 'protect_ports', 'require_exe_under_root', 'min_disk_free_gb')
    foreach ($k in 'min_commit_free_gb', 'abort_commit_free_gb', 'max_backend_gb', 'wait_lock_timeout_sec', 'min_disk_free_gb') { Assert-Type $guard 'guard' $k 'int' }
    foreach ($k in 'auto_down', 'require_exe_under_root') { Assert-Type $guard 'guard' $k 'bool' }
    Assert-Type $guard 'guard' 'wait_lock_file' 'string'

    $timeouts = Get-Opt $raw 'timeouts'
    Assert-KnownKeys $timeouts 'timeouts' @('database_sec', 'backend_sec', 'frontend_sec', 'init_sec')
    foreach ($k in 'database_sec', 'backend_sec', 'frontend_sec', 'init_sec') { Assert-Type $timeouts 'timeouts' $k 'int' }

    $backendWork = Resolve-StackPath ([string](Get-Opt $backend 'work_dir' 'work/backend')) $root
    $frontendWork = Join-Path $root 'work\frontend'

    $nodeExe = ''
    if ($frontendEnabled) {
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
            WatchDbnums = $watch
            Env = ConvertTo-EnvTable (Get-Opt $backend 'env') 'backend.env'
            ConfigSet = $configSet
        }
        Frontend = @{
            Enabled = $frontendEnabled
            Root = Resolve-StackPath ([string](Get-Opt $frontend 'root' '')) $root
            Node = $nodeExe
            Port = [int](Get-Opt $frontend 'port' 0)
            WorkDir = $frontendWork
            WrapperConfig = Join-Path $frontendWork 'vite.stack.config.mjs'
            CacheDir = Join-Path $frontendWork 'vite-cache'
            Env = ConvertTo-EnvTable (Get-Opt $frontend 'env') 'frontend.env'
        }
        Init = @{
            Dbnums = ConvertTo-IntArray (Get-Opt $init 'dbnums') '[init] dbnums'
            Mode = $initMode
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
            FrontendSec = [int](Get-Opt $timeouts 'frontend_sec' 90)
            InitSec = [int](Get-Opt $timeouts 'init_sec' 600)
        }
    }
    return $cfg
}

function Get-ServiceNames($Cfg) {
    $names = @('database', 'backend')
    if ($Cfg.Frontend.Enabled) { $names += 'frontend' }
    return ,$names
}

function Get-ServiceTitle([string]$Name) {
    if ($Name -eq 'database') { return '数据库' }
    if ($Name -eq 'backend') { return '后端' }
    if ($Name -eq 'frontend') { return '前端' }
    return $Name
}

function Get-ServicePort($Cfg, [string]$Name) {
    if ($Name -eq 'database') { return $Cfg.Database.Port }
    if ($Name -eq 'backend') { return $Cfg.Backend.Port }
    return $Cfg.Frontend.Port
}

function Get-ServiceTimeout($Cfg, [string]$Name) {
    if ($Name -eq 'database') { return $Cfg.Timeouts.DatabaseSec }
    if ($Name -eq 'backend') { return $Cfg.Timeouts.BackendSec }
    return $Cfg.Timeouts.FrontendSec
}

function Get-BackendBindHost($Cfg) {
    if ($Cfg.LoopbackOnly) { return '127.0.0.1' }
    return '0.0.0.0'
}

function Get-BackendUrl($Cfg) { return "http://127.0.0.1:$($Cfg.Backend.Port)" }
function Get-FrontendUrl($Cfg) { return "http://127.0.0.1:$($Cfg.Frontend.Port)" }

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

function Get-ChildProcs([int]$ProcessId) {
    return @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$ProcessId" -ErrorAction SilentlyContinue)
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
function Get-ListenTable([switch]$Fresh) {
    if (-not $Fresh -and $null -ne $script:ListenCache -and ((Get-Date) - $script:ListenCacheAt).TotalMilliseconds -lt 800) { return $script:ListenCache }
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

function Get-PortOwners([int]$Port, [switch]$Fresh) {
    $table = Get-ListenTable -Fresh:$Fresh
    if ($table.ContainsKey($Port)) { return ,@($table[$Port]) }
    return ,@()
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
        }
    }
    return $state
}

function Save-StackState($Cfg, $State) {
    New-Dir $Cfg.RunDir
    $services = [ordered]@{}
    foreach ($name in 'database', 'backend', 'frontend') {
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
    if (Test-Path -LiteralPath $Cfg.StateFile) { Remove-Item -LiteralPath $Cfg.StateFile -Force }
    Move-Item -LiteralPath $tmp -Destination $Cfg.StateFile
}

# 这条记录里的进程还在不在、是不是本套自己起的那一个。返回 @{ Alive; Ours; Why; Proc }
function Test-RecordedProcess($Cfg, [string]$Name, $Record) {
    $out = @{ Alive = $false; Ours = $false; Why = ''; Proc = $null }
    if ($null -eq $Record -or $Record.pid -le 0) { $out.Why = '没有记录'; return $out }
    $proc = Get-ProcInfo $Record.pid
    if ($null -eq $proc) { $out.Why = "进程 $($Record.pid) 已经不在了"; return $out }
    $out.Proc = $proc
    if (-not (Test-SameStart $Record.startTicks $proc)) {
        $out.Why = "本套起的那个进程（$($Record.pid)）已经不在了；这个进程号现在是另一个进程（$($proc.Name)，起于 $(Format-ProcClock $proc)）"
        return $out
    }
    $out.Alive = $true
    if ($Name -eq 'frontend') {
        if (-not $Record.marker -or "$($proc.CommandLine)".IndexOf($Record.marker, [System.StringComparison]::OrdinalIgnoreCase) -lt 0) {
            $out.Why = "进程 $($Record.pid) 的命令行里没有本套的标记，不认它是本套的"
            return $out
        }
    } else {
        if (-not $proc.ExecutablePath -or ($proc.ExecutablePath -ine $Record.exe)) {
            $out.Why = "进程 $($Record.pid) 的程序路径是 $($proc.ExecutablePath)，不是本套记录的 $($Record.exe)，不认它是本套的"
            return $out
        }
    }
    foreach ($port in $Cfg.Guard.ProtectPorts) {
        if ((Get-PortOwners $port) -contains [int]$Record.pid) {
            $out.Why = "进程 $($Record.pid) 占着受保护的端口 $port，不认它是本套的"
            return $out
        }
    }
    $out.Ours = $true
    return $out
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
        gen_model = ($Cfg.Init.Mode -eq 'data+model')
        gen_mesh = ($Cfg.Init.Mode -eq 'data+model')
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
        $lines.Insert(0, "# 由 stack.ps1 从模板生成（$((Get-Date).ToString('yyyy-MM-dd HH:mm:ss'))）。入口改过的键：$(@($keys.Keys) -join '、')。其余照模板。")
        [System.IO.File]::WriteAllText($configFile, ($lines -join $newline), $script:Utf8NoBom)
        Say "  后端配置已从模板生成：$configFile（入口改了 $(@($keys.Keys).Count) 个键，其余照模板）"
    }
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
}

function Initialize-FrontendFiles($Cfg) {
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
    $out = @{ Ok = $false; Text = '' }
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
            try { $process.Kill() } catch { }
            $out.Text = "等了 $TimeoutSec 秒没有结束"
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
    $url = (Get-FrontendUrl $Cfg) + '/'
    $out.Checked = "GET $url 要回前端首页，并且经它转发的 /api/v1/health 来自本套的后端"
    $r = Invoke-StackHttp 'GET' $url '' 8
    if (-not $r.Ok) { $out.Reason = "没有应答（$($r.Error)）"; return $out }
    if ($r.Body -notmatch 'id="app"') { $out.Reason = '应答不是前端首页（里面没有 id="app"）'; return $out }
    $proxied = Invoke-StackHttp 'GET' ((Get-FrontendUrl $Cfg) + '/gm/api/v1/health') '' 8
    if ($proxied.Status -eq 0) { $out.Reason = "首页有了，但经前端转发的后端健康检查没通（$($proxied.Error)）"; return $out }
    $pj = ConvertFrom-JsonSafe $proxied.Body
    if ($null -eq $pj) { $out.Reason = "首页有了，但经前端转发的后端健康检查没有拿到正常应答（HTTP $($proxied.Status)）。多半是后端没在跑"; return $out }
    $bind = [string](Get-Prop (Get-Prop $pj 'http') 'bind_addr')
    if ([string](Get-Prop $pj 'mode') -ne 'setup' -and -not $bind.EndsWith(':' + $Cfg.Backend.Port)) {
        $out.Reason = "前端把接口请求转给了别的后端（那一个监听「$bind」），不是本套的 $($Cfg.Backend.Port)"
        return $out
    }
    $out.Ok = $true
    $version = ''
    try { $version = [string]((ConvertFrom-JsonSafe ([System.IO.File]::ReadAllText((Join-Path $Cfg.Frontend.Root 'package.json'), [System.Text.Encoding]::UTF8))).version) } catch { }
    $out.Version = "plant3d-web $version（开发服务器）"
    $out.Detail = '接口请求转给本套的后端'
    return $out
}

# 一个服务此刻的状态。State：ok 正常 / stopped 没起 / bad 异常
function Get-ServiceState($Cfg, $State, [string]$Name) {
    $port = Get-ServicePort $Cfg $Name
    $info = @{ Name = $Name; Title = (Get-ServiceTitle $Name); State = 'stopped'; Reason = ''; Pid = 0; Port = $port; Version = ''; Start = ''; MemGb = 0; Log = ''; Detail = ''; Checked = ''; Next = ''; Ours = $false }
    $record = $null
    if ($State.ContainsKey($Name)) { $record = $State[$Name] }
    $owners = Get-PortOwners $port
    if ($null -eq $record) {
        if ($owners.Count -gt 0) {
            $info.State = 'bad'
            $info.Checked = "本套有没有起它的记录；端口 $port 上有没有进程在听"
            $info.Reason = "本套没起它，但端口 $port 被别的进程占着：$(Format-ProcBrief $owners[0])"
            $info.Next = "那个进程不是本套起的，入口不会动它。停掉它，或把配置里 [$Name] 的 port 换一个，再运行 stack up"
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
        if ($owners.Count -gt 0 -and $owners -notcontains [int]$record.pid) {
            $info.Reason += "；端口 $port 现在被 $(Format-ProcBrief $owners[0]) 占着"
            $info.Next = "端口 $port 现在被别的进程占着，入口不会动它。先停掉那个进程（或换端口），再运行 stack up"
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
    Say ((Format-Cell '服务' 8) + (Format-Cell '状态' 6) + (Format-Cell '端口' 7) + (Format-Cell '进程号' 8) + (Format-Cell '起于' 10) + (Format-Cell '内存' 9) + '版本 / 说明')
    foreach ($row in $Rows) {
        $stateText = '没起'
        $color = 'DarkGray'
        if ($row.State -eq 'ok') { $stateText = '正常'; $color = 'Green' }
        elseif ($row.State -eq 'bad') { $stateText = '异常'; $color = 'Red' }
        $pidText = '-'
        if ($row.Pid -gt 0) { $pidText = "$($row.Pid)" }
        $memText = '-'
        if ($row.MemGb -gt 0) { $memText = "$($row.MemGb) GB" }
        $startText = '-'
        if ($row.Start) { $startText = $row.Start }
        $tail = $row.Version
        if ($row.State -eq 'ok' -and $row.Detail) { $tail = "$tail；$($row.Detail)" }
        if ($row.State -ne 'ok' -and $row.Reason) { $tail = $row.Reason }
        Say ((Format-Cell $row.Title 8) + (Format-Cell $stateText 6) + (Format-Cell "$($row.Port)" 7) + (Format-Cell $pidText 8) + (Format-Cell $startText 10) + (Format-Cell $memText 9) + $tail) $color
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
    # 前端
    if ($Cfg.Frontend.Enabled) {
        if (-not $Cfg.Frontend.Node -or -not (Test-Path -LiteralPath $Cfg.Frontend.Node -PathType Leaf)) { Add-Problem '找不到 node' '配置 [frontend] node（缺省是从 PATH 里找 node）' '没有找到 node.exe' '装 Node.js 20 以上，或在 [frontend] node 里写明 node.exe 的完整路径' }
        if (-not (Test-Path -LiteralPath (Join-Path $Cfg.Frontend.Root 'vite.config.ts'))) { Add-Problem '前端目录不对' '配置 [frontend] root 下面有没有 vite.config.ts' "没有：$($Cfg.Frontend.Root)" '改成前端仓库的实际位置' }
        elseif (-not (Test-Path -LiteralPath (Join-Path $Cfg.Frontend.Root 'node_modules\vite\bin\vite.js'))) { Add-Problem '前端的依赖没装' '前端目录下有没有 node_modules\vite' "没有：$(Join-Path $Cfg.Frontend.Root 'node_modules\vite')" '在前端目录里装好依赖再来（入口不替人装依赖）' }
    }
    # 拼进命令行的路径
    $cmdTexts = @($Cfg.Database.Exe, $Cfg.Database.DataDir, $Cfg.Backend.Exe, $Cfg.Root)
    if ($Cfg.Frontend.Enabled) { $cmdTexts += @($Cfg.Frontend.Node, $Cfg.Frontend.Root) }
    foreach ($t in $cmdTexts) { if ($t -and -not (Test-CmdSafe $t)) { Add-Problem '路径里有入口处理不了的字符' '路径里有没有双引号或百分号' "有：$t" '换一个不带这两种字符的目录' } }

    # 端口
    $seen = @{}
    foreach ($name in $names) {
        $port = Get-ServicePort $Cfg $name
        $title = Get-ServiceTitle $name
        if ($port -lt 1 -or $port -gt 65535) { Add-Problem "$title 的端口不合法" "配置 [$name] port" "$port 不在 1–65535 之间" "改配置里 [$name] 的 port"; continue }
        if ($seen.ContainsKey($port)) { Add-Problem "$title 和 $($seen[$port]) 用了同一个端口" '各服务的端口有没有重复' "都是 $port" '给它们各配一个端口' } else { $seen[$port] = $title }
        if ($Cfg.Guard.ProtectPorts -contains $port) { Add-Problem "$title 的端口在受保护名单里" "配置 [$name] port 是不是在 [guard] protect_ports 里" "$port 在名单里，本套不许用" "给 [$name] 换一个端口"; continue }
        $owners = Get-PortOwners $port -Fresh
        if ($owners.Count -eq 0) { continue }
        $ours = $false
        if ($State.ContainsKey($name)) {
            $check = Test-RecordedProcess $Cfg $name $State[$name]
            if ($check.Ours -and $owners -contains [int]$State[$name].pid) { $ours = $true }
        }
        if ($ours) { Add-Note "$title 已经在跑（进程 $($State[$name].pid)，端口 $port）" }
        else { Add-Problem "$title 要用的端口 $port 被占着" "本机有没有别的进程在听 $port" "$(Format-ProcBrief $owners[0])在听。它不是本套起的，我没有动它" "停掉那个进程，或把配置里 [$name] 的 port 换一个，然后再运行 stack up" }
    }

    # 程序版本。问版本要执行程序；后端一启动就会在当前目录下建 logs\，所以工作目录放到本套的 run\ 里。
    $probeDir = ''
    if (Test-Path -LiteralPath $Cfg.Root -PathType Container) { New-Dir $Cfg.RunDir; $probeDir = $Cfg.RunDir }
    if ($probeDir -and (Test-Path -LiteralPath $Cfg.Database.Exe -PathType Leaf)) {
        $v = Invoke-Capture $Cfg $Cfg.Database.Exe 'version' $probeDir
        if ($v.Text -notmatch '(?<!\d)2\.1\.\d+') { Add-Problem '数据库程序的版本不对' "$($Cfg.Database.Exe) version 的输出里有没有 2.1.x" "输出是「$(Limit-Text $v.Text 100)」" '换成本系统自带的那一份（自家分支的 2.1.4）。装成 3.x 会要求做不可逆的数据迁移，不要照做' }
        else { Add-Note "数据库程序：$(Limit-Text $v.Text 100)" }
    }
    if ($probeDir -and (Test-Path -LiteralPath $Cfg.Backend.Exe -PathType Leaf)) {
        $v = Invoke-Capture $Cfg $Cfg.Backend.Exe 'version' $probeDir
        if (-not $v.Ok) { Add-Problem '后端程序跑不起来' "$($Cfg.Backend.Exe) version 能不能正常结束" "不能：$(Limit-Text $v.Text 160)" '确认这是 64 位 Windows 上能运行的那一份后端程序' }
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
    if (-not $hasEnvironment -and $Cfg.Frontend.Enabled) { Add-Problem '这台机器的 .NET 太老' '.NET 是不是 4.6 以上（传给服务的环境变量名要保留大小写）' '不是：变量名会被变成小写，前端开发服务器认不出 VITE_ 开头的变量' '装 .NET Framework 4.6 以上（或 PowerShell 5.1），或者把 [frontend] enabled 设成 false' }

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
    if ($tail.Count -gt 0) {
        Say "  它输出的最后 $($tail.Count) 行（$Log）："
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
    $log = [string](Get-Prop $spawn 'log')

    # 在 cmd 那一层下面找到真正的服务进程
    $found = $null
    $deadline = (Get-Date).AddSeconds(15)
    while ($null -eq $found -and (Get-Date) -lt $deadline) {
        foreach ($kid in (Get-ChildProcs $wrapperPid)) {
            if ($kid.Name -ieq 'conhost.exe') { continue }
            if ($Name -eq 'frontend') {
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
        wrapperTicks = [string](Get-Prop $spawn 'wrapperTicks')
        log = $log
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
    while ($true) {
        $check = Test-RecordedProcess $Cfg $Name $record
        if (-not $check.Ours) {
            Show-Failure $Cfg $Name $Index $Total "进程已经退出（$($check.Why)）" $record.log '看上面它自己报的原因；改好之后再运行 stack up'
            Stop-Stack "$title 起来之后又退出了" 4
        }
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

function Invoke-Init($Cfg, $State) {
    if ($Cfg.Init.Dbnums.Count -eq 0) { return }
    if ($Cfg.Guard.ProtectPorts -contains $Cfg.Backend.Port) { Stop-Stack '后端端口在受保护名单里，不向它发初始化请求' 2 }
    $health = Test-ServiceHealth $Cfg 'backend'
    if ($null -ne $health.Json -and [string](Get-Prop $health.Json 'mode') -eq 'setup') {
        Say '后端在设置模式（配置还不完整），先不初始化库。在向导页里填完之后再运行 stack up。' 'Yellow'
        return
    }
    $list = $Cfg.Init.Dbnums -join '、'
    $total = (Get-ServiceNames $Cfg).Count
    Say "初始化库 $list（$($Cfg.Init.Mode)）"
    $rows = Get-DbnumRows $Cfg
    if ($null -eq $rows) { Stop-Stack '后端的库列表（GET /api/v1/dbnums）读不出来，没法初始化。运行 stack status 看后端是否正常' 4 }
    $need = @()
    foreach ($db in $Cfg.Init.Dbnums) {
        $row = Get-DbnumRow $rows $db
        if ($null -eq $row) { Stop-Stack "库 $db 不在后端的库列表里：这个 MDB 里没有它，或者它的库文件不在。请核对配置 [init] dbnums 和 [project] mdb" 4 }
        if ((Get-Prop $row 'initialized') -ne $true) { $need += $db }
    }
    if ($need.Count -eq 0) {
        Say '  都已经初始化过了，不重复做。'
    } else {
        $body = '{"project":"' + $Cfg.Project.Name + '","dbnums":[' + ($need -join ',') + ']}'
        $r = Invoke-StackHttp 'POST' ((Get-BackendUrl $Cfg) + '/api/v1/update/execute') $body 120
        if (-not $r.Ok) { Stop-Stack "后端没有接受初始化请求（$($r.Error)）：$(Limit-Text $r.Body 300)" 4 }
        $j = ConvertFrom-JsonSafe $r.Body
        $enqueued = @(Get-Prop $j 'enqueued' | Where-Object { $null -ne $_ }).Count
        Say "  已请求初始化库 $($need -join '、')：入队 $enqueued 个批次（系统库和字典库按后端的设计一并处理）"
        foreach ($b in @(Get-Prop $j 'blocked')) { if ($null -ne $b) { Say "  ✗ 库 $(Get-Prop $b 'dbnum') 被后端挡下：$(Limit-Text ([string](Get-Prop $b 'reason')) 200)" 'Red' } }
        foreach ($w in @(Get-Prop $j 'warnings')) { if ($w) { Say "  后端提示：$(Limit-Text ([string]$w) 200)" 'Yellow' } }
    }
    $started = Get-Date
    $deadline = $started.AddSeconds($Cfg.Timeouts.InitSec)
    $lastNote = Get-Date
    while ($true) {
        $rows = Get-DbnumRows $Cfg
        $done = ($null -ne $rows)
        if ($done) {
            foreach ($db in $Cfg.Init.Dbnums) {
                $row = Get-DbnumRow $rows $db
                if ($null -eq $row -or (Get-Prop $row 'initialized') -ne $true) { $done = $false }
            }
        }
        if ($done -and (Test-BackendIdle $Cfg)) {
            Say "  ✓ 初始化完成（$([int]((Get-Date) - $started).TotalSeconds) 秒）" 'Green'
            return
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
    foreach ($name in $names) {
        $index += 1
        $title = Get-ServiceTitle $name
        $port = Get-ServicePort $Cfg $name
        Say "[$index/$total] $title（端口 $port）"
        $info = Get-ServiceState $Cfg $state $name
        if ($info.State -eq 'ok') {
            Say "  已经在跑（进程 $($info.Pid)），不动它" 'Green'
            continue
        }
        if ($info.Ours) {
            Say "  进程 $($info.Pid) 在，但检查还没通过（$($info.Reason)）。等它…"
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
            Show-Failure $Cfg $name $index $total "端口 $port 被占着：$(Format-ProcBrief $owners[0])。它不是本套起的，我没有动它" '' "停掉那个进程，或把配置里 [$name] 的 port 换一个，然后再运行 stack up"
            Stop-Stack "$title 的端口被占" 4
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
        if ($name -eq 'frontend') { Initialize-FrontendFiles $Cfg }
        Start-StackService $Cfg $state $name $index $total
        Wait-ServiceReady $Cfg $state $name $index $total
    }

    if (-not $NoInit) { Invoke-Init $Cfg $state }

    Say ''
    $rows = @()
    foreach ($name in $names) { $rows += (Get-ServiceState $Cfg $state $name) }
    Show-StatusTable $rows
    Say ''
    if ($Cfg.Frontend.Enabled) { Say "界面 $(Get-FrontendUrl $Cfg)/    后端自带的界面和配置向导 $(Get-BackendUrl $Cfg)/    运维页 $(Get-BackendUrl $Cfg)/ops.html" }
    else { Say "后端自带的界面和配置向导 $(Get-BackendUrl $Cfg)/    运维页 $(Get-BackendUrl $Cfg)/ops.html" }
    Say '全部起好。看状态：stack status；检查：stack smoke；停：stack down。' 'Green'
}

# ─────────────────────────── 命令：down ───────────────────────────

function Invoke-Down($Cfg, [bool]$OnlyTell) {
    $state = Read-StackState $Cfg
    $stopped = 0
    $failed = 0
    if ($OnlyTell) { Say "stack down -WhatIf —— 只说不做（$($Cfg.Name)）" } else { Say "stack down —— $($Cfg.Name)" }
    foreach ($name in 'frontend', 'backend', 'database') {
        $title = Get-ServiceTitle $name
        if (-not $state.ContainsKey($name)) { Say "  $title：本套没有起它的记录，不用停"; continue }
        $record = $state[$name]
        $check = Test-RecordedProcess $Cfg $name $record
        if (-not $check.Ours) {
            Say "  $title：$($check.Why)。不停任何进程。" 'Yellow'
            if (-not $OnlyTell) { $state.Remove($name); Save-StackState $Cfg $state }
            continue
        }
        $what = "$title：进程 $($record.pid)（$($check.Proc.Name)，起于 $(Format-ProcClock $check.Proc)，端口 $($record.port)"
        if ($name -eq 'frontend') { $what += "，命令行带本套的标记 $($record.marker)）" } else { $what += "，程序 $($record.exe)）" }
        if ($OnlyTell) { Say "  会停 $what"; continue }
        # 前端的开发服务器带一个打包用的子进程；先记下来，主进程停了之后核对它们是否跟着退了。
        $kids = @()
        foreach ($kid in (Get-ChildProcs $record.pid)) { $kids += @{ pid = [int]$kid.ProcessId; ticks = (Get-ProcTicks $kid); name = $kid.Name } }
        if (-not (Stop-VerifiedProcess $record.pid $record.startTicks)) {
            Say "  ✗ $title：进程 $($record.pid) 没有停下来" 'Red'
            $failed += 1
            continue
        }
        foreach ($kid in $kids) {
            if ($kid.name -ieq 'conhost.exe') { continue }
            Start-Sleep -Milliseconds 300
            $p = Get-ProcInfo $kid.pid
            if ($null -ne $p -and (Test-SameStart $kid.ticks $p)) {
                if (Stop-VerifiedProcess $kid.pid $kid.ticks) { Say "    连带停了它的子进程 $($kid.pid)（$($kid.name)）" }
            }
        }
        # 外面包的那层 cmd 会在服务退出后自己结束；没结束才按「进程号 + 启动时间」核对后收掉。
        if ($record.wrapperPid -gt 0) {
            Start-Sleep -Milliseconds 300
            $w = Get-ProcInfo $record.wrapperPid
            if ($null -ne $w -and $w.Name -ieq 'cmd.exe') { Stop-VerifiedProcess $record.wrapperPid $record.wrapperTicks | Out-Null }
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
    foreach ($name in (Get-ServiceNames $Cfg)) {
        $port = Get-ServicePort $Cfg $name
        $owners = Get-PortOwners $port -Fresh
        if ($owners.Count -gt 0) { $left += "$port（$(Format-ProcBrief $owners[0])）" }
    }
    if ($left.Count -gt 0) { Say "  注意：这些端口上还有进程在听，不在本套的记录里，没有动：$($left -join '；')" 'Yellow' }
    Say "停了 $stopped 个。"
    if ($failed -gt 0) { return 1 }
    return 0
}

# ─────────────────────────── 命令：status / doctor / smoke / watch / config ───────────────────────────

function Invoke-Status($Cfg) {
    $state = Read-StackState $Cfg
    $rows = @()
    foreach ($name in (Get-ServiceNames $Cfg)) { $rows += (Get-ServiceState $Cfg $state $name) }
    Say "stack status —— $($Cfg.Name)（$((Get-Date).ToString('HH:mm:ss'))；可提交内存余量 $(Get-CommitFreeGb) GB）"
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
    $pre = Invoke-Preflight $Cfg $state
    Say ''
    if ($pre.Problems.Count -gt 0) { Say "✗ $($pre.Problems.Count) 处要处理。" 'Red'; return 4 }
    Say '✓ 可以运行 stack up。' 'Green'
    return 0
}

function Invoke-Smoke($Cfg, [int]$MaxLevel) {
    $state = Read-StackState $Cfg
    $results = New-Object System.Collections.ArrayList
    function Add-Result([int]$Lv, [string]$What, [bool]$Ok, [string]$Detail) { [void]$results.Add(@{ Level = $Lv; What = $What; Ok = $Ok; Detail = $Detail }) }
    Say "stack smoke —— $($Cfg.Name)（做到第 $MaxLevel 层）"
    if ($Cfg.Guard.ProtectPorts -contains $Cfg.Backend.Port) { Stop-Stack '后端端口在受保护名单里，不对它做冒烟' 2 }

    # 第 0 层：三个服务在跑并通过检查
    $allUp = $true
    foreach ($name in (Get-ServiceNames $Cfg)) {
        $info = Get-ServiceState $Cfg $state $name
        $detail = $info.Version
        if ($info.Detail) { $detail = "$detail；$($info.Detail)" }
        if ($info.State -ne 'ok') { $detail = $info.Reason; if (-not $detail) { $detail = '没起' }; $allUp = $false }
        Add-Result 0 "$($info.Title)在跑并通过检查" ($info.State -eq 'ok') $detail
    }
    $setupMode = $false
    if ($allUp) {
        $health = Test-ServiceHealth $Cfg 'backend'
        $j = $health.Json
        if ([string](Get-Prop $j 'mode') -eq 'setup') {
            $setupMode = $true
            Add-Result 0 '后端已经配置好' $false "后端在设置模式：打开 $(Get-BackendUrl $Cfg)/ 按向导填完配置"
        } else {
            $status = [string](Get-Prop $j 'status')
            Add-Result 0 '后端自报健康' ($status -eq 'ok') "status=$status；降级项 [$(@(Get-Prop $j 'degraded_sections') -join ',')]；阻断项 [$(@(Get-Prop $j 'blocking_conditions') -join ',')]"
            $closure = [string](Get-Prop (Get-Prop $j 'mdb_closure') 'status')
            Add-Result 0 '后端读得到 E3D 工程目录' ($closure -eq 'ok') "工程目录检查（mdb_closure.status）=$closure；项目 $(Get-Prop $j 'project')，MDB $(Get-Prop $j 'mdb')"
        }
    }

    # 第 1 层：模型树有根
    $nodes = @()
    if ($allUp -and -not $setupMode -and $MaxLevel -ge 1) {
        $r = Invoke-StackHttp 'GET' ((Get-BackendUrl $Cfg) + '/api/v1/tree/roots') '' 120
        $nodes = @(Get-Prop (ConvertFrom-JsonSafe $r.Body) 'nodes' | Where-Object { $null -ne $_ })
        $detail = "根 $($nodes.Count) 个"
        if (-not $r.Ok) { $detail = "请求失败（$($r.Error)）：$(Limit-Text $r.Body 160)" }
        Add-Result 1 '模型树有根' ($r.Ok -and $nodes.Count -gt 0) $detail
        if ($Cfg.Init.Dbnums.Count -gt 0) {
            $rows = Get-DbnumRows $Cfg
            foreach ($db in $Cfg.Init.Dbnums) {
                $row = Get-DbnumRow $rows $db
                $ok = ($null -ne $row -and (Get-Prop $row 'initialized') -eq $true)
                $detail = '后端的库列表里没有它'
                if ($null -ne $row) { $detail = "initialized=$(Get-Prop $row 'initialized')；数据到会话 $(Get-Prop $row 'applied_sesno')，文件最新会话 $(Get-Prop $row 'file_latest_sesno')；模型判定 $(Get-Prop $row 'model_verdict')" }
                Add-Result 1 "点名的库 $db 已初始化" $ok $detail
            }
        }
    }

    # 第 2 层：照界面点开一个节点那样取一次模型——取到记录，再取到其中一个网格
    if ($allUp -and -not $setupMode -and $MaxLevel -ge 2) {
        $candidates = @($nodes)
        if ($Cfg.Init.Dbnums.Count -gt 0) { $candidates = @($nodes | Where-Object { $Cfg.Init.Dbnums -contains [int](Get-Prop $_ 'dbnum') }) }
        if ($candidates.Count -eq 0) {
            Add-Result 2 '取到一个模型（记录 + 网格）' $false '模型树的根里没有属于点名的库的（[init] dbnums），没有可取的对象'
        } else {
            $got = $false
            $detail = ''
            foreach ($node in ($candidates | Select-Object -First 8)) {
                $refno = ([string](Get-Prop $node 'refno')).Replace('_', '/')
                $label = "根 $refno（$(Get-Prop $node 'name')，库 $(Get-Prop $node 'dbnum')）"
                $body = '{"generation_root":"' + $refno + '","limit":500}'
                $r = Invoke-StackHttp 'POST' ((Get-BackendUrl $Cfg) + '/api/v1/model/records') $body 120
                if (-not $r.Ok) { $detail = "$label 取记录失败（$($r.Error)）：$(Limit-Text $r.Body 160)"; continue }
                $j = ConvertFrom-JsonSafe $r.Body
                $hash = ''
                foreach ($item in @(Get-Prop $j 'items')) {
                    foreach ($inst in @(Get-Prop $item 'insts')) {
                        $h = [string](Get-Prop $inst 'geo_hash')
                        if ($h.Length -gt 3) { $hash = $h; break }
                    }
                    if ($hash) { break }
                }
                if (-not $hash) { $detail = "$label 有 $(Get-Prop $j 'total') 条记录，但前 500 条里没有带网格的"; continue }
                $mesh = Invoke-StackHttp 'GET' ((Get-BackendUrl $Cfg) + "/api/v1/meshes/$hash.mesh") '' 60
                if ($mesh.Ok -and $mesh.Bytes -gt 0) {
                    $got = $true
                    $detail = "$label：记录 $(Get-Prop $j 'total') 条（来源 $(Get-Prop $j 'source')，complete=$(Get-Prop $j 'complete')）；网格 $hash 取到 $($mesh.Bytes) 字节"
                    break
                }
                $detail = "$label 的网格 $hash 没取到（$($mesh.Error)）"
            }
            Add-Result 2 '取到一个模型（记录 + 网格）' $got $detail
        }
    }

    $failed = 0
    foreach ($r in $results) {
        $mark = '✓'
        $color = 'Green'
        if (-not $r.Ok) { $mark = '✗'; $color = 'Red'; $failed += 1 }
        Say ("  第 $($r.Level) 层  $mark " + (Format-Cell $r.What 30) + $r.Detail) $color
    }
    Say ''
    if ($failed -gt 0) {
        Say "冒烟没过：$($results.Count) 项里 $failed 项不通过。下一步：先运行 stack status 看服务是否都正常；服务正常而模型取不到，多半是库还没初始化完，稍后再试。" 'Red'
        return 5
    }
    Say "冒烟通过：$($results.Count) 项都过了。" 'Green'
    return 0
}

function Invoke-Watch($Cfg, [int]$WatchSeconds) {
    Say "stack watch —— 盯 $WatchSeconds 秒（$($Cfg.Name)）"
    $deadline = (Get-Date).AddSeconds($WatchSeconds)
    $peak = 0
    while ($true) {
        $state = Read-StackState $Cfg
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

# 只生成各服务的配置文件（和 up 里生成的一模一样），不起任何服务。给想先看一眼的人用。
function Invoke-Prepare($Cfg) {
    Say "stack prepare —— 只生成配置文件，不起服务（$($Cfg.Name)）"
    Initialize-BackendFiles $Cfg
    Say "  后端配置：$($Cfg.Backend.ConfigFile)"
    Say "  数据库函数脚本：$(Join-Path $Cfg.Backend.WorkDir 'resource\surreal')"
    if ($Cfg.Frontend.Enabled) {
        Initialize-FrontendFiles $Cfg
        Say "  前端的包装配置：$($Cfg.Frontend.WrapperConfig)"
    }
    Say '生成好了。后端配置只在不存在时生成；已经在的不覆盖。'
    return 0
}

# 只把配置解析出来给人看：不查端口、不碰进程、不执行任何程序。
function Invoke-ShowConfig($Cfg) {
    Say "stack config —— 解析结果（$($Cfg.Name)）。只读配置，不查端口、不碰进程。"
    Say "  配置文件：$($Cfg.ConfigPath)"
    Say "  运行目录：$($Cfg.Root)"
    Say "  项目：$($Cfg.Project.Name)（编码 $($Cfg.Project.Code)，MDB $($Cfg.Project.Mdb)，$($Cfg.Project.E3dVersion)），工程根目录 $($Cfg.Project.E3dRoot)"
    $listenText = '维持现状：后端和前端监听所有网卡'
    if ($Cfg.LoopbackOnly) { $listenText = '只许本机访问：后端和前端都只听 127.0.0.1' }
    Say "  对谁开放：$listenText；数据库听 $($Cfg.Database.Host)"
    $initText = '不点名初始化任何库'
    if ($Cfg.Init.Dbnums.Count -gt 0) { $initText = "点名初始化库 $($Cfg.Init.Dbnums -join '、')（$($Cfg.Init.Mode)）" }
    Say "  初始化：$initText；启动时自动初始化新库 = $($Cfg.Init.AutoNew)"
    Say "  护栏：开工线 $($Cfg.Guard.MinCommitFreeGb) GB，收摊线 $($Cfg.Guard.AbortCommitFreeGb) GB，后端上限 $($Cfg.Guard.MaxBackendGb) GB（0 = 不设）；越线自动收摊 = $($Cfg.Guard.AutoDown)；受保护端口 [$($Cfg.Guard.ProtectPorts -join ', ')]"
    $keys = Get-BackendConfigKeys $Cfg
    Say ''
    Say "  后端配置 $($Cfg.Backend.ConfigFile)"
    Say "    模板：$($Cfg.Backend.ConfigTemplate)（文件不存在时从它生成一次，以后不覆盖）"
    Say '    入口写进去的键：'
    foreach ($key in $keys.Keys) { Say "      $key = $(Format-TomlLiteral $keys[$key] $false)" }
    Say '    （写进文件时保持模板里原来的写法：模板里某个数字带着引号，就继续带引号。）'
    Say '    账号口令两个键（v_user / v_password）照模板，入口不改也不显示。'
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
        foreach ($key in $spec.SetEnv.Keys) { Say "      $key = $($spec.SetEnv[$key])" }
        foreach ($key in $spec.Late.Keys) { Say "      $key = （$($spec.Late[$key])）" }
        Say "    环境变量——照搬本机系统的（只这些名字，值不显示）：$($spec.InheritedNames -join '、')"
        Say '    除此之外不带任何环境变量。'
    }
    return 0
}

function Show-Help {
    $text = @"
stack $script:StackVersion —— 读一份配置，把数据库、后端、前端按顺序起起来，起一个查一个

用法
  stack <命令> [-Config <配置文件>]
      不给 -Config 时用当前目录下的 stack.toml。
  不经 stack.cmd、直接跑脚本：
      powershell -NoProfile -ExecutionPolicy Bypass -File stack.ps1 <命令> -Config <配置文件>
  双击：stack-up.cmd 起、stack-down.cmd 停（用它们旁边的 stack.toml；也可以把一份配置文件拖到它们上面）。

命令
  up        预检 → 生成各服务的配置 → 按 数据库、后端、前端 的顺序起没起的，起一个查一个
            → 初始化配置里点名的库。已经在跑的不动；再跑一遍不会多起进程。
              -NoInit      只起服务，不初始化库
  down      倒着停：前端、后端、数据库。只停本套自己起的。
              -WhatIf      只列出会停哪些进程，不真停（也可以写 -DryRun）
  status    一张表：每个服务的状态、端口、进程号、起的时间、内存、版本；不正常的说出原因
  smoke     冒烟。第 0 层 三个服务在跑并通过检查；第 1 层 模型树有根；第 2 层 取到一个模型
              -Level n     只做到第 n 层（缺省 2）
  doctor    只检查、不起任何服务：路径、端口、程序版本、内存和磁盘余量
  watch     盯一段时间：每 5 秒报一次各服务的内存；越过配置的线就按配置处理
              -Seconds n   盯多久（缺省 120）
  config    把配置解析出来给人看：每个服务用什么程序、参数、工作目录、环境变量。
            不查端口、不碰进程
  prepare   只生成各服务的配置文件（和 up 里生成的一样），不起服务。想先看一眼再起时用
  help      这一页

配置
  从 stack.example.toml 拷一份改名 stack.toml。必填的是 [project] 的四项（E3D 工程根目录、项目名、
  项目编码、E3D 版本）和各服务的程序位置、端口；其余都有缺省值。
  配置里有不认识的写法或不认识的键，会报错并指出行号，不会悄悄忽略。
  相对路径：[stack] root 相对配置文件所在的目录；其余路径都相对 root。

它替人做了什么
  · 后端的配置（DbOption.toml）从模板生成一次，只改入口管的那十几个键，其余照模板；以后不覆盖，
    在向导页里改的东西留得住。它要连的数据库、监听的端口和 stack 的配置不一致时不起。
  · 每个服务的环境变量由入口明确给出（stack config 能看到全部），不带调用方环境里的东西。
  · 数据库账号口令沿用后端配置模板里现有的那一对，经环境变量递给数据库；不打印、不上命令行、
    不写进状态文件和日志。
  · 认进程靠「进程号 + 启动时间 + 程序路径」（前端靠命令行里的标记），从不按进程名；
    状态文件里没有记的进程一律当别人的。所以数据库程序和后端程序要用放在运行目录里的那一份
    （[guard] require_exe_under_root）。
  · 缺省不改对谁开放：后端、前端监听所有网卡。只想让本机访问，把 [listen] loopback_only 设成 true。

文件都在哪（都在 [stack] root 下面）
  run\state.json        记着本套起了哪些进程（没有口令）
  run\stack.lock        up / down 正在跑的时候占着它，防止两个同时跑
  logs\<服务>\          每个服务自己的输出，每起一次一个文件
  logs\stack\           入口自己打印过的内容，按天一个文件
  work\backend\         后端的工作目录：生成的 DbOption.toml、网格、日志、锁
  work\frontend\        前端的包装配置和依赖缓存
  data\surreal\         数据库的数据
  tmp\                  给服务用的临时目录

退出码
  0 正常              2 配置或用法有问题      3 被内存线或让路文件挡下
  4 预检没过、或有服务没起好                  5 冒烟没过              1 其它
"@
    Write-Host $text
}

# ─────────────────────────── 主流程 ───────────────────────────

try {
    $commandName = $Command.ToLowerInvariant()
    if ($commandName -eq 'help' -or $commandName -eq '-h' -or $commandName -eq '--help' -or $commandName -eq '/?' -or $commandName -eq '-?') {
        Show-Help
        exit 0
    }
    if ('up', 'down', 'status', 'smoke', 'doctor', 'watch', 'config', 'prepare', '__spawn' -notcontains $commandName) {
        Write-Host "不认识的命令「$Command」。可用的命令：up、down、status、smoke、doctor、watch、config、prepare、help。" -ForegroundColor Red
        exit 2
    }
    # 不认识的参数不悄悄吞掉。（没写名字的多余参数会被 PowerShell 顺位塞进 -Config、-Service，所以一并查。）
    $extra = @()
    if ($Config -like '-*') { $extra += $Config; $Config = '' }
    if ($commandName -ne '__spawn' -and $Service) { $extra += $Service }
    foreach ($a in $args) { $extra += "$a" }
    if ($extra.Count -gt 0) {
        Write-Host "不认识的参数：$($extra -join ' ')。认识的有 -Config、-WhatIf（-DryRun）、-NoInit、-Level、-Seconds；用法见 stack help。" -ForegroundColor Red
        exit 2
    }
    $configFile = $Config
    if (-not $configFile) { $configFile = Join-Path (Get-Location).Path 'stack.toml' }
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
    elseif ($commandName -eq 'smoke') { $outcome = Invoke-Smoke $cfg $Level }
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
