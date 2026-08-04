# AMS 增量更新测试与验证汇总

> 汇总日期：2026-07-29  
> 适用项目：`D:\work\plant-code\old\gen-model`  
> 数据库版本：`D:\work\plant-code\old\gen-model\bin\surreal.exe`  
> 当前前端与三维验收端：`D:\work\plant-code\old\plant-ui`  
> 旧版截图来源：`D:\work\plant-code\old\rs-plant3-d`（仅保留为历史证据）  
> 主要实库证据日期：2026-07-24 至 2026-07-27

## 1. 结论

AMS 增量更新的核心链路已经完成较广覆盖：

- 已验证数据变化分类、最小交付单元路由、按需模型生成、模型删除/替换、反向引用级联、队列恢复、水位推进和幂等性。
- 已在 dbnum 7997、8000 上执行真实 E3D 会话或实库增量验证；dbnum 7999 已完成基线完整性验证。
- 管道、设备、结构、支吊架、GENSEC、FLOOR/WALL、目录依赖和负向几何等主要类型均已有自动化或实库证据。
- `FTUB` 已按 BRAN 子元件处理，不再作为最小交付单元。
- 旧版前端中，D-11 已完成“E3D 修改 → 服务端增量 → rs-plant3-d 前后截图”的历史视觉闭环。
- 前端切换到 plant-ui 后，D-01～D-15 都需要按新链路重新完成“预览 → 入队 → 队列完成 → 模型树自动刷新 → 三维模型自动刷新 → 前后截图”验收。
- 旧版 rs-plant3-d 截图继续证明当时生成结果可显示，但不再算作 plant-ui 的当前视觉验收结果。

当前分支也不能表述为“测试全绿”：

- 2026-07-29 运行 `rtk cargo test -j 2 --lib` 时编译失败，共 4 个错误，均为测试代码仍引用已经不存在的 `revision` 字段。
- 2026-07-29 本地 SurrealDB `127.0.0.1:8009` 未启动，因此未重新执行实库完整性脚本；本文实库结论以 2026-07-27 已保存证据为准。
- 本次只调整测试文档，尚未执行 plant-ui 的 `cargo test --workspace` 和实机截图流程。

## 2. 验证层级

| 层级 | 含义 | 当前用途 |
|---|---|---|
| A | Rust/前端单元测试 | 验证属性分类、根节点选择、队列、状态机和界面交互逻辑 |
| B | 真实 SurrealDB/服务端测试 | 验证实际 PE、实例、AABB、反向索引、模型队列和水位 |
| C | 真实 E3D 会话测试 | 验证增、删、改产生的 session 数据能被正确扫描和执行 |
| D | plant-ui 前后截图 | 验证任务队列完成后，模型树和三维模型自动刷新且最终显示正确 |
| D-旧 | rs-plant3-d 历史截图 | 仅证明旧版查看器当时可以显示，不替代 plant-ui 复验 |

只有同时具备 B/C/D 证据的场景，才算当前前端的完整端到端视觉验收。单元测试通过或旧版截图存在，都不等于该类型已经在 plant-ui 中完成显示验收。

## 3. dbnum 覆盖情况

| dbnum | 已验证内容 | 当前结论 | 待补内容 |
|---|---|---|---|
| 7997 | EQUI、BRAN、DAMP、HANG、FLOOR/WALL、真实删除、位置移动、名称修改、负向几何、SPCO 级联、按需生成 | 实库增量覆盖最完整；数据、模型、队列和水位证据通过 | 在 plant-ui 补齐模型树、任务队列和三维前后截图 |
| 7999 | 基线初始化与完整性检查；记录为 34,653 条 PE/info，水位 41/41 | 基线已验证 | 增加真实 E3D 增删改和 plant-ui 视觉用例 |
| 8000 | FTUB/BRAN、EQUI、SUPPO、GENSEC、目录闭包、按需生成、位置移动 | 类型覆盖较多；按需生成和目录依赖验证通过 | 在 plant-ui 补齐结构类和首次按需加载截图 |

说明：7999 的数字来自本任务执行记录，目前尚无单独的 7999 evidence 文件；数据库恢复在线后应使用完整性脚本重新固化一次结果。

## 4. 已测试的增量更新场景

### 4.1 管道、设备及通用属性

