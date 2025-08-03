# 基于 pdms-io 现有 Raphtory 集成的增量更新方案

## 一、现有实现分析

### 1.1 pdms-io 中的 Raphtory 集成
pdms-io 项目已经实现了完整的 Raphtory 集成，包括：

1. **核心功能**
   - 时间戳转换：sesno ↔ timestamp
   - 批量数据存储到 Raphtory 图
   - 历史状态查询
   - 时间范围变更查询

2. **关键接口**
   ```rust
   // 查询指定时间点的元素状态
   query_historical_state(refno: RefU64, timestamp: i64) -> Result<Option<HistoricalElement>>
   
   // 获取元素的完整时间线
   get_element_timeline(refno: RefU64) -> Result<ElementTimeline>
   
   // 查询时间范围内的所有变更
   query_changes_in_range(start_timestamp: i64, end_timestamp: i64) -> Result<HashMap<RefU64, ElementTimeline>>
   ```

### 1.2 与 gen-model 的结合点
gen-model 项目的 `gen_all_geos_data` 函数已经支持：
- `target_sesno` 参数用于历史数据生成
- `IncrGeoUpdateLog` 用于增量更新记录
- 基于 sesno 的历史查询（通过 aios-core）

## 二、集成方案设计

### 2.1 架构设计

```
gen-model (模型生成)
    ↓
aios-core (核心接口层)
    ↓
pdms-io/raphtory_integration (Raphtory 时序图存储)
    ↓
Raphtory Graph Database
```

### 2.2 在 aios-core 中添加适配层

```rust
// 在 aios-core 中添加 Raphtory 查询接口
use pdms_io::raphtory_integration::{RaphtoryIntegration, TimeUtils};

/// Raphtory 增量查询适配器
pub struct RaphtoryIncrementAdapter {
    integration: RaphtoryIntegration,
}

impl RaphtoryIncrementAdapter {
    /// 计算两个 sesno 之间的增量变化
    pub async fn calculate_increments_between_sesnos(
        &self,
        from_sesno: u32,
        to_sesno: u32,
    ) -> anyhow::Result<IncrGeoUpdateLog> {
        let start_timestamp = TimeUtils::session_to_timestamp(from_sesno as i32);
        let end_timestamp = TimeUtils::session_to_timestamp(to_sesno as i32);
        
        // 查询时间范围内的变更
        let changes = self.integration.query_changes_in_range(start_timestamp, end_timestamp)?;
        
        // 转换为 IncrGeoUpdateLog
        let mut result = IncrGeoUpdateLog::default();
        
        for (refno, timeline) in changes {
            // 分析变更类型并分类
            for historical_element in timeline {
                match &historical_element.data.detail {
                    EleOperationDetail::Deleted => {
                        result.delete_refnos.insert(refno);
                    }
                    _ => {
                        // 根据元素类型分类
                        let element_type = self.get_element_type_at_sesno(refno, to_sesno).await?;
                        match element_type.as_str() {
                            "PRIM" => result.prim_refnos.insert(refno),
                            "LOOP" => result.loop_owner_refnos.insert(refno),
                            "BRAN" | "HANGER" => result.bran_hanger_refnos.insert(refno),
                            "CATA" => result.basic_cata_refnos.insert(refno),
                            _ => false,
                        };
                    }
                }
            }
        }
        
        Ok(result)
    }
    
    /// 查询指定 sesno 的元素类型
    async fn get_element_type_at_sesno(
        &self,
        refno: RefnoEnum,
        sesno: u32,
    ) -> anyhow::Result<String> {
        let timestamp = TimeUtils::session_to_timestamp(sesno as i32);
        
        if let Some(historical) = self.integration.query_historical_state(refno, timestamp)? {
            // 从 Raphtory 图中获取元素类型
            if let Some(node) = self.integration.graph().node(&refno.to_string()) {
                if let Some(prop) = node.properties().get("element_type") {
                    return Ok(prop.to_string());
                }
            }
        }
        
        Ok("UNKNOWN".to_string())
    }
}
```

