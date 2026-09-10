#Requires -Version 7
<#
.SYNOPSIS
量「删 `pe` 顶点会不会把挂在它身上的边一并带走」（ADR-072 D2 / 计划 §4.3-2，任务 t-468）。

.DESCRIPTION
回退到水位的数据面那一句是 `DELETE pe WHERE dbnum = X AND sesno > S`。计划 §4.3-2 的原话是
「`pe` 是图顶点，删它级联掉 `pe_owner` / `ref_rev` / `tubi_relate` 等挂在它身上的边——回退
恰好要这个级联」。这条脚本把那句话在 SurrealDB 2.1.4 上真跑一遍，顺带回答一个计划没问的
问题：级联带走的边里，**有没有本来该留着的**。

它起一个**全新的一次性 `memory` 实例**（不连任何项目库，退出前 kill 掉），所以带写。
只读的那一半在 `Measure-RollbackPredicateIndexes.ps1`，两者刻意分开：验收②的
「脚本只含 SELECT/EXPLAIN/INFO」钉的是那一个文件。

.EXAMPLE
pwsh -File scripts/Test-PeVertexCascadeOnMem.ps1
#>
[CmdletBinding()]
param(
    [string]$SurrealExe = "D:\work\plant-code\old\gen-model\bin\surreal.exe",
    [int]$Port = 8199
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path $SurrealExe)) {
    "**NOT RUN**：找不到 surreal 可执行文件 $SurrealExe"
    exit 0
}

$endpoint = "http://127.0.0.1:$Port/sql"
$auth = [Convert]::ToBase64String([Text.Encoding]::ASCII.GetBytes("root:root"))
$headers = @{
    Accept        = "application/json"
    Authorization = "Basic $auth"
    "Surreal-NS"  = "cascade"
    "Surreal-DB"  = "probe"
}

function Invoke-Sql([string]$Sql) {
    $raw = Invoke-WebRequest -Method Post -Uri $endpoint -Headers $headers `
        -ContentType "application/surrealql" -UseBasicParsing -Body $Sql
    $raw.Content | ConvertFrom-Json
}

$server = Start-Process -FilePath $SurrealExe -PassThru -WindowStyle Hidden `
    -ArgumentList @("start", "--user", "root", "--pass", "root", "--bind", "127.0.0.1:$Port", "memory")
try {
    $ready = $false
    foreach ($_ in 1..40) {
        Start-Sleep -Milliseconds 250
        try { Invoke-Sql "RETURN 1;" | Out-Null; $ready = $true; break } catch { }
    }
    if (-not $ready) { "**NOT RUN**：一次性 memory 实例没起来（端口 $Port）"; exit 0 }

    "# 删 pe 顶点的级联范围（一次性 mem 实例，SurrealDB $(& $SurrealExe version))"
    ""

    # 夹具：一个属主 31 名下挂着两个孩子 30 / 33；30 还带一条 tubi_relate 出边与一条
    # ref_rev 入边。窗口只动 30（sesno 43），31 与 33 停在 41——这正是「Modified 元素的
    # 属主没进窗口」那一档。
    Invoke-Sql @"
DEFINE TABLE pe SCHEMALESS;
DEFINE TABLE pe_owner TYPE RELATION IN pe OUT pe SCHEMALESS;
DEFINE TABLE tubi_relate TYPE RELATION SCHEMALESS;
DEFINE TABLE ref_rev TYPE RELATION SCHEMALESS;
INSERT INTO pe [
  { id: pe:c30, dbnum: 8000, sesno: 43, noun: 'BRAN', owner: pe:o31 },
  { id: pe:c33, dbnum: 8000, sesno: 41, noun: 'BRAN', owner: pe:o31 },
  { id: pe:o31, dbnum: 8000, sesno: 41, noun: 'ZONE' }];
INSERT RELATION INTO pe_owner [
  { id: pe_owner:[pe:o31, 0], in: pe:c30, out: pe:o31 },
  { id: pe_owner:[pe:o31, 1], in: pe:c33, out: pe:o31 }];
INSERT RELATION INTO tubi_relate [{ id: tubi_relate:[pe:c30, 0], in: pe:c30, out: pe:o31 }];
INSERT RELATION INTO ref_rev [{ id: ref_rev:r1, in: pe:o31, out: pe:c30 }];
"@ | Out-Null

    function Show-State([string]$Label) {
        $rows = Invoke-Sql @"
SELECT VALUE <string>id FROM pe;
SELECT VALUE <string>id FROM pe_owner;
SELECT VALUE <string>id FROM tubi_relate;
SELECT VALUE <string>id FROM ref_rev;
"@
        "- **$Label**"
        "  - pe: $($rows[0].result -join ', ')"
        "  - pe_owner: $($rows[1].result -join ', ')"
        "  - tubi_relate: $($rows[2].result -join ', ')"
        "  - ref_rev: $($rows[3].result -join ', ')"
    }

    Show-State "删之前"
    $deleted = Invoke-Sql "DELETE pe WHERE dbnum = 8000 AND sesno > 42;"
    "- 回退那一句：``DELETE pe WHERE dbnum = 8000 AND sesno > 42;``（只命中 pe:c30），耗时 $($deleted[0].time)"
    Show-State "删之后"

    ""
    "结论看 `pe_owner`：`pe_owner:[pe:o31, 0]` 的 `in` 指着被删的 c30。它还在不在，就是"
    "「回退删顶点会不会把属主名下那条成员边一并带走」的答案；`[pe:o31, 1]`（指着没被删的"
    "c33）是对照组，必须原样留着。"
}
finally {
    if ($server -and -not $server.HasExited) { Stop-Process -Id $server.Id -Force }
}
