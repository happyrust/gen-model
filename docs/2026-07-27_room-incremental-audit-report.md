# 房间计算的增量更新现状审计（2026-07-27）

范围：`gen-model` 增量更新链路对「房间归属」（`room_relate`）与其依赖的 AABB 空间树的覆盖情况。

结论一句话：**房间计算没有增量更新**。增量链路只把 AABB 空间树带上了一半，而那一半在当前
配置下实际不生效；房间归属仍然只在进程启动时全量重建一次，且那次重建是只增不删的。

§4 的实测补了三条：工作库（**8009**）里管路几何是齐的（906 个 `inst_relate.aabb`），
但**房间边是 0 条**；内存空间树被一个自我维持的陷阱卡在 45 条（D8，§4.2）；
房间算不出来的直接原因是**面板一个都没有几何**——124 个合规房间全部在第一步空手而归
（§4.3）。另查出配置项 `room_keyword` 因键名拼写不一致从未生效（D10）。

本文只做取证与定性，不含改动。设计决策见 `docs/adr/ADR-010-room-membership-incremental-update.md`。

---

## 1. 三层现状

### 1.1 第一层 · AABB 空间树 —— 链路通了，但不生效

调用链是完整的：

```
model_update_pending::drain                     (model_update_pending.rs:611)
  → ModelRefreshPolicy::generate_roots          (model_refresh.rs:35)
    → gen_all_geos_data                         (gen_model.rs:160)
      → process_meshes_update_db_deep           (gen_model.rs:201)
        → update_inst_relate_aabbs_by_refnos    (occ_generate.rs:324)
          → GLOBAL_AABB_TREE.update_aabbs       (occ_generate.rs:740)
      → GLOBAL_AABB_TREE.serialize_to_bin_file  (gen_model.rs:263)
```

`generate_roots` 设置 `debug_root_refnos`，`gen_all_geos_data` 的
`has_debug = db_option.debug_root_refnos.is_some()` 因此为真，走增量分支。收尾落盘到
`accel_tree.bin`。

问题在于这条链上有三个独立的洞，见 §2 的 D1–D4。

### 1.2 第二层 · 房间归属 —— 零增量

`build_room_relations`（`room_model.rs:96`）的**活调用点只有一个**：`lib.rs:241`，
在 `if db_option.gen_spatial_tree`（`lib.rs:232`）块内，位置在 `run_cli` 进入
`async_watch`（`lib.rs:307`）**之前**。进程一旦进入监听态，房间就再也不算了。

其余引用都不是活路径：

| 位置 | 状态 |
|---|---|
| `main.rs:42` | 只 `use`，没有调用 |
| `src/api/room_code.rs` | `api/mod.rs:12` 注释掉，未编译 |
| `src/plug_in/room_setting.rs` | `lib.rs:380` 的 `// pub mod plug_in;` 注释掉，未编译 |
| `src/ssc.rs` | `lib.rs:62` 的 `// pub mod ssc;` 注释掉，未编译 |

增量侧也没有给房间留任何位置：`ModelWorkAction` 只有
`RegenRoot / Transform / DeleteCleanup / CascadeExpand`（`model_update_pending.rs:436`
的 `execute_item` 全部分支）；`model_impact.rs` 的效果分类
（`DataOnly / TransformOnly / DirectGeometry / DependencyCascade / StructuralMembership / Unknown`）
里没有「几何位置变了 → 房间归属可能变」这一类。`docs/` 下 7 份增量审计与测试计划、
`2026-07-27_increment-update-backlog-reaudit-and-fixes.md` 的 16 项待办清单，
房间相关一条都没有。

### 1.3 第三层 · 消费方

`room_relate` 是**查询期**被读的，读它的只有两个函数：

- `fn::room_code` —— `resource/surreal/fn_query_room_code.surql`（`_hh` 变体同名）
- `fn::get_room_number` —— `resource/surreal/material_common.surql:184`，
  以及 `rs_surreal/material_list/common.surql`、`rs_surreal/material_list/tf/common.surql`
  两份重复定义