### 2.3 修改 gen_all_geos_data 使用 Raphtory

```rust
pub async fn gen_all_geos_data_with_raphtory(
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOption,
    incr_updates: Option<IncrGeoUpdateLog>,
    target_sesno: Option<u32>,
) -> anyhow::Result<bool> {
    // 如果没有提供增量更新，但指定了目标 sesno，自动计算增量
    let incr_updates = if incr_updates.is_none() && target_sesno.is_some() {
        let current_sesno = target_sesno.unwrap();
        let previous_sesno = get_previous_sesno(current_sesno).await?;
        
        // 使用 Raphtory 计算增量
        let adapter = RaphtoryIncrementAdapter::new(db_option.db_num)?;
        Some(adapter.calculate_increments_between_sesnos(previous_sesno, current_sesno).await?)
    } else {
        incr_updates
    };
    
    // 调用原有的 gen_all_geos_data
    gen_all_geos_data(manual_refnos, db_option, incr_updates, target_sesno).await
}
```

## 三、实现步骤

### 3.1 第一阶段：集成准备
1. 在 aios-core 中添加对 pdms-io 的依赖
2. 创建 Raphtory 查询适配器接口
3. 实现 sesno 到 DateTime 的转换接口（通过 SurrealDB）

### 3.2 第二阶段：增量计算实现
1. 实现 `calculate_increments_between_sesnos` 方法
2. 集成到 gen_all_geos_data 流程中
3. 添加缓存机制优化性能

### 3.3 第三阶段：完整集成
1. 支持批量 sesno 查询
2. 实现层级结构的时序查询
3. 添加性能监控和优化

## 四、使用示例

### 4.1 基本增量更新
```rust
// 生成 sesno 100 到 150 之间的增量模型
let db_option = DbOption {
    db_num: 7999,
    ..Default::default()
};

gen_all_geos_data_with_raphtory(
    vec![],
    &db_option,
    None, // 自动计算增量
    Some(150), // 目标 sesno
).await?;
```

### 4.2 手动指定增量
```rust
// 使用 Raphtory 查询增量
let adapter = RaphtoryIncrementAdapter::new(7999)?;
let increments = adapter.calculate_increments_between_sesnos(100, 150).await?;

// 只生成受影响的模型
gen_all_geos_data(
    vec![],
    &db_option,
    Some(increments),
    Some(150),
).await?;
```

### 4.3 查询历史状态
```rust
// 查询特定 refno 在 sesno 100 时的状态
let adapter = RaphtoryIncrementAdapter::new(7999)?;
let state = adapter.query_model_state_at_sesno(refno, 100).await?;
```

## 五、性能优化建议

### 5.1 缓存策略
- 缓存常用 sesno 范围的增量计算结果
- 使用 Redis 缓存热点查询
- 实现增量计算的并行处理

### 5.2 批量处理
- 批量查询多个 refno 的状态
- 使用 Raphtory 的并行查询能力
- 优化大批量数据的处理流程

### 5.3 索引优化
- 在 Raphtory 中为常用查询创建索引
- 优化时间窗口查询性能
- 使用合适的图分区策略

## 六、总结

利用 pdms-io 中现有的 Raphtory 集成，可以快速实现 gen-model 项目中基于 sesno 的增量更新功能。主要优势：

1. **无需重复开发** - 直接使用已有的 Raphtory 集成
2. **功能完善** - 支持时序查询、增量计算等核心功能
3. **性能优秀** - Raphtory 的时间窗口查询经过优化
4. **易于扩展** - 可以逐步添加更多时序分析功能

通过在 aios-core 中添加适配层，可以将 pdms-io 的 Raphtory 功能无缝集成到 gen-model 的模型生成流程中，实现高效的增量更新。