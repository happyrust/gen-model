# 房间启动与增量审核：隔离红测试（2026-10-16）

> 当前状态：清边原子性、启动代号/缺水位判据、启动跨库依赖三阶段修复已完成。回归模块 23 过 / 0 红；离线 lib 全量 2535 过 / 0 失败 / 132 ignored。以下前半部分保留修复前历史，最新结果见末尾。真实项目验收与性能验证未完成。

## 范围

用户选择先补隔离红测试，不修业务逻辑。本轮仅向 `src/web_service/room_rebuild_regression_tests.rs` 末尾追加 4 个测试（+116 行），另新增本证据文件。未修改 handlers.rs、room_recompute.rs 或 room_resident.rs；未提交、推送。工作区原有大量改动，不归入本轮成果。

测试只调用纯函数、include_str! 源码契约与独立 mem:// 连接。没有调用 HTTP 重建入口、SUL_DB、真实项目配置、E3D 模型文件或进程镜像写入。命令会正常生成 Cargo 构建产物。

## 新增用例及证据等级

1. `room_startup_audit_regression_missing_watermark_is_not_up_to_date`（行 350）：纯函数行为测试。先验证相同会话号的已知水位可返回 UpToDate，再移除水位行；实际仍返回 `UpToDate { room_updated_at: "2026-10-16T10:00:00+08:00" }`，目标断言失败。
2. `room_startup_audit_regression_current_generation_is_checked_before_up_to_date`（行 380）：源码契约。限定启动 runner 的计划分派前区域，要求出现持久代号与当前模型代号读取。目前缺失而失败。它不是重启行为复现；仅出现相应文本也不充分证明比较逻辑正确。未来若委托给统一 gate，应更新契约并补行为测试，而非添加无效文本取悦测试。
3. `room_startup_audit_regression_foreign_hits_are_handed_off_before_panel_writes`（行 398）：源码契约。限定启动定向更新函数，检查面板持久化之前是否消费 `affected.foreign`。当前缺失而失败。它不证明端到端跨库更新，也不能仅凭文本出现证明交接完成；委托封装时需要同步更新测试。
4. `room_startup_audit_regression_partial_clear_cannot_leave_old_success_proof`（行 416）：内存数据库行为测试。创建两类关系边、g1 水位与完成记录，在夹具中把两个凭据字段定义为非 optional string/object，让 NONE 作废语句确定失败；真实生产清理 helper 返回 Err。事后两张关系表均为 0，而水位仍是 g1、完成记录仍存在，目标断言失败。故障注入是夹具 schema 约束，不是生产 schema 事实、断网测试或进程强杀测试。安全实现可通过事务回滚，或可靠作废成功之后才清边来满足不变量。

所有失败都在目标断言，夹具控制断言已通过，没有 should_panic/ignore 隐藏失败。

## 实际运行

### 仅新增四条

```text
cargo test --offline --lib room_startup_audit_regression_ -- --nocapture --test-threads=1
```

桥接命令 j345，退出码 101，40.9s。编译成功，测试用时 0.43s：

```text
0 passed; 4 failed; 0 ignored; 0 measured; 2655 filtered out
```

目标失败位置依次为测试文件第 390、408、373、456 行。

部分清理原文：

```text
partial clear destroyed edges but kept both old success credentials: members=0, panels=0, generation=Some("g1"), record=Some(RoomUpdateRecord { room_updated_at: "2026-10-16T10:00:00+08:00", room_dbnum_sesnos: {7000: 10}, generation: "g1" })
```

### 原回归模块与新增测试一起运行

```text
cargo test --offline --lib room_rebuild_regression -- --nocapture --test-threads=1
```

桥接命令 j346，退出码 101，1.6s，测试用时 0.44s：

```text
11 passed; 4 failed; 0 ignored; 0 measured; 2644 filtered out
```

注意 Cargo 过滤匹配完整模块路径；新测试也位于 room_rebuild_regression_tests 模块，因此该命令包含全部新测试。原有 11 条全部通过，只有新增 4 条红。原有 catch_unwind 测试会打印预期 panic，但该用例实际通过。

未运行全量测试；普通全量套件现在也会包含这四条有意保留的红测试，不能声称全量通过。

## 诊断和警告

