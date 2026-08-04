# ADR-007：SYS 元数据文件解析不受 included_db_files 限制

状态：已接受
日期：2026-07-24
关联：ADR-006；`src/versioned_db/database.rs::sync_total_async_threaded`、`sync_pdms`

## 背景

修好 CURD/DBLS 解析（ADR-006）后模型树仍空。第二层根因：`only_sync_sys` 重解析根本没解析 SYS 文件。

`sync_pdms` 分两次调 `sync_total_async_threaded`：先 `["DICT","SYST","GLB","GLOB"]`（SYS），后 `["DESI","CATA"]`。其文件筛选原为：

```
if (is_parse_sys && is_total_sync)
   || included_db_files.is_none()
   || included_db_files.contains(&file_name) { ... 再按 db_type 过滤 ... }
```

- SYS 元数据（SYST/DICT/GLB/GLOB）存在**专属文件**（`amssys` / `acpsys` …），**从不出现在 `included_db_files` 里**（那里列的是 DESI/CATA 库文件，如 `ams8000_0001`）。
- 旧条件下，SYS 文件只有在 `is_total_sync=true` 时才绕过 `included_db_files`；当 `only_sync_sys=true` 且 `total_sync=false` 时，`amssys` 不在 `included_db_files` → **被静默跳过**（日志仍打印「同步UDA和SYS数据成功」，实则一个 SYS 文件都没解析）。
- 后果：设计 MDB/CURD/DB 从不落库，SurrealDB 里只剩一份旧的目录 MDB `/ALL`（CURD=NULL），`get_world_refno("/ALL")` 取不到设计库 → 空树。

## 决策

SYS 同步（`is_parse_sys`）**始终遍历项目全部文件**、再由既有的 `db_type` 过滤只保留 SYS 文件——即把筛选条件的 `(is_parse_sys && is_total_sync)` 改为 `is_parse_sys`：

```
if is_parse_sys
   || included_db_files.is_none()
   || included_db_files.contains(&file_name) { ... }
```

- SYS 文件的解析不再依赖 `total_sync`，`only_sync_sys` 亦可正确重建 MDB/CURD/DB/WORL。
- DESI/CATA 同步（`is_parse_sys=false`）行为不变，仍受 `included_db_files` 约束。

## 验证

用 `total_sync=true`（等效绕过）先验证：解析 amssys 后 MDB 1→**51** 行（设计 MDB 齐全，含设计 `/ALL` DESC=CNPE，CURD 71 项）、`DB` 出现 **DBNO=8000**(STYP=1)；`get_world_refno` 的 MDB→CURD→DBNO 链解析成功（$f=1112）。改代码后 `only_sync_sys` 单独亦应得到同样结果。

## 结果 / 约束 / 遗留

- 死标志 `reset_mdb_project`（options 里定义、全仓从未使用）与本问题无关，不依赖它。
- 遗留①：设计库与目录库的 SYS 若同时解析，二者的 `/ALL` 会并存（本轮见两行 `/ALL`）；`get_world_refno` 用 `LIMIT 1` 选其一，存在歧义——后续可让其优先选 CURD 非空的设计 MDB（另开 ADR）。
- 遗留②：`get_world_refno` 取 CURD 中第一个 STYP=1 库的 WORL；该设计库的 DESI 数据须已加载，树才非空（属正常的数据加载前置，非本 ADR 范围）。