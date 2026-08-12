# Python 绑定后续完善计划（CI 首跑、协同跟进与收尾）

- 状态：待评审（2026-08-12）
- 前置：`docs/plans/2026-08-11-python-binding-api-plan.md`（V1，M1–M4 已落地）、
  `docs/plans/2026-08-11-python-binding-next-steps-plan.md`（P0-1/P0-3/P1-2/P1-3/
  P2-2/P2-3 已落地，P0-2 已决策不修，P2-1 遗留）、本日 M6–M9 批次
  （`d75d9820` 测试轨 + CI、`62aa1490` /health 端点 + 探测精确化）
- 协同：`docs/plans/2026-08-12-spatial-tree-consistency-closure-plan.md`
  （空间树一致性闭环，在途；其 §6 消费者门禁与「Python spatial.* 纳入
  SPATIAL_STATE_SERIAL」直接牵动绑定）

## 1. 现状快照（2026-08-12 13:45 实测）

- 两条 pytest 轨全绿：离线档 60 条（3.4s）+ 房间增量/连接层档 14 条，合计 74 条
  （release 扩展 12.3s）；绑定 crate 另有 8 条 Rust 单元测试
  （`cargo test -p aios-py --lib`，探测判定纯函数）。
- CI 的 `python-bindings` job 已写进 `windows-tests.yml`，**但从未在 GitHub
  Actions 上真正跑过**——M8 的验收（「Actions 上绿一次 + wheel 可下载」）还差
  这最后一步。
- `full_init` 互踩探测已能按「同库」判（`/health` 的 `sul_db.endpoint`），
  对本机 9099（0.1.13 老包）走保守判分支已活体验证；
  `python/tests/conftest.py` 的 `force=True` 因此暂留。
- 遗留唯一未做项：长任务取消点（原 P2-1，历次都排最低）。

## 2. 工作项

### W1 CI 首跑闭环（0.5 天，含看护；**需要推送，见决策点 D1**）

`python-bindings` job 的每一步都只在本机等价物上验证过（YAML 结构、离线档
秒级绿、OCCT provisioning 抄自已在跑的 `windows-binary.yml`），但 CI 首跑
必然暴露本机看不见的东西：冷缓存下 debug OCC 的编译时长、maturin 在 runner
上的 Python 发现、DLL 搜索路径经 `GITHUB_PATH` 的传递、缓存键与
`db8000-model-increment` job 的并发争用。

- [ ] 推送当前分支（或 cherry-pick 绑定线提交到干净分支）后 `workflow_dispatch`
      触发一次；推送前按仓库纪律用 `Toggle-LocalDeps.ps1` 关回 vendor patch，
      过 pre-push 守卫。
- [ ] 看护首跑：记录各步耗时（尤其 maturin build 冷缓存），失败就地修；
      预算内（<120 min）则维持 debug wheel，超预算再议（方向：拆独立 workflow
      降低触发频率，或 cache 预热 job）。
- [ ] 绿了之后下载 wheel artifact，在一台没有 Rust 工具链的机器（或干净 venv）
      上 `pip install` + `import aios_db` + `parse.header` 冒烟——wheel 的
      「给同事用」承诺只有这样才算验过。
- 验收：Actions 绿一次；wheel 在非开发机可用；首跑耗时记录回填本文档。

### W2 版本护栏自锁（0.25 天，无依赖，建议最先做）

`aios_client.EXPECTED_SERVER_VERSION`（现 `"0.1.18"`）是手抄常量——下次
`chore(release)` 把 `Cargo.toml` 升到 0.1.19 时没有任何东西会提醒同步它，
护栏自己先漂移。而 `/health` 端点字段恰恰是 0.1.19 起才有，这个对表马上就有
实际意义。

- [ ] 离线档新增用例（`test_client_offline.py` 或独立文件）：解析仓库根
      `Cargo.toml` 的 `package.version`，断言与 `EXPECTED_SERVER_VERSION`
      一致。release bump 忘改常量时 CI 直接红，红的地方就写着修法。
- [ ] 顺手在 `aios_client.py` 的常量注释里写明这条钉在哪。
- 验收：故意把常量改错能红（防伪）；改回绿。

### W3 空间树闭环落地后的绑定跟进（0.5–1 天，事件驱动；**等对方工作流提交**）

