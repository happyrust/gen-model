# 部署站点流程图集合

本文档包含AIOS数据库管理平台部署站点功能的各种流程图，使用Mermaid语法绘制。

## 1. 数据结构关系图

```mermaid
graph TB
    %% 前端数据结构
    subgraph "前端 JavaScript"
        JS_NewSite[newSite 表单数据]
        JS_Config[config 配置对象]
        JS_TaskReq[taskRequest 任务请求]
    end

    %% 请求数据结构
    subgraph "API 请求结构"
        CreateReq[DeploymentSiteCreateRequest]
        TaskReq[DeploymentSiteTaskRequest]
        UpdateReq[DeploymentSiteUpdateRequest]
    end

    %% 核心数据模型
    subgraph "核心数据模型"
        DeploymentSite[DeploymentSite 部署站点]
        E3dProject[E3dProjectInfo E3D项目信息]
        DatabaseConfig[DatabaseConfig 数据库配置]
        TaskInfo[TaskInfo 任务信息]
    end

    %% 枚举类型
    subgraph "状态枚举"
        SiteStatus[DeploymentSiteStatus<br/>- Configuring<br/>- Deploying<br/>- Running<br/>- Failed<br/>- Stopped]
        TaskStatus[TaskStatus<br/>- Pending<br/>- Running<br/>- Completed<br/>- Failed<br/>- Cancelled]
        TaskType[TaskType<br/>- DataGeneration<br/>- SpatialTreeGeneration<br/>- FullGeneration<br/>- MeshGeneration]
        TaskPriority[TaskPriority<br/>- Low<br/>- Normal<br/>- High<br/>- Critical]
    end

    %% 数据库存储
    subgraph "SurrealDB 存储"
        DB_Sites[(deployment_sites 表)]
        DB_Tasks[(tasks 表)]
        DB_Projects[(projects 表)]
    end

    %% 管理器
    subgraph "应用状态管理"
        AppState[AppState]
        TaskManager[TaskManager]
        ConfigManager[ConfigManager]
    end

    %% 关系连接
    JS_NewSite --> CreateReq
    JS_Config --> DatabaseConfig
    JS_TaskReq --> TaskReq

    CreateReq --> DeploymentSite
    TaskReq --> TaskInfo

    DeploymentSite --> E3dProject
    DeploymentSite --> DatabaseConfig
    DeploymentSite --> SiteStatus

    TaskInfo --> TaskStatus
    TaskInfo --> TaskType
    TaskInfo --> TaskPriority
    TaskInfo --> DatabaseConfig

    DeploymentSite --> DB_Sites
    TaskInfo --> DB_Tasks

    AppState --> TaskManager
    AppState --> ConfigManager
    TaskManager --> TaskInfo

    %% 样式
    classDef frontend fill:#e1f5fe
    classDef request fill:#f3e5f5
    classDef model fill:#e8f5e8
    classDef enum fill:#fff3e0
    classDef storage fill:#fce4ec
    classDef manager fill:#f1f8e9

    class JS_NewSite,JS_Config,JS_TaskReq frontend
    class CreateReq,TaskReq,UpdateReq request
    class DeploymentSite,E3dProject,DatabaseConfig,TaskInfo model
    class SiteStatus,TaskStatus,TaskType,TaskPriority enum
    class DB_Sites,DB_Tasks,DB_Projects storage
    class AppState,TaskManager,ConfigManager manager
```

## 2. 部署站点创建流程

```mermaid
graph TD
    A[用户访问Web界面] --> B[点击创建站点]
    B --> C[填写站点基本信息]
    C --> D{站点名称是否唯一?}
    D -->|否| E[显示错误提示]
    E --> C
    D -->|是| F[输入E3D项目路径]
    F --> G[系统扫描项目目录]
    G --> H{项目路径有效?}
    H -->|否| I[显示路径错误]
    I --> F
    H -->|是| J[解析项目信息]
    J --> K[配置数据库参数]
    K --> L[预览站点配置]
    L --> M{用户确认创建?}
    M -->|否| N[返回修改]
    N --> C
    M -->|是| O[提交创建请求]
    O --> P[API验证数据]
    P --> Q{验证通过?}
    Q -->|否| R[返回验证错误]
    R --> C
    Q -->|是| S[创建DeploymentSite对象]
    S --> T[存储到SurrealDB]
    T --> U{存储成功?}
    U -->|否| V[显示数据库错误]
    U -->|是| W[返回成功响应]
    W --> X[更新前端站点列表]
    X --> Y[显示创建成功消息]

    %% 样式
    classDef userAction fill:#e3f2fd
    classDef validation fill:#fff3e0
    classDef process fill:#e8f5e8
    classDef error fill:#ffebee
    classDef success fill:#e8f5e8

    class A,B,C,F,K,L,M,N userAction
    class D,H,P,Q,U validation
    class G,J,O,S,T,X process
    class E,I,R,V error
    class W,Y success
```

