# E3D模型生成系统重构建议分析

## 概述

基于对E3D模型生成系统的深入代码分析，本文档识别了系统中存在的主要代码异味和重构机会，并提供了具体的重构建议和实施方案。

## 1. 主要代码异味识别

### 1.1 大型函数问题 🔴

#### 问题描述
系统中存在多个超过100行的大型函数，违反了单一职责原则：

**典型案例**：
- `src/fast_model/cata_model.rs::gen_cata_geos()` - 约1350行
- `src/fast_model/occ_generate.rs::apply_insts_boolean_occ()` - 约300行
- `src/fast_model/gen_model.rs::gen_geos_data()` - 约200行

#### 重构建议
```rust
// 重构前：大型函数
pub async fn gen_cata_geos(/* 大量参数 */) -> anyhow::Result<bool> {
    // 1350行代码混合了多种职责
    // - 数据查询
    // - 几何计算
    // - 性能统计
    // - 错误处理
    // - 结果发送
}

// 重构后：职责分离
pub struct CataGeometryGenerator {
    db_option: Arc<DbOption>,
    performance_tracker: PerformanceTracker,
    error_handler: ErrorHandler,
}

impl CataGeometryGenerator {
    pub async fn generate(&self, refnos: &[RefnoEnum]) -> Result<GenerationResult> {
        let query_result = self.query_cata_data(refnos).await?;
        let geometries = self.compute_geometries(query_result).await?;
        let result = self.process_results(geometries).await?;
        Ok(result)
    }
    
    async fn query_cata_data(&self, refnos: &[RefnoEnum]) -> Result<CataQueryResult> { /* ... */ }
    async fn compute_geometries(&self, data: CataQueryResult) -> Result<Vec<Geometry>> { /* ... */ }
    async fn process_results(&self, geometries: Vec<Geometry>) -> Result<GenerationResult> { /* ... */ }
}
```

### 1.2 魔法数字和硬编码常量 🔴

#### 问题描述
代码中存在大量魔法数字和硬编码值：

**典型案例**：
```rust
// src/fast_model/aabb_tree.rs
const QUERY_CHUNK_SIZE: usize = 1000;
const PROCESS_CHUNK_SIZE: usize = 100;

// src/fast_model/occ_generate.rs
for chunk in refnos.chunks(100) { /* ... */ }

// src/fast_model/cata_model.rs
let chunk = (params.len() / 16).max(1);

// src/consts.rs
pub const BATCH_CHUNKS_CNT: usize = 50;
```

#### 重构建议
```rust
// 创建配置结构体
#[derive(Debug, Clone)]
pub struct ProcessingConfig {
    pub query_chunk_size: usize,
    pub process_chunk_size: usize,
    pub parallel_workers: usize,
    pub batch_size: usize,
}

impl Default for ProcessingConfig {
    fn default() -> Self {
        Self {
            query_chunk_size: 1000,
            process_chunk_size: 100,
            parallel_workers: 16,
            batch_size: 50,
        }
    }
}

// 使用配置驱动的处理
pub struct GeometryProcessor {
    config: ProcessingConfig,
}

impl GeometryProcessor {
    pub async fn process_in_chunks<T>(&self, items: &[T]) -> Result<Vec<ProcessResult>> {
        let chunk_size = self.config.process_chunk_size;
        let workers = self.config.parallel_workers;
        
        for chunk in items.chunks(chunk_size) {
            // 使用配置值而不是硬编码
        }
    }
}
```

### 1.3 重复的错误处理模式 🔴

#### 问题描述
系统中存在大量重复的错误处理代码：

**典型案例**：
```rust
// 重复模式1：数据库查询错误处理
match SUL_DB.query(&sql).await {
    Ok(mut response) => {
        let r = response.take::<Vec<QueryGeoParam>>(0);
        if let Err(e) = &r {
            init_deserialize_error("Vec<QueryGeoParam>", e, &sql, &std::panic::Location::caller().to_string());
            return;
        }
        let result: Vec<QueryGeoParam> = r.unwrap();
    }
    Err(e) => {
        dbg!(&e);
    }
}

// 重复模式2：文件操作错误处理
match load_manifold(&dir, &id, transform, false) {
    Ok(manifold) => { /* 处理成功情况 */ }
    Err(_) => {
        println!("布尔运算失败: 无法加载manifold, refno: {}", refno);
        update_sql.push_str(&format!("update {}<-inst_relate set bad_bool=true;", inst_info_id));
        continue;
    }
}
```

