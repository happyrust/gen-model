$ErrorActionPreference = 'Stop'

$source = Get-Content -LiteralPath "$PSScriptRoot\GenModelRvmExport.cs" -Raw
$expected = @(
    'cmds.Add("repre lev " + level);'
    'cmds.Add("repre lev pipe " + level);'
    'cmds.Add("repre lev nozz " + level);'
    'cmds.Add("repre lev struc " + level);'
    'cmds.Add("repre lev hvac " + level);'
)

foreach ($command in $expected) {
    if (-not $source.Contains($command)) {
        throw "RVM representation command missing: $command"
    }
}

'PASS: RVM export applies GENMODEL_RVM_LEVEL to general, PIPE, NOZZ, STRUC, and HVAC representations.'