get_problems 对修改的测试文件：errors=0 / warnings=0。Cargo 仍报告 vendor 中三个 dead_code 警告、manifest 的 unused cc，以及 num-bigint-dig/wide 的 future-incompatibility 提示；不声称零构建告警，本轮未处理它们。

get_changes 已检查：桥接返回相对本窗口首次改动前的整个文件 +461 行；其中前 345 行是本轮开始前就存在的内容，本轮仅追加 116 行。

## SHA-256

- 测试文件修改前：984205926594ec8791e64355789e80d84f1d44b8ecfe611dfcffb65aebf9d642
- 测试文件修改后：f6bd4e57939664bbd1bb1efe59b9a4890f5f341f01e33950d6a9630a7b61804b

## 下一步（待用户选择）

优先修复清边原子性，先让内存故障注入测试转绿；再处理启动缺水位/代号核对及跨库交接。后两者应增加可隔离的行为入口，不能把源码契约转绿等同于运行时正确。

## 后续修复：清边事务原子性（2026-10-16，用户选择 atomic_clear）

上述 4 条全红及 +116 行是修复前阶段的历史记录，不是当前状态。本阶段只处理清边原子性，未处理另外三条启动/跨库红测试。

### 实现

`src/web_service/handlers.rs::clear_room_rebuild_edges_on` 在原四条清理语句外加显式 `BEGIN TRANSACTION; ... COMMIT TRANSACTION;`，保留 `.check()` 的逐语句错误检查。两张关系表删除、水位作废、完成记录清空现在一起提交或回滚。没有把整个后台重建包进事务，逐库重算继续沿用原有失败处理；同时修正“任何一步失败都不回滚”的过时注释。

本阶段业务文件 diff 为 +6/-3；未修改 room_recompute.rs、room_resident.rs、changelog.md，也未提交或推送。桥接重连后改动追踪基线重置，当前 get_changes 与前一阶段不是同一基线。

### 隔离测试加强

原故障测试提取共用夹具，增加严格回滚断言，并新增两条行为测试：

- 两个凭据作废都被 schema 拒绝：清理返回 Err，成员边与拓扑边各保留 1，水位 g1 与旧完成记录不变。
- `room_startup_audit_regression_final_record_failure_rolls_back_the_clear`：只拒绝最后的完成记录作废，证明前面的水位更新也随边删除一起回滚。
- `room_startup_audit_regression_successful_clear_invalidates_both_credentials`：无故障，返回成功，两张关系表归零、水位代号 NONE、完成记录 NONE。

这些测试依旧只写各自新建的 mem:// 连接。它们不是生产数据库、断网、强杀或并发压力验收。真实数据库结果不明的传输失败仍需调用方保守处理；本修复证明的是同一显式事务内的写入原子性。

### 实际结果

1. 不排除任何模块内测试：

```text
cargo test --offline --lib room_rebuild_regression -- --nocapture --test-threads=1
```

桥接 j1：编译成功，退出 101，39.7s，测试 0.55s：**14 passed; 3 failed; 0 ignored; 2644 filtered out**。

原有 11 条全部通过，清边相关三条行为测试全部通过。仍失败的仅为：

- room_startup_audit_regression_current_generation_is_checked_before_up_to_date
- room_startup_audit_regression_foreign_hits_are_handed_off_before_panel_writes
- room_startup_audit_regression_missing_watermark_is_not_up_to_date

2. 明确排除上述三个范围外已知红测试，单独验证本次修复及原有回归：

```text
cargo test --offline --lib room_rebuild_regression -- --test-threads=1 --skip room_startup_audit_regression_current_generation_is_checked_before_up_to_date --skip room_startup_audit_regression_foreign_hits_are_handed_off_before_panel_writes --skip room_startup_audit_regression_missing_watermark_is_not_up_to_date
```

桥接 j2：退出 0，1.7s，测试 0.54s：**14 passed; 0 failed; 0 ignored; 2647 filtered out**。

跳过只发生在这条显式范围验证命令，没有给测试加 ignore/should_panic，也没有移除红测试。不排除时仍为 14 过 / 3 红，不能宣称套件全绿。未运行全量 lib 测试。

### 检查

