#Requires -Version 7
<#
.SYNOPSIS
「回退到水位」三组谓词的索引量测（只读；ADR-072 D2 / 计划 §4.3 / §9-7，任务 t-468）。

.DESCRIPTION
ADR-072 D2 把回退从「SELECT id FROM pe … 再逐键删」改成纯谓词 DELETE。谓词删在
SurrealDB 上**拿不到二级索引**是已记录的事故（room_model::render_room_relate_write 的
头注：10 万条边上 `DELETE … WHERE in = X` 3.132s vs 图遍历 244.973ms），所以落地之前要
先量一遍：这三组谓词各自走不走索引、走不了要补哪几条 DEFINE INDEX。

本脚本**只发 SELECT / EXPLAIN / INFO**，一条写都没有（验收②的 grep 钉就钉这一点）。
量的是谓词本身的选择路径：`DELETE t WHERE p` 与 `SELECT … FROM t WHERE p` 走同一套
迭代器选择（`Iterate Index` / `Iterate Table`），所以 EXPLAIN 一条 SELECT 就等于量了那条
DELETE，而且不动库。

.PARAMETER Dbnums
要量的库号。任务点名 7997 / 1112；库里没有的会原样报 `absent`，不假装量过。

.EXAMPLE
pwsh -File scripts/Measure-RollbackPredicateIndexes.ps1 -Endpoint http://127.0.0.1:8010/sql

