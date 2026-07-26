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
| DirectGeometry / TransformOnly / DataOnly | 通过 | `output/live-action-kinds-20260727.out.log` |
| 恢复、幂等、队列、水位 | 7/7 通过 | 独立 live 测试 |
| 默认后端单测 | 190/190 通过 | 42 个 ignored live/bench 测试未纳入默认集 |

FTUB 被验证为 BRAN 内的组件，不是最小交付单元：FTUB 变化只调度所属 BRAN；
跨 BRAN 移动同时更新旧、新 BRAN。窗口内 `Add -> Deleted` 若元素在窗口前的基线已存在，
按删除处理，避免残留模型。

DirectGeometry、TransformOnly、DataOnly 使用真实 ProjAMS EQUI/BOX、真实规划器和真实模型
执行器验证；操作载荷由测试构造，没有向 E3D 写入新 sesno。因此这项证明三类动作的分类与
执行路径正确，不作为“实际 E3D 新会话文件”证据。

CATA 按当前产品决定不在本轮验证范围内。
