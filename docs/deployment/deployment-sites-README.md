# 部署站点功能说明

## 📋 功能概述

AIOS数据库管理平台的部署站点功能提供了一个完整的E3D项目管理和任务执行解决方案。用户可以通过Web界面创建、管理部署站点，并为每个站点执行各种数据处理任务。

## 🏗️ 核心功能

### 1. 部署站点管理
- ✅ 创建和配置部署站点
- ✅ 管理E3D项目集合
- ✅ 配置数据库连接参数
- ✅ 设置环境和负责人信息
- ✅ 站点状态监控

### 2. 任务管理
- ✅ 为站点创建各类处理任务
- ✅ 任务优先级设置
- ✅ 实时任务进度监控
- ✅ 任务日志查看
- ✅ 错误信息追踪

### 3. 数据处理
- ✅ PDMS数据解析
- ✅ 3D模型生成
- ✅ 空间树构建
- ✅ 网格数据生成
- ✅ 空间索引构建

## 🚀 快速开始

### 1. 启动服务
```bash
# 启动Web UI服务
cargo run --bin web_server

# 或启动完整服务
cargo run
```

### 2. 访问界面
打开浏览器访问：`http://localhost:8080`

### 3. 创建部署站点
1. 点击"创建站点"按钮
2. 填写站点基本信息
3. 输入E3D项目路径（每行一个）
4. 配置数据库参数
5. 提交创建

### 4. 执行任务
1. 在站点列表中选择目标站点
2. 点击"创建任务"
3. 选择任务类型和优先级
4. 提交任务并监控执行

## 📊 数据结构

### 主要模型

#### DeploymentSite (部署站点)
```rust
pub struct DeploymentSite {
    pub id: Option<String>,                    // 站点ID
    pub name: String,                          // 站点名称
    pub description: Option<String>,           // 描述
    pub e3d_projects: Vec<E3dProjectInfo>,     // E3D项目列表
    pub config: DatabaseConfig,                // 数据库配置
    pub status: DeploymentSiteStatus,          // 站点状态
    pub env: Option<String>,                   // 环境
    pub owner: Option<String>,                 // 负责人
    // ... 其他字段
}
```

#### E3dProjectInfo (E3D项目)
```rust
pub struct E3dProjectInfo {
    pub name: String,                          // 项目名称
    pub path: String,                          // 项目路径
    pub project_code: Option<u32>,             // 项目代码
    pub db_file_count: u32,                    // DB文件数量
    pub size_bytes: u64,                       // 项目大小
    pub selected: bool,                        // 是否选中
    // ... 其他字段
}
```

#### DatabaseConfig (数据库配置)
```rust
pub struct DatabaseConfig {
    pub project_name: String,                  // 项目名称
    pub project_code: u32,                     // 项目代码
    pub db_type: String,                       // 数据库类型
    pub db_ip: String,                         // 数据库IP
    pub db_port: String,                       // 数据库端口
    pub gen_model: bool,                       // 生成模型
    pub gen_spatial_tree: bool,                // 生成空间树
    // ... 其他配置
}
```

## 🌐 API接口

### 站点管理
```http
GET    /api/deployment-sites              # 获取站点列表
POST   /api/deployment-sites              # 创建站点
GET    /api/deployment-sites/{id}         # 获取站点详情
PUT    /api/deployment-sites/{id}         # 更新站点
DELETE /api/deployment-sites/{id}         # 删除站点
```

### 任务管理
```http
POST   /api/deployment-sites/{id}/tasks   # 为站点创建任务
GET    /api/tasks                         # 获取任务列表
GET    /api/tasks/{id}                    # 获取任务详情
POST   /api/tasks/{id}/start              # 启动任务
POST   /api/tasks/{id}/stop               # 停止任务
```

## 🔧 配置说明

### 默认数据库配置
```javascript
config: {
    name: '默认配置',
    project_name: 'AvevaMarineSample',
    project_code: 1516,
    mdb_name: 'ALL',
    module: 'DESI',
    db_type: 'surrealdb',
    surreal_ns: 1516,
    db_ip: 'localhost',
    db_port: '8009',
    db_user: 'root',
    db_password: 'root',
    gen_model: true,
    gen_mesh: false,
    gen_spatial_tree: true,
    apply_boolean_operation: true,
    mesh_tol_ratio: 3.0,
    room_keyword: '-RM'
}
```

### 环境配置
- `dev` - 开发环境
- `staging` - 测试环境  
- `prod` - 生产环境

## 📝 使用场景

### 1. 开发环境
- 快速原型验证
- 功能测试
- 调试和开发

### 2. 测试环境
- 集成测试
- 性能测试
- 用户验收测试

### 3. 生产环境
- 正式数据处理
- 批量任务执行
- 生产监控

## 🔍 任务类型

### 数据处理任务
- `DataGeneration` - 数据生成
- `ParsePdmsData` - PDMS数据解析
- `GenerateGeometry` - 几何数据生成

### 模型处理任务
- `MeshGeneration` - 网格生成
- `SpatialTreeGeneration` - 空间树生成
- `FullGeneration` - 完整生成

### 索引构建任务
- `BuildSpatialIndex` - 空间索引构建
- `BatchDatabaseProcess` - 批量数据库处理

## 📈 监控和日志

### 任务状态
- `Pending` - 等待执行
- `Running` - 正在执行
- `Completed` - 执行完成
- `Failed` - 执行失败
- `Cancelled` - 已取消

### 日志级别
- `Info` - 信息日志
- `Warn` - 警告日志
- `Error` - 错误日志
- `Debug` - 调试日志

## 🚨 故障排除

### 常见问题

#### 1. 站点创建失败
**问题**: "站点名称已存在"
**解决**: 使用唯一的站点名称

#### 2. 项目扫描失败
**问题**: "无法扫描项目目录"
**解决**: 
- 检查路径是否存在
- 确认路径访问权限
- 验证路径格式正确

#### 3. 数据库连接失败
**问题**: "数据库连接失败"
**解决**:
- 检查SurrealDB服务状态
- 验证连接参数
- 确认网络连通性

#### 4. 任务执行失败
**问题**: 任务状态显示"Failed"
**解决**:
- 查看任务详细日志
- 检查配置参数
- 验证数据文件完整性

### 日志查看
```bash
# 查看Web UI日志
tail -f web_server.log

# 查看任务执行日志
tail -f model_gen_output.log
```

## 🔒 安全注意事项

1. **数据库密码**: 避免在配置中使用明文密码
2. **文件权限**: 确保项目目录有适当的访问权限
3. **网络安全**: 在生产环境中使用HTTPS
4. **输入验证**: 所有用户输入都经过严格验证
5. **访问控制**: 实施基于角色的访问控制

## 📚 相关文档

- [详细工作流程文档](./deployment-sites-workflow.md)
- [流程图集合](./deployment-sites-flowcharts.md)
- [API接口文档](../README_web_server.md)
- [数据库设计文档](./database_abstraction.md)

## 🤝 贡献指南

1. Fork项目仓库
2. 创建功能分支
3. 提交代码变更
4. 创建Pull Request
5. 等待代码审查

## 📞 技术支持

如有问题或建议，请通过以下方式联系：

- 创建GitHub Issue
- 发送邮件至开发团队
- 查看项目Wiki文档

---

*版本: v1.0*  
*最后更新: 2025-01-11*  
*维护者: AIOS开发团队*
