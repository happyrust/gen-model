# plant-mbd 进展审核与下一步计划：plant-ui 右键查看尺寸标注

> 计划 ID：`2026-09-09-plant-ui-mbd-dimensions`（承接 `2026-09-06-plant-mbd-pipe-dimension`）
> 创建：2026-09-09
> 状态：**draft（待 plannotator 门禁）**
> 缘起（用户 2026-09-09）：审核 `old/vendor/plant-mbd` 现在的进展、提出下一步开发计划；**第一目标：在 `old/plant-ui` 上通过管道右键「查看尺寸标注」，绘制自动尺寸标注线**。
> 本轮核实手段：读 plant-mbd 十笔提交 + 未提交 diff、跑 `cargo test --workspace`（见 §1.2 实测结果）、读上一计划 task_plan/findings、勘察 plant-ui 菜单 / 视口 / HTTP 面（文件行号见 §2）。

## 1. 进展审核（截至 `b6f6583` + 工作树在途改动）

### 1.1 已落地（十笔提交，Phase 0–4 全过，Phase 5 过大半）

| 提交 | 内容 | 对应 |
|---|---|---|
| `8055f47` | 建仓：自 rs-mbd `b6b1027` 迁入纯求解器与全部 fixtures，去 `aios_core` | Phase 0 |
| `d5f09ec` | 求解器几何全程 f64（契约边界仍 `[f32;3]`） | D5 |
| `ec7e880` | Tail 取 `tpos`、首批六条真实金样（E3D `.dat` 直读） | Phase 3 |
| `287b1f0` | 线性尺寸按 PML `isoDim` 模型重做（构件原点投影 running dimension、od/cheight 行位、选向系统接线），`layout_mode = isodim_main` | Phase 4 / ADR-0006 |
| `2d7ca54` | ATTA 子尺寸（`getAttaDims`），主尺寸按 `twoormoredims` 升第 2 行 | Phase 5① |
| `da4868e` | 坡度符号（`isoSlope`）+ 斜管 XYZ 分解（`dimslope`） | Phase 5② / ADR-0007 |
| `43c9882` | 焊缝标记（显式 WELD → `weld_mark`，编号有才出 label） | Phase 5③ / ADR-0008 |
| `dc0eec7` | field-dictionary 补 `dtxr` / `weld_number` | 文档 |
| `844ce0e` | 位号 / 信息标签（`isoTag` 系，label + leader_line，不建 Polar） | Phase 5④ / ADR-0009 |
| `b6f6583` | `angle_dim / aid_arc / aid_circle` 契约补 PML AIDARC 弧几何，与前端 ADR 0055 同版 | Phase 5⑤② / ADR-0011 |

配套（另两仓，已验证过）：gen-model 适配层 `src/mbd/`（Route → `BranchQueryResult`，交叉核对位置最差 0.0009 mm）与端点 `GET /api/mbd/v2/pipe/{refno}`（axum 回环测试 200/404/422/400 全过，wire 体经前端 `parseMbdV2PipeData` ACCEPTED）；plant3d-web `40dd2e5` 契约扩展（ADR 0055，259 用例）。金样 6 条 + `golden record/diff` 机制在用。

### 1.2 在途（未提交，**当前是红的**）

`crates/plant-mbd/src/isodim.rs` +109/−2：**ADR-0012「管体之外的尺寸点」守卫**——PML 对 Head/Tail 不做偏轴检查、构件只在水平直段上查，现场库 TPOS 离最后一个弯头出口 1.3 m 拐 47° 的 BRAN 会画出折线的压缩投影。缘起 2026-09-08 全库扫查（AvevaMarineSample 2431 条 BRAN：412 条尺寸两端横跨袋轴 ≥ 10 mm，其中 407 条在 Head/Tail）。实现：`trim_off_body_points`（取点后删横跨袋轴超 `od·2/3` 的点，记 `linear.point_off_axis`）+ 出图时 `linear.segment_spans_kink` 兜平行错开管段。