.NOTES
输出直接贴进 docs/evidence/2026-09-10-rollback-predicate-index-measurement.md。
库不在线时脚本照常退出并打印 NOT RUN 原因，不留半份证据。
#>
[CmdletBinding()]
param(
    [string]$Endpoint = "http://127.0.0.1:8010/sql",
    [string]$Namespace = "1516",
    [string]$Database = "AvevaMarineSample",
    [string]$User = "root",
    [string]$Password = "root",
    [int[]]$Dbnums = @(7997, 1112),
    # 每个库的 S 取「当前水位的一半」：既保证谓词能选中一大段行（量的是真扫描），
    # 又不至于退到空集。显式给值时按给的来。
    [int]$Sesno = 0
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

function Invoke-Sql([string]$Sql) {
    $raw = Invoke-WebRequest -Method Post -Uri $Endpoint -Headers $headers `
        -ContentType "application/surrealql" -UseBasicParsing -Body $Sql
    $raw.Content | ConvertFrom-Json
}

function Write-Section([string]$Title) {
    ""
    "### $Title"
    ""
}

"# 回退谓词索引量测（只读）"
""
"- endpoint: $Endpoint  ns/db: $Namespace/$Database"
"- 时刻: $(Get-Date -Format o)"

try {
    $ping = Invoke-Sql "RETURN 1;"
    if ($ping[0].status -ne "OK") { throw "ping 未返回 OK: $($ping[0] | ConvertTo-Json -Compress)" }
}
catch {
    "- **NOT RUN**：连不上持久层（$($_.Exception.Message)）。脚本原样留下，库起来后重跑。"
    exit 0
}

# ── 1. 索引清单 ─────────────────────────────────────────────────────────────
Write-Section "1. 相关表的索引清单（INFO FOR TABLE）"
foreach ($table in @("pe", "inst_relate", "inst_info", "tubi_relate", "gen_root", "pe_owner", "ref_rev")) {
    $info = Invoke-Sql "INFO FOR TABLE $table;"
    $indexes = $info[0].result.indexes
    if ($null -eq $indexes -or $indexes.PSObject.Properties.Count -eq 0) {
        "- ``$table``：无索引"
    }
    else {
        "- ``$table``："
        foreach ($property in $indexes.PSObject.Properties) {
            "  - $($property.Value)"
        }
    }
}

# ── 2. 库存量 ───────────────────────────────────────────────────────────────
Write-Section "2. 库存量与水位"
$present = @()
foreach ($dbnum in $Dbnums) {
    $rows = Invoke-Sql @"
SELECT count() AS n FROM pe WHERE dbnum = $dbnum GROUP ALL;
SELECT applied_sesno, model_sesno, room_sesno FROM dbnum_watermark:$dbnum;
SELECT math::min(sesno) AS lo, math::max(sesno) AS hi FROM pe WHERE dbnum = $dbnum GROUP ALL;
"@
    $count = if ($rows[0].result.Count -gt 0) { $rows[0].result[0].n } else { 0 }
    if ($count -eq 0) {
        "- dbnum=$dbnum：**absent**（本实例里没有这个库的 pe 行，本轮量不了）"
        continue
    }
    $mark = if ($rows[1].result.Count -gt 0) { $rows[1].result[0] } else { $null }
    $span = $rows[2].result[0]
    $applied = if ($mark) { [int]$mark.applied_sesno } else { [int]$span.hi }
    $s = if ($Sesno -gt 0) { $Sesno } else { [Math]::Max(0, [int]($applied / 2)) }
    $present += [pscustomobject]@{ Dbnum = $dbnum; Count = $count; Applied = $applied; S = $s }
    "- dbnum=$dbnum：pe $count 行，applied_sesno=$applied，sesno 跨度 $($span.lo)..$($span.hi)，本轮 S=$s"
}
if ($present.Count -eq 0) {
    ""
    "**NOT RUN**：点名的库在本实例里一个都没有。"
    exit 0
}

# ── 3. 谓词 EXPLAIN ─────────────────────────────────────────────────────────
#
# 每条谓词两个变体：`<int>sesno`（今天 prune_above_watermark 的写法）与裸 `sesno`。
# 转型写在列上会让优化器整条放弃索引，这一对就是用来把那件事量出来的。
Write-Section "3. 谓词 EXPLAIN（回退真正要发的那三组）"
foreach ($row in $present) {
    $dbnum = $row.Dbnum
    $s = $row.S
    "#### dbnum=$dbnum（S=$s）"
    ""
    $probes = @(
        @{ Name = "数据面 pe（带 <int> 转型，今天的写法）"; Sql = "SELECT count() AS n FROM pe WHERE dbnum = $dbnum AND <int>sesno > $s GROUP ALL" },
        @{ Name = "数据面 pe（裸 sesno）"; Sql = "SELECT count() AS n FROM pe WHERE dbnum = $dbnum AND sesno > $s GROUP ALL" },
        @{ Name = "数据面 pe（只按 dbnum）"; Sql = "SELECT count() AS n FROM pe WHERE dbnum = $dbnum GROUP ALL" },
        @{ Name = "模型面 inst_relate"; Sql = "SELECT count() AS n FROM inst_relate WHERE dbnum = $dbnum AND direct_model.sesno > $s GROUP ALL" },
        @{ Name = "模型面 inst_info"; Sql = "SELECT count() AS n FROM inst_info WHERE dbnum = $dbnum AND direct_model.sesno > $s GROUP ALL" },
        @{ Name = "模型面 tubi_relate"; Sql = "SELECT count() AS n FROM tubi_relate WHERE dbnum = $dbnum AND direct_model.sesno > $s GROUP ALL" },
        @{ Name = "凭证 gen_root"; Sql = "SELECT count() AS n FROM gen_root WHERE dbnum = $dbnum AND source_end_sesno > $s GROUP ALL" },
        @{ Name = "房间面 room_relate（谓词落在 in.dbnum）"; Sql = "SELECT count() AS n FROM room_relate WHERE in.dbnum = $dbnum GROUP ALL" },
        @{ Name = "房间面 room_panel_relate（谓词落在 in.dbnum）"; Sql = "SELECT count() AS n FROM room_panel_relate WHERE in.dbnum = $dbnum GROUP ALL" }
    )
    foreach ($probe in $probes) {
        # 两种 EXPLAIN 都要：
        # * `count() … GROUP ALL` 会走 `Iterate Index Count` 那条**只读索引计数**的快路，
        #   它连行都不迭代，于是把 `sesno > S` 这一半直接吞掉——计数看着走了索引，其实
        #   压根没执行第二个条件（本轮实测：裸 sesno 的计数与「只按 dbnum」一模一样）。
        # * `SELECT id … LIMIT 5` 必须真迭代行，这才是 `DELETE t WHERE p` 走的那条路。
        #   量索引就看这一条。
        $rowSql = $probe.Sql -replace '^SELECT count\(\) AS n FROM ', 'SELECT VALUE id FROM ' `
            -replace ' GROUP ALL$', ' LIMIT 5'
        $result = Invoke-Sql "$($probe.Sql);`n$($probe.Sql) EXPLAIN;`n$rowSql EXPLAIN;"
        $n = if ($result[0].result.Count -gt 0) { $result[0].result[0].n } else { 0 }
        $plan = { param($rows) ($rows | ForEach-Object {
                    "$($_.operation) $($_.detail | ConvertTo-Json -Compress -Depth 6)"
                }) -join " | " }
        "- **$($probe.Name)**"
        "  - count 行数 $n，耗时 $($result[0].time)"
        "  - count 计划（可能走索引计数快路，不代表逐行删也走索引）：``$(& $plan $result[1].result)``"
        "  - **逐行计划（= DELETE 走的那条）：``$(& $plan $result[2].result)``**"
    }
    ""
}

# ── 4. 结论 ────────────────────────────────────────────────────────────────
Write-Section "4. 读到这里请自己看计划里的 operation"
"``Iterate Index`` = 走了索引；``Iterate Table`` = 整表扫。"
"``Fallback`` 那一行的 reason 会点名是哪个表达式让优化器放弃了索引。"
