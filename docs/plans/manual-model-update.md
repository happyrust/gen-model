# 手动模型更新实施计划

依据：

- `docs/specs/manual-model-update.md`
- `docs/adr/ADR-001-dbnum-update-state.md`
- 根目录 `CONTEXT.md`

## 实施原则

- 不新增第三方依赖。
- 复用 `DbOption`、`SesnoRangeResolver`、`IncrementPipeline`、现有模型生成函数和 Bevy 显式事件。
- 不复制自动更新的解析逻辑；预览与执行共用同一套范围解析和变化归并函数。
- 不使用 SurrealDB LIVE 查询。
- 后端先形成稳定公共 API，前端只负责触发、展示状态和显式刷新。

## 阶段 0：移除数据库 LIVE 订阅

状态：已完成

前端已删除：

- `src/live/pe_live.rs`
- `src/live/geom_live.rs`
- `src/live/mod.rs`
- LIVE 初始化 SystemSet、资源和 `geom_live` 项目配置

验证：

- 全仓不存在 `LIVE SELECT`、`Notification` 流和 LIVE 专用资源引用。
- `git diff --check` 通过。
- 当前全量 `cargo check --lib` 被仓库既有的 `wgpu-hal` 与 `windows` 0.54/0.58 依赖冲突阻断；实施期间需先使用项目已有可编译锁定环境或单独修复该基线问题。

## 阶段 1：建立权威 DBNUM 状态

状态：已完成（`src/data_interface/dbnum_state.rs` + 单测）

后端主要文件：

- `src/data_interface/sesno_range.rs`
- `src/data_interface/increment_pipeline.rs`
- 新增最小的 `src/data_interface/dbnum_state.rs`
- `src/data_interface/mod.rs`

工作：

1. 先修复 `persist_latest_main_data`：任一 SurrealDB 批次写入失败必须返回错误，禁止记录错误后继续推进水位。
2. 定义 `DbnumState`，映射 ADR 中的最小字段。
3. 实现按 `dbnum` 读取、独立刷新观察字段和成功推进水位。
4. 实现一次性旧水位迁移：
   - `dbnum_watermark.sesno`
   - 缺失时才读取 `dbnum_info_table`
5. 将 `SesnoRangeResolver` 改为只读取 `applied_sesno`，并继续使用 `get_nearest_large_sesno` 处理会话号间隙。
6. 将 `IncrementPipeline` 的水位推进改为调用同一状态写入函数。
7. 加入文件回退、缺失、重复和合法路径迁移判定。

最小检查：

- 状态迁移优先级单元测试。
- 扫描只更新观察值、失败不推进应用值的测试。
- 任一主数据批次写入失败时 `applied_sesno` 保持不变的测试。
- `file_latest_sesno < applied_sesno` 被拒绝的测试。

完成条件：

- 新状态建立后，运行路径中不再跨表取最大水位。

## 阶段 2：拆出只读预览管线

状态：已完成（`src/data_interface/manual_update.rs` 的 `preview_manual_update` + `model_impact.rs` + 单测）

后端主要文件：

- `src/data_interface/sesno_range.rs`
- `src/data_interface/increment_pipeline.rs`
- 新增 `src/data_interface/manual_update.rs`
- `src/options.rs`

公共入口建议直接放在 `AiosDBManager` 上，避免为单一实现创建 trait：

```rust
pub async fn preview_manual_update(
    &self,
    project: &str,
) -> anyhow::Result<ManualUpdatePreview>;

pub async fn execute_manual_update(
    &self,
    project: &str,
) -> ManualUpdateResult;
```

工作：

1. 从 `IncrementPipeline::apply_one` 中提取无副作用的变化收集函数：
   - 打开 E3D 文件。
   - 按范围调用 `collect_increment_eles`。
   - 不调用 `update_elements_to_database`。
2. 扫描仅限当前项目目录，不遍历其他 `included_projects`。
3. 生成 `dbnum → sesno` 原始变化视图。
4. 实现纯内存净变化归并：
   - 新增→修改
   - 多次修改
   - 修改→删除
   - 新增→删除
5. 复用 `EleOperationData::is_geometry_update` 和 `is_transform_change` 判定模型影响，不维护第二份属性名单。
6. 在 `DbOptionExt` 中增加“追加最小交付类型”配置，运行时与内置五类取并集并规范化大小写。

