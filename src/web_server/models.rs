use aios_core::options::DbOption;
use serde::{Deserialize, Serialize, Serializer};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// 任务ID计数器
static TASK_ID_COUNTER: AtomicU32 = AtomicU32::new(0);

/// 自定义SystemTime序列化函数，转换为毫秒时间戳
fn serialize_system_time<S>(time: &SystemTime, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let millis = duration.as_millis() as u64;
    serializer.serialize_u64(millis)
}

/// 自定义Option<SystemTime>序列化函数
fn serialize_optional_system_time<S>(
    time: &Option<SystemTime>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match time {
        Some(t) => serialize_system_time(t, serializer),
        None => serializer.serialize_none(),
    }
}

/// 任务信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    /// 任务ID
    pub id: String,
    /// 任务名称
    pub name: String,
    /// 任务类型
    pub task_type: TaskType,
    /// 任务状态
    pub status: TaskStatus,
    /// 配置信息
    pub config: DatabaseConfig,
    /// 创建时间
    #[serde(serialize_with = "serialize_system_time")]
    pub created_at: SystemTime,
    /// 开始时间
    #[serde(serialize_with = "serialize_optional_system_time")]
    pub started_at: Option<SystemTime>,
    /// 完成时间
    #[serde(serialize_with = "serialize_optional_system_time")]
    pub completed_at: Option<SystemTime>,
    /// 进度信息
    pub progress: TaskProgress,
    /// 错误信息
    pub error: Option<String>,
    /// 详细错误信息
    pub error_details: Option<ErrorDetails>,
    /// 日志信息
    pub logs: Vec<LogEntry>,
    /// 任务优先级
    pub priority: TaskPriority,
    /// 依赖的任务ID列表
    pub dependencies: Vec<String>,
    /// 预估执行时间（秒）
    pub estimated_duration: Option<u32>,
    /// 实际执行时间（毫秒）
    pub actual_duration: Option<u64>,
}

/// 任务类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskType {
    /// 数据生成
    DataGeneration,
    /// 房间计算
    SpatialTreeGeneration,
    /// 完整生成（数据+房间计算）
    FullGeneration,
    /// 网格生成
    MeshGeneration,
    /// 解析PDMS数据
    ParsePdmsData,
    /// 生成几何数据
    GenerateGeometry,
    /// 构建空间索引
    BuildSpatialIndex,
    /// 批量数据库处理
    BatchDatabaseProcess,
    /// 批量几何生成
    BatchGeometryGeneration,
    /// 数据导出
    DataExport,
    /// 数据导入
    DataImport,
    /// 数据解析向导
    DataParsingWizard,
    /// 基于 Refno 的模型生成
    RefnoModelGeneration,
    /// 自定义任务
    Custom(String),
}

/// 任务状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskStatus {
    /// 等待中
    Pending,
    /// 运行中
    Running,
    /// 已完成
    Completed,
    /// 失败
    Failed,
    /// 已取消
    Cancelled,
}

/// 任务进度
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskProgress {
    /// 当前步骤
    pub current_step: String,
    /// 总步骤数
    pub total_steps: u32,
    /// 当前步骤编号
    pub current_step_number: u32,
    /// 百分比进度 (0-100)
    pub percentage: f32,
    /// 处理的项目数
    pub processed_items: u64,
    /// 总项目数
    pub total_items: u64,
    /// 估计剩余时间（秒）
    pub estimated_remaining_seconds: Option<u64>,
}

/// 日志条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// 时间戳
    #[serde(serialize_with = "serialize_system_time")]
    pub timestamp: SystemTime,
    /// 日志级别
    pub level: LogLevel,
    /// 消息内容
    pub message: String,
    /// 相关的错误代码（可选）
    pub error_code: Option<String>,
    /// 堆栈跟踪（可选）
    pub stack_trace: Option<String>,
}

/// 错误详情
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorDetails {
    /// 错误类型
    pub error_type: String,
    /// 错误代码
    pub error_code: Option<String>,
    /// 发生错误的步骤
    pub failed_step: String,
    /// 详细错误消息
    pub detailed_message: String,
    /// 堆栈跟踪
    pub stack_trace: Option<String>,
    /// 可能的解决方案
    pub suggested_solutions: Vec<String>,
    /// 相关的配置信息
    pub related_config: Option<serde_json::Value>,
}

/// 日志级别
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
    Critical,
}

