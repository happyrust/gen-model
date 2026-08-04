# AMS 8000 DuckLake 与 SurrealDB 解析一致性验证（2026-07-30）

环境：AvevaMarineSample，源文件 `D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001`
（13 MB，`file_latest_sesno = 34`）。验证用 SurrealDB 为 `bin\surreal.exe` 起的**内存实例**，
全程未触碰 `.surreal/ams-8009` 那份落盘工作库。DuckLake 扩展命中
`C:\Users\dpc\.duckdb\extensions\v1.5.5\windows_amd64\ducklake.duckdb_extension`
（SHA-256 与代码里钉的 `4546a5c6…` 一致）。

要回答的问题：解析一次之后，DuckLake 的层级投影与持久 SurrealDB 的 PE，是不是同一份数据。

## 一、验证手段

新增 `src/bin/verify_dbnum_hierarchy.rs`。它与 `verify_ams7997_hierarchy` 的区别是不写死
dbnum 与期望条数，全部期望值都来自本次运行自身，因此可以用在没人记录过基准数字的库上。
每次运行都建一个一次性 namespace 与一次性 hierarchy 根，对现有数据零副作用。

对账项：

1. 两侧 refno 集合互为子集（分别报 only_in_ducklake / only_in_surreal）；
2. 逐行比 `owner` / `noun` / `name`，`owner` 两侧先按同一套 sentinel 规则归一再比；
3. locator 反查每个 ref0 是否唯一指回本库；
4. 用 DuckLake 行自算森林，与 kv-mem（内嵌 SurrealDB）实际 BFS 出的子树对账，
   要求「各根子树之和 == 总行数」，即子树互不重叠且恰好覆盖全库。

## 二、结果

| 指标 | dbnum 8000 | dbnum 7997 |
|---|---|---|
| 解析元素数 | 14,178 | 157,258 |
| DuckLake `hierarchy_node` 行数 | 14,178 | 157,258 |
| SurrealDB `pe` 条数 | 14,178 | 157,258 |
| only_in_ducklake / only_in_surreal | 0 / 0 | 0 / 0 |
| owner / noun / name 不一致 | 0 / 0 / 0 | 0 / 0 / 0 |
| locator ref0 反查 | 2 个，全部命中本库 | 2 个，全部命中本库 |
| 森林可达性 | 62 根 → 14,178 全可达，0 孤儿 | 178 根 → 157,258 全可达，0 孤儿 |
| kv-mem 子树汇总 | 14,178（== 总行数） | 157,258（== 总行数） |
| 最大根 | `16192_0`，11,604 后代 | `16189_0`，145,370 后代 |
| 结论 | OK | OK |

日志：`output/plant-ui-increment/verify-ams8000.log`、`verify-ams7997-generic.log`。

根数多于直觉（62 / 178）是因为除 WORL 外还有一批元素的 owner 落在本库以外。这是跨库
owner，不是悬空引用——「子树之和 == 总行数」这一条同时排除了重叠与遗漏。

7997 这一轮同时确认：01:22 那次
`hierarchy baseline incomplete: parsed=157258 required=157259 missing=["16189_1"]`
已被控制记录门修复关掉。

## 三、生产路径（首次跑通）

`scripts\Invoke-SiteDbnumParse.ps1` 的四步，此前 7/29 与今天都停在 `[1/4]`。修掉下节两个
缺陷后全部通过：

```
[1/4] SYSSYNC|AvevaMarineSample|ok
[2/4] BASELINE|AvevaMarineSample|8000|ok|14178      同时排入 2967 个全量生成根
[3/4] 预览扫描登记 ok
[4/4] pe=14178  info=14178  applied_sesno=34  file_latest_sesno=34
      AMS DBNUM integrity passed for: 8000
```

DuckLake catalog 落盘于 `.sites\8000\output\AvevaMarineSample\hierarchy\`：

```
hierarchy.ducklake                                  5.6 MB
data\main\hierarchy_node\dbnum=8000\*.parquet     118.2 KB
data\main\hierarchy_release\*.parquet                 3 KB
locators\17fcccb4….json  +  active_locator.json
```

路径在站点目录而非仓库根，是因为 runbook 把二进制的 cwd 设成站点目录，
`HierarchyProjectionStore::for_project` 按 cwd 取相对路径。

## 四、过程中修复的两个缺陷

两处都在 `src/versioned_db/database.rs` 的全量解析路径里。

### 缺陷 1 · sibling_order 算在了未过滤的成员表上

```
children of 8646969176415535104 in dbnum 251047 must have unique contiguous sibling_order from 0: [2, 3, 4, 5, 6]
```

`children.iter().enumerate()` 先取序号、再 `continue` 掉控制记录，于是保留下来的成员序号带
空洞，撞上 `validate_hierarchy_rows` 的「唯一、连续、从 0 开始」不变量。改为只对**保留下来
的**成员递增计数。

这个缺陷是修 `16189_1` 的那个控制记录门带出来的：门把非元素挡出了投影，却没有重排序号。
DESI 库（7997 / 8000）恰好没踩到，系统库一跑就破。

### 缺陷 2 · 层级投影套到了系统/字典库头上

```
dbnum 8191 child 105548821300654 appears in multiple member lists
```

该 refno 是 **24575/1454**，在 `amssys` 里同时属于两个父节点的成员表。`amssys` 携带
MDB / WORL，它一失败，SYS 元数据同步整体失败，`initialize_ams_dbnums` 拿不到世界根，
**任何全新站点都无法引导**。

层级投影建模的是设计树，而系统与字典库的记录形态本就不遵守设计树不变量，且目前没有任何
消费者读它们的投影（`baseline_model_plan` 对 `db_type != "DESI"` 直接返回空计划，
`load_baseline_nodes` 只在 DESI 路径被调）。因此把投影范围收窄为排除
`DICT / SYST / GLB / GLOB`。见 ADR-017。

注意这是**绕过**而非查清：24575/1454 为什么会属于两个父节点仍未定论。若将来需要系统库的
层级，这条会再冒出来。

两处修复后重跑隔离验证，8000 仍然零差异，无回归。

## 五、仍然开着的口子

1. **kv-mem 查询比解析还贵。** 7997 一次全库子树遍历 `query_ms = 949124`（15.8 分钟），
   而解析整个 57 MB 库只要 `parse_ms = 63861`（64 秒）。诚实折算：验证器为找叶子把最大那棵
   子树（145,370 节点）多 BFS 了一遍，单趟约 7～8 分钟。8000 的 14k 行只要 7.0 秒，
   所以是随规模超线性恶化。**且该测量是在 readiness 关闭下取得的**——
   `docs/plans/kv-mem-hierarchy-cache-remediation.md` H1 假设的「热路径成本主要来自每查询
   一次的 readiness 往返」在这个量级下只是小头，大头在 BFS 本身
   （`owner IN $frontier` 的巨大 IN 列表）。方案里「warm p95 至少快 3 倍」的性能门禁，
   `descendants_inclusive` 目前远达不到。
2. **DuckLake 扩展离线交付未落地。** `resource/duckdb/` 为空，实际靠开发机
   `%USERPROFILE%\.duckdb` 缓存兜底，换台干净机器会在 store open 就失败。
3. **增量 change-set 路径未验。** 本轮只覆盖全量基线。
4. **`Invoke-SiteDbnumParse.ps1` 会吞错。** `& $exe *>&1 | Tee-Object` 在子进程失败时挂住
   而不报错（今天挂了 15 分钟，7/29 同样），两次「停在 [1/4]」都是它遮住了真错误。
   把二进制单独拿出来跑才拿到错误信息。