一致性闭环方案有三处直接打到绑定身上，落地时若不跟进，绑定测试会以难排查的
方式碎掉。落地信号 = 该工作流的提交出现在 `git log`（现约 +1650 行在工作区）。

- [ ] **conftest 的树产物搬挪清单**：快照介质从 `accel_tree_{project}.bin` +
      `.meta.json` 换成单文件 `accel_tree_{project}.snapshot`。
      `python/tests/conftest.py` 的 `TREE_ARTIFACTS` 只列了旧两件——不补的话，
      测试对着内存库写出的 `.snapshot` 会**顶掉真项目的快照**（旧文件对不上
      指纹只是重建，代价小；但污染是真的）。三个文件名都列上，新旧过渡期兼容。
- [ ] **消费者门禁下的夹具路径**：房间消费者只在 Ready/ReadyEmpty 放行，Rust
      测试靠 `mark_spatial_tree_fixture_preloaded()` 显式声明夹具装载。Python
      的 `fixture.create` 不走那条路——若房间档在门禁下被拒（`SPATIAL_TREE_NOT_READY`），
      把该标记通过 `aios_db.fixture`（或 `full_init` 后自动）暴露出来。
- [ ] **重建绑定 + 全套复跑**：`spatial.persist/rebuild/reconcile` 纳入
      `SPATIAL_STATE_SERIAL` 后行为可能变（阻塞点、返回时机）；
      `spawn_spatial_revalidator` 进 `full_init` 后确认 pytest 进程能正常退出
      （常驻后台任务随 runtime 走，理论无碍，跑一次为证）。
- [ ] **`tree_status` 收紧**：`test_connection_layer.py` 的稳定核子集断言升级为
      新契约全集（十五键或其时形状），`spatial.pyi` 与 `exec_api.rs` 文档同步。
- 验收：对方提交合入后 74+ 条全绿；仓库根无测试残留快照；形状断言与 /health
  新契约一致。

### W4 冒烟脚本退役标注（0.25 天，无依赖）

`scripts/smoke_m1..m5.py` 全部 `set_config(仓库根 DbOption)` + 硬编码
`D:/AVEVA/...` 与 8009——正式库已决策不修，这五个脚本现在**跑必失败**，但它们
是 M1–M5 的验收记录，不该删。

- [ ] 每个脚本头部加三行注释：历史验收基线（对应里程碑与当时环境）、8009 已
      退役不可复跑、等价物在哪（`pytest -m offline` / `-m "not offline"` /
      `testbed/run_full_loop.py`）。
- [ ] `python/README.md` 脚本表把 smoke 行合并成一行「历史验收（不可复跑，
      见头注）」。
- 验收：新人照 README 不会去跑 smoke 撞墙。

### W5 sandbox 真数据 pytest 档（1 天，**可选，见决策点 D2**）

现在的三层测试：离线（CI）、房间增量（内存库合成夹具）、真数据全链路
（`testbed/run_full_loop.py`，8019，手跑脚本）。第三层不是 pytest：没有
marker、没有 skip 语义、断言失败不进报表。

- [ ] `run_full_loop.py` 的链路改造成 `-m sandbox` 档：8019 不可达 / 项目副本
      缺失时 skip 并指路 testbed README；基线在位则复用（分钟级的首跑基线
      不塞进默认路径）。
- [ ] 与房间档的进程级约束对齐：sandbox 档要 `DbOption-pytest`，与
      roomtest / ci 配置互斥——沿用 conftest 的「按选中集合裁定配置」机制，
      三份配置三选一，混选时最重的赢并 skip 其余（规则写进 conftest 文档串）。
- 验收：`pytest -m sandbox` 在沙箱就绪的机器上全绿；三档混选不炸、skip 有理由。

### W6 release/debug 真实生成基线（0.5 天，可选）

README 现在如实写着「OCC 布尔运算的 debug/release 差距要跑真实生成才看得出来」
——数字空着。补一次一手数据，后续性能议题（`todo.md` 的生成效率优化）有基线可引。

- [ ] 沙箱 8019 上对同一构件集（如 7997 的 `/-RX-CUP-001FA` + 一个 dbnum 级
      样本）分别用 debug / release 扩展跑 `model.ensure(force=True)` 与
      `model.gen`，三轮取中位。