**本轮实测** `cargo test --workspace`：**85 通过 / 4 失败**——

- `isodim::tests::segments_spanning_an_off_axis_tube_are_suppressed`（新用例自身：期望留给 `segment_spans_kink` 的两点被 `point_off_axis` 先删了）
- `layout::tests::bent_bag_is_suppressed_straight_bag_is_dimensioned`（期望 1 条 Head→Tail，得 0）
- `layout::tests::guard_boundary_sits_on_the_main_split_angle`（同上）
- `layout::tests::toy_fixtures_never_trip_the_off_axis_guard`（`toy-bridge-and-stick` 误触 `linear.point_off_axis`）

即：守卫对既有 toy 语义**过宽**，「只剩两点」「与唯一邻点错开」的端点处置和 `segment_spans_kink` 的分工还没收敛。另两笔账：**ADR-0012 文档未写**（`docs/adr/` 现状 0009 → 0011，**0010 也空着**，全库无一处引用）；**金样未按守卫重录**（守卫会动 field 金样，改完必须 `golden diff` 人工审阅）。

### 1.3 挂账（既定计划内未完成）

- Phase 2 浏览器目测（plant3d-web）：现网 gen-model 是**旧构建（无 MBD 端点）**，要起新构建——用户终端操作。
- Phase 3/4 PML 图纸人工对照：需要 E3D 环境，一直 pending。
- Phase 5：⑤③ `angle.rs`（依赖 ⑥′）；⑥′ polar-lite **待拍板 A/B/C**（findings「Phase 5 ⑥」评估已做，建议抽 `occupancy.rs` 600–900 行）；⑦ 调整焊；⑧ PCOM / tee-olet 切段。
- Phase 6 表格 JSON 等后置面未动（本来就另立计划）。

### 1.4 审核结论

内核扎实：契约冻结 + 守卫、金样回归、ADR 纪律、f64 精度口径都立住了；两天里 Phase 4 + Phase 5①–④ 的推进速度和留痕质量都好。当前的债集中在**在途的 ADR-0012（红）**与两个文档缺口（ADR-0012 正文、0010 空号）。交付面上，链路只通到 plant3d-web（网页）；用户新定的第一目标是 **plant-ui（Bevy 桌面壳）**，该仓目前**没有任何 MBD 接线**（全仓检索无命中）——这是下一步的主战场。

## 2. 新目标勘察：plant-ui 现状（2026-09-09 核实）

- **右键菜单**：模型树与三维视口**共用** `element_menu`（`crates/plant-ui/src/workbench/tree.rs` L266，「同一件事的两个入口，内容必须一致，所以收在一处」），已有 显示/隐藏/定位/房间归属/复制 REFNO 先例；动作经 `Cmd` 上行宿主（plant-ui-app），动作自带 refno、不回读选择集。
- **视口**（`plant-ui-view3d`）：`pick(uv) → take_picked() -> Option<RefU64>`、`set_selection`、`bounds` 都有；**世界单位毫米**（注释明说「模型数据单位为毫米」）；`LineList` 线网格两处先例（地面网格 L1097、无效 TUBI 虚线 L1268/L1519）；**文字走 egui 覆盖层投影有先例**（`axis_labels`：轴端标签投影成纹理 UV，egui 侧画字）。
- **HTTP**：workspace 统一 `ureq 3.3`（`Cargo.toml` L62）；「模型服务」基址即 gen-model 服务（默认 `http://127.0.0.1:8022`，`read_face` 已在用）——**MBD 端点就在同一服务同一端口**，不需要新连接配置。
- **契约类型**：plant-mbd 是纯库（无 e3d-io / DB / HTTP 依赖），plant-ui 可直接 path 依赖拿 `MbdV2PipeData`（契约边界是 `[f32;3]` 数组，与 Bevy 的 glam 版本无碰撞面）。

结论：四个接缝（菜单、取数、画线、画字）在 plant-ui 里都有现成先例，第一目标不需要动契约、不需要动求解器、gen-model 侧零改动（端点已在）。

