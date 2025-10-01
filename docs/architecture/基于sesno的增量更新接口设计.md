# 基于 sesno 的增量更新和时间窗口控制接口设计

## 一、现状分析

### 1.1 当前实现
从代码分析可以看到，系统已经有了基础的 sesno 支持：

1. **gen_all_geos_data** 函数已经支持 `target_sesno` 参数
2. **gen_geos_data** 中已经实现了基于 sesno 的历史查询：
   ```rust
   if let Some(sesno) = target_sesno {
       if !session_exists(sesno).await? {
           return Err(anyhow::anyhow!("会话号 {} 不存在", sesno));
       }
       target_root_refnos = query_type_refnos_by_dbnum_at_sesno(
           &["SITE"],
           dbno.unwrap(),
           sesno
       ).await?;
   }
   ```

### 1.2 增量更新机制
- 使用 `IncrGeoUpdateLog` 记录模型变化
- 支持不同类型的模型更新（prim、loop、bran_hanger、basic_cata）
- 通过 `IncrEleUpdateLog` 记录详细的元素变更信息

## 二、需要在 aios-core 中实现的接口

### 2.1 sesno 和 DateTime 转换接口

```rust
/// sesno 与时间转换接口
#[async_trait]
pub trait SesnoTimeInterface: Send + Sync {
    /// 根据 sesno 查询对应的时间戳
    async fn get_datetime_by_sesno(
        &self,
        sesno: u32,
    ) -> anyhow::Result<DateTime<Utc>>;

    /// 根据时间戳查询最近的 sesno
    async fn get_sesno_by_datetime(
        &self,
        datetime: DateTime<Utc>,
    ) -> anyhow::Result<u32>;

    /// 查询 sesno 范围
    async fn get_sesno_range(
        &self,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    ) -> anyhow::Result<(u32, u32)>;

    /// 获取当前最新的 sesno
    async fn get_latest_sesno(&self) -> anyhow::Result<u32>;
}
```

### 2.2 基于 sesno 的数据查询接口

```rust
/// 基于 sesno 的数据查询接口
#[async_trait]
pub trait SesnoDataQueryInterface: Send + Sync {
    /// 查询指定 sesno 的类型参考号
    async fn query_type_refnos_at_sesno(
        &self,
        types: &[&str],
        dbno: u32,
        sesno: u32,
    ) -> anyhow::Result<Vec<RefnoEnum>>;

    /// 查询指定 sesno 的元素属性
    async fn get_attr_at_sesno(
        &self,
        refno: RefnoEnum,
        sesno: u32,
    ) -> anyhow::Result<Option<AttrMap>>;

    /// 批量查询指定 sesno 的元素属性
    async fn batch_get_attrs_at_sesno(
        &self,
        refnos: &[RefnoEnum],
        sesno: u32,
    ) -> anyhow::Result<HashMap<RefnoEnum, AttrMap>>;

    /// 查询 sesno 范围内的变更
    async fn query_changes_between_sesnos(
        &self,
        start_sesno: u32,
        end_sesno: u32,
        filter: Option<ChangeFilter>,
    ) -> anyhow::Result<Vec<IncrEleUpdateLog>>;
}
```

### 2.3 基于 sesno 的层级查询接口

```rust
/// 基于 sesno 的层级结构查询接口
#[async_trait]
pub trait SesnoHierarchyInterface: Send + Sync {
    /// 查询指定 sesno 的子节点
    async fn get_children_at_sesno(
        &self,
        parent_refno: RefnoEnum,
        sesno: u32,
    ) -> anyhow::Result<Vec<RefnoEnum>>;

    /// 查询指定 sesno 的完整层级结构
    async fn get_hierarchy_at_sesno(
        &self,
        root_refno: RefnoEnum,
        sesno: u32,
        max_depth: Option<u32>,
    ) -> anyhow::Result<HierarchyTree>;

    /// 查询指定 sesno 的祖先路径
    async fn get_ancestor_path_at_sesno(
        &self,
        refno: RefnoEnum,
        sesno: u32,
    ) -> anyhow::Result<Vec<RefnoEnum>>;

    /// 批量查询多个节点的子节点
    async fn batch_get_children_at_sesno(
        &self,
        parent_refnos: &[RefnoEnum],
        sesno: u32,
    ) -> anyhow::Result<HashMap<RefnoEnum, Vec<RefnoEnum>>>;
}
```

