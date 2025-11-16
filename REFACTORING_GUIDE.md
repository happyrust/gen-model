# handlers.rs 重构指南

## 项目背景

原 `handlers.rs` 文件包含 **7,479 行代码**，严重违反代码规范（要求 ≤250 行）。本文档提供完整的重构指南和已完成的工作。

---

## 已完成的模块（自动化提取）

### 1. port.rs（164 行）✅ 已完成
- **路径**: `src/web_server/handlers/port.rs`
- **功能**: 端口管理（检查、释放）
- **函数**:
  - `check_port_status`
  - `kill_port_api`
  - 辅助函数：`check_port_usage`, `kill_port_processes`, `is_addr_listening`, `test_tcp_connection`

### 2. config.rs（126 行）✅ 已完成
- **路径**: `src/web_server/handlers/config.rs`
- **功能**: 配置管理
- **函数**:
  - `get_config` (1582行)
  - `update_config` (1588)
  - `get_config_templates` (1602)
  - `get_available_databases` (1612)

### 3. export.rs（457 行）✅ 已完成
- **路径**: `src/web_server/handlers/export.rs`
- **功能**: 模型导出管理（GLTF/GLB/XKT）
- **函数**:
  - `create_export_task` (6749)
  - `execute_export_task` (6822)
  - `get_export_status` (6940)
  - `download_export` (6971)
  - `list_export_tasks` (7038)
  - `cleanup_export_tasks` (7079)
- **数据结构**:
  - `ExportRequest`, `ExportResponse`, `ExportStatusResponse`, `ExportProgress`
  - 全局状态：`EXPORT_TASKS`

### 4. model_generation.rs（410 行）✅ 已完成
- **路径**: `src/web_server/handlers/model_generation.rs`
- **功能**: 基于 Refno 的模型生成
- **函数**:
  - `api_generate_by_refno` (7113)
  - `execute_refno_model_generation` (7188)
  - `update_room_relations_for_refnos_incremental` (7380)
  - `batch_update_room_relations` (7444)
  - `update_room_relations_for_refnos` (7475)
- **数据结构**:
  - `RoomUpdateResult`

### 5. sctn_test.rs（382 行）✅ 已完成
- **路径**: `src/web_server/handlers/sctn_test.rs`
- **功能**: SCTN 空间接触测试
- **函数**:
  - `sctn_test_page` (5730)
  - `api_sctn_test_run` (5814)
  - `api_sctn_test_result` (5838)
  - `run_sctn_test_pipeline` (5845)
  - `finish_fail` (6051)
- **数据结构**:
  - `SctnTestRequest`, `SctnTestSnapshot`
  - 全局状态：`SCTN_TEST_RESULTS`
- **特性依赖**: `#[cfg(feature = "sqlite-index")]`

---

## 待处理的模块（需要手动完成）

### 6. database_connection.rs（预计 ~350 行）
**核心功能**: 数据库连接监控与启动

**需要提取的函数**（行号参考）:
- `check_database_connection` (6106) - 检查连接状态
- `get_startup_scripts` (6149) - 获取启动脚本
- `start_database_instance` (6199) - 启动数据库
- `check_surrealdb_connection` (6258) - SurrealDB 连接检查
- `start_surreal_with_script` (6294) - 脚本启动
- `create_default_startup_script` (6320) - 创建默认脚本
- `handle_database_connection_error` (6348) - 错误处理
- `run_database_diagnostics_api` (6437) - 运行诊断
- `database_connection_page` (6454) - 页面渲染

**数据结构**:
```rust
pub struct DatabaseConnectionStatus { ... }
pub struct DatabaseConnectionConfig { ... }
pub struct StartupScript { ... }
pub struct DbConnCheckQuery { ... }
pub struct StartDatabaseRequest { ... }
```

**辅助函数**:
- `get_db_config_from_options`
- `extract_port_from_filename`
- `is_addr_listening`（已在 port.rs）
- `test_tcp_connection`（已在 port.rs）

**注意事项**:
- 需要 `use crate::web_server::handlers::port::{is_addr_listening, test_tcp_connection};`
- 需要 `use crate::web_server::database_diagnostics::run_database_diagnostics;`

---

### 7. project/ 子目录（预计 ~800 行，需拆分为 4 个文件）

