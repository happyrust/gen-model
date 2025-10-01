# 异地协同配置功能设计文档

## 1. 数据模型设计

### 1.1 协同组 (CollaborationGroup)

```rust
/// 协同组
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollaborationGroup {
    /// 协同组 ID
    pub id: Option<String>,

    /// 协同组名称
    pub name: String,

    /// 协同组描述
    pub description: Option<String>,

    /// 协同组类型
    pub group_type: CollaborationGroupType,

    /// 组内站点 ID 列表
    pub site_ids: Vec<String>,

    /// 主站点 ID（配置源）
    pub primary_site_id: Option<String>,

    /// 共享配置
    pub shared_config: Option<DatabaseConfig>,

    /// 同步策略
    pub sync_strategy: SyncStrategy,

    /// 协同组状态
    pub status: CollaborationGroupStatus,

    /// 创建者
    pub creator: String,

    /// 创建时间
    pub created_at: Option<SystemTime>,

    /// 更新时间
    pub updated_at: Option<SystemTime>,

    /// 标签
    pub tags: Option<serde_json::Value>,
}

/// 协同组类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CollaborationGroupType {
    /// 配置共享组
    ConfigSharing,

    /// 数据同步组
    DataSync,

    /// 任务协调组
    TaskCoordination,

    /// 混合模式
    Hybrid,
}

/// 同步策略
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncStrategy {
    /// 同步模式
    pub mode: SyncMode,

    /// 同步频率（秒）
    pub interval_seconds: u32,

    /// 是否自动同步
    pub auto_sync: bool,

    /// 冲突解决策略
    pub conflict_resolution: ConflictResolution,
}

/// 同步模式
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncMode {
    /// 单向同步（主站点 -> 从站点）
    OneWay,

    /// 双向同步
    TwoWay,

    /// 手动同步
    Manual,
}

/// 冲突解决策略
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConflictResolution {
    /// 主站点优先
    PrimaryWins,

    /// 最新更新优先
    LatestWins,

    /// 手动解决
    Manual,
}

/// 协同组状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CollaborationGroupStatus {
    /// 活跃
    Active,

    /// 同步中
    Syncing,

    /// 已暂停
    Paused,

    /// 错误
    Error,
}
```

### 1.2 远程站点 (RemoteSite)

```rust
/// 远程站点信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSite {
    /// 远程站点 ID
    pub id: String,

    /// 站点名称
    pub name: String,

    /// API 地址
    pub api_url: String,

    /// 认证令牌
    pub auth_token: Option<String>,

    /// 最近连接时间
    pub last_connected: Option<SystemTime>,

    /// 连接状态
    pub connection_status: ConnectionStatus,

    /// 延迟（毫秒）
    pub latency_ms: Option<u32>,
}

/// 连接状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConnectionStatus {
    /// 已连接
    Connected,

    /// 断开连接
    Disconnected,

    /// 连接中
    Connecting,

    /// 连接失败
    Failed,
}
```

### 1.3 同步记录 (SyncRecord)

```rust
/// 同步记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRecord {
    /// 记录 ID
    pub id: Option<String>,

    /// 协同组 ID
    pub group_id: String,

    /// 源站点 ID
    pub source_site_id: String,

    /// 目标站点 ID
    pub target_site_id: String,

    /// 同步类型
    pub sync_type: SyncType,

    /// 同步状态
    pub status: SyncStatus,

    /// 同步开始时间
    pub started_at: SystemTime,

    /// 同步结束时间
    pub completed_at: Option<SystemTime>,

    /// 错误信息
    pub error_message: Option<String>,

    /// 同步的数据量
    pub data_size: Option<u64>,
}

/// 同步类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncType {
    /// 配置同步
    Config,

    /// 全量数据同步
    FullData,

    /// 增量数据同步
    IncrementalData,
}

/// 同步状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncStatus {
    /// 进行中
    InProgress,

    /// 成功
    Success,

    /// 失败
    Failed,

    /// 部分成功
    PartialSuccess,
}
```

## 2. API 接口设计

### 2.1 协同组管理

