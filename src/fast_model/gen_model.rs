use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::occ_generate::process_meshes_update_db_deep;
use crate::fast_model::occ_generate::{
    RootGenerationFailure, process_meshes_update_db_deep_report,
};
use crate::fast_model::occ_generate::{booleans_meshes_in_db, gen_meshes_in_db};
use crate::fast_model::shape_save::{SaveMode, run_shape_save_receiver};
use crate::fast_model::{
    cata_model, coverage_audit, loop_model, prim_model, resolve_desi_comp, shared,
};
use aios_core::geometry::{EleInstGeo, PlantGeoData, ShapeInstancesData};
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::CateGeoParam::{BoxImplied, TubeImplied};
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::prim_geo::tubing::TubiSize;
use aios_core::room::room::GLOBAL_AABB_TREE;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::tool::hash_tool::hash_two_str;
use aios_core::{DBType, prim_geo::*};
use aios_core::{RefU64, RefnoEnum, pdms_types::*};
use aios_core::{
    query_multi_children_refnos, query_type_refnos_by_dbnum, query_use_cate_refnos_by_dbnum,
};
use anyhow::Context as _;
use bevy_transform::prelude::Transform;
use dashmap::DashMap;
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt};
use glam::DVec3;
use glam::{DMat4, Vec3};
use nom::complete::bool;
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use parry3d::math::Isometry;
use rayon::iter::ParallelIterator;
use std::collections::HashSet;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::convert::TryFrom;
use std::io::Read;
use std::mem::take;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

///一个db生成模型里，汇总的参考号集合
#[derive(Debug, Clone, Default)]
pub struct DbModelInstRefnos {
    pub bran_hanger_refnos: Arc<Vec<RefnoEnum>>,
    pub use_cate_refnos: Arc<Vec<RefnoEnum>>,
    pub loop_owner_refnos: Arc<Vec<RefnoEnum>>,
    pub prim_refnos: Arc<Vec<RefnoEnum>>,
}

/// LOOP owner 只准由 `loop_model` 生成，不能同时落进 `prim_model`。
///
/// `NREV` / `REVO` 同时出现在 aios-core 的 primitive 与 loop-owner noun 表里：前者
/// 表示它们最终是基本体参数，后者表示参数必须先从子 LOOP/PLOO 拼出来。若两条路都跑，
/// 普通批次会在 `prim_model` 静默跳过；定向批次则把这个预期的 `None` 误报成 hard fail。
fn exclude_loop_owned_primitives(
    mut primitive_refnos: Vec<RefnoEnum>,
    loop_owner_refnos: &[RefnoEnum],
) -> Vec<RefnoEnum> {
    let loop_owners: HashSet<_> = loop_owner_refnos.iter().copied().collect();
    primitive_refnos.retain(|refno| !loop_owners.contains(refno));
    primitive_refnos
}

impl DbModelInstRefnos {
    pub async fn execute_gen_inst_meshes(
        &self,
        db_option_arc: Option<Arc<DbOption>>,
    ) -> anyhow::Result<()> {
        let mut handles = FuturesUnordered::new();
        let prim_refnos = self.prim_refnos.clone();
        let loop_owner_refnos = self.loop_owner_refnos.clone();
        let use_cate_refnos = self.use_cate_refnos.clone();
        let bran_hanger_refnos = self.bran_hanger_refnos.clone();

        let db_option = db_option_arc.clone();
        handles.push(
            crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                gen_meshes_in_db(db_option, &prim_refnos)
                    .await
                    .map_err(|error| anyhow::anyhow!("generate prim meshes failed: {error:#}"))
            }),
        );
        let db_option = db_option_arc.clone();
        handles.push(
            crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                gen_meshes_in_db(db_option.clone(), &loop_owner_refnos)
                    .await
                    .map_err(|error| anyhow::anyhow!("generate loop meshes failed: {error:#}"))
            }),
        );
        let db_option = db_option_arc.clone();
        handles.push(
            crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                gen_meshes_in_db(db_option, &use_cate_refnos)
                    .await
                    .map_err(|error| anyhow::anyhow!("generate use-cata meshes failed: {error:#}"))
            }),
        );
        let db_option = db_option_arc.clone();
        handles.push(
            crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                for bran_refnos in bran_hanger_refnos.chunks(20) {
                    let db_option_clone = db_option.clone();
                    // let refnos_str = bran_refnos.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(",");
                    let target_refnos =
                        query_multi_children_refnos(&bran_refnos)
                            .await
                            .map_err(|error| {
                                anyhow::anyhow!("query BRAN/HANG mesh children failed: {error:#}")
                            })?;
                    gen_meshes_in_db(db_option_clone, &target_refnos)
                        .await
                        .map_err(|error| {
                            anyhow::anyhow!("generate BRAN/HANG meshes failed: {error:#}")
                        })?;
                }
                Ok(())
            }),
        );
        wait_for_generation_workers(&mut handles).await
    }

    //执行布尔运算的操作
    pub async fn execute_boolean_meshes(
        &self,
        db_option_arc: Option<Arc<DbOption>>,
        failure_policy: crate::data_interface::geom_error::GeometryFailurePolicy,
    ) -> anyhow::Result<()> {
        let mut handles = FuturesUnordered::new();
        let prim_refnos = self.prim_refnos.clone();
        let loop_owner_refnos = self.loop_owner_refnos.clone();
        let use_cate_refnos = self.use_cate_refnos.clone();
        let bran_hanger_refnos = self.bran_hanger_refnos.clone();
        let db_option = db_option_arc.clone();
        handles.push(
            crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                booleans_meshes_in_db(db_option, &prim_refnos, failure_policy)
                    .await
                    .map_err(|error| anyhow::anyhow!("boolean prim meshes failed: {error:#}"))
            }),
        );
        let db_option = db_option_arc.clone();
        handles.push(
            crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                booleans_meshes_in_db(db_option, &loop_owner_refnos, failure_policy)
                    .await
                    .map_err(|error| anyhow::anyhow!("boolean loop meshes failed: {error:#}"))
            }),
        );
        let db_option = db_option_arc.clone();
        handles.push(
            crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                booleans_meshes_in_db(db_option, &use_cate_refnos, failure_policy)
                    .await
                    .map_err(|error| anyhow::anyhow!("boolean use-cata meshes failed: {error:#}"))
            }),
        );
        let db_option = db_option_arc.clone();
        handles.push(
            crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                for chunk in bran_hanger_refnos.chunks(20) {
                    let db_option_clone = db_option.clone();
                    let target_refnos =
                        query_multi_children_refnos(&chunk).await.map_err(|error| {
                            anyhow::anyhow!("query BRAN/HANG boolean children failed: {error:#}")
                        })?;
                    booleans_meshes_in_db(db_option_clone, &target_refnos, failure_policy)
                        .await
                        .map_err(|error| {
                            anyhow::anyhow!("boolean BRAN/HANG meshes failed: {error:#}")
                        })?;
                }
                Ok(())
            }),
        );
        wait_for_generation_workers(&mut handles).await
    }
}

