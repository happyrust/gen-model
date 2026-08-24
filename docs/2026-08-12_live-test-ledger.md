# live / ignored 用例台账

建账日期：2026-08-12（7-27 测试计划 Gate 3 的执行载体）
口径：全仓 `src/**` 与 `tests/**` 的 `#[ignore]` 用例逐项登记（2026-08-13 扩展：tests/
集成测试此前仅 C 组收录 issue7_e2e 一条、其余游离在外，已补录为待验行，见 E 组）。
**没有"最近通过"记录的用例视同未验资产**——本台账是唯一事实来源，动过 live 用例或
点亮新批次必须同步更新。

**2026-08-24 AMS 8000 空 RocksDB 初始化吞吐（最近通过）**：
在新的独立 RocksDB `.scratch/cata-throughput/concurrent-cache-8` 上以
`geometry_permits=8` 完整初始化，按字面完成标记停止计时，wall-clock 808.312s；
CATA 44 页/1702 个页内唯一身份合计 453.721s，p50/p95=9.722/20.591s，较串行
基线下降 73.1%/69.6%。最终表计数与串行基线完全一致，3555 个 mesh 路径/SHA-256
零差异，峰值工作集增长 19.9%。证据：
`docs/evidence/2026-08-24-cata-generation-throughput/concurrent-cache-8.md`。

**2026-08-24 AMS 8000 空 RocksDB 初始化计时（历史基线）**：
在独立 SurrealDB 2.1.4 `127.0.0.1:8169`、独立 RocksDB
`.scratch/init-timing-8000-sql-fixed/surreal-rocksdb` 上从 `tables={}` 开始。
DESI 8000 解析 6540 行约 2.7s，完整覆盖生成根 766 个；基线按 dbnum 分页得到 160 个
CATA 种子，一次页式闭包后于 15:52:47 推进水位到 233。模型按 16 根/页继续生成，
当前已确认前 80 根收口、`geom_error=0`；整轮尚未结束，不登记整体通过。证据为同目录
`empty-info.txt`、`init.stdout.log`、`init.stderr.log`。

**2026-08-24 AMS 8000 目录布尔缺失 mesh 收口**：
`data_interface::model_refresh::tests::live_generate_roots_with_coverage_audit` 最近通过两轮。
单根 `24384/22404` 为 40.76s；双根 `24384/22441,24384/22478` 为 48.41s，后者
CATA 36 个唯一项 43.55s、mesh/AABB/boolean 2.27s、总生成 46.09s。修复前同一单根
39.84s 后以 `root 24384_22404 catalogue negative boolean -> os error 2` 失败；修复后
缺失正实体 `.mesh` 进入 `geom_error(bool_pos)` 并标记 `bad_bool`，生成调用成功返回，
三条旧 `generation_root` 诊断已销账。证据：
`.scratch/model-stage-context/root-24384-22404.log`、
`.scratch/model-stage-context/root-24384-22404-after.log`、
`.scratch/model-stage-context/roots-24384-22441-22478-after.log`。

**2026-08-24 AMS 8000 页式 CATA 预加载（收敛中）**：
`production_acp7000_locator_opens_authoritative_paged_session` 最近通过，识别
`page_size_bytes=2048, sesno=272`；
`production_cata_locator_uses_paged_snapshot_below_io_budget` 最近通过，
ACP 7320 只读 38,858,752/431,941,632 字节，`record_pages=0`。AMS 8000 服务
PID 72348 已从 `model_update_pending=2228, inst_relate/inst_geo=0/0` 收敛到
`1220, 1293/1519`，CATA 页式闭包出现 `parsed=950/1908, missing=0`，无
`SessionPageData` panic；因工作单尚未清空，不登记整体初始化完成。证据：
`docs/evidence/2026-08-24-ams8000-paged-cata-preload.md`。

**2026-08-21 零尺寸 NCYL 死信恢复（执行中）**：已建立
`specs/022-model-dead-letter-recovery/`，完成定向圆柱类错误尺寸、模型工作状态快照、
死信公告去重与 health degraded 语义的离线实现。现场编排固定为
`scripts/Repair-ZeroNcylDeadLetter.ps1` 四阶段（Backup/Macro/Rebuild/Verify），回滚入口为
`scripts/Rollback-ZeroNcylDeadLetter.ps1`，证据目录为
`docs/evidence/2026-08-21-zero-ncyl-dead-letter-recovery/`。现场执行结果须在成对导出可导入、
7997 按 ADR-021 重建、目标死信单次复活、房间积压清零及 10 分钟 health 观察全部完成后
回填；在此之前不登记为最近通过。

**2026-08-19 AMS 8000 / sesno 239 staging 写回卡顿修复**：任务停在 `commit` 的根因
是 167 条 journal（118757 字节、预计 869 行）被旧的“仅按 500 条语句”策略包进单个
大事务，而 commit future 没有超时。现按 32 条 / 64 KiB / 250 行多维切块，块成功才刷新
实质进展，并为单次查询设置 120 秒停滞边界。executor 单测 **10/10 通过**，release 构建、
真实回滚和重新部署通过；现场最终水位 `239`、file latest `239`、staging=0，health 为
`model_ready`，Plant UI 与数据库均可见复制的 STRU 子树。旧大事务在停旧进程前已完成，
故未人为增加新的 E3D 保存来重复现场数据。证据：
`docs/evidence/2026-08-19-db8000-staging-writeback-stall-fix.md`。

**2026-08-19 AMS 8000 / sesno 236 成员删除纠正**：
`live_ams8000_ses236_membership_delete_matches_expected_net` **通过**（0.50s），
净窗口为 `Add 0 / Modified 1 / Deleted 1`，其中 `membership_deleted=1`。
停服运行 `db_window_repair --dbnum 8000 --from 236 --to 236 --expect-watermark 236`，
纠正报告为 `0/1/1`，水位 `236→236`，活动 staging=0；
`pe/STRU/ATT_UDA:24384_26201`、OWNER 与模型关系已清，ZONE
`24384_26199` 保留。重启 release 服务后 health 为 `model_ready`、worker 存活、
staging=0、空间树指纹一致。证据：
`docs/evidence/2026-08-19-db8000-membership-delete-repair.md`。

**2026-08-19 watch 8000 / CATA Required 闭包现场验证（非 `#[ignore]` 用例）**：在
`D:\work\plant-code\old\test-worklspace\bin` 以 `watch_dbnums=[8000]`、
`data/model=true`、`room=false` 从逻辑水位 33 重放 `34..=232`。任务
`db-20260819-105933-000000` **通过**：依赖 `parsed=404/missing=0`，状态 succeeded，
水位=232，活动 staging=0；提交期间 reconcile 保留活动 epoch。首轮 ReplaySafe 注入式
失败同时验证任务 failed、水位仍 33、窗口清零。证据：
`docs/evidence/2026-08-19-watch8000-cata-dependency-offline.md` 与测试目录
`.codex-deploy/watch8000-cata-dependency-live-20260819-102646/`。

**2026-08-19 增量阶段控制现场验证（非 `#[ignore]` 用例）**：在 `test-worklspace` 以
`watch_dbnums=[8000]`、`data=true/model=false/room=false` 启动 release 二进制；health 与
启动日志均确认三个最终值，8000 的 34..=232 只进入数据收集/暂存/写回链，模型与房间未消费。
写回仍停在既有 `staging_8000_1` 提交点，水位未推进，因此只记“阶段隔离通过”，不记数据
提交通过。证据：`docs/evidence/2026-08-19-increment-stage-data-only-live.md`。

**2026-08-19 ADR-037 完整窗口复验**：`db8000_session_pairs` 在严格冻结快照入口下
21/21 通过，`db_session_fixture_selfcheck` 15/15、`db8000_two_delete_fixture` 6/6、
`pdms_record_boundary` 3/3 通过；issue-019/020 固定窗口未出现假删除或非 MNUM 终稿降级。
本轮修改了 `increment_pipeline.rs` 中既有 ignored live 探针的 API 接线，但未对 8009 再做
写入型复跑：上一段记录的 `staging_8000_1` 仍是现场未收口前置，故不新增“数据提交通过”结论。
离线命令、vendor 门禁和该限制见 `docs/evidence/2026-08-19-dabacon-snapshot-completeness.md`。

**2026-08-18 已解析库旁新增一个库（启动检查两条入口）**：`data_interface::increment_manager::tests::live_startup_sweep_routes_a_new_db_to_baseline_beside_an_applied_one` 与 `..::live_scope_refresh_baselines_a_db_the_mdb_just_declared`（live 8019，跑法同 Gate 0，另设 `AIOS_MANUAL_UPDATE_PROJECT=AvevaMarineSample`、`RUST_MIN_STACK=16777216`；库号可配 `AIOS_STARTUP_APPLIED_DBNUM` 默认 8000 / `AIOS_STARTUP_NEW_DBNUM` 默认 7998）。**2026-08-18 四轮全通过**：默认 7998 靶 13.13s / 13.23s（命令总耗时各 25.4s），**现场口径 7999 靶**（`AIOS_STARTUP_NEW_DBNUM=7999`，56 MB / 120 会话）75.86s / 72.37s（命令总耗时 88.0s / 84.5s），窗口均为 `1..=120`。补的是 08-17 那条单库用例够不着的两件事：① **两条路由在同一份清单里不串味**——存量库 8000（`applied=file_latest=209` 且 pe 有支撑）与从未解析的 7998 同处一个一次性目录，重扫日志证实 8000 被评估过（`PathMigrated` 登记进临时目录、收尾又迁回）却一行都不排，只有 7998 走「发现从未解析过的文件」→ `新排：sesno 1..=12` → worker 基线 → 回执「首次按需初始化完成」，收尾断言 8000 水位一格未动；② **MDB 才是范围的定义**——库文件全程躺在目录里，把 7998 从 `/ALL` 的 `CURD` 摘掉后重扫报 `1 个库不在 MDB /ALL 的声明名单里（本期声明 28 个 DESI）：DESI:7998`、`DbnumState::read` 仍为 `None`（范围门排在 `record_observation` 之前，连观察值都不写），装回 CURD 后 `resweep_for_scope_change` 的 `[scope-refresh]` 重扫立刻 `新排：sesno 1..=12` 并走基线。MDB 夹具只动 `CURD`（`DBLS` 不碰），原样 CURD 存进 `queue_control:test_mdb_curd_backup` 再按原样写回；主体断言包在 `isolate_panic` 壳里保证无论红绿都先扶正 MDB，用例开头另无条件还原一次以自愈上一轮的中断。跑完实测沙箱完全还原：CURD 71 项且 7998/7999 各声明一次、备份行 0、7998 水位 12 与 7999 水位 120 且 pe 均有支撑、8000 停在 209 未动、三库登记路径均回到项目目录。跑法：

```powershell
$env:DB_OPTION_FILE = 'python/testbed/DbOption-pytest'
$env:AIOS_MANUAL_UPDATE_PROJECT = 'AvevaMarineSample'
$env:RUST_MIN_STACK = '16777216'
$env:AIOS_STARTUP_NEW_DBNUM = '7999'   # 省略则用秒级的 7998
cargo test --lib --no-default-features --features ws,gen_model,manifold,project_hd,http_api `
  data_interface::increment_manager::tests::live_startup_sweep_routes_a_new_db_to_baseline_beside_an_applied_one `
  -- --ignored --exact --nocapture
```

