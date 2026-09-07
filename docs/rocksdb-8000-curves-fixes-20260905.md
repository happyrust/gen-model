# AMS 8000 设备类型增量复测

2026-09-05 本轮已完成 **23 类整组几何与数据回归**，创建后集合、尺寸更新后集合、位移/改名后集合、进程重启后集合四个快照均通过。每个快照包含 23 个测试模型、43 组属性/顶点检查和 396 条布尔射线；最大体积误差 0.1684%，最大边界误差 0.456 mm。各类型不是同一时刻创建，较早修复的类型在整组创建快照中已经处于更新状态；单类型基线另有保存。

真实 RocksDB 静止重启：**183 张表、55,864 条记录的完整内容哈希与行数完全一致**。证据：`increment23-persistence-quiescent/before-manifest.json`、`after-manifest.json`。首次 `increment23-persistence` 检查被重启的扫描进程写入水位时间戳干扰，仅水位表不同，已保留失败记录并在无服务监听时重新验证。

环境：独立 AMS 副本，SurrealDB 2.1.4 / RocksDB，数据库端口 18049，服务端口 18050，命名空间 `rocks8000_20260905`。未修改原始共享项目。

## 已完成的修复和实测

| 类型 | 问题 | 本轮验证 |
|---|---|---|
| CYLI | 实例使用 Y 轴底部原点，和 RVM 的 Z 轴中心原点不一致 | 创建、直径 400→520、位移 120、改名通过 |
| CONE、SNOU | 独立参数图元误用粗布尔面片精度 | 创建、尺寸、位移和改名通过 |
| NCON、NSNO | 布尔圆周错误地使用 5 mm 精度 | 改为 RVM 的 10 mm 网格，36 条射线逐条通过 |
| REVO、NREV | 设计回转体误用参数图元精度 | 270° 创建、180° 更新、位移和改名通过；负体 36 条射线通过 |
| NCTO | 布尔环面环向、管截面沿用独立实体精度 | ROUT 450→520、位移和改名通过；36 条射线通过 |
| POLYHE、NPOLYH | 顶点表不在首位时被误判为旧格式 | 乱序创建、顶点更新、位移、改名均通过 |
| POHE | 将独立薄面板误拼成实心封闭壳 | 修复为每个 POGO 独立 0.99 mm 面板；创建、12 个顶点更新、位移、改名均通过 |

验收阈值保持不变：体积相对误差 <1%，边界 <2 mm；布尔宿主每个 36 条射线要求交点数量相同且位置误差 <2 mm。数据检查要求 API、RocksDB 和预期值一致。比较脚本现在分别报告 `geometry_passed`、`data_passed`，总结果要求二者同时通过。

主要证据位于 `output/rocksdb-8000-20260905/equi-increment/`：

- `cyli-centered-fixed`、`cyli-resized-fixed`、`cyli-moved-fixed`：圆柱原始 RVM、生成网格和属性。
- `curves-grid-created`、`curves-grid-resized`、`curves-grid-moved`：六种曲面及圆柱复测。最终 7 类几何与数据全部通过。
- `ctor-grid-created`：首次 NCTO 尺寸相关失败，保留未覆盖。
- `ctor-grid-corrected`、`profiles-ctor-grid-resized`：修正后的两个尺寸；后者 8 类几何与数据全部通过。
- `profiles-poly-unordered-created`：POLYHE / NPOLYH 面在前、顶点表在后的创建测试通过。
- `profiles-poly-pohe-valid`：POHE 实心壳与 RVM 薄面板的失败对照。
- `profiles-poly-pohe-fixed`、`profiles-poly-all-resized-moved`：POHE 修复及三个多面体类型的增量通过。
- `profiles-final-created`、`profiles-final-resized`、`profiles-final-moved`、`profiles-final-restarted`：23 类整组复测。机器可读汇总为 `final-23-summary.json`。

## Plant UI 截图

23 类测试对象删除后，82 个属性查询全部返回 404，持久化属性均对应已删除的 PE；当前树和模型各剩原始 BOX 一件。历史属性保留符合软删除契约。证据：`types-final-restored/deletion-softdelete-check.json`。

截图为实际验证窗口，项目代号 `rocks8000_20260905`。圆柱和回转体的位移/改名由 UI 自动刷新，无需重启 UI。

![圆柱直径更新为 520](../output/rocksdb-8000-20260905/equi-increment/screenshots/cyli-resized-520.jpg)

![圆柱位置与名称增量](../output/rocksdb-8000-20260905/equi-increment/screenshots/cyli-moved-renamed.jpg)

![回转体角度更新为 180](../output/rocksdb-8000-20260905/equi-increment/screenshots/revo-resized-180.jpg)

![回转体位置与名称增量](../output/rocksdb-8000-20260905/equi-increment/screenshots/revo-moved-renamed.jpg)

![23 类设备总览](../output/rocksdb-8000-20260905/equi-increment/screenshots/equipment-23-types-overview.jpg)

![环面角度更新为 90](../output/rocksdb-8000-20260905/equi-increment/screenshots/rtor-angle-90.jpg)

![环面尺寸、位移、名称及重启恢复](../output/rocksdb-8000-20260905/equi-increment/screenshots/rtor-moved-resized-restarted.jpg)

## 未结项

后续 PANE 厚度、位移和名称更新结果以及首次自动创建停滞记录见 [面板增量报告](rocksdb-8000-panel-increment-20260905.md)。

![删除全部测试对象后恢复原始 BOX](../output/rocksdb-8000-20260905/equi-increment/screenshots/equipment-deleted-restored.jpg)

这不是“整个分类器所有类型通过”的报告。目录 A*、几何集及其他类别尚需单独覆盖。环面 `ANGL 240` 会令 E3D 进程崩溃，但 `180→90→180` 已成功且 90° RVM 对比通过；240° 未计通过。UI 属性不完整诊断和房间/PANEL 查询失败仍存在，截图保留了这些错误。

`profiles-poly-unordered-resized` 和 `profiles-poly-pohe-created` 为导出/生成异常调查记录，不计通过。POHE 正确结构为 POHE→POGO→POIN；对空成员使用 LAST MEM、使用 NEXT MEM 导航的旧宏失败，改为实际数据库 refno 逐点更新后成功。所有临时对象位于私有测试副本。

编译通过；本轮相关单元测试：设计曲面 3 项、环面 10 项、多面体 9 项、POHE 2 项通过。真实增量证据优先于仅测试几何辅助函数。

补充完整库回归：`cargo test --manifest-path ../vendor/e3d-model/Cargo.toml --lib`，241 项通过，0 失败、0 忽略。