/// 数据库配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    /// 配置名称
    pub name: String,
    /// 手动指定的数据库编号
    pub manual_db_nums: Vec<u32>,
    /// 手动指定的 Refno 列表 (字符串格式，如 "123" 或 "1/456")
    #[serde(default)]
    pub manual_refnos: Vec<String>,
    /// 项目名称
    pub project_name: String,
    /// 项目路径
    pub project_path: String,
    /// 项目代码
    pub project_code: u32,
    /// MDB名称
    pub mdb_name: String,
    /// 模块类型
    pub module: String,
    /// 数据库类型
    pub db_type: String,
    /// SurrealDB命名空间
    pub surreal_ns: u32,
    /// 数据库IP地址
    pub db_ip: String,
    /// 数据库端口
    pub db_port: String,
    /// 数据库用户名
    pub db_user: String,
    /// 数据库密码
    pub db_password: String,
    /// 是否生成模型
    pub gen_model: bool,
    /// 是否生成网格
    pub gen_mesh: bool,
    /// 是否启用房间计算
    pub gen_spatial_tree: bool,
    /// 是否应用布尔运算
    pub apply_boolean_operation: bool,
    /// 网格容差比率
    pub mesh_tol_ratio: f64,
    /// 房间关键字
    pub room_keyword: String,
    /// 目标会话号（可选）：基于特定sesno的增量生成
    #[serde(default)]
    pub target_sesno: Option<u32>,
    /// Mesh 文件输出目录（可选）
    #[serde(default)]
    pub meshes_path: Option<String>,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            name: "默认配置".to_string(),
            manual_db_nums: vec![],
            manual_refnos: vec![],
            project_name: "AvevaMarineSample".to_string(),
            project_path: "/Users/dongpengcheng/Documents/models/e3d_models".to_string(),
            project_code: 1516,
            mdb_name: "ALL".to_string(),
            module: "DESI".to_string(),
            db_type: "surrealdb".to_string(),
            surreal_ns: 1516,
            db_ip: "localhost".to_string(),
            db_port: "8020".to_string(),  // 修改为与 DbOption.toml 一致的端口
            db_user: "root".to_string(),
            db_password: "root".to_string(),
            gen_model: true,
            gen_mesh: false,
            gen_spatial_tree: true,
            apply_boolean_operation: true,
            mesh_tol_ratio: 3.0,
            room_keyword: "-RM".to_string(),
            target_sesno: None,
            meshes_path: None,
        }
    }
}

impl DatabaseConfig {
    /// 根据 DbOption.toml 中的配置生成部署站点所需的数据库配置
    pub fn from_db_option(opt: &DbOption) -> Self {
        let manual_db_nums = opt.manual_db_nums.clone().unwrap_or_default();
        let project_code = opt.project_code.parse::<u32>().unwrap_or_default();
        let surreal_ns = opt.surreal_ns.parse::<u32>().unwrap_or(project_code);
        let mesh_tol_ratio = opt.mesh_tol_ratio.map(|v| v as f64).unwrap_or(3.0);
        let room_keyword = opt
            .get_room_key_word()
            .into_iter()
            .next()
            .unwrap_or_else(|| "-RM".to_string());

        DatabaseConfig {
            name: if opt.project_name.is_empty() {
                "DbOption 导入配置".to_string()
            } else {
                format!("{} 配置", opt.project_name)
            },
            manual_db_nums,
            manual_refnos: vec![],
            project_name: opt.project_name.clone(),
            project_path: opt.project_path.clone(),
            project_code,
            mdb_name: opt.mdb_name.clone(),
            module: opt.module.clone(),
            db_type: "surrealdb".to_string(),
            surreal_ns,
            db_ip: opt.ip.clone(),
            db_port: opt.port.clone(),
            db_user: opt.user.clone(),
            db_password: opt.password.clone(),
            gen_model: opt.gen_model,
            gen_mesh: opt.gen_mesh,
            gen_spatial_tree: opt.gen_spatial_tree,
            apply_boolean_operation: opt.apply_boolean_operation,
            mesh_tol_ratio,
            room_keyword,
            target_sesno: None,
            meshes_path: opt.meshes_path.clone(),
        }
    }
}

// ================= Projects =================

/// 项目状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProjectStatus {
    Deploying,
    Running,
    Failed,
    Stopped,
}

// ================= Deployment Sites =================

/// 部署站点状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeploymentSiteStatus {
    /// 配置中
    Configuring,
    /// 部署中
    Deploying,
    /// 运行中
    Running,
    /// 失败
    Failed,
    /// 已停止
    Stopped,
}

impl Default for DeploymentSiteStatus {
    fn default() -> Self {
        Self::Configuring
    }
}