**2026-08-17 全新库自动基线（启动重扫入口）**：`data_interface::increment_manager::tests::live_startup_sweep_baselines_a_never_parsed_db`（live 8019，跑法同 Gate 0，另设 `AIOS_MANUAL_UPDATE_PROJECT=AvevaMarineSample`、`AIOS_MANUAL_UPDATE_DBNUM=7998`、`RUST_MIN_STACK=16777216`）。**2026-08-17 通过**（测试体 10.0s）。钉 ADR-023 §4 生产缺省形状：范围内从未解析的库（`delete_dbnum_fast` DropRow 清成无水位行/统计行/pe 行）→ 启动重扫「发现从未解析过的文件」→ 上弦后 queued 不挂起（窗口 1..=12）→ worker `needs_initial_load` 路由基线 → succeeded、回执「首次按需初始化完成」、水位=12、pe 有支撑；结尾 `PathMigrated` 自动迁移还原登记路径。夹具手法：watcher 指向只含 7998 副本的一次性目录（首轮红跑实测：全目录清单含沙箱 50+ 未解析库，多相位屏障切换需生产 worker 的相位重扫循环，`drain_queue_until_empty` 单独消化不了）。证据：`docs/evidence/2026-08-17-never-parsed-auto-baseline-live.md`。

**2026-08-14 AMS 1112 WALL RVM AABB**：根因是 `inst_relate` 把 `SpineArc` 局部包围盒当盒子做 8 角变换（64° 墙 X 跨度被撑到约 3 倍）。改为环扇取样后 `live_8009_refresh_cwall_rr001_wall_aabbs` 刷新 8009，Python `rvm_aabb_compare.py --fixture 1rs-wf03-w-c-rr001` **8/8 OK**（4 WALL + 4 STWALL）。

**2026-08-14 AMS 1112 WALL mesh 级对拍**：`rvm_baseline::mesh_compare::mesh_wall_live::mesh_wall_surface_distance`（live 8009 + occ，跑法 `cargo test --features rvm_verify --lib mesh_wall_surface_distance -- --ignored --nocapture`；`--features rvm_verify` 已含 occ）。**2026-08-14 通过**。实测（双向采样表面距离，单位 mm）：

| WALL | gen→rvm mean/p95/max | rvm→gen mean/p95/max | AABB |
|---|---|---|---|
| 1 | 3.0 / 7.3 / 8.3 | 4.1 / 7.3 / 774.6 | 吻合 |
| 2 | 3.2 / 7.3 / 52.3 | 27.8 / 229.4 / 649.6 | 吻合 |
| 3 | 3.5 / 8.1 / 51.7 | 20.0 / 8.8 / 649.7 | 吻合 |
| 4 | 20.0 / 170.9 / 292.2 | 44.5 / 304.8 / 592.7 | Y 差 ~115mm |

结论：(1) **gen 表面忠实**——WALL 1/2/3 的 gen→rvm p95 ≤ 8.1mm（仅弦误差量级），测试据此断言 `gen→rvm p95 ≤ 12mm` 作圆弧墙几何回归守卫。(2) **rvm→gen 约半墙厚（~650mm）的局部离群簇 = E3D 墙面开洞、gen 实心不开洞**。取证：4 堵 WALL 均 `has_cata_neg=false`、无负实体子（只有 SPINE + JLDATU），而 1112 里 5608 个元素靠 cata-neg 子（如 FLOOR 的 NXTR 子）正常切洞——**墙洞不是 SweepSolid 问题**，开口负实体不归墙所有，来源不在 gen 消费的已解析墙数据里（`plug_in/virtual_hole.rs` 是数据中心孔洞审批工作流，非几何切洞）。定位开口来源需 E3D 侧探针，属独立议题。(3) **WALL 4 = E3D 墙角斜接延伸，非 gen 缺陷（已证）**：径向范围与 E3D 吻合（rvm≈[16096,17400]、gen=[16100,17400]），排除厚度/半径。绕世界弧心角度跨度：rvm=[−108.31,−99.07]=9.24°，gen=[−106.90,−99.07]=7.83°——**同一末端、起点差 1.41°**。离线 `parse.element` 读 E3D 文件 SPINE 原始坐标：pt0(POINSP 105942)=(−5058.219,−16648.557)＝gen start_pt、thru(CURVE 105943)=(−3909.413,−16955.131) RADI=17400、pt1(POINSP 105944)=(−2742.352,−17182.535)，三点均在 R=17400、spine 弧 pt0→pt1=7.84°＝gen 7.83°。**gen 的墙与 PDMS spine 定义逐点吻合**；E3D 从 pt0 再延伸 1.41°（SPINE `DRNS=[1,0,0]` 驱动的墙角斜接）与 WALL 3（到 −107°）交接重叠。gen→rvm 在 WALL 4 偏大是因 gen 合法端面落在 E3D 延伸墙体内部（≈半墙厚），是 E3D 延伸的后果。WALL 2/3 的 ~52mm/0.18°、WALL 1 的 8mm 同源（延伸量随墙夹角，浅弧 WALL 4 最大）。**两处「gen 缺陷」查到底均为 E3D 侧附加几何（墙角斜接延伸 + 穿透开洞），gen 几何忠实**；是否实现 E3D 口径的墙角延伸/切洞属建模范围决策，非几何修复。

**2026-08-24 AMS 1112 WALL 4 弧段斜切收口**：上面的“非 gen 缺陷”裁决被 libgm
段构造语义推翻：SPINE 几何定义中心线范围，`DRNS/DRNE` 仍必须先延伸实体再按工作平面
裁切。vendor 对 Arc3D 保留源坐标法向，主仓回转路径按截面求所需扩角并用 Manifold 裁回；
`live_8009_regenerate_cwall_rr001_wall_meshes` 强制重铺单位网格后，WALL 4 扫角
`7.83° → 9.24°`，gen→rvm `mean/p95/max = 1.40/4.05/26.55mm`，rvm→gen
`9.37/4.28/648.40mm`（最大值仍是既有墙洞范围差）。四堵 WALL 现全部受
`gen→rvm p95 ≤ 12mm` 断言保护。证据：
`docs/evidence/2026-08-24-arc-wall-mitre-rvm.md`。

**2026-08-14 AMS 8000 C-OR 管系 mesh 级对拍**：`rvm_baseline::mesh_compare::mesh_wall_live::mesh_pipe_surface_distance`（live 8009 + occ，跑法 `cargo test --features rvm_verify --lib mesh_pipe_surface_distance -- --ignored --nocapture`）。**2026-08-14 通过**（取证型，不硬断言）。gen 侧 FTUB 走 param 就地重建、BEND 走磁盘 `.mesh`（复合/布尔结果 `param=NONE`，`gen_world_mesh` 已加 param→.mesh 回退）。实测（双向表面距离 mm）：

| 构件 | gen→rvm mean/p95/max | rvm→gen mean/p95/max | 判读 |
|---|---|---|---|
| FTUBE 1..7 | ~0.55 / 1.5 / 1.5 | ~0.5 / 1.5 / 1.5 | 直管**近乎完美** |
| BEND 1 | 47 / 95 / **100** | 1.7 / 7.5 / 11 | gen 多面 |
| BEND 2 | 25 / 90 / **103** | 4.2 / 18 / 24 | gen 多面 |

结论：与墙相反——**BEND 是真 gen/E3D 逐元素几何差异**。`rvm→gen` 小（E3D BEND 全贴在 gen 上），`gen→rvm` 大（gen 弯头 3 子几何、1476/2220 三角，多出约 100mm）。只读根因取证：E3D BEND 1 世界 AABB=**51×54×30mm**（FacetGroup 6 面 24 顶点，z 2900–2930＝管径 ~30mm，即弯头本体）；gen 弯头单位几何 x±100、z 0..100（世界 z 2900–3000，比管径高出 70mm），world_trans 无缩放、平移与 E3D 一致。**gen 弯头按「arrive→leave」整段生成、含两端切向直管腿（各约 100mm），伸进相邻 FTUB 区**；E3D 的 RVM BEND 只是弯头本体、直段归相邻 FTUB。worst gen→rvm 点落在 FTUB 侧＝重叠的腿。**装配 union 验证（`mesh_branch_union_surface_distance`，2026-08-14 通过）判定为装配无害、非缺陷**：BEND 1 + 相邻 FTUBE 1/2 合并成 union 后，gen union vs E3D union 双向 mean=0.67 / p95=1.50 / **hausdorff=5.80mm**（gen→rvm 从逐元素 100mm 掉到 5.8mm）。gen 弯头腿伸进的相邻直管区正好被 E3D 的 FTUB 盖住，合起来几何一致——所谓「多算 100mm」只是 gen（弯头含腿）与 RVM（腿归直管）**元素边界拆分口径不同**，最终装配一致，无需改 aios-core。

**2026-08-14 C-OR 整条 BRANCH 端到端 union 对拍**：`rvm_baseline::mesh_compare::mesh_wall_live::mesh_full_branch_union_surface_distance`（live 8009 + occ，跑法 `cargo test --features rvm_verify --lib mesh_full_branch -- --ignored --nocapture`）。**2026-08-14 通过**（带断言 `p95≤10 / max≤30mm`）。整条 C-OR BRANCH 9 构件（FTUBE 1–7 + BEND 1–2）合成 union：gen vs E3D 双向 **mean=0.69 / p95=1.50 / hausdorff=18.67mm**（gen→rvm max=11.7 在 BEND 2、rvm→gen max=18.67，均 tessellation 量级）。逐元素的弯头腿归属差在整条 union 里自洽抵消——**整条管路 gen 几何在装配层与 E3D 逐点吻合到 ~1.5mm(95%)**，端到端验证 gen 正确。

**2026-08-14 AMS 1112 STWALL mesh 级对拍**：`mesh_stwall_surface_distance`（live 8009 + occ）。**2026-08-14 通过**（双向 p95≤12mm）。4 堵直线 STWALL 均为 12 三角盒、无内环：双向 mean/p95=0、max≤0.06mm。直线 SweepSolid 与 E3D 逐点重合。

**2026-08-24 AMS 1112 STWALL 无 OCC 复验**：libgm `setSpineSegmentTransforms` 与现场
`STWALL 4` 共同确认直接 `POSS/POSE` 扫掠的路径方向已在元素 `world_trans` 中；修正实例
重复旋转并定向重生成 CWALL 后，`mesh_stwall_surface_distance` 以
`--no-default-features --features ws,gen_model,manifold,project_hd,rvm_verify` **通过**。
4/4 均为双向 mean/p95=0，max 分别 0.03/0.06/0/0mm；STWALL 4 的生成 AABB
`[-1300,-17201.37,-20]..[1300,-17001.37,230]` 与 RVM 重合。证据：
`docs/evidence/2026-08-24-stwall-direct-transform-rvm.md`。这条记录取代上面的 OCC 参照口径。