- [ ] 数字回填 `python/README.md` 构建小节（替换「几乎一样」那句的悬置部分）。
- 验收：README 有可复现的测法与数字。

### W7 部署升级与 force=True 撤除（0.25 天 + 运维动作，**见决策点 D3**）

- [ ] 9099 的部署包升级到带 `sul_db.endpoint` 的构建（0.1.19 起；升级本身是
      运维动作，不在本仓库）。
- [ ] 升级后：删掉 `python/tests/conftest.py` 的 `force=True` 与那段注释，
      复跑房间档验证探测自然放行（这是对「同名不同库放行」分支的免费活体验证）。
- 验收：不带 force 的 `full_init` 在沙箱直接通过；74 条仍全绿。

### W8 长任务取消点（1–2 天，遗留 P2-1，仍排最后；**见决策点 D4**）

`gen_dbnum` / `sync.baseline` / `room.drain` 期间 Ctrl+C 不能中断 Rust 侧，
只能等当前调用返回——README 已写明代价，三轮计划都把它排最低，事实证明不做
也没堵住谁。保留在册但不进本轮默认范围。

- 若做，形态定为：绑定层持进程级 `AtomicBool`；长调用在 `py.detach` 内改为
  「spawn 到 runtime + 每 ~100ms attach 回来 `Python::check_signals()`」的轮询
  等待，捕获 KeyboardInterrupt 置位；主 crate 在三个入口的 refno 边界加检查点
  （幂等收口不变，半途返回不留半写状态——各入口本就按根提交）。
- 验收：整库生成中 Ctrl+C 在下一个 refno 边界内返回；库内无半写；再次调用可续。

## 3. 顺序与工作量

| 批次 | 内容 | 预估 | 触发条件 |
|---|---|---|---|
| 即刻 | W2 版本自锁 + W4 冒烟退役标注 | 0.5 天 | 无 |
| 决策后 | W1 CI 首跑闭环 | 0.5 天 | D1（推送）拍板 |
| 事件驱动 | W3 空间树跟进 | 0.5–1 天 | 闭环工作流提交合入 |
| 决策后 | W5 sandbox 档 / W6 性能基线 | 1 + 0.5 天 | D2 拍板 |
| 运维后 | W7 撤 force | 0.25 天 | D3（部署升级）完成 |
| 挂账 | W8 取消点 | 1–2 天 | D4 拍板才排期 |

## 4. 决策点（需拍板）

- **D1**：CI 首跑需要推送。当前分支 `codex/increment-staging-closure` 上还压着
  多条工作流的提交——是整分支推，还是把绑定线提交 cherry-pick 到干净分支先行？
  （推荐后者仅当分支近期不打算推；若分支本来就要推，整推 + dispatch 最省事。）
- **D2**：W5（sandbox pytest 档）与 W6（性能基线）做不做。推荐 W6 做（半天换
  一手数据），W5 视 run_full_loop 的手跑频率——每周都跑就值得转正。
- **D3**：9099 部署包何时升级（运维侧）。
- **D4**：W8 取消点是否终于排期，或从册子上划掉（写进 README 已知代价一栏了结）。

## 5. 非目标

- ams-8009 正式库恢复（已决策不修，testbed/内存沙箱替代）。
- wheel 对外发布 / PyPI（CI artifact 即止）。
- 空间树一致性闭环本体（另一条工作流；本计划只做其落地后的绑定侧跟进 W3）。
- `todo.md` 的主 crate 项（生成效率优化本体、全文检索等）——W6 只出基线数字，
  不动生成代码。

## 6. 风险

| 风险 | 对策 |
|---|---|
| CI 首跑 debug OCC 冷编译超时 | 缓存键独立 + run_id 断点续存已就位；超时再拆 workflow 或降触发频率 |
| W3 时机错过：闭环合入后没人复跑绑定套件 | 本文档挂在闭环计划的「协同」里互相引用；conftest 的 TREE_ARTIFACTS 缺项是最先炸的信号 |
| W5 三配置互斥规则复杂化 conftest | 规则只加不改：沿用现有「按选中集合裁定」，三选一 + skip，超出即砍掉 W5 |
| 版本自锁测试在 release bump PR 上红 | 这是设计目标不是事故；红处文案直接写「同步 aios_client.EXPECTED_SERVER_VERSION」 |
