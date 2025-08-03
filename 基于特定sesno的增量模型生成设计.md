# 基于特定 sesno 的增量模型生成设计

## 一、正确理解 target_sesno 的含义

当 `gen_all_geos_data` 指定了 `target_sesno: Some(sesno)` 时，其真实含义是：
- **生成该 sesno 对应的增删改元件的模型**
- **不是生成该 sesno 时间点的完整模型快照**
- **只处理在该 sesno 发生变化的元素**

## 二、基于 element_changes 表的实现

### 2.1 查询特定 sesno 的变更

```rust
/// 获取特定 sesno 的所有变更
pub async fn get_changes_at_sesno(
    db: &Surreal<Client>,
    sesno: u32,
) -> anyhow::Result<IncrGeoUpdateLog> {
    // 查询该 sesno 的所有变更记录
    let sql = format!(
        "SELECT * FROM element_changes WHERE sesno = {} ORDER BY timestamp",
        sesno
    );
    
    let mut response = db.query(sql).await?;
    let changes: Vec<ElementChange> = response.take(0)?;
    
    // 转换为 IncrGeoUpdateLog
    let mut update_log = IncrGeoUpdateLog::default();
    
    for change in changes {
        let refno = RefnoEnum::from_str(&change.refno)?;
        
        match change.operation {
            ChangeOperation::Delete => {
                update_log.delete_refnos.insert(refno);
            }
            _ => {
                // 根据元素类型分类
                match change.element_type.as_str() {
                    "PRIM" => update_log.prim_refnos.insert(refno),
                    "LOOP" => update_log.loop_owner_refnos.insert(refno),
                    "BRAN" | "HANGER" => update_log.bran_hanger_refnos.insert(refno),
                    "CATA" => update_log.basic_cata_refnos.insert(refno),
                    _ => false,
                };
            }
        }
    }
    
    Ok(update_log)
}
```

### 2.2 优化的 gen_all_geos_data 实现

```rust
pub async fn gen_all_geos_data(
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOption,
    incr_updates: Option<IncrGeoUpdateLog>,
    target_sesno: Option<u32>,
) -> anyhow::Result<bool> {
    let mut final_incr_updates = incr_updates;
    
    // 如果指定了 target_sesno，获取该 sesno 的增量数据
    if let Some(sesno) = target_sesno {
        if final_incr_updates.is_none() {
            // 从 element_changes 表获取该 sesno 的变更
            let sesno_changes = get_changes_at_sesno(&SUL_DB, sesno).await?;
            
            // 如果该 sesno 有变更，使用这些变更作为增量更新
            if sesno_changes.count() > 0 {
                println!("发现 sesno {} 的变更: {} 个元素", sesno, sesno_changes.count());
                final_incr_updates = Some(sesno_changes);
            }
        }
    }
    
    // 处理增量更新
    if let Some(updates) = &final_incr_updates {
        println!("处理增量更新，受影响的元素数量: {}", updates.count());
        
        // 获取所有受影响的 refnos（包括子元素）
        let affected_refnos = updates.get_all_geom_refnos_deep().await;
        
        // 只生成受影响的模型
        let (sender, receiver) = flume::unbounded();
        let insert_task = tokio::spawn(async move {
            while let Ok(shape_insts) = receiver.recv_async().await {
                save_instance_data(&shape_insts, false).await.unwrap();
                println!("保存增量模型实例: {}", shape_insts.inst_cnt());
            }
        });
        
        // 生成增量模型
        generate_incremental_models_internal(
            affected_refnos,
            db_option,
            sender,
        ).await?;
        
        insert_task.await?;
        
        return Ok(true);
    }
    
    // 其他情况的处理...
    // ...原有逻辑
}
```

### 2.3 内部增量模型生成函数

```rust
async fn generate_incremental_models_internal(
    affected_refnos: HashSet<RefnoEnum>,
    db_option: &DbOption,
    sender: flume::Sender<ShapeInstancesData>,
) -> anyhow::Result<()> {
    // 按类型分组
    let mut type_groups: HashMap<String, Vec<RefnoEnum>> = HashMap::new();
    
    for refno in affected_refnos {
        let type_name = get_type_name(refno).await?;
        type_groups.entry(type_name).or_default().push(refno);
    }
    
    // 并行处理不同类型的模型
    let mut handles = FuturesUnordered::new();
    
    for (type_name, refnos) in type_groups {
        let sender_clone = sender.clone();
        let db_option_clone = db_option.clone();
        
        handles.push(tokio::spawn(async move {
            match type_name.as_str() {
                "PRIM" => generate_prim_models(&refnos, &db_option_clone, sender_clone).await,
                "LOOP" => generate_loop_models(&refnos, &db_option_clone, sender_clone).await,
                "BRAN" | "HANGER" => generate_bran_models(&refnos, &db_option_clone, sender_clone).await,
                _ => Ok(()),
            }
        }));
    }
    
    // 等待所有任务完成
    while let Some(result) = handles.next().await {
        result??;
    }
    
    Ok(())
}
```

