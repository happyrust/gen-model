# 准备网页检查用的 mock 根目录：装假 stack.ps1 和 stack.toml（fixtures\fake-stack.ps1），按 -UpgradeKey 开或关升级。可以重复跑。
# 有了假 stack.ps1，网页上的单服务升级才会真走「停 → 换 → 起」；没有就只换文件。
# webcheck.ps1 有一项验「没开升级时老接口回 403」，upload-check.ps1 又要开着升级，所以分开备：
#   pwsh tests\mock-root.ps1               关升级 → webcheck.ps1、webcheck-running.ps1
#   pwsh tests\mock-root.ps1 -UpgradeKey   开升级 → upload-check.ps1
# mock 代理起一次就行（它每次验签都重读密钥文件，开关升级不用重启）：
#   <ExeDir>\deploy-dash-agent.exe --root <Root> --listen 127.0.0.1:18691 --mock
param(
    # 缺省 tools\deploy-dash\target\test-run\mock（target 不进库）
    [string]$Root = (Join-Path $PSScriptRoot '..\target\test-run\mock'),
    # deploy-dash-agent.exe 所在的目录，缺省 tools\deploy-dash\target\debug
    [string]$ExeDir = (Join-Path $PSScriptRoot '..\target\debug'),
    # 开升级（没有升级密钥就生成一把）；不带就关掉
    [switch]$UpgradeKey
)
$ErrorActionPreference = 'Stop'
$Root = [IO.Path]::GetFullPath($Root)
$agentExe = Join-Path $ExeDir 'deploy-dash-agent.exe'
if (-not (Test-Path $agentExe)) { throw "缺代理 exe：$agentExe（cargo build -p deploy-dash --bin deploy-dash-agent --features headless-agent）" }
New-Item -ItemType Directory -Force -Path $Root | Out-Null

# 只覆盖自己装的假文件（认 deploy-dash-fake-stack 这个记号），-Root 指到真安装目录上就停下
$stack = Join-Path $Root 'stack.ps1'
$toml = Join-Path $Root 'stack.toml'
foreach ($f in $stack, $toml) {
    if ((Test-Path -LiteralPath $f) -and -not (Select-String -LiteralPath $f -SimpleMatch 'deploy-dash-fake-stack' -Quiet)) {
        throw "$f 不是测试用的假文件，不覆盖（-Root 指错了？）"
    }
}
Copy-Item (Join-Path $PSScriptRoot 'fixtures\fake-stack.ps1') $stack -Force
[IO.File]::WriteAllText($toml, "# 配 tests\fixtures\fake-stack.ps1 用（deploy-dash-fake-stack）；各服务的文件都在缺省位置（bin\、www\）`n[project]`nname = `"mock`"`n")
# 每次从「四个服务都在跑」开始
Remove-Item (Join-Path $Root 'run\fake-stack') -Recurse -Force -ErrorAction SilentlyContinue

# 升级密钥：已有就留着（不带 --rotate 的 upgrade-key 遇到已有的会回 1）；生成时输出里有密钥本身，不打印
if ($UpgradeKey) {
    & $agentExe upgrade-key --root $Root --id *> $null
    if ($LASTEXITCODE -ne 0) {
        & $agentExe upgrade-key --root $Root *> $null
        if ($LASTEXITCODE -ne 0) { throw "生成升级密钥失败（退出码 $LASTEXITCODE）" }
    }
} else {
    & $agentExe upgrade-key --root $Root --disable *> $null
    if ($LASTEXITCODE -ne 0) { throw "关升级失败（退出码 $LASTEXITCODE）" }
}
Write-Output "mock 根已备好（升级$(if ($UpgradeKey) { '开' } else { '关' })）：$Root"
