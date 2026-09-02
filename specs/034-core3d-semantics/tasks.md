# 034 新版 pdms-io 的 Core3D 元素语义层任务

路径前缀：`ENGINE` = `d:\work\plant-code\pdms-io-fork-engine-v2`，`GEN` = 本仓
（`d:\work\plant-code\old\gen-model`）。`[P]` = 可与同组其它 `[P]` 任务并行。

## P0：`core3d_oracle`——先把尺子做出来

- [x] T001 [P] `ENGINE/crates/core3d_model/`（新建 crate：`Cargo.toml` + `src/lib.rs` +
      `src/reference.rs`）：迁入 `GEN/src/data_interface/core3d_reference.rs` 的可执行
      参考模型（`NounBits` / `ModelState` / `SearchMode` / `significant_owner` / `members` /
      `is_pending` / `ancestor_deletes` / `absent_primitives` / `granularity_expansion`），
      随附单测同迁，行为与 gen-model 版逐字节等价。依赖：无。
      2026-08-29 完成：元素 ID 泛型化为 `ElementTree::Id: Copy + Eq + Hash`（gen-model 用
      `RefnoEnum`、引擎用 `RefNo`，钉死具体类型会逼两个消费方各抄一份）；规则函数体与
      原件逐行一致，R 编号 doc 注释全部保留。
- [x] T002 [P] `ENGINE/crates/core3d_model/src/noun_bits.rs`：`trait NounBitSource`
      （significant 与 primitive 两位**分开可查** + 未登记命中计数）+ `SnapshotBits`
      （读 schema 2 快照 JSON，校验 `core_sha256`，**不过即报错不回落**）。快照副本放
      `ENGINE/crates/core3d_model/fixtures/core-noun-granularity-e3d31.json`
      （源：`GEN/tests/fixtures/` 同名文件，字节复制）。依赖：无。
      2026-08-29 完成：字节复制经 SHA-256 双侧核对（A96AEC59…）；`registered_*`（None=未登记）
      与 R0-1 默认口径（未登记=假+计数）分开（C0-2）；自洽校验九条（schema/三表齐全/
      field_type=0/unknown=not_found=0/resolved 对账/真假计数对账/非二值拒绝/noun 集合
      一致/归一化冲突），significant 与 primitive_a 的 field_id 强校验（R0-2 稳定位），
      primitive_b 不钉（跨版本会换的那位）。单测 5 条全绿。
- [x] T003 `ENGINE/Cargo.toml`：workspace members 加 `crates/core3d_model`；
      `ENGINE/crates/pdmsdb_engine_v2/Cargo.toml` 加该依赖。依赖：T001。
      2026-08-29 完成。
- [x] T004 `ENGINE/crates/pdmsdb_engine_v2/src/compare/core3d_oracle.rs` +
      `compare/mod.rs` 注册：oracle 驻点（以 `core3d_model` 参考模型为期望值）+
      `CoreDllBits`（实现 `NounBitSource`，需 E3D 的路径 `#[ignore]` 门）。依赖：T002、T003。
      2026-08-29 完成：`CoreDllBits` 取「新鲜导出」口径——采集动作复用 gen-model 既有
      导出管线（一条规则只有一份实现），本类型负责**不钉版地**装载它（结构校验全开）；
      `CORE3D_FRESH_EXPORT_JSON` 环境变量接入；`diff_noun_bits` 比两侧 noun 宇宙**并集**
      （单侧缺 noun 本身就是分歧）。oracle 自测 2 条全绿（自反零分歧、单翻位点名 noun）。
- [x] T005 [P] C 用例夹具：`ENGINE/crates/core3d_model/tests/c_cases.rs`，
      对参考模型全绿；「非 significant 子节点挡住 significant 孙节点」独立用例（C1-4）。
      依赖：T001。2026-08-29 完成：15 条全绿（C1-4/5/6/7/8/9/10、C3-4 及 R 系补充），
      测试 ID 用 u64 复刻 `RefU64::from_two_nums` 布局，走 crate 公开 API。