/// 生成几何体数据。
///
/// 两种模式，由 `db_option.debug_root_refnos` 二选一：设了就只生成那批生成根
/// （`ModelRefreshPolicy::generate_roots` 唯一的入口，增量重算走这条），没设就整库全量。
///
/// # 参数
/// * `db_option` - 数据库选项配置
///
/// # 返回值
/// * `anyhow::Result<bool>` - 返回生成结果，成功返回true，失败返回错误
pub async fn gen_all_geos_data(db_option: &DbOption) -> anyhow::Result<bool> {
    gen_all_geos_data_with_policy(
        db_option,
        crate::data_interface::geom_error::GeometryFailurePolicy::BestEffortFallback,
    )
    .await
}

pub(crate) async fn gen_all_geos_data_with_policy(
    db_option: &DbOption,
    failure_policy: crate::data_interface::geom_error::GeometryFailurePolicy,
) -> anyhow::Result<bool> {
    const CHUNK_SIZE: usize = 100;
    // 定向生成（`debug_root_refnos` 选定的一批生成根）与整库全量生成的分界。
    let targeted = db_option.debug_root_refnos.is_some();
    let time = Instant::now();
    if targeted {
        let report = gen_targeted_geos_data_with_policy(
            db_option,
            failure_policy,
            crate::data_interface::model_concurrency::effective_root_inflight(),
        )
        .await?;
        if !report.failures.is_empty() {
            anyhow::bail!(crate::fast_model::occ_generate::summarize_root_failures(
                "targeted model",
                &report.failures
            ));
        }
    } else {
        let dbnos = if db_option.manual_db_nums.is_some() {
            db_option.manual_db_nums.clone().unwrap()
        } else {
            aios_core::query_mdb_db_nums(DBType::DESI).await?
        };

        // 过滤掉exclude_db_nums中的数据库编号
        let dbnos = if let Some(exclude_nums) = &db_option.exclude_db_nums {
            dbnos
                .into_iter()
                .filter(|dbno| !exclude_nums.contains(dbno))
                .collect::<Vec<_>>()
        } else {
            dbnos
        };

        println!(
            "整库全量生成 dbnum 名单（共 {} 个）: {dbnos:?}",
            dbnos.len()
        );
        let db_option_arc = Arc::new(db_option.clone());
        for dbno in dbnos.clone() {
            println!("开始{}的模型生成", dbno);
            let time = Instant::now();
            let (sender, receiver) = flume::bounded(CHUNK_SIZE);
            let receiver: flume::Receiver<ShapeInstancesData> = receiver.clone();
            let insert_task = tokio::task::spawn(async move {
                run_shape_save_receiver(receiver, SaveMode::FullBuild).await?;
                anyhow::Ok(())
            });
            let generation_result = capture_generation(gen_geos_data_by_dbnum(
                dbno,
                db_option_arc.clone(),
                sender.clone(),
            ))
            .await;
            let (db_refnos, ()) =
                finish_shape_writer(generation_result, sender, insert_task).await?;
            println!("生成完insts时间: {}ms", time.elapsed().as_millis());
            if db_option_arc.gen_mesh {
                let time = Instant::now();
                println!("开始执行模型生成和布尔运算");
                //模型生成完之后，再进行布尔运算
                db_refnos
                    .execute_gen_inst_meshes(Some(db_option_arc.clone()))
                    .await?;
                println!("生成insts三角模型时间: {}ms", time.elapsed().as_millis());
                let time = Instant::now();
                db_refnos
                    .execute_boolean_meshes(Some(db_option_arc.clone()), failure_policy)
                    .await?;
                println!("布尔运算时间: {}ms", time.elapsed().as_millis());
            }
        }
    }
    println!(
        "GLOBAL_AABB_TREE: {:?}",
        GLOBAL_AABB_TREE.read().await.tree.size()
    );
    // 只有全量生成无条件写回项目树文件——它本来就覆盖了此前的一切增量变更。
    //
    // 定向生成一概不写。这个文件现在约 21 MB，而定向路径一次可能只改了一个 BOX 的
    // XLEN；合批失败回退逐根时更是每个根写一遍。增量路径动内存树的两处（AABB 刷新、
    // 删除清理）都会 `mark_aabb_tree_dirty`，由 worker 空闲轮的
    // `persist_aabb_tree_if_dirty` 每轮最多落一次盘（ADR-010 落盘时机、ADR-012 背景）。
    //
    // 顺带修掉一处可靠性问题：这里原先是 `?`，磁盘写失败会让一次**已经成功**的生成
    // 返回 Err，于是 `model_update_pending` 把那个根标成 failed 并计进重试次数——重试
    // 重跑的还是同一份已经生成好的几何，而磁盘问题一点没被解决。
    if !targeted {
        crate::fast_model::aabb_tree::persist_aabb_tree().await?;
    }
    println!("生成完所有模型时间: {}ms", time.elapsed().as_millis());

    Ok(true)
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TargetedGenerationReport {
    pub completed: Vec<String>,
    pub failures: Vec<RootGenerationFailure>,
}

/// 定向生成的单 Shape writer + 根级后半程入口。实例生成仍按整组执行，只有
/// query/mesh/boolean/AABB 后半程按根并发，因而能逐根报告而不重跑健康根。
pub(crate) async fn gen_targeted_geos_data_with_policy(
    db_option: &DbOption,
    failure_policy: crate::data_interface::geom_error::GeometryFailurePolicy,
    root_inflight_max: usize,
) -> anyhow::Result<TargetedGenerationReport> {
    const CHUNK_SIZE: usize = 100;
    let execution_started = Instant::now();
    let spatial_before = crate::fast_model::spatial_state::spatial_serial_snapshot();
    let (sender, receiver) = flume::bounded(CHUNK_SIZE);
    let receiver: flume::Receiver<ShapeInstancesData> = receiver.clone();
    let insert_task =
        crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
            let outcome = run_shape_save_receiver(receiver, SaveMode::TargetedReplace).await?;
            anyhow::Ok(outcome)
        });
    let generation_result =
        capture_generation(gen_geos_data(None, vec![], db_option, sender.clone())).await;
    let (target_root_refnos, shape_outcome) =
        finish_shape_writer(generation_result, sender, insert_task).await?;
    crate::data_interface::model_concurrency::record_shape_run(
        shape_outcome
            .producer_blocked
            .as_micros()
            .min(u128::from(u64::MAX)) as u64,
        shape_outcome.sql_bytes,
        shape_outcome.instance_rows,
    );
    let shape_blocked = shape_outcome.producer_blocked;
    let produced = shape_outcome.written_refnos;

    crate::data_interface::helper::prune_roots_stale_model_rows(
        &target_root_refnos,
        &produced,
        300,
    )
    .await?;

    if db_option.gen_mesh {
        let report = process_meshes_update_db_deep_report(
            db_option,
            &target_root_refnos,
            failure_policy,
            root_inflight_max,
        )
        .await;
        let geometry = crate::fast_model::concurrency::snapshot();
        let spatial_after = crate::fast_model::spatial_state::spatial_serial_snapshot();
        let aabb_wait = spatial_after
            .wait_micros
            .saturating_sub(spatial_before.wait_micros);
        let aabb_held = spatial_after
            .held_micros
            .saturating_sub(spatial_before.held_micros);
        let shape_pressure =
            shape_blocked.as_micros() > execution_started.elapsed().as_micros().saturating_div(5);
        let aabb_pressure = aabb_wait > aabb_held.saturating_div(2);
        crate::data_interface::model_concurrency::record_window(
            report.completed.len(),
            report.failures.len(),
            !report.failures.is_empty()
                || shape_pressure
                || aabb_pressure
                || geometry.waiting > geometry.quota.saturating_mul(2),
        );
        Ok(TargetedGenerationReport {
            completed: report.completed,
            failures: report.failures,
        })
    } else {
        Ok(TargetedGenerationReport {
            completed: target_root_refnos.iter().map(ToString::to_string).collect(),
            failures: Vec::new(),
        })
    }
}

