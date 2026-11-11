# 出包脚本（New-StackRelease.ps1）：清单、排除规则、不该进包的文件、清口令、压缩。
# 每条测试都摆一套假的来源（几个字节的假程序、假网页、假模板），把出包脚本当成一个独立的进程真跑一遍，
# 然后看摆出来的目录和它说的话。不碰任何真的部件，不起服务；VC++ 运行时的 DLL 是从这台机器的 System32 拷的（出包脚本本来就这么做）。
# 模板里的「口令」都是假的字样（FAKE-…），用来核对它们有没有漏进包、清单或输出里。

$script:RelScript = Join-Path $script:StackDir 'New-StackRelease.ps1'
$script:RelHost = (Get-Process -Id $PID).Path
$script:RelBom = New-Object System.Text.UTF8Encoding -ArgumentList $true

$script:RelTemplate = @'
# 假的后端配置模板（测试用）
project_name = ""
mdb_name = "ALL"
ip = "127.0.0.1"
user = "FAKE-MYSQL-USER"
password = "FAKE-MYSQL-PASS"   # 缺省口令是 FAKE-MYSQL-PASS
port = "3306"
v_ip = "127.0.0.1"
v_user = "FAKE-DB-USER"
v_password = "FAKE-DB-PASS"
v_port = "8009"
arangodb_password = ""
plat_url = ""
puhua_database_ip = "localhost:3306"
puhua_database_user = "FAKE-PUHUA-USER"
puhua_database_password = 'FAKE-PUHUA-PASS'
room_key_word = ["-RM", "-ROOM"]

[rvm_check]
work_dir = "output/rvm-check"
'@

# 摆一套假的来源，返回 @{ Root; Spec（哈希表，可以再改）; Template }。
function New-FakeRelease([string]$Template = $script:RelTemplate, [bool]$TemplateBom = $false, [string]$TemplateEol = "`n") {
    $root = New-TestDir 'rel'
    $src = Join-Path $root 'src'
    foreach ($pair in @(@('surreal.exe', 'fake surreal'), @('aios-database.exe', 'fake backend'), @('rs-server.exe', 'fake rs-server'), @('nginx\nginx.exe', 'fake nginx'), @('nginx\conf\mime.types', 'types { }'))) {
        Write-TestFile (Join-Path $src $pair[0]) $pair[1]
    }
    $templatePath = Join-Path $src 'DbOption.factory.toml'
    $text = ($Template -replace "`r?`n", $TemplateEol)
    [void][System.IO.Directory]::CreateDirectory($src)
    if ($TemplateBom) { [System.IO.File]::WriteAllText($templatePath, $text, $script:RelBom) } else { [System.IO.File]::WriteAllText($templatePath, $text, $script:TestUtf8) }
    Write-TestFile (Join-Path $src 'resource\surreal\a.surql' ) 'DEFINE FUNCTION fn::a() { RETURN 1; };'
    Write-TestFile (Join-Path $src 'resource\surreal\b.surql') 'DEFINE FUNCTION fn::b() { RETURN 2; };'
    Write-TestFile (Join-Path $src 'schema\e3d31\nouns.json') '{"nouns":[]}'
    Write-TestFile (Join-Path $src 'schema\README.md') 'schema readme'
    Write-TestFile (Join-Path $src 'web\index.html') '<html>plant-ui</html>'
    Write-TestFile (Join-Path $src 'web\assets\app.js') 'console.log("plant-ui")'
    Write-TestFile (Join-Path $src 'web\config.json') '{"from":"dev machine"}'
    Write-TestFile (Join-Path $src 'web\config.example.json') '{"example":true}'
    Write-TestFile (Join-Path $src 'web\assets\config\project.json') '{"local":true}'
    Write-TestFile (Join-Path $src 'www\index.html') '<html>plant3d-web</html>'
    Write-TestFile (Join-Path $src 'www\assets\index-1.js') 'var a = "FAKE-LIVE-URL"; var b = "FAKE-LIVE-URL";'
    Write-TestFile (Join-Path $src 'www\keep.html') 'keep'
    foreach ($demo in 'incremental-demo\a.json', 'incremental-demo\sub\b.json', 'dimension-kernel-demo.html', 'dimension-perf.html', 'pms-review-simulator.html', 'test-parquet-7997.html') {
        Write-TestFile (Join-Path $src "www\$demo") 'demo'
    }
    Write-TestFile (Join-Path $src 'rs\resource\surreal\a.surql') 'DEFINE FUNCTION fn::a() { RETURN 1; };'
    Write-TestFile (Join-Path $src 'rs\resource\keep.txt') 'keep'
    Write-TestFile (Join-Path $src 'rs\resource\HOLE_RECORD\x.json') '{}'
    Write-TestFile (Join-Path $src 'rs\resource\metadata_archive\y.txt') 'y'
    Write-TestFile (Join-Path $src 'rs\resource\threed_blueprint\z.txt') 'z'
    Write-TestFile (Join-Path $src 'rs\assets\logo.txt') 'logo'
    $spec = [ordered]@{
        package_name = 'fake-package-test'
        version = '0.0.1-test'
        channel = 'test'
        release_note = '假的包，测试用。'
        sources = [ordered]@{
            database_exe = (Join-Path $src 'surreal.exe')
            backend_exe = (Join-Path $src 'aios-database.exe')
            rs_server_exe = (Join-Path $src 'rs-server.exe')
            nginx_exe = (Join-Path $src 'nginx\nginx.exe')
            nginx_mime_types = (Join-Path $src 'nginx\conf\mime.types')
            backend_config_template = $templatePath
            backend_resource = (Join-Path $src 'resource')
            e3d_schema = (Join-Path $src 'schema')
            plant_ui_web = (Join-Path $src 'web')
            frontend_dist = (Join-Path $src 'www')
            rs_server_resource = (Join-Path $src 'rs\resource')
            rs_server_assets = (Join-Path $src 'rs\assets')
        }
        components = @(
            [ordered]@{ id = 'backend'; title = '后端'; version = '0.0.1'; path = 'bin/aios-database.exe'; source = '假的' }
            [ordered]@{ id = 'frontend'; title = '前端'; version = '0.0.1'; path = 'www/index.html' }
        )
        known_issues = @('第一条已知问题', '第二条已知问题')
    }
    return @{ Root = $root; Src = $src; Spec = $spec; Template = $templatePath; Stage = (Join-Path $root 'stage'); Out = (Join-Path $root 'out') }
}