```
POST   /api/collaboration-groups                    # 创建协同组
GET    /api/collaboration-groups                    # 获取协同组列表
GET    /api/collaboration-groups/:id                # 获取协同组详情
PUT    /api/collaboration-groups/:id                # 更新协同组
DELETE /api/collaboration-groups/:id                # 删除协同组
```

### 2.2 站点管理

```
POST   /api/collaboration-groups/:id/sites          # 添加站点到协同组
DELETE /api/collaboration-groups/:id/sites/:site_id # 从协同组移除站点
GET    /api/collaboration-groups/:id/sites          # 获取组内站点列表
```

### 2.3 远程站点

```
POST   /api/remote-sites                            # 注册远程站点
GET    /api/remote-sites                            # 获取远程站点列表
PUT    /api/remote-sites/:id                        # 更新远程站点
DELETE /api/remote-sites/:id                        # 删除远程站点
POST   /api/remote-sites/:id/test                   # 测试远程站点连接
```

### 2.4 同步操作

```
POST   /api/collaboration-groups/:id/sync           # 触发同步
GET    /api/collaboration-groups/:id/sync-records   # 获取同步记录
GET    /api/collaboration-groups/:id/sync-status    # 获取同步状态
```

### 2.5 配置管理

```
POST   /api/collaboration-groups/:id/push-config    # 推送配置到组内站点
GET    /api/collaboration-groups/:id/config-diff    # 比较配置差异
POST   /api/collaboration-groups/:id/resolve-conflict # 解决配置冲突
```

## 3. 前端 UI 设计

### 3.1 页面结构

```
/app/collaboration/
  ├── page.tsx                          # 协同组列表页
  ├── [id]/
  │   ├── page.tsx                      # 协同组详情页
  │   └── sync/page.tsx                 # 同步管理页
  └── create/page.tsx                   # 创建协同组页
```

### 3.2 核心组件

#### 3.2.1 协同组列表 (GroupList)
- 展示所有协同组
- 卡片视图/列表视图切换
- 过滤和搜索
- 状态指示器

#### 3.2.2 创建协同组对话框 (CreateGroupDialog)
- 步骤1：基本信息（名称、描述、类型）
- 步骤2：选择站点（从现有站点选择或添加远程站点）
- 步骤3：配置同步策略
- 步骤4：预览和确认

#### 3.2.3 站点选择器 (SiteSelector)
- 本地站点列表
- 远程站点列表
- 站点健康状态显示
- 支持多选

#### 3.2.4 同步配置面板 (SyncConfigPanel)
- 同步模式选择
- 频率设置
- 冲突解决策略
- 高级选项

#### 3.2.5 协同组详情 (GroupDetail)
- 组信息展示
- 站点拓扑图
- 实时同步状态
- 操作按钮（同步、编辑、删除）

#### 3.2.6 同步状态监控 (SyncMonitor)
- 实时同步进度
- 同步历史记录
- 错误日志
- 性能指标

#### 3.2.7 远程站点管理 (RemoteSiteManager)
- 添加远程站点
- 测试连接
- 编辑认证信息
- 查看连接状态

### 3.3 UI 交互流程

#### 创建协同组流程
```
1. 点击"创建协同组"按钮
2. 填写基本信息
   - 名称
   - 描述
   - 类型选择
3. 选择站点
   - 从本地站点列表选择
   - 或添加远程站点
   - 设置主站点
4. 配置同步策略
   - 选择同步模式
   - 设置同步频率
   - 选择冲突解决方案
5. 预览配置
6. 确认创建
```

#### 触发同步流程
```
1. 进入协同组详情页
2. 点击"立即同步"按钮
3. 显示同步确认对话框
   - 同步范围
   - 预计时间
   - 影响范围
4. 确认后开始同步
5. 实时显示同步进度
6. 完成后显示结果摘要
```

## 4. 技术实现要点

### 4.1 后端实现