`rs_surreal/material_list/**` 下约 20 个材料表 surql 全部通过这两个函数取房间号。
另有 `resource/surreal/get_room_nodes.surql` 里两处直读 `<-room_relate`。

也就是说：**房间数据的陈旧会直接、且只会、体现在材料表的房间号列上**，
没有任何中间环节会先报错。

---

## 2. 缺陷清单

按修复依赖排序。D1 与 D2 是相互独立的两个洞——修好任何一个，另一个依然成立。
D9 与 D1 则是叠加的：只修 D1 不修 D9，修复不生效。

### D1 · `TransformOnly` 路径完全不刷新 AABB

`update_world_transforms`（`increment_manager.rs:1494`）只做一件事：重算
`get_world_transform` 并 `UPDATE {inst_relate} SET world_trans = …`。函数在
`println!("world transform更新完成")` 后直接 `Ok(())`（`:1556`），既不重算
`inst_relate.aabb`，也不碰任何一棵树。

而 `inst_relate.aabb` 正是用 `world_trans * geo.trans` 变换 `geo.aabb` 合并出来的
（`occ_generate.rs:709-715`）。`world_trans` 一变，存的 `aabb` 立即失效。

后果：**一次纯 `POS` / `ORI` 移动——最典型的「把设备从 A 房挪到 B 房」——走的是
`TransformOnly` 便宜路径，不重生成几何，因此永远进不了
`process_meshes_update_db_deep`，也就永远调不到 `update_inst_relate_aabbs_by_refnos`。
包围盒和 R 树里的位置都停在旧地方。**

注：这条便宜路径本身是有意设计的，`docs/2026-07-26_p3-t903-t904-assessment.md`
专门论证过 `TRANSFORM_ONLY_ATTR_NAMES` 只收 `POS`/`ORI`（`model_impact.rs:111`）。
不应取消它，而是给它补上包围盒刷新——补起来很便宜，`geo.aabb` 没变，只需重新合并一次。

### D2 · `replace_mesh = false` 让存量元素的包围盒永不刷新

`process_meshes_update_db_deep` 把 `replace_exist = dboption.is_replace_mesh()`
（`occ_generate.rs:286`）一路传到 `update_inst_relate_aabbs_by_refnos`，后者在
`replace_exist` 为假时给 SQL 追加 `and aabb=none`（`occ_generate.rs:697-699`）——
**只有还没有包围盒的行才会被算**。

`DbOption.toml:33` 是 `replace_mesh = false`，而
`ModelRefreshPolicy::generate_roots`（`model_refresh.rs:53-57`）只覆盖
`gen_model` / `gen_mesh` / `debug_refno_types` / `debug_root_refnos`，没有覆盖它。

这是**代码事实**，静态数据看不出来：库里 906 个 `inst_relate` 有包围盒，
但「它们在 `world_trans` 变化后有没有被刷新」需要动态复现才能确认（§4.5）。

### D3 · `update_aabbs` 条件写反，且按值删除匹配不到旧记录

`rs-core-pin/src/accel_tree/acceleration_tree.rs:189`：

```rust
pub fn update_aabbs(&mut self, bboxes: Vec<RStarBoundingBox>) {
    //检查 refno 是否已经存在了，如果存在，先移除，再添加进去
    for bbox in bboxes {
        if self.ids.insert(bbox.refno) {
            self.tree.remove(&bbox);
        }
        self.tree.insert(bbox);
    }
}
```

两个独立问题：

1. **条件反了。** `HashSet::insert` 返回 `true` 表示该值**原来不存在**。于是「新 refno」
   才去 `remove`（无事可做），「老 refno」反而跳过 `remove` 直接 `insert`，留下重复条目。
2. **就算条件改对也删不掉。** `RStarBoundingBox` 是 `#[derive(…, PartialEq)]`
   全字段比较（`:23`，含 `aabb` / `refno` / `noun`），`rstar` 的 `remove` 按相等性
   查找，拿**新** aabb 去删存的**旧** aabb 记录必然失配；而且它按新 envelope 定位，
   旧条目可能根本不在搜索路径上。