/// E3D项目信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct E3dProjectInfo {
    /// 项目名称
    pub name: String,
    /// 项目路径
    pub path: String,
    /// 项目代码
    pub project_code: Option<u32>,
    /// 数据库文件数量
    pub db_file_count: u32,
    /// 项目大小（字节）
    pub size_bytes: u64,
    /// 最后修改时间
    #[serde(serialize_with = "serialize_system_time")]
    pub last_modified: SystemTime,
    /// 是否被选中
    pub selected: bool,
    /// 项目描述
    pub description: Option<String>,
}

/// 部署站点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentSite {
    /// SurrealDB 记录 ID
    pub id: Option<String>,
    /// 站点名称（唯一）
    pub name: String,
    /// 站点描述
    pub description: Option<String>,
    /// 包含的E3D项目列表
    pub e3d_projects: Vec<E3dProjectInfo>,
    /// 数据库配置
    pub config: DatabaseConfig,
    /// 站点状态
    #[serde(default)]
    pub status: DeploymentSiteStatus,
    /// 访问地址
    #[serde(default)]
    pub url: Option<String>,
    /// 健康检查地址
    #[serde(default)]
    pub health_url: Option<String>,
    /// 环境（prod/staging/dev）
    #[serde(default)]
    pub env: Option<String>,
    /// 负责人
    #[serde(default)]
    pub owner: Option<String>,
    /// 标签
    #[serde(default)]
    pub tags: Option<serde_json::Value>,
    /// 备注
    #[serde(default)]
    pub notes: Option<String>,
    /// 创建时间
    #[serde(serialize_with = "serialize_optional_system_time")]
    pub created_at: Option<SystemTime>,
    /// 更新时间
    #[serde(serialize_with = "serialize_optional_system_time")]
    pub updated_at: Option<SystemTime>,
    /// 最近健康检查
    #[serde(default)]
    pub last_health_check: Option<String>,
}

/// 创建部署站点请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentSiteCreateRequest {
    /// 站点名称
    pub name: String,
    /// 站点描述
    pub description: Option<String>,
    /// 根目录路径（用于扫描E3D项目）
    pub root_directory: Option<String>,
    /// 选中的项目路径列表
    pub selected_projects: Vec<String>,
    /// 数据库配置
    pub config: DatabaseConfig,
    /// 环境
    pub env: Option<String>,
    /// 负责人
    pub owner: Option<String>,
    /// 标签
    pub tags: Option<serde_json::Value>,
    /// 备注
    pub notes: Option<String>,
}

/// 从 DbOption 导入部署站点的请求
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeploymentSiteImportRequest {
    /// DbOption.toml 文件路径
    #[serde(default)]
    pub path: Option<String>,
    /// 覆盖默认生成的站点名称
    #[serde(default)]
    pub name: Option<String>,
    /// 描述
    #[serde(default)]
    pub description: Option<String>,
    /// 环境
    #[serde(default)]
    pub env: Option<String>,
    /// 负责人
    #[serde(default)]
    pub owner: Option<String>,
    /// 健康检查地址
    #[serde(default)]
    pub health_url: Option<String>,
    /// 标签
    #[serde(default)]
    pub tags: Option<serde_json::Value>,
    /// 备注
    #[serde(default)]
    pub notes: Option<String>,
}

/// 更新部署站点请求
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeploymentSiteUpdateRequest {
    /// 站点名称
    #[serde(default)]
    pub name: Option<String>,
    /// 站点描述
    #[serde(default)]
    pub description: Option<String>,
    /// 数据库配置
    #[serde(default)]
    pub config: Option<DatabaseConfig>,
    /// 站点状态
    #[serde(default)]
    pub status: Option<DeploymentSiteStatus>,
    /// 访问地址
    #[serde(default)]
    pub url: Option<String>,
    /// 环境
    #[serde(default)]
    pub env: Option<String>,
    /// 负责人
    #[serde(default)]
    pub owner: Option<String>,
    /// 健康检查地址
    #[serde(default)]
    pub health_url: Option<String>,
    /// 标签
    #[serde(default)]
    pub tags: Option<serde_json::Value>,
    /// 备注
    #[serde(default)]
    pub notes: Option<String>,
}

/// 部署站点查询参数
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeploymentSiteQuery {
    /// 搜索关键词
    #[serde(default)]
    pub q: Option<String>,
    /// 状态过滤
    #[serde(default)]
    pub status: Option<String>,
    /// 负责人过滤
    #[serde(default)]
    pub owner: Option<String>,
    /// 环境过滤
    #[serde(default)]
    pub env: Option<String>,
    /// 分页页码
    #[serde(default)]
    pub page: Option<u32>,
    /// 每页数量
    #[serde(default)]
    pub per_page: Option<u32>,
    /// 排序方式
    #[serde(default)]
    pub sort: Option<String>, // e.g., "updated_at:desc"
}

