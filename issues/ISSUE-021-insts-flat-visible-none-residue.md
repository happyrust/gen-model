# Issue #21：`insts_flat = NONE` 的读者可见残留（库 A 快照 40 行）

- **类型**：Bug 🐛 → **判为非缺陷**
- **优先级**：Medium 🟡
- **状态**：**Closed ❌（非缺陷：快照早于清扫）** — 2026-08-25 当日定性
- **创建日期**：2026-08-25
- **解决日期**：2026-08-25
- **相关模块**：`src/fast_model/pdms_inst.rs`（`sweep_inst_relate_flat`）
- **归属线**：ADR-041 / **ADR-043（`insts_flat` 失效协议）** + `specs/025-insts-flat-invalidation/`。
  **不属于 `specs/009-retire-occ`**——它只是在数 T053 的段数等价类时撞出来的。

## 结论（2026-08-25，开条当日就查完了）

**不是缺陷。这份库 A 快照根本没跑过清扫，也没跑过 RM13 修复 migration。**
三条追问逐条有答，证据在下面的「定性实验」一节：

| 追问 | 答案 |
|---|---|
| 2. 迁移标记是不是落早了？ | **标记压根不存在。** `queue_control` 整张表只有 `main`（`paused = true`，2026-08-09）与 `watermark_seed`（2026-08-04），没有 `booled_flat_repair_migration`。所以不是「落早」，是**从未落过**——这一库上 migration 一次都没跑。 |
| 1. 跑一次清扫会不会归零？ | **会，一个批次就归零**（40 → 0）。清扫的 `WHERE` 与 `UPDATE` 都够得着这 40 行。 |
| 3. 是不是卡在进程级脏位那个缝里？ | **无需援引。** 前两条已经解释完：这库自 2026-08-09 起 `queue_control:main` 就是 `paused`，是一份冻结基线，不是在跑的实例。脏位那个缝仍然是 ADR-043 的真问题，但**不是这 40 行的成因**。 |

**顺带量出一件更值得记的：同一快照上 RM13 修复 migration 有 6,599 行在等。**
判据是 migration 自己的 `VALID_BOOLED AND BOOLED_FLAT_MISMATCH`——`booled_id` 有值
而 `insts_flat` 首元素对不上，正是 RM13 那批老格式行。照 migration 的 SQL 原样重放，
**14 个批次 6,595 行收敛到 0**（另外 4 行已被前一步清扫顺手写对），复核干净、
按流程就该落标记。也就是说这库是一份**前 migration 的冻结基线**，不是坏库。

> **对 D3 的一句提醒**：endgame plan 的 D3 选了库 A 当 RVM 主基准。RVM 对拍走
> `tessellate_libgm_param`、不读 `insts_flat`，所以这 6,599 行不影响 RVM；
> 但**若拿这库起服务给人看**，读侧会端出 RM13 那种带原语缩放的错误正体，
> 直到清扫跑完为止。起服务前先让它跑完启动序列。

## 现象

`.surreal/ams-7997-e3d-test-20260805`（库 A）的快照上，`inst_relate` 62,824 行里
**1,479 行 `insts_flat = NONE`，其中 40 行是对读者可见的（`aabb.d != none`）**。

`sweep_inst_relate_flat` 的清扫段圈的正是这一类：

```rust
// src/fast_model/pdms_inst.rs
"LET $rows = SELECT VALUE id FROM inst_relate WHERE insts_flat = NONE AND aabb.d != none LIMIT {BATCH};"
```

同文件的 live 断言也把它写成了不变量：

```rust
assert_eq!(residue, 0, "不应残留 insts_flat = NONE 且对读者可见的行");
```

**按仓内自己的口径，这 40 行是欠的。**

另外 1,439 行 `insts_flat = NONE` 不可见，清扫本来就不圈（读侧走 slim 兜底），
不算残留。

## 这 40 行是什么

dbnum 7997 与 8000 上的实在几何，不是空壳：

| dbnum | noun | 例 |
|---|---|---|
| 7997 | `CONE` / `BOX` / `NCYL` / `PANE` | `inst_relate:24381_100678`（CONE）、`24381_35844`（PANE） |
| 8000 | `GENSEC` / `FIXING` | `inst_relate:24384_25743`（GENSEC）、`24384_25748`（FIXING，5 条 CataNeg+Compound 边） |