净效果：`replace_mesh` 一旦打开，同一 refno 会在 R 树里越堆越多份历史包围盒。
当前 `replace_mesh = false` 使这条暂时不发作——两个 bug 恰好互相掩盖。

### D4 · 删除路径不回收树内条目

`delete_inst_relate_subtree`（`helper.rs`）只删库里的边，不碰空间树；
`AccelerationTree` 也没有对外的 remove 接口。被删元素的包围盒会永久留在树里。

### D5 · 反向查询链路是死的

`load_room_aabb_tree`（`rs-core-pin/src/room/room.rs:71`）里的 SQL 括号未闭合、
内层 `select` 没有主表（`:85-90`）：

```sql
select value (select in as refno, aabb.d.* as aabb,
    in.noun as noun from only out->inst_relate
    where aabb.d!=none
start {offset} limit {page_count}
```

解析失败 → `SUL_DB.query(&sql).await?` 返回 Err → 而唯一的调用方
`query_room_panel_by_point`（`room/query.rs:36`）写的是
`load_room_aabb_tree().await.unwrap()`（`:38`）→ **panic**。

又因为没有任何其他地方填过 `GLOBAL_ROOM_AABB_TREE`，它永远是空的，
`load_room_aabb_tree` 开头那个「非空则早退」的分支也救不了。

### D6 · 房间归属零增量，且全量重建本身不幂等

除了 §1.2 说的「只在启动时跑一次」，`build_room_relations` 即便手动重跑也不是一次
干净的重建：

- `save_room_relate`（`room_model.rs:131`）只 `relate … set room_num=…`
  （`:139-145`），**从不删除**已经不再属于该房间的旧边；
- `build_room_panels_relate_common` 的 `room_panel_relate`
  （`room_model.rs:239-244`）不带 record id，每跑一次多一批重复边；
- 两处拼出来的大 SQL 都走 `SUL_DB.query(...)` 且**没有 `.check()`**
  （`:149`、`:247`），语句级错误（例如 `room_relate:<id>` 已存在）被静默吞掉。

### D7 · 多房间归属 + `limit 1` 无序 → 房间号不确定

`build_room_relations` 是**逐 panel** 调 `cal_room_refnos` 的（`room_model.rs:106-118`），
一个横跨两间房的构件会被两个 panel 各自判为成员，写出两条 `room_relate` 边——
数据模型本来就允许多归属。

但两个消费函数都是盲取第一条，没有 `ORDER BY`：

- `fn::room_code`：`select * from only $pe<-room_relate limit 1`（`:17`）
- `fn::get_room_number`：`($pe<-room_relate.room_num)[0]`

全量重建时写入顺序固定，表现相对稳定；一旦改成增量、边被删了再写回去，
顺序就会变——**同一个件的房间号会在两个值之间无规律跳，且没有任何日志会提示。**

### D9 · `TransformOnly` 把 `world_trans` 写成了错误的形状（静默）

比 D1 更狠的一条，同在 `update_world_transforms`（`increment_manager.rs:1494`）。

库里 `inst_relate.world_trans` 存的是**记录链接** `trans:⟨hash⟩`，真正的 Transform 挂在
被链接记录的 `d` 字段上。实测：

```json
{ "world_trans": "trans:⟨9827129573169261000⟩",
  "wt_d": { "rotation": [...], "scale": [1,1,1], "translation": [7308.02, 6955.73, 2685.0] },
  "wt_is_record": true }
```

全库 3298 条 `trans` 记录。生成侧一直是这么写的
（`pdms_inst.rs:187/285/631`、`equip_model.rs:40`，配 `save_transforms_to_surreal`）。

但 `update_world_transforms` 写的是**裸对象**：

```rust
"UPDATE {} SET world_trans = {};",
refno.to_inst_relate_key(),
serde_json::to_string(&world_transform)?      // {"translation":…,"rotation":…,"scale":…}
```

`inst_relate` 是 **schemaless**（`INFO FOR TABLE inst_relate` 的 `fields` 为空），
所以这次写入**静默成功**，而 `world_trans.d` 从此变成 `none`。后果连锁：

