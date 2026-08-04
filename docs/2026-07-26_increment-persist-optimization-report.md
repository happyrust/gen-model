# 增量落库链路优化报告（P0 三项 + 窗口折叠）

日期：2026-07-26

## 结论

- amssys（SYST）冷启动窗口（169 会话）的 persist 阶段从 **~20 s 降到 ~2.6 s（−87%）**，在一次性内存实例上两轮复现一致。
- 收益来自**字节数**而非语句条数：落库 SQL 从 16.73 MB 降到 1.98 MB（−88%），而语句条数只降了 30.7%。在 SYST 库里被反复修改的恰好是带大列表的重元素，它们占语句数不到三成、却扛了载荷的八成半。
- **但这个收益不普适**。本项目三个设计库（7997 / 7999 / 8000）的窗口有 99% 以上是 `Add`、`Modified` 只有个位数到几十个，折叠省下 **0.0%~0.3%**。折叠只在「宽且改动密集」的窗口上有意义，而这恰好就是 SYS meta 库冷启动的形态。折叠本身是 O(n) 一趟扫描，在无效场景下代价可忽略。
- 折叠的等价性在真实文件上验证过：2171 个 refno 的窗口末态与未折叠回放**逐一相同**。
- **测量陷阱**：同一窗口 `collect_changes` 在 debug 下 150,001 ms、release 下 1,612 ms，差 93 倍。用 `cargo test`（默认 debug）跑埋点会得出「解析才是瓶颈」的错误结论。

## 实测数据

### 窗口规模（amssys / dbnum 8191 / sesno 1..=169）

| 指标 | 值 |
|---|---|
| 有变更的会话 | 151 |
| 操作总数 | 5214（Add 2120 / Modified 1626 / Deleted 889 / None 579） |
| 去重 refno | 2171 |
| 平均重复度 | 2.16 次/refno |
| 最热 refno | `24575/1509`，一个窗口内被写 55 次 |

重复度分布（写入次数 → refno 个数）：

| 1 次 | 2 次 | 3 次 | 4 次 | 5 次 | ≥6 次 |
|---|---|---|---|---|---|
| 1226 | 728 | 102 | 32 | 16 | 44 |

### persist A/B（一次性内存实例，非生产库）

| 轮次 | 未折叠 | 折叠后 | 降幅 |
|---|---|---|---|
| 第 1 轮 | 20,524 ms | 2,648 ms | −87.1% |
| 第 2 轮 | 19,075 ms | 2,597 ms | −86.4% |

| 形态 | 语句数 | SQL 体积 |
|---|---|---|
| 未折叠 | 4635 | 16.73 MB |
| 折叠后 | 3213 | 1.98 MB |

两轮交替执行，数值一致，排除预热与顺序效应。

**限制**：空白内存实例没有存量行、没有索引、没有并发，绝对毫秒数不等于生产。可信的是比值。

### 折叠在哪些窗口有效

同一个探针扫过全部四个库。设计库分别取「完整窗口」和「最近 13 个会话」两种口径：

| 库 | 类型 | 窗口 | 落库操作数（不含 None） | Add | Modified | Deleted | 折叠收益 |
|---|---|---|---|---|---|---|---|
| 8191 amssys | SYST | 1..=169 | 4635 | 2120 | 1626 | 889 | **−30.7%** |
| 7997 | DESI | 1..=83 | 176,219 | 176,181 | 14 | 24 | −0.0% |
| 7997 | DESI | 69..=83 | 1926 | 1902 | 11 | 13 | −0.3% |
| 7999 | DESI | 1..=41 | 101,125 | 101,102 | 9 | 14 | −0.0% |
| 7999 | DESI | 27..=41 | 34,662 | 34,651 | 9 | 2 | −0.0% |
| 8000 | DESI | 1..=30 | 30,745 | 30,692 | 52 | 1 | −0.1% |
| 8000 | DESI | 16..=30 | 5088 | 5063 | 24 | 1 | −0.2% |

设计库的历史几乎全是元素创建，平均重复度恒为 1.00 次/refno，最热的 refno 也只被写 4~6 次。
**折叠对它们等于空转**——但也不亏，那趟扫描是 O(n) 的，相对于渲染几十万条语句可忽略。

真正吃到收益的是 amssys：项目结构库里 MDB / DB / TEAM 这类定义会被跨会话反复编辑，而它们又
恰好带大成员列表。

一个附带发现：`collect_changes` 的耗时跨度远比 amssys 那一次大得多，release 下从 1.0 s
（7997 窄窗）到 110 s（7997 完整窗口）。「解析很便宜」只对窄窗口成立。