**2026-08-14 AMS 8000 C-IY 槽盒 BRANCH union 对拍**：`mesh_c_iy_full_branch_union_surface_distance`（live 8009 + occ，`test_data/rvm/C-IY-1R330-B.rvm`）。**2026-08-14 通过**（守卫 `gen→rvm p95≤10 / max≤30mm`）。18 FTUB + 18 BEND 中 FTUBE 6 为零长隐含直管（HEIG=0、RVM `geometries=[]`、无 `inst_relate`），两侧无表面，跳过；其余 35 构件 union：gen→rvm **mean=0.85 / p95=4.14 / max=24.93mm**（gen 贴在 E3D 里），rvm→gen mean=21.3 / p95=100 / max=111.6mm。根因：目录 `LSTU=/ACP1000-Trough/ACP1000-TUBE:100`，E3D RVM 含约 150mm 高槽体外壳（FTUBE 1 aabb z=430–580），gen 管段 z=430–480（50mm）；worst rvm→gen 全在 z=580 槽顶。与 C-OR 圆管（insu off、双向 ~1.5mm）不同，是槽盒表示范围差，不是 gen 画错。

**2026-08-14 AMS 1112 GWALL 挤出 union 对拍**：`mesh_gwall_union_surface_distance`（live 8009 + occ，同一份 `1RS-WF03-W-C-RR001.rvm`）。**2026-08-14 通过**（盒状 ≤16 三角的 gen→rvm p95≤1mm）。20/20 两侧都有网格。11 堵盒状 GWALL 贴合（p95=0）。高面片 E3D 墙（GWALL 3/4/15/18/19，最多 908 三角）rvm→gen p95=180–378、max=450–650mm，与 WALL 开洞同量级。1:1 AABB 中心配对不可用（同簇多墙），故走 union。

**2026-08-14 AMS 1112 GWALL 大体量 gen 余量根因**：`mesh_gwall_extra_against_cwall_union`（live 8009 + occ）。**2026-08-14 通过**（NXTR 计数 + 布尔后距离守卫）。生产 `query_valid_insts` 用 `booled_id` 网格，对拍原先只重建正挤出，且 `{refno}_{sesno}.mesh` 未落盘。对齐 `booled_id` 并补跑 `gen_inst_meshes`+`apply_insts_boolean_manifold` 后：`105828` gen→gwall p95=0.1/max=77.6，`105880` p95=9.3/max=105，`116569` p95=137/max=152（未布尔时为 870/786/591）。守卫：前两堵 p95≤12，116569 回归 ≤180。

**2026-08-24 AMS 1112 GWALL 无 OCC 重生成复验**：新增
`live_8009_regenerate_extreme_fillet_gwall_and_boolean`，对 `105828/105880/116569`
以及日志中已标 `bad_bool` 的 `105691/116713` 和全部 NXTR 强制重铺，并以
`GeometryFailurePolicy::Required` 执行目录/设计两层 Manifold 布尔，**1/1 通过**；
五行最终均为 `booled=true, bad_bool=false`。随后
`mesh_gwall_extra_against_cwall_union` **1/1 通过**：
NXTR=`4/5/8`，gen→GWALL p95=`0.1/9.3/167.5mm`；
`mesh_gwall_union_surface_distance` **1/1 通过**，20/20 两侧齐全，gen→RVM
mean/p95=`3.86/8.06mm`，盒状硬门未放宽。证据：
`docs/evidence/2026-08-24-gwall-nonzero-manifold-boolean.md`。此记录取代上述 `+occ`
构建口径，但保留其历史数据用于回归比较。

同日把 `.surreal/ams-7997-e3d-test-20260805` 的独立副本挂到 8039，以独立 mesh
目录再次运行同一 Required 用例；历史 `bad_bool` 的 `FLOOR 17496/230353` 也成功
复活为 `booled=true, bad_bool=false`。副本最初 11 个坏布尔中余下 10 个均属于非目标
dbnum 7324 的 SJOI。测试后 8039 已停止。证据同上。

**2026-08-24 OCC 退役双副本 RVM 推进**：RVM live 测试增加
`AIOS_RVM_DB_ENDPOINT` / `AIOS_RVM_MESH_DIR` 隔离覆盖。7997 副本从生成根
`17496/105799` 重建后，`mesh_gwall_union_surface_distance` 与
`mesh_gwall_extra_against_cwall_union` 均通过：20/20 双侧齐全，union gen→RVM
p95=`4.14mm`，三件大体量 GWALL p95=`0.1/9.3/167.5mm`。默认 8009 同候选复验
WALL/STWALL 两项均通过（WALL p95 最大 8.63mm，STWALL 双向 p95=0）。7997 副本
不含 8 条历史测试专用 WALL/STWALL 生产关系，首轮准确失败于“目标库没有生成几何”；
随后用 `mesh_wall_and_stwall_from_source_attributes` 从两个副本各自的 PE/CATA/SPINE
源属性走生产解析器，两边均通过（WALL p95=`7.86/7.83/8.62/4.05mm`，STWALL 双向
p95 全 0）。`full_annulus_matches_two_halves_joined` 同批通过，关闭 360° SANN 1% 体积门。
证据：`docs/evidence/2026-08-24-occ-retire-dual-copy-rvm.md`。

**mesh 批次收编**：mesh 用例收进 `scripts/live-batches/mesh-verify-8009.json`（只读 8009，config=`DbOption`、features=`rvm_verify`）。2026-08-14 用 `cargo test --features rvm_verify --lib surface_distance -- --ignored --test-threads=1` 一批跑过 **4/4**（33.6s）。同日补上四份 e2e 探针的 `DiscoveredBatch.{phase,epoch_id}` 后，`cargo build --lib --tests --features rvm_verify` 已过；标准 runner `powershell -File scripts\Run-LiveBatch.ps1 -Manifest scripts\live-batches\mesh-verify-8009.json` **4/4 pass**（50.5s；报告 `output/live-batch/20260814-210048/report.json`）。扩 STWALL + C-IY 后 runner `-Only mesh_stwall_surface_distance,mesh_c_iy_full_branch` **2/2 pass**（56.9s；报告 `output/live-batch/20260814-212144/report.json`）。

**2026-08-14 模型实例保存合批专项**：`fast_model::shape_save::tests` 6 项与
`fast_model::pdms_inst::tests` 15 项通过（其中 staged mem 覆盖有序 journal、两次重放
幂等与失败停止后续 packet）；`test-worklspace` 对同一 16 个 BRAN 完成旧/新二进制 A/B，
150 行 `inst_relate` canonical SHA-256 均为
`c775a8dc5daa201e5ec219911740a39370f1a86f07e9a4e9597e5c59442c4d37`，候选
41,036/41,722 ms 均落在旧版 40,827～42,054 ms 区间。固定 16 根/16 小批性能夹具 save 与非删除 SQL packet
均下降 93.75%。证据：`docs/evidence/2026-08-14-shape-save-coalescing.md`。

跑法（Gate 0 能力，rs-core `DB_OPTION_FILE` 已落地）：

```powershell
# 单个：
$env:DB_OPTION_FILE = 'python/testbed/DbOption-pytest'   # 或其它 db_options/ 配置
cargo test --lib --features http_api <测试名> -- --ignored --exact --nocapture
# 批量：scripts/Run-LiveBatch.ps1 -Manifest scripts/live-batches/<批次>.json
```

**批次 1 战果（2026-08-12）**：A 组 26 项全部有了结论——**23 项首次取得可复现
通过记录**（12 项 @ testbed 8019、11 项 @ 一次性空库 8071），3 项阻塞已定性
（积压前置 / 数据依赖 / 断言写死生产语义，见各行）。过程中修复三处测试腐化
（白名单前的夹具命名、状态机前的发布门缺声明 ×2），并确认 room_fixture 系
需要专用空库清单。报告与逐项日志在 `output/live-batch/`。

**批次 2 战果（2026-08-12 夜 ～ 08-13 晨，B0/B1/B2 @ testbed 8019）**：

- **B0 环境建设**：`live_manual_baseline_all_design_dbnums` 通过（4 库全量基线）；
  一次性出清 regen 积压 **30,416 根 / 321 分钟**，稳态残留 48 行全是真实几何缺陷
  （见下）；冷备 `python/testbed/.surreal/pytest-ams.bak-b0-20260813`（812MB）；
  批次 1 阻塞的 `live_generation_failure_keeps_pending_and_watermark` 出清后点亮。
- **B 组 39 项：19 项首次通过**，16 项定性（CATA 前置 6 / 8009 数据绑定 2 /
  店态与夹具期望漂移 5 / 清理路径待查 1 / 溢出缺陷 1 / 长跑专项 1），4 项未跑
  （room mesh / 空 8009 前置，见各行）。批次 1 记阻塞的
  `resolves_the_real_mdb_declaration` 在 SYS meta 解析后连 29 库精确计数一起通过。
- **测试腐化修复（第 4 处）**：`model_update_pending.rs` 的 4 个 drain 型 live 用例
  写于空间状态机之前，房间副作用撞就绪门（`SPATIAL_TREE_NOT_READY`）——照
  `live_incomplete_room_panels` 的先例补 `rebuild_tree_from_pointers()` 前置
  （共享 helper + zone/spco-cascade 用例体），`zone_owned_equi` 随即点亮。
- **testbed 店准备动作**（复跑他批前须在位，均分钟级）：SYS meta 引导
  （`aios_db.incr.execute_manual` 第一遍，需 `RUST_MIN_STACK=16777216`）、
  空间树重建落盘（`aios_db.spatial.rebuild()` + `persist(force=True)`，43k 条 ready）、
  清除 48 行已立档 failed 残留。
- **新发现缺陷 3 个**（均真实数据暴露，非队列机制）：
  1. **NCYL/NREV/NRTO/RTOR/PYRA/CTOR 负几何/回转体 BREP 生成缺陷**：48 根顽固
     失败（45 invalid / 3 no shape），清单在出清日志 `.scratch/b0-drain-console2.log`
     与首轮报告；复现根样例 `24381/36946`（NREV no shape）、`24381/116383`（NCYL invalid）。
  2. **`live_backfill_anc_on_configured_db` i64 打包溢出**：批次 1 魔术 refno 残留
     （ref0=4000000001）在回填 SQL `ref0 * 2^32` 处溢出——生产 ref0 不会到 2^31，
     但回填工具对超界行应跳过并告警而非中断。
  3. **SYS/DICT 解析链默认栈溢出**：`execute_manual` 首遍解析 SYST 8191（169 会话）
     默认线程栈下 0xC00000FD；`RUST_MIN_STACK=16MB` 稳定通过，解析深递归待收敛。
- **`live_manual_update_project` 语义确认**：第二遍会把 /ALL 里 25 个从未导入的库
  拉**首建基线**（观测 1800s 超时中断后留 5,027 pending，靠冷备回滚）——testbed
  常规批不点亮，归专项窗口；跑它前必须先冷备。

**批次 2 补测（2026-08-13 上午，CATA 闭包修复后）**：库侧闭包收集器的
`refno.*[WHERE …]` 对记录链接形状静默返回空（真 schema 里 `refno` 是指向类型化
属性表的记录链接），修为 `object::values` 展开后按需 CATA 端到端打通——
**再点亮 12 项**（B 组 19→31/39）：