| 场景 | 样本/会话 | 已验证结果 | 层级 |
|---|---|---|---|
| EQUI 位置移动 | 7997 `24381/100677`，session 77～80 | 识别为 TransformOnly；实例世界变换和 AABB 更新；模型树根保持正确 | B/C/D-旧 |
| BRAN 更新 | 7997 `24381/100817` | BRAN 作为最小交付单元重新生成，rs-plant3-d 有历史结果截图 | B/D-旧 |
| FTUB 位置移动 | 8000 `24384/22403`，session 26→27 | FTUB 不作为根；所属 BRAN 更新；位置和 AABB 均移动 100 mm；水位推进 | B/C/D-旧 |
| FTUB 跨 BRAN 移动 | 8000 session 31～32 | 旧 BRAN 与新 BRAN 均重新生成；恢复会话也正确 | B/C |
| BRAN 子元件重排 | 8000 session 33～34 | BRAN 重新生成；顺序恢复后仍幂等 | B/C |
| DAMP 名称修改 | 7997 session 82 | 分类为 DataOnly；名称和水位更新，不触发模型任务 | B/C |
| SPRE/等级库修改 | 单元测试和反向级联实库测试 | 分类为 CATA/共享引用级联；消费者根重新生成 | A/B |
| CACHID/LCHKDA 修改 | 属性分类测试 | 已修正为 DataOnly，不再误触发模型生成 | A |
| NCYL 负向几何变化 | 7997 `24381/100680` | 子元件变化正确提升到 EQUI `24381/100677` 重新生成 | B |
| HANG 按需生成 | 7997 `24381/177948` | HANG 最小交付单元生成成功 | B |

### 4.2 真实增删改

| 操作 | 样本/会话 | 已验证结果 | 层级 |
|---|---|---|---|
| 删除 | 7997 session 84，删除 VTWA `24381/107146` | `pe.deleted` 置位；实例和 owner 关系删除；BRAN 子件 46→45；水位 83→84；重复执行为 up-to-date | B/C |
| 新增 | 结构 GENSEC Add 场景 | 新增构件被扫描，归并到 SUPPO/结构根并生成 | B/C |
| 修改位置 | EQUI、FTUB | 只更新变换时不做不必要的完整几何重建；实例/AABB 正确变化 | B/C/D-旧 |
| 修改几何 | BOX.XLEN、WALL.JUSL、FRAD、HEIG、DESP | 分类为 DirectGeometry 或根级几何更新；目标最小交付单元重新生成 | A/B/C |
| 修改名称 | DAMP.NAME | 数据和树节点名称更新，不产生模型任务 | B/C |
| 子件重排 | FTUB/BEND 顺序 | 所属 BRAN 重新生成，顺序恢复可重复执行 | B/C |

### 4.3 结构专业

| 类型 | 样本 | 已验证结果 | 层级 |
|---|---|---|---|
| FLOOR | 7997 `24381/180272` | 正确提升到 CFLOOR `24381/180271`；真实按需生成成功 | A/B |
| WALL | 7997 `24381/180032` | 正确提升到 CWALL `24381/180031`；JUSL 修改触发模型更新 | A/B/C |
| STWALL | 7997 `24381/180037` | 真实生成，有限 AABB/网格 | B |
| GWALL | 7997 `24381/180703` | 真实生成，有限 AABB/网格 | B |
| GENSEC/BEAM | 8000 `24384/25743`、`24384/25888`、`24384/29771` | GENSEC 不是最小交付单元；正确归并到 SUPPO；实例和网格生成成功 | A/B |
| GENSEC/BOX | 8000 `24384/25923` | BOX 变体生成成功，归并到 SUPPO `24384/25872` | A/B |
| SUPPO | 多个 8000 样本 | SUPPO 作为结构最小交付单元，按需生成成功 | A/B |

结构测试中已修复并覆盖：

- PAVE/VERT 隐式 noun 和缺失 owner 的兼容处理。
- 极端圆角、OCC fillet、SPINE 端法向和 SweepSolid 默认递归问题。
- FLOOR/WALL 子构件向 CFLOOR/CWALL 根提升。
- GENSEC、FTUB 等隐藏结构子件不被误判为最小交付单元。
- 目录闭包缺失 SEXT/NSEX/SREV/NSRE/SLOO 导致的生成不完整问题。

### 4.4 按需生成与 CATA 依赖