/// 部署站点任务配置请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentSiteTaskRequest {
    /// 站点ID
    pub site_id: String,
    /// 任务类型
    pub task_type: TaskType,
    /// 任务名称（可选，默认根据站点名称生成）
    pub task_name: Option<String>,
    /// 任务优先级
    #[serde(default)]
    pub priority: Option<TaskPriority>,
    /// 覆盖配置（可选）
    pub config_override: Option<DatabaseConfig>,
}

impl Default for ProjectStatus {
    fn default() -> Self {
        Self::Running
    }
}

/// 已部署项目（用于首页展示与 API 返回）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectItem {
    /// SurrealDB 记录 ID（如 "projects:xxxx"）
    pub id: Option<String>,
    /// 项目名（唯一）
    pub name: String,
    /// 版本号或镜像 tag
    #[serde(default)]
    pub version: Option<String>,
    /// 访问地址
    #[serde(default)]
    pub url: Option<String>,
    /// 环境（prod/staging/dev）
    #[serde(default)]
    pub env: Option<String>,
    /// 状态
    #[serde(default)]
    pub status: ProjectStatus,
    /// 负责人
    #[serde(default)]
    pub owner: Option<String>,
    /// 标签
    #[serde(default)]
    pub tags: Option<serde_json::Value>,
    /// 备注
    #[serde(default)]
    pub notes: Option<String>,
    /// 健康检查地址
    #[serde(default)]
    pub health_url: Option<String>,
    /// 上次健康检查
    #[serde(default)]
    pub last_health_check: Option<String>,
    /// 创建/更新时间（ISO8601 或毫秒）
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// 创建项目请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectCreateRequest {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub env: Option<String>,
    #[serde(default)]
    pub status: Option<ProjectStatus>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub tags: Option<serde_json::Value>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub health_url: Option<String>,
}

/// 更新项目请求（全部可选）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectUpdateRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub env: Option<String>,
    #[serde(default)]
    pub status: Option<ProjectStatus>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub tags: Option<serde_json::Value>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub health_url: Option<String>,
}

/// 查询项目请求参数
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub per_page: Option<u32>,
    #[serde(default)]
    pub sort: Option<String>, // e.g., "updated_at:desc"
}

/// 任务优先级
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskPriority {
    /// 低优先级
    Low = 1,
    /// 普通优先级
    Normal = 2,
    /// 高优先级
    High = 3,
    /// 紧急优先级
    Urgent = 4,
}

impl Default for TaskPriority {
    fn default() -> Self {
        TaskPriority::Normal
    }
}

/// 任务模板
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskTemplate {
    /// 模板ID
    pub id: String,
    /// 模板名称
    pub name: String,
    /// 模板描述
    pub description: String,
    /// 任务类型
    pub task_type: TaskType,
    /// 默认配置
    pub default_config: DatabaseConfig,
    /// 是否允许自定义配置
    pub allow_custom_config: bool,
    /// 预估执行时间（秒）
    pub estimated_duration: Option<u32>,
}

/// 批量任务配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchTaskConfig {
    /// 任务名称前缀
    pub name_prefix: String,
    /// 数据库编号列表
    pub db_nums: Vec<u32>,
    /// 是否并行执行
    pub parallel_execution: bool,
    /// 最大并发数
    pub max_concurrent: Option<u32>,
    /// 失败时是否继续
    pub continue_on_failure: bool,
}

/// 系统状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStatus {
    /// 系统运行时间
    pub uptime: Duration,
    /// CPU使用率
    pub cpu_usage: f32,
    /// 内存使用率
    pub memory_usage: f32,
    /// 活跃任务数
    pub active_tasks: u32,
    /// 数据库连接状态
    pub database_connected: bool,
    /// SurrealDB连接状态
    pub surrealdb_connected: bool,
}

/// 数据库信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseInfo {
    /// 数据库编号
    pub db_num: u32,
    /// 数据库名称
    pub name: String,
    /// 记录数量
    pub record_count: u64,
    /// 最后更新时间
    #[serde(serialize_with = "serialize_system_time")]
    pub last_updated: SystemTime,
    /// 是否可用
    pub available: bool,
}