/// 等生成与写入两侧都收尾，两边都成功才把结果交出去。
///
/// 写入侧的产物类型是泛型：定向重生成要把「本轮产出过几何的元素」带出来做收尾清理，
/// 整库全量生成不需要，交 `()`。
async fn finish_shape_writer<T, W>(
    generation_result: anyhow::Result<T>,
    sender: flume::Sender<ShapeInstancesData>,
    insert_task: tokio::task::JoinHandle<anyhow::Result<W>>,
) -> anyhow::Result<(T, W)> {
    drop(sender);
    let writer_result = match insert_task.await {
        Ok(result) => result,
        Err(error) => Err(anyhow::anyhow!("shape writer task failed: {error}")),
    };
    match (generation_result, writer_result) {
        (Ok(value), Ok(written)) => Ok((value, written)),
        (Err(error), Ok(_)) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(writer_error)) => {
            Err(error.context(format!("shape writer also failed: {writer_error:#}")))
        }
    }
}

async fn capture_generation<T>(
    generation: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    match std::panic::AssertUnwindSafe(generation)
        .catch_unwind()
        .await
    {
        Ok(result) => result,
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown panic payload");
            Err(anyhow::anyhow!("geometry generation panicked: {message}"))
        }
    }
}

///更新模型数据
/// 根据数据库编号处理网格数据
///
/// # 参数
///
/// * `dbnos` - 数据库编号数组
/// * `db_option` - 数据库选项配置
///
/// # 返回值
///
/// 返回 `anyhow::Result<()>` 表示处理是否成功
pub(crate) async fn process_meshes_by_dbnos(
    dbnos: &[u32],
    db_option: &DbOption,
) -> anyhow::Result<()> {
    let mut time = Instant::now();
    let include_history = db_option.is_gen_history_model();

    // 过滤掉exclude_db_nums中的数据库编号
    let filtered_dbnos = if let Some(exclude_nums) = &db_option.exclude_db_nums {
        dbnos
            .iter()
            .filter(|&&dbno| !exclude_nums.contains(&dbno))
            .copied()
            .collect::<Vec<_>>()
    } else {
        dbnos.to_vec()
    };

    for &dbno in &filtered_dbnos {
        let sites = query_type_refnos_by_dbnum(&["SITE"], dbno, None, include_history).await?;
        // 错误向上传播（不再 .expect panic），与增量路径一致。
        process_meshes_update_db_deep(db_option, &sites).await?;
    }
    println!("更新所有模型时间: {}ms", time.elapsed().as_millis());
    Ok(())
}

