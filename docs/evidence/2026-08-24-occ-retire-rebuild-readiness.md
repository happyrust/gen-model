# OCC 退役 caliber 原子重建预检（2026-08-24）

## 工具

新增只读脚本 `scripts/Test-OccRetireRebuildReadiness.ps1`。它覆盖六类复用曲面，并同时检查：

- 缺失 `mesh_caliber` 的 `inst_geo` 身份；
- 仍指向旧身份的 `geo_relate`；
- `bad=true` 的复用身份；
- 未收口的 `model_update_pending`；
- 无 `geo_relate` 引用的复用身份；
- `meshed=true` 但目标 mesh 目录缺文件的复用身份。

默认只报告；维护窗口的部署后门使用 `-RequireReady`，任一条件不满足即退出 1。
源码守卫 `rebuild_readiness_gate_covers_every_reusable_surface_and_fails_closed` 钉住六类范围和
全部失败闭合条件。

## 输入

- 8009：当前验证库，namespace `1516` / database `AvevaMarineSample`。
- 7997：`.surreal/ams-7997-e3d-test-20260805` 的独立工作副本，锁定 SurrealDB
  `2.1.4+20250317.45013fc9` 挂到 8039；mesh 目录 `.scratch/meshes-8039`。
- 两次均为只读查询；8039 结束后停止。

## 字面结果

```powershell
powershell -File scripts/Test-OccRetireRebuildReadiness.ps1 `
  -Endpoint http://127.0.0.1:8009/sql -MeshDir assets/meshes `
  -OutJson .scratch/occ-readiness-8009.json
```

| 8009 变体 | 总身份 | 带 caliber | 缺 caliber | 旧身份引用边 |
|---|---:|---:|---:|---:|
| PrimLCylinder | 7 | 6 | 1 | 1,517 |
| PrimSphere | 0 | 0 | 0 | 0 |
| PrimLSnout | 3 | 0 | 3 | 6 |
| PrimDish | 0 | 0 | 0 | 0 |
| PrimCTorus | 8 | 0 | 8 | 9 |
| PrimRTorus | 17 | 0 | 17 | 75 |

汇总：缺 caliber 身份 29、旧身份引用边 1,607、未收口队列分组 4、坏复用身份 0、
复用身份孤儿 0。以本工作树 `assets/meshes` 为部署目录时缺文件 29。`ready=false`，退出 0
（报告模式）。

```powershell
powershell -File scripts/Test-OccRetireRebuildReadiness.ps1 `
  -Endpoint http://127.0.0.1:8039/sql -MeshDir .scratch/meshes-8039 `
  -OutJson .scratch/occ-readiness-7997.json
```

| 7997 副本变体 | 总身份 | 带 caliber | 缺 caliber | 旧身份引用边 |
|---|---:|---:|---:|---:|
| PrimLCylinder | 2 | 1 | 1 | 20,661 |
| PrimSphere | 0 | 0 | 0 | 0 |
| PrimLSnout | 112 | 0 | 112 | 1,201 |
| PrimDish | 17 | 0 | 17 | 102 |
| PrimCTorus | 95 | 0 | 95 | 664 |
| PrimRTorus | 167 | 0 | 167 | 1,945 |

汇总：缺 caliber 身份 392、旧身份引用边 24,573、未收口队列分组 4、坏复用身份 0、
复用身份孤儿 0；隔离 mesh 目录缺旧复用文件 384。`ready=false`，退出 0（报告模式）。

失败闭合复验：

```text
Test-OccRetireRebuildReadiness.ps1 ... -RequireReady
require_ready_exit=1

rebuild_readiness_gate_covers_every_reusable_surface_and_fails_closed ... ok
1 passed; 0 failed; exit 0
```

## 裁决

两份库都明确处于重建前状态，且 7997 副本的爆炸半径远大于 8009。维护窗口必须继续执行
“停止生成 → 快照 → 部署 → ADR-021 整库重建 → 本脚本 `-RequireReady` → 双库 RVM”这一
原子序列；当前数字禁止把代码发布或局部重生成解释为身份迁移完成。
