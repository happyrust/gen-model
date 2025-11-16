# handlers.rs 重构计划

## 当前状态
- **总行数**: 7,479 行
- **函数数量**: ~100 个函数
- **问题**: 严重违反代码规范（规范要求 ≤250 行，违规 30 倍）

## 功能模块分析

基于函数名称分析，识别出以下功能模块：

### 1. **端口管理模块** (Port Management)
- `check_port_usage` (44-60)
- `kill_port_processes` (63-94)
- `check_port_status` (97-123)
- `kill_port_processes_api` (125-...)
- `is_port_in_use` (3572-...)
- `is_addr_listening` (3557-...)
- **预估行数**: ~200 行

### 2. **项目管理模块** (Project Management)
- `ensure_projects_schema` (354-...)
- `api_get_projects` (363-...)
- `api_create_project` (653-...)
- `api_get_project` (763-...)
- `api_update_project` (813-...)
- `api_delete_project` (958-...)
- `api_healthcheck_project` (990-...)
- `api_projects_demo` (1106-...)
- `projects_health_scheduler` (1184-...)
- **预估行数**: ~800 行

### 3. **任务管理模块** (Task Management)
- `get_tasks` (1243-...)
- `get_task` (1276-...)
- `get_task_error_details` (1294-...)
- `get_task_logs` (1327-...)
- `create_task` (1385-...)
- `start_task` (1411-...)
- `stop_task` (1446-...)
- `restart_task` (1469-...)
- `delete_task` (1540-...)
- `get_next_task_number` (1566-...)
- `get_task_templates` (1981-...)
- `create_batch_tasks` (2097-...)
- `execute_real_task` (3903-...)
- `execute_parse_pdms_task` (4606-...)
- **预估行数**: ~1200 行

### 4. **配置管理模块** (Configuration Management)
- `get_config` (1582-...)
- `update_config` (1588-...)
- `get_config_templates` (1602-...)
- `get_available_databases` (1612-...)
- **预估行数**: ~150 行

### 5. **部署站点管理模块** (Deployment Sites)
- `ensure_deployment_sites_schema` (2156-...)
- `api_get_deployment_sites` (2165-...)
- `api_import_deployment_site_from_dboption` (2284-...)
- `api_create_deployment_site` (2457-...)
- `api_get_deployment_site` (2562-...)
- `api_update_deployment_site` (2577-...)
- `api_delete_deployment_site` (2653-...)
- `api_browse_deployment_site_directory` (2770-...)
- `api_create_deployment_site_task` (2948-...)
- `api_healthcheck_deployment_site` (3007-...)
- `api_healthcheck_deployment_site_post` (3058-...)
- `api_export_deployment_site_config` (3070-...)
- **预估行数**: ~900 行

### 6. **SurrealDB 服务管理模块** (SurrealDB Server)
- `get_system_status` (3109-...)
- `start_surreal_server` (3162-...)
- `start_surreal_process_improved` (3224-...)
- `stop_surreal_server` (3449-...)
- `restart_surreal_server` (3649-...)
- `get_surreal_status` (3708-...)
- `test_surreal_connection` (3752-...)
- `run_remote_ssh` (3612-...)
- `test_tcp_connection` (3589-...)
- `test_database_functionality` (3602-...)
- `command_exists` (3579-...)
- **预估行数**: ~600 行

### 7. **数据库状态管理模块** (Database Status)
- `get_db_status_list` (4916-...)
- `get_db_status_detail` (4997-...)
- `execute_incremental_update` (5020-...)
- `set_update_finalize` (5097-...)
- `check_file_versions` (5126-...)
- `convert_to_db_status` (5160-...)
- `check_model_status` (5239-...)
- `check_mesh_status` (5266-...)
- `get_file_version_info` (5276-...)
- `check_single_file_version` (5305-...)
- **预估行数**: ~500 行

### 8. **数据库连接管理模块** (Database Connection)
- `check_database_connection` (6106-...)
- `get_startup_scripts` (6149-...)
- `start_database_instance` (6199-...)
- `check_surrealdb_connection` (6258-...)
- `start_surreal_with_script` (6294-...)
- `create_default_startup_script` (6320-...)
- `handle_database_connection_error` (6348-...)
- `run_database_diagnostics_api` (6437-...)
- **预估行数**: ~350 行

### 9. **空间查询模块** (Spatial Query)
- `sqlite_spatial_page` (1695-...)
- `api_sqlite_spatial_rebuild` (1708-...)
- `spatial_query_page` (1826-...)
- `api_sqlite_spatial_query` (1833-...)
- `api_space_suppo_trays` (5436-...)
- `api_space_fitting` (5446-...)
- `api_space_wall_distance` (5456-...)
- `api_space_fitting_offset` (5468-...)
- `api_space_steel_relative` (5480-...)
- `api_space_tray_span` (5492-...)
- `api_sqlite_tray_supports_detect` (5591-...)
- **预估行数**: ~450 行