- [ ] T006 验收测试：`core_sha256` 篡改后 `SnapshotBits` 加载报错（CI 可跑）✅
      （`tests/core3d_noun_bits.rs` 2 条 + crate 内 tamper 3 形态，全绿）；
      快照 vs FFI 对 1931 noun 全等（`#[ignore]` live）——测试已落
      （`snapshot_agrees_with_fresh_core_dll_export_on_all_nouns_requires_env_core3d_fresh_export_json`，
      变量未设时**失败不静默**），**尚欠现场新鲜导出跑一遍 + 记
      `GEN/docs/2026-08-12_live-test-ledger.md`**。依赖：T004。
- [x] T007 `cargo check`/`cargo test`（ENGINE 侧相关包）+ `cargo fmt`。依赖：T003–T006。
      2026-08-29 完成：`core3d_model` 5+15 全绿；`pdmsdb_engine_v2 --lib` 16 全绿
      （含既有 14 条不回归）；集成测试 2 过 1 ignore（live 档）；fmt 只作用于新文件，
      未触碰工作副本内他人在飞改动（27 个已修改文件原样）。

## P1：db4 元素语义层

- [x] T101 `ENGINE/crates/pdmsdb_engine_v2/src/db4/core3d.rs`（新建）+ `db4/mod.rs` 注册：
      `Core3dSemantics` trait：`is_valid`(R3) / `db_type`(R1) / `climb`(R2) /
      `is_significant`·`is_primitive`(R0-1，另给 `primitive_bits -> (bool,bool)`) /
      `significant_owner`(R14：含自身、无深度上限、visited 环保护) / `members`(R11) /
      `exists`(R26)。每个公开函数 doc 注释回引 R 编号。依赖：T007。
      2026-08-29 完成：生产实现 `Core3dContext<'_, B: NounBitSource>`（打开的库 +
      位表来源）；noun_hash → noun 名走 **db1 词哈希镜像**（`noun_hash`/`noun_name`，
      逐字节镜像 rs-core `db1_hash`/`db1_dehash_const`——引擎是上游 crate 不能依赖
      aios_core，只能镜像；漂移由两条单测钉住：XGEOM↔7739277 已证常量 + 快照 1931
      noun 逐个回环）。R1 读文件头 `0x20..0x24` 类型字就地解码为 `DbKind`（P3 T301
      下沉 db_lookup 前的驻点）；`exists` 的视图 ID 清单判据由调用方 `probe` 闭包注入
      （引擎无 idlist 概念）。crate 根 re-export 全套类型。
- [x] T102 同文件：`members` 三模迭代器——显式栈 LIFO、收集判据与下潜判据两个独立闭包、
      返回迭代器不物化；mode 2 实现但 `#[doc(hidden)]`。依赖：T101。
      2026-08-29 完成：`MembersWalk` 实现 `Iterator<Item=Result<RefNo, EngineError>>`，
      成员读取失败当场冒错并终止游标（原则 III）；mode 2（`members_negative`）negative
      位由谓词注入（该位 P0 按 R16 明确不进快照），标 `#[doc(hidden)]` + R16 死代码警示，
      「收集即不下潜」（`0x1047E48C jmp`）按 core 控制流复刻。
- [x] T103 [P] oracle 驱动：C 用例经 `core3d_oracle` 对 db4 实现全绿（参考模型与生产实现
      同用例同期望）。依赖：T102。
      2026-08-29 完成：用例数据单点化到 `core3d_model::cases`（R11 的 C1-4/C1-5、R14 的
      deep-chain/includes-self/skips-non-significant），参考模型侧 `c_cases.rs` 与 db4 侧
      `tests/core3d_semantics.rs` 吃同一组用例；db4 侧**三方对拍**（生产实现 vs 参考模型
      vs 共享期望，8 条全绿），夹具是真 db 文件（EngineV2 写侧建库，2048 页）。
      **发现并立案两个写侧既有 bug（写侧冻结，本阶段不修）**：
      ① `ElementRefs::serialize_members_block` 与读侧 `parse_members_region` 错位 4 字节
      （refno 落位不符，写出的成员块读侧解析不回），集成测试按读侧权威布局手工构造成员块；
      ② db3 索引页**分裂后大面积丢条目**（512 页叶容量 30，插第 31 条触发分裂，分裂后仅
      第一左叶与边界条目可查），守卫测试 `ENGINE/.../tests/known_issue_index_split.rs`
      （`#[ignore]`，解冻修复后转正），语义层夹具用 2048 页绕开（叶容量 126 > 最大树 65）。