#### 重构建议
```rust
// 创建统一的错误处理器
pub struct DatabaseErrorHandler;

impl DatabaseErrorHandler {
    pub async fn execute_query<T>(&self, sql: &str) -> Result<Vec<T>> 
    where 
        T: serde::de::DeserializeOwned + std::fmt::Debug,
    {
        let mut response = SUL_DB.query(sql).await
            .map_err(|e| DatabaseError::QueryFailed(sql.to_string(), e))?;
            
        response.take::<Vec<T>>(0)
            .map_err(|e| {
                self.log_deserialization_error::<T>(sql, &e);
                DatabaseError::DeserializationFailed(sql.to_string(), e)
            })
    }
    
    fn log_deserialization_error<T>(&self, sql: &str, error: &surrealdb::Error) {
        tracing::error!(
            type_name = std::any::type_name::<T>(),
            sql = sql,
            error = %error,
            location = %std::panic::Location::caller(),
            "Database deserialization failed"
        );
    }
}

// 创建业务特定的错误处理
pub struct GeometryErrorHandler;

impl GeometryErrorHandler {
    pub async fn handle_manifold_load_error(&self, refno: RefnoEnum, inst_info_id: &str) -> String {
        tracing::warn!(refno = %refno, "Failed to load manifold for boolean operation");
        format!("update {}<-inst_relate set bad_bool=true;", inst_info_id)
    }
}
```

### 1.4 过度复杂的SQL字符串构建 🔴

#### 问题描述
系统中存在大量复杂的SQL字符串拼接：

**典型案例**：
```rust
// src/fast_model/occ_generate.rs
let mut sql = format!(
    r#"
    select
            in as refno,
            in.noun as noun,
            world_trans.d as wt,
            aabb.d as aabb,
            (select value [out.param, trans.d] from out->geo_relate) as ts,
            (select value [in, world_trans.d, (select out.param as param, geo_type, trans.d as trans,
            out.aabb.d as aabb, object::keys(out.param)[0] as para_type
            from out->geo_relate where geo_type in ["Neg", "CataCrossNeg"] and trans.d != NONE )]
        from array::flatten(in<-neg_relate.in->inst_relate) ) as neg_ts from {} where in.id != none and !bad_bool
        and (in<-neg_relate)[0] != none and aabb.d!=none
    "#,
    inst_keys
);
```

#### 重构建议
```rust
// 创建查询构建器
pub struct SurrealQueryBuilder {
    select_fields: Vec<String>,
    from_clause: String,
    where_conditions: Vec<String>,
    joins: Vec<String>,
}

impl SurrealQueryBuilder {
    pub fn new() -> Self {
        Self {
            select_fields: Vec::new(),
            from_clause: String::new(),
            where_conditions: Vec::new(),
            joins: Vec::new(),
        }
    }
    
    pub fn select(mut self, field: &str) -> Self {
        self.select_fields.push(field.to_string());
        self
    }
    
    pub fn from(mut self, table: &str) -> Self {
        self.from_clause = table.to_string();
        self
    }
    
    pub fn where_condition(mut self, condition: &str) -> Self {
        self.where_conditions.push(condition.to_string());
        self
    }
    
    pub fn build(self) -> String {
        let select_clause = self.select_fields.join(", ");
        let where_clause = if self.where_conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", self.where_conditions.join(" AND "))
        };
        
        format!("SELECT {} FROM {}{}", select_clause, self.from_clause, where_clause)
    }
}

// 使用查询构建器
pub struct GeometryQueries;

impl GeometryQueries {
    pub fn build_boolean_operation_query(inst_keys: &str) -> String {
        SurrealQueryBuilder::new()
            .select("in as refno")
            .select("in.noun as noun")
            .select("world_trans.d as wt")
            .select("aabb.d as aabb")
            .select("(select value [out.param, trans.d] from out->geo_relate) as ts")
            .from(inst_keys)
            .where_condition("in.id != none")
            .where_condition("!bad_bool")
            .where_condition("(in<-neg_relate)[0] != none")
            .where_condition("aabb.d != none")
            .build()
    }
}
```

## 2. 架构层面的重构建议

### 2.1 引入依赖注入容器 🟡

#### 问题描述
当前系统中存在大量全局静态变量和硬编码依赖：

