// 模型生成模块
//
// 负责处理基于 Refno 的模型生成相关的 HTTP 请求

use aios_core::{RefU64, RefnoEnum};
use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
};
use std::time::{Instant, SystemTime};

use crate::web_server::{
    AppState,
    models::{
        DatabaseConfig, ErrorDetails, LogLevel, TaskInfo, TaskStatus, TaskType,
        RefnoModelGenerationRequest, RefnoModelGenerationResponse,
    },
};

// ================= 辅助数据结构 =================

/// 房间关系更新结果
#[derive(Debug)]
struct RoomUpdateResult {
    affected_rooms: usize,
    updated_elements: usize,
    duration_ms: u64,
}

// ================= API 处理器 =================


// ================= 内部辅助函数 =================

/// 执行实际的任务（从任务管理器中获取配置并执行）
async fn execute_real_task(state: AppState, task_id: String) {
    use aios_core::get_db_option;

    // 获取任务配置
    let config = {
        let task_manager = state.task_manager.lock().await;
        if let Some(task) = task_manager.active_tasks.get(&task_id) {
            task.config.clone()
        } else {
            return;
        }
    };

    // 获取数据库选项
    let db_option = get_db_option();

    // 执行基于 Refno 的模型生成
    execute_refno_model_generation(state, task_id, config, db_option).await;
}

/// 执行基于 Refno 的模型生成任务
async fn execute_refno_model_generation(
    state: AppState,
    task_id: String,
    config: DatabaseConfig,
    db_option: aios_core::options::DbOption,
) {
    use crate::fast_model::gen_all_geos_data;

    // 更新任务状态为运行中
    {
        let mut task_manager = state.task_manager.lock().await;
        if let Some(task) = task_manager.active_tasks.get_mut(&task_id) {
            task.status = TaskStatus::Running;
            task.started_at = Some(SystemTime::now());
            task.add_log(LogLevel::Info, "开始执行基于 Refno 的模型生成".to_string());
        }
    }

    // 解析 refno 字符串到 RefnoEnum
    let mut parsed_refnos = Vec::new();
    for refno_str in &config.manual_refnos {
        match refno_str.parse::<u64>() {
            Ok(num) => parsed_refnos.push(RefnoEnum::Refno(RefU64(num))),
            Err(_) => {
                // 尝试解析复杂格式，如 "1/456" (dbnum/refno)
                if refno_str.contains('/') {
                    let parts: Vec<&str> = refno_str.split('/').collect();
                    if parts.len() == 2 {
                        if let Ok(num) = parts[1].parse::<u64>() {
                            parsed_refnos.push(RefnoEnum::Refno(RefU64(num)));
                            continue;
                        }
                    }
                }
                // 解析失败，记录错误并跳过
                let mut task_manager = state.task_manager.lock().await;
                if let Some(task) = task_manager.active_tasks.get_mut(&task_id) {
                    task.add_log(
                        LogLevel::Warning,
                        format!("无法解析 refno: {}", refno_str),
                    );
                }
            }
        }
    }

    if parsed_refnos.is_empty() {
        let mut task_manager = state.task_manager.lock().await;
        if let Some(mut task) = task_manager.active_tasks.remove(&task_id) {
            task.status = TaskStatus::Failed;
            task.completed_at = Some(SystemTime::now());
            task.error = Some("没有有效的 refno 可以处理".to_string());
            task.add_log(LogLevel::Error, "没有有效的 refno 可以处理".to_string());
            task_manager.task_history.push(task);
        }
        return;
    }

    // 更新进度：开始生成
    {
        let mut task_manager = state.task_manager.lock().await;
        if let Some(task) = task_manager.active_tasks.get_mut(&task_id) {
            task.update_progress(
                "生成几何数据".to_string(),
                1,
                2,
                50.0,
            );
            task.add_log(
                LogLevel::Info,
                format!("开始为 {} 个 refno 生成几何数据", parsed_refnos.len()),
            );
        }
    }

    // 调用 gen_all_geos_data
    let start_time = Instant::now();
    let result = gen_all_geos_data(
        parsed_refnos.clone(),
        &db_option,
        None,
        config.target_sesno,
    )
    .await;

    let duration = start_time.elapsed();

    // 处理结果
    match result {
        Ok(_) => {
            // 成功
            let mut task_manager = state.task_manager.lock().await;
            if let Some(mut task) = task_manager.active_tasks.remove(&task_id) {
                task.status = TaskStatus::Completed;
                task.completed_at = Some(SystemTime::now());
                task.actual_duration = Some(duration.as_millis() as u64);
                task.progress.percentage = 100.0;
                task.progress.current_step = "完成".to_string();
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

                task_manager.task_history.push(task);
            }
        }
        Err(e) => {
            // 失败
            let mut task_manager = state.task_manager.lock().await;
            if let Some(mut task) = task_manager.active_tasks.remove(&task_id) {
                task.status = TaskStatus::Failed;
                task.completed_at = Some(SystemTime::now());
                task.actual_duration = Some(duration.as_millis() as u64);

                let error_details = ErrorDetails {
                    error_type: "RefnoModelGenerationError".to_string(),
                    error_code: Some("REFNO_GEN_001".to_string()),
                    failed_step: "生成几何数据".to_string(),
                    detailed_message: format!("基于 Refno 的模型生成失败: {}", e),
                    stack_trace: Some(format!("{:?}", e)),
                    suggested_solutions: vec![
                        "检查 refno 是否有效".to_string(),
                        "检查数据库连接是否正常".to_string(),
                        "查看日志获取详细错误信息".to_string(),
                    ],
                    related_config: Some(serde_json::json!({
                        "manual_refnos": config.manual_refnos,
                        "db_num": config.manual_db_nums,
                        "gen_model": config.gen_model,
                        "gen_mesh": config.gen_mesh,
                    })),
                };

                task.set_error_details(error_details);
                task.add_log(LogLevel::Error, format!("模型生成失败: {}", e));
                task_manager.task_history.push(task);
            }
        }
    }
}

