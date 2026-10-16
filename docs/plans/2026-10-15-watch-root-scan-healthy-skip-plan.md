# 监控目录重解析加健康闸：搜索根扫描只在有缺口时做（开发计划）

- 日期：2026-10-15
- 关联：ADR-078（项目闭包）、ADR-085 D3（重挂轮补目录后先重绑再补扫）、`docs/plans/2026-10-15-mdb-bind-directory-scan-stall-fix-plan.md`（同族：远程盘上的扫描都要限时、要出声）、`docs/plans/2026-09-29-update-scope-reads-the-registered-declaration-fix-plan.md`（同族：读路径不碰盘）
- 起因：用户现场日志——`[root-scan] 搜索根扫描开始/完成: root=D:\AVEVA\Projects\E3D3.1` 在稳态下反复出现（本地盘 hits=8 elapsed_ms=0）。问「初始化扫一次不就行了」。
- 状态：**已实施**（2026-10-15，T1–T5 全落地；`cargo test --lib` 2509/0，钉与真值表见 T4）。

---

## 1. 问题定位

`scan_roots`（`src/data_interface/project_locator.rs:96`）本意是给「项目码 → 库目录」的定位服务，设计时刻只有两个角色：

- **权威重做时刻**（ADR-085）：startup / syst-refresh / remount / fallback——盘上真值可能变了的时刻，必须现算；
- **读者**：成员清单取 `mdb_membership::get()` 登记表、闭包取 `latest_for` 缓存——零盘 IO。

现状有两处把这个边界踩穿了，扫描成了周期/逐任务行为：

**P1 · 重挂轮每拍无条件重解析**（`src/data_interface/increment_manager.rs:4817`）。`async_watch` 的事件循环里 `remount_tick` 每 `AIOS_WATCH_REMOUNT_SECS` 秒（默认 60，`5020-5023`）调 `remount_watch_dirs`，而它开头就是 `plan_watch_dirs(&db_option)` → `resolve_for` → `scan_roots`——**不管有没有目录缺席，每拍都真扫全部搜索根**。重新解析的必要性注释写在 `4795-4800`：启动时掉线的共享盘连它的 `*000` 目录都列不出来，那些目录从没进过启动清单，只重试老清单永远等不到。理由成立，但「上一轮解析完整、没有目录缺席」的健康态下每拍重扫没有收益——远程根上每拍还押着事件循环等最多 15 s。

**P2 · 任务开工探针每任务重解析**（`src/fast_model/e3d_model_service.rs:3149`）。`current_mdb_source_inventory()` 里 `plan_watch_dirs(option)` 同样是无条件 `resolve_for`。它被 `current_mdb_sources()`（`3208`）调用，后者挂在 `latest_frozen_sources`（`room_topology.rs:389`，ADR-073 D12 **每个 dbnum 任务开工前探一次**）、生成入口、`model_source` 逐次查找上——每起一个任务就真扫一遍搜索根。探针自己的注释写着「这是探针，得便宜」（`room_topology.rs:370`），而紧邻的上一行成员清单走的就是 `get()` 注册表（`3144-3148`）：闭包一边用缓存、一边现算，口径还不一致。

两处共同结果：用户看到的「循环扫描」。本地盘上只是日志噪音，共享盘现场是周期性远程 IO + 探针延迟。

## 2. 目标

- **稳态零搜索根 IO**：上一轮闭包完整（所有根可达、无未定位项目、无声明错误）且没有目录缺席/失联时，重挂轮整轮跳过 `plan_watch_dirs`；
- **有缺口照常自愈**：根不可达 / 项目未定位 / 目录缺席 / 刚失联——任一成立就走现有的「重解析 → 补挂 → `RebindReason::Remount` 重绑 → 补扫」全链，逐字不变；
- **探针走缓存**：`current_mdb_source_inventory` 改用 `latest_for`（key 命中零扫描、miss 才现算），与成员清单的 `get()` 兜底同口径；
- 权威重做时刻（startup / syst-refresh / remount / fallback / 向导「看闭包」）的 `resolve_for` 现算语义**不动**——`latest` 缓存恰由它们刷新，闸与探针读的都是「最近一次权威解析」。

## 3. 任务分解

### T1 闭包缺口判定 + 只读缓存口（`src/data_interface/mdb_closure.rs`）

- `ClosurePlan::has_watch_gaps(&self) -> bool`：根不可达 ∪ 监控目录计划有 problem，一条覆盖全部缺口形态：

  ```rust
  self.roots.iter().any(|root| !root.reachable)
      || !self.watch_dir_plan().problems().is_empty()
  ```

  `watch_dir_plan().problems()` 已把「主项目未定位 / 声明的项目码无目录 / `*000` 全缺 / 声明读不到」逐条折成 problem（`mdb_closure.rs:484-531`），不另立清单；`roots` 可达性单查——「根掉了但它下面没有声明项目」不产生 problem 条目，可那根恢复后正要靠重解析发现新目录，不能漏。