```rust
// src/defines.rs
lazy_static! {
    pub static ref PDMS_ATT_MAP_CACHE: CacheMgr<NamedAttrMap> = CacheMgr::new("ATTR_MAP_CACHE", false);
    pub static ref PDMS_ANCESTOR_CACHE: CacheMgr<RefU64Vec> = CacheMgr::new("ANCESTOR_CACHE", false);
}

// src/fast_model/mod.rs
pub static EXIST_MESH_GEO_HASHES: Lazy<DashMap<String, Aabb>> = Lazy::new(DashMap::new);
```

#### 重构建议
```rust
// 创建服务容器
pub struct ServiceContainer {
    cache_manager: Arc<CacheManager>,
    database_manager: Arc<DatabaseManager>,
    geometry_processor: Arc<GeometryProcessor>,
    performance_tracker: Arc<PerformanceTracker>,
}

impl ServiceContainer {
    pub fn new(config: &SystemConfig) -> Self {
        let cache_manager = Arc::new(CacheManager::new(&config.cache_config));
        let database_manager = Arc::new(DatabaseManager::new(&config.db_config));
        let geometry_processor = Arc::new(GeometryProcessor::new(
            Arc::clone(&cache_manager),
            Arc::clone(&database_manager),
        ));
        let performance_tracker = Arc::new(PerformanceTracker::new());
        
        Self {
            cache_manager,
            database_manager,
            geometry_processor,
            performance_tracker,
        }
    }
}

// 使用依赖注入
pub struct ModelGenerator {
    services: Arc<ServiceContainer>,
}

impl ModelGenerator {
    pub fn new(services: Arc<ServiceContainer>) -> Self {
        Self { services }
    }
    
    pub async fn generate_model(&self, request: GenerationRequest) -> Result<GenerationResult> {
        // 使用注入的服务而不是全局变量
        let cached_data = self.services.cache_manager.get_cached_data(&request.refno).await?;
        let geometry = self.services.geometry_processor.process(cached_data).await?;
        Ok(GenerationResult { geometry })
    }
}
```

### 2.2 实现策略模式处理不同模型类型 🟡

#### 问题描述
当前系统中存在大量条件分支处理不同的模型类型：

```rust
// 当前的条件分支处理
match model_type {
    "PRIM" => gen_prim_geos(db_option, refnos, sender).await?,
    "LOOP" => gen_loop_geos(db_option, refnos, sjus_map, sender).await?,
    "CATA" => gen_cata_geos(db_option, refnos, branch_map, sjus_map, sender).await?,
    _ => return Err(anyhow!("Unknown model type")),
}
```

#### 重构建议
```rust
// 定义策略接口
#[async_trait]
pub trait ModelGenerationStrategy {
    async fn generate(&self, context: &GenerationContext) -> Result<GenerationResult>;
    fn supports_type(&self, model_type: &str) -> bool;
}

// 实现具体策略
pub struct PrimModelStrategy {
    db_option: Arc<DbOption>,
}

#[async_trait]
impl ModelGenerationStrategy for PrimModelStrategy {
    async fn generate(&self, context: &GenerationContext) -> Result<GenerationResult> {
        gen_prim_geos(&self.db_option, &context.refnos, context.sender.clone()).await
    }
    
    fn supports_type(&self, model_type: &str) -> bool {
        model_type == "PRIM"
    }
}

// 策略管理器
pub struct ModelGenerationStrategyManager {
    strategies: Vec<Box<dyn ModelGenerationStrategy + Send + Sync>>,
}

impl ModelGenerationStrategyManager {
    pub fn new() -> Self {
        Self {
            strategies: vec![
                Box::new(PrimModelStrategy::new()),
                Box::new(LoopModelStrategy::new()),
                Box::new(CataModelStrategy::new()),
            ],
        }
    }
    
    pub async fn generate(&self, model_type: &str, context: &GenerationContext) -> Result<GenerationResult> {
        let strategy = self.strategies
            .iter()
            .find(|s| s.supports_type(model_type))
            .ok_or_else(|| anyhow!("No strategy found for model type: {}", model_type))?;
            
        strategy.generate(context).await
    }
}
```

### 2.3 引入观察者模式处理性能监控 🟡

#### 问题描述
当前系统中性能统计代码散布在各处：

