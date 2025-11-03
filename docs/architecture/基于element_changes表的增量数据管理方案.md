# 基于 element_changes 表的增量数据管理方案

## 一、背景和目标

参考 pdms-io 的 `update_elements_to_database` 实现，我们希望在创建 Raphtory 图的过程中，同步将增量数据保存到 SurrealDB 的 `element_changes` 表中。这样可以：

1. 直接通过 SQL 查询获取两个 sesno 之间的增量数据
2. 避免重复计算增量
3. 支持高效的增量模型生成

## 二、element_changes 表结构设计

### 2.1 表结构

```sql
-- SurrealDB 中的 element_changes 表定义
DEFINE TABLE element_changes SCHEMAFULL;

-- 字段定义
DEFINE FIELD refno ON element_changes TYPE string;
DEFINE FIELD sesno ON element_changes TYPE int;
DEFINE FIELD dbnum ON element_changes TYPE int;
DEFINE FIELD operation ON element_changes TYPE string
    ASSERT $value IN ['CREATE', 'UPDATE', 'DELETE'];
DEFINE FIELD change_type ON element_changes TYPE string
    ASSERT $value IN ['ATTRIBUTE', 'GEOMETRY', 'HIERARCHY', 'REFERENCE'];
DEFINE FIELD old_data ON element_changes TYPE object;
DEFINE FIELD new_data ON element_changes TYPE object;
DEFINE FIELD timestamp ON element_changes TYPE datetime;
DEFINE FIELD owner ON element_changes TYPE string;
DEFINE FIELD element_type ON element_changes TYPE string;
DEFINE FIELD affected_attrs ON element_changes TYPE array;
DEFINE FIELD children ON element_changes TYPE array;

-- 索引定义
DEFINE INDEX idx_sesno ON element_changes COLUMNS sesno;
DEFINE INDEX idx_refno ON element_changes COLUMNS refno;
DEFINE INDEX idx_dbnum ON element_changes COLUMNS dbnum;
DEFINE INDEX idx_timestamp ON element_changes COLUMNS timestamp;
DEFINE INDEX idx_compound ON element_changes COLUMNS refno, sesno;
```

### 2.2 数据结构映射

```rust
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};
use surrealdb::sql::RecordId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementChange {
    pub id: Option<RecordId>,
    pub refno: String,
    pub sesno: u32,
    pub dbnum: i32,
    pub operation: ChangeOperation,
    pub change_type: ChangeType,
    pub old_data: Option<serde_json::Value>,
    pub new_data: Option<serde_json::Value>,
    pub timestamp: DateTime<Utc>,
    pub owner: Option<String>,
    pub element_type: String,
    pub affected_attrs: Vec<String>,
    pub children: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChangeOperation {
    #[serde(rename = "CREATE")]
    Create,
    #[serde(rename = "UPDATE")]
    Update,
    #[serde(rename = "DELETE")]
    Delete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChangeType {
    #[serde(rename = "ATTRIBUTE")]
    Attribute,
    #[serde(rename = "GEOMETRY")]
    Geometry,
    #[serde(rename = "HIERARCHY")]
    Hierarchy,
    #[serde(rename = "REFERENCE")]
    Reference,
}
```

## 三、在创建 Raphtory 图时保存增量数据

### 3.1 增强的 Raphtory 图创建函数

