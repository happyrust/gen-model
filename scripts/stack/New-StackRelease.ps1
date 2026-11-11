<#
出一个离线安装包：把已经编好的各个部件收进一个目录，写版本清单，压成 zip。

这个脚本只做「拷贝 + 清单 + 压缩」：不编译任何东西，不起任何服务，不改任何源目录。
各部件在哪、是哪个提交编的，写在一份说明文件里（-Spec，JSON）；样子见 package\release-spec.example.json。
口令类的东西它不读、不显示：后端的配置模板按字节原样拷贝。

用法（要 PowerShell 7；Windows PowerShell 5.1 也能跑，只是清单的 JSON 排版难看些）：
  pwsh -NoProfile -File New-StackRelease.ps1 -Spec <release-spec.json>
  pwsh -NoProfile -File New-StackRelease.ps1 -Spec <release-spec.json> -NoZip     只摆目录、写清单，不压缩

包里的布局（解压出来就是运行目录，配置里的相对路径都按它写）：
  stack.ps1  stack.cmd  stack.default.toml  stack.example.toml
      包里不带 stack.toml：那是用户自己的配置，第一次「开始」时入口照 stack.default.toml 生成；
      这样换新一版、解压覆盖时不会把用户改过的配置盖掉。
  开始.cmd / start.cmd   停止.cmd / stop.cmd   状态.cmd / status.cmd
  安装和测试说明.md / README.txt   release.json
  bin\        surreal.exe  aios-database.exe  rs-server.exe  VC++ 运行时的 DLL   nginx\nginx.exe  nginx\conf\mime.types
  config\     DbOption.factory.toml（后端配置的模板）
  resource\   后端的 resource     schema\   E3D 库结构定义     web\   plant-ui 网页版     www\   plant3d-web 的构建产物
  rs-server\  resource\  assets\（rs-server 的运行文件；入口起它之前拷进它的运行目录）
  redist\     VC++ 运行时的安装程序（备用）
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Spec,
    [string]$Stage = '',
    [string]$OutDir = '',
    [switch]$NoZip,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$utf8NoBom = New-Object System.Text.UTF8Encoding -ArgumentList $false
$utf8Bom = New-Object System.Text.UTF8Encoding -ArgumentList $true
$ascii = New-Object System.Text.ASCIIEncoding
$stageMarker = '.stack-release-stage'

function Fail([string]$Message) { Write-Host "✗ $Message" -ForegroundColor Red; exit 2 }
function Step([string]$Message) { Write-Host $Message }
function Opt($Object, [string]$Name, $Default = $null) {
    if ($null -ne $Object -and $null -ne $Object.PSObject.Properties[$Name] -and $null -ne $Object.$Name) { return $Object.$Name }
    return $Default
}
function New-Folder([string]$Path) { [void][System.IO.Directory]::CreateDirectory($Path) }

# ───────────── 读说明文件 ─────────────
$specPath = [System.IO.Path]::GetFullPath($Spec)
if (-not (Test-Path -LiteralPath $specPath -PathType Leaf)) { Fail "找不到说明文件：$specPath" }
$s = [System.IO.File]::ReadAllText($specPath, [System.Text.Encoding]::UTF8) | ConvertFrom-Json
$name = [string](Opt $s 'package_name' '')
if ($name -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]+$') { Fail "说明文件里的 package_name 只能用字母、数字、点、横线、下划线，现在是「$name」" }
$src = Opt $s 'sources'
if ($null -eq $src) { Fail '说明文件里没有 sources 这一节' }
$stage = $Stage
if (-not $stage) { $stage = [string](Opt $s 'stage_dir' '') }
if (-not $stage) { Fail '没说包摆在哪：给 -Stage，或在说明文件里写 stage_dir' }
$stage = [System.IO.Path]::GetFullPath($stage)
$outDir = $OutDir
if (-not $outDir) { $outDir = [string](Opt $s 'out_dir' '') }
if (-not $NoZip -and -not $outDir) { Fail '没说 zip 放哪：给 -OutDir，或在说明文件里写 out_dir（只摆目录不压缩就加 -NoZip）' }