```rust
// 分散的性能统计代码
let mut db_time_get_named_attmap = 0;
let mut db_time_get_world_transform = 0;
let mut db_time_query_single = 0;
// ... 更多统计变量

*stats.entry("query_single".to_string()).or_insert(0) += db_time_query_single as u64;
```

#### 重构建议
```rust
// 定义性能事件
#[derive(Debug, Clone)]
pub enum PerformanceEvent {
    DatabaseQueryStarted { operation: String, sql: String },
    DatabaseQueryCompleted { operation: String, duration: Duration },
    GeometryGenerationStarted { model_type: String, refno: RefnoEnum },
    GeometryGenerationCompleted { model_type: String, refno: RefnoEnum, duration: Duration },
}

// 性能观察者接口
pub trait PerformanceObserver {
    fn on_event(&self, event: PerformanceEvent);
}

// 性能监控器
pub struct PerformanceMonitor {
    observers: Vec<Box<dyn PerformanceObserver + Send + Sync>>,
}

impl PerformanceMonitor {
    pub fn notify(&self, event: PerformanceEvent) {
        for observer in &self.observers {
            observer.on_event(event.clone());
        }
    }
    
    pub fn time_operation<F, R>(&self, operation: &str, f: F) -> R 
    where 
        F: FnOnce() -> R,
    {
        let start = Instant::now();
        let result = f();
        let duration = start.elapsed();
        
        self.notify(PerformanceEvent::DatabaseQueryCompleted {
            operation: operation.to_string(),
            duration,
        });
        
        result
    }
}
```

## 3. 数据访问层重构

### 3.1 统一数据访问接口 🟡

#### 问题描述
当前系统中存在多种数据库访问方式，缺乏统一的抽象：

```rust
// MySQL访问
let result = sqlx::query(&sql).fetch_one(pool).await?;

// SurrealDB访问
let mut response = SUL_DB.query(&sql).await.unwrap();

// ArangoDB访问
let data_vec: Vec<T> = database.aql_query(aql).await?;
```

#### 重构建议
```rust
// 统一数据访问接口
#[async_trait]
pub trait DataRepository {
    type Error;
    
    async fn query<T>(&self, query: &str) -> Result<Vec<T>, Self::Error>
    where 
        T: serde::de::DeserializeOwned;
        
    async fn execute(&self, command: &str) -> Result<u64, Self::Error>;
}

// MySQL实现
pub struct MySqlRepository {
    pool: Pool<MySql>,
}

#[async_trait]
impl DataRepository for MySqlRepository {
    type Error = sqlx::Error;
    
    async fn query<T>(&self, query: &str) -> Result<Vec<T>, Self::Error>
    where 
        T: serde::de::DeserializeOwned,
    {
        sqlx::query_as::<_, T>(query)
            .fetch_all(&self.pool)
            .await
    }
}

// SurrealDB实现
pub struct SurrealRepository;

#[async_trait]
impl DataRepository for SurrealRepository {
    type Error = surrealdb::Error;
    
    async fn query<T>(&self, query: &str) -> Result<Vec<T>, Self::Error>
    where 
        T: serde::de::DeserializeOwned,
    {
        let mut response = SUL_DB.query(query).await?;
        response.take(0)
    }
}
```

## 4. 实施优先级和时间估算

### 高优先级重构 (1-2周) 🔴
1. **大型函数拆分** - 将超过100行的函数拆分为更小的职责单一的函数
2. **魔法数字消除** - 创建配置结构体替换硬编码值
3. **错误处理统一** - 实现统一的错误处理机制

### 中优先级重构 (2-4周) 🟡  
1. **依赖注入实现** - 减少全局变量依赖
2. **策略模式应用** - 重构模型类型处理逻辑
3. **数据访问层统一** - 创建统一的数据访问接口

### 低优先级重构 (4-8周) 🟢
1. **观察者模式实现** - 重构性能监控系统
2. **查询构建器实现** - 替换复杂的SQL字符串拼接
3. **缓存策略优化** - 实现更灵活的缓存管理

## 5. 重构风险评估

### 高风险区域 ⚠️
- **核心几何生成逻辑** - 需要充分的测试覆盖
- **数据库查询逻辑** - 可能影响性能
- **并发处理代码** - 需要仔细处理线程安全

