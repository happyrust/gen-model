// 兼容层：保留旧版 gen_model.rs 中的公共 API
//
// 这个模块提供与旧代码的兼容性，逐步迁移到新的优化版本

use anyhow::Result;
use std::sync::Arc;
use std::time::Instant;

use aios_core::RefnoEnum;

use crate::data_interface::increment_record::IncrGeoUpdateLog;
use crate::data_interface::sesno_increment::get_changes_at_sesno;
use crate::fast_model::capture::capture_refnos_if_enabled;
use crate::fast_model::mesh_generate::process_meshes_update_db_deep;
use crate::fast_model::pdms_inst::save_instance_data_optimize;
use crate::options::DbOptionExt;
use crate::{e3d_info};
#[cfg(feature = "sqlite-index")]
use crate::spatial_index::SqliteSpatialIndex;

use super::config::FullNounConfig;
use super::full_noun_mode::gen_full_noun_geos_optimized;
use super::models::{DbModelInstRefnos, NounCategory};

/// 主入口函数：生成所有几何体数据
///
/// 这是兼容旧版 API 的函数：
/// - Full Noun 模式：走新 gen_model 管线（优化版本 + 深度几何收集）
/// - 非 Full Noun 模式：仍由旧 gen_model_old 管线处理（暂未迁移）
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
    println!(
        "[gen_model] Full Noun 模式配置: full_noun_mode={}, concurrency={}, batch_size={}",
        db_option.full_noun_mode,
        db_option.get_full_noun_concurrency(),
        db_option.get_full_noun_batch_size()
    );

    // =========================
    // Full Noun 模式：新管线
    // =========================
    if db_option.full_noun_mode {
        println!("[gen_model] 进入 Full Noun 模式（新 gen_model 管线）");

        if db_option.manual_db_nums.is_some() || db_option.exclude_db_nums.is_some() {
            println!(
                "[gen_model] 警告: Full Noun 模式下 manual_db_nums 和 exclude_db_nums 配置将被忽略"
            );
        }

        if final_incr_updates.is_some() {
            println!("[gen_model] 警告: Full Noun 模式下增量更新将被忽略，将执行全库重建");
        }

        let full_start = Instant::now();

        // 1️⃣ 先用兼容函数生成/更新 inst_relate，并获取分类后的根 refno
        let db_refnos = gen_full_noun_geos(db_option, None)
            .await
            .map_err(|e| anyhow::anyhow!("Full Noun 生成失败: {}", e))?;

        println!(
            "[gen_model] Full Noun 模式 insts 入库完成，用时 {} ms",
            full_start.elapsed().as_millis()
        );

        // 2️⃣ 可选执行 mesh 生成（使用深度收集逻辑）
        if db_option.inner.gen_mesh {
            let mesh_start = Instant::now();
            println!("[gen_model] Full Noun 模式开始生成三角网格（深度收集几何节点）");
            db_refnos
                .execute_gen_inst_meshes(Some(Arc::new(db_option.inner.clone())))
                .await;
            println!(
                "[gen_model] Full Noun 模式三角网格生成完成，用时 {} ms",
                mesh_start.elapsed().as_millis()
            );

            // 3️⃣ 可选执行布尔运算（基于 inst_relate 状态的 Worker）
            if db_option.inner.apply_boolean_operation {
                let bool_start = Instant::now();
                println!("[gen_model] Full Noun 模式开始布尔运算（boolean worker）");
                if let Err(e) = crate::fast_model::mesh_generate::run_boolean_worker(
                    Arc::new(db_option.inner.clone()),
                    100,
                )
                .await
                {
                    eprintln!("[gen_model] Full Noun 布尔运算失败: {}", e);
                } else {
                    println!(
                        "[gen_model] Full Noun 模式布尔运算完成，用时 {} ms",
                        bool_start.elapsed().as_millis()
                    );
                }
            }
        }

        println!(
            "[gen_model] Full Noun 模式全部完成，总用时 {} ms",
            full_start.elapsed().as_millis()
        );
        println!(
            "[gen_model] gen_all_geos_data 总耗时: {} ms",
            time.elapsed().as_millis()
        );

        return Ok(true);
    }

    // 非 Full Noun 模式：暂时仍由旧 gen_model_old 管线负责
    let is_incr_update = final_incr_updates.is_some();
    let has_manual_refnos = !manual_refnos.is_empty();
    let has_debug = db_option.inner.debug_model_refnos.is_some();

    if is_incr_update || has_manual_refnos || has_debug {
        let mode_label = if is_incr_update {
            "增量"
        } else if has_manual_refnos {
            "手动"
        } else {
            "调试"
        };

        let target_count = if is_incr_update {
            incr_count
        } else if has_manual_refnos {
            manual_refnos.len()
        } else {
            db_option
                .inner
                .debug_model_refnos
                .as_ref()
                .map(|v| v.len())
                .unwrap_or(0)
        };

        println!(
            "[gen_model] 进入{}生成路径，目标节点数: {}",
            mode_label, target_count
        );

        let (sender, receiver) = flume::unbounded();
        let receiver: flume::Receiver<aios_core::geometry::ShapeInstancesData> = receiver.clone();

        let replace_exist = db_option.inner.is_replace_mesh();

        let insert_task = tokio::task::spawn(async move {
            while let Ok(shape_insts) = receiver.recv_async().await {
                if let Err(e) = save_instance_data_optimize(&shape_insts, replace_exist).await {
                    eprintln!("保存实例数据失败: {}", e);
                }
            }
        });

        let target_root_refnos = gen_geos_data(
            None,
            manual_refnos.clone(),
            db_option,
            final_incr_updates.clone(),
            sender.clone(),
            target_sesno,
            has_manual_refnos, // 手动模式时启用手动布尔运算
        )
        .await?;

        drop(sender);
        let _ = insert_task.await;

        println!(
            "[gen_model] {}路径模型生成完成，共 {} 个根节点",
            mode_label,
            target_root_refnos.len()
        );

        if db_option.inner.gen_mesh {
            let mesh_start = Instant::now();
            println!(
                "[gen_model] 开始更新 {} 个根节点的 mesh 数据（深度收集几何节点）",
                target_root_refnos.len()
            );

            if let Err(e) =
                process_meshes_update_db_deep(&db_option.inner, &target_root_refnos).await
            {
                eprintln!("[gen_model] 更新模型数据失败: {}", e);
            } else {
                println!(
                    "[gen_model] 完成 mesh 更新，用时 {} ms",
                    mesh_start.elapsed().as_millis()
                );
            }

            // 手动布尔运算模式：在 mesh 生成完成后执行布尔运算
            if has_manual_refnos && db_option.inner.apply_boolean_operation {
                use crate::fast_model::manifold_bool::{apply_cata_neg_boolean_manifold, apply_insts_boolean_manifold};
                use std::collections::HashSet;
                
                e3d_info!("[gen_model] 手动布尔运算模式：开始执行布尔运算");
                
                // 查询需要布尔运算的实例（基于 target_root_refnos 的子孙节点）
                let mut boolean_refnos = vec![];
                for &root_refno in &target_root_refnos {
                    // 查询深度可见实例
                    if let Ok(visible_refnos) = aios_core::query_deep_visible_inst_refnos(root_refno).await {
                        boolean_refnos.extend(visible_refnos);
                    }
                    // 查询深度负实例
                    if let Ok(neg_refnos) = aios_core::query_deep_neg_inst_refnos(root_refno).await {
                        boolean_refnos.extend(neg_refnos);
                    }
                }
                
                // 去重
                let boolean_refnos: Vec<aios_core::RefnoEnum> = boolean_refnos.into_iter().collect::<HashSet<_>>().into_iter().collect();
                
                if !boolean_refnos.is_empty() {
                    let replace_exist = db_option.inner.is_replace_mesh();
                    e3d_info!("[gen_model] 手动布尔运算模式：找到 {} 个需要布尔运算的实例", boolean_refnos.len());
                    
                    let boolean_start = Instant::now();
                    
                    // 执行元件库级布尔运算
                    if let Err(e) = apply_cata_neg_boolean_manifold(&boolean_refnos, replace_exist).await {
                        eprintln!("[gen_model] 手动布尔运算模式：元件库级布尔运算失败: {}", e);
                    }
                    
                    // 执行实例级布尔运算
                    if let Err(e) = apply_insts_boolean_manifold(&boolean_refnos, replace_exist).await {
                        eprintln!("[gen_model] 手动布尔运算模式：实例级布尔运算失败: {}", e);
                    } else {
                        e3d_info!(
                            "[gen_model] 手动布尔运算模式：布尔运算完成，用时 {} ms",
                            boolean_start.elapsed().as_millis()
                        );
                    }
                } else {
                    e3d_info!("[gen_model] 手动布尔运算模式：没有需要布尔运算的实例");
                }
            }
        }

        if let Err(err) = capture_refnos_if_enabled(&target_root_refnos, &db_option.inner).await {
            eprintln!("[capture] 捕获截图失败: {}", err);
        }

        #[cfg(feature = "sqlite-index")]
        {
            if SqliteSpatialIndex::is_enabled() {
                match SqliteSpatialIndex::with_default_path() {
                    Ok(_index) => println!("SQLite spatial index initialized"),
                    Err(e) => eprintln!("Failed to initialize SQLite spatial index: {}", e),
                }
            }
        }

        println!(
            "[gen_model] gen_all_geos_data 完成，总耗时 {} ms",
            time.elapsed().as_millis()
        );

        Ok(true)
    } else {
        // 原有的按 dbno 循环生成路径
        let dbnos = if db_option.inner.manual_db_nums.is_some() {
            db_option.inner.manual_db_nums.clone().unwrap()
        } else {
            aios_core::query_mdb_db_nums(None, aios_core::DBType::DESI).await?
        };

        // 过滤掉 exclude_db_nums 中的数据库编号
        let dbnos = if let Some(exclude_nums) = &db_option.inner.exclude_db_nums {
            dbnos
                .into_iter()
                .filter(|dbno| !exclude_nums.contains(dbno))
                .collect::<Vec<_>>()
        } else {
            dbnos
        };

        println!(
            "[gen_model] 进入全量生成路径，共 {} 个数据库待处理",
            dbnos.len()
        );

        let db_option_arc = Arc::new(db_option.inner.clone());
        if dbnos.is_empty() {
            println!("[gen_model] 未找到需要生成的数据库，直接结束");
        }

        for dbno in dbnos.clone() {
            println!("[gen_model] -> 开始处理数据库 {}", dbno);
            let db_start = Instant::now();

            let (sender, receiver) = flume::unbounded();
            let receiver: flume::Receiver<aios_core::geometry::ShapeInstancesData> =
                receiver.clone();

            let insert_task = tokio::task::spawn(async move {
                while let Ok(shape_insts) = receiver.recv_async().await {
                    if let Err(e) = save_instance_data_optimize(&shape_insts, false).await {
                        eprintln!("保存实例数据失败: {}", e);
                    }
                }
            });

            let db_refnos = crate::fast_model::gen_model_old::gen_geos_data_by_dbnum(
                dbno,
                db_option_arc.clone(),
                sender.clone(),
                target_sesno,
            )
            .await?;

            drop(sender);
            let _ = insert_task.await;

            println!(
                "[gen_model] -> 数据库 {} insts 入库完成，用时 {} ms",
                dbno,
                db_start.elapsed().as_millis()
            );

            if db_option_arc.gen_mesh {
                let mesh_start = Instant::now();
                println!("[gen_model] -> 数据库 {} 开始生成三角网格", dbno);

                db_refnos
                    .execute_gen_inst_meshes(Some(db_option_arc.clone()))
                    .await;

                println!(
                    "[gen_model] -> 数据库 {} 三角网格生成完成，用时 {} ms",
                    dbno,
                    mesh_start.elapsed().as_millis()
                );

                let boolean_start = Instant::now();
                println!("[gen_model] -> 数据库 {} 开始布尔运算", dbno);
                db_refnos
                    .execute_boolean_meshes(Some(db_option_arc.clone()))
                    .await;
                println!(
                    "[gen_model] -> 数据库 {} 布尔运算完成，用时 {} ms",
                    dbno,
                    boolean_start.elapsed().as_millis()
                );
            }

            println!(
                "[gen_model] -> 数据库 {} 处理完成，总耗时 {} ms",
                dbno,
                db_start.elapsed().as_millis()
            );
        }

        #[cfg(feature = "sqlite-index")]
        {
            if SqliteSpatialIndex::is_enabled() {
                match SqliteSpatialIndex::with_default_path() {
                    Ok(_index) => println!("SQLite spatial index initialized"),
                    Err(e) => eprintln!("Failed to initialize SQLite spatial index: {}", e),
                }
            }
        }

        println!(
            "[gen_model] gen_all_geos_data 完成，总耗时 {} ms",
            time.elapsed().as_millis()
        );

        Ok(true)
    }
}

