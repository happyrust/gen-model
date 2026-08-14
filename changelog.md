# 变更记录

## 2026-08-14

### 新增

- 新增 ADR-024 与 `specs/005-shape-save-coalescing`，定义模型实例保存的有界合批、先计划后修改、确定性分包和成功后计入产出约束。
- 新增 ADR-026：扫掠体公开步骤按 `DB_Gensec` 蛇形命名；可复用直线无斜切时单位网格身份只键目录截面。
- Python RVM AABB 对拍：`python/scripts/rvm_aabb_compare.py` 先打 AMS 1112 CWALL `/1RS-WF03-W-C-RR001` 的 WALL/STWALL（SweepSolid），口径对齐 `rvm_gate_c_or_1r345_c.ps1`（3mm / 3%）。
- Mesh 级对拍：`fast_model::shared::two_sided_surface_distance` 双向采样表面距离（mean/rms/p95/Hausdorff，parry3d，无新依赖、进 CI），三角化无关；`rvm_baseline::mesh_compare` 用 rvm-rs `Tessellate` 取 RVM 世界三角、gen 侧 `inst_geo.param` 就地 `gen_occ_shape`+`gen_occ_mesh`（param 为空的布尔/复合几何回退磁盘 `.mesh`）取世界三角。对 1112 CWALL 4 堵 WALL 与 8000 C-OR 管系（FTUB/BEND）做 mesh 对拍（live 8009+occ）：墙与 FTUB 几何忠实；BEND 逐元素多约 100mm，但 BEND+相邻 FTUB 的 union 与 E3D 只差 5.8mm——是元素边界拆分口径不同、装配无害。gen 几何经 mesh 级验证在装配层正确。
- Mesh 对拍扩到 1112 直线 STWALL（双向 ≤0.06mm）与 8000 `/C-IY-1R330-B`（ACP1000 槽盒，35 构件 union 的 gen→rvm p95=4.14 / max=24.9mm）；E3D 槽体外壳比 gen 管段大约 100mm 高，rvm→gen 大是范围差。
- Mesh 对拍扩到同一 CWALL 的 20 堵 GWALL 挤出：11 堵盒状（≤16 三角）gen→rvm p95=0；大体量 3 堵 gen 多出 0.6–1.3m，高面片墙 rvm→gen ~450–650mm 为开洞。

### 修复

- 管道 FTUB 续测关闭三处增量缺口：staged 初始化尾事务保留未执行的 `RegenRoot`，模型 drain 优先最新真实保存而非历史更新时间，Add 复用旧世代 Refno 时先清理全部旧 owner/children 边；F5 同步换成当前文件可达的 FTUB 夹具并增加工程控制库与依赖 Refno 前置检查。
- 管道增量续测修复两处可重放缺陷：L3 变更宏现在识别带保存注释的 `SAVEWORK`，TTY 夹具显式传递目标 DB 与项目并按会话事实分类；水位仍为 0 的中断基线会先清除未提交 PE 再全量解析，避免 `INSERT IGNORE` 永久保留陈旧行。
- 模型实例纯数据写入的源码顺序守卫改为跟随当前 SavePlan 入口，并统一 CRLF/LF 后再切片；Windows 与 CI 现在验证同一段生产保存路径。
- 四份 e2e 探针（`staged_pane_replay_probe` / `staged_regen_e2e` / `staged_transform_e2e` / `issue7_e2e_increment`）的 `DiscoveredBatch` 补上 `phase`/`epoch_id`，`Run-LiveBatch.ps1` 的 `cargo build --lib --tests` 预编译不再被挡住。
- 模型持久工作改为每页至多 16 根、逐根执行，并在认领前及每根前后检查初始化 epoch、模型门和数据队列；数据到达时以 `model_drain/yielded` 收口，未执行行不改变状态、错误或重试次数，健康状态补充让位原因、耗时和 attempts 变化量。
- E3D 变更宏按 DONE、ALIVE、退出码及保存前后会话号分类；已保存但未确认的运行继续验证而不重放 apply，只有已知的保存前启动失败允许一次重试。L3 夹具统一采用 `preview → SAVEWORK → execute`，并验证保存会话进入 `merged_sesnos`。
- L3 的 Plant UI 运行改用隔离设置文件和仓库根工作目录；自动化按 `tree_item + refno` 定位，数据批次后等待关联 `model_drain` 与 pending 收敛，不再把数据任务的 `units` 当作模型完成证据。

- `inst_relate` 世界包围盒对 `PrimLoft` 圆弧扫掠按环扇取样（含世界轴交叉），不再把局部 AABB 当盒子做 8 角变换。
- 扫掠体斜切改为相对该段切向的垂直/平行抑制（`1e-6`），不再用世界 `±Z`；BANG/PLAX/镜像/路径方向进实例变换。目录路径组合 `get_trans` 旋转，SavePlan loft 夹具改用非 Z 切向方切。

- 合并生成器小尾批的重复元数据查询与 SQL/journal 写入；NaN 和持久化 ID 冲突在删除旧模型前响亮失败。
- 启动、回退重建及同轮稳态更新统一为 `SYS/DICT → CATA → DESI → 模型 → 房间`：
  完整清单按 epoch 安装，Watcher 事件只触发防抖全量重扫；早期阶段失败、身份阻断
  或目录不可读会关闭后续数据与模型门。队列、健康状态和手动回执新增阶段/epoch/
  blocker/shadowed 观测字段。
- 新增 `catalogue_project_priority`，跨项目 DICT/CATA 同 dbnum 由显式项目顺序选主；
  同项目重复、未知配置或无优先级候选整阶段阻断，被遮蔽文件不写 observation 与水位。
- 全量 `sync_pdms` 改为全局 Meta、Catalogue、Design 三次 await；启动提前开放 Web 与
  data-only worker，最后一个 DESI 水位完成后才执行全量模型、持久模型工作、AABB 与房间。
- 启动重扫现在会在应用水位恰好追平文件时校验数据支撑：`pe` 零行且没有匹配
  空基线凭据的库按首次导入入队，由 worker 重建基线；合法空库凭据与水位同事务
  收口。生产缺省 `startup_autorun` 同步翻为 `true`，未解析库和异常水位库无需再等
  下一次文件保存触发。
- 增量正确性阻断：数据队列新增 `apply_window` / `reinitialize` 显式意图，回退到
  零会话也会以 `0..=0` 到达 worker；排队重建意图占优、运行中保留后继，冻结点
  复核仍判 Rollback 才清库。空文件清库后直接 Applied 且水位保持 0。
- `fast_delete` 的统计/持久队列、spatial epoch 与水位清零纳入同一显式事务；
  关系和 Ref0 区间删除继续独立幂等，水位更新保持事务末句。
- 幽灵水位清库会从 `dbnum_info_table` 的 record id 恢复真实 Ref0，并与 PE 前缀
  取并集后删除对应区间；不再把 dbnum 数值冒充 Ref0，PE 零行时也能清理派生行。
- 净窗口把不可读子页、层级不下降、终稿解析失败和 last-touch 缺失升级为整窗
  错误，杜绝残缺触达集落库推进水位；Modified 基版本失败仍保守降级为 Add。
- `ref_rev_maintain` 补偿载荷改为非空、全量严格解析，任一非法 refno 均进入失败
  记账并保留队列行，不再以空修复调用静默销账。
- 收集接口统一为 `CollectedWindow`，把冻结窗口的实际会话页清单与操作流、口径和
  warnings 一起贯穿预收集、崩溃重放及成功回执；空保存、自抵消和稀疏会话现在
  正确进入 `merged_sesnos` 及平行保存时刻。Replay 清单与操作共用一次文件打开，
  两种模式首条 warning 固定自报口径，且后续计划失败也会保留该 warning。
- 基线入口开始硬消费共享 `ScanGate`：身份阻断和回退重建均在计数/解析/水位前退出；
  范围外 CATA 改为跨 scope 收齐全部候选后复用 watcher 的同项目 dbnum 判重，重复组
  零 observation、撤销旧 locator 路径，并在预览/入队回执列出全部路径。

## 2026-08-13

### 新增

