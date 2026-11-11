# 生成配置：后端配置「只改指定的键」、账号口令两行一字不动；rs-server 的配置清空该清空的；nginx 的配置路由对。
# 用一份假的模板（不是真的出厂模板，里面的「口令」是摆样子的字样）。

$script:GenSaid = New-Object System.Collections.ArrayList
function Say([string]$Text = '', [string]$Color = '') { [void]$script:GenSaid.Add($Text) }

$script:GenTemplate = @'
# 假的后端配置模板，只给测试用。
project_path = "D:/old/path"   # 工程根目录
project_name = "OldName"
project_code = "0000"
surreal_ns = "0000"
mdb_name = "OLD"
e3d_version = "e3d210"
v_ip = "localhost"
v_port = "8009"
v_user = "fixture-user"
v_password = "fixture-not-a-real-secret"
http_api_addr = "0.0.0.0:9099"
server_release_ip = "0.0.0.0:9099"
ip = "mysql.example.invalid"
port = "3306"
user = "fixture-mysql-user"
password = "fixture-not-a-real-secret-2"
puhua_database_ip = "puhua.example.invalid"
puhua_database_user = "fixture-puhua-user"
puhua_database_password = "fixture-not-a-real-secret-3"
plat_url = "http://platform.example.invalid"
gen_model = false
other_number = 42   # 行尾注释要留着

[some_table]
keep = "as is"
'@