### 2.4 增量更新接口

```rust
/// 增量更新处理接口
#[async_trait]
pub trait IncrementalUpdateInterface: Send + Sync {
    /// 获取两个 sesno 之间的增量更新
    async fn get_incremental_updates(
        &self,
        from_sesno: u32,
        to_sesno: u32,
    ) -> anyhow::Result<IncrGeoUpdateLog>;

    /// 应用增量更新到指定 sesno
    async fn apply_incremental_updates(
        &self,
        base_sesno: u32,
        updates: IncrGeoUpdateLog,
    ) -> anyhow::Result<u32>; // 返回新的 sesno

    /// 获取影响特定模型的所有增量
    async fn get_model_increments(
        &self,
        refno: RefnoEnum,
        start_sesno: u32,
        end_sesno: u32,
    ) -> anyhow::Result<Vec<IncrEleUpdateLog>>;

    /// 计算增量更新的影响范围
    async fn calculate_update_impact(
        &self,
        updates: &IncrGeoUpdateLog,
    ) -> anyhow::Result<UpdateImpact>;
}
```

### 2.5 时间窗口控制接口

```rust
/// 时间窗口控制接口
#[async_trait]
pub trait TimeWindowControlInterface: Send + Sync {
    /// 创建基于 sesno 的时间窗口
    async fn create_time_window(
        &self,
        start_sesno: u32,
        end_sesno: u32,
    ) -> anyhow::Result<TimeWindow>;

    /// 在时间窗口内查询数据
    async fn query_in_window<T>(
        &self,
        window: &TimeWindow,
        query: WindowQuery,
    ) -> anyhow::Result<T>
    where
        T: DeserializeOwned + Send;

    /// 生成时间窗口内的模型快照
    async fn generate_window_snapshot(
        &self,
        window: &TimeWindow,
        options: SnapshotOptions,
    ) -> anyhow::Result<ModelSnapshot>;

    /// 比较两个时间窗口的差异
    async fn compare_windows(
        &self,
        window1: &TimeWindow,
        window2: &TimeWindow,
    ) -> anyhow::Result<WindowDiff>;
}
```

## 三、增量模型生成优化

### 3.1 优化后的 gen_all_geos_data 流程

```rust
pub async fn gen_all_geos_data_with_incremental(
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOption,
    incr_updates: Option<IncrGeoUpdateLog>,
    target_sesno: Option<u32>,
) -> anyhow::Result<bool> {
    // 1. 如果指定了 target_sesno，先验证会话存在性
    if let Some(sesno) = target_sesno {
        if !session_exists(sesno).await? {
            return Err(anyhow::anyhow!("会话号 {} 不存在", sesno));
        }
    }

    // 2. 处理增量更新
    if let Some(updates) = &incr_updates {
        // 获取增量更新影响的所有 refnos
        let affected_refnos = updates.get_all_geom_refnos_deep().await;
        
        // 只生成受影响的模型
        return generate_affected_models(
            affected_refnos,
            db_option,
            target_sesno,
        ).await;
    }

    // 3. 处理手动指定的 refnos
    if !manual_refnos.is_empty() {
        return generate_specific_models(
            manual_refnos,
            db_option,
            target_sesno,
        ).await;
    }

    // 4. 全量生成（使用 sesno 控制时间窗口）
    generate_all_models_at_sesno(db_option, target_sesno).await
}
```

### 3.2 增量部分的单独模型生成

