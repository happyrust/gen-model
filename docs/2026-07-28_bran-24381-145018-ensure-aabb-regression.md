# 按需重生成把 VALV 的世界 AABB 写松 —— BRAN 24381/145018 复现报告

- 日期：2026-07-28（发现于 plant-ui-109 会话的生成验证）
- 状态：**未修复，现场保留**（库里就是坏值，可直接观察）
- 复现载体：AvevaMarineSample ns=1516，dbnum 7997，BRAN `24381/145018`（`/Copy-of-RCS0014-1R43012新`，17 个子件：ELBO×9 / ATTA×6 / OLET / VALV）
- 结论一句话：**今天 02:54 与 13:35 两个二进制对同一个根做同样的 force ensure，前者收口后 VALV 的
  `inst_relate.aabb` 是网格口径的紧盒，后者写成一个没有任何持久几何支撑的松盒**——回归窗口 =
  今天 02:54→13:35 之间 gen-model / rs-core-pin 的未提交改动。

## 1. 复现步骤

1. 服务：debug 构建（`D:\Rust\target\debug\aios-database.exe`，编译于 2026-07-28 13:35:12），8022 端口，`DbOption.toml` 当前配置（`replace_mesh=false`）。
2. `POST /api/v1/model/ensure {"refno":"24381/145018","force":true}`。HTTP 层 120s 超时属预期，后台继续；冷缓存下全程约 11 分钟，收口后 `model_update_pending` 清空、水位 84=84。
3. 完成后查 `SELECT aabb FROM inst_relate WHERE in = pe:24381_145035;`。

## 2. 现象

VALV `24381/145035`（分支最后一个管件）的边 AABB：

| | aabb 记录 | mins | maxs |
| --- | --- | --- | --- |
| 重生成前（紧，正确） | `aabb:⟨6466746607843462392⟩` | 9569.75, 9789.443, 18512.3 | 10386.363, 10220.18, 19864.15 |
| 重生成后（松，错误） | `aabb:⟨12450518991627078186⟩` | 9544.75, 9624.393, 18272.3 | 10417.25, 10229.671, 19867.803 |

-Y 面凭空外扩 165mm、-Z 面 240mm、+X 面 31mm，体积约 2 倍。旧 aabb 记录仍在库里可对照。

同一次重生成还把 OLET `24381/145032` 之前缺失的 aabb 补上了（紧值，这是修复不是回归）。

## 3. 证据链

1. **新盒子没有几何支撑**：用库里**当前**的 `inst_info:⟨13648074980233081144⟩->geo_relate`
   （24 个几何，全部 meshed、无 bad）做「网格 aabb × (world_trans × 实例 trans)」盒变换合并，
   结果与**旧值逐位一致**（复算脚本
   `D:\work\plant-code\old\plant-ui\.context\check_valv_aabb.py`，数据快照同目录
   `bran145018_valv_geos.json` / `bran145018_valv_wt.json`）。
   即：照现行 `update_inst_relate_aabbs_by_refnos` 的口径重算，得到的就是旧值，不是新值。
2. **也不是网格变了**：force 重写后的 .mesh 顶点实测合并（plant-ui 测试
   `crates/plant-ui-data/tests/bran_24381_145018_geometry.rs`）与旧值精确一致到小数点后三位
   （union max 角 10386.363 / 19864.15 == 旧 VALV aabb）。
3. **松盒子长什么样**：换算到阀门局部坐标是 mins [-225, -422.787, -392.5]、
   maxs [647.5, 182.491, 1203.003]。局部 maxZ=1203.0 恰好是该阀 5 号 p-point 的 z；
   maxX=647.5 ≈ 3 号 p-point（x=626，pconnect=TUB，bore 15）加管径——两处精确对齐，
   指向**生成期的解析包络（含 P-point/接口延伸）被当成世界 AABB 写库**。具体写入语句未定位到行。
4. **回归窗口**：今晨 03:22（UTC 2026-07-27T19:22:37Z）postfix 全量 sweep（02:54 编译的二进制，
   `gen_root:24381_145018` 报告 Generated/renderable=56/written=18）之后 VALV 仍是紧值
   （13:47 快照 `plant-ui/.context/bran145018_before.json`）；13:44 用 13:35 二进制 force
   重生成后变松（`bran145018_after.json`）。两次都是同一条 ensure 链路、同一份设计数据（水位 84 未动）。