### 10. **导出管理模块** (Export Management)
- `create_export_task` (6749-...)
- `execute_export_task` (6822-...)
- `get_export_status` (6940-...)
- `download_export` (6971-...)
- `list_export_tasks` (7038-...)
- `cleanup_export_tasks` (7079-...)
- **预估行数**: ~350 行

### 11. **模型生成模块** (Model Generation)
- `api_generate_by_refno` (7113-...)
- `execute_refno_model_generation` (7188-...)
- `update_room_relations_for_refnos_incremental` (7380-...)
- `batch_update_room_relations` (7444-...)
- `update_room_relations_for_refnos` (7475-...)
- **预估行数**: ~300 行

### 12. **SCTN 测试模块** (SCTN Testing)
- `sctn_test_page` (5730-...)
- `api_sctn_test_run` (5814-...)
- `api_sctn_test_result` (5838-...)
- `run_sctn_test_pipeline` (5845-...)
- `finish_fail` (6051-...)
- **预估行数**: ~350 行

### 13. **页面渲染模块** (Page Rendering)
- `index_page` (5343-...)
- `dashboard_page` (5348-...)
- `config_page` (5352-...)
- `tasks_page` (5356-...)
- `task_detail_page` (5366-...)
- `task_logs_page` (5376-...)
- `batch_tasks_page` (5386-...)
- `xkt_test_page` (5397-...)
- `deployment_sites_page` (5409-...)
- `wizard_page` (5414-...)
- `space_tools_page` (5422-...)
- `tray_supports_page` (5504-...)
- `db_status_page` (5331-...)
- `serve_incremental_update_page` (1957-...)
- `serve_database_status_page` (1969-...)
- `database_connection_page` (6454-...)
- `spatial_visualization_page` (6465-...)
- **预估行数**: ~1400 行

## 重构方案

### 目标结构

```
src/web_server/
├─ handlers/
│  ├─ mod.rs                        (模块导出，~50 行)
│  ├─ port.rs                       (端口管理，~200 行) ✅
│  ├─ project.rs                    (项目管理，~400 行) ⚠️ 需要拆分
│  ├─ task.rs                       (任务管理，~500 行) ⚠️ 需要拆分
│  ├─ config.rs                     (配置管理，~150 行) ✅
│  ├─ deployment_site.rs            (部署站点，~450 行) ⚠️ 需要拆分
│  ├─ surreal_server.rs             (SurrealDB 服务，~600 行) ⚠️ 需要拆分
│  ├─ database_status.rs            (数据库状态，~500 行) ⚠️ 需要拆分
│  ├─ database_connection.rs       (数据库连接，~350 行) ⚠️ 需要拆分
│  ├─ spatial_query.rs              (空间查询，~450 行) ⚠️ 需要拆分
│  ├─ export.rs                     (导出管理，~350 行) ⚠️ 需要拆分
│  ├─ model_generation.rs           (模型生成，~300 行) ⚠️ 需要拆分
│  ├─ sctn_test.rs                  (SCTN 测试，~350 行) ⚠️ 需要拆分
│  └─ pages.rs                      (页面渲染，~700 行) ⚠️ 需要拆分
├─ handlers.rs → 删除 (迁移完成后)
└─ ... (其他现有文件)
```

### 进一步拆分大文件

对于超过 250 行的模块，需要进一步拆分为子模块：

#### project.rs → handlers/project/
```
handlers/project/
├─ mod.rs           (模块导出，~30 行)
├─ crud.rs          (CRUD 操作，~250 行)
├─ schema.rs        (Schema 管理，~50 行)
├─ health.rs        (健康检查，~100 行)
└─ demo.rs          (Demo 数据，~100 行)
```

#### task.rs → handlers/task/
```
handlers/task/
├─ mod.rs           (模块导出，~30 行)
├─ crud.rs          (CRUD 操作，~200 行)
├─ execution.rs     (任务执行，~250 行)
├─ batch.rs         (批量任务，~150 行)
└─ templates.rs     (任务模板，~100 行)
```

#### deployment_site.rs → handlers/deployment_site/
```
handlers/deployment_site/
├─ mod.rs           (模块导出，~30 行)
├─ crud.rs          (CRUD 操作，~250 行)
├─ import.rs        (导入功能，~180 行)
├─ browse.rs        (目录浏览，~180 行)
└─ health.rs        (健康检查，~100 行)
```

#### surreal_server.rs → handlers/surreal_server/
```
handlers/surreal_server/
├─ mod.rs           (模块导出，~30 行)
├─ lifecycle.rs     (启动/停止/重启，~250 行)
├─ status.rs        (状态查询，~150 行)
└─ utils.rs         (工具函数，~170 行)
```