- `pub fn cached_plan_for(db_option: &DbOption) -> Option<Arc<ClosurePlan>>`：只查 `latest_slot`、key 不符或没有就 `None`，**永不现算**（重挂轮闸位用——`latest_for` 会在 miss 时现算，闸位不许碰盘）。key 复用 `plan_key`（字面根 key，不走 `canonicalize`，见 `755-758` 注释）。
- 纯函数/纯缓存，`#[cfg(test)]` 直接喂 `ClosurePlan::build` 断言真值表。

### T2 重挂轮加闸（`src/data_interface/increment_manager.rs` `remount_watch_dirs`，4804-4908）

顺序调整为「先免费检查、有缺口才重解析」：

```rust
async fn remount_watch_dirs(&self, watcher, mounted) {
    // ① 已存清单的健康复查（无搜索根 IO；refresh_health/missing 语义不变）
    let dirs = self.watch_dirs();
    let newly_lost = mounted.refresh_health(&dirs);   // 照旧告警
    let released = mounted.unwatch_lost(watcher);      // 照旧打行
    let missing = mounted.missing(dirs);               // 失联目录 unwatch 后自然落进 missing

    // ② 闸：全健康 = 整轮跳过，零搜索根 IO
    let plan_has_gaps = mdb_closure::cached_plan_for(&self.db_option)
        .map(|plan| plan.has_watch_gaps())
        .unwrap_or(true);                            // 没解析过 = 有缺口
    if !plan_has_gaps && missing.is_empty() {
        return;
    }

    // ③ 有缺口 → 现有链路逐字保留：plan_watch_dirs → record_watch_dir_owners /
    //    record_discovered_watch_dirs → mounted.missing(新清单) → mount →
    //    RebindReason::Remount 重绑 → sweep_dirs(share-remount 子集)
}
```

- `known`（4809-4814 的启动快照集合）、`record_discovered_watch_dirs`、补挂后「先重绑再补扫」的顺序全部不动——钉 `a_remount_rebinds_members_before_the_catch_up_sweep`（`2168`）必须原样过；
- 跳过时不出声（健康态刷日志正是要消掉的东西），至多 `log::debug!` 一行；
- `refresh_health`/`unwatch_lost` 每拍照跑（本就如此；已挂目录的可达性复查不属于本计划要动的成本面）。

### T3 探针换缓存（`src/fast_model/e3d_model_service.rs:3149`）

```rust
// 现：
let watch_dirs = crate::data_interface::project_paths::plan_watch_dirs(option);
// 改：
let watch_dirs = crate::data_interface::mdb_closure::latest_for(option).watch_dir_plan();
```

- `record_watch_dir_owners(&watch_dirs)`（`3150`）照旧——登记表是合并语义，缓存计划喂进去幂等；
- miss（启动早期缓存未建 / 配置换了）时 `latest_for` 内部落到 `resolve_for`，语义与今天相同；
- 顺带消掉一个隐患：该函数在 async 调用链里同步跑 `scan_roots`（最坏 15 s 押 tokio worker），命中缓存后这条暴露面也没了；
- `plan_watch_dirs`（`resolve_for` 版）保留给启动挂 watcher、T2 的缺口分支、向导「看闭包」这些要求新鲜真值的时刻，签名不动。

### T4 测试与钉

- **真值表**：`has_watch_gaps`——干净闭包 false；根不可达 true；`unresolved` 非空 true；主项目 `main=None` true；`declaration_error` true；`cached_plan_for` key 命中/未命中（沿用 `mdb_closure` 测试的 Fixture + `scan_roots` 临时根风格）。
- **行为钉**：新用例 `a_healthy_remount_tick_never_rescans_search_roots`——钉 `remount_watch_dirs` 函数体里 `cached_plan_for`/`has_watch_gaps` 与早退 `return` 都排在 `plan_watch_dirs` 之前（照 `a_remount_rebinds_members_before_the_catch_up_sweep` 的 `include_str!` + 位置断言写法）；闸内分支之后的 `RebindReason::Remount` 仍在 `share-remount` 补扫之前。
- **探针钉**：`current_mdb_source_inventory` 函数体含 `latest_for`、不含 `plan_watch_dirs`（新用例，挂 `service_source()` 族钉旁边）。
- `a_share_that_comes_back_is_replanned_and_mounted_once`（`project_paths.rs:841`）直调 `plan_watch_dirs`，不受闸影响，原样过——它也是「重解析机制仍在」的回归门。
- 全量：`cargo test --lib`；受影响模块 `increment_manager` / `mdb_closure` / `e3d_model_service` / `project_paths` 单独跑绿。