**建议结构**:
```
src/web_server/handlers/project/
├── mod.rs          # 模块导出
├── crud.rs         # CRUD 操作（~200行）
├── schema.rs       # 数据模式管理（~150行）
├── health.rs       # 健康检查（~150行）
└── demo.rs         # 演示数据生成（~200行）
```

**关键函数**（需要用 grep 定位具体行号）:
- 项目 CRUD: `create_project`, `get_project`, `update_project`, `delete_project`, `list_projects`
- 数据模式: `get_project_schema`, `update_project_schema`
- 健康检查: `check_project_health`
- 演示数据: `generate_demo_data`, `populate_demo_project`

**提取命令**:
```bash
grep -n "pub async fn.*project" src/web_server/handlers.rs | grep -v "deployment"
```

---

### 8. task/ 子目录（预计 ~1200 行，需拆分为 4 个文件）

**建议结构**:
```
src/web_server/handlers/task/
├── mod.rs              # 模块导出
├── crud.rs             # 任务CRUD（~300行）
├── execution.rs        # 任务执行逻辑（~400行）
├── batch.rs            # 批量处理（~300行）
└── templates.rs        # 任务模板（~200行）
```

**关键函数**:
- 任务管理: `create_task`, `get_task`, `update_task`, `cancel_task`, `list_tasks`
- 任务执行: `execute_real_task`, `run_task_background`
- 批量操作: `batch_create_tasks`, `batch_cancel_tasks`
- 模板管理: `get_task_templates`, `apply_template`

**提取命令**:
```bash
grep -n "pub async fn.*task" src/web_server/handlers.rs | head -50
```

---

### 9. deployment_site/ 子目录（预计 ~900 行）

**建议结构**:
```
src/web_server/handlers/deployment_site/
├── mod.rs          # 模块导出
├── crud.rs         # CRUD 操作（~250行）
├── import.rs       # 导入功能（~300行）
├── browse.rs       # 浏览接口（~200行）
└── health.rs       # 健康检查（~150行）
```

**关键函数**:
- CRUD: `create_deployment_site`, `get_deployment_site`, `list_deployment_sites`
- 导入: `import_deployment_site`, `validate_import_data`
- 浏览: `browse_sites`, `search_sites`

---

### 10. surreal_server/ 子目录（预计 ~600 行）

**建议结构**:
```
src/web_server/handlers/surreal_server/
├── mod.rs          # 模块导出
├── lifecycle.rs    # 生命周期管理（~250行）
├── status.rs       # 状态查询（~200行）
└── utils.rs        # 工具函数（~150行）
```

**关键函数**:
- 生命周期: `start_surreal_server`, `stop_surreal_server`, `restart_surreal_server`
- 状态查询: `get_server_status`, `check_server_health`

---

### 11. database_status/ 子目录（预计 ~500 行）

**建议结构**:
```
src/web_server/handlers/database_status/
├── mod.rs      # 模块导出
├── query.rs    # 状态查询（~200行）
├── update.rs   # 状态更新（~150行）
└── check.rs    # 健康检查（~150行）
```

---

### 12. spatial_query/ 子目录（预计 ~450 行）

**建议结构**:
```
src/web_server/handlers/spatial_query/
├── mod.rs          # 模块导出
├── api.rs          # API 接口（~250行）
└── detection.rs    # 检测算法（~200行）
```

**关键函数**（已找到部分）:
- `sqlite_spatial_page` (1694) - 空间查询页面
- `spatial_visualization_page` (6465) - 可视化页面
- `render_spatial_visualization_page` (6476) - 渲染函数

---

### 13. pages/ 子目录（预计 ~1400 行）

**建议结构**:
```
src/web_server/handlers/pages/
├── mod.rs          # 模块导出
├── core.rs         # 核心页面（~400行）
├── task.rs         # 任务相关页面（~300行）
├── database.rs     # 数据库页面（~350行）
├── spatial.rs      # 空间查询页面（~250行）
└── test.rs         # 测试页面（~100行）
```

**关键函数**:
- 已识别：`sctn_test_page`, `database_connection_page`, `spatial_visualization_page`
- 需要查找更多页面渲染函数

---

## 重构执行步骤

### 步骤 1: 确认目录结构
```bash
mkdir -p src/web_server/handlers/{project,task,deployment_site,surreal_server,database_status,spatial_query,pages}
```