```rust
use raphtory::prelude::*;
use surrealdb::Surreal;
use surrealdb::engine::remote::ws::Client;

/// 创建 Raphtory 图并同时保存增量数据到 SurrealDB
pub async fn create_raphtory_graph_with_changes(
    db: &Surreal<Client>,
    elements: Vec<EleOperationData>,
    sesno: u32,
) -> anyhow::Result<Graph> {
    let graph = Graph::new();
    let mut element_changes = Vec::new();

    for element in elements {
        let refno = element.refno.to_string();

        // 1. 添加到 Raphtory 图
        match &element.detail {
            EleOperationDetail::Add(attr_map) => {
                // 添加节点到图
                graph.add_node(
                    refno.clone(),
                    sesno as i64,
                    vec![
                        ("operation", "CREATE"),
                        ("type", &element.element_type),
                    ],
                )?;

                // 添加属性
                for (key, value) in attr_map.iter() {
                    graph.add_node_property(
                        &refno,
                        key,
                        value.to_string(),
                        sesno as i64,
                    )?;
                }

                // 创建增量记录
                element_changes.push(ElementChange {
                    id: None,
                    refno: refno.clone(),
                    sesno,
                    dbnum: element.dbnum,
                    operation: ChangeOperation::Create,
                    change_type: ChangeType::Attribute,
                    old_data: None,
                    new_data: Some(serde_json::to_value(attr_map)?),
                    timestamp: Utc::now(),
                    owner: attr_map.get_owner().map(|o| o.to_string()),
                    element_type: element.element_type.clone(),
                    affected_attrs: attr_map.keys().cloned().collect(),
                    children: vec![],
                });
            }

            EleOperationDetail::Modify { old, new } => {
                // 更新节点
                graph.add_node(
                    refno.clone(),
                    sesno as i64,
                    vec![
                        ("operation", "UPDATE"),
                        ("type", &element.element_type),
                    ],
                )?;

                // 计算变更的属性
                let mut affected_attrs = Vec::new();
                for (key, new_value) in new.iter() {
                    let old_value = old.get(key);
                    if old_value != Some(new_value) {
                        affected_attrs.push(key.clone());
                        graph.add_node_property(
                            &refno,
                            key,
                            new_value.to_string(),
                            sesno as i64,
                        )?;
                    }
                }

                // 创建增量记录
                element_changes.push(ElementChange {
                    id: None,
                    refno: refno.clone(),
                    sesno,
                    dbnum: element.dbnum,
                    operation: ChangeOperation::Update,
                    change_type: determine_change_type(&affected_attrs),
                    old_data: Some(serde_json::to_value(old)?),
                    new_data: Some(serde_json::to_value(new)?),
                    timestamp: Utc::now(),
                    owner: new.get_owner().map(|o| o.to_string()),
                    element_type: element.element_type.clone(),
                    affected_attrs,
                    children: vec![],
                });
            }

            EleOperationDetail::Delete(attr_map) => {
                // 标记节点为删除
                graph.add_node(
                    refno.clone(),
                    sesno as i64,
                    vec![
                        ("operation", "DELETE"),
                        ("deleted", "true"),
                    ],
                )?;

                // 创建增量记录
                element_changes.push(ElementChange {
                    id: None,
                    refno: refno.clone(),
                    sesno,
                    dbnum: element.dbnum,
                    operation: ChangeOperation::Delete,
                    change_type: ChangeType::Attribute,
                    old_data: Some(serde_json::to_value(attr_map)?),
                    new_data: None,
                    timestamp: Utc::now(),
                    owner: attr_map.get_owner().map(|o| o.to_string()),
                    element_type: element.element_type.clone(),
                    affected_attrs: vec![],
                    children: vec![],
                });
            }
        }
    }

    // 2. 批量保存到 SurrealDB
    save_element_changes_batch(db, element_changes).await?;

    Ok(graph)
}

/// 批量保存增量数据到 SurrealDB
async fn save_element_changes_batch(
    db: &Surreal<Client>,
    changes: Vec<ElementChange>,
) -> anyhow::Result<()> {
    const BATCH_SIZE: usize = 1000;

    for chunk in changes.chunks(BATCH_SIZE) {
        let sql = generate_batch_insert_sql(chunk);
        db.query(sql).await?;
    }

    Ok(())
}

/// 生成批量插入 SQL
fn generate_batch_insert_sql(changes: &[ElementChange]) -> String {
    let values: Vec<String> = changes.iter().map(|change| {
        format!(
            r#"{{
                refno: "{}",
                sesno: {},
                dbnum: {},
                operation: "{}",
                change_type: "{}",
                old_data: {},
                new_data: {},
                timestamp: "{}",
                owner: {},
                element_type: "{}",
                affected_attrs: {:?},
                children: {:?}
            }}"#,
            change.refno,
            change.sesno,
            change.dbnum,
            serde_json::to_string(&change.operation).unwrap(),
            serde_json::to_string(&change.change_type).unwrap(),
            change.old_data.as_ref().map(|d| d.to_string()).unwrap_or("null".to_string()),
            change.new_data.as_ref().map(|d| d.to_string()).unwrap_or("null".to_string()),
            change.timestamp.to_rfc3339(),
            change.owner.as_ref().map(|o| format!(r#""{}""#, o)).unwrap_or("null".to_string()),
            change.element_type,
            change.affected_attrs,
            change.children,
        )
    }).collect();

    format!("INSERT INTO element_changes [{}]", values.join(", "))
}
```

## 四、基于 element_changes 表的增量查询接口

### 4.1 查询接口实现

