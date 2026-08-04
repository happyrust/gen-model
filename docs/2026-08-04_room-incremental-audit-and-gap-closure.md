# 2026-08-04 房间增量更新：现状审核与三个遗留缺口的关闭

一句话：对 ADR-010 体系做了一轮「声明 vs HEAD 代码」的一致性审核（关键不变量逐一锚定、
残留清单逐项核对、运行库取证），确认无漂移；随后把审核建议的三个可修项全部落地并
live 验证（`d485baad` + `d62f1497`）。承接同日水位播种工作
（`2026-08-04_watermark-seed-audit-and-hardening.md`）。

## 1. 审核结论（对当时 HEAD，含 `eb59bfd5` 的六项收口）

关键不变量逐一锚定成立：

- 队列骨架：`RoomRecalcElement/Panel` 两个 action、行 id 不带 dbnum、`drain_rooms`
  第三阶段、房间收敛统一在 worker 空闲轮 `room_round`（ADR-011 合流后的归宿）。
- AABB 差异触发的两个入队点都在（`increment_manager` 的 TransformOnly 路径、
  `occ_generate` 的定向重生成路径），变更判定基线取空间树旧值。
- 吸收封闭性检查（`absorption_is_closed`）在位；两条重算分支共享判定谓词
  `element_in_panel`、共享边 id、都是先清后写——「增量==全量」的收敛保证结构性成立。
- 脏标记落盘（`AABB_TREE_DIRTY` + 空闲轮收尾）、失败保脏、临时文件 + 原子 rename。
- `eb59bfd5` 的六项修复与八条回归测试全部在位（B2 泳道 detail 收敛后覆写、C2 收口
  失败不误标 failed、A2 死信 HTTP 复活出口、C1 durable pending 先行等）。

残留清单与记载一致（审核时点）：D12 非几何触发未实现、`accel_tree.bin` 仍是 cwd
裸文件（当日演练日志实证了换目录静默空树）、D11 surql hd/hh 覆盖错位（演练日志再次
实证加载顺序）、两个已记载后续项（`gen_inst_meshes` 指针顺序、深层子树缓存失效）、
跨仓项（A3 / B1 / B3 / B4）。

运行库取证（site-8000-incrtest）：`room_relate` / `room_panel_relate` / pending 房间
任务均为 0——该库只生成过管路，结构库 PANE 从未生成，房间链路处于「空转无害」状态，
与 ADR 记载的验收卡点一致。

## 2. 三项修复（`d485baad`，3 文件 +367/-4）

### D12：非几何房间结构变更的触发规则（业务可见的最大缺口）

计划层（`model_update_plan.rs`）新增：

- `collect_room_structural_triggers`（纯函数）：FRMW/SBFR 的 NAME 变更且**新旧任一**
  名字命中 `room_key_word`（改进房间与改出房间都要重算）→ 房间触发；PANE 的 OWNER
  变更（复用 ADR-009 的 `owner_change`）→ 搬迁触发。只看 Modified：新建走 AABB
  差异链路，删除走 DeleteCleanup 清边。关键字未配置时不触发（防一次结构库批量改名
  引发整间重算风暴）。
- `panels_under_rooms`：改名房间名下全部 PANE，子 + 孙两层（FRMW → CWALL/CFLOOR →
  PANE，与归属计算层级覆盖同口径）。
- 接线在 `build_model_update_plan` 尾部：触发目标去重后追加 `RoomRecalcPanel`
  工作项；面板枚举失败降级为告警——房间归属是可事后重建的派生数据，下一次启动的
  全量重建仍是兜底，不能掐断数据窗口。
- 两个纯函数测试：改进/改出/无关改名/搬迁/普通属性五种形态 + 无关键字静默。

### accel_tree.bin 项目化（ADR-010 §6「路径带项目名」落地）

rs-core 硬编码裸文件名且反向索引重建方法私有，本仓无法让它读写别的路径，采用
**搬运语义**（`aabb_tree.rs`）：

- 加载前 `stage_project_aabb_tree_file()`：`accel_tree_{project}.bin` 存在则复制到
  裸名（覆盖别的项目残留）；只有裸文件则首次迁移沿用；两处加载点（run_app /
  run_cli）都已接。
- 落盘成功后 `archive_project_aabb_tree_file()` 归档回项目名；**归档失败上抛**并由
  脏位驱动下轮连同序列化一起重试——吞掉的话旧项目文件会在下次启动盖掉新树。
- 已知限制：多项目**并发**共用同一 cwd 时裸文件仍是竞态窗口（rs-core 硬编码之下
  无解）；先后切换项目的实际部署形态已闭环。

### D11：surql hd/hh 无条件按文件名覆盖

`run_cli` 在 `define_common_functions` 之后按 `project_hd` feature **重放**
`fn_query_room_code.surql`，把 `_hh` 文件对同名 `fn::room_code` 的覆盖矫正回来
（加载顺序在 rs-core 里改不到）；`project_hh` 构建无需处理（hh 本来就最后加载）。
成功 / 失败均有启动日志。

## 3. live 验证（`d62f1497`，一次性内存实例实跑）

新增 `live_room_structural_triggers_enqueue_panel_recalc`：

- 房间改名（旧名 `/1RX-RM03-R301` 命中全局关键字 `-RM`，与夹具实际 NAME 解耦——
  验证的正是「改出房间也要重算」）→ `build_model_update_plan` 经 `panels_under_rooms`
  的真库子 + 孙查询为两块面板排出 `RoomRecalcPanel`；
- PANE 搬迁（OWNER 变更）→ 自身排出；
- 改名计划纯净：不混入任何几何重生成工作项。

验证中修复一个夹具缺口：夹具 pe 行缺 `name` 字段，计划层（`resolve_unit_rollup`）
加载 OWNER 图时以「expected a string, found None」整批失败——房间路径自身不读它，
D12 用例是第一个把夹具窗口喂给计划层的（与夹具注释记载的 `pe.owner` / `generic`
非 Option 坑同构）。补齐后回归复跑 `live_room_fixture_parity` 与
`live_room_incremental_parity` 均通过。

消费路径（整间分支、先清后写、吸收封闭性、对拍）由既有五条 live 用例覆盖，未重复。

## 4. 遗留（均已有记载，本轮未动）

- `gen_inst_meshes` 的 `inst_geo.aabb` 指针写入顺序（与 D9 同构）；
- `QUERY_DEEP_CHILDREN_REFNOS` 深层后代变更时高层根快照陈旧（正确修法在计划层
  按根失效）；
- 跨仓项：A3 ensure 超时语义三处不一致，B1/B3/B4 客户端接线（plant-ui /
  rs-plant3-d）；
- rs-core 的 array-id 版 `define_dbnum_event` 待在该仓删除（本仓已有读回自证兜底，
  见水位存档报告）。
