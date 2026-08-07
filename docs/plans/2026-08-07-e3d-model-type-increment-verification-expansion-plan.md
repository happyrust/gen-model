# E3D 增量模型类型验证 · 扩展规划（2026-08-07）

> 依据：`scripts/e3d/ams_model_type_cases.json` 现状（41 注册 / 33 verified / 8 pending，
> 本文写作时另一会话仍在逐例推进，数字以 manifest 为准）+ 本轮对 AMS 库的**只读**盘点
> （pe 按 noun×页分布、inst_relate 按页分布、页→dbnum 映射）。
> 关联：`docs/2026-08-06_e3d-l3-automation-test-plan.md`（l3_suite 变更形态场景表）、
> ADR-019（E3D TTY 无人值守通道）、ADR-010（房间归属增量）、ADR-018（金基线成对恢复）、
> `docs/plans/2026-08-07-staged-transform-write-routing-fix-plan.md`（P0：暂存 Transform 写路由）。
> 本文只写「验什么、按什么顺序、怎么算验完」。

## 0. 现行方式（不变，作为所有新增用例的骨架）

`Run-RoomE3DE2E.ps1 -ModelTypes <noun>`：manifest 取 refno → 生成 apply/restore 宏对
（默认 `BY U 10` / `BY D 10`，可用 `select_command` / `selector` / `apply_command` /
`restore_command` 覆盖）→ `l3_suite --check-driver` 走 TTY 投宏 + SAVEWORK →
`issue7_e2e_increment` 只入队本库批次消化 → 断言：水位推进到文件最新会话、
AABB 变化（apply 后 ≠ before，restore 后回归）、`room_relate` 归属边按
`expect_room` / `apply_expected_edges` / `restore_expected_edges` 收敛、房间拓扑不漂移。
准备阶段 `ensure_model_generated(靶元素)` 按需生成模型（dynamic baseline），
所以**靶元素所在库不需要整库生成过**——7999 的 CAP 用例即先例。

覆盖判定：`Test-AmsModelTypeCoverage.ps1` 以「当前库 `inst_relate` 出现过的 noun」为全集。
**注意：这个全集是"已生成模型"的像，不是"可生成模型的类型"的像**——本轮盘点证实
大量可生成类型因所在库从未跑过生成而不在全集里（§2–§4）。全集会随生成扩大，
脚本的 missing 检查会逼着 manifest 同步登记，这个机制保持不动。

### 数据面盘点结论（2026-08-07，只读查询 ws://127.0.0.1:8009）

页→dbnum 映射与生成状态：

| 页前缀 | dbnum | 学科 | inst 数 | 生成状态 |
|---|---|---|---|---|
| 24381 | 7997 | 舾装主库 | 45653 | 已生成 |
| 24384 | 8000 | 管道/舾装 | 5293 | 已生成 |
| 24383 | 7999 | 管道（阀/法兰/垫片密集） | 2 | **未生成**（仅手工 ensure 的 CAP） |
| 15516 / 23708 | 7324 | 结构 | 476 + 1560 | 部分生成 |
| 17496 / 25688 | 1112 | 房间结构（PANE/CWALL/…） | 恰好 1000 PANE + 1000 NXTR | **疑似截断的部分生成** |
| 23710 | 7326 | 吊架 | 0 | 未生成 |
| 23711 | 7327 | 吊架 | 0 | 未生成 |

一个直接后果：房间 drain 报 **437 间在册房间的面板里 351 块没有可用几何**（全在 1112），
落在这些房间里的构件会被收敛成「不属于任何房间」——它压着所有 expect_room 用例
向 1112 房间区扩展的可信度（§6-1）。

## 1. W0 · 存量清账：8 个 pending

| noun | refno | dbnum | 现状与做法 |
|---|---|---|---|
| ANCI | 24384_25727 | 8000 | 24384 页已有 137 条 ANCI inst，现骨架直接跑 |
| FIXING | 24384_25748 | 8000 | 同上（788 条），直接跑 |
| GENSEC | 24384_25743 | 8000 | 同上（398 条），直接跑；另有现成 `l3_gensec_add_*.mac` 新增宏留给 T4 |
| SCTN | 15516_102 | 7324 | 该 refno 本身就有 inst，直接跑 |
| SJOI | 23708_28532 | 7324 | 23708 页 SJOI inst 存在，直接跑 |
| FITT | 24384_19980 | 8000 | 已带 `selector /-RX-CCV-S2020-V1/F1`（refno 直选有问题的工作区），沿 selector 跑 |
| BRAN | 24384_23822 | 8000 | 挪 BRAN=挪全体子件+TUBI 重生成，语义上是属主级位移（§5），但 BRAN 自己有 inst（17 条），现骨架可以先跑出一个样本 |
| PANE | 24381_10004 | 7997 | **元素侧断言不适用**：PANE 是 `room_relate` 的 in 端（面板侧），不是 out 端（构件侧）；此前三个选择探针宏日志全空。需要 §6-2 的面板侧断言扩展后再验 |

