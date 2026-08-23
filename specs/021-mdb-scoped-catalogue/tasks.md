# Tasks 021：Catalogue 相位按 MDB 声明口径收敛

- [ ] T001 [US1..3] `specs/021-mdb-scoped-catalogue/spec.md`：评审两条定稿决定——
      (a) CATA 相位成员改由 MDB 声明；(b) **相位屏障不放宽**（依据 `DB_MDB::openMDB`
      的 fail-fast + `internalCloseMDB` 回滚）。同时确认「被排除的 CATA 从此不建立
      完整应用水位」是可接受的，且与监听限定域下的既有行为同口径。
- [ ] T002 `docs/evidence/2026-08-20-core-dll-dbno-namespace.md`（新文件）：落四条
      IDA 取证结论，每条带函数名、地址与关键反编译片段——`DB_DB::findDB(int)` 的单键
      map（`0x6395048`）、`findDB(int,int)` 第二键是抽取号（+136 / +348 / +352 与
      `extractChildNumber` / `leafExtract` 互证）、`checkDBNoInuse` 用
      `DB_System::getAllDbs` 强制 dbno 全局唯一（253 号消息）、`DB_MDB::openMDB`
      成员失败即全部回滚。**先落档再改代码**：它是 T010 与 plan 里「不放宽屏障」
      的唯一依据。
- [ ] T003 [US1] `src/data_interface/update_scope.rs`：**先落失败回归**——
      `admits` 的 CATA 分支单测（声明过的放行 / 没声明的挡住 / SYS meta 与
      `unrestricted` 两条早退不受影响）+ `interpret` 的冷启动三分单测
      （无 MDB → bootstrap 那句；MDB 在但无 CATA → 新的那句；名字打错 → `bail!`）。
      此时函数还没改，必红。
- [ ] T004 [US1] `src/data_interface/update_scope.rs`：把 `MDB_DBNOS` 常量改成
      `fn mdb_dbnos(param: &str) -> String`（今天写死 `$db_type`，一次往返问两种类型
      会撞名）；`fetch` 发三条 `RETURN`（MDB 名字列表 / `$desi_type` / `$cata_type`），
      **保持一次往返**；`UpdateScope` 增 `cata: BTreeSet<u32>`；`interpret` 收两份
      dbnos 并三分告警（`warning` 需能装两条，`Option<String>` → `Vec<String>`，
      同步 `scope.warning()` 的全部调用点）；`admits` 的 CATA 分支改为查名单；
      增 `declared_cata()`。让 T003 转绿。
- [ ] T005 [US1] `src/data_interface/update_scope.rs` + `increment_manager.rs`：
      新增 `for_tests_with_catalogue(mdb, desi, cata)`；**逐个**检查既有 4 处
      `UpdateScope::for_tests` 调用点——CATA 从「恒放行」变成「恒不放行」，
      凡断言涉及 CATA 的改用新构造器，不涉及的原样留着。
- [ ] T006 [US2] `src/data_interface/increment_manager.rs`：**先落失败回归**——
      源码顺序断言，`dependency_identities.push(` 必须早于 CATA 范围门、
      范围门必须早于 `by_type.entry(`。这条顺序就是 US2 的全部内容：
      定位索引全量、相位成员收窄。此时门还没加，必红。
- [ ] T007 [US1][US2] `src/data_interface/increment_manager.rs::catalogue_manifest_for_dirs`：
      签名增 `scope: &UpdateScope`（调用点 `sweep_dirs` 手上已有 `scope`，
      上面一行刚用过 `scope.warning()`）；只对 `db_type == "CATA"` 过
      `scope.admits`；**DICT 一字不改**（它在 `COLD_START_DB_TYPES` 里）；
      `dependency_identities` 一条不动。让 T006 转绿。
- [ ] T008 [US1] `src/data_interface/increment_manager.rs`：第四个范围桶
      `mdb_excluded_cata`，循环外聚合成一句，点名「MDB `{mdb}` 的 CURD 里没有声明
      这些目录库」并说明「它们仍可被按需引用闭包定位」。补一条与
      `the_sweep_keeps_watch_exclusions_out_of_the_other_two_buckets` 同型的断言，
      把四个桶两两隔开（issue #10 的嗓音混同教训）。
- [ ] T009 [US1] `src/data_interface/manual_update.rs`（约 3959–4019 行那份清单选择）：
      喂 `select_catalogue_candidates` 之前过同一道 `scope.admits("CATA", …)`
      （`scope` 已由 `self.update_scope(mdb).await?` 解出）。补一条源码形状断言：
      禁止在清单选择里出现第二个手写的 CATA 名单比对（宪法 II）。