- **会话索引差分：db 文件 sesno 窗口净增删改秒级判定**
  （`data_interface/session_index_diff` + `aios_db.parse.net_changes` +
  `python/testbed/net_changes_probe.py`）。每个会话页都带当时的索引根
  （copy-on-write B-tree），取窗口两端的根做双根差分：目标树只下降「页号 >
  base 会话末页」的新页、共享子树整枝剪掉，base 树按共享根集合剪——IO 正比于
  变更量，与窗口内会话数解耦。判定**纯文件**：不查库、不逐会话解析记录，窗口
  由调用方显式给定（源码断言钉死零 `SUL_DB`）。存在性口径与生产 B+ 点查逐字
  对齐，三条规则均由真实 ams8000 实测逼出并钉成回归单测：同键子指针首见者胜
  （Save Work 重写子树留下的陈旧指针，跟进会捞出 1.9 万条已被发布抛弃的临时
  记录）、路由不看 flag（flag=0 的首见指针才是发布后的子树）、键范围路由
  （回收页残留条目键在本叶范围之外，点查不可达）。验收：模块纯单测 11 条 +
  `db8000_session_pairs` 性质 h（差分 ≡ 回放折叠，台账腿由性质 e 闭环）+
  Python 离线档 3 条（issue-019 夹具）+ live 对拍 4 窗口差分 ≡ 生产点查零分歧
  （全窗口 695ms vs 回放 10.8s，debug 15–34×）；探针 `--verify` 全量窗口审计
  154 条差异全部点查仲裁归因为回放旧口径盲区（漏报存在 67 / 孤儿腿误报 86 /
  误判 1）。证据 `docs/evidence/2026-08-13-session-index-diff-net-changes.md`，
  live 台账 D 组两条已登记。同日补测 amssys（SYST 8191，169 会话）：10.6×，
  回放折叠净集 **43%（818 条）与生产点查仲裁的两端状态不符**（孤儿 Deleted 腿
  653 为主）；点查是**同源判定基准**（列出/归类分歧，非独立证明全是旧口径盲区），
  删除判据的独立性另由 core.dll 键集差佐证。
- **ADR-022 + specs/003：增量窗口收集改用会话索引差分（净窗口；已接受，P0 已落地、默认 off 灰度中，核心机制层已由 live IDA 闭合，翻默认余下受验收 5 结果层门阻断）**。
  工具层对拍收口后的引擎采纳决策：执行体与预览的收集阶段由
  `collect_net_changes` 接管（逐会话回放退为诊断工具），输出形状兼容——每
  refno 恰一条 `EleOperationData`，净修改由 base/终稿两端版本**一次 diff**
  合成（属性差量 + children 两端 + old/new owner，diff 实现与回放同源单一
  权威）；下游模型计划 / ref_rev / MySQL / 渲染零改动。灰度开关
  `net_window_collection`（默认 off）+ `AIOS_NET_WINDOW`，预览与执行同谓词。
  四条明示行为变化：改了又改回不再 regen、加了又删不留墓碑行、删了又建判净
  修改、逐会话明细退出预览主口径。窗口起点仍由水位给出（ADR-001），回退/
  幽灵水位仍走 ADR-021 整库重建，跨库级联仍走 ref_rev（ADR-003）。
  - 实现落地（同日晚，P0 引擎接线）：`net_window::collect_net_window`（净三态 →
    同形状操作流合成；`diff_ele_data` 忠实复刻 vendor 内联 diff，九桶 + children
    两端 + noun）+ `IncrementPipeline::collect_window` 唯一派发点（预览、执行体、
    崩溃恢复重收集、worker 尾段重收集四处接入，源码断言禁直调回放）；
    `NetEntry::base_loc` 让净修改直读两端版本不付点查；净口径回执首条警告自报
    口径与计数。真实文件逼出第三条口径对齐：字典缺项系统记录（ams8000 全窗口
    64 条）终稿解析失败按回放同口径跳过 + 计数 + 聚合警告，不整批硬失败。
    验收：`db8000_session_pairs` 性质 i（净收集 Modified 负载与回放**逐桶相等**，
    全部案例窗口全绿·样本为各窗口实际 Modified 条目非 test binary 计数）+ live
    负载对拍（6,499 条 Add 渲染逐字符相等，全窗口净收集 1.24s vs
    回放 10.9s）+ lib 710 passed + 离线档 65 passed。已知偏差记 evidence
    「引擎接线」节（`merged_sesnos` 会话页清单口径留待翻默认值前落地）。
  - **live A/B 全链路执行（同日深夜，切默认值前的最后一道证据，已收口）**：
    `python/tests/test_net_window_ab.py`（房间增量档，opt-in
    `$env:AIOS_NET_AB='1'; .venv\Scripts\python.exe -m pytest
    tests/test_net_window_ab.py -q -s`，@8071 一次性内存库）。testbed 8000
    （基线 6,542 行）同一起点、同一窗口 105..=209（净三态 +6/-51/~16，其中原样
    重写 7），off/on 各走一遍完整执行（暂存窗口 + 窗口内生成 + 提交 + 水位收口）：
    **终态逐维等价**——水位 / 共同活行 6,543（逐字段）/ noun 属性表 / pe_owner
    6,542 边 / pending / dbnum_info 记账恒等式全部相等；仅有的偏差全部归因：
    净臂多持 2 个文件真值元素（回放连同旧基线的最终索引漏报，点查仲裁站净一边）、
    13 条 ref_rev 边为回放对 7 个原样重写元素的顺手重建（§5.1 家族，重置后空
    ref_rev 店放大）。窗口全链路耗时回放 35.0s vs 净 11.0s（3.2×），收集阶段
    差分自报 154ms。连续两轮全绿（各 3 分 16 秒）；全量绑定档 83 passed +
    1 skipped（36.4s）。证据同文件「live A/B 全链路执行」节。
  - **M1 正确性闭环（同日，T20 / T11b / T19 / T18a 落地；T13 阻塞未闭）**：
    - **T20 合成器纯单测**：`collect_net_window` 抽出纯合成内层
      `synthesize_net_window(net, resolve)`（`NetChangeSet` 按值接收、resolver 收窄成
      `FnMut(RecordLoc) -> Result<EleData>`、解析上下文错误文案留在合成器），**七条
      纯单测**覆盖三形状 + 基版本失败按新增 + 终稿失败跳过计数聚合 + `base_loc`
      缺失硬失败 + 原样重写计数（原样重写**不是降级**，是正常判定的正常结果）。
      纯提取不伪称先红：安全网是性质 i + 既有 live 对拍，新测试有效性由**逐分支变异
      抽检**证明（5 处准确红，变异代码不入库）。`net_window` lib 13 passed / 0 failed /
      1 ignored（ignored 是需真实 ams8000 的 live，**本轮未跑**）+ `db8000_session_pairs`
      集成目标 20 passed（含性质 i，是用例数不是覆盖窗口数）+ Python 离线
      66 passed / 20 deselected。ADR-022 验收 1 就此满足。
    - **T11b 存量库删除等价直证**：补上「起点早于删除会话、库内确有活行」的形态——
      原 A/B 删除腿是空跑（被删元素在基线本就无行）。切点 K=24、窗口 25..=209，
      文件层净删除 oracle 4 条，起点确为活行且净口径**真立碑 2 条**
      （`24384_24778`/`24384_24779`，⊆ oracle），共同活行 6,536 逐字段一致、
      **0 未归因**，live 118s 全绿；`AIOS_T11B_FORCE_EMPTYRUN=1` 强制空跑变异准确变红。
      存量基线由 `python/tests/_session_snapshot.py`（`session_cut.rs` 的 Python 镜像，
      与 Rust `db_session_fixture inspect` 双向对拍）切 @K 得到；文件换入换出走**同卷
      临时文件 + fsync + `os.replace` 原子替换** + `pristine` 备份 + `finally` SHA 校验，
      收尾源文件 16,504,832 字节无损恢复。**删除判据是纯文件**（core.dll
      `elementsDeletedBetween` 键集差的复刻）；**DB 查询只验证窗口前活行与窗口后墓碑
      两个状态，不作删除判据**，也不用 `search_latest_refno` 点查自证。
    - **T19 qualifier 恢复对拍（非阻断，CLOSED）**：断言落 `db8000_session_pairs.rs`
      性质 i 的 Modified 分支，两臂 `qualified_changes` 逐项相等，集成绿，**未扩公开
      DTO**。强度如实标：当前 issue-019 夹具两案例都是删除、数组属性零变化，这条现在
      是 **empty == empty**，**不是 qualifier 语义已覆盖的证据**，价值只在防回归。
    - **T18a release 方向性单点测量（n=1，非性能门）**：高复触窗 104..=209（106 会话，
      a/d/m = 6/51/16，回放 `ops_total` 215，复触率 2.95）完整净收集 3ms vs 回放 53ms
      ≈ **17.7×**，该窗 raw 两臂发散 72 条全部归因回放旧口径盲区、点查零分歧；对照
      Add 地板窗 1..=209（复触率 1.05）126ms vs 792ms ≈ 6.3×（形态决定，不作判定）。
      **结论仅限**「在动机形状上 ADR-022 决策 4 不需修订」；T18 正式统计
      （1 warmup + ≥5 次 / median·min·p95 / warm 判定 cold 另报）与 **250206 SYST
      现场硬门仍未完成**。另：A/B probe 的 4.4× 已明确降级为「净差分 vs 回放完整收集
      的混层下界参考，非门证据」。
    - **T13 Added 夹具 BLOCKED（不得标完成）**：仓内**不存在**同时满足「Added > 0」
      且「raw 净集 == 回放折叠集」的真实窗口——带 Added 的窗口都伴随回放旧口径盲区，
      raw 两集不等，性质 h/i 指过去必红。须用受控 E3D 录 `scratch-create` 案例
      （新建 SITE/ZONE → 建元素 → Save Work，窗口内无删除无临时态）；**不得**为点亮
      它放宽 h/i 断言。**M1 Exit gate 因此仍未通过，M2（T17/T12/T18/T15）不得启动。**
  - **决策澄清（同日，评审后最小补写，不改决策主体）**：ADR-022 新增「算法来源
    与正确性边界」——会话索引差分**不是** core.dll `DB_DB::elementsChangedBetween`
    的复刻，而是 gen-model 吃 dabacon 追加式 CoW B+ 树形状推出来的加速。证据边界
    同时写死：core31-retrace 证据只显示其**外层语义**是元素 /（属性, qualifier）
    级的三阶段六桶差分、外层未见索引根双根页差分，但
    `attributesChangedBetween` / `elementsDeletedBetween` /
    `elementsInsertedBetween` 的页级实现**未逆向**，故**不断言**内核内部绝不触及
    索引根；core.dll 继续是属性/桶语义的唯一权威，本路径的索引差分不援引它作为
    算法来源。正确性契约写明：端点存在性以生产 B+ 点查可达性为 oracle、净三态
    由两端 leaf `(pgno, offset)` 集合差定义、净修改仍用两版本 `diff_ele_data` 对齐
    core.dll 语义，正确性靠三重对拍而非「复刻内核」。同时记两条机制层未闭合风险
    （叶 `flag` 的取值语义与取值全集未逆向、当前口径本就不依赖 flag；删除是移除
    leaf 还是墓碑 flag；`is_start_page` 只是索引条目起始哨兵行为、底层位定义未知
    ——三者均无 live IDA 证据，现有零分歧只证结果层，且**差分≡生产点查是同源**
    （二者都不看 flag），不能当 flag 机制的独立证明），并把翻
    `net_window_collection` 默认值的门写成验收 5：要么 (a) 在可达 core.dll/idb 上
    闭合机制；要么 (b) **显式接受机制层未闭合的残余风险**并补一份**结果层**样本——
    其独立 oracle 必须是**生产可见终态**（E3D/权威库侧对同一元素在删除/重建后的
    在场与否），而非同源的点查仲裁或带旧口径盲区的回放，样本覆盖已观察 flag 取值/
    删除重建/Added-Deleted-Modified 三态，走 (b) 机制层仍标未闭合。默认 off 的
    诊断与灰度不受此门阻断。
    **⚠ 本条的「机制层未闭合 / 无 live IDA 证据 / 只证结果层」口径已于同日晚被
    live IDA 逆向推翻，以下一条为准。**
  - **机制层闭合（同日晚，live IDA 逆向，推翻上条保守口径）**：
    `docs/evidence/2026-08-13_reverse-core-dll-index-leaf-report.md`
    （ida-bridge / idalib，core.dll 3.1，SHA `3c1f…417d`，符号系二进制自带 MSVC
    修饰名、非猜名）证实 core.dll 会话变更枚举（`DB_DB::elementsChanged /
    Deleted / InsertedBetween` → `DB_IndexTableCompare`，dabacon 比较引擎 opcode
    266/270，主索引表 `13387743`）**本就是双根 B+ 索引归并差分**——与 gen-model
    `session_index_diff` **同思想**（gen-model 是纯文件重实现 + 共享子树剪枝，
    非逐指令复刻内核代码）。三处旧「机制未闭合」悬案就此闭合：① 删除 = 键在旧根
    不在新根的**集差、非墓碑 flag**（kind=3）；② 变更检测**全链路**（页取 + begin +
    双根归并）**不读 / 不按 flag 过滤**（flag 在链路外是否另有可见性门未闭合，不写
    功能性否定，report §4.5/C3/C4）；③ `0x80000001` 是**页内键哨兵**（核内以
    `-2147483647` 作键边界识别）。**残留（不阻断翻 on，仅登记）**：`flag` 自身位
    编码（存在 / 偏移 / 位宽 / 枚举）与 **flag 在变更检测链路之外是否另有可见性 /
    过滤门**均未逆向（report C3/C4，有意收口）——可断言的只是「权威变更检测链路
    不以 flag 作门」，不写「flag 全无功能」。据此把 ADR-022 / spec / plan / tasks
    的翻默认门从「(a) 闭合机制 / (b) 接受残余风险」改写为**结果层门**（存量库删除
    A/B、Added 独立夹具、批次冻结快照、会话页清单、SYST 性能——性能门当前**未达**，
    debug 完整收集仅 8.8× / probe 4.4×）。qualifier 维：core.dll 变更粒度含
    `(attr, qualifier)`，gen-model `ModifiedElement` 按属性名聚合会丢 qualifier；
    这是回放与净路径**共享的既有形状限制**、切臂不新增，翻 on 不阻断但**非无条件
    安全**，待评估（tasks T19）。