- [x] T104 [P] 未登记 noun 命中计数从 `NounBitSource` 暴露到引擎统计面
      （`PageReadStats` 同级），不许静默。依赖：T101。
      2026-08-29 完成：`Core3dContext::noun_bit_stats() -> NounBitStats`
      （`unregistered_hits` 累计 + `unregistered_nouns` 去重名单），生产链路集成测试
      （SnapshotBits + 真 noun EQUI / 未登记 QQQQZ）验证「位按假答 + 三查三计 + 名单可追」。

## P2：CE 导航栈补齐

- [ ] T201 `ENGINE/crates/pdmsdb_engine_v2/src/db4/ce.rs`：`NavDirection` 驱动
      `navigate(dir)`（走 db3 索引 + db4 记录，不整树加载）；`save_position` /
      `restore_position` 对齐 `DSAVE`/`DRESTO`。依赖：T101。
- [ ] T202 同文件：`owner_chain()` 迭代器；`climb(noun)` 改基于它。依赖：T201。
- [ ] T203 [P] 测试：深度 N 子树遍历 `record_pages_read` 与元素数同阶；
      `NavDirection` 五方向 round-trip 各一条。依赖：T201。

## P3：db2 库类型与 extent

- [ ] T301 [P] `ENGINE/crates/pdmsdb_engine_v2/src/db2/db_lookup.rs`：暴露 `DbKind`，
      数值语义对齐 `DB_DB::type(db) == 1 → DESI`（R1）。依赖：T007。
- [ ] T302 `ENGINE/crates/pdmsdb_engine_v2/src/db2/extract.rs` + `db1/page_store.rs`：
      extent 链解析、按 `(extent, pgno)` 定址；补齐前打开多 extent 库显式报错并点名文件。
      依赖：T301。
- [ ] T303 双 extent 夹具测试（`ENGINE/crates/pdmsdb_engine_v2/tests/`），跨 extent 的
      refno 可定位解析。依赖：T302。

## P4：页大小与会话时点（收尾，根因已由 348d187/cb7dd95 落地）

- [ ] T401 [P] 回归：17 个已知骗过探测器的真库文件（含 `ams7329_0001`）不给 hint 读出
      `page_size=2048` 与权威 sesno，用例进
      `ENGINE/crates/pdmsdb_engine_v2/tests/engine_v2_read_real_db.rs`。依赖：T007。
- [ ] T402 `ENGINE/crates/pdmsdb_engine_v2/src/db2/session.rs`：`open_at(path, sesno)`，
      pin 会话与读最新共用一条实现；与 `PdmsIO::search_latest_refno(_, Some(sesno))`
      对同一 (refno, sesno) 同结果的对拍测试。依赖：T007。

## P5：gen-model 联动（升 rev 收口）

- [ ] T501 pdms-io 侧改动经上游提交，`GEN` 依赖升 rev（`vendor/old-parse-pdms-db` 的
      `pdmsdb_engine_v2` rev + 新增 `core3d_model` 依赖）；开发期 Toggle-LocalDeps
      重定向的，验收前钉回正式 rev。依赖：P0–P4 全部。
- [ ] T502 `GEN/src/data_interface/core3d_reference.rs` 删除，改 re-export 共享 crate；
      引用点（`tests/model_impact.rs` 等）随迁。依赖：T501。
- [ ] T503 `DbElement` 门面（`GEN/docs/plans/direct-dbelement-read-api.md` 的 L2）薄封装：
      分类/遍历/攀爬转调 db4 `Core3dSemantics`，gen-model 内不得存在第二份判据实现
      （`rg` 抽查作验收）。依赖：T501。
- [ ] T504 [P] `GEN/src/data_interface/generation_root.rs`：名单判定接位表，按 R9 口径
      **加层不换判据**；既有三条守卫测试（快照自洽、两位不冗余、SUPPO 唯一分歧）保持绿。
      依赖：T501。
- [ ] T505 验收：`direct_attmap_probe` 走新门面复跑 dbnum 8000/7333 零真值冲突；
      `GEN/src/data_interface/on_demand_db.rs` 的 legacy 多 extent 回退改断言；
      live 结果记台账。依赖：T502–T504。