/// 数据库状态信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbStatusInfo {
    /// 数据库编号
    pub dbnum: u32,
    /// 文件名
    pub file_name: String,
    /// 数据库类型 (DESI, CATA, DICT, SYST, GLB, GLOB)
    pub db_type: String,
    /// 项目名称
    pub project: String,
    /// 记录数量
    pub count: u64,
    /// 当前会话号
    pub sesno: u32,
    /// 最大ref1值
    pub max_ref1: u64,
    /// 最后更新时间
    #[serde(serialize_with = "serialize_system_time")]
    pub updated_at: SystemTime,
    /// 解析状态
    pub parse_status: ParseStatus,
    /// 模型生成状态
    pub model_status: ModelStatus,
    /// 网格生成状态
    pub mesh_status: MeshStatus,
    /// 文件版本信息
    pub file_version: Option<FileVersionInfo>,
    /// 是否需要更新
    pub needs_update: bool,
    /// 本地缓存最大sesno（redb）
    #[serde(default)]
    pub cached_sesno: Option<u32>,
    /// 当前文件最大sesno（PDMS）
    #[serde(default)]
    pub latest_file_sesno: Option<u32>,
    /// 自动更新类型（ParseOnly/ParseAndModel/Full）
    #[serde(default)]
    pub auto_update_type: Option<String>,
    /// 是否自动更新
    #[serde(default)]
    pub auto_update: bool,
    /// 是否正在更新
    #[serde(default)]
    pub updating: bool,
    /// 最后一次更新时间
    #[serde(serialize_with = "serialize_optional_system_time")]
    pub last_update_at: Option<SystemTime>,
    /// 最后一次更新结果（Success/Failed 等）
    pub last_update_result: Option<String>,
}

/// 解析状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ParseStatus {
    /// 未解析
    NotParsed,
    /// 解析中
    Parsing,
    /// 解析完成
    Parsed,
    /// 解析失败
    ParseFailed,
}

/// 模型生成状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ModelStatus {
    /// 未生成
    NotGenerated,
    /// 生成中
    Generating,
    /// 生成完成
    Generated,
    /// 生成失败
    GenerationFailed,
}

/// 网格生成状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MeshStatus {
    /// 未生成
    NotGenerated,
    /// 生成中
    Generating,
    /// 生成完成
    Generated,
    /// 生成失败
    GenerationFailed,
}

/// 文件版本信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileVersionInfo {
    /// 文件路径
    pub file_path: String,
    /// 文件版本号
    pub file_version: u32,
    /// 文件大小
    pub file_size: u64,
    /// 文件修改时间
    #[serde(serialize_with = "serialize_system_time")]
    pub file_modified: SystemTime,
    /// 是否存在
    pub exists: bool,
}

/// 增量更新请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncrementalUpdateRequest {
    /// 要更新的数据库编号列表
    pub dbnums: Vec<u32>,
    /// 是否强制更新
    pub force_update: bool,
    /// 更新类型
    pub update_type: UpdateType,
    /// 可选目标会话号
    #[serde(default)]
    pub target_sesno: Option<u32>,
}

/// 更新类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UpdateType {
    /// 仅解析数据
    ParseOnly,
    /// 解析并生成模型
    ParseAndModel,
    /// 完整更新（解析+模型+网格）
    Full,
}

/// 数据库状态查询参数
#[derive(Debug, Deserialize)]
pub struct DbStatusQuery {
    /// 项目名称过滤
    pub project: Option<String>,
    /// 数据库类型过滤
    pub db_type: Option<String>,
    /// 状态过滤
    pub status: Option<String>,
    /// 是否只显示需要更新的
    pub needs_update_only: Option<bool>,
    /// 分页大小
    pub limit: Option<usize>,
    /// 分页偏移
    pub offset: Option<usize>,
}

/// 项目信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    /// 项目名称
    pub name: String,
    /// 项目路径
    pub path: String,
    /// 项目代码
    pub project_code: Option<u32>,
    /// 数据库文件数量
    pub db_file_count: u32,
    /// 项目大小（字节）
    pub size_bytes: u64,
    /// 最后修改时间
    #[serde(serialize_with = "serialize_system_time")]
    pub last_modified: SystemTime,
    /// 是否可用
    pub available: bool,
    /// 项目描述
    pub description: Option<String>,
}

/// 目录扫描请求
#[derive(Debug, Deserialize)]
pub struct DirectoryScanRequest {
    /// 要扫描的目录路径
    pub directory_path: String,
    /// 是否递归扫描子目录
    pub recursive: bool,
    /// 最大扫描深度
    pub max_depth: Option<u32>,
}

/// 目录扫描结果
#[derive(Debug, Serialize)]
pub struct DirectoryScanResult {
    /// 扫描的根目录
    pub root_directory: String,
    /// 找到的项目列表
    pub projects: Vec<ProjectInfo>,
    /// 扫描耗时（毫秒）
    pub scan_duration_ms: u64,
    /// 扫描的目录总数
    pub scanned_directories: u32,
    /// 错误信息
    pub errors: Vec<String>,
}