```rust
/// 基于 element_changes 表的增量查询接口
pub struct ElementChangesQueryInterface {
    db: Surreal<Client>,
}

impl ElementChangesQueryInterface {
    /// 查询两个 sesno 之间的所有变更
    pub async fn get_changes_between_sesnos(
        &self,
        from_sesno: u32,
        to_sesno: u32,
        dbnum: Option<i32>,
    ) -> anyhow::Result<Vec<ElementChange>> {
        let mut sql = format!(
            "SELECT * FROM element_changes WHERE sesno > {} AND sesno <= {}",
            from_sesno, to_sesno
        );

        if let Some(db) = dbnum {
            sql.push_str(&format!(" AND dbnum = {}", db));
        }

        sql.push_str(" ORDER BY sesno, timestamp");

        let mut response = self.db.query(sql).await?;
        let changes: Vec<ElementChange> = response.take(0)?;

        Ok(changes)
    }

    /// 获取特定 refno 的变更历史
    pub async fn get_element_history(
        &self,
        refno: &str,
        from_sesno: Option<u32>,
        to_sesno: Option<u32>,
    ) -> anyhow::Result<Vec<ElementChange>> {
        let mut sql = format!(
            "SELECT * FROM element_changes WHERE refno = '{}'",
            refno
        );

        if let Some(from) = from_sesno {
            sql.push_str(&format!(" AND sesno >= {}", from));
        }

        if let Some(to) = to_sesno {
            sql.push_str(&format!(" AND sesno <= {}", to));
        }

        sql.push_str(" ORDER BY sesno");

        let mut response = self.db.query(sql).await?;
        let changes: Vec<ElementChange> = response.take(0)?;

        Ok(changes)
    }

    /// 转换为 IncrGeoUpdateLog
    pub async fn get_incremental_update_log(
        &self,
        from_sesno: u32,
        to_sesno: u32,
    ) -> anyhow::Result<IncrGeoUpdateLog> {
        let changes = self.get_changes_between_sesnos(from_sesno, to_sesno, None).await?;

        let mut update_log = IncrGeoUpdateLog::default();

        for change in changes {
            let refno = RefnoEnum::from_str(&change.refno)?;

            match change.operation {
                ChangeOperation::Delete => {
                    update_log.delete_refnos.insert(refno);
                }
                _ => {
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

    /// 获取受影响的模型统计
    pub async fn get_change_statistics(
        &self,
        from_sesno: u32,
        to_sesno: u32,
    ) -> anyhow::Result<ChangeStatistics> {
        let sql = format!(
            r#"
            SELECT
                operation,
                element_type,
                count() as count
            FROM element_changes
            WHERE sesno > {} AND sesno <= {}
            GROUP BY operation, element_type
            "#,
            from_sesno, to_sesno
        );

        let mut response = self.db.query(sql).await?;
        let stats: Vec<(String, String, i64)> = response.take(0)?;

        let mut statistics = ChangeStatistics::default();
        for (operation, element_type, count) in stats {
            statistics.add(operation, element_type, count);
        }

        Ok(statistics)
    }
}
```

### 4.2 优化的增量模型生成

```rust
/// 基于 element_changes 表的增量模型生成
pub async fn generate_incremental_models(
    from_sesno: u32,
    to_sesno: u32,
    db_option: &DbOption,
) -> anyhow::Result<bool> {
    // 1. 从 element_changes 表获取增量数据
    let query_interface = ElementChangesQueryInterface::new(SUL_DB.clone());
    let update_log = query_interface
        .get_incremental_update_log(from_sesno, to_sesno)
        .await?;

    // 2. 只生成受影响的模型
    gen_all_geos_data(
        vec![],
        db_option,
        Some(update_log),
        Some(to_sesno),
    ).await
}
```

## 五、优势和性能优化

### 5.1 主要优势

1. **避免重复计算**：增量数据在写入时就已保存，查询时直接读取
2. **高效查询**：利用 SurrealDB 的索引，快速获取特定 sesno 范围的变更
3. **灵活过滤**：支持按 dbnum、element_type、operation 等维度过滤
4. **历史追溯**：完整保存每个元素的变更历史

### 5.2 性能优化策略

1. **批量写入**：使用批量 INSERT 减少数据库交互
2. **索引优化**：在常用查询字段上建立索引
3. **分区策略**：按 sesno 范围分区，提高查询效率
4. **缓存机制**：缓存热点 sesno 范围的查询结果

### 5.3 数据清理策略

```rust
/// 清理过期的增量数据
pub async fn cleanup_old_changes(
    db: &Surreal<Client>,
    retention_days: i64,
) -> anyhow::Result<()> {
    let cutoff_date = Utc::now() - chrono::Duration::days(retention_days);

    let sql = format!(
        "DELETE element_changes WHERE timestamp < '{}'",
        cutoff_date.to_rfc3339()
    );

    db.query(sql).await?;
    Ok(())
}
```

## 六、使用示例

```rust
// 1. 在处理增量更新时自动保存
let graph = create_raphtory_graph_with_changes(&db, elements, sesno).await?;

// 2. 查询增量数据
let query = ElementChangesQueryInterface::new(db);
let changes = query.get_changes_between_sesnos(100, 150, Some(7997)).await?;

// 3. 生成增量模型
generate_incremental_models(100, 150, &db_option).await?;

// 4. 获取变更统计
let stats = query.get_change_statistics(100, 150).await?;
println!("Created: {}, Updated: {}, Deleted: {}",
    stats.created, stats.updated, stats.deleted);
```

## 七、总结

通过在创建 Raphtory 图时同步保存增量数据到 SurrealDB 的 `element_changes` 表，我们实现了：

1. **实时增量记录**：在数据变更时立即记录
2. **高效增量查询**：直接从表中查询，无需重新计算
3. **灵活的数据分析**：支持多维度的变更统计和分析
4. **优化的模型生成**：只处理真正变化的部分

这种方案既保持了 Raphtory 的时序图优势，又充分利用了 SurrealDB 的查询能力，为增量模型生成提供了高效的数据基础。