## 3. 任务创建和执行流程

```mermaid
graph TD
    A[选择部署站点] --> B[点击创建任务]
    B --> C[选择任务类型]
    C --> D[设置任务优先级]
    D --> E[可选配置覆盖]
    E --> F[提交任务请求]
    F --> G[API接收请求]
    G --> H{站点是否存在?}
    H -->|否| I[返回站点不存在错误]
    H -->|是| J[获取站点配置]
    J --> K[创建TaskInfo实例]
    K --> L[生成任务ID]
    L --> M[设置任务状态为Pending]
    M --> N[添加到TaskManager]
    N --> O[任务进入队列]
    O --> P[等待执行]
    P --> Q[开始执行任务]
    Q --> R[更新状态为Running]
    R --> S[执行具体业务逻辑]
    S --> T{执行成功?}
    T -->|否| U[记录错误信息]
    U --> V[更新状态为Failed]
    T -->|是| W[更新状态为Completed]
    V --> X[通知前端更新]
    W --> X
    X --> Y[用户查看任务结果]

    %% 子流程：具体业务逻辑
    S --> S1[数据生成]
    S --> S2[空间树生成]
    S --> S3[网格生成]
    S --> S4[空间索引构建]

    %% 样式
    classDef userAction fill:#e3f2fd
    classDef apiProcess fill:#f3e5f5
    classDef taskProcess fill:#e8f5e8
    classDef decision fill:#fff3e0
    classDef error fill:#ffebee
    classDef success fill:#e8f5e8

    class A,B,C,D,E,Y userAction
    class F,G,J,X apiProcess
    class K,L,M,N,O,P,Q,R,S,S1,S2,S3,S4 taskProcess
    class H,T decision
    class I,U,V error
    class W success
```

## 4. API请求处理流程

```mermaid
sequenceDiagram
    participant Frontend as 前端界面
    participant API as API服务
    participant Validator as 数据验证器
    participant DB as SurrealDB
    participant TaskMgr as 任务管理器

    %% 创建站点流程
    Frontend->>API: POST /api/deployment-sites
    API->>Validator: 验证请求数据
    Validator-->>API: 验证结果
    alt 验证失败
        API-->>Frontend: 400 Bad Request
    else 验证成功
        API->>DB: 检查站点名称唯一性
        DB-->>API: 查询结果
        alt 名称已存在
            API-->>Frontend: 409 Conflict
        else 名称可用
            API->>API: 扫描E3D项目
            API->>API: 创建DeploymentSite对象
            API->>DB: 存储站点记录
            DB-->>API: 存储结果
            API-->>Frontend: 201 Created
        end
    end

    %% 创建任务流程
    Frontend->>API: POST /api/deployment-sites/{id}/tasks
    API->>DB: 获取站点信息
    DB-->>API: 站点数据
    API->>API: 创建TaskInfo对象
    API->>TaskMgr: 添加任务到队列
    TaskMgr-->>API: 任务ID
    API-->>Frontend: 200 OK

    %% 任务执行流程
    TaskMgr->>TaskMgr: 开始执行任务
    TaskMgr->>TaskMgr: 更新任务状态
    TaskMgr->>API: 通知状态变更
    API->>Frontend: WebSocket推送更新
```

## 5. 前端状态管理流程