### 风险缓解策略
1. **渐进式重构** - 逐步替换而不是大规模重写
2. **充分测试** - 每个重构步骤都要有对应的测试
3. **性能基准** - 重构前后进行性能对比
4. **回滚计划** - 准备快速回滚机制

这个重构计划将显著提升代码的可维护性、可测试性和可扩展性，同时降低系统的复杂度和维护成本。

## 6. 配置管理重构

### 6.1 配置文件碎片化问题 🔴

#### 问题描述
当前系统存在多个配置文件，配置分散且重复：

```toml
# DbOption.toml
manual_db_nums = [1112]
debug_refno_types = ["CATA", "LOOP", "PRIM"]

# DbOption-aba.toml
manual_db_nums = [7326]
debug_refno_types = ["CATA", "LOOP", "PRIM"]

# DbOption-ams.toml
# 类似的重复配置...
```

#### 重构建议
```rust
// 创建分层配置系统
#[derive(Debug, Clone, Deserialize)]
pub struct SystemConfig {
    pub database: DatabaseConfig,
    pub processing: ProcessingConfig,
    pub debug: DebugConfig,
    pub performance: PerformanceConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub mysql: MySqlConfig,
    pub surreal: SurrealConfig,
    pub arango: ArangoConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProcessingConfig {
    pub batch_size: usize,
    pub parallel_workers: usize,
    pub chunk_sizes: ChunkSizeConfig,
}

// 配置管理器
pub struct ConfigManager {
    base_config: SystemConfig,
    environment_overrides: HashMap<String, serde_json::Value>,
}

impl ConfigManager {
    pub fn load() -> Result<Self> {
        let mut config = Config::builder()
            .add_source(File::with_name("config/base"))
            .add_source(File::with_name(&format!("config/{}", env::var("ENVIRONMENT").unwrap_or_else(|_| "development".to_string()))))
            .add_source(Environment::with_prefix("E3D"))
            .build()?;

        let base_config: SystemConfig = config.try_deserialize()?;

        Ok(Self {
            base_config,
            environment_overrides: HashMap::new(),
        })
    }

    pub fn get_database_config(&self) -> &DatabaseConfig {
        &self.base_config.database
    }
}
```

### 6.2 常量管理优化 🟡

#### 问题描述
常量定义分散在多个文件中，缺乏分类和文档：

```rust
// src/consts.rs - 混合了各种类型的常量
pub const ARANGODB_SAVE_AMOUNT: usize = 10000;
pub const PDMS_EXPLICIT_TABLE: &'static str = "EXPLICIT_ATT";
pub const BATCH_CHUNKS_CNT: usize = 50;
```

#### 重构建议
```rust
// 按功能域分类常量
pub mod database_constants {
    pub mod table_names {
        pub const PDMS_EXPLICIT_TABLE: &str = "EXPLICIT_ATT";
        pub const PDMS_UDA_ATT_TABLE: &str = "UDA_ATT";
        pub const PDMS_ELEMENTS_TABLE: &str = "PDMS_ELEMENTS";
    }

    pub mod batch_sizes {
        pub const ARANGODB_SAVE_AMOUNT: usize = 10000;
        pub const DEFAULT_QUERY_BATCH_SIZE: usize = 1000;
    }
}

pub mod processing_constants {
    pub mod chunk_sizes {
        pub const DEFAULT_BATCH_CHUNKS: usize = 50;
        pub const GEOMETRY_PROCESS_CHUNK: usize = 100;
    }

    pub mod timeouts {
        pub const DATABASE_TIMEOUT_SECS: u64 = 600; // 10 minutes
        pub const GEOMETRY_GENERATION_TIMEOUT_SECS: u64 = 300; // 5 minutes
    }
}

// 使用类型安全的常量
#[derive(Debug, Clone, Copy)]
pub struct BatchSize(pub usize);

impl BatchSize {
    pub const DEFAULT: Self = Self(50);
    pub const LARGE: Self = Self(1000);
    pub const SMALL: Self = Self(10);
}
```

## 7. 测试架构重构

### 7.1 测试代码组织问题 🔴

#### 问题描述
当前测试代码混合在业务代码中，缺乏系统性：

```rust
// src/fast_model/occ_generate.rs
#[tokio::test]
pub async fn test_gen_geos() -> anyhow::Result<()> {
    init_test_surreal().await;
    process_meshes_update_db_deep_default((&["17496/171559".into(), "24381/35844".into()]))
        .await
        .unwrap();
    Ok(())
}
```