- [ ] T010 [US3] `src/data_interface/initialization_phase.rs`：**不改行为**，
      只补一条回归钉住 SC-006b——MDB 声明过的 CATA 出身份 blocker 时，
      Catalogue 相位不就绪、Design 不被派发。它防的是日后有人「顺手」缩小爆炸半径。
- [ ] T011 [US1] `src/web_service/handlers.rs`：`/health` 报本期声明的 CATA 数量
      （与既有 `declared_desi` 同一处出口）；手动 preview / execute 回执同批带上。
- [ ] T012 文档四处：`docs/adr/ADR-025-strict-initialization-phases.md` 第 2 条加修订
      注记（Catalogue 成员 = MDB 声明的 CATA，第 3、6 条不动）；
      `specs/006-strict-initialization-order/spec.md` FR-005 对齐；
      `CONTEXT.md` 立「相位成员 (Phase Member)」词条并在「严格初始化阶段」条补一句
      CATA 口径；`changelog.md` 一条。
- [ ] T013 [US1] live 验收（现场四项目机器）：SC-001 `/health` 的
      `initialization.blockers` 为空、`status` 到 `model_ready`；SC-004 逐 dbnum 对拍
      MDB 声明的每个 CATA 都进了 Catalogue 相位并建立完整应用水位，一个不少。
- [ ] T014 [US2] live 验收（安全带）：SC-002 `[manifest] … 依赖身份清单：N 个文件`
      的 N 与改动前**相同**（现场基线 544）。
- [ ] T015 [US1] live 验收（对照组）：SC-003 把 `catalogue_project_priority` 整键
      注释掉重启，SC-001 仍然成立——证明冲突是被消灭而不是被仲裁掉的。
- [ ] T016 [US2] live 验收（几何）：SC-005 取一个已知引用了跨项目目录库的生成根，
      改动前后几何逐字节对拍。**这是本特性唯一会静默出错的地方**，不能只看日志。
- [ ] T017 冷启动验收：SC-006，用一次性空 SurrealDB 命名空间跑一轮，
      断言日志出现的是 bootstrap 那句、**不是**「MDB 未声明 CATA」那句；第二轮跑出真范围。
      **不要拿现场库试。**
- [ ] T018 `cargo fmt`；定向 `cargo test --lib data_interface::update_scope` 与
      `data_interface::increment_manager`；CI 口径全量单测
      （`--no-default-features --features ws,gen_model,manifold,project_hd`）；
      `cargo check`（release 口径带 `occ,http_api`）。

## Dependencies

- T002 在一切代码改动之前（它是 T010 的依据，也是 ADR 修订的引用来源）。
- T003 → T004 → T005（先红后绿，再收调用点）。
- T006 → T007 → T008（同上）。
- T009 依赖 T004（要有 `admits` 的新语义）；可与 T006/T007 并行。
- T010、T011 在 T007 之后；两者可并行。
- T012 在代码终态之后。
- **live 顺序不许颠倒**：T013 → T014 → T015 → T016。T014 是 T013 的安全带，
  T016 是唯一能抓出静默画错的一步。T017 单独用空命名空间，与 T013–T016 无序关系。
- T018 最后。

## Notes

- 本特性最大的风险不是「改错」而是「改静默」：它的动作就是让一批库不再参与。
  T008 的第四个桶、T011 的 `/health` 出口、T004 的三分告警是三道并列护栏，
  **一道都不能省**——省掉任何一道，现场就回到「怎么只有几个目录库在动」的
  issue #10 形状。
- `catalogue_project_priority` 与已落地的 `included_projects` 顺序兜底**都留着**，
  只是从主要机制降格为最后一道：MDB 声明过、却仍有多个项目拿得出同号文件时才轮到。
  T015 就是在证明这件事。
- 建议的提交边界（各自能独立编译、独立回滚）：
  1. T002 证据文档（纯文档）；
  2. T003–T005 `update_scope` 的名单与判定；
  3. T006–T009 两条清单选择路径过门 + 第四个桶；
  4. T010–T012 守护测试、`/health` 出口与文档。
- 若 T013 之后现场仍有 blocker，**先看它是不是 MDB 真声明过的库**再动屏障。
  屏障是对的；成员集才是要修的那个（T002 的第四条结论）。