WALL 已于本日 12:06 双腿跑绿并标记 verified（另一会话），不在此列。

## 2. T1 · 管道目录件 19 noun（最高性价比的新增覆盖）

**发现**：以下 noun 在 dict 里都认定为几何、走 piping/cata 生成路由
（`docs/plans/stage3-noun-routing-gaps.md` §3a 名单），AMS 里有大量真实元素，
但**全库零 inst**——不是生成管线缺口，而是它们密集所在的 7999 从未整库生成、
7997 只按到位单元生成过。它们是「可生成模型的类型」减去「已生成」的最大一块差集。

各 noun 在已登记库的元素分布（只列 ≥1 的页；执行时按此选靶）：

| noun | 7997(24381) | 7999(24383) | 1112(17496/25688) | 备注 |
|---|---|---|---|---|
| VALV | 287 | 350 | 20 | 阀 |
| TEE | 255 | 205 | 12 | 三通 |
| FLAN | 22 | 394 | 15 | 法兰 |
| GASK | 9 | 190 | 5 | 垫片 |
| COUP | 257 | 51 | – | 接头 |
| OLET | 145 | 249 | 21 | 支管台 |
| WELD | 1094 | 185 | 64 | 焊点，可能无独立几何 |
| UNIO | 149 | – | – | 活接 |
| TAPE | 120 | 1 | – | 锥管 |
| TRNS | 43 | – | – | 变径过渡 |
| THRE | 16 | 1 | – | 螺纹件 |
| FLEX | 25 | – | – | 软管 |
| OFST | 66 | – | – | 偏置，可能无几何 |
| INST | 12 | 66 | – | 仪表 |
| BRCO | 156 | 2 | – | 分支连接 |
| CROS | 4 | – | – | 四通 |
| PCOM | 5 | 3 | – | 管道部件，可能无几何 |
| FBLI | 2 | 4 | – | 盲板 |
| SILE | –（17496 仅 1） | – | – | 消音器，样本极少 |

**执行配方（每 noun 一遍，可批量）**：

1. 选靶：`SELECT VALUE record::id(id) FROM pe WHERE noun='<N>' AND dbnum=<D> LIMIT 20`，
   优先 7999（房间区丰富、CAP 先例同库）；
2. 探针：对候选 `ensure_model_generated` → 查 `inst_relate.aabb != NONE`。
   **无几何是合法结论**（WELD/OFST/PCOM/INST 一类抽象件预期如此）：manifest 记
   `"coverage": "no_geometry"` + 证据路径，不硬造用例；
3. 定房间预期：对有几何的靶跑一次成员重算（或直接跑 apply 腿看 dynamic baseline），
   确定 `expect_room` 与 room/panel 字段；
4. 注册 manifest（`mode: relative_position`）→ 现骨架跑绿 → 标 verified。

优先级：VALV / TEE / FLAN / GASK / OLET / COUP 六大头先行，长尾随后。

### T1 六大头选靶结果（2026-08-07 只读盘点，配方第 1 步已做完）

选靶口径：全部取 dbnum 7999；VALV–OLET 取已验 CAP 同 ZONE `/1WCC-PIPE-RX`（六大头里唯独
COUP 在该 ZONE 只有 2 个，改取 `/1RCV-PIPE-RX`）；BRAN 名尾部编码了管路走向的舱室号
（如 `-R52-R710` ），优先挑指向**已验证有几何面板的房间**（R710/R312/R420）的支管，
预判 `expect_room=true` 概率最高；同 noun 三个候选尽量跨不同 PIPE；避开
`…-OLD-备用` ZONE 与 `Copy-of-*-OLD` 支管。组件全部无名，E3D 选择用 `=refno`
（与已验用例同口径，无需 selector）。