### 步骤 2: 定位函数（使用 grep）
```bash
# 查找所有 pub async fn
grep -n "^pub async fn" src/web_server/handlers.rs > function_list.txt

# 按模块分类
grep "project" function_list.txt > project_functions.txt
grep "task" function_list.txt > task_functions.txt
grep "deployment" function_list.txt > deployment_functions.txt
# ... 以此类推
```

### 步骤 3: 提取函数（参考已完成的模块）

以 `database_connection.rs` 为例：

1. **读取原函数**（使用行号范围）:
   ```bash
   sed -n '6106,6146p' src/web_server/handlers.rs > temp_function.rs
   ```

2. **创建新文件头部**:
   ```rust
   // database_connection.rs

   use axum::{...};
   use serde::{Deserialize, Serialize};
   use crate::web_server::{AppState, models::*};
   use super::port::{is_addr_listening, test_tcp_connection};
   ```

3. **复制函数和数据结构**

4. **验证编译**:
   ```bash
   cargo check --all-features
   ```

### 步骤 4: 更新 mod.rs
在 `src/web_server/handlers/mod.rs` 中添加：
```rust
pub mod database_connection;
pub mod project;
pub mod task;
// ...

pub use database_connection::*;
pub use project::*;
pub use task::*;
// ...
```

---

## 自动化脚本

为了加速重构，我编写了以下 Python 脚本辅助提取（需手动审核输出）:

```python
#!/usr/bin/env python3
# extract_functions.py - 辅助提取函数的脚本

import re
import sys

def extract_function_range(file_path, start_line, end_line):
    """从文件中提取指定行范围"""
    with open(file_path, 'r') as f:
        lines = f.readlines()
    return ''.join(lines[start_line-1:end_line])

def find_function_end(file_path, start_line):
    """查找函数结束位置（简单版本，基于括号匹配）"""
    with open(file_path, 'r') as f:
        lines = f.readlines()

    brace_count = 0
    in_function = False

    for i, line in enumerate(lines[start_line-1:], start=start_line):
        if '{' in line:
            in_function = True
            brace_count += line.count('{')
        if '}' in line:
            brace_count -= line.count('}')

        if in_function and brace_count == 0:
            return i

    return len(lines)

# 使用示例:
# python3 extract_functions.py handlers.rs 6106 database_connection_functions.rs
```

---

## 验证清单

重构完成后，请逐项检查：

- [ ] 所有原函数都已迁移（无遗漏）
- [ ] 每个新文件 ≤250 行
- [ ] 所有 `pub` 函数已在 `mod.rs` 重新导出
- [ ] 数据结构已移至正确位置（`models.rs` 或模块内）
- [ ] 导入语句完整（use 语句）
- [ ] `cargo check` 通过
- [ ] `cargo test` 通过（如有测试）
- [ ] 原 `handlers.rs` 已备份（重命名为 `handlers.rs.backup`）

---

## 常见问题

### Q1: 如何处理共享的辅助函数？
**A**: 创建 `src/web_server/handlers/common.rs` 存放共享工具函数。

### Q2: 数据结构应该放在哪里？
**A**:
- 公共数据结构 → `src/web_server/models.rs`
- 模块专属数据结构 → 模块内部定义

### Q3: 如何处理条件编译特性（#[cfg(feature = "...")]）？
**A**: 保留在新文件中，确保在相同位置使用。示例：
```rust
#[cfg(feature = "sqlite-index")]
use crate::fast_model::spatial_index::SqliteSpatialIndex;
```

### Q4: 如何处理全局静态变量（static LAZY）？
**A**: 移动到使用它的模块中，如果跨模块使用则保留在 `mod.rs`。

---

## 下一步行动

1. **立即开始**: 使用本指南完成 `database_connection.rs`
2. **并行处理**: 可以同时处理多个简单模块
3. **复杂模块**: project/, task/ 等需要子目录的模块建议逐步拆分
4. **持续验证**: 每完成一个模块就运行 `cargo check`

---

## 联系与支持

如遇到问题，请参考已完成的模块（config.rs, export.rs, model_generation.rs, sctn_test.rs）作为参考模板。

---

**预计总工作量**: 8-12 小时（取决于对代码库的熟悉程度）

**优先级排序**:
1. 简单模块: database_connection.rs（先完成，建立信心）
2. 中等模块: spatial_query/, surreal_server/
3. 复杂模块: project/, task/, deployment_site/
4. 最后: pages/（包含大量 HTML）

**完成标准**: 原 handlers.rs 可以删除，所有功能通过新模块提供。