最小检查：

- 纯函数测试覆盖每种跨会话合并序列。
- 预览调用前后元素表、模型表和 `applied_sesno` 不变。
- `sync_live=true` 时公共入口拒绝执行。

完成条件：

- 前端无需了解 E3D 文件解析细节即可获得完整预览 DTO。

## 阶段 3：解析 ZONE 和模型交付单元

状态：已完成（`manual_update.rs`：`OwnershipSnapshot`/`build_owner_overlay`/`resolve_zone`/`resolve_delivery_unit`/`build_zone_rollup`，预览 DTO `DbnumPreview.zones` 填充仅限 DESI 库；单测覆盖本阶段全部最小检查。测试运行用 `cargo test --lib --no-default-features --features "ws,gen_model,project_hd"`——默认 occ/manifold 特性生成的测试二进制在本机因 OCCT 7.8 的 `jemalloc.dll`/`tbb12.dll` 运行时依赖缺失无法启动，库本身 `cargo check --lib` 默认特性编译通过）

后端主要文件：

- `src/data_interface/manual_update.rs`
- `src/data_interface/increment_pipeline.rs`
- `src/data_interface/side_effect_pending.rs`

工作：

1. 使用当前活动的 Surreal PE/OWNER 图查询祖先，不启用当前未导出的 `ssc.rs`/Arango 路径。
2. 建立变化范围内的 OWNER 覆盖图，用于预览新增、移动和祖先同时变化的情况。
3. 解析：
   - 更新前 ZONE 与最近交付单元
   - 更新后 ZONE 与最近交付单元
4. 按 `sesno` 保留原始记录，按 ZONE 和交付单元输出去重汇总。
5. 无最小交付单元时回退到 ZONE；两者都未知时输出警告且不生成模型。
6. 删除使用更新前快照；移动同时加入原、新交付单元。

最小检查：

- BRAN/HANG/SUPPO/EQUI 最近祖先选择。
- 自定义追加类型与自定义完整类型集合（`append_delivery_unit_types` / `delivery_unit_types`）。
- 嵌套交付类型只选择最近祖先。
- 跨 ZONE、跨交付单元移动同时影响两端。
- ZONE 未知但最小交付单元已知时仍可生成。

完成条件：

- 规格中的预览层级和计数可完全由后端 DTO 表达。

## 阶段 4：手动执行与待重试

状态：已完成（`manual_update.rs`：`AiosDBManager::execute_manual_update(project, progress)` 重扫并按 dbnum 固定 `end_sesno`、逐 dbnum 复用 `IncrementPipeline::apply`（水位只在其成功路径推进）、更新前先取旧归属快照、DESI 成功批次去重出交付单元后与 `manual_model_pending` 表（每单元一行：dbnum/root/end_sesno/attempts/last_error，(dbnum,root) 键保证只留最新任务）合并逐单元独立生成；两段进度经 `ManualUpdateEvent` 回调输出；预览新增 `pending_model_retries` 且 `up_to_date` 计入待重试。纯逻辑（状态聚合、worklist 合并、预览后合并会话、执行互斥守卫等）已单测）

后端主要文件：

- `src/data_interface/manual_update.rs`
- `src/data_interface/increment_pipeline.rs`
- `src/data_interface/side_effect_pending.rs`
- `src/data_interface/model_refresh.rs`
- `src/fast_model/gen_model.rs`

工作：

1. 执行时重新扫描，并为每个 `dbnum` 固定本次 `end_sesno`。
2. 逐 `dbnum` 复用 `IncrementPipeline` 应用完整范围。
3. 数据成功后才推进 DBNUM 应用水位。
4. 从成功批次汇总并去重交付单元；CATA 和系统库不进入模型生成。
5. 以交付单元根参考号调用现有模型生成路径，不按变化元素逐个生成。
6. 将模型待重试从“整批 changed_refnos”细化为独立交付单元任务。
7. 每个任务保存 `dbnum`、交付单元根、来源 `end_sesno`、尝试次数和最后错误。
8. 同一待重试单元再次受新数据影响时只保留一个最新任务。
9. 输出数据批次和模型交付单元两段进度事件。

实现约束：