```mermaid
stateDiagram-v2
    [*] --> Loading: 页面初始化
    Loading --> SitesList: 加载站点列表成功
    Loading --> Error: 加载失败
    
    SitesList --> Creating: 点击创建站点
    SitesList --> Viewing: 点击查看详情
    SitesList --> Editing: 点击编辑站点
    SitesList --> Deleting: 点击删除站点
    SitesList --> TaskCreating: 点击创建任务
    
    Creating --> SitesList: 创建成功
    Creating --> Creating: 创建失败，重试
    Creating --> SitesList: 取消创建
    
    Viewing --> SitesList: 关闭详情
    Viewing --> Editing: 点击编辑
    Viewing --> TaskCreating: 创建任务
    
    Editing --> SitesList: 编辑成功
    Editing --> Editing: 编辑失败，重试
    Editing --> SitesList: 取消编辑
    
    Deleting --> SitesList: 删除成功
    Deleting --> SitesList: 删除失败
    
    TaskCreating --> SitesList: 任务创建成功
    TaskCreating --> TaskCreating: 创建失败，重试
    TaskCreating --> SitesList: 取消创建
    
    Error --> Loading: 重试加载
    Error --> [*]: 退出页面
```

## 6. 数据库操作流程

```mermaid
graph LR
    subgraph "SurrealDB操作"
        A[连接数据库] --> B[选择命名空间]
        B --> C[选择数据库]
        C --> D[执行SQL查询]
        D --> E{操作成功?}
        E -->|是| F[返回结果]
        E -->|否| G[返回错误]
    end

    subgraph "站点CRUD操作"
        H[CREATE deployment_sites] --> I[INSERT站点记录]
        J[SELECT deployment_sites] --> K[查询站点列表]
        L[UPDATE deployment_sites] --> M[更新站点信息]
        N[DELETE deployment_sites] --> O[删除站点记录]
    end

    subgraph "索引管理"
        P[创建唯一索引] --> Q[idx_deployment_sites_name]
        Q --> R[确保名称唯一性]
    end

    A --> H
    A --> J
    A --> L
    A --> N
    
    F --> P
```

## 7. 错误处理流程

```mermaid
graph TD
    A[用户操作] --> B[前端验证]
    B --> C{前端验证通过?}
    C -->|否| D[显示前端错误]
    C -->|是| E[发送API请求]
    E --> F[后端验证]
    F --> G{后端验证通过?}
    G -->|否| H[返回400错误]
    G -->|是| I[执行业务逻辑]
    I --> J{业务逻辑成功?}
    J -->|否| K[返回500错误]
    J -->|是| L[返回成功结果]
    
    D --> M[用户修正输入]
    H --> N[前端显示错误消息]
    K --> N
    N --> O[用户重试或取消]
    L --> P[前端更新界面]
    
    M --> A
    O --> A

    %% 错误类型分类
    subgraph "错误类型"
        E1[400 - 请求参数错误]
        E2[401 - 未授权]
        E3[403 - 权限不足]
        E4[404 - 资源不存在]
        E5[409 - 资源冲突]
        E6[500 - 服务器内部错误]
    end

    H --> E1
    H --> E4
    H --> E5
    K --> E6
```

## 8. 任务状态转换图

```mermaid
stateDiagram-v2
    [*] --> Pending: 任务创建
    Pending --> Running: 开始执行
    Pending --> Cancelled: 用户取消
    
    Running --> Completed: 执行成功
    Running --> Failed: 执行失败
    Running --> Cancelled: 用户中断
    
    Completed --> [*]: 任务结束
    Failed --> [*]: 任务结束
    Cancelled --> [*]: 任务结束
    
    Failed --> Pending: 重新执行
    
    note right of Running
        任务执行中可以：
        - 查看进度
        - 查看日志
        - 中断执行
    end note
    
    note right of Completed
        任务完成后可以：
        - 查看结果
        - 下载报告
        - 删除任务
    end note
```

## 使用说明

这些流程图可以通过以下方式使用：

1. **在Markdown文档中直接渲染**（支持Mermaid的编辑器）
2. **在线Mermaid编辑器**：https://mermaid.live/
3. **VS Code插件**：Mermaid Preview
4. **导出为图片**：使用mermaid-cli工具

### 导出命令示例
```bash
# 安装mermaid-cli
npm install -g @mermaid-js/mermaid-cli

# 导出为PNG
mmdc -i deployment-sites-flowcharts.md -o flowcharts.png

# 导出为SVG
mmdc -i deployment-sites-flowcharts.md -o flowcharts.svg
```

---

*流程图版本: v1.0*  
*最后更新: 2025-01-11*  
*维护者: AIOS开发团队*
