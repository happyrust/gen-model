# 配置解析：写对了解析得出来、缺省值对；写错了报错（退出码 2），能指出行号的指出行号，不悄悄忽略。

$script:CfgBase = @'
[project]
e3d_root = "D:/e3d"
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
'@

# 把一段配置写成临时目录里的 stack.toml，返回路径。
function New-TestConfig([string]$Text) {
    $dir = New-TestDir 'cfg'
    $path = Join-Path $dir 'stack.toml'
    Write-TestFile $path $Text
    return $path
}
# 在基准配置上换掉一处（找不到要换的那一处就是测试写错了）。
function Edit-Base([string]$Find, [string]$Replace) {
    if ($script:CfgBase.IndexOf($Find) -lt 0) { throw "测试写错了：基准配置里没有「$Find」" }
    return $script:CfgBase.Replace($Find, $Replace)
}
function Assert-ConfigError([string]$Text, [string]$Pattern, [string]$What) {
    $path = New-TestConfig $Text
    [void](Assert-StackError { Get-StackConfig $path } 2 $Pattern $What)
}

Test '写对的：解析得出来，相对路径相对配置所在的目录，没写的用缺省值' {
    $path = New-TestConfig $script:CfgBase
    $root = Split-Path -Parent $path
    $cfg = Get-StackConfig $path
    Assert-Equal $root $cfg.Root '运行目录缺省是配置文件所在的目录'
    Assert-Equal (Join-Path $root 'bin\surreal.exe') $cfg.Database.Exe '相对路径相对运行目录'
    Assert-Equal (Join-Path $root 'data\surreal') $cfg.Database.DataDir '数据目录的缺省值'
    Assert-Equal (Join-Path $root 'work\backend\DbOption.toml') $cfg.Backend.ConfigFile '后端配置的位置'
    Assert-Equal (Join-Path $root 'data\rvm-check') $cfg.Backend.RvmCheckDir '「核对」产物目录的缺省值'
    Assert-Equal '127.0.0.1' $cfg.Database.Host '数据库缺省只听本机'
    Assert-Equal 'ALL' $cfg.Project.Mdb 'MDB 的缺省值'
    Assert-Equal 'False/False/False' "$($cfg.LoopbackOnly)/$($cfg.RsServer.Enabled)/$($cfg.Frontend.Enabled)" '没写 [listen]、[rs_server]、[frontend]'
    Assert-Equal '30/180/120/180/90/600' "$($cfg.Timeouts.DatabaseSec)/$($cfg.Timeouts.BackendSec)/$($cfg.Timeouts.RsServerSec)/$($cfg.Timeouts.StorageVersionSec)/$($cfg.Timeouts.FrontendSec)/$($cfg.Timeouts.InitSec)" '超时的缺省值'
    Assert-Equal 0 $cfg.Init.Dbnums.Count '缺省不点名初始化任何库'
    Assert-Equal $false $cfg.Init.AutoNew '缺省不自动初始化新库'
    Assert-Equal 'True/2' "$($cfg.Guard.RequireExeUnderRoot)/$($cfg.Guard.MinDiskFreeGb)" '护栏的缺省值'
    Assert-Equal @('database', 'backend') (Get-ServiceNames $cfg) '只有数据库和后端'
}

Test '写对的：BOM、CRLF、行尾注释、单引号里的反斜杠路径、一行写完的数组都认' {
    $text = [string][char]0xFEFF + ($script:CfgBase -replace "`r?`n", "`r`n")
    $text = $text.Replace('e3d_root = "D:/e3d"', "e3d_root = 'D:\AVEVA\Projects'   # 工程根目录")
    $text += "`r`n[init]`r`ndbnums = [7997, 8000]   # 点名`r`n`r`n[rs_server]`r`nexe = `"bin/rs-server.exe`"`r`nport = 51098`r`ntemplate_dir = `"rs-server`"`r`n"
    $cfg = Get-StackConfig (New-TestConfig $text)
    Assert-Equal 'D:\AVEVA\Projects' $cfg.Project.E3dRoot '单引号里的反斜杠原样'
    Assert-Equal @(7997, 8000) @($cfg.Init.Dbnums) '数组'
    Assert-Equal @('database', 'backend', 'rs_server') (Get-ServiceNames $cfg) '有 [rs_server] 这一节就算启用'
    Assert-Equal $true $cfg.RsServer.WaitStorageVersion '缺省等库版本'
}

Test '写错的：键名、节名拼错——报错并指出行号' {
    Assert-ConfigError (Edit-Base 'port = 51009' 'prot = 51009') '第 9 行.*不认识的键「prot」' '键名拼错'
    Assert-ConfigError (Edit-Base '[database]' '[databse]') '第 7 行.*不认识「databse」' '节名拼错'
    Assert-ConfigError ($script:CfgBase + "`n[backend.extra]`nx = 1`n") '不认识的键「extra」' '不认识的子节'
}