| 场景 | 已验证结果 | 层级 |
|---|---|---|
| BRAN 首次请求 | 无模型时返回 Generated；重复请求返回 AlreadyAvailable | B |
| FLOOR/WALL 首次请求 | 自动选择 CFLOOR/CWALL 根并生成 | B |
| SUPPO/GENSEC 首次请求 | 自动选择 SUPPO 根并生成 | B |
| 同一根并发请求 | 根级锁避免重复生成 | A/B |
| CATA 依赖缺失 | 自动补解析目录闭包；SVER 0→5181、SLOO 0→198 | B |
| 强制重新生成 | 104 个根执行；可渲染实例达到 1528/1535（99.5%） | B |

这里验证的是“模型按需生成时自动解析所需 CATA 依赖”。直接修改 CATA 源文件后触发全量消费者更新，尚未作为当前产品范围内的完整视觉验收用例。

## 5. D-01～D-15 完整矩阵

| 编号 | 场景 | 数据/模型验证 | plant-ui 视觉验证 | 当前状态 |
|---|---|---|---|---|
| D-01 | 共享 SPCO 修改，级联多个消费者 | 72 个 DAMP 反向消费者，67 个 BRAN 生成 | 待拍队列、树和三维前后图 | 后端通过，前端待复验 |
| D-02 | 子元件跨最小交付单元移动 | 真实 E3D 会话通过，旧/新根均更新 | 待确认两个根都自动刷新 | 后端通过，前端待复验 |
| D-03 | 删除有几何子元件 | 真实 session 84 删除通过，实例和关系清理正确 | 待拍树节点和几何同时消失 | 后端通过，前端待复验 |
| D-04 | 新增嵌套几何元件 | GENSEC Add 路由和生成通过 | 待拍树节点和几何同时出现 | 后端通过，前端待复验 |
| D-05 | FLOOR/PAVE 修改 | CFLOOR 路由及真实生成通过 | 待拍结构属性修改前后图 | 后端通过，前端待复验 |
| D-06 | WALL/GENSEC 修改 | WALL.JUSL、CWALL、GENSEC 变体通过 | 待拍 CWALL/SUPPO 自动刷新 | 后端通过，前端待复验 |
| D-07 | SUPPO 参数修改 | SUPPO/GENSEC 根选择和生成通过 | 待真实参数会话、队列和截图 | 会话与前端均待补 |
| D-08 | 负向几何参数 | NCYL 变化提升到 EQUI 并生成 | 待拍负值几何正确显示 | 后端通过，前端待复验 |
| D-09 | 子元件重排 | 真实 session 33～34 通过 | 待拍树顺序与三维结果 | 后端通过，前端待复验 |
| D-10 | NAME 数据更新 | 名称更新且无模型任务 | 待拍树名称变化、几何保持不变 | 后端通过，前端待复验 |
| D-11 | POS 变换更新 | FTUB 位移、AABB、水位和 BRAN 更新均通过 | 只有 rs-plant3-d 历史图；plant-ui 待重拍 | 后端通过，前端待复验 |
| D-12 | 缺模型时首次请求 | BRAN/FLOOR/WALL/GENSEC 服务端按需生成通过 | 待验证 plant-ui 显示操作能自动触发 ensure | 服务端通过，客户端链路待验证 |
| D-13 | 反向索引级联中断恢复 | durable CascadeExpand 恢复通过 | 待重连后自动刷新和最终截图 | 后端通过，前端待复验 |
| D-14 | 后端中断/前端重开 | 队列和水位恢复通过 | 待验证任务队列降级、重连和最终刷新 | 后端通过，前端待复验 |
| D-15 | 重复执行幂等 | `pe_owner`、finalize 和重复执行通过 | 待拍第二次执行无树/几何抖动 | 后端通过，前端待复验 |

结论：切换到 plant-ui 后，当前没有任何场景可以直接标记为新的 D 级闭环。D-11 的旧版截图作为基线保留，但必须用 plant-ui 重拍。

## 6. 自动化测试与当前复测状态

### 6.1 已保存的历史结果

| 测试范围 | 已保存结果 |
|---|---|
| 后端默认单元测试 | 190 passed，0 failed，45 ignored |
| rs-plant3-d 手动更新聚焦测试（旧前端历史结果） | 10 passed，0 failed |
| 属性字典/更新策略检查 | 12 passed，0 failed，8 ignored |
| 按需生成与 CATA 闭包 | 12 passed |
| 模型删除/替换 | 4/4 passed |

`ignored` 测试主要依赖真实 SurrealDB、E3D 会话或特定实库数据，不能与纯单元测试混为一谈。

