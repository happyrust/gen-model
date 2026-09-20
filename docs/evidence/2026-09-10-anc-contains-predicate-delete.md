# 量测：对账读 `existing_geometry_for_window` 改按根谓词删 `anc CONTAINS <root>`（SurrealDB 2.1.4，2026-09-10）

计划：`docs/plans/2026-09-10-retire-publish-stage-and-write-marker-plan.md` §4.5 / R2 / §9-7；ADR-072 D4 / W3。
任务：t-466「T2 R2 空间树就地收敛 + 树启动判据/快照 + 对账读→谓词删量测」。
只读量测：只发 `INFO` / `SELECT … EXPLAIN FULL` / `SELECT count()`，脚本里拒绝任何写动词（见 §5）。

## 0. 结论（go / no-go）

| 问题 | 结论 | 依据 |
|---|---|---|
| `anc CONTAINS <packed_root>` 在 SurrealDB 2.1.4 上能否走索引 | **go** —— `inst_relate` / `tubi_relate` 上走 `idx_*_anc`，计划器把 `CONTAINS` 降成索引 `=` 查找（`Iterate Index … operator "="`），代价 O(根下行数)，与表大小无关 | §2 B 组 EXPLAIN；1090 行 ≈ 22 ms |
| 计划 §4.5 写的删语句 `DELETE inst_relate / inst_info / tubi_relate WHERE dbnum = X AND anc CONTAINS <root>` 能否原样落地 | **no-go（本轮不改删那一半，保留 `existing_geometry_for_window` 对账读）** —— 「删那一半」要删的是 `inst_relate` / `tubi_relate` / `inst_info` / `aabb` / `trans` 五张表的行；后三张**没有 `anc`**（`inst_info` 连索引都没有，`aabb` / `trans` 只按记录 id 键、连 `dbnum` 都没有），谓词够不着；用子查询 `id IN (SELECT VALUE … WHERE anc CONTAINS r)` 补上时计划器**退化成全表扫**（`Fallback: Unsupported subquery` → `Iterate Table`），8404 行的小库上 `aabb` 一次 42–62 s | §2 C 组 EXPLAIN；§3 |
| 7997 真库上的数字 | **NOT RUN** —— 本机 `:8009`（`DbOption.toml` 指的那台）`inst_relate` 为空；`:8010` 只有 dbnum 1112 / 7324 的模型行。量测按 §5 的脚本在 `:8010` 上跑（同版本 2.1.4、同索引定义），7997 上线后原样 `-Endpoint http://127.0.0.1:8009/sql -Dbnum 7997` 复量即可 | §1 |

**为什么 no-go 不是 go**：索引这一问的答案是肯定的，但它只覆盖「删那一半」的两张表。要把对账读整个退掉，
剩下三张表得二选一——(a) 行上补 `anc`（`aabb` / `trans` 还要补 `dbnum`）并建索引、历史行回填，走纯谓词删；
(b) 同一事务里 `LET $rows = SELECT id, out, aabb, world_trans FROM inst_relate WHERE anc CONTAINS <root>;`
再 `DELETE $rows.aabb; DELETE $rows.world_trans; DELETE $rows.out; DELETE $rows.id;`（服务端一次索引游走、
不回客户端，但语句里仍有一条 `SELECT`，与 W3「其余全部是写或删」的字面口径要对一下）。两条都不是「顺手」
能改的：(a) 是 schema + 回填，(b) 要在 7997 上量 `DELETE <array>` 的形状（含 `out` 为 NONE 的烘焙行）并重写
`persist_group` 的首块。本轮按任务契约的 no-go 分支处理：`existing_geometry_for_window`（一任务一读、
按组就地推进的基线快照，2026-09-10 O(N²) 修复）原样保留，`GeometryBaseline` 不退；
`CataSeedMemo`（`data_interface/cata_dependency.rs`，同一条「绑数据窗口、换窗即失效」的记忆纪律）
随之保留——它记的是本窗已落过的 CATA seed，与对账读同进同退。

## 1. 环境