## 三、使用场景示例

### 3.1 处理特定 sesno 的增量

```rust
// 场景：用户只想生成 sesno 150 的变更模型
gen_all_geos_data(
    vec![],
    &db_option,
    None,  // 不提供预计算的增量
    Some(150),  // 指定 sesno
).await?;

// 内部流程：
// 1. 查询 element_changes WHERE sesno = 150
// 2. 获取所有在 sesno 150 发生变更的元素
// 3. 只生成这些变更元素的模型
```

### 3.2 处理 sesno 范围的增量

```rust
// 场景：生成 sesno 100-150 之间的所有变更
let increments = get_increments_between_sesnos(100, 150).await?;
gen_all_geos_data(
    vec![],
    &db_option,
    Some(increments),  // 提供预计算的增量
    Some(150),  // 目标 sesno（可选）
).await?;
```

### 3.3 实时增量处理

```rust
// 场景：监听到新的 sesno 变更，立即生成对应模型
async fn handle_new_sesno_change(sesno: u32) {
    // 直接生成该 sesno 的增量模型
    gen_all_geos_data(
        vec![],
        &db_option,
        None,
        Some(sesno),
    ).await?;
}
```

## 四、性能优化策略

### 4.1 缓存机制

```rust
// 缓存最近的 sesno 变更查询结果
lazy_static! {
    static ref SESNO_CHANGES_CACHE: DashMap<u32, IncrGeoUpdateLog> = DashMap::new();
}

pub async fn get_changes_at_sesno_cached(
    db: &Surreal<Client>,
    sesno: u32,
) -> anyhow::Result<IncrGeoUpdateLog> {
    // 先检查缓存
    if let Some(cached) = SESNO_CHANGES_CACHE.get(&sesno) {
        return Ok(cached.clone());
    }
    
    // 查询数据库
    let changes = get_changes_at_sesno(db, sesno).await?;
    
    // 缓存结果
    SESNO_CHANGES_CACHE.insert(sesno, changes.clone());
    
    Ok(changes)
}
```

### 4.2 批量处理

```rust
/// 批量处理多个 sesno 的增量
pub async fn batch_generate_sesno_models(
    sesnos: Vec<u32>,
    db_option: &DbOption,
) -> anyhow::Result<()> {
    // 批量查询所有 sesno 的变更
    let sql = format!(
        "SELECT * FROM element_changes WHERE sesno IN [{}] ORDER BY sesno, timestamp",
        sesnos.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(",")
    );
    
    let mut response = SUL_DB.query(sql).await?;
    let all_changes: Vec<ElementChange> = response.take(0)?;
    
    // 按 sesno 分组
    let mut sesno_groups: HashMap<u32, Vec<ElementChange>> = HashMap::new();
    for change in all_changes {
        sesno_groups.entry(change.sesno).or_default().push(change);
    }
    
    // 并行处理每个 sesno
    let mut handles = FuturesUnordered::new();
    for (sesno, changes) in sesno_groups {
        let db_option_clone = db_option.clone();
        handles.push(tokio::spawn(async move {
            let update_log = convert_changes_to_update_log(changes);
            gen_all_geos_data(vec![], &db_option_clone, Some(update_log), Some(sesno)).await
        }));
    }
    
    // 等待所有任务完成
    while let Some(result) = handles.next().await {
        result??;
    }
    
    Ok(())
}
```

## 五、与 Raphtory 的集成

### 5.1 在 Raphtory 中记录 sesno 增量

```rust
/// 在 Raphtory 图中记录特定 sesno 的变更
pub fn add_sesno_changes_to_graph(
    graph: &mut Graph,
    sesno: u32,
    changes: Vec<ElementChange>,
) -> anyhow::Result<()> {
    for change in changes {
        // 在图中标记该 sesno 的变更
        graph.add_node(
            change.refno.clone(),
            sesno as i64,
            vec![
                ("sesno", &sesno.to_string()),
                ("operation", &format!("{:?}", change.operation)),
                ("change_type", &format!("{:?}", change.change_type)),
            ],
        )?;
        
        // 如果是层级变化，更新边
        if matches!(change.change_type, ChangeType::Hierarchy) {
            // 处理父子关系变化
            // ...
        }
    }
    
    Ok(())
}
```

## 六、总结

正确理解 `target_sesno` 的含义后，我们的实现策略是：

1. **精确查询**：只查询特定 sesno 的变更，而不是时间范围
2. **增量生成**：只生成变更的元素，而不是完整快照
3. **高效处理**：利用 element_changes 表的索引快速定位变更
4. **灵活扩展**：支持单个 sesno、sesno 范围、批量处理等多种场景

这种设计大大提升了增量模型生成的效率和精确性。