注：设计库的完整窗口在生产中不会自然出现——`COLD_START_DB_TYPES` 只放行 SYST/DICT/GLB/GLOB，
DESI 必须靠人为压水位才会重放到 sesno 1。上表列它只是为了看清窗口成分。

## 改了什么

### P0-1 `update_datacenter_version`：N 次串行往返 → 分块批量

原实现对每个 Deleted / Modified 元素各发一次 `SUL_DB.query().await`，完全串行。拆成纯函数
`render_datacenter_statements`（可单测）+ 分块发送器，每 500 条一次 query。

另外两处附带修复：

- **按 db_type 门控**。`unit = [SUPPO, BRAN, EQUI, ZONE]` 与 `belong_zone` 的语义只对 DESI
  有意义，原来对 SYST / DICT / GLB / CATA 一视同仁地跑。非 DESI 窗口（含 CATA）现在直接
  跳过——冷启动的 SYS meta 宽窗口与目录导入都不再为必然空操作的 UPDATE 买单。
- **错误不再被 `eprintln!` 吞掉**。函数原本永远返回 `Ok(())`，使得 `apply_one` 里那段
  warning 分支不可达。

### P0-2 缓存失效：14N 次加锁 + 2N 次全量清空 → 14 + 2

`clear_all_caches(refno)` 内部对世界变换缓存走的是 `cache_clear()`（整表清空）而非
`cache_remove(&refno)`，再逐个抢 12 把 async Mutex。N 个 refno 就重复这套 N 遍。

在 rs-core-pin 新增 `clear_all_caches_batch(&[RefnoEnum])`：全局缓存清 1 次、每把锁只拿 1 次、
锁内循环 remove；原 `clear_all_caches` 改为委派给它。**缓存清单仍然只有一份**——这是没有在
gen-model 里手写缓存列表的原因，否则 core 将来新增缓存时 gen-model 会静默漏失效。

### P0-3 `RegenRoot`：最多 50 次完整生成器 → 1 次

`drain_where` 原本逐个 pending 项调用 `generate_roots`，而后者内部是一次完整的
`gen_all_geos_data`（flume 管道 + insert task + mesh/布尔全流程）。在线路径 `run_owner_regen`
本来就是把所有根一次性传进去的，走持久化队列的补偿路径丢掉了这个批量语义。

现在按 action 分组、去重后一次性提交；**批量失败回退逐根重试**，所以原来「哪个根坏了就
mark_failed 哪个」的定位能力没有丢失。`target_refno` 解析失败的任务被排除在批量之外，避免
批量路径比逐个路径宽容。

审核后追加的加固（毒根防放大）：drain 的 SELECT 现在带 `attempts < MAX_ATTEMPTS(5)` 门槛
（与 `side_effect_pending` 同策略），连续失败的任务留在表里做死信，不再每个 watcher 周期
白付一次生成；`attempts > 0` 或解析失败的根**不进批量、单独逐根跑**，避免一个已知坏根把
整批健康根反复拖进「批量失败 → 逐根回退」的双倍代价。死信对手动更新路径仍然可见
（`load_pending_model_units` 不带该门槛），预览 / 手动重试是检视与复活死信的入口；新会话
再次触及同一目标时 `render_upsert` 会把 attempts 归零，任务自动复活。

### P1 窗口折叠：同 refno 的连续 Modified 合并为一条

本模块的契约是「只保留最新状态，不写 sessions / element_changes 历史表」，所以逐会话回放
中间态是纯粹的浪费：一个 refno 在窗口里被改 N 次，就产生 N 组 `UPSERT … MERGE` +
`UPDATE pe`，而最后一条已经覆盖了前面所有。

三个设计决定：

1. **只作用在 `persist_latest_main_data` 内部**。`range_eles` 本体不动，模型计划、反向索引、
   缓存失效、datacenter 四个下游看到的仍是原始数据，爆炸半径最小。
2. **按 key 的 last-writer-wins，不是 union**。这是唯一真正容易写错的地方：会话 5 删属性
   X、会话 9 又加回 X，简单 union 会让 X 同时出现在 `added` 和 `deleted` 里，而
   `to_modify_surql` 把 `deleted` 放在最后处理——值会被静默抹成 `NULL`。
3. **合并后的语句放在 run 最后一个操作的位置**，全局语句顺序不变。安全的前提是：生成的语句
   里每个值都是字面量，**没有任何一条语句会读另一条记录**，所以丢掉中间写不可能改变后续
   语句看到的东西。`Add` / `Deleted` 不参与合并，建记录→立墓碑的顺序原封不动。

## 等价性怎么验证的

### SurrealQL 语义（P0 批量化的前提）

用 `bin/surreal.exe` 起内存实例（2.1.4，与生产同版）实测，不是推断：

