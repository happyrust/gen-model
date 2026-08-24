# Issue #21：`insts_flat = NONE` 的读者可见残留（库 A 快照 40 行）

- **类型**：Bug 🐛（待定性，见「尚未回答的那一问」）
- **优先级**：Medium 🟡
- **状态**：Open 📝
- **创建日期**：2026-08-25
- **相关模块**：`src/fast_model/pdms_inst.rs`（`sweep_inst_relate_flat`）
- **归属线**：ADR-041 / **ADR-043（`insts_flat` 失效协议）** + `specs/025-insts-flat-invalidation/`。
  **不属于 `specs/009-retire-occ`**——它只是在数 T053 的段数等价类时撞出来的。

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

## 尚未回答的那一问

**静态副本分不清「缺陷」与「清扫在这份快照之后没跑过」。**
`sweep_inst_relate_flat` 挂在启动序列与 worker 空闲轮（脏位门控）两处，
副本落盘的时点相对最后一次清扫在哪儿，从库里读不出来。所以这条**只报不判**。

要定性，至少答三问：

1. 在这份副本上跑一次 `sweep_inst_relate_flat`，40 行会不会归零？
   归零 ⇒ 只是快照时点问题；不归零 ⇒ 清扫的 `WHERE` 或 `UPDATE` 有漏。
   （现成入口：`pdms_inst` 里那条 `#[ignore]` 的手动 live 用例，
   它自己就断言残留为 0。）
2. 那三行带 `booled_id` 的为什么连修复段也没碰到——
   `run_booled_flat_repair_migration_on` 的 `BOOLED_FLAT_MISMATCH` 明确包含
   `insts_flat = NONE`，若迁移标记 `queue_control:booled_flat_repair_migration`
   已落而这些行还在，那就是「标记落早了」（复核没拦住）。
   **先查库上有没有这个标记**，这一条比第 1 问更能分出真假。
3. 脏位（`INSTS_FLAT_DIRTY`）是进程级 `AtomicBool`：写过 `inst_relate` 的那个进程
   若在空闲轮到来之前退出，脏位随进程消失，下一次启动的存量回填才补。
   40 行是不是就卡在这个缝里？——若是，那不是「清扫有 bug」，
   而是 ADR-043 决策 5「缓存变旧前必须先 durable 失效」在**回填侧**的同款缺口。

## 相关

- 证据与全部查询：`docs/evidence/2026-08-25-t053-segment-identity-scope.md` §三
- 判据出处：`src/fast_model/pdms_inst.rs` 的 `sweep_inst_relate_flat` /
  `VALID_BOOLED` / `BOOLED_FLAT_MISMATCH` / `BOOLED_FLAT_REPAIR_MARKER`
- 同表另一种脏值（已治）：ADR-043 决策 2、`specs/019-booled-flat-backfill-closure/`

## 标签

`insts-flat` `cache-invalidation` `adr-043` `spec-025` `live-data`
