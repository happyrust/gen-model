// 兼容层：保留旧版 gen_model.rs 中的公共 API
//
// 这个模块提供与旧代码的兼容性，逐步迁移到新的优化版本

use std::sync::Arc;
use std::time::Instant;
use anyhow::Result;

use aios_core::RefnoEnum;

use crate::data_interface::increment_record::IncrGeoUpdateLog;
use crate::data_interface::sesno_increment::get_changes_at_sesno;
use crate::options::DbOptionExt;

use super::models::DbModelInstRefnos;
use super::config::FullNounConfig;
use super::full_noun_mode::gen_full_noun_geos_optimized;

/// 主入口函数：生成所有几何体数据
///
/// 这是兼容旧版 API 的函数，内部会根据配置选择使用优化版本
///
/// # Arguments
/// * `manual_refnos` - 手动指定的 refno 列表
/// * `db_option` - 数据库配置
/// * `incr_updates` - 增量更新日志
/// * `target_sesno` - 目标 sesno
pub async fn gen_all_geos_data(
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOptionExt,
    incr_updates: Option<IncrGeoUpdateLog>,
    target_sesno: Option<u32>,
) -> Result<bool> {
    let time = Instant::now();
    let mut final_incr_updates = incr_updates;

    // 如果指定了 target_sesno，获取该 sesno 的增量数据
    if let Some(sesno) = target_sesno {
        if final_incr_updates.is_none() {
            match get_changes_at_sesno(sesno).await {
                Ok(sesno_changes) => {
                    if sesno_changes.count() > 0 {
                        final_incr_updates = Some(sesno_changes);
                    } else {
                        println!("[gen_model] sesno {} 没有发现变更，跳过增量生成", sesno);
                        return Ok(false);
                    }
                }
                Err(e) => {
                    eprintln!("获取 sesno {} 的变更失败: {}", sesno, e);
                    return Err(e);
                }
            }
        }
    }

    let incr_count = final_incr_updates
        .as_ref()
        .map(|log| log.count())
        .unwrap_or(0);

    println!(
        "[gen_model] 启动 gen_all_geos_data: manual_refnos={}, incr_updates={}, target_sesno={:?}",
        manual_refnos.len(),
        incr_count,
        target_sesno,
    );

    // 调试：打印 Full Noun 模式配置
    println!("[gen_model] Full Noun 模式配置: full_noun_mode={}, concurrency={}, batch_size={}",
        db_option.full_noun_mode,
        db_option.get_full_noun_concurrency(),
        db_option.get_full_noun_batch_size()
    );

    // 检查是否启用 Full Noun 模式
    if db_option.full_noun_mode {
        println!("[gen_model] 进入 Full Noun 模式（优化版本）");

        if db_option.manual_db_nums.is_some() || db_option.exclude_db_nums.is_some() {
            println!(
                "[gen_model] 警告: Full Noun 模式下 manual_db_nums 和 exclude_db_nums 配置将被忽略"
            );
        }

        if final_incr_updates.is_some() {
            println!(
                "[gen_model] 警告: Full Noun 模式下增量更新将被忽略，将执行全库重建"
            );
        }

        // 使用优化版本的 Full Noun 生成
        let full_start = Instant::now();
        let config = FullNounConfig::from_db_option_ext(db_option)
            .map_err(|e| anyhow::anyhow!("配置错误: {}", e))?;

        let (sender, receiver) = flume::unbounded();

        // TODO: 实际的几何体接收逻辑需要从旧代码迁移
        let handle = tokio::spawn(async move {
            while let Ok(_data) = receiver.recv_async().await {
                // 处理几何体数据
            }
        });

        let categorized = gen_full_noun_geos_optimized(
            Arc::new(db_option.inner.clone()),
            &config,
            sender,
        )
        .await
        .map_err(|e| anyhow::anyhow!("Full Noun 生成失败: {}", e))?;

        drop(handle);

        println!(
            "[gen_model] ✅ Full Noun 模式完成，处理 {} 个 refno，用时 {} ms",
            categorized.total_count(),
            full_start.elapsed().as_millis()
        );

        // TODO: 可选执行 mesh 和布尔运算
        // 这部分需要从旧代码迁移

        println!(
            "[gen_model] gen_all_geos_data 总耗时: {} ms",
            time.elapsed().as_millis()
        );

        return Ok(true);
    }

    // 非 Full Noun 模式：暂时调用旧实现
    println!("[gen_model] ⚠️ 使用旧版实现（待迁移）");

    // TODO: 将旧版的非 Full Noun 逻辑迁移到这里
    // 目前返回 false 表示未实现
    Ok(false)
}

/// 兼容函数：旧版的 gen_full_noun_geos
///
/// 为了保持向后兼容，保留这个函数签名
#[deprecated(note = "请使用 gen_full_noun_geos_optimized 替代")]
pub async fn gen_full_noun_geos(
    db_option: &DbOptionExt,
    _extra_nouns: Option<Vec<&'static str>>,
) -> Result<DbModelInstRefnos> {
    println!("⚠️ 警告：使用已弃用的 gen_full_noun_geos，建议迁移到优化版本");

    // TODO: 实现兼容层或直接调用优化版本
    Ok(DbModelInstRefnos::default())
}

/// 兼容函数：旧版的 gen_geos_data
///
/// 这个函数在优化版本中暂未实现，需要从 gen_model_old.rs 迁移
#[deprecated(note = "此函数未在优化版本中实现，需要迁移")]
pub async fn gen_geos_data(
    _dbno: Option<u32>,
    _manual_refnos: Vec<RefnoEnum>,
    _db_option: &DbOptionExt,
    _incr_updates: Option<IncrGeoUpdateLog>,
    _sender: flume::Sender<aios_core::geometry::ShapeInstancesData>,
    _target_sesno: Option<u32>,
) -> Result<Vec<RefnoEnum>> {
    eprintln!("⚠️ 错误：gen_geos_data 未在优化版本中实现");
    eprintln!("  提示：此功能需要从 gen_model_old.rs 迁移");
    Err(anyhow::anyhow!("gen_geos_data 未实现"))
}