### 6.2 2026-07-29 当前分支复测

命令：

```powershell
Set-Location D:\work\plant-code\old\gen-model
rtk cargo test -j 2 --lib
```

结果：编译失败，未进入测试执行。4 个错误均位于 `src/data_interface/manual_update.rs` 的测试代码：

1. `UnitTask` 已无 `revision` 字段，但测试仍在构造时赋值。
2. `PendingModelUnit` 已无 `revision` 字段，但测试仍在构造时赋值。
3. 测试仍读取 `five.revision`。
4. 测试仍读取 `seven.revision`。

因此历史 190/190 结果仍是有效的已保存证据，但不代表 2026-07-29 当前工作树仍能通过同一套测试。修正字段契约后必须重新运行完整测试。

### 6.3 实库完整性复测

数据库在线后运行：

```powershell
Set-Location D:\work\plant-code\old\gen-model
rtk powershell -ExecutionPolicy Bypass -File .\scripts\Test-AmsDbnumIntegrity.ps1
```

2026-07-29 执行时 `127.0.0.1:8009` 无法连接，所以没有覆盖 2026-07-27 的已保存实库结果。

## 7. plant-ui 新测试方案

### 7.1 前置条件

新方案固定使用以下三部分，不混用 release 包自带数据库或旧前端：

| 部分 | 固定入口 | 判据 |
|---|---|---|
| SurrealDB | `D:\work\plant-code\old\gen-model\bin\surreal.exe` | 监听 8009，数据目录为本项目 `.surreal\ams-8009` |
| 模型服务 | 当前 gen-model 的 `aios-database` | `DbOption.toml` 的 `http_api_addr` 与 plant-ui 完全一致 |
| 前端 | `D:\work\plant-code\old\plant-ui` | 同时连接 8009 数据库和 gen-model REST/WebSocket |

不要直接使用 `plant-ui\release\plant-suite-0.1.4\Start-Plant.ps1` 执行本轮验收。该脚本会启动 release 包内自带的 `backend\bin\surreal.exe` 和数据库目录，不符合本轮必须使用 gen-model `bin\surreal.exe` 的约束。

当前 gen-model `DbOption.toml` 记录的模型服务地址是 8022，而 plant-ui 出厂默认值是 8021。启动时必须显式统一，不能依赖默认值。

### 7.2 启动测试栈

终端一，启动指定数据库：

```powershell
Set-Location D:\work\plant-code\old\gen-model
.\bin\surreal.exe start --user root --pass root --bind 127.0.0.1:8009 `
  rocksdb:D:/work/plant-code/old/gen-model/.surreal/ams-8009
```

终端二，启动模型服务：

```powershell
Set-Location D:\work\plant-code\old\gen-model
rtk cargo run --release
```

终端三，启动带验收探针的 plant-ui：

```powershell
Set-Location D:\work\plant-code\old\plant-ui
$env:PLANT_MODEL_API_URL = "http://127.0.0.1:8022"
$env:EGUI_INSPECTION = "1"
rtk cargo run -p plant-ui-app --bin plant-ui-app
```

启动后先检查：

```powershell
curl.exe -s http://127.0.0.1:8022/api/v1/health
curl.exe -s http://127.0.0.1:8022/api/v1/dbnums
curl.exe -s http://127.0.0.1:8022/api/v1/queue
```

plant-ui 配置中的 database、namespace、MDB 和 `model_api_url` 必须与服务端身份一致。当前增量执行范围由 MDB 声明的 DESI 库决定，不再由手写 dbnum 列表决定。

### 7.3 每个增量用例的固定步骤

每个 D-01～D-15 场景都按同一顺序执行：

1. 在 plant-ui 模型树定位目标最小交付单元，展开相关父子节点并显示模型。
2. 截取修改前画面，必须同时看见模型树目标节点和三维模型。
3. 在 E3D 修改目标属性并保存新 session。
4. 在 plant-ui 打开“模型增量更新”，执行预览。
5. 核对预览中的 dbnum、session 区间、增/改/删数量、模型影响数量和最小交付单元。
6. 确认执行，核对回执进入任务队列；不直接调用旧版查看器刷新命令。
7. 在任务队列观察应用中、生成中和终态；保存一张队列过程截图。
8. 等待队列清空和欠账单元收敛，确认模型树与三维视图在不手动重载的情况下自动更新。
9. 截取修改后画面，使用与修改前相同的树展开状态、相机角度和可见集合。
10. 查询数据库并记录水位、实例/AABB、关系或删除状态；重复执行一次，确认 up-to-date 且画面无抖动。

对 DataOnly 场景，正确结果是模型树文字更新、三维几何保持不变、任务队列没有模型生成单元。对删除场景，树节点和几何必须同时消失。对跨根移动场景，旧根和新根必须都刷新。

### 7.4 plant-ui 截图与控件操作

plant-ui 原生端提供 `inspect` 验收探针，应用必须带 `EGUI_INSPECTION=1` 启动。探针不抢窗口焦点，可读取控件树、注入点击并保存 PNG。

```powershell
Set-Location D:\work\plant-code\old\plant-ui