- `query_insts`（`rs-core-pin/src/rs_surreal/inst.rs:168`）取 `world_trans.d as world_trans`
  → 拿到空值，该元素的世界坐标在几何查询里失效；
- `update_inst_relate_aabbs_by_refnos` 的 `where world_trans.d != none`
  → 该元素被**整条过滤掉**，包围盒再也刷不上；
- `cal_room_refnos` 经 `query_insts` 读 panel → 同样受影响。

**这条与 D1 是叠加的**：D1 的修复（transform 之后补刷包围盒）单独上线不起作用，
因为等到刷新时 `world_trans` 已经被改成裸对象，刷新的过滤条件正好把它排除。

**已修**：改为按生成侧的口径写——`gen_bytes_hash` 算 hash、
`save_transforms_to_surreal` 先落 `trans` 记录、再
`UPDATE … SET world_trans = trans:⟨hash⟩`。

### D11 · `project_hd` / `project_hh` 的 SQL 变体无条件按文件名顺序加载，后者永远覆盖前者

`fn::room_code` 有两份定义：`resource/surreal/fn_query_room_code.surql`（hd）与
`fn_query_room_code_hh.surql`（hh）。启动日志里两份**都加载**，且按文件名顺序：

```
载入surreal fn_query_room_code.surql
载入surreal fn_query_room_code_hh.surql
```

后加载的 `_hh` 里第一行就是 `REMOVE FUNCTION fn::room_code;`，于是 hd 版被无条件覆盖。
实测线上定义（`INFO FOR DB`）确实是 **hh 版**：

```sql
DEFINE FUNCTION fn::room_code($pe: record) {
LET $noun = $pe.refno.TYPE;
LET $uda_room = (SELECT VALUE v FROM type::thing(ATT_UDA, record::id($pe)).udas WHERE u.NAME = /ROOM);
...
```

而 Rust 侧编译的是 `project_hd`（默认 feature），走 `FRMW` + `^[A-Z]\d{3}$`。
**两侧的项目变体是错位的**：Rust 按 hd 算并写边，SQL 按 hh 语义读。

加载不按 feature 门控这件事本轮未修——它牵涉 surql 加载器的改造，且改了之后
生效的函数会从 hh 换成 hd，属于行为变更，应单独评审。本轮的排序改动**两份都做了**，
以免修好门控后又出现新的不一致。

### 附 · 落盘容错

`rs-core-pin/src/accel_tree/acceleration_tree.rs:240-256`：

- `deserialize_from_bin_file` 内部是 `bincode::deserialize(&buf).unwrap()`——
  文件损坏或结构体改过字段就**直接 panic**，而不是降级重建；
- 调用方是 `deserialize_from_bin_file().unwrap_or_default()`
  （`room/room.rs:29`）——文件**不存在则静默得到一棵空树**，没有任何告警；
- 路径是硬编码相对路径 `File::create("accel_tree.bin")`，跟着 cwd 走，不带项目名。

---

## 3. 判定语义的两套口径

设计增量时必须先知道现有的两个方向用的不是同一套规则。

**正向**（panel → 元素，`cal_room_refnos`，`room_model.rs:251`）：

1. 用 panel 的 world AABB 去 `GLOBAL_AABB_TREE.locate_intersecting_bounds` 拉候选
   （`:281-284`）；
2. 过滤 NaN/Inf 包围盒、自身、以及**所有房间面板**（`exclude_refnos`，`:290-300`）；
3. 候选的 **AABB 8 个顶点全部**落在 panel TriMesh 内 → 直接算成员（`:302-309`）；
4. 只有**部分**顶点在内的进第二轮：拉 `inst_relate → inst_info → geo_relate → inst_geo`
   的实际几何点（带 `where !booled`，`:339-346`），**任一点**在内即算成员（`:355-382`）；
5. **一个顶点都不在内的直接丢弃，不做点检查**（`:315`）。

TriMesh 用 `ORIENTED | MERGE_DUPLICATE_VERTICES`（`:277`）。