#### 重构建议
```rust
// tests/integration/geometry_generation.rs
pub struct GeometryGenerationTestSuite {
    test_db: TestDatabase,
    test_config: TestConfig,
}

impl GeometryGenerationTestSuite {
    pub async fn new() -> Self {
        let test_db = TestDatabase::setup().await;
        let test_config = TestConfig::load_test_config();

        Self { test_db, test_config }
    }

    pub async fn test_prim_geometry_generation(&self) -> Result<()> {
        // 准备测试数据
        let test_refnos = self.test_db.create_test_prim_elements().await?;

        // 执行测试
        let generator = PrimModelStrategy::new(Arc::new(self.test_config.db_option.clone()));
        let context = GenerationContext::new(test_refnos, self.test_db.get_sender());
        let result = generator.generate(&context).await?;

        // 验证结果
        assert!(!result.geometries.is_empty());
        assert!(result.performance_metrics.total_time < Duration::from_secs(10));

        Ok(())
    }
}

// tests/unit/query_builder.rs
#[cfg(test)]
mod query_builder_tests {
    use super::*;

    #[test]
    fn test_simple_select_query() {
        let query = SurrealQueryBuilder::new()
            .select("id")
            .select("name")
            .from("users")
            .where_condition("active = true")
            .build();

        assert_eq!(query, "SELECT id, name FROM users WHERE active = true");
    }
}

// 测试工具类
pub struct TestDatabase {
    connection: TestConnection,
}

impl TestDatabase {
    pub async fn setup() -> Self {
        let connection = TestConnection::create_in_memory().await;
        Self { connection }
    }

    pub async fn create_test_prim_elements(&self) -> Result<Vec<RefnoEnum>> {
        // 创建测试用的基本体元素
        todo!()
    }

    pub async fn cleanup(&self) -> Result<()> {
        self.connection.drop_all_tables().await
    }
}
```

### 7.2 模拟和存根系统 🟡

#### 问题描述
当前系统缺乏有效的模拟机制，测试依赖真实数据库：

#### 重构建议
```rust
// 创建模拟接口
#[async_trait]
pub trait DatabaseService {
    async fn query_geometry_data(&self, refnos: &[RefnoEnum]) -> Result<Vec<GeometryData>>;
    async fn save_mesh_data(&self, mesh: &MeshData) -> Result<()>;
}

// 真实实现
pub struct RealDatabaseService {
    mysql_pool: Pool<MySql>,
    surreal_client: SurrealClient,
}

#[async_trait]
impl DatabaseService for RealDatabaseService {
    async fn query_geometry_data(&self, refnos: &[RefnoEnum]) -> Result<Vec<GeometryData>> {
        // 真实的数据库查询逻辑
        todo!()
    }
}

// 模拟实现
pub struct MockDatabaseService {
    geometry_data: HashMap<RefnoEnum, GeometryData>,
    call_log: Arc<Mutex<Vec<DatabaseCall>>>,
}

#[async_trait]
impl DatabaseService for MockDatabaseService {
    async fn query_geometry_data(&self, refnos: &[RefnoEnum]) -> Result<Vec<GeometryData>> {
        self.call_log.lock().await.push(DatabaseCall::QueryGeometry(refnos.to_vec()));

        Ok(refnos.iter()
            .filter_map(|refno| self.geometry_data.get(refno).cloned())
            .collect())
    }
}

impl MockDatabaseService {
    pub fn with_geometry_data(data: HashMap<RefnoEnum, GeometryData>) -> Self {
        Self {
            geometry_data: data,
            call_log: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn verify_query_called(&self, expected_refnos: &[RefnoEnum]) -> bool {
        let calls = self.call_log.lock().await;
        calls.iter().any(|call| match call {
            DatabaseCall::QueryGeometry(refnos) => refnos == expected_refnos,
            _ => false,
        })
    }
}
```

## 8. 日志和监控重构

### 8.1 日志系统标准化 🟡

#### 问题描述
当前系统使用多种日志方式，缺乏统一标准：

```rust
// 混合的日志方式
println!("gen mesh param: {}", &g.id);
dbg!(&result);
#[cfg(feature = "debug_model")]
println!("gen mesh hash: {}", id);
```

