<!-- 86c8b9cf-3365-4327-990d-726cfdc6724d 2d24a2e4-462a-493f-aa6c-6dc7138fc3de -->
# gen-model-fork 移除 AiosDBMgr 重构计划

## 一、依赖分析

### 1.1 直接使用 AiosDBMgr 的位置

**核心文件**：

- `src/lib.rs:21,289` - `AiosDBMgr::init_from_db_option()` 用于初始化
- `src/versioned_db/database.rs:9,91,346` - `create_info_database()` 函数参数和 `get_global_pool()`, `get_project_pool()`
- `src/team_data.rs:3,23` - `sync_team_data()` 函数参数，使用 `PdmsDataInterface` trait

**导入但可能未使用**：

- `src/versioned_db/pe.rs:6` - 导入 AiosDBMgr，需要检查是否实际使用

### 1.2 使用 MySQL 连接池的位置

**直接调用 AiosDBMgr 方法**：

- `src/versioned_db/database.rs:92,142` - `aios_mgr.get_global_pool()`, `aios_mgr.get_project_pool()`
- `src/team_data.rs:75` - `mgr.get_project_pool()`

**通过其他管理器调用**（需要检查）：

- `src/data_interface/db_model.rs:336,382,505,521` - `get_global_pool()`, `get_puhua_pool()`, `get_project_pool()`, `get_project_pool_by_refno()`
- `src/version_management/query_status.rs:14` - `aios_mgr.get_project_pool()`
- `src/rvm/elements.rs:34,60,256` - `get_project_pool_by_refno()`
- `src/pcf/pcf_api.rs:119,127,256,266` - `get_project_pool_by_refno()`
- `src/pcf/bran.rs:222` - `get_project_pool_by_refno()`
- `src/other_plat/plat_user.rs:22` - `get_global_pool()`
- `src/api/attr.rs:344` - `get_project_pool_by_refno()`
- `src/api/admin.rs:124` - `get_project_pool_by_refno()`

### 1.3 使用 PdmsDataInterface 的位置

**直接使用**：

- `src/team_data.rs:38,46` - `mgr.get_name()`, `mgr.get_attr()`

**通过 data_interface 模块**：

- `src/data_interface/interface.rs` - 定义了 `PdmsDataInterface` trait（需要移除）
- `src/fast_model/cata_model.rs:3` - 导入 `PdmsDataInterface`
- `src/fast_model/gen_model_old.rs:2` - 导入 `PdmsDataInterface`
- `src/fast_model/loop_model.rs:2` - 导入 `PdmsDataInterface`
- `src/fast_model/prim_model.rs:1` - 导入 `PdmsDataInterface`
- `src/data_interface/increment_manager.rs:31` - 导入 `PdmsDataInterface`

## 二、迁移策略

### 2.1 依赖 rs-core 的新模块

gen-model-fork 依赖 `aios_core`，需要确保使用 rs-core 中已创建的新模块：

1. **连接池管理** - 使用 `aios_core::db_pool` 模块

- `get_global_pool(db_option)` - 需要传入 `DbOption`
- `get_project_pool(db_option)` - 需要传入 `DbOption`
- `get_puhua_pool(db_option)` - 需要传入 `DbOption`

2. **QueryProvider** - 使用 `aios_core::query_provider` 模块

- `QueryRouter::surreal_only()` - 替代 `AiosDBMgr::init_from_db_option()`
- `QueryProvider` trait - 替代 `PdmsDataInterface`

### 2.2 迁移步骤

#### 步骤 1：替换初始化代码

- `src/lib.rs:289` - `AiosDBMgr::init_from_db_option()` → 使用 `QueryRouter::surreal_only()` 或直接使用 `SUL_DB`
- 移除 `AiosDBMgr` 的导入

#### 步骤 2：替换连接池获取

- `src/versioned_db/database.rs:91` - `create_info_database()` 函数签名改为接收 `DbOption` 而不是 `AiosDBMgr`
- 内部使用 `aios_core::db_pool::get_global_pool(db_option)` 和 `get_project_pool(db_option)`
- `src/team_data.rs:75` - 改为使用 `aios_core::db_pool::get_project_pool(db_option)`

#### 步骤 3：替换 PdmsDataInterface 使用

- `src/team_data.rs:23` - `sync_team_data()` 函数签名改为接收 `QueryProvider` 或直接使用 `aios_core` 函数
- `mgr.get_name()` → 使用 `aios_core::get_name()` 或 `QueryProvider` 方法
- `mgr.get_attr()` → 使用 `aios_core::get_named_attmap_with_uda()` 或 `QueryProvider` 方法

#### 步骤 4：检查并迁移其他模块

- `src/data_interface/db_model.rs` - 检查 `get_project_pool_by_refno()` 等方法，可能需要重构
- `src/version_management/query_status.rs` - 检查连接池使用方式
- `src/rvm/elements.rs`, `src/pcf/pcf_api.rs` 等 - 检查 `get_project_pool_by_refno()` 的使用