handlers.rs 与测试文件的 get_problems 均 errors=0 / warnings=0；构建依赖警告仍在。get_changes 已检查，本阶段代码 diff 为两文件 +46/-13（后续仅另修正测试注释中的历史计数，不改变测试逻辑）。

修复时 handlers.rs SHA-256：d7ceb3b75eed4faabdf2d82f5a1387c15f833107472ce2baf378d78116021d9c。

下一步仍待选择：启动缺水位/代号核对，或跨库交接；不能以清边测试转绿替代这两项。

## 后续修复：启动代号与水位准入（2026-10-16，用户选择 startup）

### 规则与改动

`plan_startup_room_check` 增加 `current_generation: &str` 参数。先检查记录，再核对代号：当前代号为空、保存代号不匹配（包括旧版仅存时间戳的空代号记录）均返回 NeedsFullInit。任何含房库缺少水位行，也返回 NeedsFullInit，不再 continue 丢弃。只有同代且含房库水位齐全才继续原有时间戳/会话号窗口判断。

生产入口传入 `outcome.model.input_version()`，不是旧记录代号；完整初始化仍走原有 `room_dbnums ∪ expected_dbnums` 目标集，不按 watch 限定。同步更新原有五处纯函数测试调用和说明/日志；完整初始化日志不再一律谎称“未发现记录/首次”，也不在记录写入 helper 无返回值时承诺“已记录时间戳”。

跨代采用保守完整初始化，因为持久记录没有旧拓扑，不能仅凭新拓扑证明受影响面板集合完整。不是每次启动都全量：同代且水位完整仍按原窗口增量逻辑执行。长期缺水位会重复触发补检；本阶段不自动创建水位行、不修改摄入登记流程，也没有修改完整初始化执行体的逐库枚举/盖章实现。真实项目上首次升级/跨代启动的耗时与水位缺失恢复，仍需独立验收。

### 测试证据升级

原代号检查源码钉已替换为真实规划函数的行为测试：相同时间戳、相同会话号下，g1→g1 为 UpToDate；g1→g2、空→g1、g1→空、空→空均 NeedsFullInit。它仍不是进程重启端到端测试。

保留缺水位行为红钉并让它转绿；新增 `missing_watermark_vetoes_an_incremental_plan`：一个库已形成增量窗口、另一个含房库缺水位时，不能只处理前者，而应整体进入完整初始化；控制断言确认只看前者时仍为窗口增量。

新增 `runner_passes_installed_generation_to_planner` 接线契约，限定生产调用参数必须来自当前 outcome.model。这条只证接线，不把源码文本出现当成比较逻辑正确；比较逻辑由上述行为测试覆盖。

### 实际运行

```text
cargo test --offline --lib room_rebuild_regression -- --nocapture --test-threads=1
```

桥接 j3：退出 101，编译成功，40.0s，测试 0.52s：**18 passed; 1 failed; 0 ignored; 2644 filtered out**。

唯一红测试：`room_startup_audit_regression_foreign_hits_are_handed_off_before_panel_writes`，目标断言位置当时为测试文件第 468 行。本阶段未修跨库交接、未改这条测试；原有 11 条与上一阶段清边三条全过，代号和缺水位两条旧红已转绿，两条新增验证也通过。

```text
cargo test --offline --lib data_interface::room_recompute::tests:: -- --test-threads=1
```

桥接 j4：退出 0，1.5s，测试 0.41s：**19 passed; 0 failed; 0 ignored; 2644 filtered out**。包含原窗口判定、缺记录初始化、留白、目标并集与内存数据库窗口/记录读写回归。

未运行全量 lib 测试、没有跳过或隐藏剩余红测试，不宣称套件全绿或真实项目已验收。构建依赖警告仍存在。

### 检查与版本

编辑器语言服务未能提供引用，因此改用 search_files 全 src 查调用；通过编译验证签名调整。两个修改文件的 get_problems 均 errors=0 / warnings=0，get_changes 已检查生产文件变化。本阶段未修改 handlers.rs、room_resident.rs 或 changelog.md，未提交/推送。

- room_recompute.rs：bf1f5132c521d89c356083f18dc18a1e3ee423857420d6c0b496bf780ab910c7
- room_rebuild_regression_tests.rs：7ea60039a49f50d25d0c551e30b15df958f71046b226580256744eab8312542e