- **CATA 家族 7 项**：`bran`/`hang` pending 重生成（SPRE→23274 规格行按需入店）、
  `issue5` ×2（隐含直管段判定拿到规格数据后走整根重生成）、`scom` ×2（先
  `aios_db.model.ensure('24384/22456')` 让 ams5052 目录 1,727 行按需入店；合跑
  一进程时有跨测试缓存干扰，批跑工具本就单测试单进程，无碍）、
  `transform_branch`（DAMP 模型随 bran 重生成落位，本质是 CATA 下游非漂移）。
- **共享 SPCO 三件套**：钉死常量拆层（72/67/68 → 结构断言 + `AIOS_EXPECT_SPCO_*`
  env 钉；本店切面消费者=75），expands/cascade 自足重建 ref_rev 不再依赖兄弟用例
  顺序。两个并行实例同时跑三件套互扰下仍全绿。
- **`backfill_anc`**：回填改为跳过 ref0 超出 u64 打包上限的行并告警（testbed 实测
  7 行批次 1 魔术残留，一行即可炸整批 UPDATE 事务），复核口径同步收窄到可打包
  范围。溢出缺陷就此闭环。
- **`cleanup_by_pe_state`**：第 5 处测试腐化——期望向量要求删 `inst_geo`，与
  `render_cascade_delete` 的设计（内容寻址共享节点不做写路径单边回收，防跨根
  数据损坏）矛盾；期望修正为 inst_geo 幸存、归后台 sweep。
- **数据绑定定性收口**：`projams_real_attribute` / `projams_nested_created` 与
  `suppo` 同族——期望钉在一份**本机不存在的 ams8000 世代**上（testbed 与
  D:\AVEVA 两份文件会话链完整 1–209，sesno 21 均无 GENSEC Add，25725/25743
  家族元素在文件与解析到位的店里都不存在）。需持有那份历史文件或在 E3D 重录
  后重钉期望（`AIOS_PROJAMS_GEOMETRY_FILE`/`AIOS_PROJAMS_DATA_ONLY_FILE`
  可指向任意副本）。

类别口径：

- **A 自建夹具**：数据自造自清（fixture 记录 / 魔术大 dbnum / 一次性目录），只要
  配置的 Surreal 可达 + schema 在位。可在 testbed 沙箱（8019）反复跑。
- **B 需生成基线**：依赖已解析/已生成的 AMS 数据（inst_info、共享 inst、特定构件
  在位）。testbed 跑过全量基线+生成后可另立批次。
- **C 需真实 E3D**：依赖真实 E3D 会话历史、宏驱动或真实项目库写入。归生产空窗
  runbook。
- **D 专用夹具 / bench / 探针**：特定数据集（7324、ACP 7320、fold 文件）、吞吐
  基准或 ad-hoc 探针，按需手跑，不进常规批次。

## A 自建夹具（批次 1，2026-08-12 执行）

跑出来的一条硬结论：**room_fixture 系必须跑一次性空库实例**（专用清单
`scripts/live-batches/room-fixture-8071.json`，config `python/tests/DbOption-roomlive`）。
它们刻意只灌夹具那几条盒子进树，而房间全量重建的覆盖率闸门拿「库内可用指针数」
当分母——在带真实基线的 8019 上（1.7 万条指针）必撞闸门，9/11 红即此因，非回归。
空库上夹具行自然对得上分母，11/11 全绿且闸门语义原样保留。其余 A 组成员留在
`batch1-selfcontained.json`（@ testbed 8019）。

| 测试 | 位置 | 前置 | 最近通过 | 结论 |
|---|---|---|---|---|
| `live_room_fixture_probe` | room_fixture.rs:352 | 一次性空库 @8071（先跑 parity） | 2026-08-12 @8071 | **通过**（1s） |
| `live_room_structural_triggers_enqueue_panel_recalc` | room_fixture.rs:440 | 一次性空库 @8071 | 2026-08-12 @8071 | **通过**（1.2s） |
| `live_room_rename_into_compliance_recomputes_membership` | room_fixture.rs:580 | 一次性空库 @8071 | 2026-08-12 @8071 | **通过**（2s） |
| `live_room_fixture_parity` | room_fixture.rs:911 | 一次性空库 @8071 | 2026-08-12 @8071 | **通过**（1.2s） |
| `live_room_panel_move_parity` | room_fixture.rs:1036 | 一次性空库 @8071 | 2026-08-12 @8071 | **通过**（1.6s） |
| `live_room_panel_task_absorbs_element_task_in_the_same_round` | room_fixture.rs:1133 | 一次性空库 @8071 | 2026-08-12 @8071 | **通过**（1.6s） |
| `live_room_cross_panel_move_defeats_absorption` | room_fixture.rs:1205 | 一次性空库 @8071 | 2026-08-12 @8071 | **通过**（1.8s） |
| `live_room_delete_clears_membership` | room_fixture.rs:1295 | 一次性空库 @8071 | 2026-08-12 @8071 | **通过**（1.4s） |
| `live_room_incremental_parity` | room_fixture.rs:1378 | 一次性空库 @8071 | 2026-08-12 @8071 | **通过**（1.6s） |
| `live_room_deleted_edges_come_back_after_a_move` | room_fixture.rs:1491 | 一次性空库 @8071 | 2026-08-12 @8071 | **通过**（1.8s） |
| `live_room_tubi_row_enters_tree_and_tracks_regen` | room_fixture.rs:1663 | 一次性空库 @8071 | **2026-08-20 @8071 复验通过**（脚本 2.4s；测试本体 1.07s）：BRAN 重生成后的隐含 TUBI 进入空间树并参加房间计算，成员边 6→7，日志明确记录 `4000000001_30` 从无房间进入 `K100`；证据 `docs/evidence/2026-08-20-bran-room-tubi-live.md` |
| `live_record_scan_never_moves_the_applied_watermark` | dbnum_state.rs:1398 | 魔术 dbnum | 2026-08-12 批次1 @8019 | **通过**（3.5s） |
| `live_blocked_observation_keeps_the_verdict_evidence_intact` | dbnum_state.rs:1500 | 魔术 dbnum | 2026-08-12 批次1 @8019 | **通过**（3.4s） |
| `live_finalize_is_crash_safe_and_idempotent` | model_update_pending.rs:4326 | 魔术 dbnum | 2026-08-12 批次1 @8019 | **通过**（3.4s） |
| `live_os_kill_preserves_prepared_attempt` | model_update_pending.rs:4410 | 魔术 dbnum + 杀助手进程 | 2026-08-12 批次1 @8019 | **通过**（5.8s） |
| `live_non_regen_drain_consumes_the_whole_queue` | model_update_pending.rs:4525 | 魔术 dbnum | 2026-08-12 批次1 @8019 | **通过**（11s） |
| `live_failed_queue_cleanup_does_not_stall_the_rest` | model_update_pending.rs:4590 | 魔术 dbnum | 2026-08-12 批次1 @8019 | **通过**（4.1s） |
| `live_generation_failure_keeps_pending_and_watermark` | model_update_pending.rs:4661 | 魔术 dbnum；**前置：目标库 regen 积压已出清**（drain 会先消化整个存量队列） | 2026-08-13 B0 @8019 | **通过**（145.2s，B0 出清后；先消化了 48 个顽固重试再跑自身场景） |
| `live_incomplete_room_panels_enqueue_targeted_repairs` | model_update_pending.rs:5351 | **数据依赖：库里须有缺陷面板**（探针型，改归 B 组口径） | 2026-08-19 @8019 | **前置阻断复现**：当前 testbed 缺陷面板计数为 0，目标修复队列未入行；日志 `output/live-batch/remaining-room-and-panel-20260819-083240/01-live_incomplete_room_panels_enqueue_targeted_repairs.log` |
| `live_finalize_capacity_is_atomic_and_idempotent` | model_update_pending.rs:5038 | 5k+5k 容量验证 | 2026-08-12 批次1 @8019 | **通过**（12.2s） |
| `resolves_the_real_mdb_declaration` | update_scope.rs:358 | SYS meta 已解析（`execute_manual` 引导一遍）；精确计数走 `AIOS_EXPECT_DESI_COUNT` | 2026-08-13 B1重测 @8019 | **通过**（3.2s；断言已拆结构层+计数层，testbed /ALL 同样解出 29 个 DESI，`AIOS_EXPECT_DESI_COUNT=29` 全绿） |
| `an_unparsed_project_bootstraps_instead_of_deadlocking` | update_scope.rs:387 | 空 NS | 2026-08-12 批次1 @8019 | **通过**（3.2s） |
| `live_watch_directory_blocks_duplicate_dbnum_files` | increment_manager.rs | E3D 文件头 + 一次性副本目录 | 2026-08-14 @8019 | **再确认通过**（0.01s）。ADR-028 后 `ams9990_0001/_0002` 仍 Duplicate。2026-08-12 批次1 首次点亮。 |
| `live_watch_directory_collapses_master_and_extract` | increment_manager.rs | E3D 文件头拷成 `ams{dbnum}` + `ams{dbnum}_0001` | 2026-08-14 @8019 | **通过**（0.01s）。主库+唯一叶子 duplicate 集为空，collapse 选叶子。 |
| `live_extract_tree_ams7355_refno_sets` | extract_family.rs | 本机 AMS `ams7355` / `ams7355_0001` 实文件 | 2026-08-14 | **通过**（0.57s）。父层 102716 refno / 叶子 135278 / **parent_only=0**（叶子 ⊇ 父层）。基线只解析叶子，父层留作按需 miss。 |
| `test_ams7355_headers_same_dbnum_leaf_is_later` / `test_ams7355_collapse_selects_leaf_and_parent_gap_is_zero` | python/tests/test_extract_tree_offline.py | 本机 AMS 实文件；缺文件 skip | 2026-08-14 | **通过**（`pytest -m offline` 83 passed / 5.40s；抽取树档含 AMS 头 sesno 13 vs 15、collapse 选叶子、parent_gap=0）。 |
| `preview_manual_update` dbnum=7355（Python 连接层 @8019） | python `aios_db.db.preview_manual_update` | testbed 项目副本含 `ams7355`+`ams7355_0001`；只刷新扫描观察，不推进 `applied_sesno` | 2026-08-15 @8019 | **通过**（~10s）。整目录 collapse：445 候选 / 444 选中 / Duplicate 空 / 仅 `ams7355` 进 shadowed。预览行：`file_name=ams7355_0001`、`file_latest_sesno=15`、`blocked=false`、主库不再单独占一行。首次预览曾报 `path_migrated`（旧身份主库→叶子，不阻断），观察刷新后 anomaly 清空。本期 3 条 Duplicate 是跨项目 `acp7009/7011/7015` vs `zdj*`，与抽取树无关。 |
| `live_rollback_wipe_clears_the_dbnum_for_reinit` | manual_update.rs（tests） | 魔术 dbnum + 保留段 ref0（空库即可） | 2026-08-13 @8019 | **通过**（4.7s）。前身 `live_watermark_realign_rebaselines_a_rolled_back_dbnum`（2026-08-12 随档位新增，08-13 同参数通过 4.7s）随 ADR-021 重写：缝合式对齐（prune + 补洞）改为整库清空 `wipe_dbnum_for_reinit`，断言改为全删（幸存行也不留）+ 统计清空 + 水位清值不删行 + spatial epoch 递增 |
| `live_rollback_and_ghost_watermark_reinit_end_to_end` | manual_update.rs（live_tests） | testbed 沙箱 + `AIOS_MANUAL_UPDATE_PROJECT`；靶库默认 7998（**会物理清空重建**）；debug 构建需 `RUST_MIN_STACK=16777216` | 2026-08-14 @8019 | **通过**（33.92s，三幕）。幕一回退：入队不删数据 → worker 复核 → 整库清空 → 首次导入基线 → 水位对齐文件；幕二幽灵水位（file_latest>applied>0 且 pe 零行）：路由到基线而非增量；幕三追平幽灵水位（file_latest=applied>0 且 pe 零行、空基线凭据为空）：人工入队形成首次导入并恢复 PE/水位。首跑曾抓出增量窗口开在基线路由之前的缺陷，现由 `batch_reroutes_to_initial_load` 在冻结点开窗前预判 |
| `live_startup_sweep_repairs_a_caught_up_ghost_watermark` | increment_manager.rs（tests） | testbed 沙箱 + `AIOS_MANUAL_UPDATE_PROJECT`；靶库默认 7998（**会删除 PE 后重建**）；debug 构建需 `RUST_MIN_STACK=16777216` | 2026-08-14 @8019 | **通过**（19.26s，测试体；命令总耗时 31.7s）。真实启动重扫检出 `file_latest=applied=12`、PE 零行且无匹配空基线凭据，排成 held 的 `apply_window` 首次导入窗口；同 dbnum 人工触发放行后 worker 复核、清理并建基线，任务 Succeeded，PE 恢复且应用水位回到 12 |
| `live_startup_sweep_baselines_a_never_parsed_db` | increment_manager.rs（tests） | testbed 沙箱 + `AIOS_MANUAL_UPDATE_PROJECT`；靶库 `AIOS_MANUAL_UPDATE_DBNUM` 默认 7998（**会清成从未解析后重建**）；debug 构建需 `RUST_MIN_STACK=16777216` | 2026-08-17 @8019 | **通过**（10.0s，测试体）。详见本页顶部 2026-08-17 段与 `docs/evidence/2026-08-17-never-parsed-auto-baseline-live.md` |
| `live_startup_sweep_routes_a_new_db_to_baseline_beside_an_applied_one` | increment_manager.rs（tests） | testbed 沙箱 + `AIOS_MANUAL_UPDATE_PROJECT`；`AIOS_STARTUP_APPLIED_DBNUM` 默认 8000（**前提：已追平且 pe 有支撑**，只读不写）、`AIOS_STARTUP_NEW_DBNUM` 默认 7998（**会清成从未解析后重建**）；debug 构建需 `RUST_MIN_STACK=16777216` | 2026-08-18 @8019 | **通过**（13.13s，测试体；命令总耗时 25.4s）。存量库与新库同处一个一次性目录，一轮重扫里存量库零入队、新库 `apply_window 1..=12` → 基线；收尾断言存量库水位未动。真靶口径 `-NewDbnum 7999`（56 MB / 120 会话，基线以分钟计） |
| `live_scope_refresh_baselines_a_db_the_mdb_just_declared` | increment_manager.rs（tests） | 同上，外加**会临时改写沙箱库里 `/ALL` 的 `CURD`**（原样备份进 `queue_control:test_mdb_curd_backup`，无论红绿都还原） | 2026-08-18 @8019 | **通过**（13.23s，测试体；命令总耗时 25.4s）。摘出 CURD → 重扫零入队且不写观察值 → 装回 CURD → `scope-refresh` 重扫发现并走基线。跑后实测 CURD 71 项、备份行 0 |
| `live_direct_delete_crash_before_persist_recovers_by_rebuild` | helper.rs:908 | testbed 指定（推进 epoch、重建树文件） | 2026-08-12 批次1 @8019 | **修复后通过**（5.5s）——用例写于状态机之前，自灌树后 persist 被发布门拒（Uninitialized），补 `mark_spatial_tree_fixture_preloaded()` |
| `live_direct_refresh_crash_before_persist_recovers_by_rebuild` | occ_generate.rs:2057 | testbed 指定（需基线+生成在位） | 2026-08-12 批次1 @8019 | **修复后通过**（5.8s）——同上，补测试装载模式声明 |
| `live_sync_aabb_tree_fills_tree_from_db` | aabb_tree.rs:2072 | 重写 inst_relate.aabb + 树文件（走 AIOS_LIVE_WS 三件套） | 2026-08-12 批次1 @8019 | **通过**（1.2s，工具补齐 AIOS_LIVE_* 派生后） |