| noun | 候选 1（首选） | 候选 2 | 候选 3 |
|---|---|---|---|
| VALV | `24383_112`（/1WCC0198A-…-R52-R710/B2） | `24383_184`（/1WCC0207-…-R52-R710/B1） | `24383_1938`（/1WCC0304-…-R70-R312） |
| TEE | `24383_114`（/1WCC0198A-…-R52-R710/B2） | `24383_1073`（/1WCC0308A-…-R80-R522） | `24383_1035`（/1WCC0292-…-R80-R422/B2） |
| FLAN | `24383_101`（/1WCC0198A-…-R52-R710/B1） | `24383_103`（同支管备选） | `24383_1006`（/1WCC0092A-…-R80-R522） |
| GASK | `24383_102`（/1WCC0198A-…-R52-R710/B1） | `24383_1007`（/1WCC0092A-…-R80-R522） | `24383_1043`（/1WCC0292-…-R80-R422/B2） |
| OLET | `24383_110`（/1WCC0198A-…-R52-R710/B1） | `24383_1001`（/1WCC0092A-…-R80-R522） | `24383_1107`（/1WCC0080A-…-R80-R522） |
| COUP | `24383_74127`（/1RCV0115A-…-R31-R420） | `24383_74544`（/1RCV0214-…-R31-R420/B1） | `24383_74271`（/1RCV0196-…-R23-R341） |

注：R522/R422/R341 等房间的面板若在 1112（无几何），动态基线会把 expect_room 判成
false——这不是错误，是 §6-1 生成缺口的像；执行时以配方第 2–3 步（ensure 探针 +
成员重算）的结论为准回填 manifest。`/1WCC0198A` 一根支管同时覆盖 VALV/TEE/FLAN/GASK/OLET
五个 noun 的首选靶，探针阶段可一次 ensure 整根支管摊薄成本。

### T1 六大头探针结果（2026-08-07 执行，配方第 2–3 步已做完）

用 `tests/gen_one_root_probe.rs` 对 6 根支管 `ensure_model_generated`（7999 的
`inst_relate` 2 → 59 条）：**VALV / FLAN / COUP / OLET 有几何**（另顺带产出
INST / WELD，一并注册），**TEE / GASK 三根不同管线全部零产出，判定 no_geometry**。
manifest 已注册 6 条 pending + 2 条 no_geometry（各行带探针证据路径），
`Test-AmsModelTypeCoverage.ps1` 已认识 no_geometry 终态并修复 PS 5.1 BOM（§6-3 完成）。
剩余执行步骤：待 E3D 通道空闲，对 6 个 pending 逐个跑双腿（房间预期若与动态基线
不符，按探针实测回填）。

## 3. T2 · 结构边界变体（房间语义的同族补全）

CWALL（1112:1786、7997:64）、STWALL（1112:1436、7997:50）、CFLOOR（1112:331、7997:50）
——已验 GWALL/FLOOR/WALL 的同族，全库零 inst。房间边界件位移会改**其它构件**的归属，
GWALL 用例的 `apply_expected_edges` / `restore_expected_edges` 机制直接复用。

- 先用 7997 页的靶（避开 1112 生成缺口），`ensure_model_generated` 单点补齐；
- 此前 `cfloor.mac` 探针（`CE /1LR-WF05-F-AB-F002`）日志为空——选择没成。执行前先用
  `--check-driver` 单独排选择口径（名字是否在当前 MDB、要不要 refno 直选）；
- 1112 靶等 §6-1 补生成后再扩。

## 4. T3 · 吊架家族（把 noun 全集从 41 扩到 50+）

7326 / 7327 两库已解析入 pe（水位 615 / 1470）但零生成。HANG 本身是交付单元类型
（本仓 unit types = BRAN/HANG/SUPPO/EQUI），部件多为目录件：
HANG（3918+2749）、HROD（4750+3226）、HNUT（10870+8914）、HPIN、CLEV、EYRD、
VSPR、SLUG、RCPL、SCLA、REST。

1. 第一步：挑一个 HANG 根 `ensure_model_generated` → 挪整个 HANG（属主位移形态，§5）
   → 增量断言子树 inst 全部平移；
2. 第二步：部件级按 T1 配方逐 noun 补用例（HROD/CLEV/EYRD/VSPR 优先，几何直观）；
3. 这一步做完 `inst_relate` noun 全集自然扩大，coverage 脚本会开始要求 manifest
   登记这些新 noun——按脚本提示补行即可。