**反向**（点 → panel，`query_room_panel_by_point`）：单个点、只有 `ORIENTED`
（`room/query.rs:68`）、命中第一个 panel 就 `return`（`:74`）。

同一个横跨两间房的桶形件，正向可能判给两间，反向只会给它找到的第一间。
再叠加 D7 的 `limit 1`，这种不一致在材料表上表现为「房间号时不时跳一下」。

---

## 4. 实测取证

2026-07-27，只读 SQL。

### 4.0 先说一个坑：现在有两个 SurrealDB 实例，别探错

同时在听的有 8009 / 8011 / 8020 / 8022。首轮探测打在了 8022 上——因为当时
`DbOption.toml`（14:36 那一版）写的就是 `v_port = 8022`。结果是一副「几何层全空」的
图景，据此曾错误地记过一条「网格从未生成」的 D8——**该条已作废**。

真相是产出当前这批数据的那次运行连的是 **8009**：`_env_setup_run.log:9` 写着
`数据库已经连接到 AvevaMarineSample, 站点: ws://localhost:8009`。

> **本文写作期间该文件又被本会话之外的写入方改过一次**（15:15），`v_port` 已从 8022
> 改回 8009，并补了一句「前端（plant-ui、rs-plant3-d）的 v_port 也是 8009，三边必须
> 一致」。也就是说配置现在是对的，但这棵树同时有多方在动——与
> `2026-07-27_increment-update-backlog-reaudit-and-fixes.md` §8 的观察一致。

| 实例 | `inst_relate` | 有 `aabb` 的 | `aabb` 表 | 性质 |
|---|---|---|---|---|
| **8009** | 1096 | **906** | **1366** | 有几何，是真正的工作库 |
| 8022 | 500 | 0 | 0 | 近乎空的另一实例 |

下面的数字全部取自 **8009**。**做任何取证前先确认端口**——两个实例的 ns/db 名字
完全一样（`1516 / AvevaMarineSample`），从返回值上分辨不出来。

### 4.1 几何与 AABB 层是好的

| 查询 | 结果 |
|---|---|
| `SELECT count() FROM inst_relate` | 1096 |
| `SELECT count() FROM inst_relate WHERE aabb != none` | 906 |
| `SELECT count() FROM aabb` | 1366 |
| `SELECT count() FROM inst_geo` | 587 |
| `SELECT count() FROM inst_geo WHERE aabb != none` | 584 |
| `SELECT count() FROM inst_geo WHERE pts != none` | 584 |

`assets/meshes` 下有 733 个 `.mesh`，最新一批时间戳 14:04:20，与
`accel_tree.bin` 的写入时间一致。网格生成、`inst_geo.aabb`/`pts` 落库、
`inst_relate.aabb` 合并——**这三步都跑通过**。

D2（`replace_mesh = false` 使 SQL 追加 `and aabb=none`）仍然是成立的**代码事实**，
但它的影响是「已有包围盒的元素不再刷新」，静态数据看不出来，需要 4.3 的动态复现。

### 4.2 `accel_tree.bin` 3KB 与 906 个包围盒对不上

库里有 906 个 `inst_relate.aabb`，而 `accel_tree.bin` 只有 3,107 字节。

不用推断——`gen_all_geos_data` 收尾就把树的大小打出来了（`gen_model.rs:262`），
历史日志里直接可读：

| 日志 | 打印值 |
|---|---|
| `output/live-d03-drain-20260727.log:3559` | `GLOBAL_AABB_TREE: 45` |
| `output/live-d03-delete-20260727.log:3678` | `GLOBAL_AABB_TREE: 45` |
| `output/live-ftub-delete-move-reorder-fixed-20260727.log` | `1` → `8` → `26` |
| `output/live-suppo-direct-20260727.log:3782` | `7` |
| `output/increment_viewer7997e.stdout.log` | `13` → `26` |