| 项 | 值 |
|---|---|
| 目标库（计划口径） | `http://127.0.0.1:8009/sql`，ns `1516` / db `AvevaMarineSample`（`DbOption.toml`：`v_port = 8009`） |
| 8009 实况 | `surrealdb-2.1.4+20250317.45013fc9`；`inst_relate` 0 行、`pe` 0 行 —— **7997 不在线** |
| 代跑库 | `http://127.0.0.1:8010/sql`，ns `1516` / db `AvevaMarineSample`；同版本 2.1.4 |
| 8010 实况 | `inst_relate` 8,404 行（dbnum 1112：4,702；7324：3,702）；`pe` 90,617 行；`tubi_relate`（1112）0 行 |
| 索引 | `inst_relate`：`idx_inst_relate_anc ON anc` / `idx_inst_relate_dbnum ON dbnum` / `idx_inst_relate_in ON in`；`tubi_relate` 同三条；**`inst_info` 无索引** |
| 量测根 | 1112 里 `published_geometry_count` 最大的 gen_root `17496/92413`（1,090 条几何），packed `75144747903229` |
| 重复 | 每条 SQL 3 次，取服务端返回的 `time` |

## 2. EXPLAIN FULL 与耗时

### A. 现有对账读（`existing_geometry_of_dbnums_on`，一库一次）

```sql
SELECT direct_model.geometry_id AS geometry_id, anc FROM inst_relate
 WHERE dbnum IN [1112] AND direct_model.source = 'e3d-model' AND direct_model.geometry_id != NONE
```

| 计划 | 行 | 时间 ×3 |
|---|---|---|
| `Iterate Index idx_inst_relate_dbnum (union [1112])` → Fetch 4702 | 4,702 | 123.2 / 121.3 / 121.7 ms |

一库整读，代价 O(库内 e3d-model 行数)；7997（≈17 万行）按比例约 4–5 s 一次。今天靠 `GeometryBaseline`
把它收成一任务一次（此前每组一次、68 组累计 180 万行是 O(N²) 的来源，见 `model_db_adapter.rs` 头注）。

### B. 按根谓词（有 `anc` 的两张表）—— **走索引**

| SQL | 计划 | 行 | 时间 ×3 |
|---|---|---|---|
| `SELECT id FROM inst_relate WHERE anc CONTAINS 75144747903229` | `Iterate Index idx_inst_relate_anc (= 75144747903229)` | 1,090 | 21.9 / 22.7 / 21.8 ms |
| `… WHERE dbnum = 1112 AND anc CONTAINS 75144747903229` | 同上（计划器选 `anc` 索引，`dbnum` 作过滤） | 1,090 | 21.8 / 22.0 / 22.8 ms |
| `SELECT id FROM tubi_relate WHERE anc CONTAINS 75144747903229` | `Iterate Index idx_tubi_relate_anc (=)` | 0 | 0.64 / 0.52 / 0.46 ms |
| `… tubi_relate WHERE dbnum = 1112 AND anc CONTAINS …` | 同上 | 0 | 0.67 / 0.47 / 0.45 ms |

`CONTAINS` 对数组字段的普通索引在 2.1.4 上被降成对单个元素值的 `=` 查找（数组索引按元素展开），
所以 22 ms 里读的就是那 1,090 行本身；`dbnum = X` 多写不多写不影响计划。这一问是 **go**。

### C. 「删那一半」的另外三张表 —— **子查询形态退化全表扫**

| SQL | 计划 | 行 | 时间 ×3 |
|---|---|---|---|
| `SELECT id FROM inst_info WHERE id IN (SELECT VALUE out FROM inst_relate WHERE anc CONTAINS 75144747903229)` | `Iterate Table inst_info` + `Fallback: Unsupported subquery` | 0（该根全是烘焙行，`out` 为 NONE） | 3.46 / 3.70 / 2.95 **s** |
| `SELECT id FROM aabb WHERE id IN (SELECT VALUE aabb FROM inst_relate WHERE anc CONTAINS 75144747903229)` | `Iterate Table aabb` + `Fallback: Unsupported subquery` | 1,090 | 45.4 / 44.3 / 42.7 **s**（首次 61.9 s） |
| `SELECT id FROM inst_info WHERE dbnum = 1112` | `Iterate Table inst_info`（无索引） | 0 | 2.8 / 2.6 / 2.5 ms |