- **ADR-021 + specs/002：水位必须有数据支撑，回退默认整库重建（去档位）**。
  ADR-001 的「失败不推进水位」管的是写的一侧；读的一侧有两个洞。其一，
  `needs_initial_load` 只问水位不问数据，「水位非零、`pe` 零行」被判成正常
  增量，从 `applied+1` 起重放，`1..applied` 静默缺失（看得见数据的
  `baseline_needs_full_parse` 在 `initialize_dbnum_baseline` 内部，够不着
  路由）。其二，文件回退默认只阻断等人，而阻断会静默消失——`file_latest`
  一旦涨回 `applied` 之上，被替换的那段差异永久丢失。
  - 现场实证：8009 上 dbnum 7350 / 7353 / 7741 的 `applied_sesno` 为
    208 / 101 / 94 而 `pe` 零行；同日 8 个库因文件在 08-12 19:04 被整批换成
    更旧副本而回退阻断。证据 `.scratch/realign-20260813-114321`。
  - 决策要点（评审决议 2026-08-13）：回退**默认整库重建**——扫描只分类入队
    重建批次，worker 冻结点复核仍判回退才 `wipe_dbnum_for_reinit`（整库清空 +
    水位行清值不删行 + 统计与队列残留清空 + spatial epoch 递增），随后按首次
    导入全量解析；`watermark_realign` 档位、`AIOS_WATERMARK_REALIGN`、
    `POST /dbnums/{dbnum}/realign` 端点与 `aios_db.sync.realign` 绑定全部
    移除。幽灵水位（`applied>0` 零数据）由 `needs_initial_load` 的数据支撑
    维度路由到基线（判据落在路由、不落入队门——空库会无限重解析）。
    `TypeChanged` / `Duplicate` / `Missing` / `ForeignProject` 照旧阻断。
  - 实现落地（同日）：`needs_initial_load` 增加数据支撑维度 +
    `dbnum_has_any_pe_row` 存在性探针（只在有增量窗口要跑时付一次，读失败上浮为
    批次 Failed）；`scan_and_check_file` 返回三态 `ScanGate`（放行/阻断/重建），
    sweep 与 watch 对回退构造 `reinit_batch`（applied=0 形状）入队；
    `fast_delete::wipe_dbnum_for_reinit`（与快删同源的三阶段删除，元数据阶段改为
    统计清空 + epoch 递增 + 水位清值不删行且置尾作提交点）；执行体
    `execute_one_dbnum` 冻结点复核仍判回退才清库，清库失败计 Failed 幂等重放；
    预览 `blocked`/`initialization_required` 与执行体同谓词。
    `FileAnomaly::auto_realignable` 更名 `requires_reinit`。拆除面：
    `WatermarkRealign` 档位与环境变量、`realign_rolled_back_dbnum` /
    `realign_dbnum_checked`、HTTP `POST /dbnums/{dbnum}/realign`、
    `aios_db.sync.realign` 绑定、`AiosClient.realign_dbnum`、
    `python/tests/test_watermark_realign.py`（由 `test_rollback_reinit.py` 接棒，
    见下）。
  - Python 闭环用例 `python/tests/test_rollback_reinit.py`（房间增量档，@8071
    一次性内存库）：走与服务同一台机器（`incr.execute_manual` 子集），模块级
    引导一次（SYS meta 解析撑起 MDB 范围 + 7998 首次基线），三条用例分别钉
    回退整库重建（幸存位/幽灵位标记行都必须物理消失）、幽灵水位路由到基线
    （行数回到完整基线，增量重放做不到）、类型变更照旧阻断（水位与数据纹丝
    不动）。conftest 导入期补 `RUST_MIN_STACK=16777216`（执行链在默认线程栈
    上溢出，与 testbed 脚本同一惯例）。全套 `pytest -q` 80 绿（36.5s）。
  - live 首跑抓出并修复一个真缺陷：增量形状（start_sesno>1）的批次先开 ADR-017
    暂存窗口、执行体改道基线后窗口缺 finalize plan 而 failed——`batch_worker`
    开窗前新增冻结点预判 `batch_reroutes_to_initial_load`（applied=0 / 回退 /
    幽灵水位一律不开窗），与执行体共用同一个数据支撑探针。
  - 验证：CI 口径受影响模块单测 155 绿；live
    `live_rollback_wipe_clears_the_dbnum_for_reinit`（4.7s @8019）与
    `live_rollback_and_ghost_watermark_reinit_end_to_end`（22.3s @8019，两幕）
    通过，台账与 `docs/evidence/2026-08-13-adr021-rollback-reinit-live.md` 留痕；
    Python 离线档 62 绿。
  - 「在水位行上记录来源（基线收口 / 增量收口 / info 表播种）」与
    「`applied_sesno_time` 交叉核验（停机窗口内回退又长回去）」记为后续项。

