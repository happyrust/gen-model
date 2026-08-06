# issue #10：全量解析复活已删子树，模型树看不到新增节点

- 关联：[issue #10「多次增量后，程序失效问题」](https://github.com/happyrust/gen-model/issues/10)
- 日期：2026-08-06
- 影响面：全量解析（`sync_total_async_threaded`）落库的每一个设计库

## 症状

E3D 里 `PIPE /1WCC1135` 下有两条 BRAN（原来的 `/1WCC1135/B1` 和复制出来的
`Copy-(2)-of-1WCC1135/B1`），查看器的模型树里只有一条。增量能报出变化，树纹丝不动，
做多少次都一样。

## 根因

模型树是沿 `pe_owner` 边遍历出来的。而**同一个信号**在两条路径上被判成了相反的意思：

| 路径 | 代码 | 「owner 的成员表里没有我」意味着 |
|---|---|---|
| 增量 | `pdms_io` `io.rs:1801` | `EleOperationDetail::Deleted` —— 我被删了 |
| 全量 | `parse_pdms_db` `parse.rs:1029` | 断链，按记录自带的 owner 把我补回去 |

全量解析 `parse_db_basic_data` 先自顶向下按成员块展开（这一步是尊重删除的，删掉的
元素走不到），随后有一轮补救：把表内没被走到的元素也当作根展开，再由
`relink_children_by_owner` 依据每条记录**自带的 owner** 把缺失的父子边补回去
（`parse.rs:1080-1095`）。那一轮是为「同一元素有多条物理记录、选中的那条不带成员块，
展开会在那里断掉」写的（TEST 项目的 DESI 会整库解析出 0 个元素），但它分不清：

- 选中的记录压根没有成员块 —— 确实是断链，该补；
- owner 列了成员、里面就是没有我 —— 那是 E3D 表达删除的方式。

于是**每一个被删掉的元素都会被原样挂回成员树**，连带整棵子树。

## 现场证据

源库 `ams000/ams7999_0001`（`incr_fold_probe --from 1 --to 41`，纯文件解析）：

```
sesno=3   refno=24383_2      noun=SITE  owner=16191_0   建 /1WCC-PIPEBJ 及整棵子树
sesno=21  refno=24383_2      删除
sesno=26  refno=24383_2      删除
sesno=29  refno=24383_2      删除
sesno=30  refno=24383_66456  noun=SITE  owner=16191_0   同名子树用全新 refno 整棵重建
```

落库之后（`.surreal/ams-7997-e3d-test-20260805`）：

| | 旧那棵（sesno 3） | 新那棵（sesno 30） |
|---|---|---|
| SITE `/1WCC-PIPEBJ` | `pe:24383_2` | `pe:24383_66456` |
| ZONE `/1WCC-PIPE-RX` | `pe:24383_3` | `pe:24383_66457` |
| PIPE `/1WCC1135` | `pe:24383_4` | `pe:24383_66458` |
| BRAN `/1WCC1135/B1` | `pe:24383_5` | `pe:24383_66459` |
| ZONE 数 / PIPE 数 | 2 / 118 | 2 / 118 |

两棵全部 `deleted: false`；**整库 `deleted = true` 的 pe 行数为 0**，源库那 14 条删除
记录一条都没落地。幽灵那棵 refno 小、在按 refno 排序的列表里靠前，展开的就是它；
新复制的 BRAN 挂在真身 `24383_66458` 下面，所以永远看不见。

问题不止这一处。按下面的检测语句，本机这套库里有 23 组同名重复的 SITE/ZONE，
分布在 dbnum 1112、7997、7999。

## 修复

`src/versioned_db/member_prune.rs`，接在 `sync_total_async_threaded` 里
`parse_file_db_basic_data` 之后、`all_refnos` 之前。判据取**元素自己的成员块**
（复用解析层同一个 `parse_ele_membs`，两边不会漂移）：

1. 逐个 owner：它自己的成员块非空时，把不在其中的子节点从 `children_map` 摘掉；
2. 摘完从 WORLD 做一次可达性遍历，丢弃不可达的元素。

两处刻意保守，都是为了不把补链轮原本要救的毛病放回来：

- owner 的成员块**为空**时视作「没有权威」而非「确实没有子元素」，其名下的边原样保留；
- WORLD 自己的成员块为空时整轮跳过（根都没有权威，再往下走会把整个库判成不可达）。

代价是空记录 owner 名下的幽灵留得下来——宁可多留，不可错删。

## 验证

- 5 条纯函数用例（判据、成员顺序不得重排、两处保守分支、干净库零副作用）；
- 真实文件定靶 `the_deleted_site_is_pruned_from_a_real_parse`（默认 `#[ignore]`）：

```powershell
$env:GEN_MODEL_PRUNE_FIXTURE = "D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7999_0001"
cargo test --lib the_deleted_site_is_pruned_from_a_real_parse -- --ignored --nocapture
```

```
All refnos count: 44901
PruneReport { dropped_edges: 2, dropped_elements: 9827, skipped_no_root_authority: false }
```

摘掉的两条边都在 WORLD 名下：已删并重建的 `/1WCC-PIPEBJ`（`24383_2`），以及已删且
本身为空的 `/1RS-CIVIL`（`24383_1`）。前者连带 9826 个后代，后者只有自己。真身
`24383_66456` / `24383_66458` 原样留下。

- 端到端 `a_reparse_lands_exactly_one_site_per_name`（默认 `#[ignore]`）：带修复把 7999
  解析进一台独立空库（`db_options/DbOption-issue10-e2e.toml`，监听 8077，不碰别人正在用的
  8009），落库 **34652** 行（修复前 44178 行）。dbnum 7999 的 SITE 从 6 个降到 4 个——幽灵
  `/1WCC-PIPEBJ`（`24383_2`）与已删且本身为空的 `/1RS-CIVIL`（`24383_1`）都不再出现；
  `/1WCC-PIPEBJ` 只剩 `24383_66456`，`/1WCC1135` 只剩 `24383_66458`。下面那条检测语句
  在这份新库上返回空。
- `cargo test --lib`：418 passed / 0 failed。

## 存量库

已经落库的幽灵不会自己消失（库里两棵长得一模一样，光靠 SQL 分不出谁是谁，权威在
源库文件里）。

**检测**（只读，认名字重复这个指纹；未命名元素本来就允许重名，已排除）：

```sql
SELECT * FROM (
  SELECT dbnum, noun, name, count() AS n
  FROM pe
  WHERE noun IN ['SITE', 'ZONE'] AND !deleted AND name != ''
  GROUP BY dbnum, noun, name
) WHERE n > 1 ORDER BY n DESC;
```

**清理**：对检出的 dbnum 带修复重新做一次全量解析，`replace_dbs = true` 会先
`check_and_clear_db(db_no)` 清干净再写，不需要手写删除语句。

```toml
manual_db_nums = [7999]   # 换成检出的 dbnum
replace_dbs = true
total_sync = true
```

清理后重跑上面的检测语句，对应 dbnum 应当不再出现在结果里。

## 尚未处理

`EleOperationDetail::Deleted` 渲染的是单行墓碑（`io.rs:906`）：

```sql
UPDATE pe:{id} SET deleted = true, sesno = {sesno}
```

**不级联子树**。而 PDMS 删一棵子树时，会话里只记根那一条（本轮 14 条删除记录里没有
任何 `24383_3/4/5`）。所以增量路径删掉一个 SITE 之后，它名下 118 个 PIPE 的整棵后代
仍然是活行——它们从根那里不可达，模型树看不见，但 `pe` 行、按名字的检索、空间索引与
房间归属都还看得见。这条单独记着。