**树最多只到 45 条，库里是 906 条。** 45 × 约 55 字节 ≈ 2.5 KB，与 3,107 字节的文件
大小也对得上。数字在一次会话内单调增长（1 → 8 → 26），跨会话靠 `accel_tree.bin`
累积，但**永远不会跳到 906**——因为除了本次重生成碰到的那几个根，没有任何东西
会把库里的包围盒放进树里。

看 `load_aabb_tree`（`rs-core-pin/src/room/room.rs:20`）就能解释：
**从库里分页 bulk-load 的那段代码是注释掉的**（`:38-67`），它现在只做一件事——
反序列化 `accel_tree.bin`。唯一能从库里把树填满的入口是
`manual_update_aabbs`，而 `run_app`（`lib.rs:364`）只在
`GLOBAL_AABB_TREE.is_empty()` 时才调它。

于是形成一个自我维持的陷阱：

> `accel_tree.bin` 里只要**有几条**（非空），`is_empty()` 就是 false，
> `manual_update_aabbs` 永远不会触发，树也就永远停在那几条上，
> 库里的 906 个包围盒一个都进不来。

一棵近乎空的树，对 `cal_room_refnos` 的效果等同于「候选集为空」。
这一条记为 **D8**（替换掉作废的那条）。

**已修并实跑验证**：新增 `fast_model::aabb_tree::sync_aabb_tree_with_db()` 与库对账，
少了就调 `manual_update_aabbs(true)` 重建并落盘；`run_app`（`lib.rs:359`）的
`is_empty()` 判断替换为它。用例
`live_sync_aabb_tree_fills_tree_from_db`（默认 `#[ignore]`，需 `AIOS_LIVE_WS`）：

```
第一次跑：空间树只有 45 条，可重建 403 条（库中存量 906），正在重建空间树...
          空间树重建完成: 403 条
          GLOBAL_AABB_TREE: 45 -> 403
第二次跑：GLOBAL_AABB_TREE: 403 -> 403      # 不再重建
```

**判据用的是「可重建数」而不是「存量数」。** 906 是存量上界，但其中只有 403 个还能
从 geo 侧重算出来：

```sql
SELECT count() FROM inst_relate
WHERE world_trans.d != none
  AND count((SELECT id FROM out->geo_relate
             WHERE out.aabb.d != none AND trans.d != none)) > 0
GROUP ALL          -- → 403，与重建后的树条目数完全一致
```

拿 906 当判据会让每次启动都白重建一遍。所以实现是两级：先用便宜的存量计数放行健康的
树，只有不满足时才跑这条较慢的精确计数（本库 ~127ms）。

**顺带记一笔（待查）**：906 − 403 = **503 个 `inst_relate` 带着存量包围盒，但它们的
几何源现在已经取不到 aabb 了**。这批数据是怎么变成这样的、要不要清理，本次没有追。

### 4.3 房间：命名与结构都是对的，卡在面板没有几何

先纠正一个本报告早期版本的错误结论。曾据 `DbOption.toml` 的 `room_keyword = "-R-"`
匹配 0 条，判定「本项目没有房间结构」——**这个判断是错的**，原因见下。

#### D10 · 配置项 `room_keyword` 从来没有生效过

`DbOption` 的字段名是 **`room_key_word`**（三段下划线），而仓库里**每一份** toml 写的
都是 **`room_keyword`**（两段）：

```rust
// rs-core-pin/src/options.rs:247
pub room_key_word: Option<Vec<String>>,

// rs-core-pin/src/options.rs:292
pub fn get_room_key_word(&self) -> Vec<String> {
    self.room_key_word.clone().unwrap_or(vec!["-RM".to_string()])
}
```

键名对不上，`config` 又是按字段名精确匹配的，于是该字段恒为 `None`，
**实际生效的房间关键字永远是默认值 `-RM`**，与 toml 里写什么无关。
（顺带：字段是 `Option<Vec<String>>`，而 toml 写的是裸字符串，类型也对不上。）

这条本身是缺陷（配置静默失效），但在本项目上**歪打正着**——`-RM` 恰好才是对的。

#### 用生效的 `-RM` 重新测量