- **增量模型生成单元测试总纲**：重写
  `docs/2026-08-06_model-increment-unit-test-plan.md`，把 S0–S13、U1–U13、
  暂存窗口 I1–I9、房间 RI-1–RI-15、离线夹具与 live / E3D 边界收进同一入口；
  明确 P0/P1/P2 待补项、具体文件落点、“回退即红”条件、Constitution Check
  和分波次门禁。当前源码枚举快照为 765 项（82 ignored），`http_api` 为 776 项
  （82 ignored）；长期状态仍以源码枚举和 live 台账为准，不再复制漂移总数。

### 修复

- **`inst_geo` 几何参数双变体深合并毒化共享单位行（live A/B 抓出的真缺陷）**：
  `render_inst_geo_merge`（2026-08-13 `276aa5f6` 用 `UPSERT … MERGE` 替换
  `INSERT IGNORE`）忽略了「不同 `PdmsGeoParam` 变体可以合法共享同一记录 id」——
  普通 LCylinder 与非切角 SCylinder 的单位网格同为单位圆柱，
  `hash_unit_mesh_params` 按设计同返 `CYLINDER_GEO_HASH`，两个变体先后 MERGE 把
  `param` 深合并成 `{PrimLCylinder, PrimSCylinder}` 双键对象，enum 反序列化永久
  失败，**所有**引用该共享行的根从此生成不出来（A/B run4 实测：2,229 根批量重
  生成全灭 + 逐根重试全灭，`decode mesh parameters failed`）。改为
  `render_inst_geo_upsert`：`param` 整值 `SET` 覆盖——行缺失补齐、半成品修复、
  meshed/aabb/pts 派生字段保留，且对已被旧写法打坏的双键行**自愈**（下次参数
  刷新整值盖掉即恢复可解，2026-08-13 后跑过生成的持久店无需手工修）。回归：
  `a_variant_switch_on_a_shared_unit_row_replaces_param_wholesale`（回退 MERGE
  写法当场红）+ 半成品修复用例改跟新入口 + 源码钉
  `production_inst_geo_writes_replace_param_wholesale`（禁 MERGE 回潮）。受影响
  面：lib 定向 12 条全绿、`db8000_session_pairs` 20/20 全绿、全量绑定档 83
  passed + 1 skipped。
- **`room_model` 无 project 特性构建编译修复（响亮拒绝）**：`configured_match_room_fn` /
  `load_room_panel_map` / `load_room_panel_map_from_pe` / `build_room_panels_relate` /
  `build_room_panels_relate_common` / `load_room_panel_groups` 此前只有 `project_hd` /
  `project_hh` 两条 cfg 分支，两者皆未启用（CI 单测组合 `ws,gen_model,manifold`）时
  `configured_match_room_fn` 无返回值（E0308）、`let sql` 门控外的取用点找不到 `sql`
  （E0425×2），整个 lib 编译不过。按宪法「禁止填近似值」改为**响亮拒绝**：无 project
  特性时各入口 `anyhow::bail!` 明示「需要 project_hd 或 project_hh」，原实现体整体入
  `cfg(any(...))`；`configured_match_room_fn` 同样门控（无 project 时其调用方已全部
  bail，不再被引用）。附回归单测
  `room_subsystem_loaders_loudly_refuse_without_a_project_feature`（仅在无 project 组合
  编译运行，断言两个 loader 返 Err 且提示特性名）。
- **增量流程文档一致性修复（2026-08-13 流程审计定案，纯文档面）**：
  - 宪法 v1.0.0 → **v1.1.0**（`.specify/memory/constitution.md`）：I 条回退语义按 ADR-021
    改写（回退默认整库重建、仅 `TypeChanged`/`Duplicate`/`Missing`/`ForeignProject` 身份
    歧义阻断、补「承诺必须有数据支撑」读侧对偶），附加约束「并发模型」按 ADR-011
    2026-08-09 修订改写（一个派发器 + 至多 8 个在飞批次）；Governance 增修订记录
    （动机 / 受影响 ADR / 迁移路径），Last Amended 2026-08-13。
  - AGENTS.md 水位段与配置段对齐（消除「回退阻断」与「回退默认整库重建」同文矛盾，
    补数据支撑一条），队列门控段的「同一个 worker」补派发器限定。
  - ADR-021 状态「提议中」→「已接受（2026-08-13 评审决议）」。
  - ADR-011 2026-08-09 修订下游同步：`docs/specs/web-service-api.md` §2/§4.3/§6、
    `specs/002-watermark-data-backing/spec.md` Assumptions、ADR-021 引言——「单 worker /
    一个消费者」措辞补「一个派发器、默认 1、可配至 8 在飞」限定，行为描述不变。
  - `specs/002-watermark-data-backing/` 补 `plan.md`（含 Constitution Check：I 条冲突
    处置 = 本次修宪）与 `tasks.md`（按已落地事实事后补记留痕，每条带文件路径）。
  - live 台账（`docs/2026-08-12_live-test-ledger.md`）：合计修正 86→**92**——A 27→28
    （08-13 新增的端到端用例漏计）；新增 E 组补录 tests/ 目录 5 条集成 `#[ignore]`
    待验行（staged_regen / staged_transform / staged_pane_replay / room_rebuild_repair /
    gen_one_root；`db8000_session_pairs` 的命中经复核是文档注释、无真实 ignore 用例），
    口径行同步扩展到 `tests/**`；C 组 issue7_e2e 行加注「旧语义现场，ADR-021 后需按
    新语义重估重跑」。
  - 房间增量测试计划（`docs/plans/2026-08-12-room-incremental-live-test-plan.md`）§7
    增 08-13 重估行：db1112 的 F6 阻断判词被 ADR-021 取代——部署新二进制后首轮重扫
    将排整库重建批次，Phase C 前置需纳入重建时序、代价与重新定标。
  - CONTEXT.md 增词条：数据支撑 / 幽灵水位 / 重建批次（各带 _Avoid_ 清单）。

- **fix(incr)：副作用补偿队列补齐死信可观测 / 人工复活（`/update/side-effects/retry`）/
  done 行清扫三出路，并将 `room_panel_relate` 纳入整库重建的 Ref0 区间清库（补
  `room_relate` 漏删的姐妹边）**。逆向核实确认：`room_panel_relate` 的 id 形态
  `{room_refno}_{panel}` 可按 Ref0 区间寻址，而房间重算只对现存实体先清后写、从不
  整表清空，整库重建后的孤儿边无人回收（ADR-010 D4 幽灵同类）；修复为
  `fast_delete.rs` `RANGE_TABLES` 增表 + 回归测试
  `the_wipe_deletes_room_panel_relate_alongside_room_relate`，ADR-021 §4 口径同步补记。

- **fix(incr)：重扫路径读不出文件最新会话号时不再吞成 0（消除假回退告警与失实的整库
  重建播报）；CATA 定位器读登记表失败上浮、缺 `db_type` 的库计入 missing 并告警；
  MySQL 镜像 NAME 改参数绑定、DBNO 缺失告警；MQTT 同步去重查询插值统一过
  `escape_surql_str`；各附回归测试**。

## 2026-08-12

### 新增