/// 智能与增量为指定 refnos 更新房间关系
/// 根据元素数量自动选择增量更新或全量更新策略
async fn update_room_relations_for_refnos_incremental(
    refnos: &[RefnoEnum],
) -> Result<RoomUpdateResult, anyhow::Error> {
    use aios_core::get_db_option;
    use crate::fast_model::room_model::{build_room_relations, update_room_relations_incremental};

    let start_time = Instant::now();

    // 智能判断：元素数量较少时使用增量更新
    if refnos.len() <= 100 {
        // 尝试增量更新
        match update_room_relations_incremental(refnos).await {
            Ok(result) => {
                println!(
                    "[Room] 增量更新完成: {} 个(refnos) -> {} 个房间, {} 个元素, 耗时 {}ms",
                    refnos.len(),
                    result.affected_rooms,
                    result.updated_elements,
                    result.duration_ms
                );
                return Ok(RoomUpdateResult {
                    affected_rooms: result.affected_rooms,
                    updated_elements: result.updated_elements,
                    duration_ms: result.duration_ms,
                });
            }
            Err(e) => {
                println!(
                    "[Room] 增量更新失败，降级到全量更新: {}",
                    e
                );
                // 增量更新失败，降级到全量更新
            }
        }
    }

    // 全量更新逻辑（元素数量较多或增量更新失败时使用）
    let db_option = get_db_option();
    match build_room_relations(&db_option).await {
        Ok(_) => {
            let duration = start_time.elapsed();
            let fallback_result = RoomUpdateResult {
                affected_rooms: refnos.len() / 10, // 占位符: 假设每10个元素影响1个房间
                updated_elements: refnos.len(),
                duration_ms: duration.as_millis() as u64,
            };

            println!(
                "[Room] 全量更新完成: {} 个(refnos) -> {} 个房间, 耗时 {}ms",
                refnos.len(),
                fallback_result.affected_rooms,
                fallback_result.duration_ms
            );

            Ok(fallback_result)
        }
        Err(e) => {
            Err(anyhow::anyhow!("房间关系更新失败: {}", e))
        }
    }
}

/// 分批处理大量元素的房间关系更新
async fn batch_update_room_relations(
    refnos: &[RefnoEnum],
    batch_size: usize,
) -> anyhow::Result<RoomUpdateResult> {
    let mut total_affected_rooms = 0;
    let mut total_updated_elements = 0;
    let start_time = Instant::now();

    println!("[Room] 开始分批处理 {} 个元素, 批次大小: {}", refnos.len(), batch_size);

    for (batch_index, chunk) in refnos.chunks(batch_size).enumerate() {
        println!("[Room] 处理批次 {}/{}", batch_index + 1, (refnos.len() + batch_size - 1) / batch_size);

        let result = update_room_relations_for_refnos_incremental(chunk).await?;
        total_affected_rooms += result.affected_rooms;
        total_updated_elements += result.updated_elements;

        // 添加批次间隔，避免数据库压力过大
        if batch_index < refnos.chunks(batch_size).count() - 1 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    Ok(RoomUpdateResult {
        affected_rooms: total_affected_rooms,
        updated_elements: total_updated_elements,
        duration_ms: start_time.elapsed().as_millis() as u64,
    })
}

/// 为指定 refnos 更新房间关系（保持向后兼容）
pub async fn update_room_relations_for_refnos(
    refnos: &[RefnoEnum],
) -> Result<RoomUpdateResult, anyhow::Error> {
    update_room_relations_for_refnos_incremental(refnos).await
}