| 查询 | 结果 |
|---|---|
| `FRMW WHERE '-RM' IN NAME` | **124** |
| 其中末段匹配 `^[A-Z]\d{3}$` 的 | 124（抽样名字见下） |
| 其中拥有 PANE 子孙的 | **124** |

名字长这样，与 `test_room.rs:268` 那个 `^/\d+[A-Z]{2}-RM\d{2}-R\d{3}$` 的用例完全吻合：

```
/1RX-RM03-R301   →  末段 R301
/1RX-RM03-R310   →  末段 R310
/1RX-RM03-R320   →  末段 R320
```

**所以 AvevaMarineSample 有一套完整、合规的房间结构：124 个房间，全部带面板、
全部通过命名校验。** 层级也对（`FRMW → CWALL/CFLOOR → PANE`，正好落在
`build_room_panels_relate_common` 的子 + 孙两层覆盖范围内）。

#### 真正的卡点：面板一个都没有几何

| 查询 | 结果 |
|---|---|
| `inst_relate WHERE in.noun = 'PANE'` | **0** |
| `inst_relate WHERE aabb != none` | 403 |

那 403 个有几何的元素的 noun 分布，全是管路 / 暖通件，**没有任何面板**：

```
BEND 67  STRT 66  NCYL 43  NBOX 37  PFIT 34  DAMP 28
TAPE 19  ATTA 18  NXTR 16  BOX 11   BRCO 11  TRNS 8
```

`cal_room_refnos` 的第一件事就是 `query_insts(&[panel_refno], true)`，
拿不到实例就直接 `return Ok(Default::default())`（`room_model.rs:258-264`）。
面板全都没有 `inst_relate`，于是 124 个房间**每一个都在第一步空手而归**。

原因是生成范围：`DbOption.toml` 的
`manual_db_nums = [7997]` / `included_db_files = ["ams7997_0001", "amssys"]`
只生成了 7997 这个管路库，结构库（PANE / CWALL / CFLOOR 所在）从未参与生成。

**结论修正**：房间在本项目上跑不出东西，不是「没有房间」，也不是「关键字调错」，
而是**结构库没有生成几何**。这比原先的判断乐观——补生成结构库之后，
真实数据上的房间对拍验收是**有可能**做起来的，不必只依赖合成夹具。

### 4.4 D1 的修复已按真实数据验证了投影部分

`update_inst_relate_aabbs_by_refnos` 新增的 `aabb.d as old_aabb` 在 8009 上返回：

```json
{ "id": "inst_relate:24381_100678", "noun": "CONE",
  "old_aabb": { "mins": [7096.4014, 6744.137, 2600.0],
                "maxs": [7519.6387, 7167.3276, 2770.0] } }
```

正是 `parry3d::Aabb` 的 serde 形状，能直接反序列化进 `Option<Aabb>`；
`aabb` 为 none 时返回 `null` → `None`（`#[serde(default)]` 兜底）。投影本身没问题。

### 4.5 尚未验证

- **D1 的端到端复现**——对一个已有几何的元素只改 `POS`，跑一轮增量，确认
  `inst_relate.aabb` 确实跟着变了、且 R 树里没有留下重复条目。需要能触发一次
  真实的 `TransformOnly` 增量。
- **D3 的重复堆叠**——需要 `replace_mesh = true` 且树里已有该 refno 的条目。
- **503 个「有存量包围盒但重算不出来」的 `inst_relate`**（§4.2 末）——成因未查。
- **D9 的修复效果**——`world_trans` 写出来是否确为 `trans:⟨hash⟩`、`.d` 能否取到。
- **补生成结构库之后房间能否真的算出来**——§4.3 表明命名与层级都对，只差面板几何。
  这是把房间从「零产出」推到「有产出」最短的一步，也是让真实数据对拍成为可能的前提。

### 4.6 合成夹具：已跑通（首跑失败，补齐两个字段后转绿）

夹具（`src/fast_model/room_fixture.rs`：`FRMW → CWALL → PANE` 两室重叠 + 5 个构件，
带真实 `.mesh`）已建好并在 8009 上跑过一轮，随后已完整清理（`inst_relate` 回到 1096、
`room_panel_relate` 回到 0、`.mesh` 已删）。