### T5 文档

- `remount_watch_dirs` 头注（`4795-4803`）与 `AIOS_WATCH_REMOUNT_SECS` 注释（`5016-5019`）改口：每拍重试挂载不变，**重新解析只在有缺口时**；
- ADR-085 追记一条（D3 补：重解析加健康闸的判据与「新目录发现」取舍边界）；
- changelog 2026-10-15 条目。

## 4. 决策点（推荐项已标）

- **D1 闸位判据**：`has_watch_gaps ∪ missing`（**推荐**） vs 只看 `missing`。只看 missing 会丢掉「启动掉线的盘，目录从没进清单」这条主自愈路径——`missing` 对从没见过的目录恒为空，唯有「上一轮解析有缺口」能记住它们。
- **D2 健康态下「往已挂载根里新拷一个项目目录」要不要兜底发现**：**推荐不兜底**——这正是「初始化一次」语义的另一面；恢复手段是 SYST 刷新触发重绑、或重启。真出现这类现场再加一个慢拍兜底（如 `AIOS_WATCH_REPLAN_SECS`，默认关），本计划不预建。
- **D3 跳过节拍是否出声**：**推荐静默**（至多 `log::debug!`）。`[root-scan]` 行存在的意义就是给「卡住的根」定位，健康态打行只会重演今天的困惑。

## 5. 验收门

- [ ] 单测全绿，T4 真值表与三条钉通过；
- [ ] 稳态（全部目录已挂、闭包无 problem）下连续 N 个 `AIOS_WATCH_REMOUNT_SECS` 拍**不出现** `[root-scan]` 行；
- [ ] 拔掉共享盘 → 拍内出现「监控目录失联」；接回 → 重挂行 + `reason=remount` 阶段行 + 补扫行——自愈链与今天逐字一致；
- [ ] 任务开工（`latest_frozen_sources` 路径）不再伴随 `[root-scan]`；
- [ ] `AIOS_WATCH_REMOUNT_SECS=0` 旧语义不变（仍是整体关闭重挂）。

## 6. 范围外

- `scan_roots` 内部（15 s 超时、逐根出声、三种摆法）——不动；
- 300 s 周期对账 `sweep_watch_dirs`（扫的是已存清单内的库文件，不扫根）——不动；
- `refresh_health` 对已挂目录的逐目录可达性检查（现存成本，UNC 掉线时的阻塞语义另案）；
- 新目录/新项目在**全健康**运行期的发现（D2 推掉的那条）；
- `latest` 缓存容量/多配置并存（单 DbOption 进程，现有一格缓存口径不动）。

## 7. 风险

- **闸判错把缺口当健康**：`has_watch_gaps` 判据取 `watch_dir_plan().problems()` 同一出口，漏判 = 把某种缺口形态漏出 problem 之外——已用「根可达性」单查兜底「根掉了但无声明项目」那格；真值表测试覆盖五格。
- **`cached_plan_for` 与 `latest` 分叉**：`latest` 由每个 `resolve_for` 写（启动、SYST 刷新、重挂、兜底都写），闸读的是同一份；若有人日后新增一条绕开 `resolve_for` 的解析路径，闸会读到旧 plan——用 T4 的钉 + ADR 追记把「权威解析只走 `resolve_for`」这条不变量写明。
- **顺序重排的回归**：`refresh_health`/`unwatch_lost` 从「重解析后」挪到「闸前」，失联目录在本拍就进 `missing`（以前是等下一拍）——只会更早、不会更晚，方向是安全的；补挂路径的「先重绑再补扫」钉保证。
- **探针拿到旧计划**：`latest_for` 命中的是最近一次权威解析，与 `get()` 成员清单同一代——比今天「计划新鲜、清单旧」的混配口径更一致，不算回归。

## 8. 相关文件

- `src/data_interface/mdb_closure.rs`（`ClosurePlan::has_watch_gaps` 新增、`cached_plan_for` 新增、`latest_for`/`resolve_for`/`watch_dir_plan`/`problems`）
- `src/data_interface/increment_manager.rs`（`remount_watch_dirs` 4804-4908 重排、remount_tick 5129-5131、`AIOS_WATCH_REMOUNT_SECS` 5020-5023、钉 2168）
- `src/fast_model/e3d_model_service.rs`（`current_mdb_source_inventory` 3139-3206，3149 行换 `latest_for`）
- `src/data_interface/project_paths.rs`（`WatchDirPlan`/`MountState`/`record_discovered_watch_dirs`/`record_watch_dir_owners` 语义依据）
- `src/fast_model/room_topology.rs`（`latest_frozen_sources` 375-422，探针调用方）
- `docs/adr/ADR-085-*.md`（D3 追记）、`changelog.md`
