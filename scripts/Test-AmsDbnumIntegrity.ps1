[CmdletBinding()]
param(
    [int[]]$Dbnums = @(7997, 7999, 8000),
    [string]$Endpoint = "http://127.0.0.1:8009/sql",
    [string]$Namespace = "1516",
    [string]$Database = "AvevaMarineSample",
    [string]$User = "root",
    [string]$Password = "root"
)

$ErrorActionPreference = "Stop"

function Invoke-SurrealSql {
    param([Parameter(Mandatory)][string]$Sql)

    $pair = "${User}:${Password}"
    $auth = [Convert]::ToBase64String([Text.Encoding]::ASCII.GetBytes($pair))
    $headers = @{
        Accept       = "application/json"
        Authorization = "Basic $auth"
        "Surreal-NS" = $Namespace
        "Surreal-DB" = $Database
    }
    $raw = Invoke-WebRequest `
        -Method Post `
        -Uri $Endpoint `
        -Headers $headers `
        -ContentType "application/surrealql" `
        -UseBasicParsing `
        -Body $Sql
    $statements = @(ConvertFrom-Json -InputObject $raw.Content)
    [pscustomobject]@{ statements = $statements }
}

$failures = [System.Collections.Generic.List[string]]::new()
$rows = foreach ($dbnum in $Dbnums) {
    $sql = @"
SELECT count() AS pe_count FROM pe WHERE dbnum = $dbnum GROUP ALL;
SELECT math::sum(count) AS info_count FROM dbnum_info_table WHERE dbnum = $dbnum GROUP ALL;
SELECT applied_sesno, file_latest_sesno FROM dbnum_watermark:$dbnum;
"@
    $response = @()
    foreach ($item in (Invoke-SurrealSql -Sql $sql).statements) {
        if ($item -is [array]) {
            $response += $item
        } else {
            $response += ,$item
        }
    }
    foreach ($statement in $response) {
        if ($statement.status -ne "OK") {
            $failures.Add("dbnum=$dbnum Surreal statement failed: $($statement.result)")
        }
    }

    $peCount = if (@($response[0].result).Count) {
        [int64]$response[0].result[0].pe_count
    } else {
        0
    }
    $infoCount = if (@($response[1].result).Count) {
        [int64]$response[1].result[0].info_count
    } else {
        0
    }
    $watermark = @($response[2].result)
    $applied = if ($watermark.Count) { [int]$watermark[0].applied_sesno } else { 0 }
    $latest = if ($watermark.Count) { [int]$watermark[0].file_latest_sesno } else { 0 }

    if ($peCount -le 0) {
        $failures.Add("dbnum=$dbnum has no PE baseline")
    }
    if ($peCount -ne $infoCount) {
        $failures.Add("dbnum=$dbnum PE/stat mismatch: pe=$peCount info=$infoCount")
    }
    if ($applied -ne $latest) {
        $failures.Add("dbnum=$dbnum watermark mismatch: applied=$applied latest=$latest")
    }

    [pscustomobject]@{
        dbnum             = $dbnum
        pe_count          = $peCount
        info_count        = $infoCount
        applied_sesno     = $applied
        file_latest_sesno = $latest
    }
}

$rows | Format-Table -AutoSize
if ($failures.Count) {
    throw "AMS DBNUM integrity failed:`n - $($failures -join "`n - ")"
}

Write-Host "AMS DBNUM integrity passed for: $($Dbnums -join ', ')"