```rust
/// 生成受影响的模型
async fn generate_affected_models(
    affected_refnos: HashSet<RefnoEnum>,
    db_option: &DbOption,
    target_sesno: Option<u32>,
) -> anyhow::Result<bool> {
    // 按类型分组
    let mut prim_refnos = Vec::new();
    let mut loop_refnos = Vec::new();
    let mut bran_refnos = Vec::new();
    
    for refno in affected_refnos {
        // 元素类型不会变化，直接查询当前类型
        let type_name = get_type_name(refno).await?;
        match type_name.as_str() {
            "PRIM" => prim_refnos.push(refno),
            "LOOP" => loop_refnos.push(refno),
            "BRAN" | "HANGER" => bran_refnos.push(refno),
            _ => {}
        }
    }

    // 并行生成不同类型的模型
    let handles = vec![
        tokio::spawn(generate_prim_models(prim_refnos, db_option, target_sesno)),
        tokio::spawn(generate_loop_models(loop_refnos, db_option, target_sesno)),
        tokio::spawn(generate_bran_models(bran_refnos, db_option, target_sesno)),
    ];

    // 等待所有生成任务完成
    for handle in handles {
        handle.await??;
    }

    Ok(true)
}
```

## 四、数据结构定义

### 4.1 时间窗口相关

```rust
/// 时间窗口
#[derive(Debug, Clone)]
pub struct TimeWindow {
    pub start_sesno: u32,
    pub end_sesno: u32,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
}

/// 窗口查询
#[derive(Debug, Clone)]
pub struct WindowQuery {
    pub query_type: QueryType,
    pub filters: Vec<QueryFilter>,
    pub include_deleted: bool,
}

/// 更新影响分析
#[derive(Debug, Clone)]
pub struct UpdateImpact {
    pub directly_affected: Vec<RefnoEnum>,
    pub indirectly_affected: Vec<RefnoEnum>,
    pub deleted_models: Vec<RefnoEnum>,
    pub new_models: Vec<RefnoEnum>,
    pub estimated_generation_time: Duration,
}
```

### 4.2 变更过滤器

```rust
/// 变更过滤器
#[derive(Debug, Clone)]
pub struct ChangeFilter {
    pub dbnos: Option<Vec<u32>>,
    pub types: Option<Vec<String>>,
    pub operations: Option<Vec<EleOperation>>,
    pub refnos: Option<Vec<RefnoEnum>>,
}
```

## 五、使用示例

### 5.1 基于 sesno 的增量更新

```rust
// 获取两个会话之间的增量
let updates = incremental_interface
    .get_incremental_updates(100, 150)
    .await?;

// 生成增量部分的模型
gen_all_geos_data(
    vec![],
    &db_option,
    Some(updates),
    Some(150), // 目标 sesno
).await?;
```

### 5.2 时间窗口查询

```rust
// 创建时间窗口
let window = time_window_interface
    .create_time_window(100, 200)
    .await?;

// 在窗口内查询
let models = time_window_interface
    .query_in_window::<Vec<ModelData>>(
        &window,
        WindowQuery {
            query_type: QueryType::GeometryModels,
            filters: vec![
                QueryFilter::Type("EQUI".to_string()),
                QueryFilter::Dbno(7997),
            ],
            include_deleted: false,
        }
    ).await?;
```

### 5.3 层级查询

```rust
// 查询特定 sesno 的层级结构
let hierarchy = hierarchy_interface
    .get_hierarchy_at_sesno(root_refno, 150, Some(5))
    .await?;

// 批量查询子节点
let children_map = hierarchy_interface
    .batch_get_children_at_sesno(&parent_refnos, 150)
    .await?;
```

## 六、性能优化建议

1. **缓存策略**
   - 缓存常用 sesno 的查询结果
   - 实现 sesno 到时间的映射缓存
   - 缓存层级结构快照

2. **批量处理**
   - 批量查询接口减少数据库访问
   - 并行处理不同类型的模型生成

3. **增量计算**
   - 只处理变更的部分
   - 重用未变更的模型数据

4. **索引优化**
   - 在 Raphtory 中建立基于 sesno 的时间索引
   - 优化频繁查询的路径

通过这些接口的实现，可以高效地支持基于 sesno 的增量更新和时间窗口控制，大幅提升模型生成的性能。