## B 需生成基线（批次 2，2026-08-12/13 执行 @ testbed 8019）

清单：`batch2-b0.json`（环境建设）、`batch2-b1-baseline.json`（基线即可批）、
`batch2-b2-generated.json`（生成产物批）、`batch2-retest2.json` /
`batch2-retest-cata.json` / `batch2-retest-spco.json`（缺口修复后的定向重测）。
首日 19/39，CATA 闭包修复补测后 **31/39 通过**；余 8 项=数据绑定 3（消失的
ams8000 世代）+ 长跑专项 1（manual_update 二遍）+ 未跑 4（room mesh / 空 8009
前置）。

| 测试 | 位置 | 前置 | 最近通过 | 结论 |
|---|---|---|---|---|
| `live_deleted_branch_subtree_includes_known_damp_child` | helper.rs:641 | AMS 7997 已知 DAMP 子树 | 2026-08-13 B2 @8019 | **通过**（3.1s） |
| `live_shared_inst_info_is_deleted_only_after_last_reference` | helper.rs:656 | 共享 inst_info 在位 | 2026-08-13 B2 @8019 | **通过**（3.2s） |
| `live_inst_info_without_geo_relate_is_reclaimed` | helper.rs:759 | 生成产物在位 | 2026-08-13 B2 @8019 | **通过**（3.2s） |
| `live_soft_deleted_subtree_removes_all_model_nodes` | helper.rs:815 | 生成产物在位 | 2026-08-13 B2 @8019 | **通过**（3.3s） |
| `live_transform_branch_includes_known_model_child` | increment_manager.rs:2415 | BRAN `24381/100817` 已生成（CATA 闭包在位） | 2026-08-13 补测 @8019 | **通过**（2.9s；此前失败是 CATA 下游——DAMP 无模型，闭包修复 + bran 重生成后落位） |
| `live_manual_baseline_all_design_dbnums` | manual_update.rs:6826 | 全量基线工具本体 | 2026-08-12 B0 @8019 | **通过**（4 库基线，小时级；`AIOS_MANUAL_UPDATE_PROJECT=AvevaMarineSample`） |
| `live_manual_update_project` | manual_update.rs:6871 | 基线在位 + SYS meta；**先冷备** | — | 长跑专项：二遍会把 /ALL 未导入 25 库拉首建基线（1800s 超时中断留 5k pending，冷备回滚）。一遍（SYS meta 引导）已验证可用，需 `RUST_MIN_STACK=16M` |
| `live_ref_rev_roundtrip_selfcheck` | manual_update.rs:6935 | ref_rev 数据在位 | 2026-08-13 B1 @8019 | **通过**（3.1s） |
| `live_rebuild_ref_rev_covers_shared_spco_consumers` | manual_update.rs:6991 | 共享 SPCO 数据（23274 规格行已按需入店）；精确数走 `AIOS_EXPECT_SPCO_CONSUMERS` | 2026-08-13 补测 @8019 | **通过**（钉死 72 拆层为结构断言+env 钉；本店切面=75） |
| `live_shared_spco_expands_to_generation_roots` | manual_update.rs:7036 | 同上；自足重建 ref_rev | 2026-08-13 补测 @8019 | **通过**（不再依赖兄弟用例顺序；归并断言改「非零且 ≤ 消费者数」+ `AIOS_EXPECT_SPCO_ROOTS` env 钉） |
| `force_init_watcher_incr_once` | increment_pipeline.rs:3330 | 基线 + 监控目录 | 2026-08-13 B2 @8019 | **通过**（4.3s） |
| `live_add_pe_owner_replay_is_idempotent` | increment_pipeline.rs:3351 | 基线在位 | 2026-08-13 B1 @8019 | **通过**（3.9s） |
| `live_cleanup_by_pe_state_clears_subtree_and_spares_live_sibling` | model_refresh.rs:162 | 自建夹具（4000000001 保留段） | 2026-08-13 补测 @8019 | **修复后通过**（2.4s）——期望向量写于 inst_geo 共享化决策之前（第 5 处测试腐化）：级联删除按设计不回收内容寻址的 inst_geo，归后台 sweep |
| `live_generate_roots_with_coverage_audit` | model_refresh.rs:240 | `AIOS_GEOM_COVERAGE_ROOTS`（如 `24384/24776`） | 2026-08-13 重测 @8019 | **通过**（4.1s） |
| `live_projams_direct_transform_and_data_only_actions_are_distinct` | model_update_plan.rs:1756 | ProjAMS EQUI 在位 | 2026-08-13 B2 @8019 | **通过**（4.6s） |
| `live_issue5_moving_the_reported_cap_plans_a_branch_regeneration` | model_update_plan.rs:1835 | `/1WCC1135/B1`（7999）+ CATA 闭包在位 | 2026-08-13 补测 @8019 | **通过**（3.5s；规格行入店后隐含直管段判定走整根重生成） |
| `live_issue5_moving_a_container_regenerates_the_branches_beneath_it` | model_update_plan.rs:1872 | `/1WCC-PIPE-RX` zone（7999）+ CATA 闭包在位 | 2026-08-13 补测 @8019 | **通过**（4.1s） |
| `live_projams_real_attribute_sessions_plan_and_execute_distinctly` | 历史位置 model_update_plan.rs:1945 | **一份本机不存在的 ams8000 世代**（见批次 2 补测段） | 2026-08-19 @8019 | **现行源码已退役**：按全名执行匹配 0 项；保留本行作为历史数据绑定记录，不计当前可执行用例 |
| `live_projams_nested_created_routes_and_generates_delivery_roots` | 历史位置 model_update_plan.rs:2119 | 同上 | 2026-08-19 @8019 | **现行源码已退役**：按全名执行匹配 0 项；原 sesno 21/GENSEC 数据绑定记录保留 |
| `live_projams_negative_geometry_change_regenerates_owning_equi` | model_update_plan.rs:2205 | NCYL 负几何 EQUI | 2026-08-13 B2 @8019 | **通过**（5.1s；BREP 缺陷不影响其目标 EQUI） |
| `live_bran_pending_is_actually_regenerated` | model_update_pending.rs:4805 | 既有 BRAN + CATA 闭包在位 | 2026-08-13 补测 @8019 | **通过**（15.9s；闭包收集器 object::values 修复后 SPRE→23274 规格按需入店，子树出模型） |
| `live_hang_pending_is_actually_regenerated` | model_update_pending.rs:4852 | 既有 HANG + CATA 闭包在位 | 2026-08-13 补测 @8019 | **通过**（10.9s） |
| `live_suppo_pending_is_actually_regenerated` | model_update_pending.rs:5269 | SUPPO `24384/25725` | 2026-08-19 @8019 | **前置阻断复现**：夹具查询得到 `None`，期望 `Some("SUPPO")`；该 SUPPO 世代仍不在当前文件与店中；日志 `output/live-batch/remaining-b-data-bound-20260819-083041/03-live_suppo_pending_is_actually_regenerated.log` |
| `live_zone_owned_equi_pending_is_actually_regenerated` | model_update_pending.rs:4870 | 既有 ZONE-owned EQUI；空间树就绪 | 2026-08-13 重测2 @8019 | **通过**（10.5s，空间门前置修复后点亮） |
| `live_shared_spco_cascade_regenerates_every_consumer` | model_update_pending.rs:4957 | SPCO `23274/295504`（规格行按需入店）；自足重建 ref_rev | 2026-08-13 补测 @8019 | **通过**（钉死 1+67 拆为动态口径：先用同一展开器算本店切面根数再对 drain；两个并行实例互扰下仍全绿） |
| `live_generates_a_missing_model` | on_demand_model.rs:447 | `AIOS_ON_DEMAND_TEST_REFNO=24384/24777`（先删该行 `inst_relate` 走真缺失恢复） | 2026-08-13 重测 @8019 | **通过**（4.2s，BOX 按需再生） |
| `test_cal_rooms` | room_model.rs:33 | 房间 mesh 在位 | 2026-08-19 @8019 | **修订后通过**（227.06s）：空间树验真后从库指针重建 42343 条，识别 214 间房/229 块面板，写入 41370 条成员边；旧 124/147 硬编码改为非空门，精确切面可用 `AIOS_EXPECT_ROOM_COUNT` / `AIOS_EXPECT_ROOM_PANEL_COUNT` 钉住；日志 `output/live-batch/room-count-live-fix-20260819-084920/rerun2.log` |
| `test_cal_distance` | room_model.rs:94 | mesh 在位 | 2026-08-19 @8019 | **通过**（14.6s）；日志 `output/live-batch/remaining-room-and-panel-20260819-083240/03-test_cal_distance.log` |
| `test_build_room_panels_relate_common` | room_model.rs:1941 | 改写配置库房间关系 | 2026-08-19 @8019 | **夹具断言失败**：当前返回 0 条关系，历史切面期望 6；日志 `output/live-batch/remaining-room-and-panel-20260819-083240/04-test_build_room_panels_relate_common.log` |
| `live_database_uncovered_noun_histogram` | coverage_audit.rs:236 | 只读，基线在位 | 2026-08-13 B1 @8019 | **通过**（9s） |
| `live_database_uncovered_nouns_resolve_to_modeled_roots` | coverage_audit.rs:267 | 只读，基线在位 | 2026-08-13 B1 @8019 | **通过**（75.9s） |
| `scom_geometry_resolves_from_stored_reference_attributes` | resolve.rs:112 | 13244（ams5052）SCOM 已入店（`aios_db.model.ensure('24384/22456')` 一次即可） | 2026-08-13 补测 @8019 | **通过**（3.4s） |
| `both_catalogue_shapes_resolve_geometry_from_the_scom` | resolve.rs:132 | 同上 | 2026-08-13 补测 @8019 | **通过**（8.1s，六种形态全解出；与 scom_geometry 合进一个测试进程时有缓存干扰，批跑工具单进程无碍） |
| `live_backfill_anc_on_configured_db` | pdms_inst.rs:947 | 基线在位 | 2026-08-13 补测 @8019 | **修复后通过**（3.6s）——回填跳过 ref0 超出 u64 打包上限的行并告警（7 行批次 1 魔术残留），复核口径同步收窄；溢出缺陷闭环 |
| `live_sweep_inst_relate_flat_on_configured_db` | pdms_inst.rs:992 | 生成产物在位 | 2026-08-13 B2 @8019 | **通过**（32.7s） |
| `test_boolean_refno_parse_error` | manifold_bool.rs:670 | mesh 在位 | 2026-08-13 B2 @8019 | **通过**（3.1s） |
| `test_gen_geos` | occ_generate.rs:37 | 基线 + mesh 目录 | 2026-08-13 B2 @8019 | **通过**（3.4s） |
| `test_ancestor`（team_data.rs:166） | team_data.rs:166 | 项目数据在位 | 2026-08-13 B1 @8019 | **通过**（3.2s） |
| `a_reparse_lands_exactly_one_site_per_name` | member_prune.rs:441 | 空 8009 + 本地 AMS 文件 | — | 未跑：需空 8009 专项窗口 |