# 真跑一遍出包脚本。返回 @{ Code; Text; Stage; Manifest }
function Invoke-Release($Fake, [string[]]$More = @('-NoZip')) {
    $specPath = Join-Path $Fake.Root 'spec.json'
    [System.IO.File]::WriteAllText($specPath, ($Fake.Spec | ConvertTo-Json -Depth 8), $script:TestUtf8)
    $argList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $script:RelScript, '-Spec', $specPath, '-Stage', $Fake.Stage) + $More
    # 子进程往错误流里写东西时，Windows PowerShell 5.1 会把它当成这边的错误抛出来；这里只要它的输出和退出码。
    $lines = @()
    $code = -1
    $oldPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $lines = & $script:RelHost @argList 2>&1
        $code = $LASTEXITCODE
    } finally { $ErrorActionPreference = $oldPreference }
    $manifest = $null
    $manifestPath = Join-Path $Fake.Stage 'release.json'
    if ($code -eq 0 -and (Test-Path -LiteralPath $manifestPath)) { $manifest = [System.IO.File]::ReadAllText($manifestPath, [System.Text.Encoding]::UTF8) | ConvertFrom-Json }
    return @{ Code = $code; Text = (@($lines | ForEach-Object { "$_" }) -join "`n"); Stage = $Fake.Stage; Manifest = $manifest }
}

function Get-RelHash([string]$Path) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    $stream = [System.IO.File]::OpenRead($Path)
    try { return (-join ($sha.ComputeHash($stream) | ForEach-Object { $_.ToString('x2') })) } finally { $stream.Dispose() }
}