## 5. T4 · 变更形态维度（与 l3_suite 分工，不塞进 manifest）

manifest 通道只管「noun × 相对位移」。其它变更形态归 l3_suite 场景表
（08-06 计划 §2：M1 参数改 / M2 位移 / M3 删除 / F4 改名 DataOnly 已有，
F5 新增、F6 跨 BRAN 移动计划中未实现）。本轮建议**新增两类**：

| 新场景 | 内容 | 为什么现在做 |
|---|---|---|
| Owner 整体位移 | 挪 EQUI（7997 有 992 个）或 ZONE → 断言全子树 inst 的 `world_trans`/AABB 平移、房间归属跟随 | transform 便宜路径目前**零 E2E 覆盖**，而它正是 P0（`update_world_transforms` 写路由泄漏）的宿主路径。直连模式（`GEN_MODEL_DIRECT_INCREMENT=1`）现在就能验；**暂存模式的同型用例等 P0 W1 修复合入后加一轮**——P0 计划 §3 的"可选实机验证"即此，两个工作流在这里汇合 |
| ORI 旋转 | 对一个已验靶（如 BOX/DAMP）改 ORI | 位姿的另一半分量，现有用例只动 POS |

BRAN（§1）跑通后即覆盖「属主位移带 TUBI 重生成」的 manifest 侧样本；
删除/新增仍归 l3_suite M3/F5，不重复建设。

## 6. T5 · 基础设施补账

1. **1112 补全生成**：PANE/NXTR 恰好各 1000 条，像被截断的回填。整库重生成后
   解锁 351 块无几何面板 + T2 的 1112 靶材 + 1112 独有类型
   （POLYHE 86、HANDRA 61、RLADDR 17、RLCAGE 15、GRIL、MESH）。这是房间侧
   扩大验证面的前置条件；
2. **PANE 面板侧断言**（小改 `issue7_e2e_increment`）：挪 PANE 断言
   `room_relate` 的 in 边集合（它罩着的构件归属重算）+ `room_panel_relate`
   拓扑随几何更新，而不是套构件侧的 out 边断言；
3. **coverage 脚本 BOM 修复**：PS 5.1 下管道给 `surreal sql` 的查询带 UTF-8 BOM，
   解析直接失败（本轮实测复现）。改成 `[IO.File]::WriteAllText(..., ASCII)` 落临时文件
   再管道，或声明只支持 pwsh 7；
4. 7999 **不需要**整库生成：T1 按靶 ensure 即可（CAP 先例）。

## 7. 顺序与验收

| 轮 | 内容 | 出口判据 |
|---|---|---|
| R1 | W0 清账：ANCI/FIXING/GENSEC/SCTN/SJOI/FITT 六个直接跑 + BRAN 样本 | manifest pending 只剩 PANE；coverage 脚本无 missing/stale |
| R2 | T1 六大头（VALV/TEE/FLAN/GASK/OLET/COUP）探针+注册+跑，随后长尾 13 个 | 每 noun 要么 verified 要么 no_geometry+证据 |
| R3 | T4：EQUI 属主位移（直连模式）+ ORI 旋转（进 l3_suite 场景表） | l3_suite 新场景绿；P0 W1 合入后补暂存模式一轮 |
| R4 | T5-1（1112 补生成）→ T5-2（PANE 断言扩展）→ T2 + PANE | 437 面板几何缺口清零；PANE verified |
| R5 | T3 吊架：HANG 根 + 部件 noun | noun 全集扩大且 manifest 同步登记 |

统一纪律：每例跑绿立即回写 manifest `coverage` 字段（不回写视为没跑，沿 08-06 计划口径）；
apply/restore 必须成对（SAVEWORK 会话号不可逆，失败轮金基线兜底 ADR-018）；
E3D TTY 通道单机独占——**开跑前查 des.exe 残留**，与 l3_suite/其它会话互斥
（本文写作时即有另一会话在占用通道跑 W0，这是真实风险不是假设）。

## 8. 明确不做（本期）

- CATA 库自身变更（目录参数改动传播到设计件）的增量验证——`skip_cata` 语义另案；
- 暂存模式的属主位移 E2E——等 P0 W1 修复合入（避免拿已知坏路径刷绿）；
- plant-ui V 级四图证据——归 l3_suite（08-06 计划 §7），本计划只管库侧断言；
- 7323/7326/7327 之外未登记库的扩展、ZDJ 项目——超出 AMS 样本范围。