## C 需真实 E3D（生产空窗 runbook）

| 测试 | 位置 | 依赖 | 最近结果 |
|---|---|---|---|
| `live_real_ftub_delete_move_and_reorder` | increment_pipeline.rs:3437 | AMS 文件里的真实 FTUB 会话史 | — |
| `live_real_delete_session_cleans_up_model_and_regenerates_branch` | increment_pipeline.rs:4290 | `projams_incr_delete_apply.mac` 造的删除会话 | — |
| `live_issue7_real_db_deleted_edges_come_back` | room_live_issue7.rs:204 | 真实项目库（7999 房间） | **2026-08-19 @8019 失败**：生产 drain 已先恢复位置并收口任务，测试末端仍期望消费 1 条，实得 0；日志 `output/live-batch/remaining-room-live-20260819-084116/01-live_issue7_real_db_deleted_edges_come_back.log` |
| `live_issue13_c2_moving_out_of_the_room_clears_membership` | room_live_issue7.rs:356 | 真实项目库 | **2026-08-19 @8019 前置阻断**：起点无归属边，需先建立该房间基线；日志 `output/live-batch/remaining-room-live-20260819-084116/02-live_issue13_c2_moving_out_of_the_room_clears_membership.log` |
| `live_issue5_moving_the_fitting_moves_its_implicit_tubing` | room_live_issue7.rs:523 | 真实项目库 | **2026-08-19 修订后通过**（5.39s）：生产计划精确为 BRAN `RegenRoot` + 靶件 `PostRegenAabb`，位移与恢复均验证；日志 `output/live-batch/issue5-live-expectation-fix-20260819-084804/rerun.log` |
| `live_issue13_c3_on_demand_pending_is_invisible_to_its_own_database` | room_live_issue7.rs:642 | 真实项目库 | **2026-08-19 @8019 通过**（14.8s） |
| `live_issue7_probe` | room_live_issue7.rs:715 | 只读探针（真实项目） | **2026-08-19 @8019 通过**（14.0s） |
| `the_deleted_site_is_pruned_from_a_real_parse` | member_prune.rs:369 | 真实 E3D 库文件 | — |
| `live_identity_query` | e3d_mcp.rs:240 | AMS E3D 装机 + TTY | **2026-08-13 通过**（D: AMS、只读 TTY，3.82s；`output/e3d-mcp/query-66444-20260813152439638629/identity.log`） |
| Plant UI + db8000 FTUB OWNER 搬移/恢复（手工 live 场景） | `l3_ftub_move_apply.mac` / `l3_ftub_move_restore.mac` | 8009 测试库 + 隔离项目镜像 + Plant UI | **2026-08-14 通过并发现回执缺陷**：223→224 数据批次成功，`changed_elements=6`，最终 OWNER 与 UI 模型树恢复；两会话回执仍错误返回 `merged_sesnos=[]`。设备夹具另发现 5 类模型任务漏根及 UI 夹具树定位失败。证据：`docs/evidence/2026-08-14-plant-ui-e3d-crud.md`。 |
| Plant UI + db8000 管道 FTUB 增量续测 | `Test-TtyNetWindow.ps1` / `l3_suite --check-driver` | 隔离 SurrealDB、最小 Design 镜像、Plant UI 隔离设置 | **2026-08-19 TTY 清单覆盖门通过**：隔离 gold 数据库 @8090，actual=58 / manifest=58 / verified=58 / pending=0 / no_geometry=0；日志 `output/tty-coverage-isolated8090-20260819-084723/coverage.log`。**同日 TTY 净窗口续测通过**：真实 db8000 会话 230→231→232，FTUB.POS.U `2900→3400→2900`，合并窗口目标业务变化净零，仅余 BRAN.CACHID 保存元数据，rollback 已验证；证据 `output/e3d-tty-net-window/20260819-082310/`、`docs/evidence/2026-08-14-e3d-tty-net-window.md`。**2026-08-14 管道通过（GENSEC 子场景阻断）**：FTUB 226→227→228 位移/恢复均分类 Completed；数据任务应用 225..=228、`applied_sesno=228`；BRAN 模型生成、FTUB AABB、UI 树/属性/三维显示一致。提交后 F6 OWNER 搬移再通过：228→229→230，OWNER `22402→22404→22402`，数据任务成功、最终水位 230，两个 BRAN 模型生成成功，UI 精确定位 `24384_22403` 并显示恢复后的 owner。CATA 5052 水位 0 的未提交残留先清后重放，最终 PE/统计=306945、水位=189。GENSEC apply 在 SAVEWORK 前 access violation，被分类 Indeterminate 且只执行一次。证据：`docs/evidence/2026-08-14-pipeline-increment-continuation.md`。 |
| db8000 FTUB Refno 重生、模型后移与恢复 | `l3_ftub_add_apply.mac` / `l3_ftub_add_restore.mac` | 隔离 E3D Design 文件 + 8009 + 阶段化后端 | **2026-08-15 数据/队列/恢复通过，新增几何阻断**：237→238 窗口的 Add 复用旧世代 Refno 后只剩当前 owner 边且旧 children 槽清零；BRAN `RegenRoot` 持久保留并在 2016 年历史积压前执行。239 删除恢复后 owner/model 边清空、水位 239。E3D `COPY` 未保留 `SPRE/LSTU`，新增件无可渲染实例，因此本轮未追加 Plant UI 新增几何通过记录。证据：`docs/evidence/2026-08-15-pipeline-ftub-refno-rebirth.md`。 |
| db8000 会话 239 复制 STRU 的 E3D / Plant UI 几何对拍 | `24384/26205`、`inst_geo:15682999992713024124` | AMS E3D + 8009 + 可见 aios-database 控制台 + Plant UI | **2026-08-19 修复后通过**：E3D 与库内 EXTR/VERT、源/副本变换一致；定位到 manifold `.mesh` 为 8 顶点/16 三角/0 法线，导致端盖逐三角异常着色。修复后强制重生成返回 `Generated`、4 个实例；目标网格为 48 顶点/16 三角/48 个面法线，16/16 绕向一致、0 退化；Plant UI 查询 4 元素/4 实例并显示为连续 V 形硬边，水位保持 239、staging=0。证据：`docs/evidence/live-model-mismatch-20260819/verification.md`。 |
| db8000 复制 BRAN 直管权威替换与不可成管诊断线型 | `24384/26229`、BEND `24384/26246` | AMS E3D + 8009 + 可见 aios-database 控制台 + Plant UI | **2026-08-20 通过**：会话 244~250 连删 `26232/26241/26243/26244/26245` 并两次改 BEND `ANGL`（62.342→90→100）后，旧代码的 7 行 `tubi_relate` 四轮改动一字未动、其中 3 行仍指向已删元件。Spec 016 部署后一次 force ensure 收敛为 4 行、第 1 段由 `26232→26236` 纠正为 `26231→26236`、`trans`/`aabb` 全部可解引用。Spec 017 部署后为 6 行：新增的 `26239→26246`（4769.66mm）与 `26246→尾点`（1301.57mm）带 `invalid=true, invalid_reason='direction'`，长度与离线核算一致；Plant UI 以虚线中心线画出这两段，与 E3D 点线一致，A/B 切回 `invalid=false` 即退化为实体管。同库未重生成 BRAN 的旧行无该字段、按可成管处理。证据：`docs/evidence/2026-08-20-bran-tubing-authoritative-replacement.md`、`docs/evidence/2026-08-20-invalid-tubing-diagnostic-line.md`。 |
| db8000 复制 BRAN 内 TEE 删除增量与模型清理 | `24384/26229`、TEE `24384/26232` | AMS E3D + 8009 + 可见 aios-database 控制台 + Plant UI/API | **2026-08-19 通过**：E3D 原生命令窗口删除 TEE 并 SAVEWORK，水位 243→244；PE 正确立碑，OWNER 与元件实例关系清空，模型实例 57→51、生成元素 17→16，staging=0、dead_letters=0。E3D 与 Plant UI 均不再显示该 TEE，剩余管道拓扑一致。证据：`docs/evidence/pipeline-model-current-20260819/verification.md`。 |
| db8000 EQUI BOX 增删管道与模型清理 | `db8000_equi_add_box_apply.mac` / `db8000_equi_add_box_restore.mac` | AMS E3D + 8009 + 正确 Release 控制台 + Plant UI/API | **2026-08-19 通过并纠正错误部署造成的已提交漏删**：会话 240 创建 `32576/1`，数据净变化 Add 1 / Modified 1，模型归一到 EQUI `24384/24776` 且实例数 1；会话 241 删除后，真实文件离线口径为 Modified 1 / Deleted 1 / membership_deleted 1。发现启动脚本误取 `D:\work\plant-code\old\target` 旧程序，改用实际 `D:\Rust\target`；`db_window_repair` 原子清理 `32576/1`，水位保持 241、staging=0，重启后健康为 ok 且模型接口返回 404。证据：`docs/evidence/2026-08-19-db8000-equi-box-increment.md`。 |
| Plant UI + db7999 设备九场景修复复验 | `l3_suite --fixture-ui` | E3D 空窗 + 7999 DESI + 隔离 Plant UI 设置 | **2026-08-17 无 UI 档九场景全绿（首次完整通过）**：隔离副本 `test-increment` 上 9/9 PASS、四平面断言全过（tree/attributes/model/room），会话推进 227→247、水位随批次逐条推进、恢复与幂等复核通过。此前同日两轮九场景全红于第一道断言 `saved session N is absent from data task merged_sesnos`，`--debug-dbnum 7999` trace 定位为**夹具缺陷而非引擎回归**：`execute_fixture_and_wait` 在 SAVEWORK 之后轮询 preview 等窗口张开，而预览唯一写操作 `record_observation` 把 merged_sesnos 冻结基线推到窗口右端（全部 19 条入队记录 frozen_prev==右端），并入名单按规格恒空；已改为读镜像文件头的就绪门（不再咨询 preview），并加源码形状回归测试 `the_execute_gate_reads_the_file_header_not_preview` 钉住。同日 `--fixture-ui` 档：**数据四平面 9/9 全绿复现，UI 冒烟 0/2**——仅有的两个 ui_smoke 场景（data / room-member）都在**变更前**的树定位失败（`inspect tree could not locate before TreeItem`，命令行 locate 后可见树里无任何 `AIOS-INC` 行，UI 同屏显示「增量刷新网格失败：1023 个」与「正在取回工作」），即 2026-08-14 已记录的「UI 夹具树定位失败」在隔离环境复现，属 plant-ui 侧待查，与数据链无关。证据：`test-increment/runs/fixture7999-20260817-145932-trace/`（红轮 trace 诊断）、`.../154724-gatefix/`（无 UI 绿轮）、`.../161633-ui/`（UI 轮，含 inspect 树转储与截图）。2026-08-14 的预检通过与互斥门阻断记录见 `docs/evidence/2026-08-14-increment-closure-rerun.md`。 |
| `issue7_e2e_room_comes_back_after_e3d_save` | tests/issue7_e2e_increment.rs:353 | `Run-RoomE3DE2E.ps1` + AMS E3D TTY + 隔离 Surreal | **2026-08-13 基线阻断，未执行 SAVEWORK**：FIXING 在目录 manifold 旧格式解码失败；BOX 命中回退门 `file=104 < applied=238`。证据：`output/room-e3d-e2e/20260813-{fixing,box}-first/report.md`。**（旧语义现场：ADR-021 后回退不再阻断而是排整库重建批次——重跑前须按新语义重估基线恢复步骤并重新定性）** |