Test '写错的：类型不对、忘了引号、取值不在范围里——报错并指出行号' {
    Assert-ConfigError (Edit-Base 'port = 51022' 'port = "51022"') '第 13 行.*要写整数' '端口带了引号'
    Assert-ConfigError (Edit-Base 'name = "Sample"' 'name = Sample') '第 3 行.*不认识的值' '字符串忘了引号'
    Assert-ConfigError (Edit-Base 'e3d_version = "e3d31"' 'e3d_version = "e3d99"') '第 5 行.*只能是 e3d31 或 e3d210' 'E3D 版本写成别的'
    Assert-ConfigError (Edit-Base 'code = 1516' 'code = "1516"') '第 4 行.*要写整数' '项目编码带了引号'
    Assert-ConfigError ($script:CfgBase + "`n[init]`ndbnums = [7997, `"8000`"]`n") '要写整数' '数组里混了字符串'
    Assert-ConfigError ($script:CfgBase + "`n[timeouts]`nbackend_sec = 0`n") '第 18 行.*不小于 1' '超时写成 0'
    Assert-ConfigError ($script:CfgBase + "`n[listen]`nloopback_only = `"true`"`n") '要写 true 或 false' '布尔值带了引号'
}

Test '写错的：同一个键写两次、双引号里写反斜杠、多行数组、行内表——都不认' {
    Assert-ConfigError (Edit-Base 'port = 51009' "port = 51009`nport = 51010") '第 10 行.*「port」写了两次' '同一个键写两次'
    Assert-ConfigError (Edit-Base 'e3d_root = "D:/e3d"' 'e3d_root = "D:\AVEVA\Projects"') '第 2 行.*不认识的转义.*正斜杠' '双引号里的反斜杠路径'
    Assert-ConfigError ($script:CfgBase + "`n[init]`ndbnums = [7997,`n  8000]`n") '数组要写在一行里' '多行数组'
    Assert-ConfigError ($script:CfgBase + "`n[guard]`nx = { a = 1 }`n") '不支持行内表' '行内表'
    Assert-ConfigError ($script:CfgBase + "`nbackend.port = 1`n") '不支持带点的键' '带点的键'
    Assert-ConfigError (Edit-Base 'name = "Sample"' 'name = "Sample" extra') '第 3 行.*值后面多出了' '值后面多了东西'
}

Test '写错的：少了必填项、空文件、找不到文件' {
    Assert-ConfigError (Edit-Base "code = 1516`n" '') '\[project\] 缺必填项「code」' '少了项目编码'
    Assert-ConfigError (Edit-Base "config_template = `"config/DbOption.factory.toml`"`n" '') '\[backend\] 缺必填项「config_template」' '少了后端配置模板'
    Assert-ConfigError '' '缺必填项' '空文件'
    [void](Assert-StackError { Get-StackConfig (Join-Path (New-TestDir 'nofile') 'stack.toml') } 2 '找不到配置文件' '找不到文件')
    Assert-ConfigError ($script:CfgBase + "`n[rs_server]`nport = 51098`n") '\[rs_server\] 缺必填项「exe」' 'rs-server 启用了却没写程序'
    Assert-ConfigError ($script:CfgBase + "`n[frontend]`nkind = `"apache`"`nroot = `"www`"`nport = 51100`n") '只能是 nginx.*或 vite-dev' '前端形态写成别的'
    Assert-ConfigError ($script:CfgBase + "`n[frontend]`nkind = `"nginx`"`nroot = `"www`"`nport = 51100`n") '缺必填项「exe」' 'nginx 形态没写 nginx.exe'
}

Test '写错的：口令类的东西不许放进环境变量和 config_set；env 写成一个值不悄悄忽略' {
    Assert-ConfigError ($script:CfgBase + "`n[backend.env]`nDB_PASSWORD = `"x`"`n") '不要放口令类的环境变量' '环境变量里放口令'
    Assert-ConfigError ($script:CfgBase + "`n[backend.env]`nAPI_TOKEN = `"x`"`n") '不要放口令类的环境变量' '环境变量里放令牌'
    Assert-ConfigError ($script:CfgBase + "`n[backend.config_set]`nv_password = `"x`"`n") '不要放账号口令类的键' 'config_set 里放口令'
    Assert-ConfigError (Edit-Base 'schema_root = "schema"' "schema_root = `"schema`"`nenv = `"AIOS_X=1`"") '应当是单独的一节' 'env 写成了一个字符串'
    Assert-ConfigError ($script:CfgBase + "`n[backend.env]`n1BAD = `"x`"`n") '环境变量名「1BAD」不合法' '环境变量名不合法'
}

Test '前端两种形态、rs-server 的可选项都解析得出来' {
    $text = $script:CfgBase + @'

[rs_server]
exe = "bin/rs-server.exe"
port = 51098
template_dir = "rs-server"
wait_storage_version = false
public_url = "http://host.example.invalid:51098/"
cors_origins = ["http://a.example.invalid:8080"]

[frontend]
kind = "nginx"
exe = "bin/nginx/nginx.exe"
root = "www"
port = 51100

[listen]
loopback_only = true

[guard]
protect_ports = [8009, 8022]
'@
    $cfg = Get-StackConfig (New-TestConfig $text)
    Assert-Equal @('database', 'backend', 'rs_server', 'frontend') (Get-ServiceNames $cfg) '四个服务'
    Assert-Equal 'nginx' $cfg.Frontend.Kind '前端形态'
    Assert-Equal $false $cfg.RsServer.WaitStorageVersion '不等库版本'
    Assert-Equal 'http://host.example.invalid:51098' $cfg.RsServer.PublicUrl '末尾的斜杠去掉了'
    Assert-Equal 'http://host.example.invalid:51098' (Get-RsServerPublicUrl $cfg) '浏览器访问 rs-server 用配置里给的地址'
    Assert-Equal '127.0.0.1' (Get-BackendBindHost $cfg) '只许本机时后端听回环'
    Assert-Equal @(8009, 8022) @($cfg.Guard.ProtectPorts) '受保护端口'
    $origins = Get-RsServerCorsOrigins $cfg
    Assert-True ($origins -contains 'http://a.example.invalid:8080') '额外的跨域来源带上了'
    Assert-True ($origins -contains 'http://127.0.0.1:51100') '前端的地址在跨域白名单里'
    Assert-ConfigError ($text.Replace('cors_origins = ["http://a.example.invalid:8080"]', 'cors_origins = ["a.example.invalid/path"]')) 'cors_origins 里每一项要写成' '跨域来源写得不对'
}