**走通的部分**：`build_room_panels_relate_common` 正确识别了合成房间——名字过滤、
`^[A-Z]\d{3}$` 校验、`FRMW → CWALL → PANE` 两层遍历全部命中，写出了 2 条
`room_panel_relate`。盒形网格的 `contains_point` 也单独验过（不连库的单测
`box_mesh_supports_point_containment`）。

**卡住的部分与两个发现**：

1. **`query_insts` 在夹具上反序列化失败**：
   `Serialization error: failed to deserialize; expected a string, found None`。
   夹具没给 `pe.owner` / `inst_relate.generic` / `dt` 这类 `GeomInstQuery` 要求的
   非 Option 字符串字段。这是夹具自身要补的，但顺带暴露了一个真问题——
   `cal_room_refnos` 对它是 `query_insts(...).unwrap_or_default()`
   （`room_model.rs:258`），**错误被吞成空 Vec**，然后
   `if geom_insts.is_empty() { return Ok(Default::default()) }` 静静返回。
   也就是说：任何一个字段形状不对，整间房就会**无声地**算成 0 个成员，
   日志里连一行都不会有。

2. **空间树里有大量重复**：探针打印 `tree size = 45646`，而同期库里
   `inst_relate WHERE aabb != none` 只有 913 条——约 **50 倍**的膨胀。
   `accel_tree.bin` 也从本会话重建后的 26,967 字节涨到了 **3,039,563 字节**。
   这是 D3 那类重复堆叠在真实环境中的直接观测。
   （写入时间 15:22:33 来自**本会话之外**的进程，具体是哪条路径产生的重复未追。）

**补齐后转绿**。给 `pe` 补 `owner`、给 `inst_relate` 补 `generic` 之后，整条链路跑通：

```
[room_model.rs:105] room_panel_map.len() = 1     # 房间识别
[room_model.rs:112] refnos.len() = 3             # A 室成员
[room_model.rs:112] refnos.len() = 3             # B 室成员
test live_room_fixture_parity ... ok
```

断言的 6 条边全部命中：A→{20, 21, **24**}、B→{22, 23, **24**}。这意味着
`cal_room_refnos` 的**两轮判定都验到了**——20/21/22/23 走 AABB 八顶点快路径，
跨界的 24 八顶点判不出来、落到第二轮逐点兜底，并被两室同时收录（多归属成立）。

**跑法（重要）**：不要指向共享的工作库。夹具会写 `pe` / `inst_*` / `room_*` 多张表，
用独立 namespace：

```text
AIOS_LIVE_WS=ws://localhost:8022 AIOS_LIVE_NS=zzfx AIOS_LIVE_DB=zzfx \
  cargo test --lib live_room_fixture_parity -- --ignored --nocapture
```

用 8022 而不是 `surreal start memory` 起一次性实例，是因为本仓用的是
**fork 版 SurrealDB 客户端**，跟 PATH 上的官方 `surreal.exe` 握不上手：
`WebSocket protocol error: SubProtocol error: Server sent no subprotocol`。
要真正的一次性实例，得先从 `../../surrealdb` 构建 fork 版服务端。

### 4.7 并发告警：这台机器上有别的进程在动同一份数据

本节的测量期间发现：

- `DbOption.toml` 在 15:15 被本会话之外的写入方改过（`v_port` 8022 → 8009）；
- `accel_tree.bin` 在 15:22:33 被改过（26,967 → 3,039,563 字节）；
- 15:34:26 起有一个 `aios-database` 进程在运行；
- 收尾阶段（约 16:5x）**8009 / 8011 / 8020 / 8022 四个 SurrealDB 实例全部被关停**，
  只剩那个 `aios-database` 进程占着 8021。此后任何实库验证都跑不了。

**结论：8009 这个库不是独占的。** 本报告 §4 的所有数字都是某一时刻的快照，
复现时要重新测。后续要做写操作（补生成结构库、跑夹具）之前，
应先与其他写入方对齐，否则会互相踩。