## 4. 为什么以前没暴露

- 定向重生成链路（`process_meshes_update_db_deep`）里 `update_inst_relate_aabbs_by_refnos`
  跑的是 `replace_exist=false`（随 `replace_mesh=false`）——**只补缺失、不纠正已有值**。
  生成期一旦写进一个 aabb，网格口径的刷新就永远轮不到它。
- 历史上启动期空间树对账 `sync_aabb_tree_with_db` → `manual_update_aabbs(true)` 的全量紧致重刷
  会把这类脏值整库覆盖掉，把问题掩住。下一次触发全量重建前，这个松盒子会一直错着
  （viewer 取景/拾取、房间归属候选都用它）。

## 5. 顺带的结构性发现（与本回归相关但另案）

- 隐含管与管件**共用 `inst_relate` 记录 id**（管段按 `leave_refno` 落到
  `inst_relate:{leave_refno}`，见 `pdms_inst.rs` TUBI 分支带 aabb、管件分支不带 aabb，
  两批 INSERT 同 id 按字段合并）：BRAN 自身那条边 `inst_relate:24381_145018` 实际是头段管记录
  （out=`inst_info:⟨2⟩` 单位圆柱）。
- `inst_geo:⟨2⟩`（共享单位管圆柱）**库里从未 meshed**（无 meshed/aabb 字段，
  `assets/meshes/2.mesh` 文件倒是存在）——隐含管在 plant-ui 的 query_insts 链路里画不出来。
- gen_root 报告的 renderable 从 56（03:22）降到 54（本次），可能与管段记录被覆盖有关，
  但两次计数用的二进制不同，口径变化未排除，不作为定论。

## 6. 建议排查方向

1. 在 02:54→13:35 的未提交改动里（gen-model 与 `../../rs-core-pin` 都要看）找「保存实例时
   往 `inst_relate` 写 aabb / 生成期解析包络」的新增或行为变化——第 3 节的局部包络特征
   （P-point 延伸）可以当指纹。
2. 无论回归本身怎么修，建议给定向重生成收尾加一道**对本次范围 `replace_exist=true` 的
   网格口径刷新**（范围是本根子树，代价可控），把「单一权威=网格口径」钉死，
   不再依赖启动期全量重建兜底。
3. 修复后验收：跑 plant-ui 的
   `cargo test -p plant-ui-data --test bran_24381_145018_geometry`（当前红，
   relative_error=0.00296，阈值 1e-3），并确认 `aabb:⟨12450518991627078186⟩` 不再被任何边引用。

## 6.5 复查结论：不是「解析包络」，是**隐含管段的几何混进了这条边的几何集**
（2026-07-28 晚，plant-ui-162 会话；只查不改，现场未动）

**先说结论，它推翻本报告第 2/3 节的定性：那个「松盒子」有几何支撑，而且支撑得严丝合缝。**
拿库里**此刻**这条边的几何集重算，结果与库里那个所谓的错值**逐位相同**：

```
现查合并    mins [9544.75, 9624.393, 18272.3]   maxs [10417.25, 10229.671, 19867.8]
库里新(松)  mins [9544.75, 9624.393, 18272.3]   maxs [10417.25, 10229.671, 19867.803]
库里旧(紧)  mins [9569.75, 9789.443, 18512.3]   maxs [10386.363, 10220.18,  19864.15]
```

复算脚本 `plant-ui/.context/check_valv_aabb_now.py`（与第 3.1 节那份同一套盒变换数学，
只换输入），输入 `bran145018_valv_geos_now.json` 为现查。

**第 3.1 节为什么会得出相反的结论：它手里的几何集少了 20 条。**
`inst_relate:24381_145035` 的 `out` 仍是 `inst_info:⟨13648074980233081144⟩`（没变），
但这个 inst_info 现在挂着 **44** 条 `geo_relate`（`aabb.d` 与 `trans.d` 都非空），
而 13:55 那份快照只有 **24** 条。44 − 24 = 20，**这 20 条全是 `inst_geo:⟨2⟩`**
——共享的单位圆柱，也就是隐含直管段。

```
20  x  inst_geo:2          ← 隐含直管段
 2  x  18232764951396482166
 2  x  4208060549403734237
 2  x  14763471500668989479
 2  x  10628572723856983635
 1  x  16322448502055413187 ...
```