## D 专用夹具 / bench / 探针（按需手跑）

| 测试 | 位置 | 说明 |
|---|---|---|
| `the_live_7324_owner_ancestor_survives_pruning` | member_prune.rs:325 | AMS 7324 专用夹具 |
| `live_7324_parse_failure_is_preserved_as_pe_metadata` | database.rs:320 | AMS 7324 专用夹具 |
| `production_cata_locator_is_identical_and_below_io_budget` | on_demand_db.rs:461 | 生产 ACP 7320 夹具（对拍模式） |
| `folding_a_real_window_preserves_final_state` | increment_pipeline.rs:2602 | `AIOS_FOLD_TEST_FILE` 指定真实窗口 |
| `live_ams8000_net_diff_agrees_with_point_lookups_and_covers_replay` | increment_pipeline.rs（`cache_tests`）——2026-08-19 收集器下沉 pdms-io 时由 session_index_diff.rs 迁入，参照臂 `collect_changes` 在本层 | 真实 ams8000 文件（`AIOS_AMS8000_FILE` 可覆盖），纯文件不连库。**2026-08-18 primaryList 快照接入后复验通过**（`1 passed`、exit 0，18.55s；收集三态不变，证据 `docs/evidence/2026-08-18-core-primary-list-snapshot.md`）。**2026-08-18 T14 共享 diff 后复验通过**（`1 passed`、exit 0，16.08s，证据 `docs/evidence/2026-08-18-core-element-diff-single-source.md`）。**2026-08-18 复验通过**（`test-increment` 副本，`1 passed`、exit 0，15.99s）：cea58087（08-14）把「子页读不动 / 层级不下降」升为整窗硬错误，实测在当前 ams8000 与 07-24 备份上必现失败——已回退为跳过整枝 + 计入 `unreadable_child_pages` / `level_anomalies`（与生产点查 `filter_index_data` 同口径），四窗口逐 refno 仲裁重回全一致（225..=230 与 230..=230 两窗 diff 18ms/11ms vs 回放 490ms/412ms，重复指针 t/b=53/53、层级异常 0）。**2026-08-14 复验通过**：`cargo test --locked --lib data_interface::session_index_diff::tests::live_ams8000_net_diff_agrees_with_point_lookups_and_covers_replay --no-default-features --features ws,gen_model,manifold,project_hd -- --ignored --exact --nocapture`，`1 passed`、exit 0（16.04s）。原始 4 窗口仲裁与性能明细见 **2026-08-13 通过**记录：差分 ≡ 生产 B+ 点查逐 refno 仲裁全一致，695/84/17/11ms vs 回放 10772/2169/471/376ms（debug）。证据 `docs/evidence/2026-08-13-session-index-diff-net-changes.md` |
| `live_ams8000_diagnose_reachable_paths_for_one_refno` | **pdms-io** `src/session_index_diff.rs`（tests）——2026-08-19 随收集器下沉，自足不依赖本仓 | 诊断探针（工具非测试）：`AIOS_DIAG_REFNO`/`AIOS_DIAG_SESNO` 指定目标，dump 索引树可达路径与同键重复条目——差分口径三条实测规则（同键首见、flag 盲、键范围路由）都是它挖出来的 |
| `live_ams8000_net_window_payloads_match_replay_on_single_touch_refnos` | increment_pipeline.rs（`cache_tests`）——2026-08-19 收集器下沉 pdms-io 时由 net_window.rs 迁入，参照臂 `collect_changes` 在本层 | 真实 ams8000 文件（`AIOS_AMS8000_FILE` 可覆盖），纯文件不连库。**2026-08-18 primaryList 快照接入后复验通过**（`1 passed`、exit 0，20.61s；payload 对拍不变，证据 `docs/evidence/2026-08-18-core-primary-list-snapshot.md`）。**2026-08-18 T14 共享 diff 后复验通过**（`1 passed`、exit 0，18.72s，证据 `docs/evidence/2026-08-18-core-element-diff-single-source.md`）。**2026-08-18 复验通过**（`test-increment` 副本，`1 passed`、exit 0，18.76s）：cea58087（08-14）把「终稿记录解析失败」升为整窗硬错误，实测全窗 1..=230 上 64 条字典缺项系统记录必现（首例 `16192_1`：`MNUM not exist in attr_info_map`），含系统段的窗口会整批打死——已回退为跳过 + `unparseable_finals` 计数 + 聚合警告（回放路径对同一批记录同样以 `None` 落空、从未入库）；回退后负载对拍 Add 6,496 逐字符相等、警告 1 条（聚合），净收集 1245ms vs 回放 10959ms。**2026-08-14 复验通过**：`cargo test --locked --lib data_interface::net_window::tests::live_ams8000_net_window_payloads_match_replay_on_single_touch_refnos --no-default-features --features ws,gen_model,manifold,project_hd -- --ignored --exact --nocapture`，`1 passed`、exit 0（18.60s）。原始载荷与性能明细见 **2026-08-13 通过**记录：净窗口收集器 6,499 条 Add 负载与回放渲染逐字符相等；全窗口净收集 1.24s vs 回放 10.9s；字典缺项系统记录 64 条按回放同口径跳过并聚合告警。证据 `docs/evidence/2026-08-13-session-index-diff-net-changes.md`「引擎接线」节。**ADR-022 验收 3 的 live A/B 全链路执行**由 Python 房间增量档承接（不是 `#[ignore]`，不计入本表合计）：`python/tests/test_net_window_ab.py`，跑法 `cd python; $env:AIOS_NET_AB='1'; .venv\Scripts\python.exe -m pytest tests/test_net_window_ab.py -q -s`（@8071 一次性内存库，~3 分 20 秒）。**2026-08-13 通过**（连续两轮）：testbed 8000 窗口 105..=209 两口径完整执行终态逐维等价（差异全部归因：净持真 2 元素 + §5.1 ref_rev 13 边），窗口执行回放 35.0s vs 净 11.0s；顺带抓出并修复 `inst_geo` param 双变体深合并毒化（见 changelog 修复条）。证据同上文件「live A/B 全链路执行」节 |
| `test_net_and_replay_agree_on_a_stock_deletion`（T11b 原双臂名） | `python/tests/test_net_window_ab.py` | — | **2026-08-13 通过**（118s，双臂形态） | **退役（ADR-031）**：改名为 `test_net_window_agrees_on_a_stock_deletion`（净臂单跑）。原双臂记录保留为历史证据。 |
| `test_net_window_agrees_on_a_stock_deletion`（T11b 固定存量删除直证） | `python/tests/test_net_window_ab.py` | `AIOS_NET_AB=1`；@8071 memory；已跟踪 issue-019 ZIP/manifest，baseline@24 → final@26；隔离项目原 db8000 SHA `2eae3055…37137` | **2026-08-20 隔离复验通过**：`1 passed in 20.65s`（exit 0），窗口 25..=26 的净三态为 `0/1/2`，两个固定目标由活行变为墓碑，finally 恢复后 SHA 仍为 `2eae3055…37137`；此前 2026-08-18 与终态签名合跑 `2 passed in 32.55s` | 固定 EQUI `24384_24778`、BOX `24384_24779` 执行前均为活行、执行后恰好立碑。`AIOS_T11B_FORCE_EMPTYRUN=1` 准确报“固定删除目标在起点不是活行”，证明红证有效；旧运行时 oracle / 可变切点已退役。证据 `docs/evidence/2026-08-18-net-window-stable-signature-live.md`。 |
| `test_net_and_replay_full_executions_land_equivalent_states` | `python/tests/test_net_window_ab.py` | `AIOS_NET_AB=1` | **2026-08-13 通过**（连续两轮） | **退役为历史证据（ADR-031）**：执行层双臂在单路径下不可能保留。两轮全绿见 `docs/evidence/2026-08-13-session-index-diff-net-changes.md`「live A/B 全链路执行」节。回归改由下条净臂终态签名承担。 |
| `test_net_window_full_execution_lands_a_stable_signature` | `python/tests/test_net_window_ab.py` | 同上 issue-019 固定窗口；archive SHA `6f7abbf5…f871`，baseline SHA `aa199e88…2d0`，final SHA `84b0040f…6454` | **2026-08-18 primaryList 快照接入后通过**：与 T11b 合跑 `2 passed in 40.52s`（exit 0），原文件恢复 SHA `2eae3055…7137`；T14 后为 `2 passed in 32.55s` | 全链严格签名：队列消费，`changed_elements=3`，`merged_sesnos=[25,26]`，三水位=26，a/m/d=`0/1/2`；墓碑集恰为固定 EQUI+BOX，ZONE 活行，活行相对 baseline 恰减 2，PE key 集不增不减。首轮揭示 preview 漏空操作会话 25，修复为从冻结会话页清单建 `sessions[]` 并补纯单测。证据 `docs/evidence/2026-08-18-net-window-stable-signature-live.md` 与 `docs/evidence/2026-08-18-core-element-diff-single-source.md` 与 `docs/evidence/2026-08-18-core-primary-list-snapshot.md`。 |
| `T18a` release 方向性单点测量（**非性能门，n=1**） | 手跑，非 `#[ignore]` 用例 | release 构建 + testbed 8000 | **2026-08-13 测得** | **非门**：只为预判 ADR-022 决策 4 是否被推翻。高复触窗 104..=209（106 会话，a/d/m=6/51/16，`ops_total` 215，复触率 2.95）完整净收集 3ms vs 回放 53ms ≈17.7×；Add 地板窗 1..=209（复触率 1.05）126ms vs 792ms ≈6.3×（形态决定，不作判定）。**不构成 T18 性能门证据**——正式门要 1 warmup + ≥5 次、median/min/p95、warm 判定 cold 另报，且 250206 SYST 现场硬门未跑。证据同文件「release 方向性单点测量」节 |
| `live_ams8000_single_caliber_release_timing`（T18 记录项） | increment_pipeline.rs（`cache_tests`）——2026-08-19 由 net_window.rs 迁入，计时对象 `collect_window` 在本层 | release 构建 + ams8000 文件（`AIOS_AMS8000_FILE` 或 testbed 8000） | **2026-08-18 测得**（6.51s，exit 0） | **记录项，非门（ADR-031）**：testbed 8000 latest=209，release，Ryzen 9 7950X。高复触窗 `104..=209` warm median 10/9/10ms vs 回放 53/53/53ms ≈5.3×（复触率 3.21）；Add 地板窗 `1..=209` 128/123/180ms vs 908/806/1030ms ≈7.1×（复触率 1.05）。计时对象是生产入口 `collect_window`（含打开文件），不是 T18a 的内层 17.7×。SYST 250206 列为上线后现场复测。证据 `docs/evidence/2026-08-18-single-caliber-net-window.md`。 |
| `persist_ab_on_a_throwaway_instance` | increment_pipeline.rs:2747 | 一次性 8099 实例 A/B 基准 |
| `bench_anc_contains_vs_deep_traversal` | fork_surreal_compat.rs:1048 | 170k 行 fork rocksdb 吞吐基准 |
| `test_model_generation_24383_66456` | test_performance.rs:652 | 生成性能基准 |
| `probe_live_sql` | helper.rs:737 | `AIOS_PROBE_SQL` ad-hoc 探针（工具非测试） |