function Get-Source([string]$Key, [switch]$Optional) {
    $value = [string](Opt $src $Key '')
    if (-not $value) { if ($Optional) { return '' }; Fail "说明文件的 sources 里缺 $Key" }
    if (-not (Test-Path -LiteralPath $value)) { Fail "sources.$Key 指的东西不存在：$value" }
    return (Resolve-Path -LiteralPath $value).Path
}

# ───────────── 摆包的目录：只动自己建的 ─────────────
if (Test-Path -LiteralPath $stage) {
    if (-not (Test-Path -LiteralPath (Join-Path $stage $stageMarker))) { Fail "$stage 已经在，而且不是这个脚本建的（里面没有 $stageMarker），不动它。换一个目录，或先自己清掉" }
    Remove-Item -LiteralPath $stage -Recurse -Force
}
New-Folder $stage
[System.IO.File]::WriteAllText((Join-Path $stage $stageMarker), "New-StackRelease.ps1 摆包用的目录。$((Get-Date).ToString('yyyy-MM-dd HH:mm:ss'))", $utf8NoBom)
Step "摆包：$stage"

function Copy-One([string]$From, [string]$Rel) {
    if (-not (Test-Path -LiteralPath $From -PathType Leaf)) { Fail "找不到文件：$From" }
    $to = Join-Path $stage $Rel
    New-Folder (Split-Path -Parent $to)
    [System.IO.File]::Copy($From, $to, $true)
}
function Copy-Tree([string]$From, [string]$Rel, [string[]]$Exclude = @()) {
    if (-not (Test-Path -LiteralPath $From -PathType Container)) { Fail "找不到目录：$From" }
    $root = $From.TrimEnd('\')
    $count = 0
    foreach ($file in @(Get-ChildItem -LiteralPath $root -Recurse -File -Force)) {
        $relative = $file.FullName.Substring($root.Length + 1)
        $skip = $false
        foreach ($pattern in $Exclude) { if ($relative -like $pattern) { $skip = $true; break } }
        if ($skip) { continue }
        $to = Join-Path (Join-Path $stage $Rel) $relative
        New-Folder (Split-Path -Parent $to)
        [System.IO.File]::Copy($file.FullName, $to, $true)
        $count += 1
    }
    return $count
}
# 文本文件：统一成 CRLF（老一点的记事本不认只有 LF 的换行）。
function Write-Text([string]$Rel, [string]$Text, $Encoding) {
    $to = Join-Path $stage $Rel
    New-Folder (Split-Path -Parent $to)
    [System.IO.File]::WriteAllText($to, ($Text -replace "`r?`n", "`r`n"), $Encoding)
}
function Read-Text([string]$Path) { return [System.IO.File]::ReadAllText($Path, [System.Text.Encoding]::UTF8) }

# ───────────── 程序 ─────────────
Copy-One (Get-Source 'database_exe') 'bin\surreal.exe'
Copy-One (Get-Source 'backend_exe') 'bin\aios-database.exe'
$rsExe = Get-Source 'rs_server_exe' -Optional
if ($rsExe) { Copy-One $rsExe 'bin\rs-server.exe' }
Copy-One (Get-Source 'nginx_exe') 'bin\nginx\nginx.exe'
Copy-One (Get-Source 'nginx_mime_types') 'bin\nginx\conf\mime.types'

# VC++ 运行时：从打包机的 System32 拷到程序旁边（和 New-LocalRelease.ps1 同一份名单），目标机不用另装。
$vcNames = 'concrt140.dll', 'msvcp140.dll', 'msvcp140_1.dll', 'msvcp140_2.dll', 'msvcp140_atomic_wait.dll', 'msvcp140_codecvt_ids.dll', 'vcruntime140.dll', 'vcruntime140_1.dll', 'vcruntime140_threads.dll'
$vcVersion = ''
foreach ($dll in $vcNames) {
    $from = Join-Path $env:SystemRoot "System32\$dll"
    if (-not (Test-Path -LiteralPath $from)) { Fail "这台打包机上没有 $from。先装 VC++ 2015–2022 运行时（x64）再打包" }
    Copy-One $from "bin\$dll"
    if (-not $vcVersion) { $vcVersion = [string](Get-Item -LiteralPath $from).VersionInfo.FileVersion }
}
$vcInstaller = Get-Source 'vc_redist_installer' -Optional
if ($vcInstaller) { Copy-One $vcInstaller 'redist\VC_redist.x64.exe' }

# ───────────── 后端要的文件 ─────────────
Copy-One (Get-Source 'backend_config_template') 'config\DbOption.factory.toml'
# 后端的 resource 里只带数据库函数脚本（和以前的发布包一样）；入口起后端之前把它们拷进后端的工作目录。
$surrealSource = Join-Path (Get-Source 'backend_resource') 'surreal'
$n = 0
foreach ($file in @(Get-ChildItem -LiteralPath $surrealSource -Filter '*.surql' -File -ErrorAction SilentlyContinue)) { Copy-One $file.FullName "resource\surreal\$($file.Name)"; $n += 1 }
if ($n -eq 0) { Fail "sources.backend_resource 下面没有 surreal\*.surql：$surrealSource" }
Step "  resource\surreal\：$n 个文件"
# 库结构定义：缺省整个带；说明文件里的 e3d_schema_exclude 可以点名不带的（比如里面的 README.md）。
$n = Copy-Tree (Get-Source 'e3d_schema') 'schema' @(Opt $s 'e3d_schema_exclude' @())
Step "  schema\：$n 个文件"
# plant-ui 网页版。三样不带：config.json（后端读盘优先，一份静态文件会盖掉它按现场配置现算的那份；
# 入口需要时自己生成）；assets\config\ 下的工程配置（开发机上的本地文件，后端也不从这里发它）；
# config.example.json（示例配置，里面写着示例的账号口令，运行时用不到）。
$n = Copy-Tree (Get-Source 'plant_ui_web') 'web' @('config.json', 'assets\config\*', 'config.example.json')
Step "  web\：$n 个文件"

# ───────────── plant3d-web 的构建产物 ─────────────
# 前端产物里有几样不该给用户的：调试 / 演示页，和一份带开发机路径的演示数据（都是前端仓库 public\ 里现有的东西）。
# 缺省不带；要换名单，在说明文件里写 frontend_exclude（写成空数组 = 全带）。
$frontendExclude = @(Opt $s 'frontend_exclude' @('incremental-demo\*', 'dimension-kernel-demo.html', 'dimension-perf.html', 'pms-review-simulator.html', 'test-parquet-7997.html'))
$frontendSource = Get-Source 'frontend_dist'
$frontendSkipped = @()
foreach ($file in @(Get-ChildItem -LiteralPath $frontendSource -Recurse -File -Force)) {
    $relative = $file.FullName.Substring($frontendSource.TrimEnd('\').Length + 1)
    foreach ($pattern in $frontendExclude) { if ($relative -like $pattern) { $frontendSkipped += $relative.Replace('\', '/'); break } }
}
$n = Copy-Tree $frontendSource 'www' $frontendExclude
Step "  www\：$n 个文件（没带的 $($frontendSkipped.Count) 个：$(($frontendSkipped | Select-Object -First 6) -join '、')$(if ($frontendSkipped.Count -gt 6) { ' …' })）"
if (-not (Test-Path -LiteralPath (Join-Path $stage 'www\index.html'))) { Fail 'sources.frontend_dist 里没有 index.html，不像是前端的构建产物' }
$fv = Opt $s 'frontend_version'
if ($null -ne $fv) {
    # PowerShell 7 读 JSON 时会把 ISO 写法的时间自动变成时间对象，这里写回 ISO 写法。
    $buildDate = Opt $fv 'buildDate' ''
    if ($buildDate -is [datetime]) { $buildDate = $buildDate.ToUniversalTime().ToString("yyyy-MM-dd'T'HH:mm:ss.fff'Z'") }
    $versionDoc = [ordered]@{ version = [string](Opt $fv 'version' ''); commit = [string](Opt $fv 'commit' ''); buildDate = [string]$buildDate }
    [System.IO.File]::WriteAllText((Join-Path $stage 'www\version.json'), (($versionDoc | ConvertTo-Json) + "`n"), $utf8NoBom)
}

# ───────────── rs-server 的运行文件 ─────────────
$surqlCheck = $null
if ($rsExe) {
    $rsExclude = @(Opt $s 'rs_server_resource_exclude' @('HOLE_RECORD\*.json', 'metadata_archive\*', 'threed_blueprint\*'))
    $n = Copy-Tree (Get-Source 'rs_server_resource') 'rs-server\resource' $rsExclude
    Step "  rs-server\resource\：$n 个文件"
    $rsAssets = Get-Source 'rs_server_assets' -Optional
    if ($rsAssets) { $n = Copy-Tree $rsAssets 'rs-server\assets'; Step "  rs-server\assets\：$n 个文件" }
    if (@(Get-ChildItem -LiteralPath (Join-Path $stage 'rs-server\resource\surreal') -Filter '*.surql' -ErrorAction SilentlyContinue).Count -eq 0) { Fail 'rs-server 的 resource\surreal\ 里没有 .surql，它起不来' }

    # 后端和 rs-server 各有一份数据库函数库，启动时都灌进同一个库，后启动的说了算——共有的文件必须逐字一样。
    $a = Join-Path $stage 'resource\surreal'
    $b = Join-Path $stage 'rs-server\resource\surreal'
    $surqlCheck = [ordered]@{ identical = @(); different = @(); only_in_backend = @(); only_in_rs_server = @() }
    $all = @(Get-ChildItem -LiteralPath $a -Filter '*.surql' | ForEach-Object { $_.Name }) + @(Get-ChildItem -LiteralPath $b -Filter '*.surql' | ForEach-Object { $_.Name }) | Sort-Object -Unique
    foreach ($file in $all) {
        $pa = Join-Path $a $file; $pb = Join-Path $b $file
        if (-not (Test-Path -LiteralPath $pb)) { $surqlCheck.only_in_backend += $file; continue }
        if (-not (Test-Path -LiteralPath $pa)) { $surqlCheck.only_in_rs_server += $file; continue }
        $same = ((Read-Text $pa) -replace "`r`n", "`n") -ceq ((Read-Text $pb) -replace "`r`n", "`n")
        if ($same) { $surqlCheck.identical += $file } else { $surqlCheck.different += $file }
    }
    if ($surqlCheck.different.Count -gt 0 -and -not [bool](Opt $s 'allow_surql_mismatch' $false)) {
        Fail "后端和 rs-server 的数据库函数库有不一样的文件：$($surqlCheck.different -join '、')。rs-server 后启动，会把后端灌的函数换掉。先把两边对齐（确实要这样出，就在说明文件里写 allow_surql_mismatch: true）"
    }
}

# ───────────── 入口、配置、说明 ─────────────
$stackPs1 = Read-Text (Join-Path $here 'stack.ps1')
$entryVersion = ''
if ($stackPs1 -match "\`$script:StackVersion\s*=\s*'([^']+)'") { $entryVersion = $Matches[1] }
Write-Text 'stack.ps1' $stackPs1 $utf8Bom
foreach ($pair in @(@('stack.cmd', 'stack.cmd'), @('stack-up.cmd', 'start.cmd'), @('stack-up.cmd', '开始.cmd'), @('stack-down.cmd', 'stop.cmd'), @('stack-down.cmd', '停止.cmd'), @('stack-status.cmd', 'status.cmd'), @('stack-status.cmd', '状态.cmd'))) {
    $text = Read-Text (Join-Path $here $pair[0])
    if ($text -match '[^\x00-\x7F]') { Fail "$($pair[0]) 里有非 ASCII 字符。批处理文件要保持纯 ASCII（cmd 按本机代码页读它）" }
    Write-Text $pair[1] $text $ascii
}
# 包里带的是缺省配置 stack.default.toml；stack.toml 是用户自己的那一份，不进包（见文件头的说明）。
Write-Text 'stack.default.toml' (Read-Text (Join-Path $here 'package\stack.default.toml')) $utf8Bom
Write-Text 'stack.example.toml' (Read-Text (Join-Path $here 'stack.example.toml')) $utf8Bom

$created = Get-Date
$components = @(Opt $s 'components' @())
$notIncluded = @(Opt $s 'not_included_fixes' @())
$componentLines = @()
foreach ($c in $components) {
    $line = "- **$(Opt $c 'title' (Opt $c 'id' ''))**"
    if (Opt $c 'version') { $line += " $(Opt $c 'version')" }
    $line += '：'
    $bits = @()
    if (Opt $c 'source') { $bits += [string](Opt $c 'source') }
    if (Opt $c 'built_at') { $bits += "构建于 $(Opt $c 'built_at')" }
    if ([bool](Opt $c 'dirty' $false)) { $bits += "**带未提交的改动**$(if (Opt $c 'dirty_note') { '（' + (Opt $c 'dirty_note') + '）' })" }
    if (Opt $c 'note') { $bits += [string](Opt $c 'note') }
    $componentLines += ($line + ($bits -join '；') + '。')
}
$notIncludedLines = @()
foreach ($item in $notIncluded) {
    $text = "- $(Opt $item 'issue' '')"
    if (Opt $item 'title') { $text += "　$(Opt $item 'title')" }
    $notIncludedLines += $text
}
$knownIssues = @(Opt $s 'known_issues' @())
$knownLines = @()
foreach ($item in $knownIssues) { $knownLines += "$($knownLines.Count + 1). $item" }
$doc = Read-Text (Join-Path $here 'package\INSTALL.zh.md')
$doc = $doc.Replace('{{KNOWN_ISSUES}}', ($knownLines -join "`n"))
$doc = $doc.Replace('{{PACKAGE}}', $name).Replace('{{VERSION}}', [string](Opt $s 'version' '')).Replace('{{CREATED}}', $created.ToString('yyyy-MM-dd HH:mm')).Replace('{{ENTRY_VERSION}}', $entryVersion)
$doc = $doc.Replace('{{COMPONENTS}}', ($componentLines -join "`n")).Replace('{{NOT_INCLUDED}}', ($notIncludedLines -join "`n")).Replace('{{NOT_INCLUDED_NOTE}}', [string](Opt $s 'not_included_note' ''))
# 这一版特有的几句话（哪一包是最新的、从上一包怎么换过来、带上了哪些修复）；说明文件里没写就留空。
$doc = $doc.Replace('{{RELEASE_NOTE}}', [string](Opt $s 'release_note' '')).Replace('{{UPGRADE_NOTE}}', [string](Opt $s 'upgrade_note' '')).Replace('{{INCLUDED_NOTE}}', [string](Opt $s 'included_note' ''))
$doc = [regex]::Replace($doc, "(\r?\n){3,}", "`n`n")
if ($doc -match '\{\{[A-Z_]+\}\}') { Fail "说明文档里还有没填的占位符：$($Matches[0])" }
Write-Text '安装和测试说明.md' $doc $utf8Bom
Write-Text 'README.txt' $doc $utf8Bom

# ───────────── 抹掉文本里不该带出去的字样（可选） ─────────────
# 有的东西编进了别的仓库的产物里（比如前端把更新日志全文打进了脚本，里面有一处线上环境的地址），这里改不了源头，
# 就在摆包目录的副本上抹掉。说明文件里写 redactions，每条：
#   { "files": "www/assets/index-*.js", "pattern": "<正则>", "replacement": "<换成什么>", "what": "<这是什么>" }
# files 是包里的相对路径（正斜杠，可以带通配符）。只动摆包目录里的副本，不动来源。
# 一处都没命中就报错（来源变了，或者规则写错了）；命中了几处、动了哪些文件，记进清单的 redactions。
$redactionLog = @()
foreach ($rule in @(Opt $s 'redactions' @())) {
    if ($null -eq $rule) { continue }
    $like = [string](Opt $rule 'files' '')
    $pattern = [string](Opt $rule 'pattern' '')
    $what = [string](Opt $rule 'what' $pattern)
    if (-not $like -or -not $pattern) { Fail 'redactions 里每一条都要有 files 和 pattern' }
    $total = 0
    $touched = @()
    foreach ($file in @(Get-ChildItem -LiteralPath $stage -Recurse -File -Force)) {
        $relative = $file.FullName.Substring($stage.Length + 1).Replace('\', '/')
        if ($relative -notlike $like) { continue }
        $bytes = [System.IO.File]::ReadAllBytes($file.FullName)
        $hadBom = ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF)
        $text = [System.IO.File]::ReadAllText($file.FullName, [System.Text.Encoding]::UTF8)
        $count = [regex]::Matches($text, $pattern).Count
        if ($count -eq 0) { continue }
        $encoding = $utf8NoBom
        if ($hadBom) { $encoding = $utf8Bom }
        [System.IO.File]::WriteAllText($file.FullName, [regex]::Replace($text, $pattern, [string](Opt $rule 'replacement' '')), $encoding)
        $total += $count
        $touched += $relative
    }
    if ($total -eq 0) { Fail "redactions：「$what」一处都没命中（files = $like）。来源变了，或者规则写错了" }
    $redactionLog += [ordered]@{ what = $what; files = $touched; hits = $total }
    Step "  抹掉了「$what」：$total 处（$($touched -join '、')）"
}

# ───────────── 不该进包的东西：再查一遍名字 ─────────────
$bad = @()
foreach ($file in @(Get-ChildItem -LiteralPath $stage -Recurse -File -Force)) {
    $relative = $file.FullName.Substring($stage.Length + 1)
    if ($relative -eq $stageMarker) { continue }
    if ($file.Name -like 'DbOption*.toml' -and $relative -ne 'config\DbOption.factory.toml') { $bad += $relative }
    elseif ($file.Name -like '.env*' -or $file.Name -like '*.pem' -or $file.Name -like '*.pfx' -or $file.Name -like 'id_rsa*' -or $file.Name -like '*.key') { $bad += $relative }
    elseif ($relative -like 'web\config.json') { $bad += $relative }
    # 用户自己的配置不进包：进了包，换新一版时解压就会把用户改过的那份盖掉。
    elseif ($relative -eq 'stack.toml') { $bad += $relative }
}
if ($bad.Count -gt 0) { Fail "这些文件不该进包（带账号口令的配置、密钥一类，或者用户自己的配置 stack.toml）：$($bad -join '、')" }

# ───────────── 清单 ─────────────
Step '算每个文件的校验值…'
$sha = [System.Security.Cryptography.SHA256]::Create()
$files = @()
$totalBytes = [int64]0
foreach ($file in @(Get-ChildItem -LiteralPath $stage -Recurse -File -Force | Sort-Object { $_.FullName.Substring($stage.Length + 1).Replace('\', '/') })) {
    $relative = $file.FullName.Substring($stage.Length + 1).Replace('\', '/')
    if ($relative -eq $stageMarker) { continue }
    $stream = [System.IO.File]::OpenRead($file.FullName)
    try { $hash = -join ($sha.ComputeHash($stream) | ForEach-Object { $_.ToString('x2') }) } finally { $stream.Dispose() }
    $files += [ordered]@{ path = $relative; bytes = $file.Length; sha256 = $hash }
    $totalBytes += $file.Length
}
$byPath = @{}
foreach ($f in $files) { $byPath[$f.path] = $f }
$componentDocs = @()
foreach ($c in $components) {
    $entry = [ordered]@{}
    foreach ($p in $c.PSObject.Properties) { $entry[$p.Name] = $p.Value }
    $main = [string](Opt $c 'path' '')
    if ($main -and $byPath.ContainsKey($main)) { $entry['bytes'] = $byPath[$main].bytes; $entry['sha256'] = $byPath[$main].sha256 }
    $componentDocs += $entry
}
$manifest = [ordered]@{
    schema = 1
    name = $name
    version = [string](Opt $s 'version' '')
    channel = [string](Opt $s 'channel' '')
    created_at = $created.ToString('yyyy-MM-ddTHH:mm:sszzz')
    entry = [ordered]@{ script = 'stack.ps1'; version = $entryVersion; config = 'stack.toml'; default_config = 'stack.default.toml'; config_note = '包里只带 stack.default.toml；stack.toml 在第一次「开始」时照它生成，是用户自己的，不在包里' }
    default_ports = Opt $s 'default_ports'
    components = $componentDocs
    vc_runtime = [ordered]@{ version = $vcVersion; app_local_dlls = $vcNames; installer = $(if ($vcInstaller) { 'redist/VC_redist.x64.exe' } else { '' }) }
    release_note = [string](Opt $s 'release_note' '')
    supersedes = @(Opt $s 'supersedes' @())
    included_note = [string](Opt $s 'included_note' '')
    not_included_fixes = $notIncluded
    not_included_note = [string](Opt $s 'not_included_note' '')
    checks = [ordered]@{ surql_libraries = $surqlCheck }
    # 来源里有、这个包里故意没带的文件（前端的调试 / 演示页和演示数据）。
    excluded = [ordered]@{ frontend = $frontendSkipped }
    # 在包里的副本上抹掉的字样（来源没动）。
    redactions = $redactionLog
    known_issues = $knownIssues
    notes = @(Opt $s 'notes' @())
    totals = [ordered]@{ files = $files.Count + 1; bytes_without_manifest = $totalBytes }
    files = $files
}
$manifestText = ($manifest | ConvertTo-Json -Depth 8) + "`n"
[System.IO.File]::WriteAllText((Join-Path $stage 'release.json'), $manifestText, $utf8NoBom)
Step ("  {0} 个文件，{1:N1} MB（不含清单本身）" -f $files.Count, ($totalBytes / 1MB))

if ($NoZip) { Step "只摆目录，不压缩（-NoZip）。目录：$stage"; exit 0 }

# ───────────── 压缩 ─────────────
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
New-Folder $outDir
$outDir = (Resolve-Path -LiteralPath $outDir).Path
$zipPath = Join-Path $outDir ($name + '.zip')
if (Test-Path -LiteralPath $zipPath) {
    if (-not $Force) { Fail "$zipPath 已经在。换一个 package_name，或加 -Force 覆盖" }
    Remove-Item -LiteralPath $zipPath -Force
}
Step "压缩：$zipPath"
$zip = [System.IO.Compression.ZipFile]::Open($zipPath, [System.IO.Compression.ZipArchiveMode]::Create)
try {
    foreach ($relative in (@($files | ForEach-Object { $_.path }) + 'release.json')) {
        [void][System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, (Join-Path $stage $relative.Replace('/', '\')), $relative, [System.IO.Compression.CompressionLevel]::Optimal)
    }
} finally { $zip.Dispose() }
$stream = [System.IO.File]::OpenRead($zipPath)
try { $zipHash = -join ($sha.ComputeHash($stream) | ForEach-Object { $_.ToString('x2') }) } finally { $stream.Dispose() }
$zipBytes = (Get-Item -LiteralPath $zipPath).Length
[System.IO.File]::WriteAllText("$zipPath.sha256", "$zipHash  $($name).zip`n", $ascii)
[System.IO.File]::Copy((Join-Path $stage 'release.json'), (Join-Path $outDir "$name.release.json"), $true)
Step ("完成：{0}（{1:N1} MB，{2:N0} 字节）" -f $zipPath, ($zipBytes / 1MB), $zipBytes)
Step "  sha256 $zipHash"
Step "  清单另存一份：$(Join-Path $outDir "$name.release.json")"