/// 兼容函数：旧版的 gen_full_noun_geos
///
/// 为了保持向后兼容，保留这个函数签名
#[deprecated(note = "请使用 gen_full_noun_geos_optimized 替代")]
pub async fn gen_full_noun_geos(
    db_option: &DbOptionExt,
    _extra_nouns: Option<Vec<&'static str>>,
) -> Result<DbModelInstRefnos> {
    println!("⚠️ 警告：使用已弃用的 gen_full_noun_geos，内部已转发到优化版本");

    let config = FullNounConfig::from_db_option_ext(db_option)
        .map_err(|e| anyhow::anyhow!("配置错误: {}", e))?;

    let (sender, receiver) = flume::unbounded();
    let replace_exist = db_option.inner.is_replace_mesh();

    let insert_handle = tokio::spawn(async move {
        while let Ok(shape_insts) = receiver.recv_async().await {
            if let Err(e) = save_instance_data_optimize(&shape_insts, replace_exist).await {
                eprintln!("保存实例数据失败: {}", e);
            }
        }
    });

    let categorized =
        gen_full_noun_geos_optimized(Arc::new(db_option.inner.clone()), &config, sender)
            .await
            .map_err(|e| anyhow::anyhow!("Full Noun 生成失败: {}", e))?;

    let _ = insert_handle.await;

    let cate = categorized.get_by_category(NounCategory::Cate);
    let loops = categorized.get_by_category(NounCategory::LoopOwner);
    let prims = categorized.get_by_category(NounCategory::Prim);

    let result = DbModelInstRefnos {
        bran_hanger_refnos: Arc::new(Vec::new()),
        use_cate_refnos: Arc::new(cate),
        loop_owner_refnos: Arc::new(loops),
        prim_refnos: Arc::new(prims),
    };

    Ok(result)
}

/// 兼容函数：旧版的 gen_geos_data
///
/// 这个函数在优化版本中暂未实现，需要从 gen_model_old.rs 迁移
#[deprecated(note = "此函数未在优化版本中实现，需要迁移")]
pub async fn gen_geos_data(
    dbno: Option<u32>,
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOptionExt,
    incr_updates: Option<IncrGeoUpdateLog>,
    sender: flume::Sender<aios_core::geometry::ShapeInstancesData>,
    target_sesno: Option<u32>,
    manual_boolean_mode: bool,
) -> Result<Vec<RefnoEnum>> {
    println!(
        "[gen_model] 兼容层 gen_geos_data -> 转发到 gen_model_old::gen_geos_data (dbno={:?}, manual_refnos_len={})",
        dbno,
        manual_refnos.len()
    );

    // 直接转发到旧实现，保持行为一致
    crate::fast_model::gen_model_old::gen_geos_data(
        dbno,
        manual_refnos.clone(),
        &db_option.inner,
        incr_updates,
        sender,
        target_sesno,
        manual_boolean_mode, // 传递手动布尔运算模式参数
    )
    .await
}
