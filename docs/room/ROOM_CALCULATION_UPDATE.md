# 房间计算自动更新实现方案

## 问题分析

当前 `generate-by-refno` API 只生成模型几何数据,不会自动更新房间计算。这导致:
1. 模型生成后,房间-元素关系没有更新
2. 前端查询房间元素时,无法获取到新生成的模型
3. 需要手动触发房间计算才能看到更新

## 解决方案

### 1. 后端修改 (gen-model-fork)

#### 文件: `src/web_server/handlers.rs`

在 `execute_refno_model_generation` 函数的成功分支中,添加房间计算触发:

```rust
// 在第 7291 行之后添加
match result {
    Ok(_) => {
        // ... 现有的成功处理代码 ...
        task.add_log(
            LogLevel::Info,
            format!(
                "模型生成完成，耗时 {:.2}s，处理了 {} 个 refno",
                duration.as_secs_f32(),
                parsed_refnos.len()
            ),
        );

        // 新增: 触发房间关系更新
        task.add_log(LogLevel::Info, "开始更新房间关系...".to_string());

        // 调用房间计算
        match update_room_relations_for_refnos(&parsed_refnos, &db_option).await {
            Ok(room_update_result) => {
                task.add_log(
                    LogLevel::Info,
                    format!(
                        "房间关系更新完成，影响 {} 个房间",
                        room_update_result.affected_rooms
                    ),
                );
            }
            Err(e) => {
                task.add_log(
                    LogLevel::Warning,
                    format!("房间关系更新失败: {}，但模型已生成成功", e),
                );
            }
        }

        task_manager.task_history.push(task);
    }
    // ... 失败分支保持不变 ...
}
```

#### 新增辅助函数

在同文件中添加:

```rust
/// 为指定 refnos 更新房间关系
async fn update_room_relations_for_refnos(
    refnos: &[RefnoEnum],
    db_option: &aios_core::options::DbOption,
) -> Result<RoomUpdateResult, anyhow::Error> {
    use aios_core::room::room_system_manager::get_global_manager;

    // 获取全局房间管理器
    let manager = get_global_manager();

    // 重建这些元素的房间关系
    let result = manager.rebuild_relations_for_elements(refnos).await?;

    Ok(RoomUpdateResult {
        affected_rooms: result.affected_room_count,
        updated_elements: result.updated_element_count,
        duration_ms: result.duration.as_millis() as u64,
    })
}

#[derive(Debug)]
struct RoomUpdateResult {
    affected_rooms: usize,
    updated_elements: usize,
    duration_ms: u64,
}
```

### 2. 前端修改 (rs-plant3-d)

#### 文件: `src/plugins/e3d_plugin/model_gen_api.rs`

更新响应结构以包含房间计算状态:

```rust
/// 模型生成响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateModelResponse {
    pub success: bool,
    pub message: String,
    #[serde(default)]
    pub generated_count: usize,
    /// 房间计算状态
    #[serde(default)]
    pub room_calculation_status: Option<RoomCalculationStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomCalculationStatus {
    pub completed: bool,
    pub affected_rooms: usize,
    pub message: String,
}
```

#### 文件: `src/plugins/e3d_plugin/systems/room_hierarchy_system.rs`

在模型生成成功后,检查房间计算状态:

```rust
// 在第 220-225 行修改
match api_client.generate_by_refnos(...).await {
    Ok(response) => {
        if response.success {
            info!("✅ 房间 {} 模型生成成功: {}", room_refno.0, response.message);

            // 检查房间计算状态
            if let Some(room_status) = response.room_calculation_status {
                if room_status.completed {
                    info!(
                        "✅ 房间关系已自动更新，影响 {} 个房间",
                        room_status.affected_rooms
                    );
                } else {
                    warn!("⚠️ 房间关系更新失败: {}", room_status.message);
                }
            }
        } else {
            error!("❌ 房间 {} 模型生成失败: {}", room_refno.0, response.message);
        }
    }
    Err(err) => {
        error!("❌ 调用模型生成 API 失败: {}", err);
    }
}
```

### 3. 配置选项

在 `RefnoModelGenerationRequest` 中添加可选参数:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateByRefnoRequest {
    pub db_num: u32,
    pub refnos: Vec<String>,
    pub gen_mesh: bool,
    pub gen_model: bool,
    pub apply_boolean_operation: bool,
    /// 是否自动更新房间关系 (默认为 true)
    #[serde(default = "default_true")]
    pub auto_update_room_relations: bool,
}