## 3. 下一步开发计划

### Phase A — 收尾 ADR-0012，工作树转绿（plant-mbd，≈0.5–1 人日）

- [x] 修 4 条红：toy 语义不回退（`toy-bridge-and-stick` 等不得触发守卫）；「只剩两点」「端点与唯一邻点错开」的删点口径与 `segment_spans_kink` 的分工按 diff 注释意图收敛（平行错开管段留给出图抑制，不在取点期删）。——判据改成「断口」（相邻成员端口不相接 = E3D 没画管），plant-mbd `1a741b2`；后续 `b6b8c4d` 排除 ATTA 穿过件的幽灵断口。
- [x] 写 `docs/adr/0012-…`（阈值 `od·2/3` 的出处、与 PML 行号对照、2026-09-08 扫查数据）；顺手处置 **0010 空号**（补记实际决策或留占位声明，不许悬着）。——同 `1a741b2`。
- [x] `golden diff` 全跑：受影响 field 金样重录，`fixtures/golden/README.md` 逐条写为什么变。——三条 Tail 隔断口删点重录，三条逐字节不变；`f888a50` 箭头随字高缩放又重录六条（仅 arrow_lines）。
- 验收：`cargo test --workspace` 全绿、`golden diff` 6/6 OK、单笔提交（feat(isodim): ADR-0012）。——2026-09-09 达成（96/96，golden 6/6）。

### Phase B — plant-ui 右键「查看尺寸标注」（**第一目标**，≈3–4 人日）

- [x] **B1 契约与客户端**（0.5）：plant-ui-app 加 path 依赖 `plant-mbd`（只用 contract 类型）；新模块 `mbd_api`：`GET {模型服务 base}/api/mbd/v2/pipe/{w0_w1}`（ureq、后台线程、超时与取消跟既有取数同口径）；404 / 422（带 issues）/ 网络失败 / 解析失败分类成可展示错误。——plant-ui `bdc985b54`（2026-09-09）：`mbd_api::{pipe_dimensions, MbdError}`（ehttp——仓里 HTTP 已统一到 ehttp 双端，ureq 是它的原生后端；30 s 超时）+ `data::Req::PipeDimensions{epoch, refno}` → `Evt::PipeDimensions`（数据线程取数、宿主 `dimensions_epoch` 认帧丢旧结果，不走读面）；裸 404 单列 `endpoint_missing`（R1）、`geometry_space ≠ source_mm` 边界拒收；`cargo test -p plant-ui-app` 150/150。B2 接上发送点后删 `Req` 变体上的 `allow(dead_code)`。
- [ ] **B2 菜单与动作**（0.5）：`element_menu` 增「查看尺寸标注」——落点是 BRAN 直接用；落点是 BRAN 成员（ELBO / ATTA…）向上归属到所在 BRAN；其余元素不出现该项。再点一次（或菜单变「隐藏尺寸标注」）清除。新 `Cmd` 带 refno 上行宿主。
- [ ] **B3 视口尺寸标注层**（1.5–2）：`View3d` 新命令 `SetDimensions(refno, batch)` / `ClearDimensions`。几何映射：`linear_dim`（尺寸线 + 延长线 + 箭头小线段）、`aid_line / leader_line / slope_mark / weld_mark` → 世界毫米系 `LineList`（直接用 `source_mm` 几何，**不吃** `source_to_design`——那是网页设计系的事）；`aid_arc / aid_circle / angle_dim` 弧按 ADR-0011 框架采样成折线；文字（尺寸数值 / label / `aid_text`）→ 锚点投影 egui 覆盖层（`axis_labels` 同机制），首版不做遮挡剔除与避让。换场景 / 卸载模型 / 切项目时清层。
- [ ] **B4 状态与错误呈现**（0.5）：加载中 / 失败 toast；`meta.layout_mode` 与 issues 计数先给轻量入口（状态条或 tooltip），完整 issues 面板后置。
- [ ] **B5 验收**（0.5）：六条金样 BRAN（`24383_99996` 等）实机右键出线，尺寸数值与当前金样一致；开关、切项目清层、服务不可达报错不崩、非 BRAN 无菜单项各走一遍；plant-ui `cargo test` 绿；截图存本计划目录。
- 依赖：Phase A 先转绿（B 的 path 依赖要钉在提交过的基线上）；本机起**新构建** gen-model 服务（旧构建无端点，R1）。
- 边界（首版不做）：尺寸编辑 / 导出 / 持久化；PIPE 级批量标注；文字深度遮挡与避让；库供数直连路径（MBD 永远走模型服务，与命令面同口径）。