其中**三行带着 `booled_id`**，按修复段本该被写成 `[{ geo_hash: booled_id }]`：

- `inst_relate:24381_100679` → `booled_id = '24381_100679_65'`
- `inst_relate:24381_100682` → `booled_id = '24381_100682_65'`
- `inst_relate:24381_100684` → `booled_id = '24381_100684_65'`
- `inst_relate:24381_100686` → `booled_id = '24381_100686_65'`

## 已排除的解释

同一轮盘点把 `insts_flat = []`（空数组）的 **11,992 行**逐行对着回填式的四道过滤
分了类，**空数组全部是正确终态，不是这个问题的另一种表现**：

| 为什么是空数组 | 行数 |
|---|---:|
| 该构件的边**全是负体**（`Neg` / `CataNeg` / `CataCrossNeg` / `Compound`） | 11,979（99.9%） |
| 有 `Pos` 边但全部 `visible = false` | 13 |
| **有合格边却仍为空（陈旧缓存）** | **0** |
| **`booled_id` 有效却仍为空** | **0** |

回填式只装正体（`geo_type = 'Pos'`），所以纯负体构件的空数组是对的。
**本 issue 只针对 `NONE` 那一侧的 40 行。**

## 复现

不需要连生产库；库 A 的副本就够。整段可直接粘：

```powershell
# 1. 起一份一次性副本（别碰母本，也别占用 8009）
Copy-Item -Recurse .surreal/ams-7997-e3d-test-20260805 .surreal/scratch-issue021
Start-Process -FilePath ".\bin\surreal.exe" -ArgumentList `
  "start","--user","root","--pass","root","--bind","127.0.0.1:8039", `
  "rocksdb:.surreal/scratch-issue021" -WindowStyle Hidden
Start-Sleep -Seconds 8

# 2. 数残留（期望 0，实得 40）
.\scripts\Invoke-Surreal8009.ps1 -Endpoint "http://127.0.0.1:8039/sql" `
  -Sql "SELECT count() FROM inst_relate WHERE insts_flat = NONE AND aabb.d != none GROUP ALL;"

# 3. 看它们是什么
.\scripts\Invoke-Surreal8009.ps1 -Endpoint "http://127.0.0.1:8039/sql" `
  -Sql "SELECT id, in.noun AS noun, booled_id, dbnum,
        (SELECT VALUE geo_type FROM out->geo_relate) AS gt
        FROM inst_relate WHERE insts_flat = NONE AND aabb.d != none;"

# 4. 对照：不可见的那 1,439 行（清扫不圈，不算残留）
.\scripts\Invoke-Surreal8009.ps1 -Endpoint "http://127.0.0.1:8039/sql" `
  -Sql "SELECT count() FROM inst_relate WHERE insts_flat = NONE GROUP ALL;"

# 5. 收摊
Get-CimInstance Win32_Process -Filter "Name='surreal.exe'" |
  Where-Object { $_.CommandLine -like "*scratch-issue021*" } |
  ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
Start-Sleep -Seconds 4
Remove-Item -Recurse -Force .surreal/scratch-issue021
```

> **踩过的坑**：`inst_geo` / `inst_relate` 的 id 是**字符串**（显示成 `inst_geo:⟨2⟩`），
> `WHERE out = inst_geo:2` 一行都匹配不到**且不报错**。要写
> `type::thing('inst_geo','2')`。

## 定性实验（在一次性副本上重放，母本未碰）

不建 Rust、不改配置——`sweep_inst_relate_flat` 与
`run_booled_flat_repair_migration_on` 的语句都是纯 SurrealQL，原样贴进去就行。
接着上面「复现」那三步：

```powershell
# ① 三个量：清扫残留 / migration 残留 / 标记在不在
$sql = @'
RETURN { sweep_residue: array::len((SELECT VALUE id FROM inst_relate
           WHERE insts_flat = NONE AND aabb.d != none)),
         migration_residue: array::len((SELECT VALUE id FROM inst_relate
           WHERE booled_id != NONE AND booled_id != '' AND string::lowercase(booled_id ?? '') != 'none'
             AND (insts_flat = NONE OR array::first(insts_flat ?? []).geo_hash != booled_id))),
         marker: array::len((SELECT VALUE id FROM queue_control:booled_flat_repair_migration)) };