///生成几何体数据
/// 根据数据库编号生成几何体数据
///
/// # 参数
///
/// * `dbno` - 数据库编号
/// * `db_option_arc` - 数据库选项的Arc指针
/// * `sender` - 形状实例数据的发送通道
///
/// # 返回值
///
/// 返回 `Result<DbModelInstRefnos>` 表示生成是否成功以及生成的模型实例引用号
pub async fn gen_geos_data_by_dbnum(
    dbno: u32,
    db_option_arc: Arc<DbOption>,
    sender: flume::Sender<ShapeInstancesData>,
) -> anyhow::Result<DbModelInstRefnos> {
    let gen_history = db_option_arc.is_gen_history_model();
    // 判断有空的层级，不用去生成。查询失败必须上抛：此前是 `unwrap_or_default()`，
    // 一次查询抖动会把「查不出来」折成「空库」，整个 dbnum 静默跳过生成且无人知晓
    // ——下同，本函数判定链上的查询失败一律等于本库生成失败，不是空集。
    let zones = query_type_refnos_by_dbnum(&["ZONE"], dbno, Some(true), gen_history)
        .await
        .with_context(|| format!("查询 dbnum={dbno} 的 ZONE 层级失败，本库模型生成中止"))?;
    if zones.is_empty() {
        return Ok(Default::default());
    }
    // let mut all_handles = FuturesUnordered::new();

    println!("gen_geos_data_by_dbnum 处理db: {}", dbno);
    // 三类目开关与调试旋钮解耦（审计 F3）：`debug_refno_types` 是「调试期只跑某几类」
    // 的过滤器，此前它为空（生产常态）时三类 flag 全 false——整库全量什么都不生成，
    // 且无任何输出，调试旋钮支配了生产行为。现语义：留空 = 三类全开（生产默认）；
    // 写了名单 = 按名单过滤（调试用途保留）。定向路径不受影响（generate_roots_report
    // 显式强设三类后才进来）。
    let d_types = db_option_arc.debug_refno_types.clone();
    let debug_type_filter_active = !d_types.is_empty();
    if debug_type_filter_active {
        println!("debug_refno_types 过滤生效，仅生成 {d_types:?}（调试旋钮；生产全量请留空配置）");
    }
    let gen_cata_flag = !debug_type_filter_active || d_types.iter().any(|x| x == "CATA");
    let gen_loop_flag = !debug_type_filter_active || d_types.iter().any(|x| x == "LOOP");
    let gen_prim_flag = !debug_type_filter_active || d_types.iter().any(|x| x == "PRIM");
    let gen_model = db_option_arc.gen_model;
    let test_refno = db_option_arc.get_test_refno();

    // dbg!(origin_root_refnos.len());
    //需要在这里把origin_root_refnos 打断成小块
    //遍历小块
    //Step 1、提前缓存ploo, 得到对齐方式的偏移
    let loop_sjus_map = DashMap::new();
    {
        //查找到子节点的所有PLOO类型
        let target_ploo_refnos =
            query_type_refnos_by_dbnum(&["PLOO"], dbno, Some(true), gen_history)
                .await
                .with_context(|| format!("查询 dbnum={dbno} 的 PLOO 失败，本库模型生成中止"))?;
        #[cfg(debug_assertions)]
        if !target_ploo_refnos.is_empty() {
            println!("target_ploo_refnos: {:?}", target_ploo_refnos.len());
        }
        if gen_model {
            for r in target_ploo_refnos.chunks(200) {
                let sql = format!(
                    "select value [OWNER, HEIG, SJUS] from [{}] where SJUS!=0",
                    r.iter()
                        .map(|x| x.to_table_key("PLOO"))
                        .collect::<Vec<_>>()
                        .join(",")
                );
                let mut response = crate::data_interface::staging::active_data_db()
                    .query(sql)
                    .await?;
                // response.take_errors()
                let tuples: Vec<(RefnoEnum, f32, String)> = response.take(0)?;
                // dbg!(&tuples[0]);
                for (owner, height, sjus) in tuples {
                    let off_z = cata_model::cal_sjus_value(&sjus, height);
                    //对齐方式的距离，应该存储下来，子节点要与其保持一致的偏移
                    //插入方向和偏移距离
                    loop_sjus_map.insert(owner, (Vec3::NEG_Z * off_z, height));
                }
            }
        }
    }
    let loop_sjus_map_arc = Arc::new(loop_sjus_map);

    // 类目生成任务集（审计 F2：恢复被注释掉的历史并发设计）。判定查询保持顺序执行，
    // 生成阶段 spawn 并发——单 Shape writer 从有界 flume 通道消费，天然承接多生产者
    // 并提供背压；`spawn_with_staged_io` 传播暂存读写上下文（无上下文时等价普通
    // spawn）。收口在函数尾：join 完全部任务再上抛第一个失败，不留 detached 任务
    // 往通道里继续发。
    let mut generation_tasks: Vec<(String, tokio::task::JoinHandle<anyhow::Result<()>>)> = vec![];

    //Step 2、按类目先逐个分好类的参考号集合
    //2.1 管道或者支吊架的分类
    let target_bran_hanger_refnos =
        Arc::new(query_type_refnos_by_dbnum(&["BRAN", "HANG"], dbno, None, gen_history).await?);
    println!(
        "当前分段使用管道或者支吊架元件库数量: {}",
        target_bran_hanger_refnos.len()
    );

    //打印管道/支吊架的使用数量
    if !target_bran_hanger_refnos.is_empty() && gen_cata_flag && gen_model {
        //查询出branch 和 branch 下的子节点
        let mut branch_refnos_map = DashMap::new();
        let mut bran_comp_eles = HashSet::new();
        for &refno in target_bran_hanger_refnos.as_slice() {
            let children = aios_core::get_children_pes(refno).await.with_context(|| {
                format!("查询 dbnum={dbno} BRAN/HANG {refno:?} 的子节点失败，本库模型生成中止")
            })?;
            bran_comp_eles.extend(children.iter().map(|x| x.refno));
            //求出元件对应的outside bore
            branch_refnos_map.insert(refno, children);
        }

        let target_bran_reuse_cata_map: DashMap<String, CataHashRefnoKV> = {
            let map = aios_core::query_group_by_cata_hash(target_bran_hanger_refnos.as_slice())
                .await
                .with_context(|| {
                    format!("查询 dbnum={dbno} BRAN/HANG 的元件库分组失败，本库模型生成中止")
                })?;
            if let Some(t_refno) = test_refno {
                if bran_comp_eles.contains(&t_refno) {
                    for kv in &map {
                        if kv.value().group_refnos.contains(&t_refno) {
                            dbg!(kv.value());
                        }
                    }
                }
            }
            map
        };

        //元件库的模型计算
        //bran，hanger下需要重用的模型
        if gen_model && (!target_bran_reuse_cata_map.is_empty() || !branch_refnos_map.is_empty()) {
            let sjus_map_clone = loop_sjus_map_arc.clone();
            let db_option = db_option_arc.clone();
            let sender = sender.clone();
            generation_tasks.push((
                "BRAN/HANG 元件库".to_string(),
                crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                    let start_time = Instant::now();
                    cata_model::gen_cata_geos(
                        db_option,
                        Arc::new(target_bran_reuse_cata_map),
                        Arc::new(branch_refnos_map),
                        sjus_map_clone,
                        sender,
                    )
                    .await?;
                    println!(
                        "BRAN/HANG cata_model::gen_cata_geos执行时间: {}ms",
                        start_time.elapsed().as_millis()
                    );
                    Ok(())
                }),
            ));
        }
    }
    let mut use_cate_refnos = vec![];
    for cate_names in USE_CATE_NOUN_NAMES.chunks(4) {
        let refnos = query_use_cate_refnos_by_dbnum(cate_names, dbno, gen_history).await?;
        if refnos.is_empty() {
            continue;
        }
        use_cate_refnos.extend(refnos.clone());
        let cur_cate_refnos = Arc::new(refnos);
        // dbg!(cur_cate_refnos.len());
        //查询单个使用元件库的数量
        let target_single_cata_map = {
            //要过滤掉owner是BRAN 和 HANG的
            let map = aios_core::query_group_by_cata_hash(cur_cate_refnos.as_slice())
                .await
                .with_context(|| {
                    format!("查询 dbnum={dbno} 单用元件库分组失败，本库模型生成中止")
                })?;
            map
        };

        println!("当前分段使用元件库数量: {}", cur_cate_refnos.len());
        if gen_model && gen_cata_flag && !target_single_cata_map.is_empty() {
            let sjus_map_clone = loop_sjus_map_arc.clone();
            let db_option = db_option_arc.clone();
            let sender = sender.clone();
            generation_tasks.push((
                format!("单用元件库 {cate_names:?}"),
                crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                    let start_time = Instant::now();
                    cata_model::gen_cata_geos(
                        db_option,
                        Arc::new(target_single_cata_map),
                        Arc::new(Default::default()),
                        sjus_map_clone,
                        sender,
                    )
                    .await?;
                    println!(
                        "单个使用元件库 cata_model::gen_cata_geos执行时间: {}ms",
                        start_time.elapsed().as_millis()
                    );
                    Ok(())
                }),
            ));
        }
    }

    let target_loop_owner_refnos = Arc::new(
        query_type_refnos_by_dbnum(&GNERAL_LOOP_OWNER_NOUN_NAMES, dbno, Some(true), gen_history)
            .await
            .with_context(|| format!("查询 dbnum={dbno} 的 LOOP owner 失败，本库模型生成中止"))?,
    );
    println!("当前分段使用LOOP的数量: {}", target_loop_owner_refnos.len());
    if gen_model && gen_loop_flag && !target_loop_owner_refnos.is_empty() {
        let sjus_map_clone = loop_sjus_map_arc.clone();
        let sender = sender.clone();
        let db_option = db_option_arc.clone();
        let target_loop_owner_refnos_arc = target_loop_owner_refnos.clone();
        generation_tasks.push((
            "LOOP".to_string(),
            crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                loop_model::gen_loop_geos(
                    db_option,
                    &target_loop_owner_refnos_arc,
                    sjus_map_clone,
                    sender,
                )
                .await?;
                Ok(())
            }),
        ));
    }

    let target_prim_refnos = Arc::new(exclude_loop_owned_primitives(
        query_type_refnos_by_dbnum(&GNERAL_PRIM_NOUN_NAMES, dbno, None, gen_history)
            .await
            .with_context(|| format!("查询 dbnum={dbno} 的基本体失败，本库模型生成中止"))?,
        target_loop_owner_refnos.as_slice(),
    ));

    println!("当前分段使用基本体数量: {}", target_prim_refnos.len());
    //基本元件的生成
    if gen_model && gen_prim_flag && !target_prim_refnos.is_empty() {
        //基本体模型的生成
        let db_option = db_option_arc.clone();
        let sender = sender.clone();
        let target_prim_refnos_arc = target_prim_refnos.clone();
        generation_tasks.push((
            "基本体".to_string(),
            crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                prim_model::gen_prim_geos(db_option, target_prim_refnos_arc.as_slice(), sender)
                    .await?;
                Ok(())
            }),
        ));
    }

    // 并发生成统一收口：先 join 完全部任务（不留 detached 任务往写通道里继续发），
    // 逐个报告失败，再上抛第一个——任一类目失败 = 本库生成失败，与判定链同一语义。
    let mut first_generation_error: Option<anyhow::Error> = None;
    for (label, handle) in generation_tasks {
        let result = match handle.await {
            Ok(result) => result.with_context(|| format!("dbnum={dbno} {label} 生成失败")),
            Err(join_error) => Err(anyhow::anyhow!(
                "dbnum={dbno} {label} 生成任务崩溃: {join_error}"
            )),
        };
        if let Err(error) = result {
            eprintln!("{error:#}");
            if first_generation_error.is_none() {
                first_generation_error = Some(error);
            }
        }
    }
    if let Some(error) = first_generation_error {
        return Err(error);
    }

    let db_refnos = DbModelInstRefnos {
        bran_hanger_refnos: target_bran_hanger_refnos,
        use_cate_refnos: Arc::new(use_cate_refnos),
        loop_owner_refnos: target_loop_owner_refnos,
        prim_refnos: target_prim_refnos,
    };

    println!("数据库号： {} 生成instances完毕。", dbno);

    Ok(db_refnos)
}