`aabb` 表除每条记录的世界盒外还放共享图元的本地盒（按 mesh id 键），是几张表里最大的一张；
子查询被当作不可下推的谓词逐行求值，8,404 行 `inst_relate` 的小库就要 40 多秒，7997 上不可用。

## 3. 「删那一半」到底删几张表

`persist_group` / `persist_root_chunks_impl` 的首块（`append_geometry_removals` + `append_tube_branch_reset`）
按记录 id 删：

| 表 | 键 | 有 `dbnum` | 有 `anc` | 索引 |
|---|---|---|---|---|
| `inst_relate` | 记录 id | ✔ | ✔ | `anc` / `dbnum` / `in` |
| `tubi_relate` | `[pe:⟨branch⟩, 槽位]` | ✔ | ✔ | `anc` / `dbnum` / `in` |
| `inst_info` | 记录 id（共享图元行才有） | ✔ | ✘ | 无 |
| `aabb` | 记录 id（世界盒）/ mesh id（共享本地盒） | ✘ | ✘ | 无 |
| `trans` | 记录 id | ✘ | ✘ | 无 |

计划 §4.5 只点了前三张，且 `inst_info` 那一张没有谓词能用。「先删后插」的**插**那一半（`WriteMode::Incremental`
先 `DELETE` 同 id 再 `INSERT`）会替被重插的记录清掉自己的四张表旧行；谓词删要替代的是**对账**——
投影里已经没有的那些记录——它们的 `inst_info` / `aabb` / `trans` 行只能靠 `inst_relate` 行上的
`out` / `aabb` / `world_trans` 三个链接找到，也就是 §0 里的 (b) 形态。

## 4. 两条可走的路（待拍板，本轮都没做）

- **(a) 纯谓词删**：`inst_info` 行补 `anc`，`aabb` / `trans` 世界盒 / 世界变换行补 `dbnum` + `anc`
  （共享本地盒不补——它按 mesh id 共享、本来就「只增不删」），三张表各建 `anc` 索引，历史行回填
  （`backfill_inst_relate_anc` 同款启动回填）。之后删那一半就是五条 `DELETE … WHERE anc CONTAINS <root>`，
  W3 字面成立。代价：schema + 回填 + 每条几何多三份 `anc` 数组。
- **(b) 事务内 `LET` 一跳**：
  ```sql
  LET $rows = (SELECT id, out, aabb, world_trans FROM inst_relate WHERE anc CONTAINS <root>);
  DELETE array::filter($rows.aabb, |$v| $v != NONE);
  DELETE array::filter($rows.world_trans, |$v| $v != NONE);
  DELETE array::filter($rows.out, |$v| $v != NONE);
  DELETE $rows.id;
  -- tubi_relate 同形（它的 aabb / world_trans 也是链接）
  ```
  服务端一次索引游走、不回客户端、O(根)；但语句里有一条 `SELECT`，与 W3 的字面口径要对一下；
  `DELETE <array>` 与闭包 `array::filter` 在 2.1.4 上的形状要在 7997 的副本（不是真库）上先跑一遍。
- 两条都要配套：嵌套根 D 挂在祖先根 A 的 `anc` 下，落 A 会连 D 的行一起删再插——只要 `records_of_root(A)`
  仍含 D 的记录（今天如此，钉在 `reconciling_an_ancestor_root_does_not_delete_a_nested_roots_rows`），
  终态与今天相同；多块提交时首块整根删掉、后块失败会让该根**整体消失**到下次重生成（今天是「陈旧行留着」），
  失败形态的取舍要写进 ADR。

## 5. 可直接跑的脚本（只读）

7997 上线后：`.\Measure-AncContainsPredicateDelete.ps1 -Endpoint http://127.0.0.1:8009/sql -Dbnum 7997`
（不给 `-Root` 时自动取该库 `published_geometry_count` 最大的 gen_root）。脚本对任何含写动词的 SQL 直接抛错。