# 查找“模型增量更新”、目标 refno 或任务队列控件
rtk cargo run -p plant-ui-app --bin inspect -- tree 模型增量更新
rtk cargo run -p plant-ui-app --bin inspect -- tree 24381/100677
rtk cargo run -p plant-ui-app --bin inspect -- tree 任务队列

# 保存截图
New-Item -ItemType Directory -Force `
  D:\work\plant-code\old\gen-model\output\plant-ui-increment | Out-Null
rtk cargo run -p plant-ui-app --bin inspect -- shot `
  D:\work\plant-code\old\gen-model\output\plant-ui-increment\D11-before.png
```

需要注入点击时，将同一次 `inspect tree` 返回的实际逻辑坐标传给 `inspect click`，不把坐标固化到测试文档。每个用例至少保存：

- `Dxx-before.png`：修改前模型树和三维模型。
- `Dxx-queue.png`：任务队列中的 dbnum、session 和生成状态。
- `Dxx-after.png`：队列完成后的模型树和三维模型。
- `Dxx-repeat.png`：重复执行后画面无变化；如果与 after 完全一致，可保存图像哈希代替第四张图。

### 7.5 plant-ui 自动化门禁

在开始实机截图前运行：

```powershell
Set-Location D:\work\plant-code\old\plant-ui
rtk cargo test --workspace
```

重点门禁包括：

- 模型更新预览、入队回执和失败包封解码。
- 当前项目/MDB 身份闸门。
- 队列暂停、恢复、合并、断线降级和任务行稳定 ID。
- 队列清空后模型加载债务收敛。
- 模型树稳定 ID、懒加载和目标定位。
- `plant-ui-view3d` 模型装载、选择和视口刷新。

当前源码已经实现 `/api/v1/update/preview`、`/api/v1/update/execute`、`/api/v1/queue`、`/api/v1/ws` 以及队列完成后的模型自动重载逻辑。

按需生成需要单独卡口：plant-ui 文档已经规定显示缺失模型应调用 `POST /api/v1/model/ensure`，但 2026-07-29 当前源码中未找到 `ensure_model` 的实际调用。D-12 必须先用实机证明“点击显示缺失模型会发出 ensure 请求”；如果没有请求，应先补齐客户端接线，再进行截图验收，不能只用服务端接口通过代替。

### 7.6 新方案的通过标准

一个用例只有同时满足以下条件才标记完成：

- plant-ui 预览范围和目标 dbnum 正确。
- 任务进入队列并到达正确终态，无未解释的欠账单元。
- 数据水位、PE、关系、实例和 AABB 与预期一致。
- 模型树在队列完成后自动反映新增、删除、改名或重排。
- 三维视图在队列完成后自动反映生成、删除、移动或几何变化。
- before/queue/after 证据齐全，且 after 不是通过重启前端或手动全量重载得到。
- 重复执行返回 up-to-date，树和三维模型无重复、无残影、无位置抖动。

## 8. 可重复执行入口

| 目的 | 入口 |
|---|---|
| 初始化 AMS dbnum | `src/bin/initialize_ams_dbnums.rs` |
| 检查 7997/7999/8000 完整性 | `scripts/Test-AmsDbnumIntegrity.ps1` |
| 枚举 7997 最小交付单元 | `scripts/Get-7997RootCover.ps1` |
| 验证 7997 模型生成 | `scripts/Verify-7997Generation.ps1` |
| 手动扫描探针 | `src/bin/manual_scan_probe.rs` |
| 手动执行探针 | `src/bin/manual_exec_probe.rs` |
| E3D 名称修改/恢复 | `scripts/e3d/projams_incr_name_apply.mac`、`scripts/e3d/projams_incr_name_restore.mac` |
| E3D 删除验证 | `scripts/e3d/projams_incr_delete.mac`、`scripts/e3d/projams_d03_probe.mac` |
| E3D 批量执行 | `scripts/e3d/projams_run_batch.mac` |

运行真实增量用例前必须确认使用本项目 `bin\surreal.exe`，并核对数据库地址、namespace、database 和目标 dbnum，避免误连其他 SurrealDB 版本或其他项目实例。

## 9. 证据文件

| 内容 | 文件 |
|---|---|
| 7997/8000 全量增量验证主证据 | `docs/evidence/2026-07-27-projams-incremental-update-validation.md` |
| 8000 按需生成与 CATA 闭包 | `docs/evidence/2026-07-27-ondemand-generation-ams8000.md` |
| 7997 真实删除 session 84 | `docs/evidence/2026-07-27-d03-delete-session-baseline.md` |
| FLOOR/WALL 结构验证 | `docs/2026-07-25_test-structure-floor-wall-incremental-report.md` |
| GENSEC/SUPPO 验证 | `docs/2026-07-25_test-structure-gensec-on-demand-report.md` |
| core.dll 对齐与早期截图 | `docs/2026-07-24_test-core-dll-incremental-alignment-report.md` |
| D-01～D-15 完整计划 | `docs/2026-07-25_test-plan-core-dll-model-update-complete-matrix-v2.md` |
| plant-ui 真服务验收步骤 | `..\..\plant-ui\docs\plans\queue-live-acceptance.md` |
| plant-ui 显示时按需生成设计 | `..\..\plant-ui\docs\adr\0009-generate-on-show-goes-through-the-service.md` |
| plant-ui 模型更新 API | `..\..\plant-ui\crates\plant-ui-app\src\model_update_api.rs` |
| plant-ui 队列完成后刷新逻辑 | `..\..\plant-ui\crates\plant-ui-app\src\main.rs` |
| plant-ui 截图探针 | `..\..\plant-ui\crates\plant-ui-app\src\bin\inspect.rs` |
| FTUB 位移旧版截图说明 | `output/increment-test/README.md` |
| FTUB 位移旧版前图 | `output/increment-test/rs-plant3-d-before.png` |
| FTUB 位移旧版后图 | `output/increment-test/rs-plant3-d-after.png` |
| EQUI 位移旧版前图 | `output/increment-test/db7997-equi-24381_100677-before.png` |
| EQUI session 80 旧版后图 | `output/increment-test/db7997-equi-24381_100677-session80-after.png` |
| BRAN session 81 旧版后图 | `output/increment-test/db7997-bran-24381_100817-session81-after.png` |

## 10. 待验证与修复清单

按优先级排序：

1. 修复 `manual_update.rs` 测试中的 `revision` 字段契约，恢复当前分支完整单元测试可运行。
2. 启动指定的 `bin\surreal.exe`，重新执行 7997/7999/8000 完整性脚本并保存结果。
3. 先运行 plant-ui `cargo test --workspace`，固化新前端自动化基线。
4. 为 D-01～D-15 补齐 plant-ui 的 before/queue/after 证据；旧版截图不抵扣新前端验收。
5. 为 7999 增加至少一组真实 E3D 增、删、改用例，而不只验证基线完整性。
6. 补齐 SUPPO 参数修改、FLOOR/PAVE 结构属性修改和 GENSEC 新增的真实 E3D 会话截图。
7. 验证并按需修复 plant-ui 的“显示缺失模型 → `/api/v1/model/ensure`”客户端接线。
8. 保留 7 个退化 ANCI 源几何作为已知数据问题，除非后续需求明确要求为零退化模型。

## 11. 验收口径

当前可以确认：

- 增量分类和最小交付单元规则已有系统性自动化覆盖。
- 主要管道、设备、结构和目录依赖类型已有真实数据库生成证据。
- E3D 增、删、改、移动、重排、名称修改和共享引用级联均已有实测。
- 队列恢复、水位、重复执行和模型删除/替换机制已有验证。

当前仍不能确认：

- 所有 D-01～D-15 场景均已在 plant-ui 中完成 before/queue/after 验收。
- 7999 已覆盖真实 E3D 增删改。
- 2026-07-29 当前工作树完整测试全绿。
- plant-ui 首次显示缺失模型时会自动调用 `/api/v1/model/ensure`。