///生成几何体数据
pub async fn gen_geos_data(
    dbno: Option<u32>,
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOption,
    sender: flume::Sender<ShapeInstancesData>,
) -> anyhow::Result<Vec<RefnoEnum>> {
    let mut all_handles = FuturesUnordered::new();
    const CHUNK_SIZE: usize = 100;
    let has_manual_refnos = !manual_refnos.is_empty();
    let debug_root_refnos = db_option.get_all_debug_refnos().await;
    let has_debug = !debug_root_refnos.is_empty();
    let skip_exist = !(db_option.is_replace_mesh() || has_manual_refnos || has_debug);
    //debug_root_refnos = [] 时表示不生成模型，如果没有这个属性表示生成所有
    if db_option.debug_root_refnos.is_some() && debug_root_refnos.is_empty() && !has_manual_refnos {
        return Ok(vec![]);
    }
    let db_option_arc = Arc::new(db_option.clone());
    let is_debug = debug_root_refnos.len() > 0;

    let include_history = db_option_arc.is_gen_history_model();
    let mut target_root_refnos = vec![];
    if is_debug || has_manual_refnos {
        target_root_refnos = if has_manual_refnos {
            manual_refnos.clone()
        } else {
            debug_root_refnos.clone()
        };
    } else if dbno.is_some() {
        target_root_refnos =
            query_type_refnos_by_dbnum(&["SITE"], dbno.unwrap(), Some(true), include_history)
                .await?
                .into_iter()
                .collect();
    }
    if dbno.is_some() {
        println!("总共 {} 个SITE", target_root_refnos.len());
    } else {
        println!("总共 {} 个结点", target_root_refnos.len());
    }
    let origin_root_refnos = target_root_refnos.clone();
    if has_manual_refnos {
        println!("处理生成模型数量: {}", manual_refnos.len());
    } else if is_debug {
        println!("调试模型数量: {:?}", debug_root_refnos.len());
    } else if dbno.is_some() {
        println!("处理db: {}", dbno.unwrap());
    }
    let d_types = db_option_arc.debug_refno_types.clone();
    let gen_cata_flag = d_types.iter().any(|x| x == "CATA") || has_manual_refnos;
    let gen_loop_flag = d_types.iter().any(|x| x == "LOOP") || has_manual_refnos;
    let gen_prim_flag = d_types.iter().any(|x| x == "PRIM") || has_manual_refnos;

    //需要在这里把origin_root_refnos 打断成小块
    let mut chunked_root_refnos = origin_root_refnos.chunks(CHUNK_SIZE);
    let gen_model = db_option_arc.gen_model || has_manual_refnos;
    //遍历小块
    while gen_model && let Some(target_refnos) = chunked_root_refnos.next() {
        //Step 1、提前缓存ploo, 得到对齐方式的偏移
        let loop_sjus_map = DashMap::new();
        //TODO 检查两个类型是否有可能在一个层级树里，如果不需要可以跳过
        {
            //查找到子节点的所有PLOO类型
            let target_ploo_refnos = aios_core::query_multi_deep_versioned_children_filter_inst(
                target_refnos,
                &["PLOO"],
                skip_exist,
            )
            .await?;
            #[cfg(debug_assertions)]
            if !target_ploo_refnos.is_empty() {
                println!("target_ploo_refnos: {:?}", target_ploo_refnos.len());
            }
            for r in target_ploo_refnos {
                let loop_att = aios_core::get_named_attmap(r).await?;
                let owner = loop_att.get_owner();
                let mut height = loop_att
                    .get_f32("HEIG")
                    .unwrap_or(loop_att.get_f32("HEIG").unwrap_or_default());
                let sjus = loop_att.get_str("SJUS").unwrap_or_default();
                let off_z = cata_model::cal_sjus_value(sjus, height);
                //对齐方式的距离，应该存储下来，子节点要与其保持一致的偏移
                //插入方向和偏移距离
                loop_sjus_map.insert(owner, (Vec3::NEG_Z * off_z, height));
            }
        }
        let loop_sjus_map_arc = Arc::new(loop_sjus_map);

        //Step 2、按类目先逐个分好类的参考号集合
        //2.1 管道或者支吊架的分类
        let target_bran_hanger_refnos: Vec<RefnoEnum> =
            aios_core::query_multi_deep_versioned_children_filter_inst(
                target_refnos,
                &["BRAN", "HANG"],
                skip_exist,
            )
            .await?
            .into_iter()
            .collect();
        let target_bran_reuse_cata_map: DashMap<String, CataHashRefnoKV> =
            { aios_core::query_group_by_cata_hash(&target_bran_hanger_refnos).await? };
        let mut use_cata_refnos = HashSet::new();
        //查询单个使用元件库的数量
        let target_single_cata_map = {
            //查询是否是单个使用元件库，父节点是BRAN HANG
            let sql = format!(
                "select value refno from [{}] where owner.noun in ['BRAN', 'HANG']",
                target_refnos
                    .iter()
                    .map(|x| x.to_pe_key())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let mut response = crate::data_interface::staging::active_data_db()
                .query(sql)
                .await?
                .check()?;
            let bran_children_refnos = response.take::<Vec<RefnoEnum>>(0)?;
            let single_refnos = target_refnos
                .iter()
                .filter(|x| !target_bran_hanger_refnos.contains(x))
                .map(|x| *x)
                .collect::<Vec<_>>();
            use_cata_refnos =
                aios_core::query_multi_deep_children_filter_spre(single_refnos, skip_exist).await?;
            // dbg!(&use_cata_refnos);
            use_cata_refnos.extend(bran_children_refnos);
            aios_core::query_group_by_cata_hash(&use_cata_refnos).await?
        };
        //打印管道/支吊架的使用数量
        if !target_bran_hanger_refnos.is_empty() && gen_cata_flag {
            println!(
                "当前分段使用管道或者支吊架元件库数量: {}",
                target_bran_hanger_refnos.len()
            );
            //查询出branch 和 branch 下的子节点
            let mut branch_refnos_map = DashMap::new();
            let mut bran_comp_eles = vec![];
            for &refno in &target_bran_hanger_refnos {
                let children = aios_core::get_children_pes(refno).await?;
                bran_comp_eles.extend(children.iter().map(|x| x.refno));
                //求出元件对应的outside bore
                branch_refnos_map.insert(refno, children);
            }

            //元件库的模型计算
            //bran，hanger下需要重用的模型
            if !target_bran_reuse_cata_map.is_empty() || !branch_refnos_map.is_empty() {
                let sjus_map_clone = loop_sjus_map_arc.clone();
                let db_option = db_option_arc.clone();
                let sender = sender.clone();
                // cata stage 是叶子（gen_cata_geos 内部顺序执行、不 spawn 过闸子任务），
                // 拿许可与 loop/prim 的分块 worker 共享同一份额度（specs/023）。
                let handle = crate::fast_model::concurrency::spawn_gated_leaf(async move {
                    let start_time = Instant::now();
                    cata_model::gen_cata_geos(
                        db_option,
                        Arc::new(target_bran_reuse_cata_map),
                        Arc::new(branch_refnos_map),
                        sjus_map_clone,
                        sender,
                    )
                    .await?;
                    println!(
                        "异步BRAN/HANG cata_model::gen_cata_geos执行时间: {}ms",
                        start_time.elapsed().as_millis()
                    );
                    anyhow::Ok(())
                });
                all_handles.push(handle);
            }
        }

        if gen_cata_flag && !target_single_cata_map.is_empty() {
            println!("当前分段使用独立的元件库数量: {}", use_cata_refnos.len());
            let sjus_map_clone = loop_sjus_map_arc.clone();
            let db_option = db_option_arc.clone();
            let sender = sender.clone();
            // 同上：cata stage 是叶子，拿许可。
            let handle = crate::fast_model::concurrency::spawn_gated_leaf(async move {
                let start_time = Instant::now();
                cata_model::gen_cata_geos(
                    db_option,
                    Arc::new(target_single_cata_map),
                    Arc::new(Default::default()),
                    sjus_map_clone,
                    sender,
                )
                .await?;
                println!(
                    "异步单个使用元件库 cata_model::gen_cata_geos执行时间: {}ms",
                    start_time.elapsed().as_millis()
                );
                anyhow::Ok(())
            });
            all_handles.push(handle);
        }

        let target_loop_owner_refnos: Vec<RefnoEnum> =
            aios_core::query_multi_deep_versioned_children_filter_inst(
                target_refnos,
                &GNERAL_LOOP_OWNER_NOUN_NAMES,
                skip_exist,
            )
            .await?
            .into_iter()
            .collect();
        let loop_owned_for_prim = target_loop_owner_refnos.clone();
        if gen_loop_flag && !target_loop_owner_refnos.is_empty() {
            println!("当前分段使用LOOP的数量: {}", target_loop_owner_refnos.len());
            let sjus_map_clone = loop_sjus_map_arc.clone();
            let sender = sender.clone();
            let db_option = db_option_arc.clone();
            let handle =
                crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                    loop_model::gen_loop_geos(
                        db_option,
                        &target_loop_owner_refnos,
                        sjus_map_clone,
                        sender,
                    )
                    .await?;
                    anyhow::Ok(())
                });
            all_handles.push(handle);
        }

        let target_prim_refnos: Vec<RefnoEnum> = exclude_loop_owned_primitives(
            aios_core::query_multi_deep_versioned_children_filter_inst(
                target_refnos,
                &GNERAL_PRIM_NOUN_NAMES,
                skip_exist,
            )
            .await?
            .into_iter()
            .collect(),
            &loop_owned_for_prim,
        );

        //基本元件的生成
        if gen_prim_flag && !target_prim_refnos.is_empty() {
            println!("当前分段使用基本体数量: {}", target_prim_refnos.len());
            //基本体模型的生成
            let db_option = db_option_arc.clone();
            let sender = sender.clone();
            let handle =
                crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                    prim_model::gen_prim_geos(db_option, target_prim_refnos.as_slice(), sender)
                        .await?;
                    anyhow::Ok(())
                });
            all_handles.push(handle);
        }

        // 阶段 2 catch-all 观测：只记录「dict 认几何但不在任何生成路由名单」的 noun 实证，
        // 不参与生成、不改变结果；默认关闭，由 AIOS_GEOM_COVERAGE_AUDIT 打开。
        coverage_audit::audit_segment(target_refnos, skip_exist).await;

        wait_for_generation_workers(&mut all_handles).await?;
    }
    wait_for_generation_workers(&mut all_handles).await?;
    coverage_audit::report_and_reset();

    if dbno.is_some() {
        println!("数据库号： {} 生成instances完毕。", dbno.unwrap());
    }

    Ok(target_root_refnos)
}