'@
.\scripts\Invoke-Surreal8009.ps1 -Endpoint "http://127.0.0.1:8039/sql" -Sql $sql
# => { sweep_residue: 40, migration_residue: 6599, marker: 0 }

# ② 清扫段原样重放（`pdms_inst::sweep_inst_relate_flat` 的三条语句）
$sweep = @'
LET $rows = SELECT VALUE id FROM inst_relate WHERE insts_flat = NONE AND aabb.d != none LIMIT 500;
UPDATE $rows SET insts_flat = IF booled_id != NONE AND booled_id != '' AND string::lowercase(booled_id ?? '') != 'none'
  THEN [{ geo_hash: booled_id }]
  ELSE (SELECT trans.d AS transform, record::id(out) AS geo_hash FROM out->geo_relate
        WHERE visible && out.meshed && trans.d != none && geo_type='Pos') END,
  aabb_d = aabb.d, world_trans_d = world_trans.d RETURN NONE;
RETURN array::len($rows);
'@
.\scripts\Invoke-Surreal8009.ps1 -Endpoint "http://127.0.0.1:8039/sql" -Sql $sweep
# => 40（一个批次就吃完，之后 sweep_residue = 0）

# ③ 修复 migration 原样重放（`run_booled_flat_repair_migration_on` 的循环体），
#    LIMIT 500 一批，循环到不足 500 为止
$mig = @'
LET $rows = SELECT VALUE id FROM inst_relate
  WHERE booled_id != NONE AND booled_id != '' AND string::lowercase(booled_id ?? '') != 'none'
    AND (insts_flat = NONE OR array::first(insts_flat ?? []).geo_hash != booled_id) LIMIT 500;
UPDATE $rows SET insts_flat = [{ geo_hash: booled_id }] RETURN NONE;
RETURN array::len($rows);
'@
# => 14 批 6,595 行后收敛，migration_residue = 0
```

**注意 `$rows` 必须走单引号 here-string（`@'…'@`）**，双引号里 PowerShell 会把它
当变量插值成空串，语句会以一个看不出所以然的方式失败。

## 原始追问（已全部作答，保留备查）

1. ~~在这份副本上跑一次 `sweep_inst_relate_flat`，40 行会不会归零？~~ → **归零，一批**。
2. ~~那几行带 `booled_id` 的为什么连修复段也没碰到？先查库上有没有迁移标记。~~
   → **标记不存在，migration 从未在此库跑过**；这一条确实比第 1 问更能分真假。
3. ~~是不是卡在进程级脏位（`INSTS_FLAT_DIRTY`）那个缝里？~~ → **不必援引**。
   脏位那个缝仍是 ADR-043 的真问题，但不是这 40 行的成因。

## 留下来的那半句

这条查完只剩一句还没有着落，**它不是本 issue 的成因，别混进来**：
ADR-043 决策 5 说的「缓存变旧前必须先 durable 失效」在**回填侧**同样有缝——
`INSTS_FLAT_DIRTY` 是进程级 `AtomicBool`，写过 `inst_relate` 的进程若在空闲轮到来
之前退出，脏位随进程消失，要等下一次启动的存量回填才补。本库因为压根没跑过清扫，
照不出这个缝；要照它得在**跑着的**实例上构造「写完即退」。归 specs/025。

## 相关

- 证据与全部查询：`docs/evidence/2026-08-25-t053-segment-identity-scope.md` §三
- 判据出处：`src/fast_model/pdms_inst.rs` 的 `sweep_inst_relate_flat` /
  `VALID_BOOLED` / `BOOLED_FLAT_MISMATCH` / `BOOLED_FLAT_REPAIR_MARKER`
- 同表另一种脏值（已治）：ADR-043 决策 2、`specs/019-booled-flat-backfill-closure/`

## 标签

`insts-flat` `cache-invalidation` `adr-043` `spec-025` `live-data`
