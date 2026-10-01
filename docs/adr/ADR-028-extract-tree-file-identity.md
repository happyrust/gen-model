# ADR-028：抽取树按文件名归并，叶子为水位权威

状态：Accepted（2026-08-14）

关联：ADR-001（身份与水位）、ADR-002（core.dll 权威）、ADR-016（同项目判重）、
ADR-021（回退重建）、ADR-025（Catalogue 同项目重复阻断）；
`specs/008-extract-tree-overlay/`

## 背景

同项目可以同时存在无后缀主库（`ams7355`）与 `_NNNN` 抽取（`ams7355_0001`）。
core.dll 的 `openExtractTree` 先开当前层再递归开父层，`db_open_read_db` 把整条链
当成一个库。本仓没有 dabacon opcode 134，不移植 C 引擎。

旧口径按裸 `(project, header db_no)` 判重，于是 7355 这对被 F6 `Duplicate` 整库
阻断；旧 SQL `collect_project_db_files` 又只认 `_0001` 并丢掉主库。兄弟抽取
（`ams9990_0001` + `ams9990_0002`）与人手副本仍必须阻断。

## 探针（AMS 实文件，2026-08-14）

`D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000`：

| 文件 | 大小 | db_type / dbnum | latest_sesno | 最新保存 |
|---|---|---|---|---|
| `ams7355` | 42.6 MB | CATA / 7355 | **13** | 2015-01-29 |
| `ams7355_0001` | 53.2 MB | CATA / 7355 | **15** | 2015-09-11 |

两边都只有稀疏会话页，**会话流不可比**，禁止把父层 sesno 并进叶子窗口。

2026-08-14 索引探针（`live_extract_tree_ams7355_refno_sets`）：父层 102716 个 refno，
叶子 135278 个，**parent_only=0**（叶子 ⊇ 父层）。因此基线热路径只解析叶子即可；
父层保留为按需 miss 兜底，不在基线里做并集全量重解析。

## 决策

1. **抽取家族是文件名规则，不读 SYS。** 家族键 = `(归属项目, 文件名所解析的库号)`，
   与头里 `db_no` 交叉校验；对不上则阻断（真值原则，不猜）。
2. **归并发生在 Duplicate 之前。** 手工、自动、Catalogue `select_catalogue_candidates`、
   旧 SQL `collect_project_db_files` 共用 `extract_family::collapse_extract_families`。
3. **叶子选唯一 `_NNNN`。** 仅主库 → 主库；仅 `_0001` → 该文件；主库+唯一叶子 → 叶子；
   多个 `_NNNN` → Duplicate。不按 SYS CLAIM 选兄弟。
4. **同家族从主库改挂叶子 = PathMigrated。** 若叶子 `file_latest_sesno < applied_sesno`
   （会话空间换了），走 ADR-021 重建。`check_file_against_state` 已让 Rollback 先于
   PathMigrated。
5. **水位仍按裸 dbnum。** 只认叶子 `file_latest_sesno`。父路径每次从目录重算
   （`parent_path_of`），不新表。
6. **按需叠加。** `OnDemandDbSession` 叶子 miss 再读父文件；locator 的 ref0 扫描
   对叶子做并集。基线解析叶子；仅当父层有独有 refno 时才 INSERT IGNORE 补缺
   （7355 实测为 0）。父层不贡献增量窗口。

## 后果

- 7355 主库+叶子不再阻断；`ams9990_0001`+`_0002` 与 `copy` 仍阻断。
- 不重写 dabacon 多文件单元打开；不把水位键改成 `(project, dbnum, extract)`。

## 追记 2026-10-01：Extract / Global FLUSH 之后主库会话号单调吗——语料答不了，现行处置不变（GitHub #45）

**只读核查**（2026-09-30，AMS 实文件；语料里唯一的抽取家族仍是 7355）：

| 文件 | 文件里留着的会话 | 页头 `OWNSES` |
|---|---|---|
| `ams7355`（主库） | 13、2、1 | 每页都是 `0xFFFFFFFF` |
| `ams7355_0001`（叶子） | 15、2、1 | 每页都是 `0xFFFFFFFF` |

两边都只留稀疏的几个会话；主库最新 13（2015-01-29）、叶子最新 15（2015-09-11），叶子之后主库没有再长出会话——也就是说
**语料里看不到一次 FLUSH**：FLUSH 把叶子的改动写回主库，主库应当多出一个比 13 新的会话；页头 `OWNSES` 全是 `0xFFFFFFFF`，也看不出
哪一页是从叶子写回的。
所以「FLUSH 之后主库 header 的会话号是否单调」这一问在现有语料上无从判别，issue #45 验收第 1 条（录一条 FLUSH 链）要新造现场。

**现行处置照旧**（本文决策 4 / 5 + ADR-021 / ADR-073 D3）：水位只认叶子；主库是按需 miss 兜底的父层、不贡献增量窗口，所以主库
会话号怎么变都不进窗口。真正危险的是**叶子**的会话空间被换（FLUSH 之后重建 / 刷新抽取）：叶子 `file_latest_sesno < applied_sesno`
走 Rollback → ADR-021 整库重建，同家族改挂走 PathMigrated。这条兜底**正确但贵**；贵到值得另做，要先知道它多常发生。

**实验方案（需要用户许可与一份隔离工程，不在共享工程上做）**：拷一份带抽取的工程到隔离目录（别的服务不盯的地方）；在叶子上改几处、
SAVEWORK，记主库 / 叶子的 header 会话号与会话清单；做一次 Global FLUSH（或 `FLUSH` 到主库），再记一遍；再在叶子上改、SAVEWORK，记第三遍。
判据：叶子的会话号若单调前进 → 现行处置零成本覆盖，关票；若叶子会话空间被换 → 每次 FLUSH 都触发整库重建，再按频率决定要不要立项
「FLUSH 后按主库新会话增量追平」（issue #45 验收第 3 条）。