/// 数据解析向导配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataParsingWizardConfig {
    /// 基础数据库配置
    pub base_config: DatabaseConfig,
    /// 选中的项目列表
    pub selected_projects: Vec<String>,
    /// 根目录路径
    pub root_directory: String,
    /// 是否并行处理项目
    pub parallel_processing: bool,
    /// 最大并发数
    pub max_concurrent: Option<u32>,
    /// 失败时是否继续
    pub continue_on_failure: bool,
    /// 输出目录
    pub output_directory: Option<String>,
}

/// 向导任务创建请求
#[derive(Debug, Deserialize)]
pub struct WizardTaskRequest {
    /// 任务名称
    pub task_name: String,
    /// 向导配置
    pub wizard_config: DataParsingWizardConfig,
    /// 任务优先级
    pub priority: Option<TaskPriority>,
    /// 任务模式：ParseOnly | FullGeneration（可选，默认 ParseOnly）
    #[serde(default)]
    pub task_mode: Option<String>,
}

/// 任务队列管理器
#[derive(Debug)]
pub struct TaskQueueManager {
    /// 等待队列（按优先级排序）
    pub pending_queue: VecDeque<String>,
    /// 正在执行的任务
    pub running_tasks: HashMap<String, TaskInfo>,
    /// 已完成的任务历史
    pub completed_tasks: Vec<TaskInfo>,
    /// 失败的任务
    pub failed_tasks: Vec<TaskInfo>,
    /// 任务模板
    pub task_templates: HashMap<String, TaskTemplate>,
    /// 最大并发执行数
    pub max_concurrent: usize,
}

impl TaskQueueManager {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            pending_queue: VecDeque::new(),
            running_tasks: HashMap::new(),
            completed_tasks: Vec::new(),
            failed_tasks: Vec::new(),
            task_templates: HashMap::new(),
            max_concurrent,
        }
    }

    /// 添加任务到队列
    pub fn enqueue_task(&mut self, task: TaskInfo) {
        // 根据优先级插入到合适的位置
        let task_id = task.id.clone();
        let priority = task.priority.clone();

        // 找到合适的插入位置（按优先级排序）
        let mut insert_pos = self.pending_queue.len();
        for (i, existing_id) in self.pending_queue.iter().enumerate() {
            if let Some(existing_task) = self.get_task_by_id(existing_id) {
                if priority > existing_task.priority {
                    insert_pos = i;
                    break;
                }
            }
        }

        self.pending_queue.insert(insert_pos, task_id);
    }

    /// 获取下一个可执行的任务
    pub fn get_next_executable_task(&mut self) -> Option<String> {
        if self.running_tasks.len() >= self.max_concurrent {
            return None;
        }

        // 查找没有未完成依赖的任务
        for i in 0..self.pending_queue.len() {
            let task_id = &self.pending_queue[i];
            if let Some(task) = self.get_task_by_id(task_id) {
                if self.are_dependencies_satisfied(&task.dependencies) {
                    return Some(self.pending_queue.remove(i).unwrap());
                }
            }
        }
        None
    }

    /// 检查依赖是否满足
    fn are_dependencies_satisfied(&self, dependencies: &[String]) -> bool {
        dependencies
            .iter()
            .all(|dep_id| self.completed_tasks.iter().any(|task| task.id == *dep_id))
    }

    /// 根据ID获取任务
    fn get_task_by_id(&self, task_id: &str) -> Option<&TaskInfo> {
        if let Some(task) = self.running_tasks.get(task_id) {
            return Some(task);
        }

        for task in &self.completed_tasks {
            if task.id == task_id {
                return Some(task);
            }
        }

        for task in &self.failed_tasks {
            if task.id == task_id {
                return Some(task);
            }
        }

        None
    }

    /// 创建批量任务
    pub fn create_batch_tasks(
        &mut self,
        template_id: &str,
        batch_config: BatchTaskConfig,
    ) -> Result<Vec<String>, String> {
        // 先克隆模板以避免借用冲突
        let template = self
            .task_templates
            .get(template_id)
            .ok_or_else(|| format!("任务模板 {} 不存在", template_id))?
            .clone();

        let mut task_ids = Vec::new();
        let mut previous_task_id = None;

        for (i, db_num) in batch_config.db_nums.iter().enumerate() {
            let task_name = format!("{} - 数据库 {}", batch_config.name_prefix, db_num);

            let mut config = template.default_config.clone();
            config.manual_db_nums = vec![*db_num];
            config.name = task_name.clone();

            let mut task = TaskInfo::new(task_name, template.task_type.clone(), config);
            task.estimated_duration = template.estimated_duration;

            // 如果不是并行执行，添加依赖关系
            if !batch_config.parallel_execution {
                if let Some(prev_id) = previous_task_id {
                    task.dependencies.push(prev_id);
                }
            }

            task_ids.push(task.id.clone());
            previous_task_id = Some(task.id.clone());

            self.enqueue_task(task);
        }

        Ok(task_ids)
    }
}

