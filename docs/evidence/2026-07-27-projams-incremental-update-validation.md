# ProjAMS 增量更新验证（2026-07-27）

环境：ProjAMS（AvevaMarineSample）+ 隔离 SurrealDB `127.0.0.1:8009`。测试只读 E3D
源文件；专用 dbnum 夹具均在测试结束时清理。

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
| 模型删除与替换 | 4/4 通过 | 共享实例、无 `geo_relate`、软删子树、BRAN TUBI 替换 |
| 恢复、幂等、队列、水位 | 7/7 通过 | 独立 live 测试 |
| 默认后端单测 | 190/190 通过 | 44 个 ignored live/bench 测试未纳入默认集 |
| 前端手动更新聚焦测试 | 9/9 通过 | `manual_model_update` |

FTUB 被验证为 BRAN 内的组件，不是最小交付单元：FTUB 变化只调度所属 BRAN；
跨 BRAN 移动同时更新旧、新 BRAN。窗口内 `Add -> Deleted` 若元素在窗口前的基线已存在，
按删除处理，避免残留模型。

实际 ProjAMS 会话还暴露了 BRAN 元数据 `CACHID`、`LCHKDA` 被保守误判为几何变化的问题；
两者现已归为 DataOnly，FTUB.POS 的 27–28 窗口只执行 FTUB transform，不再多余重生成 BRAN。

FTUB MOVE/ORDER 使用真实 ProjAMS PE/CATA 状态构造合成会话。现场测试现在会在开始时清理
中断遗留，并在结束时恢复真实 sesno 30、唯一 OWNER、BRAN 成员顺序、模型及空队列；重复执行
后确认 `applied_sesno=file_latest_sesno=30`，`24384/22403` 仅属于 `24384/22402`。

尚缺的证据是 E3D 中新建并 `SAVEWORK` 的实际 MOVE/ORDER 会话及同机位前后截图。当前
`ams7997_0001`、`ams7999_0001`、`ams8000_0001` 的修改时间仍为 2026-07-26 10:50:21，
因此不能把 UI 中已执行但未写入数据库文件的操作计作新 sesno 证据。

CATA 按当前产品决定不在本轮验证范围内。