- 不直接调用会再次触发旧分类模型刷新的组合入口；手动编排只复用数据持久化、同步副作用和模型生成的底层步骤。
- 外层保持现有顺序执行，模型生成函数内部已有的并发继续复用；没有性能证据前不增加第二层并行。

最小检查：

- 一个 `dbnum` 失败、其他成功。
- 数据成功后模型失败，水位已推进且生成待重试记录。
- 无新会话时可以只重试模型。
- 执行期间新增会话留到下次。

完成条件：

- 任意失败都符合规格中的水位和重试语义。

## 阶段 5：前端菜单、窗口和任务状态

前端主要文件：

- `src/editor_ui/ui_plugin.rs`
- 新增 `src/plugins/e3d_plugin/manual_model_update.rs`
- `src/plugins/e3d_plugin/plugin.rs`
- `Cargo.toml`

工作：

1. 在“项目”菜单加入“更新模型”。
2. 仅在本地 `auto_gen` 构建注册后端调用；Web/远程构建不显示。
3. 复用 `RsTokioRuntime` 运行扫描和执行。
4. 用一个 Bevy Resource 保存项目级任务状态：
   - Idle
   - Scanning
   - Ready
   - Executing
   - Completed
5. 同一项目不重复启动任务；关闭窗口只改变显示状态。
6. 预览树展示 `dbnum → sesno → ZONE → 交付单元`。
7. 执行页分开显示数据批次和模型交付单元状态，不伪造预计百分比。
8. 显示文件异常、ZONE 未知、待重试和预览后合并会话。

完成条件：

- 用户可完成“打开—预览—确认—查看结果—关闭”的完整流程。

## 阶段 6：显式刷新元素树和场景

前端主要文件：

- `src/plugins/e3d_plugin/pdms_events/mod.rs`
- `src/plugins/e3d_plugin/systems/pdms_nodes_system.rs`
- `src/plugins/e3d_plugin/systems/model_system.rs`
- `src/plugins/e3d_plugin/trees/e3d_node_tree.rs`
- `src/plugins/e3d_plugin/manual_model_update.rs`

工作：

1. 模型生成成功后，对当前已加载的交付单元发送 `ShowModelEvent { refresh: true, ... }`。
2. 从树状态读取已展开 OWNER，仅对受影响且已展开的原、新 OWNER 发送 `FetchChildrenNodesEvent`。
3. 未展开分支不预取。
4. 刷新前保存当前选择；刷新后元素仍存在则恢复，否则清空。
5. 不改变相机 Transform。
6. 模型失败时不刷新该单元，保留旧显示。

完成条件：

- 无 LIVE 查询时，手动更新结果仍能准确反映到当前界面。

## 阶段 7：集成与验收

1. 先在 `old/gen-model` 完成后端 API 和测试。
2. 前端开发期可临时使用本地路径覆盖；不要提交机器绝对路径。
3. 后端版本可引用后，再更新前端 `aios-database` Git revision。
4. 使用一个包含以下数据的本地项目验收：
   - 多 `dbnum`
   - 多 `sesno`
   - 元素新增、修改、删除
   - OWNER 和 ZONE 移动
   - 最小交付单元与 ZONE 兜底
   - 一个故意失败的模型单元
5. 按功能规格的 12 条验收标准逐项记录结果。

## 风险与控制

| 风险 | 控制 |
|---|---|
| 主数据写入失败但水位继续推进 | 所有持久化错误向上传播，水位写入只能位于成功路径 |
| 预览和执行之间文件继续变化 | 执行前重扫，批次开始时固定 `end_sesno` |
| 删除后无法查询旧 OWNER | 应用数据前保存归属快照 |
| 祖先也在同一批次移动 | 预览使用 OWNER 覆盖图，执行后用最终数据库状态复核 |
| 旧水位错误推进 | 一次性迁移后只读 `applied_sesno` |
| 模型失败造成数据重复应用 | 水位与模型待重试分离 |
| 自动和手动更新竞争 | 复用 `sync_live`，两种模式互斥 |
| 前端失去数据库推送 | 更新完成后显式刷新，其他外部变化通过重新加载项目获取 |
| 当前 Windows 构建基线失败 | 在功能合并前先固定 `wgpu/windows` 兼容依赖或使用已知可编译锁文件 |