async fn wait_for_generation_workers(
    handles: &mut FuturesUnordered<tokio::task::JoinHandle<anyhow::Result<()>>>,
) -> anyhow::Result<()> {
    let mut first_error = None;
    while let Some(result) = handles.next().await {
        let error = match result {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(error),
            Err(error) => Some(anyhow::anyhow!("generation worker task failed: {error}")),
        };
        if first_error.is_none() {
            first_error = error;
        }
    }
    first_error.map_or_else(|| Ok(()), Err)
}

///查询tubi的大小
pub async fn query_tubi_size(
    refno: RefnoEnum,
    tubi_cat_ref: RefnoEnum,
    is_hang: bool,
) -> anyhow::Result<TubiSize> {
    let tubi_geoms_info = resolve_desi_comp(refno, Some(tubi_cat_ref))
        .await
        .unwrap_or_default();
    // dbg!(&tubi_geoms_info);
    for geom in &tubi_geoms_info.geometries {
        if let BoxImplied(d) = geom {
            return Ok(TubiSize::BoxSize((d.height, d.width)));
        } else if let TubeImplied(d) = geom {
            return Ok(TubiSize::BoreSize(d.diameter));
        }
    }
    {
        if let Ok(cat_att) = aios_core::get_named_attmap(tubi_cat_ref).await {
            let params = cat_att.get_f32_vec("PARA").unwrap_or_default();
            if params.len() >= 2 {
                let tubi_bore = params[if is_hang { 0 } else { 1 }] as f32;
                return Ok(TubiSize::BoreSize(tubi_bore));
            }
        };
    }
    return Ok(TubiSize::None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn loop_owned_revolution_is_not_sent_to_the_primitive_worker() {
        let nrev = RefnoEnum::from("24381/36946");
        let box_refno = RefnoEnum::from("24381/1");

        let routed = exclude_loop_owned_primitives(vec![nrev, box_refno], &[nrev]);

        assert_eq!(routed, vec![box_refno]);
    }

    /// 定向重生成不得整体写回 `accel_tree.bin`：文件约 21 MB，而定向路径一次可能只改了
    /// 一个属性，合批失败回退逐根时更是每根写一遍。增量变更由 `mark_aabb_tree_dirty` 加
    /// worker 空闲轮的 `persist_aabb_tree_if_dirty` 负责（ADR-010 落盘时机）。
    ///
    /// 实跑要写 cwd 下那个 21 MB 文件，单测只能钉源码。
    #[test]
    fn only_a_full_generation_persists_the_spatial_tree() {
        let source = include_str!("gen_model.rs");
        let body = source
            .split_once(concat!("pub async fn ", "gen_all_geos_data("))
            .expect("gen_all_geos_data must exist")
            .1
            .split_once(concat!("async fn ", "finish_shape_writer"))
            .expect("finish_shape_writer must follow it")
            .0;

        assert!(
            !body.contains("serialize_to_bin_file"),
            "定向重生成不得直接整体序列化空间树"
        );
        let gate_at = body
            .find("if !targeted {")
            .expect("落盘必须由 !targeted 把关");
        let persist_at = body
            .find("persist_aabb_tree()")
            .expect("全量生成仍要写回 accel_tree.bin");
        assert!(gate_at < persist_at, "落盘必须待在 !targeted 分支里");
    }

    /// 陈旧行清理必须排在「生成与写入都成功」之后，且它自己失败时向上传播。
    ///
    /// 排到前面、或者不看成败，就会在一次半途失败的生成之后把「本轮没做出来」的行当成
    /// 「不再画了」删掉——正是 ADR-014 要挡的方向。收尾部分成功时必须让整根重试，
    /// 才能补完已删 inst_relate 之后失败的房间边/空间树清理。
    #[test]
    fn stale_row_pruning_waits_for_success_and_propagates_cleanup_failure() {
        let source = include_str!("gen_model.rs");
        let body = source
            .split_once(concat!("pub async fn ", "gen_all_geos_data("))
            .expect("gen_all_geos_data must exist")
            .1
            .split_once(concat!("async fn ", "finish_shape_writer"))
            .expect("finish_shape_writer must follow it")
            .0;

        let settle_at = body
            .find("finish_shape_writer(generation_result, sender, insert_task).await?")
            .expect("生成与写入必须先一起收尾");
        let prune_at = body
            .find("prune_roots_stale_model_rows(")
            .expect("收尾之后才清理陈旧行");
        assert!(settle_at < prune_at, "清理必须排在收尾之后: {body}");

        let mesh_at = body[prune_at..]
            .find("if db_option.gen_mesh")
            .map(|offset| prune_at + offset)
            .expect("陈旧行清理之后必须仍是 mesh 阶段");
        assert!(
            body[prune_at..mesh_at].contains(".await?"),
            "清理失败必须上抛，让根 pending 重试补完部分成功的收尾: {body}"
        );
    }

    #[tokio::test]
    async fn generation_worker_error_is_returned() {
        let mut handles = FuturesUnordered::new();
        handles.push(
            crate::data_interface::staging::write_context::spawn_with_staged_io(async {
                anyhow::bail!("worker failed")
            }),
        );

        let error = wait_for_generation_workers(&mut handles)
            .await
            .expect_err("a worker failure must fail the generation request");

        assert!(error.to_string().contains("worker failed"));
    }

    #[tokio::test]
    async fn generation_worker_panic_is_returned() {
        let mut handles = FuturesUnordered::new();
        handles.push(
            crate::data_interface::staging::write_context::spawn_with_staged_io(async {
                panic!("worker panic")
            }),
        );

        let error = wait_for_generation_workers(&mut handles)
            .await
            .expect_err("a worker panic must fail the generation request");

        assert!(error.to_string().contains("task"));
    }

    #[tokio::test]
    async fn generation_worker_failure_waits_for_siblings() {
        let completed = Arc::new(AtomicBool::new(false));
        let completed_by_worker = completed.clone();
        let mut handles = FuturesUnordered::new();
        handles.push(
            crate::data_interface::staging::write_context::spawn_with_staged_io(async {
                anyhow::bail!("first worker failed")
            }),
        );
        handles.push(
            crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                completed_by_worker.store(true, Ordering::SeqCst);
                anyhow::Ok(())
            }),
        );

        wait_for_generation_workers(&mut handles)
            .await
            .expect_err("the first worker failure must be returned");

        assert!(
            completed.load(Ordering::SeqCst),
            "worker siblings must be drained before generation returns"
        );
    }

    #[tokio::test]
    async fn generation_panic_waits_for_shape_writer() {
        let writer_completed = Arc::new(AtomicBool::new(false));
        let writer_completed_by_task = writer_completed.clone();
        let (sender, receiver) = flume::bounded(1);
        let writer =
            crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                while receiver.recv_async().await.is_ok() {}
                writer_completed_by_task.store(true, Ordering::SeqCst);
                anyhow::Ok(())
            });

        let generation_result = capture_generation(async {
            panic!("generator panic");
            #[allow(unreachable_code)]
            anyhow::Ok(())
        })
        .await;
        let error = finish_shape_writer(generation_result, sender, writer)
            .await
            .expect_err("a generator panic must fail the request");

        assert!(error.to_string().contains("generator panic"), "{error:#}");
        assert!(
            writer_completed.load(Ordering::SeqCst),
            "shape writer must exit before generation returns"
        );
    }
}