- **db8000 会话对回归进 CI：夹具格式的七类性质断言，数据驱动**（方案
  `docs/plans/2026-08-12-db8000-session-snapshot-fixture-test-plan.md` 阶段三 + 四）：
  - 新增 `tests/db8000_session_pairs.rs`（18 passed）：对 `aios-session-fixture-v1`
    夹具里的**每个案例**跑七类断言——档案完整性、窗口切片、时点一致性、并集律、
    净变化折叠、快照差分对账、历史还原。不硬编码任何 sesno 或 refno。
  - **不等真实录制**：夹具来源两条，`AIOS_SESSION_FIXTURE` 指现成目录，缺省则
    从 issue-019 的 final 现场 `pack` 一份。后者是真实 db8000 会话链上的真数据
    （阶段一自检已证明现切台账与当年独立录制逐字节相等），只是案例集小。真实
    录制到货后改指环境变量即可，断言不动。
  - a) 与 g) 直接复用 `pipeline::verify_fixture`——它本就在做那两件事，重写会
    产生两套必须永远一致的口径。
  - f) 快照差分是新增的通用 oracle：`read_raw_records` + `parse_raw_ele_data`
    在 before/after 两份现切快照间逐元素比对（存在性 + noun/name/owner +
    **children** + 属性表），与净结果互证。**children 必须进比对**：实测
    issue-019 的删除序列里，父件与祖父的属性表一个字节都没动，Modified 的信号
    全在 children 列表上；只比属性会把它们误判成「增量说变了但文件没变」。
    噪声属性白名单目前为空——方案预判的 CACHID 类漂移在这条链上没有出现，
    常量留着是机制不是占位。
  - CI：`db8000_session_pairs` 接进同一 job；失败时 upload-artifact 传完整断言
    输出与夹具台账（新增 `AIOS_SESSION_FIXTURE_KEEP` 让合成夹具落在工作区，
    否则临时目录跑完就没了、远程红了无从对账）。
  - job 更名 `db8000-model-increment` → `offline-increment-regression`：它现在
    跑五步离线回归，早已不止 db8000 的模型增量。**配了分支保护必需检查项的话
    需要同步旧名字。**
  - 远程首跑（run 31572427572，2026-08-12 15:03 dispatch）：
    `offline-increment-regression` **首跑即绿，28m58s**——五步（issue-019 夹具、
    通用切割自检、session-pairs 回归、记录边界解析、删除清理 lib 用例）在
    GitHub Actions 上全过。同 run 的 `python-bindings` 在 wheel 冒烟一步红
    （runner 侧 DLL 解析，与离线回归无关），修复走 `4ddf32b9` 的 DLL 探针
    传递闭包，验证 run 另行跟进。

- **db8000 录制清单补齐到 6 类变更形态，并加了一道离线闸**（方案阶段二的离线前置；
  录制本身仍等生产空窗）：
  - 新增 8 个宏、5 个案例：`scratch-create`（added）→ `data-rename` /
    `transform-move` / `geometry-resize`（各 modified，apply+restore）→
    `delete-box`（deleted）。`-CheckOnly` 静态审查 12 条腿 / 7 案例全过。
  - **不动生产元素，改为自建 scratch 元素**：restore 腿必须把值放回原状，而
    生产元素当前的 POS / XLEN 离线不可知，照方案原文写就得先占一次空窗做探针。
    自建元素让每个 before/after 值都自决，整套宏离线可写可评审；末位 delete
    收尾后库在逻辑上回到原样。副产品是 `added` 净形态——原案例表里一个都没有。
  - 新增 `recording_manifest_survives_the_sesno_assignment_it_will_get`：按录制
    脚本的 sesno 分配规则预演清单，`plan_cases` 必须接受，`expected_net.element`
    必须已声明、宏文件必须在场。这类错原本要等占完空窗、录完一整轮才在打包时
    炸，现在 `cargo test` 就拦（防伪已验）。

- **live 用例台账 + 首批点亮**（7-27 测试计划 Gate 3 的首次执行）：
  `docs/2026-08-12_live-test-ledger.md` 给全仓 82 个 `#[ignore]` 用例建档
  （四类目标要求 + 最近通过记录），`scripts/Run-LiveBatch.ps1` 按清单逐项
  独立进程定靶批跑（`DB_OPTION_FILE` 与 `AIOS_LIVE_*` 两套寻址同源派生、
  恰一命中、逐项超时、JSON 报告）。批次 1（自建夹具类 26 项）全部得出结论：
  **23 项首次取得可复现通过**（12 @ testbed 8019；11 项 room_fixture @
  一次性空库 8071 专用清单——房间覆盖率闸门在带真实基线的库上对「只灌夹具」
  的树必拒，空库上夹具行自然对得上分母、闸门语义原样保留），3 项阻塞定性在案
  （积压前置 / 缺陷面板数据依赖 / 断言写死生产 MDB 语义）。顺带修三处测试
  腐化：白名单落地前的夹具命名（first/second 进不了判重）、状态机落地前的
  两个崩溃恢复用例缺测试装载模式声明。另补 IU-S8-05/S12-02 顺序钉（部分失败
  后缓存仍失效、水位不推进）与 IU-S0-05 的 warning 半边——7-27 矩阵点名的
  L0 缺口至此全部关闭或有台账去处。

- **房间增量默认打开（`room_incremental` 缺省值 `false` → `true`）**：`options.rs`
  的 `effective_room_incremental` 缺省翻真，`DbOption.toml` 同步写成 `true`，
  单测由 `room_incremental_is_off_unless_someone_asks_for_it` 改成
  `..._is_on_unless_someone_turns_it_off`，并补一条「认不出的环境变量值退回新
  默认」。
  - 2026-08-10 取假是为了压住现场那 2580 个查不到几何的房间目标（每页 256 个
    付两次全量查询、约 88 秒，把模型侧真正的失败埋进日志）。那批目标已经收干净
    （现场 `/update/pending-units` 的 `room_units` 为空），维持关闭的代价此刻更
    贵：关着时房间归属**只在删除路径**还会被清理（`helper.rs` 的
    `delete_room_membership` 从不看这个开关），搬家后的重算全靠启动全量重建
    回补，而那条兜底路径排在 `startup_autorun` 之后（`skip_startup_room_build`
    的三道门次序）——默认部署两个开关都关着，等于没有回补通道。
  - 门本身一处没动：两个写入点（直写事务的 `room_recalc` 语句、暂存窗口收口的
    `merge_room_recalc_changes`）与一个消费点（`room_round`）照旧读同一个函数，
    翻的只是缺省值。显式写了 `room_incremental = false` 的配置（`python/tests/
    DbOption-ci.toml`、`python/testbed/*`）行为不变；要临时关一次用
    `AIOS_ROOM_INCREMENTAL=0`，不必改文件。

- **空间树一致性闭环：V2 单文件快照、进程状态机、空间串行锁与降级自愈**（方案
  `docs/plans/2026-08-12-spatial-tree-consistency-closure-plan.md`，D1–D8 已定；
  ADR-010 2026-08-12 增补（二）、ADR-017 2026-08-12 补记）：
  - 快照介质：`accel_tree_{project}.bin` + `.meta.json` 退役，改为单文件
    `accel_tree_{project}.snapshot`（V2：树载荷 + SHA-256 自校验 + project/namespace
    身份 + 双字段指纹，原子 rename 发布）。读侧全套校验任一失败即指针重建，不回落
    旧格式；旧文件仅作一次性迁移候选（双指纹匹配且无 pending → verdict=`migrated`），
    首次 V2 发布成功后**删除**——旧二进制对 bin 缺失是无条件重建，任何回退自动安全。
  - 状态机 `spatial_state.rs`：8 态；房间消费者（启动全量重建、RoomRecalc、空闲
    房间轮）仅 Ready/ReadyEmpty 放行（`SPATIAL_TREE_NOT_READY`，durable 行保留），
    解析/生成/重放/重建/`model.spatial.bounds` 不受门禁。启动判据修正：pending
    优先仅对可读快照成立、进入 ReplayRequired 立即重放（不等派发门）、
    「树非空即 preloaded」收窄为显式夹具标记。
  - 空间串行锁 `SPATIAL_STATE_SERIAL`（`STAGED_COMMIT_SERIAL → SPATIAL_STATE_SERIAL
    → GLOBAL_AABB_TREE`）：staged 提交后收敛、direct 写路径、重建换树/发布、快照
    落盘、Python `spatial.*` 同一串行线（修掉 Python reconcile/persist 与 worker
    并发动树的竞态）；journal 写回与尾事务不持锁。
  - 指针重建：record-range 分页（fork 兼容套件双跑钉住页间无漏无重）；口径
    current-only（排除版本化数组 id 行、`in.deleted` 软删行，Rust 侧排除
    NaN/Inf/反向 AABB 并计数采样）；分页读锁外、stamp 前后比对 + 换树 + 发布锁内，
    三连漂移/查询失败进 DegradedBlocked。房间覆盖率分母同口径。
  - 降级自愈：后台 revalidator（30s 指数退避至 5min）只管 DegradedReuse/
    DegradedBlocked，恢复 Ready 唤醒调度器。
  - 崩溃注入：`AIOS_FAILPOINT=<name>` 五个注入点覆盖方案 §8 崩溃窗口。
  - **对外契约变化**：/health `spatial_tree` 九键作废换十五键（台账 G-02 契约
    迁移，形状钉随迁）；`startup_verdict` 枚举改
    reused/replayed/rebuilt/migrated/degraded/preloaded；Python
    `spatial.persist(force=True)` 在非 Ready/ReadyEmpty 拒绝。
  - 沙箱验收（testbed @8019，六场景：首启重建/快路径复用/截断/删除/rename 前
    崩溃注入/崩后收敛）全过，证据
    `docs/2026-08-12_spatial-tree-consistency-acceptance.md`；E3D 侧场景
    （TTY 复制恢复对拍、伪造旧 epoch、房间边对拍）留 runbook 待跑。