#### 步骤 5：移除 PdmsDataInterface trait

- `src/data_interface/interface.rs` - 移除 `PdmsDataInterface` trait 定义
- 所有导入 `PdmsDataInterface` 的文件改为使用 `QueryProvider` 或直接使用 `aios_core` 函数

## 三、详细迁移清单

### 3.1 文件级迁移任务

| 文件 | 当前使用 | 迁移目标 | 优先级 |
|------|---------|---------|--------|
| `src/lib.rs` | `AiosDBMgr::init_from_db_option()` | `QueryRouter::surreal_only()` 或移除 | 高 |
| `src/versioned_db/database.rs` | `AiosDBMgr` 参数和连接池方法 | `DbOption` 参数 + `db_pool` 模块 | 高 |
| `src/team_data.rs` | `AiosDBMgr` + `PdmsDataInterface` | `QueryProvider` 或 `aios_core` 函数 | 高 |
| `src/versioned_db/pe.rs` | 导入 AiosDBMgr | 检查并移除 | 中 |
| `src/data_interface/interface.rs` | 定义 `PdmsDataInterface` | 移除 trait 定义 | 高 |
| `src/data_interface/db_model.rs` | 连接池方法 | 使用 `db_pool` 模块 | 中 |
| `src/version_management/query_status.rs` | 连接池方法 | 使用 `db_pool` 模块 | 中 |
| `src/rvm/elements.rs` | `get_project_pool_by_refno()` | 重构为使用 `db_pool` | 中 |
| `src/pcf/pcf_api.rs` | `get_project_pool_by_refno()` | 重构为使用 `db_pool` | 中 |
| `src/pcf/bran.rs` | `get_project_pool_by_refno()` | 重构为使用 `db_pool` | 中 |
| `src/other_plat/plat_user.rs` | `get_global_pool()` | 使用 `db_pool` 模块 | 中 |
| `src/api/attr.rs` | `get_project_pool_by_refno()` | 重构为使用 `db_pool` | 中 |
| `src/api/admin.rs` | `get_project_pool_by_refno()` | 重构为使用 `db_pool` | 中 |
| `src/fast_model/*.rs` | 导入 `PdmsDataInterface` | 移除导入或改为 `QueryProvider` | 低 |

### 3.2 函数签名变更

**需要修改的函数签名**：

1. `create_info_database(aios_mgr: &AiosDBMgr)` 
→ `create_info_database(db_option: &DbOption)`

2. `sync_team_data(mgr: &AiosDBMgr)`
→ `sync_team_data(provider: Arc<dyn QueryProvider>, db_option: &DbOption)`
或改为直接使用 `aios_core` 函数

## 四、注意事项

### 4.1 依赖关系

- gen-model-fork 依赖 `aios_core`，需要确保 `aios_core` 中的新模块（`db_pool`, `query_provider`）已经可用
- 如果 `aios_core` 中某些函数内部仍使用 `AiosDBMgr`，需要先迁移 `aios_core`

### 4.2 测试要求

- 迁移后需要运行 gen-model-fork 的测试套件
- 特别关注数据库连接和查询功能的测试
- 验证 `sync_team_data` 和 `create_info_database` 功能正常

### 4.3 向后兼容

- 如果 gen-model-fork 被其他项目依赖，需要考虑 API 变更的影响
- 建议在迁移过程中保持功能等价性

## 五、迁移顺序

1. **第一阶段**：替换核心初始化代码（`src/lib.rs`）
2. **第二阶段**：迁移数据库相关函数（`src/versioned_db/database.rs`, `src/team_data.rs`）
3. **第三阶段**：迁移其他模块的连接池使用
4. **第四阶段**：移除 `PdmsDataInterface` trait 和相关导入
5. **第五阶段**：清理和测试

### To-dos

- [ ] 确认 aios_core 中的 db_pool 和 query_provider 模块已可用
- [ ] 迁移 src/lib.rs 中的 AiosDBMgr::init_from_db_option()
- [ ] 迁移 src/versioned_db/database.rs 中的 create_info_database 函数
- [ ] 迁移 src/team_data.rs 中的 sync_team_data 函数，移除 PdmsDataInterface 依赖
- [ ] 检查并清理 src/versioned_db/pe.rs 中的 AiosDBMgr 导入
- [ ] 迁移 src/data_interface/db_model.rs 中的连接池方法
- [ ] 迁移其他文件中的连接池调用（version_management, rvm, pcf, api 等）
- [ ] 移除 src/data_interface/interface.rs 中的 PdmsDataInterface trait 定义
- [ ] 清理所有文件中的 AiosDBMgr 和 PdmsDataInterface 导入
- [ ] 运行 gen-model-fork 的测试套件，确保迁移后功能正常