```powershell
<#
.SYNOPSIS
  只读量测：按根谓词删 `anc CONTAINS <root>` 在 SurrealDB 2.1.4 上能否走 idx_*_anc 索引，
  与现有对账读 existing_geometry_for_window（整库 e3d-model 几何身份一条 SELECT）对比。
  只发 SELECT / EXPLAIN / INFO，不发任何写。
.EXAMPLE
  .\Measure-AncContainsPredicateDelete.ps1 -Endpoint http://127.0.0.1:8009/sql -Dbnum 7997 -Root 24383/1
#>
[CmdletBinding()]
param(
    [string]$Endpoint = "http://127.0.0.1:8009/sql",
    [string]$Namespace = "1516",
    [string]$Database = "AvevaMarineSample",
    [string]$User = "root",
    [string]$Password = "root",
    [Parameter(Mandatory)][uint32]$Dbnum,
    # 生成根 word0/word1；缺省取该库 published_geometry_count 最大的 gen_root。
    [string]$Root,
    [int]$Repeat = 3
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
    if ($Sql -match '(?i)\b(DELETE|UPDATE|UPSERT|CREATE|INSERT|RELATE|REMOVE|DEFINE)\b') {
        throw "量测只读：拒绝发出写语句：$Sql"
    }
    $raw = Invoke-WebRequest -Method Post -Uri $Endpoint -Headers $headers `
        -ContentType "application/surrealql" -UseBasicParsing -Body $Sql
    $raw.Content | ConvertFrom-Json
}

function Show([string]$title, $statement) {
    Write-Host "`n### $title  (time=$($statement.time), status=$($statement.status))"
    $statement.result | ConvertTo-Json -Depth 6 -Compress | Write-Host
}

Write-Host "endpoint=$Endpoint ns=$Namespace db=$Database dbnum=$Dbnum"
Write-Host ("version: " + (Invoke-WebRequest -Uri ($Endpoint -replace '/sql$', '/version') -UseBasicParsing).Content)

$info = Invoke-Sql "INFO FOR TABLE inst_relate; INFO FOR TABLE tubi_relate; INFO FOR TABLE inst_info; SELECT count() FROM inst_relate WHERE dbnum = $Dbnum GROUP ALL; SELECT count() FROM inst_relate GROUP ALL; SELECT count() FROM tubi_relate WHERE dbnum = $Dbnum GROUP ALL; SELECT count() FROM inst_info WHERE dbnum = $Dbnum GROUP ALL;"
Show "inst_relate indexes" @{ time = $info[0].time; status = $info[0].status; result = $info[0].result.indexes }
Show "tubi_relate indexes" @{ time = $info[1].time; status = $info[1].status; result = $info[1].result.indexes }
Show "inst_info indexes" @{ time = $info[2].time; status = $info[2].status; result = $info[2].result.indexes }
Show "inst_relate rows (dbnum)" $info[3]
Show "inst_relate rows (all)" $info[4]
Show "tubi_relate rows (dbnum)" $info[5]
Show "inst_info rows (dbnum)" $info[6]

if (-not $Root) {
    $pick = Invoke-Sql "SELECT record::id(id) AS key, published_geometry_count FROM gen_root WHERE dbnum = $Dbnum ORDER BY published_geometry_count DESC LIMIT 1;"
    $Root = ($pick[0].result[0].key) -replace '_', '/'
    Write-Host "picked root: $Root (published_geometry_count=$($pick[0].result[0].published_geometry_count))"
}
$parts = $Root -split '/'
$packed = ([uint64]$parts[0] -shl 32) -bor [uint64]$parts[1]
Write-Host "root=$Root packed=$packed"

# A. 现有对账读（existing_geometry_of_dbnums_on）：整库 e3d-model 几何身份一条 SELECT。
$reconcile = "SELECT direct_model.geometry_id AS geometry_id, anc FROM inst_relate WHERE dbnum IN [$Dbnum] AND direct_model.source = 'e3d-model' AND direct_model.geometry_id != NONE"
$r = Invoke-Sql "$reconcile EXPLAIN FULL;"
Show "A. 对账读 EXPLAIN FULL" $r[0]
$times = @()
for ($i = 0; $i -lt $Repeat; $i++) { $times += (Invoke-Sql "SELECT count() FROM ($reconcile) GROUP ALL;")[0].time }
Write-Host "A. 对账读 ×${Repeat}: $($times -join ', ')"