- **db8000 会话快照夹具通用管线阶段一**（方案
  `docs/plans/2026-08-12-db8000-session-snapshot-fixture-test-plan.md` §1）：
  切割（`session_cut`）/格式（`aios-session-fixture-v1`）/打包（`archive_util`）
  /管线（`pipeline`）四个模块接上 `db_session_fixture` bin——`pack` 把
  「recording.json + 源 DB 文件」打成只入库最终文件的夹具（台账 sesno 逐切过
  sesno+存在性验证闸、散列入 manifest、6 MiB 预算、收尾即复验），`verify` 对
  夹具目录零外部依赖离线复验（解 zip → 逐台账**现切** → SHA256/大小对账 +
  验证闸，与阶段三回归同一套裁决）。阶段一验收由
  `tests/db_session_fixture_selfcheck.rs` 钉住：用通用模块从 issue-019 zip 的
  final（sesno 26）现切 24/25/26，字节散列与该夹具 manifest 台账逐一相等——
  「任意历史可从最终文件精确还原」在真实 db8000 会话链上成立。同一测试还覆盖
  **pack 往返**（真实源文件 → 夹具 → 复验全绿；台账散列与 issue-019 独立录制
  的那份相等；台账改一位后复验必须变红），因为阶段二的 E3D 录制是一次性的、
  pack 出错要再占一个生产空窗重录。issue-019 专用实现保持冻结不动。该测试已
  接进 CI（`windows-tests.yml` 的 `db8000-model-increment` job，参数与
  issue-019 步骤逐字同款），同批把一直漏在门外的离线解析边界用例
  `--test pdms_record_boundary` 也接了进来。

- **阶段二录制工具**（同方案 §2，待生产空窗执行）：
  `scripts/e3d/Record-Db8000SessionChain.ps1` + 清单驱动的
  `scripts/e3d/db8000_recording_cases.json`（加案例 = 加一对宏 + 一行），投递走
  ADR-019 的 `l3_suite --check-driver`。录制一次性且占生产空窗，所以三道闸都当场
  验：触碰 E3D 前静态审宏（恰好一个 `SAVEWORK`、无 `QUIT`/`FINISH`/`MERGE`/
  `PURGE`、`ALPHA LOG` 成对、`Q REF`+`Q NAME` 齐全）、每条腿后要求 sesno 恰好 +1、
  refno 从宏日志的 `Ref`/`Name` 相邻对回读。配套给 `db_session_fixture` 加
  `inspect` 子命令（只读打印会话链 JSON，与切割同一份解析）。`-CheckOnly` 只读档
  已对真实 `ams8000_0001` 实跑通过（baseline_sesno=210），检查器的拒绝面亦验过。

### 修复

- **直写路径的空间树变更补上 epoch 痕迹，消除崩溃后的静默漂移**（方案
  `docs/plans/2026-08-12-spatial-tree-direct-mutation-epoch-trace-plan.md`，
  ADR-010 2026-08-12 增补）：
  - 钉死不变量——**凡是改变了「树应有内容」的已提交变更，都在同一事务内 bump
    `spatial_epoch:current`**。此前只有 durable 增量与暂存窗口尾事务 bump，
    全量生成 / `manual_update_aabbs` 的普通直写刷新与删除清理两条路既不写
    `spatial_reconcile` 意图行、也不 bump：树同步完、空闲轮落盘前崩溃，重启时
    sidecar 与库指纹相等，启动判据按 Reuse 复用一棵陈旧的树，而 /health 的
    `drift` 恒为 false，无人可见。删除路径的后果尤其重——启动全量房间重建会把
    被删构件按旧包围盒重新收编进 `room_relate`（ADR-010 D4 借崩溃复活，而
    `DeleteCleanup` 任务早已 done，没有重放会再清一次）。
  - `update_inst_relate_aabbs_by_refnos_mode` 的直写事务门控从
    `durable_room_trigger && !chunk_changes.is_empty()` 放宽为
    `!chunk_changes.is_empty()`；`durable_room_trigger` 从此只决定「要不要随事务
    发布 `room_recalc` 任务」，不再决定「要不要事务与 bump」。重算值与树上旧值
    逐位相等的重刷仍走普通写、不 bump——没动树的提交不该作废别人的树文件。
  - `delete_room_membership` 的窗口外分支改为按块「取写锁 → 锁下探测这些 refno
    在不在树上 → 在则把房间边删除与 bump 包成一个事务、不在则照旧普通写 →
    摘树 → 标脏」。探测在锁下做，「要不要 bump」与「树到底动没动」由同一个快照
    裁决。暂存分支不变（意图行 + bump 仍由窗口尾事务收口）。
  - 普通直写分支补上写锁，跨度 [变更判定 → 事务 → 树同步]（durable 增量的全跨度
    锁不变）。顺带关掉一个此前没盘到的交错窗口：并发的删除清理挤在事务与同步
    之间时，刚摘掉的条目会被这里同步回树上，成为要等下次指针重建才自愈的幽灵。
  - 行为变化：全量生成、`manual_update_aabbs`、删除清理的直写提交现在会推进
    spatial epoch（按块，一次全量生成约产生「条目数/100」次 bump）。多次 bump
    语义无害（判据只比相等），代价是这些路径跑过之后，下次启动的全量房间重建
    对账凭据（`room_build:main`）会判为「空间状态已变」而照跑一次。

### 变更

- **`aios_db.model.export_obj` 改为整树单文件导出，子树收集走 anc 索引**
  （2026-08-12 增量审查修复计划 P3）：
  - 对外契约变化：此前每个实例根一个 `{refno}.obj`，现在整棵子树合成一个
    `{refno}.obj`、内部按「实例_geo_hash」分 `o` 组，`files` 恒为单元素数组；
    交付单元根（EQUI/BRAN…）自身没有直接实例行也能导出整树。
  - 子树实例收集只走 `anc CONTAINS`（`idx_inst_relate_anc` 索引查询，anc 含
    自身故根自己的实例行同谓词圈住），不再 OR 无索引的 `in = …` 臂——那会把
    整条谓词退化回全表扫（preload.rs 实测账：1.57s vs 3.1ms）。
  - 响亮失败取代静默空集：refno 解析失败直接报错（此前静默成 0、谓词永不命中）；
    空结果时按 rs-core `inst_relate_anc_ready` 同口径探一次 `anc = NONE`，
    存量未回填的库给自愈指引（启动一次 gen-model 回填）而不是谎报「没有实例」。
  - testbed 全链路（`python/testbed/run_full_loop.py`）导出步骤补形状断言：
    单文件、`o` 组数 == 导出实例数、triangles > 0、无缺失 mesh。

- **`aios_db.full_init` 增加同工程活服务探测**（行为变化：以前能起的场景现在
  可能被拒）：拿锁之后探本机 `http_api_addr` / 8022 / 9099 的 `/api/v1/health`，
  响应是合法 health JSON **且** `project` 与本配置一致就报错退出，
  `full_init(..., force=True)` 显式跳过。动机是单实例锁按「项目根」隔离，两个
  部署包各持各的锁却写同一个工程时锁根本不挡（实测踩过：`test-worklspace`
  的包在 9099、本仓库在 8022）。判据只认 project 名——`/health` 不报「它连的是
  哪个 SurrealDB」，所以隔离沙箱若与生产**重名**会被误伤，用 `force=True` 放行
  （`python/tests/conftest.py` 就是这么做的，并在注释里写明三条资源如何独立）。

- **互踩探测精确化：/health 补报库端点，探测端按「同库」而不是「同名」判**
  （上一条落地当天就被自己的测试沙箱误伤，这是补上的另一半）：
  - `/health` 的 `sul_db` 新增第六键 `endpoint`（配置的 `v_ip:v_port` 原样
    字符串），形状钉与 spec §4.1 同步；`sul_db` 其余五键语义不变。
  - `full_init` 的探测升级为三层判据：`project` 不同 → 无关；服务端报了
    `sul_db.endpoint` → 端点（localhost↔127.0.0.1 归一后）或 `namespace`
    不同都放行——同名工程各写各的库不构成互踩；老版本服务端（≤0.1.18）不报
    端点 → 分不清仍按最坏情况拦，拒绝文案会写明是哪种判法。判定函数是纯函数，
    8 条单元测试钉住（`cargo test -p aios-py --lib`，含对实测 9099@0.1.13
    响应形态的老服务端分支）。
  - `python/tests/conftest.py` 的 `force=True` 暂留：本机 9099 还跑着 0.1.13，
    等同机部署升到带 `endpoint` 的版本即可撤。

- **`aios_db.spatial.tree_status` 的文档与存根不再复述键面**：改为「原样透出
  /health `spatial_tree` 那份渲染，键面以 Rust 侧渲染半边为唯一权威」。此前注释
  与 `.pyi` 各抄了一份九键清单，而 G-02 契约迁移正把它往十五键上带——两处各说
  一套，过期是必然。Python 面只钉判漂移要用的稳定核（`entries` / `file_epoch` /
  `db_epoch` / `drift` / `startup_verdict`），全集的形状钉留在 Rust 侧一处。

