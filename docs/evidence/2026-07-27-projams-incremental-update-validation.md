# ProjAMS 增量更新验证（2026-07-27）

环境：ProjAMS（AvevaMarineSample）+ 隔离 SurrealDB `127.0.0.1:8009`。除明确记录的
D-02/D-09 `SAVEWORK` 会话外，测试只读 E3D 源文件；所有现场变更均已用后续会话恢复，
专用 dbnum 夹具均在测试结束时清理。

## 结果

| 范围 | 结果 | 证据 |
|---|---:|---|
| EQUI 最小交付单元生成 | 通过 | 真实 EQUI 根生成 |
| BRAN 最小交付单元生成 | 通过 | `output/live-bran-direct-20260727.log` |
| HANG 最小交付单元生成 | 通过 | `output/live-hang-direct-20260727.log` |
| SUPPO 最小交付单元生成 | 通过 | `output/live-suppo-direct-20260727.log` |
| FTUB 删除、跨 BRAN 移动、重排 | 通过 | `output/live-ftub-delete-move-reorder-fixed-20260727.log` |
| DirectGeometry | 通过 | 8000/25–26 `BOX.XLEN`、7997/75 `WALL.JUSL`，真实根生成 |
| TransformOnly | 通过 | 8000/27–28 `FTUB.POS`、7997/77–80 `EQUI.POS`，真实 transform 更新 |
| DataOnly | 通过 | 7997/82 `DAMP.NAME`，无模型任务且数据、水位正确 |
| 负几何 | 通过 | 真实 `NCYL 24381/100680` 变化重生成所属 `EQUI 24381/100677` |
| 缺失模型按需生成 | 通过 | 真实 `BRAN 24381/107104`、`FLOOR 24381/10624 → CFLOOR 24381/10623` |
| 共享 SPCO 级联 | 通过 | `SPCO 23274/295504` 的 72 个 DAMP 消费者归并并生成 67 个 BRAN |
| 模型删除与替换 | 4/4 通过 | 共享实例、无 `geo_relate`、软删子树、BRAN TUBI 替换 |
| 恢复、幂等、队列、水位 | 通过 | 含前端重开重试、后端进程强杀后恢复及独立 live 测试 |
| 数组属性 qualifier（C-ATTR-03） | 通过 | `array_attribute_effect_retains_changed_qualifier`：`PARA[2]` 保留为一基 qualifier 2 并触发 Regen |
| 级联范围上界（C-REF-03） | 通过 | `c_ref_03_cascade_upper_bound_rejects_every_non_dependency_attribute`：全 schema 非依赖属性不建 `ref_rev` 边 |
| 默认后端单测 | 190/190 通过 | 45 个 ignored live/bench 测试未纳入默认集（新增 D-04 live） |
| 前端手动更新聚焦测试 | 10/10 通过 | `manual_model_update` |

FTUB 被验证为 BRAN 内的组件，不是最小交付单元：FTUB 变化只调度所属 BRAN；
跨 BRAN 移动同时更新旧、新 BRAN。窗口内 `Add -> Deleted` 若元素在窗口前的基线已存在，
按删除处理，避免残留模型。

实际 ProjAMS 会话还暴露了 BRAN 元数据 `CACHID`、`LCHKDA` 被保守误判为几何变化的问题；
两者现已归为 DataOnly，FTUB.POS 的 27–28 窗口只执行 FTUB transform，不再多余重生成 BRAN。

FTUB MOVE/ORDER 先用真实 ProjAMS PE/CATA 状态构造合成会话覆盖异常与幂等路径，再用
E3D `SAVEWORK` 生成真实文件会话完成端到端验证：

| sesno | E3D 操作 | 解析结果 | 实际生成 |
|---:|---|---|---|
| 31 | `FTUB 24384/22442`：`BRAN 24384/22441 → 24384/22404` | FTUB `OWNER` 变化；新旧 BRAN `MemberChanged` | BRAN 22404、22441 |
| 32 | 恢复 FTUB 到 22441 | 反向 `OWNER` 变化；新旧 BRAN `MemberChanged` | BRAN 22404、22441 |
| 33 | `FTUB 24384/22440 BEFORE BEND 24384/22439` | BRAN 22404 `Reordered` | BRAN 22404 |
| 34 | `FTUB 22440 AFTER BEND 22439` 恢复 | BRAN 22404 `Reordered` | BRAN 22404 |

四次正式执行均返回 `status=success`。结束后确认
`applied_sesno=file_latest_sesno=34`、`24384/22442` 唯一 OWNER 为
`24384/22441`、22439/22440 的 `pe_owner` 顺序恢复为 34/35，待生成队列为 0。
随后再次执行正式手动更新返回 `status=up_to_date`，批次与生成单元均为空；
`model_update_pending`、`increment_update_attempt`、`incr_side_effect_pending`
计数均为 0。后端完整单测结果为 190 passed / 45 ignored / 0 failed，前端
`manual_model_update` 聚焦测试为 10 passed / 0 failed。