# B. 按根谓词：inst_relate / tubi_relate（有 anc + idx_*_anc）。
foreach ($pred in @(
        "SELECT id FROM inst_relate WHERE anc CONTAINS $packed",
        "SELECT id FROM inst_relate WHERE dbnum = $Dbnum AND anc CONTAINS $packed",
        "SELECT id FROM tubi_relate WHERE anc CONTAINS $packed",
        "SELECT id FROM tubi_relate WHERE dbnum = $Dbnum AND anc CONTAINS $packed"
    )) {
    $r = Invoke-Sql "$pred EXPLAIN FULL;"
    Show "B. $pred  EXPLAIN FULL" $r[0]
    $times = @()
    for ($i = 0; $i -lt $Repeat; $i++) { $times += (Invoke-Sql "SELECT count() FROM ($pred) GROUP ALL;")[0].time }
    Write-Host "   ×${Repeat}: $($times -join ', ')"
}

# C. 谓词删的第二半：inst_info / aabb / trans 没有 anc（inst_info 连索引都没有），只能靶向
#    inst_relate 的 out / aabb / world_trans 指针——量子查询形态能不能走索引。
foreach ($pred in @(
        "SELECT id FROM inst_info WHERE id IN (SELECT VALUE out FROM inst_relate WHERE anc CONTAINS $packed)",
        "SELECT id FROM aabb WHERE id IN (SELECT VALUE aabb FROM inst_relate WHERE anc CONTAINS $packed)",
        "SELECT id FROM inst_info WHERE dbnum = $Dbnum"
    )) {
    $r = Invoke-Sql "$pred EXPLAIN FULL;"
    Show "C. $pred  EXPLAIN FULL" $r[0]
    $times = @()
    for ($i = 0; $i -lt $Repeat; $i++) { $times += (Invoke-Sql "SELECT count() FROM ($pred) GROUP ALL;")[0].time }
    Write-Host "   ×${Repeat}: $($times -join ', ')"
}
```

## 6. 原始输出（8010 / dbnum 1112 / root 17496/92413，2026-09-10 18:0x）

```
endpoint=http://127.0.0.1:8010/sql ns=1516 db=AvevaMarineSample dbnum=1112
version: surrealdb-2.1.4+20250317.45013fc9
### inst_relate indexes  {"idx_inst_relate_anc":"DEFINE INDEX idx_inst_relate_anc ON inst_relate FIELDS anc","idx_inst_relate_dbnum":"DEFINE INDEX idx_inst_relate_dbnum ON inst_relate FIELDS dbnum","idx_inst_relate_in":"DEFINE INDEX idx_inst_relate_in ON inst_relate FIELDS in"}
### tubi_relate indexes  {"idx_tubi_relate_anc":…,"idx_tubi_relate_dbnum":…,"idx_tubi_relate_in":…}
### inst_info indexes  {}
### inst_relate rows (dbnum)  {"count":4702}     ### inst_relate rows (all)  {"count":8404}
### tubi_relate rows (dbnum)  {"count":0}        ### inst_info rows (dbnum)  （无行）
picked root: 17496/92413 (published_geometry_count=1090)   packed=75144747903229

### A. 对账读 EXPLAIN FULL  (time=117.0802ms)
[{"detail":{"plan":{"index":"idx_inst_relate_dbnum","operator":"union","value":[1112]},"table":"inst_relate"},"operation":"Iterate Index"},{"detail":{"type":"Memory"},"operation":"Collector"},{"detail":{"type":"KeysAndValues"},"operation":"RecordStrategy"},{"detail":{"count":4702},"operation":"Fetch"}]
A. 对账读 ×3: 123.1796ms, 121.3273ms, 121.6646ms