复算脚本里「顶出旧盒边界的几何」那一段列出了 15 条越界项，**`out` 无一例外是 `2`**：
minX / minY / minZ / maxX / maxY / maxZ 六个方向上把盒子撑开的，全部是管段圆柱。
第 3.3 节观察到的「maxZ 恰是 5 号 p-point 的 z、maxX 恰是 3 号 p-point 加管径」
因此有了平实的解释：那不是什么解析包络，就是**从这些 p-point 出发的那几根管子**。

至于 24 → 44 是「13:55 抓快照时 ensure 还在后台写」（第 1 节自己记了冷缓存全程约
11 分钟，13:44 起算正好落在 13:55 附近），还是那 20 条边是后来才挂上去的，本次没有
时间戳可查，两种都解释得通，也都不改变下面这条。

**所以真正要查的问题换了一个：为什么一个 VALV 的 inst_info 几何集里躺着 20 根隐含管段。**
第 5 节记的「隐含管与管件共用 `inst_relate` 记录 id」是同一个主题的另一面——那边是
一条边两个写入方，这边是一个几何集两种来源。同一分支里其余管件的共享看着都正常
（3 个同规格 ELBO 共用 `inst_info:⟨3691487179807065189⟩`、6 个 ATTA 共用
`⟨273164669750072258⟩`，那是内容寻址的正常形态），BRAN 自身那条边的 `out` 又是
裸的 `inst_info:⟨2⟩`——**管段几何在这条分支上的归属整体是乱的**。

**同时澄清两条会把人带偏的旧记载：**

1. **第 4 节「定向重生成链路跑的是 `replace_exist=false`」已经过时。**
   `occ_generate.rs:329` 已强制传 `true`，旁边挂着 ADR-010 D2 的理由；ensure 走的
   `generate_roots → gen_all_geos_data → process_meshes_update_db_deep` 正是这一条。
   传配置值的是另一条 `gen_meshes_in_db`（`:157`）。
2. **`:797-807` 那段「geo 侧算不出就以行内指针为准」的回退没有参与。** 现查
   `geo_aabbs` 有 44 条有效项，重算得出有效盒子，走的是正常写库分支。也就是说
   `update_inst_relate_aabbs_by_refnos` 这次的行为是**对的**：它如实地把当前几何集
   的并集写了回去。

**改法菜单跟着变**（本报告第 6 节原有的建议与本节前一版列的 A–D 一并作废）：

| | 方向 | 说明 |
|---|---|---|
| 1 | 先查清 20 条管段边是怎么挂到这个 inst_info 上的 | 这是唯一的真问题。查 `geo_relate` 的写入点（`save_instance_data` 的 `geo_relate_vec`）在什么条件下会把 `inst_tubi_map` 的几何算进某个管件的几何集 |
| 2 | 顺带把「管段与管件共用 `inst_relate` id」（第 5 节）一起定性 | 两件事很可能同源：管段的归属键取错了对象 |
| ~~3~~ | ~~补一道 `replace_exist=true` 的网格刷新~~ | **作废**，已经在代码里，而且它这次做得对 |

**回归性质待定。** 「02:54 紧 / 13:35 松」这个现象仍然成立，但既然松值是当前几何集
的忠实并集，那么变的是**几何集本身**而不是包围盒口径——`bran_24381_145018_geometry`
那条红测试（`relative_error=0.00296`）钉的也是网格并集口径，它现在钉的到底是
「阀门自己的网格」还是「阀门 + 管段」，得先定下来再说它该不该绿。

## 7. 现场与证据文件清单

- 库内现场（未动）：`inst_relate:24381_145035` 指向松 aabb；旧紧 aabb 记录
  `aabb:⟨6466746607843462392⟩` 仍在。
- `D:\work\plant-code\old\plant-ui\.context\bran145018_before.json` / `bran145018_after.json`
  —— 前后 18 条边快照（in/out/aabb）。
- `D:\work\plant-code\old\plant-ui\.context\bran145018_ensure_result.json` —— ensure 回执。
- `D:\work\plant-code\old\plant-ui\.context\check_valv_aabb.py` + `bran145018_valv_geos.json`
  / `bran145018_valv_wt.json` —— 复算脚本与输入，直接 `python check_valv_aabb.py` 可复验第 3.1 节。
