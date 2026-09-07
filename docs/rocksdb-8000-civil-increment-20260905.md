# AMS 8000 建筑类型增量验证

沿用隔离 RocksDB 18049、API 18050 和真实 E3D 私有项目副本；未修改共享项目。

## 样本层级

通过 E3D 只读查询实际 FLOOR `17496/72443`、GWALL `17496/105691` 得到层级；证据在 `civil-source-hierarchy/tty.log` 和 `civil-source-detail/tty.log`。新样本位于 8000 库的 `/1RX03-CASE-DQ` 下：

- STRU `24384/26606` → FRMW `24384/26607` → CFLOOR → FLOOR `24384/26609` → PLOO → 四个 PAVE。
- 同一 FRMW → CWALL → GWALL `24384/26616` → PLOO → 四个 PAVE。
- 同一 FRMW → SCREED `24384/26622` → PLOO → 四个 PAVE。E3D 创建、查询及保存成功，尚在补充增量验证。

FLOOR 直接挂 EQUI 的旧失败保留，不计通过。新合法样本成功创建，不再出现该崩溃。

## 已完成对比

| 快照 | 数据窗口 | 操作 | FLOOR / GWALL 几何 | API 与 RocksDB 属性 |
|---|---|---|---|---|
| civil-created | 470 | 新建，1200×1000×200 | 均通过 | 均通过 |
| civil-resized | 471 | PLOO HEIG 200→350 | 均通过 | 均通过 |
| civil-moved | 472 | BY E 120，名称追加 UPDATED | 均通过 | 均通过 |
| civil-screed-created | 473 | 加入 SCREED，1200×1000×200 | 三类均通过 | 三类均通过 |
| civil-screed-resize-result | 474 | SCREED 厚度 200→350 | 三类均通过 | 9 字段通过 |
| civil-screed-move-result | 475 | SCREED BY E 120、改名 | 三类均通过 | 9 字段通过 |
| civil-holes-create-result | 476 | 三类各加入 300×400 通孔 | 三类、108 射线通过 | 21 字段通过 |
| civil-holes-resize-result | 477 | 三类孔宽 300→500 | 三类、108 射线通过 | 21 字段通过 |

以上每阶段在模型已自动发布后抓取记录并独立导出 RVM；未用 ensure 促成发布。三阶段最大边界误差 0.0002 mm，体积误差低于 0.000026%。每阶段同时核对两类 NAME、POS 和各自 PLOO HEIG，共六个字段。

证据根目录：`output/rocksdb-8000-20260905/equi-increment/`，各快照含 RVM、网格、模型记录、数据库水位及 `types-comparison.json`。

## Plant UI

模型树自动出现新增 STRU/FRMW/CFLOOR/CWALL。完成自动厚度发布后在 UI 加载 FLOOR，随后源模型位移、改名；UI generation 8 自动刷新，名称为 `/CODEX_TYPES_FLOOR_UPDATED`，POS=120,0,0，日志 ERROR 0。

![楼板名称位置自动刷新](../output/rocksdb-8000-20260905/equi-increment/screenshots/civil-floor-moved-renamed.jpg)

后续楼板从实心自动刷新为带孔，再自动更新为孔宽 500；UI generation 14/15 留有刷新日志。SCREED 和 GWALL 在自动发布及 RVM 对比通过后由 UI 加载、聚焦，分别显示 POS=4520,0,0 和 POS=2320,0,0。它们的截图证明最终显示与数据；楼板截图另有连续自动刷新证据。

![楼板首次穿孔自动更新](../output/rocksdb-8000-20260905/equi-increment/screenshots/civil-floor-hole-300-auto.jpg)

![楼板孔宽 500 自动更新](../output/rocksdb-8000-20260905/equi-increment/screenshots/civil-floor-hole-500-auto.jpg)

![SCREED 最终穿孔及位置](../output/rocksdb-8000-20260905/equi-increment/screenshots/civil-screed-hole-moved.jpg)

![GWALL 最终穿孔及位置](../output/rocksdb-8000-20260905/equi-increment/screenshots/civil-gwall-hole-moved.jpg)

当前已覆盖矩形剖面及 NBOX 通孔的创建、尺寸和位置变化。属性不完整诊断仍存在，未将日志 ERROR 0 解释为全项目无缺陷。

## 真实 RocksDB 重启与冷读修复

停止服务后抓取数据库全表内容，再真正终止并重启 RocksDB 进程；`civil-restart/before-manifest.json` 与 `after-manifest.json` 完全相同：188 张表、55,881 条记录，逐表行数及规范化完整内容 SHA-256 一致。

首次服务重启后仍发现模型读取失败：`civil-restarted` 的数据库有三件模型和窗口 477 凭证，但 `/model/records` 只读空的进程缓存，三件模型全部缺失。21 个数据字段仍通过。该失败快照及超时记录完整保留，未用重新生成掩盖。

修复模型记录读取：持久模式从 `inst_relate`、`tubi_relate` 的已发布模型返回，内存模式继续读 ModelMemoryStore；保留烘焙网格、共享图元局部变换、管身及无效管身标记。新冷读回归覆盖三种记录并排除其他根，通过；18 项接口测试通过，服务编译通过。

`civil-restarted-durable-fixed` 为新服务冷读证据，来源 `model-database`，未调用 ensure：三件模型、108 条孔洞射线、21 个属性字段均通过，且没有发布超时。此修复解决读取路径，未把重建模型当作数据库恢复。

## 剩余族样本发现

## 删除增量与 Plant UI 子树清理

会话 478 删除原 civil STRU 的 25 个节点：模型记录为零，25 个属性接口均返回 404，RocksDB `pe.deleted` 全部为 true，见 `civil-deleted/result.json`。首次 UI 验证发现 `not_found` 被当作临时查询错误，仍保留旧子树；失败截图 `civil-deletion-stale-tree.jpg` 保留。

Plant UI 修复将类型化 `not_found` 与查询失败分离：摘掉确认缺失的分支，再由可达性清扫收集后代、卸载模型并清除选择；临时错误仍保留原数据。三个回归测试通过，覆盖父节点未重查、相邻分支保留及 SITE 卸载；应用编译通过。

独立重现使用会话 479 新建 STRU `24384/26631` → FRMW → PANE `24384/26633` → PLOO/PAVE，共八个节点。新版 UI 加载面板并选中属性后，在 E3D 会话 480 删除 STRU。未重启 UI、未手动取回工作：17:55:04 自动刷新后子树消失、面板卸载、属性选择清空，缓存元素 78→75，WARN 0 / ERROR 0。后端八个属性接口均 404，持久模型记录为零，见 `ui-delete-delete/result.json`。

![删除前模型及属性](../output/rocksdb-8000-20260905/equi-increment/screenshots/ui-delete-before-fixed.jpg)

![同一会话自动清除模型及子树](../output/rocksdb-8000-20260905/equi-increment/screenshots/ui-delete-after-fixed-auto.jpg)

## 剩余目录几何族

`remaining-source-hierarchy/tty.log` 和 `inventory.json` 保存对 54 个剩余 noun 的只读查询。39 个几何集类型找到样本及真实 owner，例如 BOXI `23274/288402` 位于 GMSE `23274/288400` 下。12 个 A* 以及 NBXI、NSSP、NTUB 在本次 MDB 查询范围内未命中；未计通过，也不据此认定类型不存在。大部分几何集样本属于目录数据库，修改前须建立真实私有副本并接入依赖增量监听。
