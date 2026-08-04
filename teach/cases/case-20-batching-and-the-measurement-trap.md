# 案例 20 · 批量化三连，与 debug/release 的 93 倍测量陷阱

<sub>族 F 性能 · 已实施 · 证据层 B（单测 + 一次性实例语义实测）+ C（真实窗口埋点）</sub>

## 一句话

三处「对 N 个元素各做一遍」的循环被压成一次；而在此之前，先得知道**用 `cargo test` 默认的 debug
跑埋点会得出完全错误的结论**——同一份工作 debug 150 s、release 1.6 s，差 93 倍。

## 测量陷阱（先说这条）

同一窗口的 `collect_changes`：

| 构建 | 耗时 |
|---|---:|
| debug | 150,001 ms |
| release | 1,612 ms |

**93 倍**。用 `cargo test`（默认 debug）跑埋点会得出「解析才是瓶颈」的结论，
从而把优化力气全押在解析上——而 release 下解析根本不是瓶颈。

阶段耗时埋点（`StageTimings`）覆盖 collect / plan / persist / cache / rev_index / datacenter / finalize
七段，在每个文件窗口结束时输出一行，**必须用 `--release` 跑**。

附带发现：`collect_changes` 的耗时跨度远比想象大，release 下从 1.0 s（7997 窄窗）到
110 s（7997 完整窗口）。「解析很便宜」只对窄窗口成立。

## 三处批量化

### P0-1 · `update_datacenter_version`：N 次串行往返 → 分块批量

原实现对每个 Deleted / Modified 元素各发一次 `SUL_DB.query().await`，完全串行。
拆成纯函数 `render_datacenter_statements`（**可单测**）+ 分块发送器，每 500 条一次 query。

两处附带修复值得单独记：

- **按 db_type 门控**。`unit = [SUPPO, BRAN, EQUI, ZONE]` 与 `belong_zone` 的语义**只对 DESI 有意义**，
  原来对 SYST / DICT / GLB / CATA 一视同仁地跑。非 DESI 窗口现在直接跳过——
  冷启动的 SYS meta 宽窗口与目录导入不再为**必然空操作**的 UPDATE 买单。
- **错误不再被 `eprintln!` 吞掉**。函数原本永远返回 `Ok(())`，使得 `apply_one` 里那段 warning
  分支**不可达**。

### P0-2 · 缓存失效：14N 次加锁 + 2N 次全量清空 → 14 + 2

`clear_all_caches(refno)` 内部对世界变换缓存走的是 `cache_clear()`（**整表清空**）而不是
`cache_remove(&refno)`，再逐个抢 12 把 async Mutex。N 个 refno 就重复这套 N 遍。

在 rs-core-pin 新增 `clear_all_caches_batch(&[RefnoEnum])`：全局缓存清 1 次、每把锁只拿 1 次、
锁内循环 remove；原 `clear_all_caches` 改为**委派**给它。

> **缓存清单仍然只有一份**——这是没有在 gen-model 里手写缓存列表的原因，
> 否则 core 将来新增缓存时 gen-model 会**静默漏失效**。

### P0-3 · `RegenRoot`：最多 50 次完整生成器 → 1 次

`drain_where` 原本逐个 pending 项调用 `generate_roots`，而后者内部是一次完整的 `gen_all_geos_data`
（flume 管道 + insert task + mesh/布尔全流程）。在线路径 `run_owner_regen` 本来就是把所有根
一次性传进去的，走持久化队列的补偿路径**丢掉了这个批量语义**。

现在按 action 分组、去重后一次性提交；**批量失败回退逐根重试**，所以原来「哪个根坏了就 mark_failed
哪个」的定位能力没有丢失。`target_refno` 解析失败的任务被排除在批量之外，
避免批量路径比逐个路径宽容。

审核后追加的加固（毒根防放大）见案例 [07](case-07-cascade-expand-and-dead-letter.md)：
drain 带 `attempts < 5` 门槛做死信，`attempts > 0` 的根不进批量、单独逐根跑。

## 批量化的语义前提（实测，不是推断）