隔离库曾只含 463 条增量触及的 `ref_rev` 边，导致共享 SPCO 级联只能消费种子本身。执行既有
`rebuild_ref_rev` 冷启动回填后，扫描 220780 个当前元素并写入 91459 条去重边；目标 SPCO
的 72 条反向边与正向 DAMP 消费者完全一致，归并为 67 个 BRAN。完整队列测试在批量写遇到
Surreal 事务冲突后按既有策略逐根回退，最终 72 个消费者均存在 `inst_relate`，预留任务队列
与 `ref_rev_rebuild` staging 表均为空。

测试计划中 G4/G8 的描述是实现前状态。当前 `pdms-io` 已从数组属性前后快照计算变化下标，
`model_impact` 会消费 `qualified_attribute_changes()`；`ref_rev` 虽仍不存属性列，但建边入口
已按 schema `ELEMENT` 引用与 curated DependencyCascade 严格门控，因此 C-ATTR-03 与
C-REF-03 的可执行断言均已通过。

D-15 的两条 live 幂等断言也已复跑：同一 `Add` 的 `pe_owner` 关系重复回放收敛，
`finalize_attempt` 重复执行保持队列/水位一致。测试后隔离库
`model_update_pending`、`increment_update_attempt`、`incr_side_effect_pending` 均为 0。

## D-01～D-15 完成度审计

“数据/模型通过”与“rs-plant-3d 视觉通过”分开记账：

| ID | 当前自动/实库证据 | 尚缺证据 |
|---|---|---|
| D-01 | 真实共享 SPCO：72 个 DAMP → 67 个 BRAN，全部生成 | 多管道同机位前后截图 |
| D-02 | E3D sesno 31 实际跨 BRAN MOVE、sesno 32 恢复；新旧 owner 两根均生成 | 两处同机位前后截图 |
| D-03 | 四类删除/替换与 FTUB 删除清理通过 | E3D 实际 Deleted session、消失截图 |
| D-04 | 8000/21 两个真实 GENSEC Add：FRMW 直接 owner → SUPPO 根，两个根实际生成 | 实体出现截图 |
| D-05 | FLOOR/PAVE 路由、CFLOOR 真实生成通过 | E3D 实际结构属性 session、外形截图 |
| D-06 | WALL.JUSL 真实 session 与 CWALL 生成；GENSEC 多变体生成通过 | 型材扫掠前后截图 |
| D-07 | 真实 SUPPO/GENSEC 生成根与模型生成通过 | E3D 实际 SUPPO 参数 session、支架截图 |
| D-08 | 真实 NCYL 变化重生成所属 EQUI | 开孔/凹槽前后截图 |
| D-09 | E3D sesno 33 实际 Reordered、sesno 34 恢复；BRAN 22404 均重生成 | 同机位顺序截图 |
| D-10 | 7997/82 DAMP.NAME：数据与水位更新、无模型任务 | 模型树改名且几何不变截图 |
| D-11 | 8000/27 FTUB.POS：数据、transform、AABB 与模型通过 | **已完成**：`output/increment-test/rs-plant3-d-before.png` / `after.png` |
| D-12 | BRAN/FLOOR/WALL/GENSEC 缺失模型按需闭包与生成通过 | 首次加载查看器截图 |
| D-13 | 畸形 `ref_rev` 触发持久化 `CascadeExpand`，修复后收敛 | 最终查看器收敛截图 |
| D-14 | 后端强杀恢复、前端重开读取持久任务通过 | 重启后最终查看器截图 |
| D-15 | `pe_owner` 重放与 `finalize_attempt` live 幂等通过 | 同机位重复执行无画面抖动截图 |

因此目前不能宣称 D 批次全部验收：D-11 完整，其余用例的数据/模型层多数已通过，
但上表列出的真实 E3D session 或 rs-plant-3d 视觉证据仍需补齐。

D-04 的真实样本为 `GENSEC 24384/25743`（owner `FRMW 24384/25742`，根
`SUPPO 24384/25725`）和 `GENSEC 24384/25923`（owner `FRMW 24384/25887`，
根 `SUPPO 24384/25872`）。`live_projams_nested_created_routes_and_generates_delivery_roots`
从 sesno 21 读取原始 Add，断言非交付 owner、生成根与计划后实际生成两个 SUPPO；
分别更新 21/23 个模型节点，最终两个 GENSEC 的 `inst_relate` 均存在。

D-02/D-09 的实际 `SAVEWORK` 会话已经补齐；剩余缺口仅是 rs-plant-3d 同机位前后截图，
不能把 E3D 命令窗口截图替代为最终查看器视觉证据。

rs-plant-3d 已连接主实例可见 AvevaMarineSample 项目与“模型更新”入口；该实例以管理员
完整性级别运行，自动化控制器只能读取画面，无法注入点击。另启的非管理员实例可注入输入，
但因不继承原启动目录配置而无法连接项目，已关闭且未改动数据库。因此本轮不把入口可见性
误记为更新窗口的交互验收，仍以聚焦 UI 测试和上表视觉缺口分别记账。

CATA 按当前产品决定不在本轮验证范围内。
