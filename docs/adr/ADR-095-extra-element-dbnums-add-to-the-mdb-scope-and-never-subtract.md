# ADR-095：`extra_element_dbnums` 只往 MDB 范围里加库，永远不减

状态：**已接受**（2026-11-01）
日期：2026-11-01
关联：ADR-020（统一范围门）、ADR-077（初始化双路直写）、ADR-085（MDB 声明登记表）、issue #10、`src/data_interface/update_scope.rs`、`src/data_interface/watch_scope.rs`

## 背景

251095（`/PIPE_4DK_FWD`）和 251137（`/HVAC_2DK_FWD`）不在任何 MDB 的 `CURD` 里，`amssys` 里也没有它们的 `DB` 声明。执行范围只由 MDB 声明定（`UpdateScope`），于是这两个库的增量一律被判「范围外」，模型树和三维也看不到，Spooling 面板查到的行无从定位。

范围曾经由手写名单（`manual_db_nums` / `exclude_db_nums`）参与决定。issue #10 里 7999 被手写名单挡在外面，watcher 每 30 秒发现一次增量、每次跳过，日志与「MDB 里没这个库」一字不差。2026-08-06 起这些名单被剥夺了增量否决权。

## 决定

**D1　新增 `DbOption.toml` 键 `extra_element_dbnums`，只增不减。** `UpdateScope::admits` 对 DESI/ISOD 的判据是「MDB 声明 ∪ 补充名单」。补充名单没有任何办法把 MDB 声明的库移出范围。缺省是空名单，行为与引入前逐位相同。

**D2　只属于配置的那个 MDB。** `UpdateScope::finish` 只在请求的 MDB 是 `mdb_name` 时才填上补充名单；查别的 MDB、以及 `UpdateScope::unrestricted()`，都不带它。

**D3　声明口径不变。** `declared_desi()` / `declares()` 仍只返回 MDB 亲口声明的库，预览的 `not_in_project`、「声明了但没文件」告警都不受影响。补充成员通过 `extra_dbnums()` 单独取。

**D4　必须出声。** 装载扩展配置时名单非空就往 stderr 打「补充成员生效：…」。范围外的单条理由与汇总句都带上补充名单个数，人看日志能区分「MDB 没声明」和「补充名单也没写」。

**D5　plant-ui 用同名键补模型树根。** plant-ui 的 `site_nodes()` 先取 MDB 的 SITE，再追加补充库的 SITE（按 refno 去重）。

**D6　模型面同样认补充成员。** `mdb_membership::resolve` 把补充名单与 CURD 声明一起绑文件（同一次目录枚举、同一套选主），按文件头定类型后放进 `MdbMembership::extra_databases()`；`databases()` 仍只是 CURD。`model_databases()` = 声明 ∪ 补充，供 `current_mdb_source_inventory()`（模型源钉、解根、目录库定位）与 `MdbRouteDirectory::from_membership`（含 `curd_order`，`element_databases()` 才看得到）使用。没文件、文件头库号不符、类型不收的各记一条 membership problem，不进成员。

**D7　补充名单也收 CATA。** 补充的设计库引用的目录库 MDB 可能同样没声明（251095 的管件 SPRE 指向 251049 `/SP_LO`，不补则 941 个元件报「db 251049 没注册进这个 DbSet」）。所以文件头是 CATA 的号也收进 `extra_databases()`，只进模型面的目录定位；`UpdateScope::admits` 的放行仍只看 DESI / ISOD，CATA 不因此变成增量执行范围。

## 与 `watch_dbnums` 的区别

`watch_dbnums` 是收窄：只处理名单里的库，属于调试用的非常规状态。`extra_element_dbnums` 是放宽：在 MDB 声明之外多加几个成员。两者正交，`watch_dbnums` 照样能把补充库也挡掉，那时听到的是监听限定那句话。

## 选项与取舍

- 把它们写进 MDB 的 CURD：要改 E3D 项目本身，不在我们的控制范围内。
- 恢复 `manual_db_nums` 的范围语义：它既能加也能减，正是 issue #10 的病根。
- 只增不减的独立键：语义单一，删掉键就回到原样。

## 回退

删掉 `DbOption.toml` 里的 `extra_element_dbnums`，重启即可。已写入的模型数据可重新初始化或按 dbnum 回滚。