impl TaskInfo {
    /// 生成任务ID格式: 站点名称_任务名_流水号
    fn generate_task_id(site_name: &str, task_name: &str) -> String {
        let counter = TASK_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
        let site_part = site_name
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
            .take(20)
            .collect::<String>();
        let task_part = task_name
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
            .take(20)
            .collect::<String>();

        // 如果站点名或任务名为空，使用默认值
        let site_part = if site_part.is_empty() {
            "default"
        } else {
            &site_part
        };
        let task_part = if task_part.is_empty() {
            "task"
        } else {
            &task_part
        };

        format!("{}_{}_{}", site_part, task_part, counter)
    }

    pub fn new(name: String, task_type: TaskType, config: DatabaseConfig) -> Self {
        // 从配置中提取站点名称，如果没有则使用"default"
        let site_name = config.name.as_str();
        let task_id = Self::generate_task_id(site_name, &name);

        Self {
            id: task_id,
            name,
            task_type,
            status: TaskStatus::Pending,
            config,
            created_at: SystemTime::now(),
            started_at: None,
            completed_at: None,
            progress: TaskProgress::default(),
            error: None,
            error_details: None,
            logs: Vec::new(),
            priority: TaskPriority::default(),
            dependencies: Vec::new(),
            estimated_duration: None,
            actual_duration: None,
        }
    }

    pub fn new_with_priority(
        name: String,
        task_type: TaskType,
        config: DatabaseConfig,
        priority: TaskPriority,
    ) -> Self {
        let mut task = Self::new(name, task_type, config);
        task.priority = priority;
        task
    }

    pub fn new_with_dependencies(
        name: String,
        task_type: TaskType,
        config: DatabaseConfig,
        dependencies: Vec<String>,
    ) -> Self {
        let mut task = Self::new(name, task_type, config);
        task.dependencies = dependencies;
        task
    }

    pub fn add_log(&mut self, level: LogLevel, message: String) {
        self.add_log_with_details(level, message, None, None);
    }

    pub fn add_log_with_details(
        &mut self,
        level: LogLevel,
        message: String,
        error_code: Option<String>,
        stack_trace: Option<String>,
    ) {
        self.logs.push(LogEntry {
            timestamp: SystemTime::now(),
            level,
            message,
            error_code,
            stack_trace,
        });
    }

    pub fn set_error_details(&mut self, error_details: ErrorDetails) {
        self.error = Some(error_details.detailed_message.clone());
        self.error_details = Some(error_details);
    }

    pub fn update_progress(&mut self, step: String, current: u32, total: u32, percentage: f32) {
        self.progress.current_step = step;
        self.progress.current_step_number = current;
        self.progress.total_steps = total;
        self.progress.percentage = percentage;
    }
}

impl Default for TaskProgress {
    fn default() -> Self {
        Self {
            current_step: "初始化".to_string(),
            total_steps: 1,
            current_step_number: 0,
            percentage: 0.0,
            processed_items: 0,
            total_items: 0,
            estimated_remaining_seconds: None,
        }
    }
}

/// 任务日志查询参数
#[derive(Debug, Deserialize)]
pub struct TaskLogQuery {
    /// 日志级别过滤
    pub level: Option<String>,
    /// 搜索关键词
    pub search: Option<String>,
    /// 分页限制
    pub limit: Option<usize>,
    /// 分页偏移
    pub offset: Option<usize>,
}

// ===== 空间计算交互模型 =====

/// 支架-桥架识别 请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuppoTraysRequest {
    pub dbnum: u32,
    pub suppo_refno: u64,
    #[serde(default)]
    pub tolerance: Option<f64>,
}

/// 预埋板识别 请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FittingRequest {
    pub dbnum: u32,
    pub suppo_refno: u64,
    #[serde(default)]
    pub tolerance: Option<f64>,
}

/// 距墙/定位块 距离 请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WallDistanceRequest {
    pub dbnum: u32,
    pub suppo_refno: u64,
    /// S1 | S2
    pub suppo_type: String,
    #[serde(default)]
    pub search_radius: Option<f64>,
}