下一项：跨库交接仍待处理；其源码契约不能替代跨库行为验证。

## 后续修复：启动跨库依赖先完成（2026-10-16，用户选择 foreign_fix）

### 为什么不直接停放

启动窗口更新可能与当前模型处于同代。现有 `parked_for_task` 在库上代号已与常驻同代时清空停放链，`scope_for_task` 同代分支也不消费这种停放。因此不能只调用 park_foreign_hits 后宣称完成交接。本修复不修改普通任务的换代/停放协议。

### 实现

- `startup_room_update_after_foreign`：去重并按序重算命中的外库；全部成功后才调用本库执行体。任一外库失败返回 false，并且不调用本库面板写入，避免删除反查旧成员所需的证据。其他外库可以继续尝试恢复，但不能盖过失败。
- 启动定向更新真实传入 `affected.foreign.keys()`，外库回调为生产 `recompute_dbnum_room_edges_now`；本库回调中保留原有定向重算/单库兜底。这个返回值直接传回启动 all_ok，失败不推进全局完成记录。
- 若未拿到完整反查（空间树不可用、反查失败、缺少 delta 等），以 `startup_room_fallback_targets` 覆盖「MDB 设计库 ∪ 当前含房库 ∪ 源库」，执行现有逐库整库房间边重算，不再只算源库。该目标集不按监听范围过滤。

成本：外库按整库补算而非只算命中根；反查不可用时覆盖完整 MDB 目标集合，同轮多个触发库可能重复补算。这里先保证覆盖完整，没有声称完成性能优化。真实两库几何变化、故障恢复与大项目耗时仍待验收。

### 新增隔离行为测试（不冒充真实两库几何验收）

1. `foreign_updates_finish_before_local_panel_writes`：对生产依赖顺序函数注入回调，外库列表重复/乱序时每库只调一次，且先完成 7010、7011，再调用本库 7000。
2. `a_failed_foreign_update_blocks_local_panel_writes`：一个外库返回 false，另一个仍可尝试；本库回调从未调用，整体 false。
3. `local_failure_is_not_hidden_by_foreign_success`：有/无外库两种情况下，本库失败均传播 false。
4. `unavailable_reverse_lookup_covers_all_mdb_targets`：目标集合是真正并集；即便其余集合为空也保留源库。

旧跨库源码钉转为接线守卫：要求消费 foreign keys 并返回依赖顺序函数的结果，位置早于本库持久化；另检查反查兜底来自 MDB 目标。它只证明接线位置，实际排序/失败传播由上述回调行为测试证明。没有声称新测试调用了真实两库几何计算器；这部分仍需端到端验收。

### 实际运行

```text
cargo test --offline --lib room_rebuild_regression -- --nocapture --test-threads=1
```

桥接 j5：退出 0，40.0s（含编译），测试 0.54s：**23 passed; 0 failed; 0 ignored; 2644 filtered out**。没有 skip/ignore 掩盖本次用例。最初四条审核红测试均已转绿；代号测试已升级为行为测试，跨库测试仍明确区分行为与接线证据。

```text
cargo test --offline --lib
```

桥接 j6：退出 0，15.2s，测试 14.10s：**2535 passed; 0 failed; 132 ignored; 0 filtered out**，共 2667 条。132 条既有 ignored 测试未运行，不算验收通过；输出中部因显示上限被截断，但结尾汇总与退出码均已返回。结果基于共享工作树，不能把全部计数变化都归因于本轮。

### 最终检查与边界

两个修改代码文件的 get_problems 均 errors=0 / warnings=0；构建仍有 vendor dead_code、manifest unused cc 及 future-incompatibility 提示。已查看 get_changes；未修改 room_reverse.rs、batch_worker.rs、room_resident.rs 或 changelog.md，未提交/推送，未调用真实房间重建 HTTP 入口。

- room_recompute.rs：82255cdd1327553e9cdd7b5483a536c29efe48533ce24662c2a40db806c95598
- room_rebuild_regression_tests.rs：f554d9fa317b3e3b673747f682b6d4fe7e636d35e44fdadd2bbaf075220c7507

当前回归已绿不等于全部审核结束：真实两库几何端到端验证、重启补检和保守全库策略的启动成本尚未实测。