#### 重构建议
```rust
// 统一日志系统
use tracing::{info, warn, error, debug, trace};

// 结构化日志
pub struct GeometryLogger;

impl GeometryLogger {
    pub fn log_mesh_generation_start(&self, refno: RefnoEnum, mesh_type: &str) {
        info!(
            refno = %refno,
            mesh_type = mesh_type,
            "Starting mesh generation"
        );
    }

    pub fn log_mesh_generation_complete(&self, refno: RefnoEnum, duration: Duration, vertex_count: usize) {
        info!(
            refno = %refno,
            duration_ms = duration.as_millis(),
            vertex_count = vertex_count,
            "Mesh generation completed"
        );
    }

    pub fn log_mesh_generation_error(&self, refno: RefnoEnum, error: &anyhow::Error) {
        error!(
            refno = %refno,
            error = %error,
            "Mesh generation failed"
        );
    }
}

// 性能监控日志
pub struct PerformanceLogger;

impl PerformanceLogger {
    pub fn log_database_query(&self, operation: &str, duration: Duration, record_count: usize) {
        debug!(
            operation = operation,
            duration_ms = duration.as_millis(),
            record_count = record_count,
            "Database query completed"
        );
    }

    pub fn log_batch_processing(&self, batch_size: usize, total_time: Duration, items_per_second: f64) {
        info!(
            batch_size = batch_size,
            total_time_ms = total_time.as_millis(),
            items_per_second = items_per_second,
            "Batch processing completed"
        );
    }
}
```

### 8.2 指标收集系统 🟡

#### 问题描述
当前性能统计代码分散，缺乏系统性的指标收集：

#### 重构建议
```rust
// 指标收集系统
use prometheus::{Counter, Histogram, Gauge, Registry};

pub struct MetricsCollector {
    geometry_generation_counter: Counter,
    geometry_generation_duration: Histogram,
    active_connections: Gauge,
    database_query_duration: Histogram,
}

impl MetricsCollector {
    pub fn new(registry: &Registry) -> Result<Self> {
        let geometry_generation_counter = Counter::new(
            "geometry_generation_total",
            "Total number of geometry generations"
        )?;

        let geometry_generation_duration = Histogram::new(
            "geometry_generation_duration_seconds",
            "Time spent generating geometry"
        )?;

        let active_connections = Gauge::new(
            "active_database_connections",
            "Number of active database connections"
        )?;

        let database_query_duration = Histogram::new(
            "database_query_duration_seconds",
            "Time spent on database queries"
        )?;

        registry.register(Box::new(geometry_generation_counter.clone()))?;
        registry.register(Box::new(geometry_generation_duration.clone()))?;
        registry.register(Box::new(active_connections.clone()))?;
        registry.register(Box::new(database_query_duration.clone()))?;

        Ok(Self {
            geometry_generation_counter,
            geometry_generation_duration,
            active_connections,
            database_query_duration,
        })
    }

    pub fn record_geometry_generation(&self, duration: Duration) {
        self.geometry_generation_counter.inc();
        self.geometry_generation_duration.observe(duration.as_secs_f64());
    }

    pub fn record_database_query(&self, duration: Duration) {
        self.database_query_duration.observe(duration.as_secs_f64());
    }
}
```

## 9. 重构实施计划

### 阶段1：基础重构 (2-3周)
1. **函数拆分** - 将大型函数拆分为小函数
2. **常量整理** - 创建分类的常量模块
3. **错误处理统一** - 实现统一错误处理机制
4. **日志标准化** - 替换println!和dbg!为结构化日志

### 阶段2：架构重构 (3-4周)
1. **依赖注入** - 实现服务容器和依赖注入
2. **策略模式** - 重构模型类型处理逻辑
3. **配置管理** - 实现分层配置系统
4. **查询构建器** - 替换复杂SQL字符串拼接

### 阶段3：高级重构 (4-6周)
1. **测试架构** - 建立完整的测试体系
2. **监控系统** - 实现指标收集和性能监控
3. **数据访问层** - 统一数据访问接口
4. **缓存优化** - 实现灵活的缓存策略

### 成功指标
- **代码复杂度降低** - 平均函数长度 < 50行
- **测试覆盖率提升** - 核心模块覆盖率 > 80%
- **性能提升** - 模型生成速度提升 20-30%
- **维护性改善** - 新功能开发时间减少 40%

这个全面的重构计划将使E3D系统更加模块化、可测试和可维护，为未来的功能扩展奠定坚实基础。