#### 4.1.1 SQLite 表结构
```sql
-- 协同组表
CREATE TABLE collaboration_groups (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT,
    group_type TEXT NOT NULL,
    site_ids TEXT NOT NULL,  -- JSON 数组
    primary_site_id TEXT,
    shared_config TEXT,      -- JSON
    sync_strategy TEXT,      -- JSON
    status TEXT NOT NULL,
    creator TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    tags TEXT
);

-- 远程站点表
CREATE TABLE remote_sites (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    api_url TEXT NOT NULL,
    auth_token TEXT,
    last_connected TEXT,
    connection_status TEXT NOT NULL,
    latency_ms INTEGER,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- 同步记录表
CREATE TABLE sync_records (
    id TEXT PRIMARY KEY,
    group_id TEXT NOT NULL,
    source_site_id TEXT NOT NULL,
    target_site_id TEXT NOT NULL,
    sync_type TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at TEXT NOT NULL,
    completed_at TEXT,
    error_message TEXT,
    data_size INTEGER,
    FOREIGN KEY (group_id) REFERENCES collaboration_groups(id)
);
```

#### 4.1.2 核心功能模块
- `collaboration_handlers.rs` - 协同组管理的 HTTP 处理器
- `remote_site_handlers.rs` - 远程站点管理的处理器
- `sync_service.rs` - 同步服务核心逻辑
- `conflict_resolver.rs` - 配置冲突解决器

### 4.2 前端实现

#### 4.2.1 状态管理
```typescript
// hooks/use-collaboration-groups.ts
export function useCollaborationGroups() {
  const [groups, setGroups] = useState<CollaborationGroup[]>([])
  const [loading, setLoading] = useState(false)
  // ... 实现
}

// hooks/use-sync-monitor.ts
export function useSyncMonitor(groupId: string) {
  const [syncStatus, setSyncStatus] = useState<SyncStatus | null>(null)
  // WebSocket 连接实时更新
  // ... 实现
}
```

#### 4.2.2 API 客户端
```typescript
// lib/api/collaboration.ts
export async function createCollaborationGroup(
  payload: CreateCollaborationGroupPayload
): Promise<CollaborationGroup>

export async function syncGroup(
  groupId: string,
  options?: SyncOptions
): Promise<SyncResult>
```

### 4.3 安全考虑

1. **认证**：远程站点间使用 JWT 令牌认证
2. **加密**：敏感数据传输使用 HTTPS
3. **权限**：基于角色的访问控制（RBAC）
4. **审计**：记录所有同步操作日志

### 4.4 性能优化

1. **增量同步**：只同步变更的部分
2. **并行处理**：多站点同步可并行执行
3. **缓存**：缓存远程站点信息
4. **连接池**：复用 HTTP 连接

## 5. 实施步骤

### 阶段 1：基础架构（1-2天）
- [ ] 添加数据模型到 models.rs
- [ ] 创建 SQLite 表结构
- [ ] 实现基础 CRUD API

### 阶段 2：核心功能（2-3天）
- [ ] 实现协同组管理
- [ ] 实现远程站点注册
- [ ] 实现配置同步逻辑

### 阶段 3：UI 开发（2-3天）
- [ ] 创建协同组列表页
- [ ] 创建协同组详情页
- [ ] 实现创建/编辑对话框

### 阶段 4：同步功能（2-3天）
- [ ] 实现同步服务
- [ ] 添加冲突解决器
- [ ] 实现实时监控

### 阶段 5：测试和优化（1-2天）
- [ ] 单元测试
- [ ] 集成测试
- [ ] 性能优化

## 6. 示例场景

### 场景 1：配置共享
```
需求：3个站点使用相同的数据库配置
解决方案：
1. 创建"配置共享组"
2. 设置站点A为主站点
3. 添加站点B、C到组
4. 启用单向同步
5. 主站点配置变更时自动推送到其他站点
```

### 场景 2：跨地域协作
```
需求：北京和上海的团队协同工作
解决方案：
1. 创建"跨区域协作组"
2. 注册远程站点（上海）
3. 配置双向同步
4. 设置冲突解决策略为"最新更新优先"
5. 定时同步（每小时）
```

### 场景 3：灾备同步
```
需求：主站点数据自动备份到灾备站点
解决方案：
1. 创建"灾备同步组"
2. 设置主站点和备用站点
3. 配置单向全量同步
4. 高频同步（每10分钟）
5. 监控同步健康状态
```