### B. SELECT id FROM inst_relate WHERE anc CONTAINS 75144747903229  EXPLAIN FULL  (time=19.5259ms)
[{"detail":{"plan":{"index":"idx_inst_relate_anc","operator":"=","value":75144747903229},"table":"inst_relate"},"operation":"Iterate Index"},…,{"detail":{"count":1090},"operation":"Fetch"}]
   ×3: 21.9274ms, 22.6552ms, 21.752ms
### B. SELECT id FROM inst_relate WHERE dbnum = 1112 AND anc CONTAINS 75144747903229  EXPLAIN FULL  (time=20.1454ms)
[{"detail":{"plan":{"index":"idx_inst_relate_anc","operator":"=","value":75144747903229},"table":"inst_relate"},"operation":"Iterate Index"},…,{"detail":{"count":1090},"operation":"Fetch"}]
   ×3: 21.8327ms, 21.9957ms, 22.8054ms
### B. SELECT id FROM tubi_relate WHERE anc CONTAINS 75144747903229  EXPLAIN FULL  (time=596µs)
[{"detail":{"plan":{"index":"idx_tubi_relate_anc","operator":"=","value":75144747903229},"table":"tubi_relate"},"operation":"Iterate Index"},…,{"detail":{"count":0},"operation":"Fetch"}]
   ×3: 636µs, 521.1µs, 459.4µs
### B. SELECT id FROM tubi_relate WHERE dbnum = 1112 AND anc CONTAINS 75144747903229  EXPLAIN FULL  (time=610.5µs)
   同上；×3: 665.6µs, 471.3µs, 453µs

### C. SELECT id FROM inst_info WHERE id IN (SELECT VALUE out FROM inst_relate WHERE anc CONTAINS 75144747903229)  EXPLAIN FULL  (time=2.9918061s)
[{"detail":{"table":"inst_info"},"operation":"Iterate Table"},{"detail":{"reason":"Unsupported subquery: (SELECT VALUE out FROM inst_relate WHERE anc CONTAINS 75144747903229)"},"operation":"Fallback"},…,{"detail":{"count":0},"operation":"Fetch"}]
   ×3: 3.4617016s, 3.6959841s, 2.9467755s
### C. SELECT id FROM aabb WHERE id IN (SELECT VALUE aabb FROM inst_relate WHERE anc CONTAINS 75144747903229)  EXPLAIN FULL  (time=61.9022235s)
[{"detail":{"table":"aabb"},"operation":"Iterate Table"},{"detail":{"reason":"Unsupported subquery: (SELECT VALUE aabb FROM inst_relate WHERE anc CONTAINS 75144747903229)"},"operation":"Fallback"},…,{"detail":{"count":1090},"operation":"Fetch"}]
   ×3: 45.4201264s, 44.2861767s, 42.6951239s
### C. SELECT id FROM inst_info WHERE dbnum = 1112  EXPLAIN FULL  (time=2.7405ms)
[{"detail":{"table":"inst_info"},"operation":"Iterate Table"},…,{"detail":{"count":0},"operation":"Fetch"}]
   ×3: 2.8066ms, 2.5842ms, 2.4896ms
```

## 7. 边界与假设

- `EXPLAIN` 只有 `SELECT` 有；`DELETE … WHERE` 在 2.x 里与 `SELECT` 共用同一个 `QueryPlanner`
  （`Iterator::prepare` 按语句的 `cond` 选索引），这一点本轮**没有**在库上验证——量测只读，不对任何库发 `DELETE`。
  走 (a) 或 (b) 之前在 7997 的副本上用真 `DELETE` 跑一次计时即可坐实。
- 8010 上 1112 的 `tubi_relate` 为 0 行、该根全是烘焙行（`out` 为 NONE）：C 组 `inst_info` 那一条的
  Fetch 0 是数据使然，不影响「计划器退化全表扫」的结论（3 s 就是扫 `inst_info` 全表付的）。
- 数字来自 8,404 行的小库；B 组是索引游走，随根大小线性、与库大小无关，可以外推；A 组随库大小线性；
  C 组随目标表大小 × 根大小，7997 上只会更坏。
