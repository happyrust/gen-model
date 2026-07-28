[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Sql,
    [string]$Endpoint = "http://127.0.0.1:8009/sql",
    [string]$Namespace = "1516",
    [string]$Database = "AvevaMarineSample",
    [string]$User = "root",
    [string]$Password = "root"
)

$ErrorActionPreference = "Stop"
$pair = "${User}:${Password}"
$auth = [Convert]::ToBase64String([Text.Encoding]::ASCII.GetBytes($pair))
$headers = @{
    Accept        = "application/json"
    Authorization = "Basic $auth"
    "Surreal-NS"  = $Namespace
    "Surreal-DB"  = $Database
}
$raw = Invoke-WebRequest -Method Post -Uri $Endpoint -Headers $headers `
    -ContentType "application/surrealql" -UseBasicParsing -Body $Sql
$raw.Content