/// 与预埋板相对定位 请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FittingOffsetRequest {
    pub dbnum: u32,
    pub suppo_refno: u64,
    #[serde(default)]
    pub tolerance: Option<f64>,
}

/// 与钢结构相对定位 请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SteelRelativeRequest {
    pub dbnum: u32,
    pub suppo_refno: u64,
    /// S1 | S2
    pub suppo_type: String,
    #[serde(default)]
    pub search_radius: Option<f64>,
}

/// 托盘跨度 请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraySpanRequest {
    pub dbnum: u32,
    /// 如果缺省则从 suppo_refno 推断其所属托盘
    #[serde(default)]
    pub tray_id: Option<u64>,
    #[serde(default)]
    pub suppo_refno: Option<u64>,
    #[serde(default)]
    pub neighbor_window: Option<f64>,
}

// ===== 基于 Refno 的模型生成 =====

/// 基于 Refno 的模型生成请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefnoModelGenerationRequest {
    /// 数据库编号
    pub db_num: u32,
    /// Refno 列表 (字符串格式，支持 "123" 或 "1/456" 等)
    pub refnos: Vec<String>,
    /// 是否生成网格 (可选，默认从配置读取)
    #[serde(default)]
    pub gen_mesh: Option<bool>,
    /// 是否生成模型 (可选，默认从配置读取)
    #[serde(default)]
    pub gen_model: Option<bool>,
    /// 是否应用布尔运算 (可选，默认从配置读取)
    #[serde(default)]
    pub apply_boolean_operation: Option<bool>,
    /// Mesh 文件输出目录 (可选，默认从配置读取)
    #[serde(default)]
    pub meshes_path: Option<String>,
    /// 🆕 客户端指定的任务 ID (可选)
    ///
    /// 如果提供，服务器将使用此 ID 创建和跟踪任务，而不是自动生成。
    /// 这确保前后端使用相同的 task_id 进行 WebSocket 订阅。
    #[serde(default)]
    pub task_id: Option<String>,
}

/// 基于 Refno 的模型生成响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefnoModelGenerationResponse {
    /// 是否成功
    pub success: bool,
    /// 任务ID
    pub task_id: String,
    /// 任务状态
    pub status: TaskStatus,
    /// 提示信息
    pub message: String,
    /// 处理的 refno 数量
    pub refno_count: usize,
}

// ===== 房间模型重新生成 =====

/// 房间模型重新生成请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomRegenerateRequest {
    /// 数据库编号
    pub db_num: u32,
    /// 房间关键词列表（可选，默认从配置读取）
    #[serde(default)]
    pub room_keywords: Option<Vec<String>>,
    /// 是否强制重新生成所有模型（默认 true）
    #[serde(default = "default_true")]
    pub force_regenerate: bool,
    /// 是否生成网格（默认 true）
    #[serde(default = "default_true")]
    pub gen_mesh: bool,
    /// 是否应用布尔运算（默认 true）
    #[serde(default = "default_true")]
    pub apply_boolean_operation: bool,
}

fn default_true() -> bool {
    true
}

/// 房间模型重新生成响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomRegenerateResponse {
    /// 是否成功
    pub success: bool,
    /// 任务ID
    pub task_id: String,
    /// 任务状态
    pub status: TaskStatus,
    /// 提示信息
    pub message: String,
    /// 查询到的房间数量
    pub room_count: usize,
    /// 需要生成的元素数量
    pub element_count: usize,
}

/// 房间模型重新生成状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomRegenerateStatus {
    /// 任务ID
    pub task_id: String,
    /// 当前阶段
    pub phase: RoomRegeneratePhase,
    /// 进度百分比 (0-100)
    pub progress: f32,
    /// 状态消息
    pub message: String,
    /// 查询到的房间数量
    pub room_count: usize,
    /// 需要生成的元素数量
    pub element_count: usize,
    /// 已生成的元素数量
    pub generated_count: usize,
    /// 房间关系更新状态
    pub room_relation_status: Option<RoomRelationUpdateStatus>,
}

/// 房间模型重新生成阶段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RoomRegeneratePhase {
    /// 查询房间参考号
    QueryingRooms,
    /// 生成模型
    GeneratingModels,
    /// 更新房间关系
    UpdatingRoomRelations,
    /// 完成
    Completed,
    /// 失败
    Failed,
}

/// 房间关系更新状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomRelationUpdateStatus {
    /// 是否完成
    pub completed: bool,
    /// 影响的房间数量
    pub affected_rooms: usize,
    /// 更新的元素数量
    pub updated_elements: usize,
    /// 耗时（毫秒）
    pub duration_ms: u64,
    /// 状态消息
    pub message: String,
}
