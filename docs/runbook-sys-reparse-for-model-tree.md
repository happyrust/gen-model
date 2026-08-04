# SYS 元数据重解析：让模型树看到真实工程库

日期：2026-07-27

## 为什么要跑

plant-ui 启动后模型树显示的是 dbnum 8000 下的三个 SITE——`/SITE-PIPING`、`/SITE-EQUIPMENT`、
`/SITE-STRUCTURE`，管子叫 `/100-B-1`、位号 `P-1001-A`，refno 全是 100/200/300/1000 这种整数。
**这是一份手写的测试夹具，不是 AvevaMarineSample 的工程数据。**

真实数据就在同一个库里：15.7 万个元素、974 个几何实例都在 7997（refno 前缀 24381），名字长
`/1CUP-HVACHB`、`/CNPE-ACP1000-TRAY-SPWL` 这样。树到不了它。

树根解析链是 `MDB → CURD → DESI 的 DBNO → WORL → SITE`。断点在第一步：

```json
// 8009 上 NAME == "/ALL" 的唯一一条 MDB
{ "id": "MDB:1", "NAME": "/ALL", "CURD": [ { "refno": "db_desc:1" } ] }
// db_desc 全表只有这一行
{ "id": "db_desc:1", "DBNO": 8000, "STYP": 1 }
```

`id` 是 `MDB:1` 而不是 refno 哈希、没有 `REFNO` 字段、CURD 指向 `db_desc` 表而不是 `DB` 表——
而 `db_desc` 在 gen-model 与 rs-core 的全部 Rust 代码里 **grep 零命中**，没有任何现存代码会写它。
这是一对遗留记录，把「本工程只有 8000 一个设计库」写死了。

对照组说明解析本身是好的：库里 84 个 MDB、56 个带 CURD，`/SPACEMANAGEMENT` 挂了 13 个 DESI 库、
`/INITIALDESIGN` 11 个——ADR-006 的跨块 CURD 修复在这份数据里生效。唯独 `/ALL` 是那条老的。

ADR-007 遗留① 预告过这个歧义（两个 `/ALL` 并存、`LIMIT 1` 选其一）。这里更极端：设计侧的
`/ALL`（ADR-007 验证时 CURD **71 项**）根本不在库里。

客户端那一半已经改了：`plant-ui` 的 `vendor/rs-core/src/rs_surreal/mdb.rs` 里五处
`from only MDB where NAME == … limit 1` 全换成了取 **DESI CURD 最长**的那条，`get_world` /
`get_world_refno` 也从死认 `$dbnos[0]` 改成按 CURD 顺序取第一个真有 WORL 的库。但库里只有一条
`/ALL` 时它没得可选——所以还欠这次重解析。

## 前置检查

1. 三份 `DbOption.toml` 的 `v_port` 都应是 **8009**（gen-model / plant-ui / rs-plant3-d）。
   8022 是 ams7997 专用落盘实例、几何层近乎空，别指过去。
2. 确认没有别的 gen-model 进程正在跑：`Get-Process aios-database`。它会占着同一份配置。

## 跑法

`DbOption.toml` 里只改一处：

```toml
only_sync_sys = true      # 跑完记得改回 false
```

其余保持现状即可确认一遍：`total_sync = false`、`sync_live = false`、`gen_model = false`、
`replace_dbs = true`（REPLACE 而非 INSERT IGNORE，正是重解析要的覆盖写）。

`included_db_files` 不用动——ADR-007 之后 SYS 解析不再受它约束，`only_sync_sys` 单独就能重建
MDB/CURD/DB。

```powershell
cd D:\work\plant-code\old\gen-model
cargo run --release --features console
```

**跑完立刻把 `only_sync_sys` 改回 `false`**，否则每次启动都重解析一遍。

## 验收

```sql
-- 1. MDB 覆盖。ADR-007 验证时是 51 行；现在是 84 行 / 56 行带 CURD
select count() from MDB group all;
select count() from MDB where CURD != NONE group all;

-- 2. /ALL 有几条、各自多长。期望出现一条 CURD 很长的设计 /ALL（ADR-007 记 71 项）
select record::id(id) as id, array::len(CURD) as curd_len,
       (select value DBNO from CURD.refno where STYP == 1) as desi
from MDB where NAME == "/ALL";

-- 3. 本次真正要回答的问题：7997 到底属不属于某个 MDB
select NAME from MDB where 7997 in (select value DBNO from CURD.refno);
```

判读：

- 第 2 条出现两条 `/ALL`（老的 CURD=1 + 新的 CURD 很长）就说明重解析成功。客户端已经改成取
  最长的那条，不会再选错。
- 第 3 条有结果 → 把 `mdb_name` 指向它，树就能覆盖 7997。
- 第 3 条仍为空 → 7997 在工程里就不属于任何 MDB，得另想办法（在 E3D 里把它加进某个 MDB，
  或者接受它只能靠按需生成单独看）。

## 还有一道坎

就算 7997 进了某个 MDB 的 CURD，树还要求 `WORL where REFNO.dbnum in $dbnos` 命中。而 7997
的 WORL 现在**不存在**：它的 15 个 SITE 的 `owner` 都指向 `pe:16189_0`，而这条记录在 `pe`
里查不到，整个 `16189_` 前缀一条都没有。

WORL 是 DESI 文件顶上的元素，SYS 重解析建不出来——那要 ams7997_0001 的 DESI 侧解析补上。
所以顺序是：先 SYS 重解析看 7997 在不在 MDB 里，在的话再处理它的 WORL。