### Phase C — 求解器续作（Phase B 之后或穿插）

- [ ] ⑥′ polar-lite 拍板（§5 D4，建议 A：抽 `occupancy.rs`）→ ⑤③ `angle.rs`（toy fixture `p3-installation-angles.json` + 真实候选 `24381_177565` 复核）。
- [ ] ⑦ 调整焊 `getadjustwelds`；⑧ PCOM / tee-olet 切段（`isoteeline`）。
- [ ] PML 图纸人工对照（待 E3D 环境，Phase 3/4 的老账）。
- [ ] plant3d-web 浏览器目测可与 B5 同一次起服务捎带做掉。

### 排序

A（转绿）→ B（B1 ∥ B2 → B3 → B4 → B5）→ C。D4（polar-lite）随时可拍板，不挡 A/B。

## 4. 风险

- **R1 端点版本**：现网 gen-model 旧构建（曾记 PID 60444）没有 `/api/mbd/v2`——B5 之前必须以新构建起服务，命令沿用上一计划 findings「Phase 2」。
- **R2 图元量**：大 BRAN 数百图元。线段合进单 mesh（LineList 先例就是这么干的）、文字每帧投影 O(n) 可忽略；超限截断 + issue 出声。
- **R3 ADR-0012 金样漂移**：守卫改动 field 金样，重录必须人工审阅（`record` 默认拒绝覆盖的机制照用）。
- **R4 glam / 依赖面**：plant-mbd glam 0.29 与 Bevy 的 glam 多版本共存没问题（契约边界是数组）；若编译面意外撞车，退路 = 契约抽轻 crate 或 plant-ui 侧 serde 镜像（那就要契约守卫双侧钉）。
- **R5 refno 形态**：plant-ui `RefU64` → 端点路径统一格式化 `w0_w1`（`parse_refno` 两种都收，但统一一种少一类日志歧义）。

## 5. 待拍板

- **D1 契约获取**：path 依赖 plant-mbd（**推荐**：单一契约源，纯库无拖累）/ plant-ui 侧 serde 镜像。
- **D2 尺寸文字**：egui 覆盖层投影（**推荐**：先例现成、字清晰）/ 3D 文字网格（随场景遮挡缩放，成本高）。
- **D3 右键落点**：仅 BRAN + 成员上溯（**推荐**）/ 首版就做 PIPE 级批量（一条 PIPE 多条 BRAN 并发取数与清层复杂度都翻倍）。
- **D4 Phase 5 ⑥′**：A 抽 polar-lite `occupancy.rs`（**建议**，上一计划 findings 已给评估）/ B 整体移植 Polar（3–4k 行，另立计划）/ C 维持现状先做 ⑤⑦。

## 6. 验证矩阵

```powershell
# Phase A（plant-mbd，离线）
cd D:\work\plant-code\old\vendor\plant-mbd
cargo test --workspace
cargo run -p plant-mbd-cli -- golden diff

# Phase B（plant-ui + 本机新构建 gen-model 服务）
cd D:\work\plant-code\old\plant-ui
cargo test --workspace
# 实机：起 gen-model 新构建（8022）→ plant-ui 打开 AvevaMarineSample →
# 视口右键金样 BRAN →「查看尺寸标注」→ 对照 fixtures/golden/<refno>.json 的尺寸文本
curl http://127.0.0.1:8022/api/mbd/v2/pipe/24383_99996   # 端点在位自检
```