### 2026-08-19 P5 跨仓编译隔离复验

- 两条纯文件 replay oracle 均显式启用 `legacy_session_replay`：session-index 对拍
  `1 passed`（21.21s，exit 0），payload 对拍 `1 passed`（23.11s，exit 0）。
- issue-019 在仅含 `amssys + ams8000` 的隔离项目副本运行（testbed 正本被现场 E3D
  占用，未触碰）：正常签名 + T11b `2 passed in 37.52s`（exit 0）；强制空跑准确失败于
  “固定删除目标在起点不是活行”，`1 error in 32.20s`（pytest exit 1）；清变量立即复跑
  `1 passed in 37.43s`（exit 0）。副本起始/恢复 SHA 均为
  `84b0040fdbc242d406540eab3d511d41a44aac899f55106821a93f5e419e6454`。
- release 记录项（latest=230，显式 feature）`1 passed`（7.46s，exit 0）：high-retouch
  warm median `11ms vs 60ms`（5.5×），add-floor `171ms vs 1185ms`（6.9×）。
- 完整命令、跨仓 SHA 与回滚记录：
  `docs/evidence/2026-08-19-legacy-session-replay-build-isolation.md`。

## E tests/ 集成 live（2026-08-13 补录，待验）

台账原口径只扫 `src/**`，tests/ 目录的集成 `#[ignore]` 用例除 issue7_e2e（C 组）外一直
游离在外——按「没有最近通过记录视同未验资产」补录如下（不伪造通过记录）。
`db8000_session_pairs.rs` 无 `#[ignore]` 用例（此前审计的命中是文档注释字样），其 CI
常跑对拍由 `index_diff_matches_net_window_on_every_case_window` 覆盖，不入本组。2026-08-20
起该 target 默认依赖图不再启用 `legacy_session_replay`，生产包装、vendor 净窗口与索引
差分直接对拍；同批 `pipeline_window_matches_vendor_net_window_on_every_case_window` 负责
Modified 九桶与 children 负载一致性。

| 测试 | 位置 | 前置 | 最近通过 | 结论 |
|---|---|---|---|---|
| `staged_regen_persists_tubi_mesh_and_boolean_before_advancing_watermark` | tests/staged_regen_e2e.rs:72 | 真实项目库 + 待应用 BRAN/HANG RegenRoot；`AIOS_STAGED_REGEN_DB_FILE` | — | **2026-08-20 执行链完成、门禁断言失败**：隔离 8019 上 db8000 窗口 210..243 暂存并提交，BRAN `24384/23257` 生成成功，水位 209→243；首次以默认线程栈触发 `STATUS_STACK_OVERFLOW`，设 `RUST_MIN_STACK=33554432` 后执行完成。夹具带 4 条已分类 warning，而门禁要求 0，故断言失败；房间阶段 `room_scope_requested=0`，未触发房间重算。测试后已恢复水位 209、PE 6542 行并关闭 8019。日志 `output/bran-room-staged/20260820-134513/staged-regeneration-stack32m.log`、`restore-verified.log`。 |
| `staged_transform_follows_a_pure_pose_move` | tests/staged_transform_e2e.rs:183 | 真实项目库 + 待应用位姿增量；不设 `GEN_MODEL_DIRECT_INCREMENT` | — | **2026-08-19 执行链通过、断言失败**：7997 窗口 105..106 已经暂存、生成 2 根并提交，水位推进到 106；结果携带净窗口口径提示及 2 个未解析生成根 warning，旧断言要求 warning=0；日志 `output/live-integration-remaining-20260819-083450/02-staged_transform_follows_a_pure_pose_move.log` |
| `staged_pane_replay_goes_through_the_kvmem_window` | tests/staged_pane_replay_probe.rs:112 | 7997 待应用窗口在位（探针型） | — | **2026-08-19 前置阻断**：前一 staged transform 已把 file/applied 收口为 106/106，本轮无待重放窗口；日志 `output/live-integration-remaining-20260819-083450/03-staged_pane_replay_goes_through_the_kvmem_window.log` |
| `rebuild_room_membership_on_the_live_project` | tests/room_rebuild_repair.rs:80 | 真实项目库；`AIOS_ROOM_KEYWORD` 可选过滤 | 2026-08-19 @8019 | **通过**（206.63s）：空间树 42341，214 间房/229 块面板，room_relate 1→41367（+41366）；1 块无几何面板按口径跳过 |
| `generating_one_root_fills_geometry_aabb_and_tree` | tests/gen_one_root_probe.rs:90 | 真实项目库；`AIOS_PROBE_DBNUM` / `AIOS_PROBE_ROOT` 可配 | 2026-08-20 @8009 | **通过（含 Plant UI / E3D 外观对拍）**：`AIOS_PROBE_FORCE=1` 定向替换 PANE `24381/36945`，生成根 `24381/36944` 返回 `Generated`、可渲染 1、写入 2；geometry 60759→60759、AABB 59394→59394、空间树 105650→105650。布尔网格逐点满足半球方程（最大径向误差 `0.061 mm`），实际界面差异根因是 `insts_flat = NONE` 时 Plant UI 的 slim 回退忽略 `booled_id`，错误加载带 `Z×234` 变换的正体。现已同步修复 Manifold/OCC 写入、平表补扫与 Plant UI 回退查询；部署新程序后同一 REFNO 右视图为宽高比 2:1 的半球，ERROR 计数 0。证据：`docs/evidence/2026-08-20-rm13-dome-live/verification-record.md`、`plant-ui-fixed-right-view.png`。 |

合计 94 项：A 29 / B 39 / C 10 / D 11 / E 5（2026-08-14：A 新增并通过
`live_startup_sweep_repairs_a_caught_up_ghost_watermark`；2026-08-13：D 增会话索引差分
对拍、诊断探针、净窗口收集器负载对拍；A 曾修正 27→28——08-13 新增的
`live_rollback_and_ghost_watermark_reinit_end_to_end` 此前漏计；E 组为 tests/ 集成用例补录）。

**2026-08-19 Oracle 二次审核正确性收口（离线门禁）**：CATA/Watch Scope、提交回执、
epoch barrier、几何双策略和硬分块单测通过；四个 CI 集成目标为 6/6、15/15、21/21、
3/3，Release 构建通过。三个依赖仓库已发布固定 revision，工作区本地 `[patch]` 已关闭。
本条只登记离线与构建结果；E3D 保存、tail 延迟注入和 staged NotManifold 尚未形成新的 live
通过结论。命令、字面输出与退出码见 `docs/evidence/2026-08-19-oracle-review-correctness-closure.md`。