- **`aios_db.db.inst` 去掉全表扫，改三段式取边**：① `anc CONTAINS`
  （`idx_inst_relate_anc` 索引查询，anc 含自身故一跳圈住整棵子树）；② 空结果
  回落 `array::flatten(SELECT VALUE ->inst_relate FROM [pe:…])` 图跳，只取元素
  自己那一跳（preload.rs 的实测账：`in` 谓词全表扫 1.57s vs 图跳 3.1ms），兜住
  `anc` 未回填的存量库与直接 `RELATE` 出来的测试夹具；③ 两条都空且库里还有
  `anc = NONE` 行时响亮报错——「查不全」不能被读成「没有」。refno 解析失败也
  改为直接报错（此前 `unwrap_or_default()` 静默成 0，谓词永不命中）。与
  `export_obj` 的差别是多了第 ② 段：那边空结果本就是错误条件，这边空是合法答案。

### 新增

- **`aios_db` 补齐测试支撑面导出，并新开房间增量 pytest 轨**：
  - 新增 `aios_db.fixture`（`create` / `drop` / `move_body` / `refnos`），直通
    `src/fast_model/room_fixture.rs` 的合成房间夹具（1 间 `/ZZ-R-K100` + 2 块
    PANE + 5 个盒形构件，其一骑在重叠区，保留 refno 段 4000000001）——与 Rust
    `room_fixture` live 轨共用同一套数据，两侧断言可互相印证。会写 pe/FRMW/
    inst_*/geo_relate/aabb/vec3 多张表并落 `zzfx_*.mesh`，**只对一次性测试库使用**。
  - 新增 `room.enqueue(changes)`（按 `model.update_aabbs` 的返回形态入队房间
    重算，PANE 走整间分支、其它走元素分支，不受 `room_incremental` 开关门控）、
    `model.delete_subtree(refnos)`（DeleteCleanup 补偿任务同一入口的级联删除）、
    `spatial.tree_status()`（空间树九键指纹，与 /health `spatial_tree` 同源、
    现读现比）、`model.update_aabbs(..., durable=True)`（生产 TransformOnly /
    定向 regen 走的直写事务路径：AABB 指针、`room_recalc` 任务与 spatial epoch
    同事务提交）。
  - 新增 `python/tests/`：对 conftest 自起的一次性内存 SurrealDB
    （`bin/surreal.exe` @8071，进程退出零残留）跑「房间增量收敛 == 全量重建」
    的**逐边**对拍，覆盖构件搬家、面板整间、空刷负例、删除清边留痕、durable
    直写五条；配置 `tests/DbOption-roomtest.toml`（`room_key_word=["ZZ-R-"]`
    只圈夹具房）。conftest 会把仓库根同名空间树文件挪开再还原，不毁真项目产物。
  - 类型存根补齐（新增 `fixture.pyi`，`model` / `room` / `spatial` /
    `__init__` 同步新入口），`py.typed` 对外契约不再漂移。

- **绑定的离线测试档进 CI**（`python/tests -m offline`，60 条）：解析层对着仓内
  `issue-019` 的 db8000 会话快照（与 Rust `db8000_two_delete_fixture` 同一份
  数据、同一串删除序列）、三层硬守护在干净子解释器里逐条验、`.pyi` 与运行时的
  名字集合逐模块对齐、HTTP 客户端对着打桩服务验 12 条 REST 路由与报文形状。
  这一档不连 SurrealDB、不碰 E3D 装机、不扫项目目录，秒级跑完。
  - `.github/workflows/windows-tests.yml` 新增 `python-bindings` job：复用
    `windows-binary.yml` 的 OCCT / protoc provisioning（绑定按 Q7 钉死「与服务
    同一套默认 feature」，必须有 OCCT）→ `maturin build` → 装 wheel → 跑离线档
    → wheel 作 artifact 上传。原 `db8000-model-increment` job 不动。
  - conftest 按选中集合裁定本进程那一份 DbOption（进程级 OnceCell，换库只能换
    进程）：有房间档用例就用 `DbOption-roomtest`，纯离线档用新增的
    `DbOption-ci`。离线用例在任一配置下都成立，两档同跑不冲突。
  - 新增连接层行为用例（`test_connection_layer.py`）：`db.inst` 三段式的每一段、
    `owner_chain` 的自 own 终止、`members`、`spatial.tree_status` 十五键形状钉。

- **版本护栏自锁**：`aios_client.EXPECTED_SERVER_VERSION` 是手抄常量，此前
  `chore(release)` 升 `Cargo.toml` 时没有任何东西提醒同步它——护栏自己先漂移，
  从「提醒你版本不一致」退化成「对着新服务端瞎报警、对着老服务端不报警」。
  离线档新增一条对表用例，bump 忘改常量时 CI 立刻红，红处文案直接写修法。
  `sul_db.endpoint` 恰好是 0.1.19 起才有的键，这条对表马上就有实际意义。

- **空间树一致性闭环落地后的绑定侧跟进**（对方工作流 `445e3cd1` 合入后复核，
  绑定重建 + 全套复跑，77 条绿）：
  - conftest 的树产物搬挪清单补上 V2 单文件快照 `accel_tree_{project}.snapshot`
    ——房间档跑在一次性内存库上，但空间树落盘写的是**仓库根**、文件名与真项目
    同款；清单漏项就会拿测试产物顶掉真项目的快照（介质迁移期间已实际残留过一个）。
    并加源码钉：从 `aabb_tree.rs` 反查 `accel_tree_{}` 的全部后缀，与清单比对，
    下次换介质忘了跟进直接红。
  - `spatial.tree_status` 的断言从「稳定核子集」收紧为**逐键全等**。钉的不是
    契约本身（那在 Rust 渲染半边旁），而是绑定透传没掉键——最容易掉的是取值为
    null 的几个（`snapshot_sha256` / `pending`），掉了不报错，只会让照着 /health
    写的脚本在绑定上撞 KeyError。
  - 钉住「Python 夹具路径免标记通过消费者门」：Rust live 夹具要显式调
    `mark_spatial_tree_fixture_preloaded()`，Python 这条路不需要——`full_init`
    走正经装载器，空库上从指针重建出空树即进 ready 态。省得下次门禁收紧时对着
    一堆 `SPATIAL_TREE_NOT_READY` 猜是不是缺了那个标记。

- **`scripts/smoke_m1..m5.py` 标注退役**：五个脚本全部钉在仓库根 `DbOption` +
  8009 正式库 + `D:/AVEVA/...` 真实工程上，而 8009 的数据目录已决定不修，照原样
  跑必失败。不删（它们是 M1–M5 的验收口径记录），改为每个脚本头注写明「历史
  验收记录 / 为何跑不了 / 等价物在哪」，README 脚本表合并成一行同款提示。
  多数段落已被两档 pytest 覆盖；`parse.noun_dict` 依赖 E3D 装机的 `attlib.dat`，
  没有自动化等价物，头注里点名说清。

- **`aios_client` 版本漂移护栏**：`health()` 比对服务端 `version` 与内置
  `EXPECTED_SERVER_VERSION`（现 0.1.18），不一致抛一次 `AiosVersionWarning`
  （同一个 client 不刷屏），`AiosClient(..., expected_version=None)` 关掉。
  回应实测踩过的 0.1.13 绑定对着 0.1.16 部署包查半天的坑——只告警不报错，跨版本
  多数字段仍通用，硬拦会把「凑合能用」变成「完全不能用」。

## 2026-08-11

### 变更

- **层级查询优化 P3（gen-model 份额）：退役 `inst_relate`/`tubi_relate` 的
  `zone_refno` 列**（方案 `docs/plans/2026-08-07-inst-relate-anc-u64-hierarchy-query-plan.md`
  P3，as-built 记录 `docs/2026-08-07_journal-fn-dependency-audit.md` §4）：
  - 写侧全退：建行字面量（普通行 + TUBI 行）不再写 `zone_refno`，
    `ResolvedInstMeta` 的 zone 槽位与 `resolve_inst_meta` 的 noun 预取移除；
    回填 `backfill_inst_relate_anc` 与 OWNER 搬迁重算
    `render_anc_repair_statements` 不再连带 `zone_refno = fn::find_ancestor_type(...)`
    ——每行一次的 9 跳 owner 上溯从回填成本中整个消失，`fn::find_ancestor_type`
    自此离开 inst 写入链（函数定义保留给材料表等读侧）。
  - 收口探针收窄：`desi_finalize_preflight` / `selfcheck_surreal_functions`
    只探剩余的收口硬依赖 `fn::anc_u64`。
  - 索引迁移：`INST_RELATE_INDEX_SQL` 前两行 `REMOVE INDEX IF EXISTS` 在启动/建窗时
    摘除旧 zone_refno 索引的两个历史名字（`idx_inst_relate_zone_refno` 本仓 F1
    修复后建的、`inst_relate_zone_refno_index` plant-ui rs-core `define_pe_index`
    建的，AMS 实库两者并存实测在案）。存量行旧值保留不删，只是不再写入、不再被
    索引；「索引不存在 / 表都不存在 / 重复摘除」三种 no-op 情况由双跑用例
    `dual_inst_relate_anc_u64_contains_index_agrees` 连 INFO 终态一起钉住。

