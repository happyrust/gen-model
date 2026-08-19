use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::shape_save::{SaveMode, run_shape_save_receiver};
use crate::fast_model::{
    booleans_meshes_in_db, cata_model, coverage_audit, gen_meshes_in_db, loop_model, prim_model,
    resolve_desi_comp, shared,
};
use crate::process_meshes_update_db_deep;
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
    ) -> anyhow::Result<()> {
        let mut handles = FuturesUnordered::new();
        let prim_refnos = self.prim_refnos.clone();
        let loop_owner_refnos = self.loop_owner_refnos.clone();
        let use_cate_refnos = self.use_cate_refnos.clone();
        let bran_hanger_refnos = self.bran_hanger_refnos.clone();
        let db_option = db_option_arc.clone();
        handles.push(
            crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                booleans_meshes_in_db(db_option, &prim_refnos)
                    .await
                    .map_err(|error| anyhow::anyhow!("boolean prim meshes failed: {error:#}"))
            }),
        );
        let db_option = db_option_arc.clone();
        handles.push(
            crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                booleans_meshes_in_db(db_option, &loop_owner_refnos)
                    .await
                    .map_err(|error| anyhow::anyhow!("boolean loop meshes failed: {error:#}"))
            }),
        );
        let db_option = db_option_arc.clone();
        handles.push(
            crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                booleans_meshes_in_db(db_option, &use_cate_refnos)
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
                    booleans_meshes_in_db(db_option_clone, &target_refnos)
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
    const CHUNK_SIZE: usize = 100;
    // 定向生成（`debug_root_refnos` 选定的一批生成根）与整库全量生成的分界。
    let targeted = db_option.debug_root_refnos.is_some();
    let time = Instant::now();
    if targeted {
        let (sender, receiver) = flume::bounded(CHUNK_SIZE);
        let receiver: flume::Receiver<ShapeInstancesData> = receiver.clone();
        let insert_task =
            crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                // 本轮产出过几何的元素（含隐含直管段）。收尾清理拿它与生成根子树求差，
                // 认出「上一版画得出、这一版画不出」的旧行——`save_instance_data` 的替换
                // 写入只覆盖得到这次也生成了的那些。
                // 只有保存成功的 outcome 才进入 produced；NaN、渲染或数据库失败会在此
                // 上抛，因此下面的 stale prune 不会建立在“收到过但没写成”的假事实之上。
                let outcome = run_shape_save_receiver(receiver, SaveMode::TargetedReplace).await?;
                anyhow::Ok(outcome.written_refnos)
            });
        let generation_result =
            capture_generation(gen_geos_data(None, vec![], db_option, sender.clone())).await;
        let (target_root_refnos, produced) =
            finish_shape_writer(generation_result, sender, insert_task).await?;

        // 收尾清理只在生成与写入都成功之后跑：这个差集分不清「真的不画了」与「本轮
        // 生成没做出来」，它的正确性押在「生成成功 ⇒ 产物完整」上（2026-08-05 决策，
        // ADR-014 的保留旧显示因此收窄为「生成失败时」）。收尾失败必须向上传播，让根
        // pending 留待重试；否则 inst_relate 已删、房间/空间树未清的部分成功会永久残留。
        crate::data_interface::helper::prune_roots_stale_model_rows(
            &target_root_refnos,
            &produced,
            300,
        )
        .await?;

        if db_option.gen_mesh {
            // 错误必须向上传播（不再 .expect panic）：mesh 失败会让
            // ModelRefreshPolicy::generate_roots 返回 Err，从而保留 model_update_pending
            // 根任务待重试，而不是把 async_watch 看门狗任务 panic 掉。
            process_meshes_update_db_deep(db_option, &target_root_refnos).await?;
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

        dbg!(&dbnos);
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
                    .execute_boolean_meshes(Some(db_option_arc.clone()))
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
pub async fn process_meshes_by_dbnos(dbnos: &[u32], db_option: &DbOption) -> anyhow::Result<()> {
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
    //判断有空的层级，不用去生成
    let zones = query_type_refnos_by_dbnum(&["ZONE"], dbno, Some(true), gen_history)
        .await
        .unwrap_or_default();
    if zones.is_empty() {
        return Ok(Default::default());
    }
    // let mut all_handles = FuturesUnordered::new();

    println!("gen_geos_data_by_dbnum 处理db: {}", dbno);
    let d_types = db_option_arc.debug_refno_types.clone();
    let mut gen_cata_flag = d_types.iter().any(|x| x == "CATA");
    let mut gen_loop_flag = d_types.iter().any(|x| x == "LOOP");
    let mut gen_prim_flag = d_types.iter().any(|x| x == "PRIM");
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
                .unwrap_or_default();
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
            let children = aios_core::get_children_pes(refno).await.unwrap_or_default();
            bran_comp_eles.extend(children.iter().map(|x| x.refno));
            //求出元件对应的outside bore
            branch_refnos_map.insert(refno, children);
        }

        let target_bran_reuse_cata_map: DashMap<String, CataHashRefnoKV> = {
            let map = aios_core::query_group_by_cata_hash(target_bran_hanger_refnos.as_slice())
                .await
                .unwrap_or_default();
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
            // let handle = tokio::spawn(async move {
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
            // });
            // all_handles.push(handle);
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
                .unwrap_or_default();
            map
        };

        println!("当前分段使用元件库数量: {}", cur_cate_refnos.len());
        if gen_model && gen_cata_flag && !target_single_cata_map.is_empty() {
            let sjus_map_clone = loop_sjus_map_arc.clone();
            let db_option = db_option_arc.clone();
            let sender = sender.clone();
            // let handle = tokio::spawn(async move {
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
            // });
            // all_handles.push(handle);
        }
    }

    let target_loop_owner_refnos = Arc::new(
        query_type_refnos_by_dbnum(&GNERAL_LOOP_OWNER_NOUN_NAMES, dbno, Some(true), gen_history)
            .await
            .unwrap_or_default(),
    );
    println!("当前分段使用LOOP的数量: {}", target_loop_owner_refnos.len());
    if gen_model && gen_loop_flag && !target_loop_owner_refnos.is_empty() {
        let sjus_map_clone = loop_sjus_map_arc.clone();
        let sender = sender.clone();
        let db_option = db_option_arc.clone();
        let target_loop_owner_refnos_arc = target_loop_owner_refnos.clone();
        // let handle = tokio::spawn(async move {
        loop_model::gen_loop_geos(
            db_option,
            &target_loop_owner_refnos_arc,
            sjus_map_clone,
            sender,
        )
        .await?;
        // });
        // all_handles.push(handle);
    }

    let target_prim_refnos = Arc::new(
        query_type_refnos_by_dbnum(&GNERAL_PRIM_NOUN_NAMES, dbno, None, gen_history)
            .await
            .unwrap_or_default(),
    );

    println!("当前分段使用基本体数量: {}", target_prim_refnos.len());
    //基本元件的生成
    if gen_model && gen_prim_flag && !target_prim_refnos.is_empty() {
        //基本体模型的生成
        let db_option = db_option_arc.clone();
        let sender = sender.clone();
        let target_prim_refnos_arc = target_prim_refnos.clone();
        // let hand le = tokio::spawn(async move {
        prim_model::gen_prim_geos(db_option, target_prim_refnos_arc.as_slice(), sender).await?;
        // });
        // all_handles.push(handle);
    }

    //Ok::<_, anyhow::Error>(())
    // while let Some(result) = all_handles.next().await {
    //     // 处理每个完成的 future 的结果
    // }

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
                let handle = crate::data_interface::staging::write_context::spawn_with_staged_io(
                    async move {
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
                    },
                );
                all_handles.push(handle);
            }
        }

        if gen_cata_flag && !target_single_cata_map.is_empty() {
            println!("当前分段使用独立的元件库数量: {}", use_cata_refnos.len());
            let sjus_map_clone = loop_sjus_map_arc.clone();
            let db_option = db_option_arc.clone();
            let sender = sender.clone();
            let handle =
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

        let target_prim_refnos: Vec<RefnoEnum> =
            aios_core::query_multi_deep_versioned_children_filter_inst(
                target_refnos,
                &GNERAL_PRIM_NOUN_NAMES,
                skip_exist,
            )
            .await?
            .into_iter()
            .collect();

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