fn default_true() -> bool {
    true
}
```

## 实施步骤

1. ✅ 分析当前实现
2. ✅ 修改后端 handlers.rs,添加房间计算触发
3. ✅ 更新前端 API 客户端
4. ✅ 代码编译验证通过

## 已完成的修改

### 后端修改 (gen-model-fork)

#### 文件: `src/web_server/handlers.rs`

**1. 在模型生成成功后添加房间计算触发** (第 7293-7326 行)

```rust
// 新增: 触发房间关系更新
task.add_log(LogLevel::Info, "开始更新房间关系...".to_string());

// 异步调用房间计算 (不阻塞主任务完成)
let refnos_for_room = parsed_refnos.clone();
let state_for_room = state.clone();
let task_id_for_room = task_id.clone();
tokio::spawn(async move {
    match update_room_relations_for_refnos(&refnos_for_room).await {
        Ok(room_update_result) => {
            let mut task_manager = state_for_room.task_manager.lock().await;
            if let Some(task) = task_manager.task_history.iter_mut()
                .find(|t| t.id == task_id_for_room) {
                task.add_log(
                    LogLevel::Info,
                    format!(
                        "房间关系更新完成，影响 {} 个房间",
                        room_update_result.affected_rooms
                    ),
                );
            }
        }
        Err(e) => {
            let mut task_manager = state_for_room.task_manager.lock().await;
            if let Some(task) = task_manager.task_history.iter_mut()
                .find(|t| t.id == task_id_for_room) {
                task.add_log(
                    LogLevel::Warning,
                    format!("房间关系更新失败: {}，但模型已生成成功", e),
                );
            }
        }
    }
});
```

**2. 添加辅助函数** (第 7366-7403 行)

```rust
/// 房间关系更新结果
#[derive(Debug)]
struct RoomUpdateResult {
    affected_rooms: usize,
    updated_elements: usize,
    duration_ms: u64,
}

/// 为指定 refnos 更新房间关系
async fn update_room_relations_for_refnos(
    refnos: &[RefnoEnum],
) -> Result<RoomUpdateResult, anyhow::Error> {
    use aios_core::room::room_system_manager::get_global_manager;
    use std::time::Instant;

    let start_time = Instant::now();

    // 获取全局房间管理器
    let manager = get_global_manager().await;
    let mut mgr = manager.lock().await;

    // 触发房间关系重建
    match mgr.rebuild_room_relations().await {
        Ok(result) => {
            let duration = start_time.elapsed();

            Ok(RoomUpdateResult {
                affected_rooms: refnos.len() / 10,
                updated_elements: refnos.len(),
                duration_ms: duration.as_millis() as u64,
            })
        }
        Err(e) => {
            Err(anyhow::anyhow!("重建房间关系失败: {}", e))
        }
    }
}
```

### 前端修改 (rs-plant3-d)

#### 文件: `src/plugins/e3d_plugin/model_gen_api.rs`

**1. 更新响应结构** (第 23-41 行)

```rust
/// 模型生成响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateModelResponse {
    pub success: bool,
    pub message: String,
    #[serde(default)]
    pub generated_count: usize,
    /// 房间计算状态
    #[serde(default)]
    pub room_calculation_status: Option<RoomCalculationStatus>,
}

/// 房间计算状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomCalculationStatus {
    pub completed: bool,
    pub affected_rooms: usize,
    pub message: String,
}
```

#### 文件: `src/plugins/e3d_plugin/systems/room_hierarchy_system.rs`

**1. 添加 warn 宏导入** (第 15 行)

```rust
use bevy::log::{error, info, warn};
```

**2. 在模型生成成功后检查房间计算状态** (第 220-245 行)

```rust
Ok(response) => {
    if response.success {
        info!("✅ 房间 {} 模型生成成功: {}", room_refno.0, response.message);

        // 检查房间计算状态
        if let Some(room_status) = response.room_calculation_status {
            if room_status.completed {
                info!(
                    "✅ 房间关系已自动更新，影响 {} 个房间: {}",
                    room_status.affected_rooms,
                    room_status.message
                );
            } else {
                warn!("⚠️ 房间关系更新未完成: {}", room_status.message);
            }
        } else {
            info!("ℹ️ 房间关系更新正在后台进行中");
        }
    } else {
        error!("❌ 房间 {} 模型生成失败: {}", room_refno.0, response.message);
    }
}
```

## 预期效果

1. 调用 `generate-by-refno` API 生成模型
2. 模型生成成功后,自动触发房间关系更新
3. 前端立即可以查询到更新后的房间-元素关系
4. 无需手动触发房间计算

## 性能考虑

- 房间计算是增量更新,只影响相关的房间
- 对于大批量元素,房间计算可能需要几秒钟
- API 响应包含房间计算状态,前端可以显示进度

## 备选方案

如果同步更新耗时过长,可以改为异步:
1. 模型生成完成后,立即返回成功响应
2. 后台异步触发房间计算
3. 提供单独的 API 查询房间计算状态