# 摆一个假的安装目录：配置、模板、两份函数脚本。返回 @{ Root; Config }
function New-GenRoot([string]$Extra = '') {
    $root = New-TestDir 'gen'
    Write-TestFile (Join-Path $root 'config\DbOption.factory.toml') $script:GenTemplate
    Write-TestFile (Join-Path $root 'resource\surreal\common.surql') '-- 假的函数脚本'
    Write-TestFile (Join-Path $root 'rs-server\resource\surreal\common.surql') '-- 假的函数脚本'
    Write-TestFile (Join-Path $root 'rs-server\assets\metadata\index.html') '<html></html>'
    Write-TestFile (Join-Path $root 'stack.toml') (@'
[project]
e3d_root = "e3d"
name = "Sample"
code = 1516
e3d_version = "e3d31"
mdb = "ALL"

[database]
exe = "bin/surreal.exe"
port = 51009

[backend]
exe = "bin/aios-database.exe"
port = 51022
config_template = "config/DbOption.factory.toml"
resource_dir = "resource"
schema_root = "schema"

[rs_server]
exe = "bin/rs-server.exe"
port = 51098
template_dir = "rs-server"

[frontend]
kind = "nginx"
exe = "bin/nginx/nginx.exe"
root = "www"
port = 51100
'@ + "`n" + $Extra)
    return @{ Root = $root; Config = (Join-Path $root 'stack.toml') }
}
function Read-Lines([string]$Path) { return @([System.IO.File]::ReadAllLines($Path, [System.Text.Encoding]::UTF8)) }
function Get-Line($Lines, [string]$Key) { return @($Lines | Where-Object { $_ -match ('^' + [regex]::Escape($Key) + '\s*=') })[0] }

Test '后端配置：只改入口管的键，账号口令两行和别的行一字不动' {
    $script:GenSaid.Clear()
    $env0 = New-GenRoot
    $cfg = Get-StackConfig $env0.Config
    Initialize-BackendFiles $cfg
    $template = @($script:GenTemplate -split "`r?`n")
    $made = Read-Lines $cfg.Backend.ConfigFile
    # 账号口令：逐字相同
    Assert-Equal (Get-Line $template 'v_user') (Get-Line $made 'v_user') 'v_user 一字不动'
    Assert-Equal (Get-Line $template 'v_password') (Get-Line $made 'v_password') 'v_password 一字不动'
    foreach ($key in 'user', 'password', 'puhua_database_user', 'puhua_database_password', 'ip', 'plat_url', 'gen_model') {
        Assert-Equal (Get-Line $template $key) (Get-Line $made $key) "$key 一字不动（后端这份配置里入口不碰它）"
    }
    Assert-Equal 'other_number = 42   # 行尾注释要留着' (Get-Line $made 'other_number') '不归入口管的键原样'
    Assert-True (@($made) -contains 'keep = "as is"') '别的节原样'
    # 入口管的键：换成了配置里的值；原来带引号的数字继续带引号；行尾注释留着
    $e3d = (Join-Path $env0.Root 'e3d').Replace('\', '/')
    Assert-Equal "project_path = `"$e3d`"   # 工程根目录" (Get-Line $made 'project_path') '工程根目录换了，行尾注释留着'
    Assert-Equal 'project_name = "Sample"' (Get-Line $made 'project_name') '项目名'
    Assert-Equal 'project_code = "1516"' (Get-Line $made 'project_code') '项目编码：模板里带引号，就继续带引号'
    Assert-Equal 'surreal_ns = "1516"' (Get-Line $made 'surreal_ns') '命名空间'
    Assert-Equal 'mdb_name = "ALL"' (Get-Line $made 'mdb_name') 'MDB'
    Assert-Equal 'e3d_version = "e3d31"' (Get-Line $made 'e3d_version') 'E3D 版本'
    Assert-Equal 'v_ip = "127.0.0.1"' (Get-Line $made 'v_ip') '数据库地址'
    Assert-Equal 'v_port = "51009"' (Get-Line $made 'v_port') '数据库端口'
    Assert-Equal 'http_api_addr = "0.0.0.0:51022"' (Get-Line $made 'http_api_addr') '后端监听地址'
    Assert-Equal 'server_release_ip = "0.0.0.0:51022"' (Get-Line $made 'server_release_ip') '后端发布地址'
    Assert-Equal 'store_mode = "rocksdb"' (Get-Line $made 'store_mode') '模板里没有的键：插进去'
    Assert-Equal 'auto_initialize_new_dbnums = false' (Get-Line $made 'auto_initialize_new_dbnums') '启动时不自动初始化'
    $rvm = (Join-Path $env0.Root 'data\rvm-check')
    Assert-True (@($made) -contains '[rvm_check]') '加了 [rvm_check] 一节'
    Assert-Equal "work_dir = '$rvm'" (Get-Line $made 'work_dir') '「核对」的产物目录'
    # 模板里的每一行，要么原样在生成的文件里，要么是入口管的那几个键
    $managed = 'project_path', 'project_name', 'project_code', 'surreal_ns', 'mdb_name', 'e3d_version', 'v_ip', 'v_port', 'http_api_addr', 'server_release_ip'
    foreach ($line in $template) {
        $isManaged = $false
        foreach ($key in $managed) { if ($line -match ('^' + $key + '\s*=')) { $isManaged = $true } }
        if (-not $isManaged) { Assert-True (@($made) -ccontains $line) "模板里的这一行原样在：$line" }
    }
    Assert-Equal ($template.Count + 6) $made.Count '多出来的行：开头一行说明、两个新键、一个空行、[rvm_check] 一节两行'
    Assert-True (Test-Path -LiteralPath (Join-Path $cfg.Backend.WorkDir 'resource\surreal\common.surql')) '函数脚本拷进了后端的工作目录'
}

Test '后端配置：只生成一次；stack.toml 后来改了只提醒不覆盖；库地址对不上不起；模板多出来的键会说' {
    $script:GenSaid.Clear()
    $env0 = New-GenRoot
    $cfg = Get-StackConfig $env0.Config
    Initialize-BackendFiles $cfg
    $first = [System.IO.File]::ReadAllText($cfg.Backend.ConfigFile)
    $script:GenSaid.Clear()
    Initialize-BackendFiles $cfg
    Assert-Equal $first ([System.IO.File]::ReadAllText($cfg.Backend.ConfigFile)) '再来一次：文件没变'
    Assert-NotMatch ($script:GenSaid -join "`n") '注意' '配置没变的时候不提醒'

    # stack.toml 里把项目名改了：提醒，但不替人改
    $text = [System.IO.File]::ReadAllText($env0.Config).Replace('name = "Sample"', 'name = "Other"')
    Write-TestFile $env0.Config $text
    $script:GenSaid.Clear()
    Initialize-BackendFiles (Get-StackConfig $env0.Config)
    Assert-Equal $first ([System.IO.File]::ReadAllText($cfg.Backend.ConfigFile)) '改了项目名：后端配置还是原来那份'
    Assert-Match ($script:GenSaid -join "`n") 'project_name：stack\.toml 要的是「Other」，后端配置里是「Sample」' '提醒了哪个键不一样'

    # 数据库端口改了：两边连的不是同一个库，不起
    Write-TestFile $env0.Config ($text.Replace('port = 51009', 'port = 51010'))
    [void](Assert-StackError { Initialize-BackendFiles (Get-StackConfig $env0.Config) } 2 '两边不一致，不起后端' '数据库端口对不上')

    # 新一版模板里多了一个键：说出键名
    Write-TestFile $env0.Config ([System.IO.File]::ReadAllText($env0.Config).Replace('port = 51010', 'port = 51009').Replace('name = "Other"', 'name = "Sample"'))
    Write-TestFile (Join-Path $env0.Root 'config\DbOption.factory.toml') ($script:GenTemplate.Replace('gen_model = false', "gen_model = false`nbrand_new_key = 1"))
    $script:GenSaid.Clear()
    Initialize-BackendFiles (Get-StackConfig $env0.Config)
    Assert-Match ($script:GenSaid -join "`n") '现有的后端配置里没有：brand_new_key' '说出了模板里多出来的键'
    Assert-Equal $first ([System.IO.File]::ReadAllText($cfg.Backend.ConfigFile)) '但没有替人改'
}

Test 'rs-server 的配置：库地址、账号照后端那份；MySQL 和平台几项清空；[rvm_check] 不带过去' {
    $script:GenSaid.Clear()
    $env0 = New-GenRoot
    $cfg = Get-StackConfig $env0.Config
    Initialize-BackendFiles $cfg
    Initialize-RsServerFiles $cfg
    $backend = Read-Lines $cfg.Backend.ConfigFile
    $rs = Read-Lines (Join-Path $cfg.RsServer.WorkDir 'DbOption.toml')
    foreach ($key in 'v_ip', 'v_port', 'v_user', 'v_password', 'surreal_ns', 'project_name', 'project_code') {
        Assert-Equal (Get-Line $backend $key) (Get-Line $rs $key) "$key 和后端那份逐字相同"
    }
    foreach ($key in 'ip', 'user', 'password', 'puhua_database_ip', 'puhua_database_user', 'puhua_database_password', 'plat_url') {
        Assert-Equal "$key = `"`"" (Get-Line $rs $key) "$key 清空了"
    }
    Assert-Equal 'server_release_ip = "0.0.0.0:51098"' (Get-Line $rs 'server_release_ip') '它自己的监听地址'
    Assert-Equal 'aios_database_url = "http://127.0.0.1:51022"' (Get-Line $rs 'aios_database_url') '后端的地址'
    Assert-True (-not (@($rs) -contains '[rvm_check]')) '[rvm_check] 一节没有带过去'
    Assert-True (@($rs) -contains 'keep = "as is"') '别的节还在'
    Assert-True (Test-Path -LiteralPath (Join-Path $cfg.RsServer.WorkDir 'resource\surreal\common.surql')) '运行文件拷进了它的运行目录'
    Assert-True (Test-Path -LiteralPath (Join-Path $cfg.RsServer.WorkDir 'assets\metadata\index.html')) '元数据网页拷进去了'
    # 它的环境变量：自动升级关着
    $spec = Get-ServiceSpec $cfg 'rs_server'
    Assert-Equal 'off' $spec.Env['AIOS_STORAGE_VERSION_UPGRADE'] '自动升级关着'
    Assert-Equal 'http://127.0.0.1:51022' $spec.Env['AIOS_DATABASE_URL'] '后端地址经环境变量给它'
}

Test '每个服务的环境变量由入口给定：不带调用方的，口令不在里面' {
    $env0 = New-GenRoot
    $cfg = Get-StackConfig $env0.Config
    $old = [Environment]::GetEnvironmentVariable('STACK_TEST_LEAK')
    [Environment]::SetEnvironmentVariable('STACK_TEST_LEAK', 'should-not-pass')
    try {
        foreach ($name in 'database', 'backend', 'rs_server', 'frontend') {
            $spec = Get-ServiceSpec $cfg $name
            Assert-True (-not $spec.Env.Contains('STACK_TEST_LEAK')) "$name 没有带上调用方环境里的变量"
            Assert-True (-not $spec.Env.Contains('SURREAL_PASS')) "$name 的说明里没有数据库口令（起的时候才读）"
            Assert-Equal (Join-Path $env0.Root 'tmp') $spec.Env['TEMP'] "$name 的临时目录在运行目录下面"
            Assert-NotMatch $spec.Env['PATH'] '(?i)cargo|nodejs|\\git\\' "$name 的 PATH 里只有系统目录"
        }
        $backend = Get-ServiceSpec $cfg 'backend'
        Assert-Equal $cfg.Backend.ConfigFile $backend.Env['DB_OPTION_FILE'] '后端的配置文件位置'
        Assert-Equal '0' $backend.Env['AIOS_OPEN_BROWSER'] '后端不自动开浏览器'
        $database = Get-ServiceSpec $cfg 'database'
        Assert-Equal 'start --no-banner --bind 127.0.0.1:51009 rocksdb:surreal' ($database.Args -join ' ') '数据库的启动参数'
        Assert-Equal @('SURREAL_USER', 'SURREAL_PASS') @($database.SecretNames) '数据库的账号口令是起的时候才读的两项'
    } finally {
        [Environment]::SetEnvironmentVariable('STACK_TEST_LEAK', $old)
    }
}