#### database_status.rs → handlers/database_status/
```
handlers/database_status/
├─ mod.rs           (模块导出，~30 行)
├─ query.rs         (状态查询，~200 行)
├─ update.rs        (增量更新，~150 行)
└─ check.rs         (版本检查，~120 行)
```

#### spatial_query.rs → handlers/spatial_query/
```
handlers/spatial_query/
├─ mod.rs           (模块导出，~30 行)
├─ api.rs           (API 接口，~250 行)
└─ detection.rs     (支架检测，~170 行)
```

#### pages.rs → handlers/pages/
```
handlers/pages/
├─ mod.rs           (模块导出，~30 行)
├─ core.rs          (核心页面，~150 行)
├─ task.rs          (任务页面，~100 行)
├─ database.rs      (数据库页面，~100 行)
├─ spatial.rs       (空间工具页面，~150 行)
└─ test.rs          (测试页面，~150 行)
```

### 最终目录结构

```
src/web_server/
├─ handlers/
│  ├─ mod.rs                        (~50 行)
│  ├─ port.rs                       (~200 行) ✅
│  ├─ config.rs                     (~150 行) ✅
│  ├─ export.rs                     (~250 行) ✅
│  ├─ model_generation.rs           (~250 行) ✅
│  ├─ sctn_test.rs                  (~250 行) ✅
│  ├─ database_connection.rs        (~250 行) ✅
│  ├─ project/
│  │  ├─ mod.rs
│  │  ├─ crud.rs
│  │  ├─ schema.rs
│  │  ├─ health.rs
│  │  └─ demo.rs
│  ├─ task/
│  │  ├─ mod.rs
│  │  ├─ crud.rs
│  │  ├─ execution.rs
│  │  ├─ batch.rs
│  │  └─ templates.rs
│  ├─ deployment_site/
│  │  ├─ mod.rs
│  │  ├─ crud.rs
│  │  ├─ import.rs
│  │  ├─ browse.rs
│  │  └─ health.rs
│  ├─ surreal_server/
│  │  ├─ mod.rs
│  │  ├─ lifecycle.rs
│  │  ├─ status.rs
│  │  └─ utils.rs
│  ├─ database_status/
│  │  ├─ mod.rs
│  │  ├─ query.rs
│  │  ├─ update.rs
│  │  └─ check.rs
│  ├─ spatial_query/
│  │  ├─ mod.rs
│  │  ├─ api.rs
│  │  └─ detection.rs
│  └─ pages/
│     ├─ mod.rs
│     ├─ core.rs
│     ├─ task.rs
│     ├─ database.rs
│     ├─ spatial.rs
│     └─ test.rs
└─ ... (其他现有文件)
```

**总文件数**: ~40 个文件，所有文件均 ≤250 行 ✅

## 实施步骤

### 阶段 1：创建基础结构（1-2 小时）
1. 创建 `src/web_server/handlers/` 目录
2. 创建所有子目录结构
3. 创建所有 `mod.rs` 文件（仅声明，暂不实现）

### 阶段 2：迁移简单模块（3-4 小时）
优先迁移行数较少、依赖较少的模块：
1. ✅ `port.rs` (~200 行)
2. ✅ `config.rs` (~150 行)
3. ✅ `export.rs` (~250 行)
4. ✅ `model_generation.rs` (~250 行)
5. ✅ `sctn_test.rs` (~250 行)
6. ✅ `database_connection.rs` (~250 行)

### 阶段 3：拆分复杂模块（8-10 小时）
按子目录逐个拆分：
1. `project/` 目录（~4 个文件）
2. `task/` 目录（~5 个文件）
3. `deployment_site/` 目录（~5 个文件）
4. `surreal_server/` 目录（~4 个文件）
5. `database_status/` 目录（~4 个文件）
6. `spatial_query/` 目录（~3 个文件）
7. `pages/` 目录（~6 个文件）

### 阶段 4：更新路由配置（1 小时）
1. 修改主路由文件，导入新的 handlers 模块
2. 确保所有路由路径正确

### 阶段 5：测试验证（2-3 小时）
1. 编译检查（`cargo check`）
2. 功能测试（手动测试关键 API）
3. 回归测试（确保无功能退化）

### 阶段 6：清理（30 分钟）
1. 删除原 `handlers.rs` 文件
2. 更新文档和注释
3. 提交代码

## 预估总工作量
- **总时间**: 15-20 小时
- **建议分配**: 2-3 个工作日

## 风险和对策

### 风险 1：依赖关系复杂
- **对策**: 先分析依赖，创建 `utils.rs` 存放共享工具函数

### 风险 2：编译错误
- **对策**: 每迁移一个模块就编译检查，及时发现问题

### 风险 3：功能回退
- **对策**: 保留原文件备份，测试通过后再删除

## 下一步行动
1. 确认重构方案
2. 开始阶段 1：创建基础结构