### 新增

- **`fn::zone_u64` / `fn::site_u64`（common.surql，P3 读侧便捷层）**：从 `anc`
  链尾 O(1) 定位 ZONE/SITE，与元素深度无关——链尾打包值 ref1==0 即 WORL 的
  自适应偏移，判据与 Rust 解析器同源；「含自身」语义与退役的
  `fn::find_ancestor_type` 口径一致，短链/空链返回 NONE。反向圈行（某 ZONE 下
  全部实例）仍走 `anc CONTAINS` 索引查询，不用它。两种链尾真实形态（悬空 WORL
  收尾 / 0_0 哨兵被滤止于 SITE）由 `zone_and_site_helpers_locate_from_the_anc_tail`
  与双跑用例 `dual_anc_u64_functions_execute_and_agree` 双引擎钉住。

## 0.1.18 - 2026-08-11

### 变更

- **空间树启动初始化改为分层判据**（方案与决策记录
  `docs/2026-08-11_spatial-tree-startup-init-plan.md`，ADR-010 2026-08-11 增补）：
  - sidecar 指纹从单一 epoch 数值扩成 **(epoch 值, 库侧 bump 时刻 `updated_at`)**
    双字段，两个字段都与库相等才直接复用树文件（评审要求：与数据库对时间戳；
    库快照回滚恰好撞回同一计数也认得出来）。旧版 sidecar 缺新字段按失配走，
    一次自愈后补齐。
  - 指纹失配但库里还有待重放空间意图 → 复用文件、交给 worker 出队前的意图重放
    自愈（不再像旧 epoch 校验那样每次崩溃重启都全量重建）；失配且无意图 →
    只读指针重建（直写崩溃 / 换文件 / 回滚库）。
  - 树文件缺失/损坏从「空树等人工」改为**自动指针重建**（决策 D1）；库侧诊断
    查询失败降级复用文件 + 告警（D2）；两处启动调用点统一为「告警降级空树、
    不阻断启动」（D3）。
  - /health 新增 `spatial_tree`：文件/库两侧指纹现读现比、`drift`、条目数与
    本次启动裁决（reused / healed_by_replay / rebuilt / empty / preloaded /
    reused_degraded）。

### 修复

- AMS/8000 房间增量灰度闭环：
  - `inst_geo` 的确定性落库从忽略重复改为 `UPSERT ... MERGE`，保留已有 mesh/AABB，
    同时补齐缺失参数并在显式重生成时清除 `bad`；重复执行同一生成批次可收敛。
  - 无圆角、共线回折的房间面板不再走一次性删点失败分支，统一使用逐交点修复器；
    加入 AMS 真实 PLOOP 参数回归。
  - `startup_autorun=false` 时，显式 `POST /update/execute` 即使所选 dbnum 已追平，
    也会为本进程上弦并放行 durable 模型/房间积压，避免人工 canary 永久停在
    `up_to_date`。
  - `Run-RoomE3DE2E.ps1` 新增复用现有 9099 服务的 `db8000-equi-copy` 案例；
    TTY 宏对 `=24384/24776` 执行 probe/apply/restore，并核对水位、新 EQUI、
    `inst_relate`、pending/dead-letter 与空间补偿。

- `AIOS_FORCE_SPATIAL_REBUILD` 只认明确真值（1/true/yes/on）：旧实现判
  `is_ok()`，部署模板写 `=0` 想关闭，实际每次启动都强制全量指针重建。三态解析
  收口在 `batch_worker::parse_explicit_flag`，与 `GEN_MODEL_DIRECT_INCREMENT`
  的 P2-1 纪律同款。

## 2026-08-06

### 新增

- **执行范围缓存 + 周期对账重扫**（现场：数据批次执行中 SUL_DB 连接抖动，watcher 的
  范围解析报 `receiving from an empty and closed channel`，整批文件事件被丢弃且无重试）：
  - 文件事件路径的 MDB 范围解析改走进程内单槽缓存（`UpdateScope::resolve_cached`）。
    名单只在 SYS meta 批次落库时才变，那一刻与 `SCOPE_DIRTY` 同点显式失效；TTL 兜底
    `AIOS_SCOPE_CACHE_SECS`（默认 300s，0 关闭）。SUL_DB 瞬时不可用时暖缓存放行并告警，
    冷缓存与配置错误（mdb_name 没填 / MDB 名不存在）维持 fail-closed 上抛。
    启动重扫、重挂补扫、周期对账与手动路径仍每次真查（fresh），它们就是缓存的刷新点。
  - watcher 事件循环新增周期对账重扫（`AIOS_WATCH_RECONCILE_SECS`，默认 300s，0 关闭）：
    按间隔整面重比「文件最新会话号 vs applied 水位」，把连接抖动、服务重启等一切来源
    丢掉的文件事件在一个周期内追回；入队按水位判定天然幂等，与启动重扫共用
    `sweep_watch_dirs`。

- **issue #10 复现套件**（`src/data_interface/staging/issue10_add_node.rs`，仅测试编译）：
  用真实渲染与真实暂存窗口（`stage_parsed_window` → `register_staged_finalize` →
  `commit_registered_to`）在 mem 引擎上模拟 E3D「复制 BRAN 并 SAVEWORK」的连续增量，
  钉住三条路径——连续多个窗口写回后新增节点必须出现在模型树（含父成员序边重建）；
  窗口因生成重试耗尽被阻断时「检测得到、树不动、水位原地」，吸收重置后重算收敛；
  journal 被持久层确定性拒绝（坏版 `update_dbnum_event` 对字符串 id 的 pe 行报
  `array::at` 类型错）时写回整体回滚、零半写，排毒后同一份 journal 重放收敛。
- **批次执行的阶段日志**（issue #12）：完成行补上 sesno 窗口与墙钟完成时间，在 E3D 里
  SAVEWORK 的人可以直接对上「屏幕上这批日志是不是我刚才那次保存触发的」；模型计划按
  action 分组计数（形如 `regen_root=3 transform=12`）而不再只报总数；交付单元、批量
  重生成、房间归属重算各自报耗时与成败；生成根列表超过 8 个截断并报出总量。

### 变更

- `DbOption.toml`：`manual_db_nums` 由 `[7998, 8000]` 放宽到 `[7997, 7998, 7999, 8000]`，
  纳入 issue #10 的 E3D 实测窗口（基线库 `.surreal/ams-7997-e3d-test-20260805`，7997
  applied=92）。取证结论就地记在配置注释里：issue 截图中的 `/1WCC0211` 属于 7999，而
  7999 一直被排除在手动窗口外（applied=3、file=41），库里的树是旧全量同步的残留。
  实测跑完可收窄回 `[7998, 8000]`。
- 房间轮次日志不再只在「队列跑空」那一轮打印，距上轮超过保底间隔触发的那轮同样报出
  目标数与死信数。

### 内部

- `attempts::record_window_block_at_on` 提升为 `pub(crate)`，`StagedFinalize` 增加
  `Debug`，供复现套件构造阻断现场。
- `src/bin/manual_scan_probe.rs`、`src/test/mod.rs` 仅 `cargo fmt` 排版整理。

---

## 历史记录

1、添加自动增量更新文件的修改，启动时会检查当前数据库和E3d数据库的一致性
## 2026-08-14

### 新增

- Python 解析层新增 `aios_db.parse.net_window(path, start, end, detail=False)`：复用生产
  `net_window::collect_net_window`，只读 dabacon 文件即可得到属性语义上的净增删改，
  并透出 `unchanged_rewrites` / warnings。与原 `parse.net_changes`（索引记录位置触达
  三态）分工明确：E3D TTY 的 apply + restore 合并窗口会过滤已恢复的业务属性，
  同时如实保留 E3D 自增的 `CACHID` 等保存期元数据，不需要反查 SurrealDB。
- 新增 `scripts/e3d/Test-TtyNetWindow.ps1`：自动执行 FTUB apply / 解析器断言 /
  restore / 合并窗口断言，`finally` 保证恢复腿执行，并产出 baseline 副本、语义 diff、
  命令退出状态与 rollback 验证记录。
## 2026-08-14

### 新增

- 定义严格的 SYS/DICT → CATA → DESI → 模型 → 房间初始化阶段与跨项目元件库优先级契约。

### 修复

- 初始化模型生成将受数据就绪门控，避免 DESI 或模型越过尚未完成的元数据、元件库阶段。