用 `bin/surreal.exe` 起内存实例（2.1.4，与生产同版）逐条坐实：

- 同一条 query 里重复 `LET $pe = …` 合法，后续语句读到的是新值 → 逐元素 SQL 可以**原样拼接**；
- `type::thing('datacenter_version', pe:1_300)` = `datacenter_version:1_300`，与 `to_table_key()` 一致；
- 一条语句报错**不阻断后续语句**（非事务）→ 批量化保留了原来「尽力而为」的行为；
- `UPDATE` 到不存在的记录 / `NONE` id 是**静默 no-op** → 把错误从吞掉改成上报不会刷屏。

## 可复现命令

release 可执行文件需要 `D:\Rust\target\debug` 里那 104 个 DLL（OpenCASCADE 等），
release 目录下一个都没有，不加进 PATH 会直接 `STATUS_DLL_NOT_FOUND` 退出。

```powershell
$env:PATH = "D:\Rust\target\debug;$env:PATH"
$env:AIOS_FOLD_TEST_FILE = "D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\amssys"

# 窗口规模 / 重复度分布 / 收益估算（只读，不连库）
cargo run --release --bin incr_fold_probe -- --file $env:AIOS_FOLD_TEST_FILE --to 169 --dbnum 8191

# persist A/B（需要先起一次性实例；台架硬性拒绝连向 :8009）
bin\surreal.exe start --user root --pass root --bind 127.0.0.1:8099 memory
cargo test --release --lib -- persist_ab_on_a_throwaway_instance --ignored --nocapture
```

## 遗留项（原样记录）

- `plan` 与 `rev_index` 两个阶段仍未实测；
- **激进折叠**（末态 Deleted 只留墓碑）可再降约 19 个百分点的语句数，但 `Add → … → Deleted`
  的元素将不再在库里留任何记录，需先确认没有消费方依赖那个 `deleted = true` 墓碑；
- `load_base_graph` 逐跳串行 `get_pe` 上溯；同文件的 `fetch_ref_rev_edges` 已经是正确的分块批量写法，
  可照抄改成逐层 BFS；
- `maintain_reverse_index` 对每个非 `None` 变更**无条件**发一条 `DELETE {referrer}->ref_rev;`，
  哪怕该元素没有任何 DependencyCascade 引用属性；
- 同一个文件一轮被打开 3~4 次（`init_watcher` → `SesnoRangeResolver::resolve` →
  `resolve_with_header` → `collect_changes`）。远程共享目录上这个成本不低；
- 全链路裸字符串拼 SQL，无参数绑定。

## 规律

**先确认测量口径，再相信任何性能数字。** debug/release 93 倍不是个别现象——
在 Rust 里凡是涉及大量迭代、序列化、哈希的路径都会有数量级差异。
把「必须 `--release`」写进复现命令，比在报告里提一句可靠得多。

**批量化不能顺手改变错误语义。** 三处改造都刻意保住了原有行为：
datacenter 保留「一条报错不阻断后续」的尽力而为；RegenRoot 保留「哪个根坏了标哪个」的定位能力
（靠批量失败回退逐根）。批量化最常见的退化就是把 N 个独立结果压成一个「全成功 / 全失败」。

**共享的清单只能有一份。** `clear_all_caches_batch` 放在 core 而不是在 gen-model 手写缓存列表——
判据是「将来谁会新增这类条目」：core 新增缓存时，手写副本会静默失效且没有任何提示。

## 关联

- [`../../docs/2026-07-26_increment-persist-optimization-report.md`](../../docs/2026-07-26_increment-persist-optimization-report.md)
- 案例 [19 窗口折叠](case-19-window-folding.md)（同一轮的 P1）· 案例 [07 死信与毒根防放大](case-07-cascade-expand-and-dead-letter.md)
- 跨仓提交须知：改动涉及 gen-model（`increment_pipeline.rs`、`model_update_pending.rs`、
  `src/bin/incr_fold_probe.rs`）与 rs-core-pin（`src/rs_surreal/query.rs`），**必须一起提交**，
  否则单独拿 gen-model 编不过。