# 摆包目录里实际有的文件（相对路径，正斜杠）。
function Get-StageFiles([string]$Stage) {
    $prefix = $Stage.TrimEnd('\').Length + 1
    return @(Get-ChildItem -LiteralPath $Stage -Recurse -File -Force | ForEach-Object { $_.FullName.Substring($prefix).Replace('\', '/') })
}

# 一个字样出现在摆包目录的哪些文件里（按字节找，文本、程序都算）。
function Find-InStage([string]$Stage, [string]$Needle) {
    $found = @()
    $prefix = $Stage.TrimEnd('\').Length + 1
    foreach ($file in @(Get-ChildItem -LiteralPath $Stage -Recurse -File -Force)) {
        if ($file.Length -gt 4MB) { continue }
        $text = [System.Text.Encoding]::UTF8.GetString([System.IO.File]::ReadAllBytes($file.FullName))
        if ($text.Contains($Needle)) { $found += $file.FullName.Substring($prefix).Replace('\', '/') }
    }
    return $found
}

Test '清单：列出来的每个文件都在、大小和校验值都对；目录里没有清单之外的文件；版本信息对' {
    $fake = New-FakeRelease
    $r = Invoke-Release $fake
    Assert-Equal 0 $r.Code "出包脚本的退出码（它说：$($r.Text)）"
    $m = $r.Manifest
    Assert-True ($null -ne $m) 'release.json 读得出来'
    Assert-Equal 'fake-package-test' $m.name '包名'
    Assert-Equal '0.0.1-test' $m.version '版本'
    Assert-Equal $script:StackVersion $m.entry.version '入口的版本号就是 stack.ps1 里那个'
    Assert-Equal 'stack.default.toml' $m.entry.default_config '缺省配置的名字'
    Assert-Equal 'stack.toml' $m.entry.config '用户配置的名字'
    $listed = @($m.files | ForEach-Object { $_.path })
    foreach ($f in $m.files) {
        $p = Join-Path $r.Stage $f.path.Replace('/', '\')
        Assert-True (Test-Path -LiteralPath $p -PathType Leaf) "$($f.path) 在"
        Assert-Equal $f.bytes (Get-Item -LiteralPath $p).Length "$($f.path) 的大小"
        Assert-Equal $f.sha256 (Get-RelHash $p) "$($f.path) 的校验值"
    }
    $actual = @(Get-StageFiles $r.Stage | Where-Object { $_ -ne 'release.json' -and $_ -ne '.stack-release-stage' } | Sort-Object)
    Assert-Equal (@($listed | Sort-Object) -join '|') ($actual -join '|') '清单和目录里的文件一一对应'
    Assert-Equal ($listed.Count + 1) $m.totals.files '文件总数 = 清单里的 + 清单自己'
    $sum = [int64]0
    foreach ($f in $m.files) { $sum += [int64]$f.bytes }
    Assert-Equal $sum ([int64]$m.totals.bytes_without_manifest) '总字节数'
    foreach ($must in 'stack.ps1', 'stack.cmd', 'start.cmd', '开始.cmd', 'stop.cmd', '停止.cmd', 'status.cmd', '状态.cmd', 'stack.default.toml', 'stack.example.toml', 'README.txt', '安装和测试说明.md', 'bin/surreal.exe', 'bin/aios-database.exe', 'bin/rs-server.exe', 'bin/nginx/nginx.exe', 'bin/nginx/conf/mime.types', 'bin/vcruntime140.dll', 'config/DbOption.factory.toml', 'resource/surreal/a.surql', 'schema/e3d31/nouns.json', 'web/index.html', 'www/index.html', 'rs-server/resource/surreal/a.surql', 'rs-server/assets/logo.txt') {
        Assert-True ($listed -contains $must) "清单里有 $must"
    }
    Assert-True ($listed -notcontains 'stack.toml') '包里没有 stack.toml（那是用户自己的）'
    $backend = @($m.components | Where-Object { $_.id -eq 'backend' })[0]
    Assert-Equal (Get-RelHash (Join-Path $r.Stage 'bin\aios-database.exe')) $backend.sha256 '部件表里后端的校验值就是包里那个文件的'
    Assert-Equal 2 @($m.known_issues).Count '已知问题带进了清单'
    Assert-Equal 'a.surql' (@($m.checks.surql_libraries.identical) -join ',') '两边都有的数据库函数脚本核对过是一样的'
    Assert-Equal 'b.surql' (@($m.checks.surql_libraries.only_in_backend) -join ',') '只有后端有的也记下了'
    # 入口、批处理和包里的是不是同一份内容（换行统一成 CRLF 之后比）
    $stackInPackage = [System.IO.File]::ReadAllText((Join-Path $r.Stage 'stack.ps1'), [System.Text.Encoding]::UTF8) -replace "`r`n", "`n"
    $stackInRepo = [System.IO.File]::ReadAllText($script:StackScript, [System.Text.Encoding]::UTF8) -replace "`r`n", "`n"
    Assert-True ($stackInPackage -ceq $stackInRepo) '包里的 stack.ps1 就是仓库里这一份'
    $doc = [System.IO.File]::ReadAllText((Join-Path $r.Stage 'README.txt'), [System.Text.Encoding]::UTF8)
    Assert-Match $doc 'fake-package-test' '说明里填上了包名'
    Assert-Match $doc '1\. 第一条已知问题' '说明里列了已知问题'
    Assert-NotMatch $doc '\{\{[A-Z_]+\}\}' '说明里没有没填的占位符'
}

Test '排除规则：前端的演示页、plant-ui 的本地配置、rs-server 的运行数据都不带；写了名单就按名单' {
    $fake = New-FakeRelease
    $r = Invoke-Release $fake
    Assert-Equal 0 $r.Code "退出码（$($r.Text)）"
    $files = Get-StageFiles $r.Stage
    foreach ($gone in 'www/incremental-demo/a.json', 'www/incremental-demo/sub/b.json', 'www/dimension-kernel-demo.html', 'www/dimension-perf.html', 'www/pms-review-simulator.html', 'www/test-parquet-7997.html', 'web/config.json', 'web/config.example.json', 'web/assets/config/project.json', 'rs-server/resource/HOLE_RECORD/x.json', 'rs-server/resource/metadata_archive/y.txt', 'rs-server/resource/threed_blueprint/z.txt') {
        Assert-True ($files -notcontains $gone) "$gone 没有带"
    }
    foreach ($kept in 'www/keep.html', 'www/assets/index-1.js', 'web/assets/app.js', 'rs-server/resource/keep.txt', 'schema/README.md') {
        Assert-True ($files -contains $kept) "$kept 带了"
    }
    Assert-Equal 6 @($r.Manifest.excluded.frontend).Count '清单里记下了前端没带的 6 个文件'
    Assert-True (@($r.Manifest.excluded.frontend) -contains 'incremental-demo/sub/b.json') '记的是相对路径'

    # 名单可以换：前端全带、库结构定义里点名不带一个、rs-server 只排一个目录
    $fake2 = New-FakeRelease
    $fake2.Spec['frontend_exclude'] = @()
    $fake2.Spec['e3d_schema_exclude'] = @('README.md')
    $fake2.Spec['rs_server_resource_exclude'] = @('metadata_archive\*')
    $r2 = Invoke-Release $fake2
    Assert-Equal 0 $r2.Code "退出码（$($r2.Text)）"
    $files2 = Get-StageFiles $r2.Stage
    Assert-True ($files2 -contains 'www/incremental-demo/a.json') 'frontend_exclude 写成空的：演示数据也带'
    Assert-Equal 0 @($r2.Manifest.excluded.frontend).Count '清单里「没带的」是空的'
    Assert-True ($files2 -notcontains 'schema/README.md') 'e3d_schema_exclude 点名的没带'
    Assert-True ($files2 -contains 'rs-server/resource/HOLE_RECORD/x.json') 'rs-server 的名单换了之后，原来缺省排掉的带上了'
    Assert-True ($files2 -notcontains 'rs-server/resource/metadata_archive/y.txt') '新名单里的没带'
    # plant-ui 那三样是写死的，不看名单
    Assert-True ($files2 -notcontains 'web/config.json') 'plant-ui 的 config.json 任何时候都不带'
}

Test '不该进包的文件：模板之外的 DbOption*.toml、.env、密钥文件——报错，不出包' {
    foreach ($case in @(
            @{ Rel = 'web\assets\DbOption.toml'; Shown = 'DbOption.toml' }
            @{ Rel = 'www\DbOption-prod.toml'; Shown = 'DbOption-prod.toml' }
            @{ Rel = 'schema\DbOption.factory.toml'; Shown = 'DbOption.factory.toml' }
            @{ Rel = 'www\.env.production'; Shown = '.env.production' }
            @{ Rel = 'rs\assets\server.pem'; Shown = 'server.pem' }
            @{ Rel = 'web\id_rsa'; Shown = 'id_rsa' }
        )) {
        $fake = New-FakeRelease
        Write-TestFile (Join-Path $fake.Src $case.Rel) 'v_password = "FAKE-STRAY-SECRET"'
        $r = Invoke-Release $fake
        Assert-Equal 2 $r.Code "$($case.Rel) 混进来源时的退出码"
        Assert-Match $r.Text ([regex]::Escape($case.Shown)) "$($case.Rel)：报错里点了这个文件的名"
        Assert-NotMatch $r.Text 'FAKE-STRAY-SECRET' '报错里没有文件的内容'
        Assert-True (-not (Test-Path -LiteralPath (Join-Path $r.Stage 'release.json'))) '没有写清单'
    }
    # 摆包目录已经在、又不是这个脚本建的：不动它
    $fake = New-FakeRelease
    Write-TestFile (Join-Path $fake.Stage 'someone-elses-file.txt') 'x'
    $r = Invoke-Release $fake
    Assert-Equal 2 $r.Code '摆包目录是别人的：退出码 2'
    Assert-True (Test-Path -LiteralPath (Join-Path $fake.Stage 'someone-elses-file.txt')) '别人的文件没动'
}

Test '清口令：点名的键清成空串，行留着、别的行一个字节都不动；值不进包、不进清单、不进输出' {
    foreach ($shape in @(@{ Bom = $true; Eol = "`r`n"; What = '带 BOM、CRLF' }, @{ Bom = $false; Eol = "`n"; What = '不带 BOM、LF' })) {
        $fake = New-FakeRelease $script:RelTemplate $shape.Bom $shape.Eol
        $sourceBefore = [System.IO.File]::ReadAllBytes($fake.Template)
        $r = Invoke-Release $fake
        Assert-Equal 0 $r.Code "$($shape.What)：退出码（$($r.Text)）"
        # 期望的结果：照来源的文本，只把那四行换掉
        $expected = ($script:RelTemplate -replace "`r?`n", "`n")
        $expected = $expected.Replace('user = "FAKE-MYSQL-USER"', 'user = ""').Replace('password = "FAKE-MYSQL-PASS"   # 缺省口令是 FAKE-MYSQL-PASS', 'password = ""')
        $expected = $expected.Replace('puhua_database_user = "FAKE-PUHUA-USER"', 'puhua_database_user = ""').Replace("puhua_database_password = 'FAKE-PUHUA-PASS'", 'puhua_database_password = ""')
        $expected = ($expected -replace "`n", $shape.Eol)
        $expectedBytes = [System.Text.Encoding]::UTF8.GetBytes($expected)
        if ($shape.Bom) { $expectedBytes = [byte[]](0xEF, 0xBB, 0xBF) + $expectedBytes }
        $staged = [System.IO.File]::ReadAllBytes((Join-Path $r.Stage 'config\DbOption.factory.toml'))
        Assert-Equal ([System.BitConverter]::ToString($expectedBytes)) ([System.BitConverter]::ToString($staged)) "$($shape.What)：包里的模板只有那四行变了（换行符、BOM 照旧）"
        $stagedText = [System.Text.Encoding]::UTF8.GetString($staged)
        Assert-Match $stagedText '(?m)^v_user = "FAKE-DB-USER"\r?$' '数据库那一对的账号没动'
        Assert-Match $stagedText '(?m)^v_password = "FAKE-DB-PASS"\r?$' '数据库那一对的口令没动'
        Assert-Match $stagedText '(?m)^ip = "127\.0\.0\.1"\r?$' 'MySQL 的地址没动（不是口令）'
        Assert-Equal ([System.BitConverter]::ToString($sourceBefore)) ([System.BitConverter]::ToString([System.IO.File]::ReadAllBytes($fake.Template))) '来源那份模板没被改'
        foreach ($secret in 'FAKE-MYSQL-USER', 'FAKE-MYSQL-PASS', 'FAKE-PUHUA-USER', 'FAKE-PUHUA-PASS') {
            Assert-Equal 0 @(Find-InStage $r.Stage $secret).Count "$secret 不在包里的任何文件里"
            Assert-NotMatch $r.Text $secret "$secret 不在出包脚本的输出里"
        }
        Assert-Equal 'config/DbOption.factory.toml' (@(Find-InStage $r.Stage 'FAKE-DB-PASS') -join ',') '数据库的口令只在模板里（清单里没有）'
        Assert-NotMatch $r.Text 'FAKE-DB-' '数据库那一对的值也不在输出里'
        $t = $r.Manifest.backend_config_template
        Assert-Equal 'user,password,puhua_database_user,puhua_database_password' (@($t.blanked | ForEach-Object { $_.key }) -join ',') '清单里记了清的是哪几个键'
        Assert-Equal '1,1,1,1' (@($t.blanked | ForEach-Object { $_.lines }) -join ',') '每个键清了一行'
        Assert-Equal 'True,True,True,True' (@($t.blanked | ForEach-Object { $_.had_value }) -join ',') '来源里原来都带着值'
        Assert-Equal 'v_user,v_password' (@($t.kept) -join ',') '清单里记了留着的那一对'
        Assert-Match $r.Text 'user' '输出里说了清的是哪些键（只有键名）'
    }
}

Test '清口令：名单可以换；模板里没有的键只说一声；名字像口令的键还带着值就不出包' {
    # 名单里多写一个模板里没有的键：照常出，说一声
    $fake = New-FakeRelease
    $fake.Spec['backend_config_blank_keys'] = @('user', 'password', 'puhua_database_user', 'puhua_database_password', 'no_such_key')
    $r = Invoke-Release $fake
    Assert-Equal 0 $r.Code "多写一个没有的键：退出码（$($r.Text)）"
    Assert-Match $r.Text 'no_such_key' '说了模板里没有它'
    $absent = @($r.Manifest.backend_config_template.blanked | Where-Object { $_.key -eq 'no_such_key' })[0]
    Assert-Equal 0 $absent.lines '清单里记的是 0 行'

    # 名单写成空的（什么都不清）：模板里的口令还带着值——不出包，点出键名，不带值
    $fake = New-FakeRelease
    $fake.Spec['backend_config_blank_keys'] = @()
    $r = Invoke-Release $fake
    Assert-Equal 2 $r.Code '什么都不清、口令还在：退出码 2'
    Assert-Match $r.Text 'password' '点了 password'
    Assert-Match $r.Text 'puhua_database_password' '点了 puhua_database_password'
    Assert-NotMatch $r.Text 'FAKE-' '没有带出任何值'
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $r.Stage 'release.json'))) '没有写清单'

    # 确实要带着的：写进「留着」的名单就放行
    $fake = New-FakeRelease
    $fake.Spec['backend_config_blank_keys'] = @('user', 'puhua_database_user')
    $fake.Spec['backend_config_keep_keys'] = @('v_user', 'v_password', 'password', 'puhua_database_password')
    $r = Invoke-Release $fake
    Assert-Equal 0 $r.Code "点名留着：退出码（$($r.Text)）"
    Assert-Equal 'password,puhua_database_password,v_password,v_user' ((@($r.Manifest.backend_config_template.kept) | Sort-Object) -join ',') '清单里记了留着的键'

    # 某一节里藏着一个令牌：也查得到，报的是「[节] 键」
    $fake = New-FakeRelease ($script:RelTemplate + "`n[extra]`napi_token = `"FAKE-TOKEN-IN-TABLE`"`n")
    $r = Invoke-Release $fake
    Assert-Equal 2 $r.Code '节里有令牌：退出码 2'
    Assert-Match $r.Text '\[extra\] api_token' '报的是哪一节的哪个键'
    Assert-NotMatch $r.Text 'FAKE-TOKEN-IN-TABLE' '没有带出值'

    # 新冒出来一个顶层的口令键、名单里没有它：不出包
    $fake = New-FakeRelease ($script:RelTemplate.Replace('plat_url = ""', "plat_url = `"`"`nplat_ukey = `"FAKE-UKEY`""))
    $r = Invoke-Release $fake
    Assert-Equal 2 $r.Code '新的口令键不在名单里：退出码 2'
    Assert-Match $r.Text 'plat_ukey' '点了键名'
    Assert-NotMatch $r.Text 'FAKE-UKEY' '没有带出值'

    # 同一个键又要清又要留：报错
    $fake = New-FakeRelease
    $fake.Spec['backend_config_keep_keys'] = @('v_user', 'v_password', 'password')
    $r = Invoke-Release $fake
    Assert-Equal 2 $r.Code '又清又留：退出码 2'

    # 值是多行字符串的不会改：报错，不带出值
    $fake = New-FakeRelease ($script:RelTemplate.Replace('password = "FAKE-MYSQL-PASS"   # 缺省口令是 FAKE-MYSQL-PASS', "password = `"`"`"FAKE-MULTI`nLINE`"`"`""))
    $r = Invoke-Release $fake
    Assert-Equal 2 $r.Code '多行字符串：退出码 2'
    Assert-NotMatch $r.Text 'FAKE-MULTI' '没有带出值'
}

Test '抹字样（redactions）：命中几处记几处；一处都没命中就报错' {
    $fake = New-FakeRelease
    $fake.Spec['redactions'] = @([ordered]@{ files = 'www/assets/index-*.js'; pattern = 'FAKE-LIVE-URL'; replacement = 'x.x.x.x'; what = '示例地址' })
    $r = Invoke-Release $fake
    Assert-Equal 0 $r.Code "退出码（$($r.Text)）"
    Assert-Equal 0 @(Find-InStage $r.Stage 'FAKE-LIVE-URL').Count '包里没有那个字样了'
    Assert-Equal 2 $r.Manifest.redactions[0].hits '清单里记了命中 2 处'
    Assert-Equal 'www/assets/index-1.js' (@($r.Manifest.redactions[0].files) -join ',') '清单里记了动了哪个文件'
    Assert-Match ([System.IO.File]::ReadAllText((Join-Path $fake.Src 'www\assets\index-1.js'))) 'FAKE-LIVE-URL' '来源没动'

    $fake = New-FakeRelease
    $fake.Spec['redactions'] = @([ordered]@{ files = 'www/assets/index-*.js'; pattern = 'NO-SUCH-TEXT'; replacement = ''; what = '不存在的字样' })
    $r = Invoke-Release $fake
    Assert-Equal 2 $r.Code '一处都没命中：退出码 2'
}

Test '压缩：zip、校验值文件、另存的清单都对；已经有同名的 zip 时不覆盖，加 -Force 才覆盖' {
    $fake = New-FakeRelease
    $r = Invoke-Release $fake @('-OutDir', $fake.Out)
    Assert-Equal 0 $r.Code "退出码（$($r.Text)）"
    $zip = Join-Path $fake.Out 'fake-package-test.zip'
    Assert-True (Test-Path -LiteralPath $zip -PathType Leaf) 'zip 在'
    $shaLine = ([System.IO.File]::ReadAllText("$zip.sha256")).Trim()
    Assert-Equal "$(Get-RelHash $zip)  fake-package-test.zip" $shaLine '校验值文件里写的就是 zip 现算的校验值'
    Assert-True (Test-Path -LiteralPath (Join-Path $fake.Out 'fake-package-test.release.json')) '清单另存了一份'
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::OpenRead($zip)
    try { $entries = @($archive.Entries | ForEach-Object { $_.FullName }) } finally { $archive.Dispose() }
    $want = @($r.Manifest.files | ForEach-Object { $_.path }) + 'release.json'
    Assert-Equal (@($want | Sort-Object) -join '|') (@($entries | Sort-Object) -join '|') 'zip 里就是清单里的文件加清单自己'
    Assert-True ($entries -notcontains '.stack-release-stage') '摆包目录的记号没有进 zip'
    Assert-True ($entries -contains '开始.cmd') '中文文件名在 zip 里是对的'

    $before = Get-RelHash $zip
    $r2 = Invoke-Release $fake @('-OutDir', $fake.Out)
    Assert-Equal 2 $r2.Code '已经有同名的 zip：退出码 2'
    Assert-Equal $before (Get-RelHash $zip) '原来的 zip 没被动'
    $r3 = Invoke-Release $fake @('-OutDir', $fake.Out, '-Force')
    Assert-Equal 0 $r3.Code "加 -Force：退出码（$($r3.Text)）"
}
