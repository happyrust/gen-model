# AMS 8000 面板增量及旧队列停滞复现

环境沿用独立 RocksDB 18049 / API 18050，命名空间 `rocks8000_20260905`。PANE 挂在设备 `24384/24776`，面板 `24384/26599`，剖面 `24384/26600`。

## 已验证

| 快照 | 操作 | RVM 几何 | API 与 RocksDB 属性 |
|---|---|---|---|
| `panels-recovered-created` | 1200×1000×200 面板，服务重启恢复 | 通过 | NAME、POS、剖面 HEIG 通过 |
| `panels-resized` | 剖面 HEIG 200→350，自动增量 | 通过 | 通过 |
| `panels-moved` | 面板向 E 位移 120、改名，自动增量 | 通过 | POS=2320,0,0、NAME、HEIG=350 通过 |
| `panels-hole-created` | NBOX 300×400 穿孔 | 通过，36 条射线一致 | 通过 |
| `panels-hole-recovered` | 500×400 穿孔，服务重启恢复 | 通过，36 条射线一致 | 通过 |
| `panels-repeat-lease-fixed-final` | 孔宽连续 300/500 切换六轮，最终 500 | 通过，36 条射线一致 | 7 个字段通过 |

最大边界误差 0.000754 mm。证据在 `output/rocksdb-8000-20260905/equi-increment/` 各快照的 `types-comparison.json`、RVM、模型记录及属性中。恢复后的创建不能替代首次创建的自动增量验收。

![面板厚度更新后的模型](../output/rocksdb-8000-20260905/equi-increment/screenshots/pane-resized-model.jpg)

![剖面厚度 350](../output/rocksdb-8000-20260905/equi-increment/screenshots/pane-thickness-350.jpg)

![面板名称和位置自动更新](../output/rocksdb-8000-20260905/equi-increment/screenshots/pane-moved-renamed.jpg)

## 首次创建失败及修复

`panels-created` 保存了真实失败：数据已应用到 453，模型停在 452，RVM 有 PANE 而模型记录缺失。`stalled-queue.json` 中重复窗口 453 属 epoch 24，当前 epoch 已到 27；该行既不能被派发，又令模型积压因 data_queued 让位。`service-stall.log` 保存现场。

新增队列收口：worker 对旧 epoch 的排队行读取持久化水位；仅项目、路径、类型一致、已初始化、窗口已完整应用且无同库运行任务的普通窗口可标记 already_applied。重建任务和未完成窗口保留。调度器 16 项测试通过，服务编译通过。厚度及位移两轮未命中收口分支，后续穿孔重复测试补充了实际命中证据，见下文。

## 连续穿孔增量补充验证

首次孔宽 500 的自动更新在 `panels-hole-resized/publication-timeout.json` 留下失败，恢复快照不能替代自动增量通过。进一步查明 `DbnumState` 的两处查询未投影 `owner_project`，导致队列收口校验始终失败；补齐投影后，`panels-repeat/service-with-stale-retirement.log` 已记录窗口 463 的真实收口。

第一组六轮重复测试仍在第六轮失败（`panels-repeat/5/failed.json`）：已认领但因数据优先让出的模型任务保留了运行租约，需等待十分钟过期。修复为在协作式消化返回时释放本轮未完成认领，校验 revision 和 lease_owner，保留其他工作者和更新版本的租约。回归测试覆盖三种情况；模型待办模块 131 项通过、12 项显式忽略。

新服务连续六轮 `300→500→300→500→300→500` 全部自动发布，窗口 464–469，每轮记录位于 `panels-repeat-lease-fixed/0..5/passed.json`。期间未调用 ensure 或重启服务。最终独立 RVM 对比体积误差 0.000161%，边界误差 0.000753 mm，36 条射线全部一致；API 和持久化 NAME、POS、HEIG、孔洞 XLEN/YLEN/ZLEN/POS 一致。

## Plant UI 房间连接及截图

房间查询的 `Already connected` 已复现并修复：原路径重复初始化全局连接，且尝试从 UI 包中加载不存在的 SQL 资源。原生端改为 OnceCell 复用登录连接；真实 RocksDB 回归同时发起八个连接调用并查询元素/面板房间，通过。日志为运行根目录的 `room-connection-test-v2.log`，旧失败日志保留。

新 UI 已显示穿孔、更新后的名称和 POS=2320,0,0。当前查询返回“该元素不属于任何房间”，日志 ERROR 0。最终截图记录 UI generation 5 自动刷新；`pane-room-query-fixed.jpg` 是此前手动加载的诊断截图，仅用于房间查询修复证据。

![连续六轮自动更新后的穿孔模型](../output/rocksdb-8000-20260905/equi-increment/screenshots/pane-hole-repeat-auto-final.jpg)

![模型及名称位置数据](../output/rocksdb-8000-20260905/equi-increment/screenshots/pane-hole-repeat-auto-data.jpg)

![孔洞负体尺寸 XLEN=500、YLEN=400、ZLEN=1000](../output/rocksdb-8000-20260905/equi-increment/screenshots/pane-hole-width-500.jpg)

## 剩余覆盖

后续已建立合法 FLOOR、GWALL、SCREED 样本，最新覆盖结果见 [建筑类型增量报告](rocksdb-8000-civil-increment-20260905.md)。

FLOOR 直接挂 EQUI 的试建导致 E3D 0xC0000005，未保存，见 `panels-floor-create`。这不是合法楼板样本验收，也未计通过。已找到 E3D 安装目录下 `cvcreatefloor.pmlfnc`、`cvcreatepanel.pmlfnc` 和 `sctlcrelem.pmlfnc`，后续按实际建筑层级准备样本。A* 目录族、其余建筑拉伸类型、几何集仍未验收。属性不完整诊断及服务端既有房间缺少面板几何问题仍存在；当前面板房间查询正常不代表整体 UI 无缺陷。
