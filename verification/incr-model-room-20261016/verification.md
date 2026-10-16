# 模型增量 + 房间增量 端到端联测（2026-10-16）

## 场景

真实 E3D SAVEWORK → gen-model（127.0.0.1:8022，AvevaMarineSample / /ALL / ns 1516，watch=7998,8000）数据批次 → 模型重算 → 房间边重算，往返两轮。

## 变更体

`EQUI /1-LNR-Q005-PJ`（refno `24384/24776`，dbnum 8000，生成根），其唯一叶子 `24384/24777`（0.4×0.6×0.55 m BOX）。

- 基线：`POS W2821 N8004 U1400`；`24384/24777` 在房间 **R224**（面板 `24381_948`，`inside_count=4`，跨库边：成员在 8000 / 面板在 7997）。
- apply：同一 E/N，`U 1400 → 53400`（`apply.mac`，一条 SAVEWORK）。
- restore：`U 53400 → 1400`（`restore.mac`，一条 SAVEWORK）。

## 执行方式

E3D TTY：`scripts/e3d/run_ams_c_entrymacro.bat` + 自制 wrapper（L3-ALIVE / L3-DONE / QUIT，`driver_apply.mac` / `driver_restore.mac`）。apply 与 restore 的 `EQUIPMENT /1-LNR-Q005-PJ` 前后 POS 均见 `apply.log` / `restore.log`。

`POST /api/v1/update/execute` 不是必需——watcher 在 SAVEWORK 后数秒内自动入队（ADR-058 D8），两批 `data_batch` 任务均 `succeeded`。

## 证据

### apply 腿（sesno 647→648，任务 `db-20261016-013752-000001`）

- 数据窗：`merged_sesnos=[647,648]`，`modified=2`，`changed_elements=2`。
- 模型：unit `24384/24776 EQUI status=generated kind=regen`，`model_verdict=in_sync`；`/model/records` 中 `24384_24777` `world_trans.t` 1400→53400，`world_aabb.z` 1125..1675 → 53125..53675。
- 房间：`room_relate` 边 `out=pe:24384_24777` 由 `in=pe:24381_948 (R224, inside_count=4)` 变为 `in=pe:24381_36945 (R000, inside_count=8)`——旧边删除、新边插入；`e3d.room.lookup` `R224 → R000`，`published_version` 646→648，`publication_status=current`，`stale=false`。
- 常驻房间参考模型**未重建**：`/health` 的 resident generation 前后均为 `72979202…94`，属定向房间重算而非全库 rebuild。

注：落点是 R000 而非最初预计的 R972——成员按其实际包围盒归属到包含它的面板，R000 面板 `24381_36945`（房间 `/1RX-RM13-R000`）命中。

### restore 腿（sesno 649→650，任务 `db-20261016-013952-000002` / `013957-000004`）

- `world_trans.t` 回到 U 1400，AABB 复原；
- `room_relate` 恢复 `in=pe:24381_948 R224, inside_count=4`；`published_version` → 650。

### 任务链

```
db-20261016-013752-000001  succeeded  8000  647→648   apply
db-20261016-013952-000002  succeeded  8000  649→649   restore 腿第一会话
db-20261016-013957-000004  succeeded  8000  650→650   restore 腿第二会话
```

每批 stages：`data → generation → publication → spatial → rooms`；房间重算在批内（`durable_pending`）完成，无单独房间轮。

## 结论

- 模型增量：EQUI 根 regen，`/model/records` 回执反映新变换；restore 后复原。✓
- 房间增量：成员边的删+插在增量窗内完成，跨库边（成员 8000 / 面板 7997）正常；常驻模型不换代。✓
- 两侧往返幂等，水位按会话推进（646→648→650），`up_to_date` 恢复 true。

## 安全网

`ams8000_0001` 基线文件已备份（`output/incr-room-equi/ams8000_0001.baseline`，SHA256 `CFB1EA5F…5BD6`）；restore 宏已把 POS 写回原值。

## 未覆盖

- plant-ui 侧刷新/陈旧标记验证（需起 UI）。
- TransformOnly / Deleted 另见 `l3_suite` m2/m3 与 `verification/ams8000-crud-20260912/`。