- 同一条 query 里重复 `LET $pe = …` 合法，后续语句读到的是新值 → 逐元素 SQL 可以**原样拼接**；
- `type::thing('datacenter_version', pe:1_300)` = `datacenter_version:1_300`，与 `to_table_key()` 一致；
- 一条语句报错**不阻断后续语句**（非事务）→ 批量化保留了原来「尽力而为」的行为；
- `UPDATE` 到不存在的记录 / `NONE` id 是静默 no-op → 把错误从吞掉改成上报不会刷屏。

### 折叠（P1）

- 10 个单测覆盖顺序敏感的边界：删后重加、加后再删、跨三个命名空间的 last-writer、
  `Add`/`Deleted` 打断 run、无关 refno 保序、最新子列表胜出。
- `folding_a_real_window_preserves_final_state`：在真实 E3D 文件上把原始序列与折叠序列**各自
  回放一遍**，逐 refno 比对末态。回放用的是独立写的状态机，不复用 `fold_window` 的 run 检测
  逻辑——否则就是自己证自己。只需文件、不连库。

## 可复现命令

release 可执行文件需要 `D:\Rust\target\debug` 里那 104 个 DLL（OpenCASCADE 等），release
目录下一个都没有，不加进 PATH 会直接 `STATUS_DLL_NOT_FOUND` 退出。

```powershell
$env:PATH = "D:\Rust\target\debug;$env:PATH"
$env:AIOS_FOLD_TEST_FILE = "D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\amssys"

# 窗口规模 / 重复度分布 / 收益估算（只读，不连库）
cargo run --release --bin incr_fold_probe -- --file $env:AIOS_FOLD_TEST_FILE --to 169 --dbnum 8191

# 折叠等价性（只读文件，不连库）
cargo test --release --lib -- folding_a_real_window_preserves_final_state --ignored --nocapture

# persist A/B（需要先起一次性实例；台架硬性拒绝连向 :8009）
bin\surreal.exe start --user root --pass root --bind 127.0.0.1:8099 memory
cargo test --release --lib -- persist_ab_on_a_throwaway_instance --ignored --nocapture

# 设计库窗口成分（上面那张对照表）
$base = "D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000"
D:\Rust\target\release\incr_fold_probe.exe --file "$base\ams7997_0001" --from 69 --to 83 --dbnum 7997
D:\Rust\target\release\incr_fold_probe.exe --file "$base\ams7999_0001" --from 27 --to 41 --dbnum 7999
D:\Rust\target\release\incr_fold_probe.exe --file "$base\ams8000_0001" --from 16 --to 30 --dbnum 8000
```

阶段耗时埋点（`StageTimings`）在每个文件窗口结束时输出一行，覆盖
collect / plan / persist / cache / rev_index / datacenter / finalize 七段。**必须用
`--release` 跑**，理由见开头的测量陷阱。

## 遗留项

- `plan` 与 `rev_index` 两个阶段仍未实测。
- 激进折叠（末态 Deleted 只留墓碑）可再降约 19 个百分点的语句数，但 `Add → … → Deleted`
  的元素将不再在库里留任何记录（`UPDATE` 到不存在的记录是 no-op），需先确认没有消费方依赖
  那个 `deleted = true` 墓碑。
- `load_base_graph` 逐跳串行 `get_pe` 上溯；同文件的 `fetch_ref_rev_edges` 已经是正确的分块
  批量写法，可照抄改成逐层 BFS。
- `maintain_reverse_index` 对每个非 `None` 变更无条件发一条 `DELETE {referrer}->ref_rev;`，
  哪怕该元素没有任何 DependencyCascade 引用属性。
- 同一个文件一轮被打开 3~4 次（`init_watcher` → `SesnoRangeResolver::resolve` →
  `resolve_with_header` → `collect_changes`）。远程共享目录上这个成本不低。
- 全链路裸字符串拼 SQL，无参数绑定。

## 跨仓提交须知

改动跨两个仓库，**必须一起提交**，否则单独拿 gen-model 编不过：

| 仓库 | 文件 |
|---|---|
| gen-model | `src/data_interface/increment_pipeline.rs`、`src/data_interface/model_update_pending.rs`、`src/bin/incr_fold_probe.rs`（新） |
| rs-core-pin | `src/rs_surreal/query.rs`（`clear_all_caches_batch`） |

rs-core-pin 是 `Cargo.toml` 里 patch 指向的钉版 aios_core 本地副本；该仓另有
`prim_geo/*` 与 `rs_surreal/geom.rs` 的既有未提交改动，**不属于本次优化**，提交时应只暂存
`src/rs_surreal/query.rs`。
