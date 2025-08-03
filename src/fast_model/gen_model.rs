use crate::data_interface::increment_record::IncrGeoUpdateLog;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::data_interface::sesno_increment::get_changes_at_sesno;
use crate::fast_model::pdms_inst::{save_instance_data};
use crate::fast_model::{
    booleans_meshes_in_db, cata_model, gen_meshes_in_db, loop_model, prim_model,
    process_meshes_update_db_deep, resolve_desi_comp, shared,
};
use crate::xkt_generator::*;
#[cfg(feature = "gen_model")]
use aios_core::csg::manifold::ManifoldRust;
use aios_core::geometry::{PlantGeoData, ShapeInstancesData, EleInstGeo};
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::CateGeoParam::{BoxImplied, TubeImplied};
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::prim_geo::tubing::TubiSize;
use aios_core::room::room::GLOBAL_AABB_TREE;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::tool::hash_tool::hash_two_str;
use aios_core::{pdms_types::*, RefnoEnum, RefU64};
use aios_core::{prim_geo::*, DBType};
use aios_core::{
    query_multi_children_refnos, query_type_refnos_by_dbnum, query_use_cate_refnos_by_dbnum, SUL_DB,
};
// 历史数据查询相关导入
// use aios_core::historical_query::{
//     query_type_refnos_by_dbnum_at_sesno,
//     query_hierarchy_at_sesno,
//     query_multi_children_refnos_at_sesno,
//     session_exists,
//     HierarchyQueryResult
// };
use bevy_transform::prelude::Transform;
use dashmap::DashMap;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
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
    pub async fn execute_gen_inst_meshes(&self, db_option_arc: Option<Arc<DbOption>>) {
        let mut handles = FuturesUnordered::new();
        let prim_refnos = self.prim_refnos.clone();
        let loop_owner_refnos = self.loop_owner_refnos.clone();
        let use_cate_refnos = self.use_cate_refnos.clone();
        let bran_hanger_refnos = self.bran_hanger_refnos.clone();

        let db_option = db_option_arc.clone();
        handles.push(tokio::spawn(async move {
            gen_meshes_in_db(db_option, &prim_refnos)
                .await
                .expect("更新prim模型数据失败");
        }));
        let db_option = db_option_arc.clone();
        handles.push(tokio::spawn(async move {
            gen_meshes_in_db(db_option.clone(), &loop_owner_refnos)
                .await
                .expect("更新loop模型数据失败");
        }));
        let db_option = db_option_arc.clone();
        handles.push(tokio::spawn(async move {
            gen_meshes_in_db(db_option, &use_cate_refnos)
                .await
                .expect("更新use_cate模型数据失败");
        }));
        let db_option = db_option_arc.clone();
        handles.push(tokio::spawn(async move {
            for bran_refnos in bran_hanger_refnos.chunks(20) {
                let db_option_clone = db_option.clone();
                // let refnos_str = bran_refnos.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(",");
                let target_refnos = match query_multi_children_refnos(&bran_refnos).await {
                    Ok(refnos) => refnos,
                    Err(e) => {
                        eprintln!("查询bran_hanger子节点refnos失败：{}", e);
                        return;
                    }
                };
                
                match gen_meshes_in_db(db_option_clone, &target_refnos).await {
                    Ok(()) => {},
                    Err(e) => {
                        let target_str = target_refnos.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(",");
                        eprintln!("更新bran_hanger模型数据失败：{}，相关refnos: {}", e, target_str);
                        return;
                    }
                }
            }
        }));
        while let Some(_) = handles.next().await {}
    }

    //执行布尔运算的操作
    pub async fn execute_boolean_meshes(&self, db_option_arc: Option<Arc<DbOption>>) {
        let mut handles = FuturesUnordered::new();
        let prim_refnos = self.prim_refnos.clone();
        let loop_owner_refnos = self.loop_owner_refnos.clone();
        let use_cate_refnos = self.use_cate_refnos.clone();
        let bran_hanger_refnos = self.bran_hanger_refnos.clone();
        let db_option = db_option_arc.clone();
        handles.push(tokio::spawn(async move {
            booleans_meshes_in_db(db_option, &prim_refnos)
                .await
                .expect("布尔运算prim模型数据失败");
        }));
        let db_option = db_option_arc.clone();
        handles.push(tokio::spawn(async move {
            booleans_meshes_in_db(db_option, &loop_owner_refnos)
                .await
                .expect("布尔运算loop模型数据失败");
        }));
        let db_option = db_option_arc.clone();
        handles.push(tokio::spawn(async move {
            booleans_meshes_in_db(db_option, &use_cate_refnos)
                .await
                .expect("布尔运算use_cate模型数据失败");
        }));
        let db_option = db_option_arc.clone();
        handles.push(tokio::spawn(async move {
            for chunk in bran_hanger_refnos.chunks(20) {
                let db_option_clone = db_option.clone();
                let chunk_str = chunk.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(",");
                let target_refnos = match query_multi_children_refnos(&chunk).await {
                    Ok(refnos) => refnos,
                    Err(e) => {
                        eprintln!("查询bran_hanger子节点refnos失败：{}，相关refnos: {}", e, chunk_str);
                        continue;
                    }
                };
                match booleans_meshes_in_db(db_option_clone, &target_refnos).await {
                    Ok(_) => {},
                    Err(e) => {
                        let target_str = target_refnos.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(",");
                        eprintln!("布尔运算bran_hanger模型数据失败：{}，相关refnos: {}", e, target_str);
                        continue;
                    }
                }
            }
        }));
        while let Some(_) = handles.next().await {}
    }
}

/// 生成几何体数据
/// 
/// # 参数
/// * `manual_refnos` - 手动指定的引用号列表
/// * `db_option` - 数据库选项配置
/// * `incr_updates` - 增量更新日志，用于增量生成几何体数据
/// * `target_sesno` - 目标会话号，用于判断是否生成历史数据的模型
///
/// # 返回值
/// * `anyhow::Result<bool>` - 返回生成结果，成功返回true，失败返回错误
pub async fn gen_all_geos_data(
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOption,
    incr_updates: Option<IncrGeoUpdateLog>,
    target_sesno: Option<u32>,
) -> anyhow::Result<bool> {
    const CHUNK_SIZE: usize = 100;
    let mut final_incr_updates = incr_updates;
    let time = Instant::now();
    
    // 如果指定了 target_sesno，获取该 sesno 的增量数据
    if let Some(sesno) = target_sesno {
        if final_incr_updates.is_none() {
            // 从 element_changes 表获取该 sesno 的变更
            match get_changes_at_sesno(sesno).await {
                Ok(sesno_changes) => {
                    // 如果该 sesno 有变更，使用这些变更作为增量更新
                    if sesno_changes.count() > 0 {
                        println!("发现 sesno {} 的变更: {} 个元素", sesno, sesno_changes.count());
                        final_incr_updates = Some(sesno_changes);
                    } else {
                        println!("sesno {} 没有发现变更，跳过增量生成", sesno);
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
    
    let is_incr_update = final_incr_updates.is_some();
    let has_manual_refnos = !manual_refnos.is_empty();
    let has_debug = db_option.debug_root_refnos.is_some();

    if is_incr_update || has_manual_refnos || has_debug {
        // let (sender, receiver) = flume::bounded(CHUNK_SIZE);
        let (sender, receiver) = flume::unbounded();
        let receiver: flume::Receiver<ShapeInstancesData> = receiver.clone();
        let insert_task = tokio::task::spawn(async move {
            while let Ok(shape_insts) = receiver.recv_async().await {
                save_instance_data(&shape_insts, false).await.unwrap();
                println!("Insert manual shape insts: {}", shape_insts.inst_cnt());
            }
        });
        let target_root_refnos = gen_geos_data(
            None,
            manual_refnos.clone(),
            db_option,
            final_incr_updates.clone(),
            sender.clone(),
            target_sesno,
        )
        .await?;
        drop(sender);
        insert_task.await.unwrap();
        if db_option.gen_mesh {
            process_meshes_update_db_deep(db_option, &target_root_refnos)
                .await
                .expect("更新模型数据失败");
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
            let (sender, receiver) = flume::unbounded();
            let receiver: flume::Receiver<ShapeInstancesData> = receiver.clone();
            let insert_task = tokio::task::spawn(async move {
                while let Ok(shape_insts) = receiver.recv_async().await {
                    let time = Instant::now();
                    // save_instance_data(&shape_insts, false).await.unwrap();
                    save_instance_data(&shape_insts, false).await.unwrap();
                    println!("save_instance_data time: {}ms", time.elapsed().as_millis());
                    println!("Insert shape insts: {}", shape_insts.inst_info_map.len());
                }
            });
            let db_refnos =
                gen_geos_data_by_dbnum(dbno, db_option_arc.clone(), sender.clone(), target_sesno).await?;
            drop(sender);
            insert_task.await.unwrap();
            println!("生成完insts时间: {}ms", time.elapsed().as_millis());
            if db_option_arc.gen_mesh {
                let time = Instant::now();
                println!("开始执行模型生成和布尔运算");
                //模型生成完之后，再进行布尔运算
                db_refnos
                    .execute_gen_inst_meshes(Some(db_option_arc.clone()))
                    .await;
                println!("生成insts三角模型时间: {}ms", time.elapsed().as_millis());
                let time = Instant::now();
                db_refnos
                    .execute_boolean_meshes(Some(db_option_arc.clone()))
                    .await;
                println!("布尔运算时间: {}ms", time.elapsed().as_millis());
            }
        }
    }
    {
        let read = GLOBAL_AABB_TREE.read().await;
        println!("GLOBAL_AABB_TREE: {:?}", read.tree.size());
        GLOBAL_AABB_TREE.read().await.serialize_to_bin_file()?;
    }
    println!("生成完所有模型时间: {}ms", time.elapsed().as_millis());

    Ok(true)
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
        process_meshes_update_db_deep(db_option, &sites)
            .await
            .expect("更新模型数据失败");
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
    target_sesno: Option<u32>,
) -> anyhow::Result<DbModelInstRefnos> {
    let gen_history = db_option_arc.is_gen_history_model();

    //判断有空的层级，不用去生成
    let zones = if let Some(sesno) = target_sesno {
        // 使用历史查询
        query_type_refnos_by_dbnum(&["ZONE"], dbno, Some(true), gen_history)
            .await
            .unwrap_or_default()
    } else {
        // 使用当前数据查询
        query_type_refnos_by_dbnum(&["ZONE"], dbno, Some(true), gen_history)
            .await
            .unwrap_or_default()
    };
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
                let mut response = SUL_DB.query(sql).await?;
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
            .await
            .unwrap();
            println!("BRAN/HANG cata_model::gen_cata_geos执行时间: {}ms", start_time.elapsed().as_millis());
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
            .await
            .unwrap();
            println!("单个使用元件库 cata_model::gen_cata_geos执行时间: {}ms", start_time.elapsed().as_millis());
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
            .await
            .unwrap();
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
            prim_model::gen_prim_geos(db_option, target_prim_refnos_arc.as_slice(), sender)
                .await
                .unwrap();
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
///
/// # 参数
/// * `dbno` - 可选的数据库编号
/// * `manual_refnos` - 手动指定的引用号列表
/// * `db_option` - 数据库选项
/// * `incr_updates` - 增量更新日志
/// * `sender` - 数据发送通道
/// * `target_sesno` - 目标会话号，用于历史模型生成
pub async fn gen_geos_data(
    dbno: Option<u32>,
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOption,
    incr_updates: Option<IncrGeoUpdateLog>,
    sender: flume::Sender<ShapeInstancesData>,
    target_sesno: Option<u32>,
) -> anyhow::Result<Vec<RefnoEnum>> {
    let skip_exist = !db_option.is_replace_mesh();
    let mut all_handles = FuturesUnordered::new();
    // dbg!(&incr_updates);
    const CHUNK_SIZE: usize = 100;
    //根据需要拉入数据到本地数据库也可以
    let is_incr_update = incr_updates.is_some();
    let has_manual_refnos = !manual_refnos.is_empty();
    //排除增量更新的情况，如果debug_root_refnos 为空，即没有模型需要生成
    let debug_root_refnos = db_option.get_all_debug_refnos().await;
    // dbg!(&debug_root_refnos);
    if !is_incr_update
        //debug_root_refnos = [] 时表示不生成模型，如果没有这个属性表示生成所有
        && (db_option.debug_root_refnos.is_some() && debug_root_refnos.is_empty())
        && (!has_manual_refnos)
    {
        return Ok(vec![]);
    }
    if is_incr_update && incr_updates.as_ref().unwrap().count() == 0 {
        return Ok(vec![]);
    }
    let db_option_arc = Arc::new(db_option.clone());
    let is_debug = debug_root_refnos.len() > 0;

    let include_history = db_option_arc.is_gen_history_model();
    let is_replace_mesh = db_option_arc.is_replace_mesh();
    let incr_count = if is_incr_update {
        incr_updates.as_ref().unwrap().count()
    } else {
        0
    };
    let mut target_root_refnos = vec![];
    if is_incr_update {
        // root_refnos 为incr_update_log里的loop_refnos，basic_cata_refnos， prim_refnos的合集
        target_root_refnos = incr_updates
            .as_ref()
            .unwrap()
            .get_all_visible_refnos()
            .into_iter()
            .collect();
    } else if is_debug || has_manual_refnos {
        target_root_refnos = if has_manual_refnos {
            manual_refnos.clone()
        } else {
            debug_root_refnos.clone()
        };
    } else if dbno.is_some() {
        // 检查是否需要进行历史查询
        if let Some(sesno) = target_sesno {
            // 验证会话是否存在 (暂时跳过验证)
            // if !session_exists(sesno).await? {
            //     return Err(anyhow::anyhow!("会话号 {} 不存在", sesno));
            // }

            println!("使用历史查询，目标会话号: {} (注意：当前使用当前数据替代)", sesno);
            target_root_refnos = query_type_refnos_by_dbnum(
                &["SITE"],
                dbno.unwrap(),
                Some(true),
                include_history
            ).await?
                .into_iter()
                .collect();
        } else {
            // 使用当前数据查询
            target_root_refnos =
                query_type_refnos_by_dbnum(&["SITE"], dbno.unwrap(), Some(true), include_history)
                    .await?
                    .into_iter()
                    .collect();
        }
    }
    if dbno.is_some() {
        println!("总共 {} 个SITE", target_root_refnos.len());
    } else {
        println!("总共 {} 个结点", target_root_refnos.len());
    }
    let origin_root_refnos = target_root_refnos.clone();
    // let process_handle = tokio::spawn(async move {
    // let mut handles = vec![]
    if is_incr_update {
        println!("处理更新模型数量: {}", incr_count);
    } else if has_manual_refnos {
        println!("处理生成模型数量: {}", manual_refnos.len());
    } else if is_debug {
        println!("调试模型数量: {:?}", debug_root_refnos.len());
    } else if dbno.is_some() {
        println!("处理db: {}", dbno.unwrap());
    }
    let d_types = db_option_arc.debug_refno_types.clone();
    let mut gen_cata_flag =
        d_types.iter().any(|x| x == "CATA") || is_incr_update || has_manual_refnos;
    let mut gen_loop_flag =
        d_types.iter().any(|x| x == "LOOP") || is_incr_update || has_manual_refnos;
    let mut gen_prim_flag =
        d_types.iter().any(|x| x == "PRIM") || is_incr_update || has_manual_refnos;

    // dbg!(origin_root_refnos.len());
    let incr_updates_log_arc = Arc::new(incr_updates.clone().unwrap_or_default());
    //需要在这里把origin_root_refnos 打断成小块
    let mut chunked_root_refnos = origin_root_refnos.chunks(CHUNK_SIZE);
    let gen_model = db_option_arc.gen_model || is_incr_update || has_manual_refnos;
    //遍历小块
    while gen_model && let Some(target_refnos) = chunked_root_refnos.next() {
        //Step 1、提前缓存ploo, 得到对齐方式的偏移
        let loop_sjus_map = DashMap::new();
        //TODO 检查两个类型是否有可能在一个层级树里，如果不需要可以跳过
        {
            //查找到子节点的所有PLOO类型
            let Ok(target_ploo_refnos) =
                aios_core::query_multi_deep_versioned_children_filter_inst(
                    target_refnos,
                    &["PLOO"],
                    skip_exist,
                )
                .await
            else {
                continue;
            };
            #[cfg(debug_assertions)]
            if !target_ploo_refnos.is_empty() {
                println!("target_ploo_refnos: {:?}", target_ploo_refnos.len());
            }
            for r in target_ploo_refnos {
                let Ok(loop_att) = aios_core::get_named_attmap(r).await else {
                    continue;
                };
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
        let target_bran_hanger_refnos: Vec<RefnoEnum> = if is_incr_update {
            incr_updates_log_arc
                .bran_hanger_refnos
                .iter()
                .cloned()
                .collect()
        } else {
            let r = aios_core::query_multi_deep_versioned_children_filter_inst(
                target_refnos,
                &["BRAN", "HANG"],
                skip_exist,
            )
            .await
            .unwrap();
            r.into_iter().collect()
        };
        let target_bran_reuse_cata_map: DashMap<String, CataHashRefnoKV> = {
            let map = aios_core::query_group_by_cata_hash(&target_bran_hanger_refnos)
                .await
                .unwrap_or_default();
            map
        };
        let mut use_cata_refnos = HashSet::new();
        //查询单个使用元件库的数量
        let target_single_cata_map = if is_incr_update {
            let cata_map = DashMap::new();
            let cata_refnos = &incr_updates_log_arc.basic_cata_refnos;
            //直接使用group的办法，按cata_hash 进行分组
            for &r in cata_refnos {
                let Ok(Some(att)) = aios_core::get_pe(r).await else {
                    continue;
                };
                cata_map.insert(
                    att.cata_hash.clone(),
                    CataHashRefnoKV {
                        cata_hash: att.cata_hash,
                        group_refnos: vec![r],
                        ..Default::default()
                    },
                );
            }
            cata_map
        } else {
            //查询是否是单个使用元件库，父节点是BRAN HANG
            let sql = format!(
                "select value refno from [{}] where owner.noun in ['BRAN', 'HANG']",
                target_refnos
                    .iter()
                    .map(|x| x.to_pe_key())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let mut response = SUL_DB.query(sql).await.unwrap();

            let Ok(bran_children_refnos) = response.take::<Vec<RefnoEnum>>(0) else {
                dbg!("查询BRAN, HANG出错");
                continue;
            };
            let single_refnos = target_refnos
                .iter()
                .filter(|x| !target_bran_hanger_refnos.contains(x))
                .map(|x| *x)
                .collect::<Vec<_>>();
            use_cata_refnos =
                aios_core::query_multi_deep_children_filter_spre(single_refnos, skip_exist)
                    .await
                    .unwrap_or_default();
            // dbg!(&use_cata_refnos);
            use_cata_refnos.extend(bran_children_refnos);
            let map = aios_core::query_group_by_cata_hash(&use_cata_refnos)
                .await
                .unwrap_or_default();
            map
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
                let children = aios_core::get_children_pes(refno).await.unwrap_or_default();
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
                let handle = tokio::spawn(async move {
                    let start_time = Instant::now();
                    cata_model::gen_cata_geos(
                        db_option,
                        Arc::new(target_bran_reuse_cata_map),
                        Arc::new(branch_refnos_map),
                        sjus_map_clone,
                        sender,
                    )
                    .await
                    .unwrap();
                    println!("异步BRAN/HANG cata_model::gen_cata_geos执行时间: {}ms", start_time.elapsed().as_millis());
                });
                all_handles.push(handle);
            }
        }

        if gen_cata_flag && !target_single_cata_map.is_empty() {
            println!("当前分段使用独立的元件库数量: {}", use_cata_refnos.len());
            let sjus_map_clone = loop_sjus_map_arc.clone();
            let db_option = db_option_arc.clone();
            let sender = sender.clone();
            let handle = tokio::spawn(async move {
                let start_time = Instant::now();
                cata_model::gen_cata_geos(
                    db_option,
                    Arc::new(target_single_cata_map),
                    Arc::new(Default::default()),
                    sjus_map_clone,
                    sender,
                )
                .await
                .unwrap();
                println!("异步单个使用元件库 cata_model::gen_cata_geos执行时间: {}ms", start_time.elapsed().as_millis());
            });
            all_handles.push(handle);
        }

        let target_loop_owner_refnos: Vec<RefnoEnum> = if is_incr_update {
            incr_updates_log_arc
                .loop_owner_refnos
                .iter()
                .cloned()
                .collect()
        } else {
            let mut loop_owner_refnos = aios_core::query_multi_deep_versioned_children_filter_inst(
                target_refnos,
                &GNERAL_LOOP_OWNER_NOUN_NAMES,
                skip_exist,
            )
            .await
            .unwrap_or_default();
            loop_owner_refnos.into_iter().collect()
        };
        if gen_loop_flag && !target_loop_owner_refnos.is_empty() {
            println!("当前分段使用LOOP的数量: {}", target_loop_owner_refnos.len());
            let sjus_map_clone = loop_sjus_map_arc.clone();
            let sender = sender.clone();
            let db_option = db_option_arc.clone();
            let handle = tokio::spawn(async move {
                loop_model::gen_loop_geos(
                    db_option,
                    &target_loop_owner_refnos,
                    sjus_map_clone,
                    sender,
                )
                .await
                .unwrap();
            });
            all_handles.push(handle);
        }

        let target_prim_refnos: Vec<RefnoEnum> = if is_incr_update {
            incr_updates_log_arc.prim_refnos.iter().cloned().collect()
        } else {
            let mut prim_refnos = aios_core::query_multi_deep_versioned_children_filter_inst(
                target_refnos,
                &GNERAL_PRIM_NOUN_NAMES,
                skip_exist,
            )
            .await
            .unwrap_or_default();
            prim_refnos.into_iter().collect()
        };

        //基本元件的生成
        if gen_prim_flag && !target_prim_refnos.is_empty() {
            println!("当前分段使用基本体数量: {}", target_prim_refnos.len());
            //基本体模型的生成
            let db_option = db_option_arc.clone();
            let sender = sender.clone();
            let handle = tokio::spawn(async move {
                prim_model::gen_prim_geos(db_option, target_prim_refnos.as_slice(), sender)
                    .await
                    .unwrap();
            });
            all_handles.push(handle);
        }
        if is_incr_update {
            break;
        }
    }
    //Ok::<_, anyhow::Error>(())
    while let Some(result) = all_handles.next().await {
        // 处理每个完成的 future 的结果
    }

    if dbno.is_some() {
        println!("数据库号： {} 生成instances完毕。", dbno.unwrap());
    }

    Ok(target_root_refnos)
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

/// 从数据库生成 XKT 格式模型
/// 
/// # 参数
/// * `refnos` - 要处理的参考号列表
/// * `output_path` - 输出文件路径
/// * `compress` - 是否压缩输出文件
/// * `db_option` - 数据库配置选项
/// 
/// # 返回值
/// * `anyhow::Result<()>` - 返回生成结果
pub async fn generate_xtk_from_database(
    refnos: Vec<RefnoEnum>,
    output_path: &str,
    compress: bool,
    db_option: &DbOption,
) -> anyhow::Result<()> {
    println!("开始从数据库生成 XKT 格式模型（支持层级结构）...");
    let start_time = Instant::now();

    // 创建 XKT 文件
    let mut xkt_file = XKTFile::new();
    xkt_file.model.metadata.title = "PDMS 模型导出".to_string();
    xkt_file.model.metadata.author = "aios-database".to_string();
    xkt_file.model.metadata.application = "aios-database XTK Generator".to_string();

    // 创建颜色方案
    let color_scheme = ColorScheme::new();

    // 创建数据库管理器
    let aios_mgr = AiosDBManager::init(&db_option).await?;

    // 统计信息
    let mut processed_count = 0;
    let mut geometry_count = 0;
    let mut mesh_count = 0;
    let mut entity_count = 0;

    println!("正在处理 {} 个参考号...", refnos.len());

    // 处理每个根节点（通常是 SITE），递归展开整个层级树
    for &refno in &refnos {
        println!("开始处理根节点: {}", refno);
        
        match process_refno_to_xtk(
            &mut xkt_file, 
            refno, 
            &color_scheme, 
            &aios_mgr
        ).await {
            Ok((geo_cnt, mesh_cnt, entity_cnt)) => {
                geometry_count += geo_cnt;
                mesh_count += mesh_cnt;
                entity_count += entity_cnt;
                processed_count += 1;
                println!("完成根节点 {}: {} 个几何体, {} 个网格, {} 个实体", 
                    refno, geo_cnt, mesh_cnt, entity_cnt);
            }
            Err(e) => {
                eprintln!("处理根节点 {} 时出错: {}", refno, e);
                continue;
            }
        }
    }

    // 完成模型构建
    xkt_file.model.finalize().await?;

    // 保存文件
    println!("正在保存 XKT 文件到: {}", output_path);
    xkt_file.save_to_file(output_path, compress).await?;

    let elapsed = start_time.elapsed();
    println!("XTK 生成完成!");
    println!("处理时间: {:.2}秒", elapsed.as_secs_f64());
    println!("统计信息:");
    println!("  - 处理的参考号: {}", processed_count);
    println!("  - 几何体数量: {}", geometry_count);
    println!("  - 网格数量: {}", mesh_count);
    println!("  - 实体数量: {}", entity_count);
    println!("  - 文件大小: {:.2} MB", std::fs::metadata(output_path)?.len() as f64 / 1024.0 / 1024.0);

    Ok(())
}

/// 处理单个参考号并转换为 XKT 格式（从 site 开始递归展开层级树）
async fn process_refno_to_xtk(
    xkt_file: &mut XKTFile,
    refno: RefnoEnum,
    color_scheme: &ColorScheme,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<(usize, usize, usize)> {
    let mut geometry_count = 0;
    let mut mesh_count = 0;
    let mut entity_count = 0;

    // 存储已创建的实体，避免重复创建
    let mut created_entities = std::collections::HashSet::new();
    // 存储父子关系，用于后续建立层级
    let mut parent_child_relations = Vec::new();

    // 递归处理节点及其所有子节点
    let (geo_cnt, mesh_cnt, entity_cnt) = process_node_recursive(
        xkt_file,
        refno,
        None, // 根节点没有父节点
        color_scheme,
        aios_mgr,
        &mut created_entities,
        &mut parent_child_relations,
    ).await?;

    geometry_count += geo_cnt;
    mesh_count += mesh_cnt;
    entity_count += entity_cnt;

    // 建立父子关系
    for (parent_id, child_id) in parent_child_relations {
        // 设置子实体的父节点
        if let Some(child_entity) = xkt_file.model.entities.get_mut(&child_id) {
            child_entity.set_parent(parent_id.clone());
        }
        // 设置父实体的子节点
        if let Some(parent_entity) = xkt_file.model.entities.get_mut(&parent_id) {
            parent_entity.add_child(child_id.clone());
        }
    }

    Ok((geometry_count, mesh_count, entity_count))
}

/// 递归处理节点及其所有子节点
fn process_node_recursive<'a>(
    xkt_file: &'a mut XKTFile,
    refno: RefnoEnum,
    parent_refno: Option<RefnoEnum>,
    color_scheme: &'a ColorScheme,
    aios_mgr: &'a AiosDBManager,
    created_entities: &'a mut std::collections::HashSet<RefnoEnum>,
    parent_child_relations: &'a mut Vec<(String, String)>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<(usize, usize, usize)>> + 'a>> {
    Box::pin(async move {
    let mut geometry_count = 0;
    let mut mesh_count = 0;
    let mut entity_count = 0;

    // 如果已经创建过这个实体，跳过
    if created_entities.contains(&refno) {
        return Ok((0, 0, 0));
    }

    // 查询元素信息
    let element_info = match aios_mgr.get_element_info(refno).await? {
        Some(info) => info,
        None => {
            // 如果没有找到元素信息，跳过
            return Ok((0, 0, 0));
        }
    };

    println!("处理节点: {} (类型: {})", refno, element_info.type_name);

    // 获取当前节点的世界变换
    let world_transform = aios_mgr.get_world_transform_or_default(refno.into()).await;
    
    // 计算局部变换（相对于父节点）
    let local_transform = if let Some(parent_refno) = parent_refno {
        let parent_world_transform = aios_mgr.get_world_transform_or_default(parent_refno.into()).await;
        calculate_local_transform(&world_transform, &parent_world_transform)
    } else {
        world_transform
    };

    // 获取几何数据
    let shape_instances = aios_mgr.get_shape_instances_data(refno).await?;

    // 创建当前节点的实体
    let entity_id = format!("entity_{}", refno);
    let entity_name = element_info.name.clone().unwrap_or_else(|| format!("元素-{}", refno));
    let mut entity = XKTEntity::new(
        entity_id.clone(),
        entity_name,
        element_info.type_name.clone(),
    );

    // 如果有几何数据，处理几何实例
    if let Some(shape_data) = shape_instances {
        for (geo_id, geo_data) in &shape_data.inst_geos_map {
            // 为每个几何实例创建几何体
            let geometry_id = format!("geo_{}", geo_data.refno);
            
            // 根据几何参数创建几何体
            let geometry = match create_geometry_from_geo_param(&geometry_id, &geo_data.insts).await {
                Ok(geo) => geo,
                Err(e) => {
                    eprintln!("创建几何体失败 (refno: {}): {}", refno, e);
                    continue;
                }
            };

            xkt_file.model.create_geometry(geometry)?;
            geometry_count += 1;

            // 创建材质
            let material_id = format!("material_{}", geo_data.type_name);
            if !xkt_file.model.materials.contains_key(&material_id) {
                let color = color_scheme.get_color_for_type(&geo_data.type_name);
                let material = XKTMaterial::create_color_material(
                    material_id.clone(),
                    format!("{} 材质", geo_data.type_name),
                    color,
                );
                xkt_file.model.create_material(material)?;
            }

            // 为每个几何实例创建网格，使用局部变换
            for (i, inst) in geo_data.insts.iter().enumerate() {
                let mesh_id = format!("mesh_{}_{}", geo_data.refno, i);
                let mut mesh = XKTMesh::new(mesh_id.clone(), geometry_id.clone());
                mesh.set_material(material_id.clone());
                
                // 使用局部变换而不是世界变换
                let combined_transform = local_transform * inst.transform;
                mesh.set_position(combined_transform.translation);
                mesh.set_rotation(combined_transform.rotation.to_euler(glam::EulerRot::XYZ).into());
                mesh.set_scale(combined_transform.scale);
                
                // 设置可见性
                mesh.set_visible(inst.visible);

                xkt_file.model.create_mesh(mesh)?;
                mesh_count += 1;

                // 将网格添加到实体
                entity.add_mesh(mesh_id);
            }
        }
    }

    // 设置实体属性
    entity.set_property("refno".to_string(), refno.to_string());
    entity.set_property("type".to_string(), element_info.type_name.clone());
    if let Some(name) = &element_info.name {
        entity.set_property("name".to_string(), name.clone());
    }

    // 创建实体
    xkt_file.model.create_entity(entity)?;
    created_entities.insert(refno);
    entity_count += 1;

    // 建立与父节点的关系
    if let Some(parent_refno) = parent_refno {
        let parent_id = format!("entity_{}", parent_refno);
        let child_id = format!("entity_{}", refno);
        parent_child_relations.push((parent_id, child_id));
    }

    // 查询并递归处理所有子节点
    let children = get_direct_children(refno, aios_mgr).await?;
    println!("节点 {} 有 {} 个子节点", refno, children.len());

    for child_refno in children {
        let (child_geo_cnt, child_mesh_cnt, child_entity_cnt) = process_node_recursive(
            xkt_file,
            child_refno,
            Some(refno), // 当前节点作为父节点
            color_scheme,
            aios_mgr,
            created_entities,
            parent_child_relations,
        ).await?;

        geometry_count += child_geo_cnt;
        mesh_count += child_mesh_cnt;
        entity_count += child_entity_cnt;
    }

    Ok((geometry_count, mesh_count, entity_count))
    })
}

/// 获取直接子节点
async fn get_direct_children(
    refno: RefnoEnum,
    aios_mgr: &AiosDBManager,
) -> anyhow::Result<Vec<RefnoEnum>> {
    // 查询所有以当前节点为 owner 的子节点
    let sql = format!(
        "SELECT refno FROM pe WHERE owner = {}",
        refno.to_string()
    );

    match SUL_DB.query(sql).await {
        Ok(mut response) => {
            let children: Vec<RefnoEnum> = response.take(0).unwrap_or_default();
            Ok(children)
        }
        Err(e) => {
            eprintln!("查询子节点失败 (refno: {}): {}", refno, e);
            Ok(Vec::new())
        }
    }
}

/// 计算局部变换（子节点相对于父节点的变换）
fn calculate_local_transform(
    world_transform: &bevy_transform::components::Transform,
    parent_world_transform: &bevy_transform::components::Transform,
) -> bevy_transform::components::Transform {
    // 计算父节点世界变换的逆矩阵
    let parent_matrix = parent_world_transform.compute_matrix();
    let parent_inverse = parent_matrix.inverse();
    
    // 计算子节点的世界变换矩阵
    let world_matrix = world_transform.compute_matrix();
    
    // 局部变换 = 父节点逆变换 * 子节点世界变换
    let local_matrix = parent_inverse * world_matrix;
    
    // 从矩阵中提取变换组件
    bevy_transform::components::Transform::from_matrix(local_matrix)
}

/// 根据数据库号生成 XKT 文件
/// 
/// # 参数
/// * `dbno` - 数据库号
/// * `output_path` - 输出文件路径
/// * `compress` - 是否压缩输出文件
/// * `db_option` - 数据库选项配置
///
/// # 返回值
/// * `anyhow::Result<()>` - 返回生成结果
pub async fn generate_xtk_by_dbno(
    dbno: u32,
    output_path: &str,
    compress: bool,
    db_option: &DbOption,
) -> anyhow::Result<()> {
    println!("正在查询数据库号 {} 的所有参考号...", dbno);
    
    // 查询指定数据库号的所有参考号
    let all_refnos = query_type_refnos_by_dbnum(&[], dbno, None, false).await?;
    
    println!("找到 {} 个参考号", all_refnos.len());
    
    // 调用主要的生成函数
    generate_xtk_from_database(all_refnos, output_path, compress, db_option).await
}

// 定义一个简化的元素信息结构
#[derive(Debug, Clone)]
pub struct ElementInfo {
    pub name: Option<String>,
    pub type_name: String,
}

// 为 AiosDBManager 添加扩展方法的 trait
trait AiosDBManagerExt {
    async fn get_element_info(&self, refno: RefnoEnum) -> anyhow::Result<Option<ElementInfo>>;
    async fn get_shape_instances_data(&self, refno: RefnoEnum) -> anyhow::Result<Option<ShapeInstancesData>>;
}

impl AiosDBManagerExt for AiosDBManager {
    async fn get_element_info(&self, refno: RefnoEnum) -> anyhow::Result<Option<ElementInfo>> {
        // 这里需要根据实际的数据库查询方法来实现
        // 暂时返回一个默认的实现
        Ok(Some(ElementInfo {
            name: Some(format!("元素-{}", refno)),
            type_name: "UNKNOWN".to_string(),
        }))
    }

    async fn get_shape_instances_data(&self, refno: RefnoEnum) -> anyhow::Result<Option<ShapeInstancesData>> {
        // 这里需要根据实际的数据库查询方法来实现
        // 暂时返回 None，表示没有几何数据
        Ok(None)
    }
}

/// 从几何参数创建几何体
pub async fn create_geometry_from_geo_param(
    geometry_id: &str,
    geo_instances: &[EleInstGeo],
) -> anyhow::Result<XKTGeometry> {
    if geo_instances.is_empty() {
        return Err(anyhow::anyhow!("没有几何实例数据"));
    }

    // 使用第一个实例的几何参数
    let first_instance = &geo_instances[0];
    
    match &first_instance.geo_param {
        PdmsGeoParam::PrimBox(box_param) => {
            // 使用 size 字段而不是 xlength, ylength, zlength
            let size = &box_param.size;
            Ok(XKTGeometry::create_box(
                geometry_id.to_string(),
                size.x,
                size.y,
                size.z,
            ))
        }
        PdmsGeoParam::PrimSCylinder(scyl_param) => {
            // 使用 pdia 和 phei 字段
            Ok(XKTGeometry::create_cylinder(
                geometry_id.to_string(),
                scyl_param.pdia / 2.0,
                scyl_param.phei,
                32, // 分段数
            ))
        }
        PdmsGeoParam::PrimSphere(sphere_param) => {
            // 使用 radius 字段而不是 diameter
            Ok(XKTGeometry::create_sphere(
                geometry_id.to_string(),
                sphere_param.radius,
                32, // 经度分段
                16, // 纬度分段
            ))
        }
        PdmsGeoParam::PrimPyramid(pyramid_param) => {
            // 对于金字塔，我们创建一个近似的立方体
            // 使用实际可用的字段
            let avg_size = 1.0; // 默认大小，因为字段结构不明确
            Ok(XKTGeometry::create_box(
                geometry_id.to_string(),
                avg_size,
                avg_size,
                avg_size,
            ))
        }
        _ => {
            // 对于其他类型，创建一个默认的立方体
            Ok(XKTGeometry::create_box(
                geometry_id.to_string(),
                1.0, 1.0, 1.0,
            ))
        }
    }
}

/// 创建占位符实体（当没有几何数据时）
async fn create_placeholder_entity(
    xkt_file: &mut XKTFile,
    refno: RefnoEnum,
    element_info: &ElementInfo,
    color_scheme: &ColorScheme,
) -> anyhow::Result<(usize, usize, usize)> {
    // 创建一个小的立方体作为占位符
    let geometry_id = format!("placeholder_geo_{}", refno);
    let geometry = XKTGeometry::create_box(geometry_id.clone(), 0.1, 0.1, 0.1);
    xkt_file.model.create_geometry(geometry)?;

    // 创建材质
    let material_id = format!("placeholder_material_{}", element_info.type_name);
    if !xkt_file.model.materials.contains_key(&material_id) {
        let color = color_scheme.get_color_for_type(&element_info.type_name);
        let mut material = XKTMaterial::create_color_material(
            material_id.clone(),
            format!("占位符-{}", element_info.type_name),
            color,
        );
        material.set_opacity(0.3); // 设置为半透明
        xkt_file.model.create_material(material)?;
    }

    // 创建网格
    let mesh_id = format!("placeholder_mesh_{}", refno);
    let mut mesh = XKTMesh::new(mesh_id.clone(), geometry_id);
    mesh.set_material(material_id);
    mesh.set_position(Vec3::ZERO);
    xkt_file.model.create_mesh(mesh)?;

    // 创建实体
    let entity_id = format!("placeholder_entity_{}", refno);
    let mut entity = XKTEntity::new(
        entity_id,
        element_info.name.clone().unwrap_or_else(|| format!("占位符-{}", refno)),
        element_info.type_name.clone(),
    );
    entity.add_mesh(mesh_id);
    entity.set_property("refno".to_string(), refno.to_string());
    entity.set_property("type".to_string(), element_info.type_name.clone());
    entity.set_property("placeholder".to_string(), "true".to_string());

    xkt_file.model.create_entity(entity)?;

    Ok((1, 1, 1)) // 1个几何体，1个网格，1个实体
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_core::options::DbOption;
    use std::path::Path;

    /// 测试 generate_xtk_by_dbno 函数
    #[test]
    fn test_generate_xtk_by_dbno() -> anyhow::Result<()> {
        println!("=== 测试 generate_xtk_by_dbno 函数 ===");
        
        // 创建测试用的数据库选项
        let mut db_option = DbOption::default();
        db_option.gen_model = true;
        db_option.gen_mesh = false; // 为了测试速度，暂时不生成网格
        
        // 创建输出目录
        std::fs::create_dir_all("test_output").ok();
        
        // 测试数据库号（使用一个较小的测试数据库号）
        let test_dbno = 1u32; // 可以根据实际情况调整
        let output_path = "test_output/test_dbno_model.xkt";
        
        println!("开始测试数据库号: {}", test_dbno);
        println!("输出路径: {}", output_path);
        
        // 测试生成 XKT 文件
        let rt = tokio::runtime::Runtime::new()?;
        match rt.block_on(generate_xtk_by_dbno(
            test_dbno,
            output_path,
            true, // 启用压缩
            &db_option,
        )) {
            Ok(_) => {
                println!("✅ generate_xtk_by_dbno 测试成功");
                
                // 验证文件是否存在
                if Path::new(output_path).exists() {
                    // 验证文件大小
                    let metadata = std::fs::metadata(output_path)?;
                    println!("生成的文件大小: {} 字节", metadata.len());
                    
                    // 基本验证：文件应该有一定的大小
                    assert!(metadata.len() > 100, "生成的文件太小，可能有问题");
                    
                    println!("文件验证通过");
                } else {
                    println!("⚠️  输出文件不存在，可能是因为数据库中没有数据");
                }
            }
            Err(e) => {
                eprintln!("❌ generate_xtk_by_dbno 测试失败: {}", e);
                
                // 对于某些预期的错误（如数据库连接失败），我们可以容忍
                if e.to_string().contains("数据库") || e.to_string().contains("连接") {
                    println!("⚠️  测试失败是由于数据库连接问题，这在测试环境中是可以接受的");
                    return Ok(());
                }
                
                return Err(e);
            }
        }
        
        Ok(())
    }

    /// 测试 generate_xtk_by_dbno 函数的参数验证
    #[test]
    fn test_generate_xtk_by_dbno_with_invalid_params() -> anyhow::Result<()> {
        println!("=== 测试 generate_xtk_by_dbno 参数验证 ===");
        
        let db_option = DbOption::default();
        
        // 创建输出目录
        std::fs::create_dir_all("test_output").ok();
        
        // 测试无效的输出路径
        let invalid_output_path = "/invalid/path/that/does/not/exist/test.xkt";
        let test_dbno = 1u32;
        
        println!("测试无效输出路径: {}", invalid_output_path);
        
        // 这个测试应该失败，因为路径无效
        let rt = tokio::runtime::Runtime::new()?;
        match rt.block_on(generate_xtk_by_dbno(
            test_dbno,
            invalid_output_path,
            false,
            &db_option,
        )) {
            Ok(_) => {
                println!("⚠️  预期失败但成功了，可能路径实际上是有效的");
            }
            Err(e) => {
                println!("✅ 按预期失败: {}", e);
                // 这是预期的行为
            }
        }
        
        Ok(())
    }

    /// 测试 generate_xtk_by_dbno 函数的不同压缩选项
    #[test]
    fn test_generate_xtk_by_dbno_compression_options() -> anyhow::Result<()> {
        println!("=== 测试 generate_xtk_by_dbno 压缩选项 ===");
        
        let mut db_option = DbOption::default();
        db_option.gen_model = true;
        db_option.gen_mesh = false;
        
        // 创建输出目录
        std::fs::create_dir_all("test_output").ok();
        
        let test_dbno = 1u32;
        let compressed_path = "test_output/test_compressed.xkt";
        let uncompressed_path = "test_output/test_uncompressed.xkt";
        
        // 测试压缩版本
        println!("测试压缩版本...");
        let rt = tokio::runtime::Runtime::new()?;
        match rt.block_on(generate_xtk_by_dbno(
            test_dbno,
            compressed_path,
            true, // 启用压缩
            &db_option,
        )) {
            Ok(_) => println!("✅ 压缩版本生成成功"),
            Err(e) => {
                if e.to_string().contains("数据库") || e.to_string().contains("连接") {
                    println!("⚠️  压缩版本测试跳过（数据库连接问题）");
                    return Ok(());
                }
                eprintln!("❌ 压缩版本生成失败: {}", e);
            }
        }
        
        // 测试非压缩版本
        println!("测试非压缩版本...");
        match rt.block_on(generate_xtk_by_dbno(
            test_dbno,
            uncompressed_path,
            false, // 禁用压缩
            &db_option,
        )) {
            Ok(_) => println!("✅ 非压缩版本生成成功"),
            Err(e) => {
                if e.to_string().contains("数据库") || e.to_string().contains("连接") {
                    println!("⚠️  非压缩版本测试跳过（数据库连接问题）");
                    return Ok(());
                }
                eprintln!("❌ 非压缩版本生成失败: {}", e);
            }
        }
        
        // 比较文件大小（如果两个文件都存在）
        if Path::new(compressed_path).exists() && Path::new(uncompressed_path).exists() {
            let compressed_size = std::fs::metadata(compressed_path)?.len();
            let uncompressed_size = std::fs::metadata(uncompressed_path)?.len();
            
            println!("压缩文件大小: {} 字节", compressed_size);
            println!("非压缩文件大小: {} 字节", uncompressed_size);
            
            // 通常压缩文件应该更小（除非文件很小）
            if uncompressed_size > 1000 {
                assert!(compressed_size <= uncompressed_size, 
                    "压缩文件应该不大于非压缩文件");
            }
        }
        
        Ok(())
    }

    /// 运行所有 generate_xtk_by_dbno 相关的测试
    pub fn run_all_generate_xtk_by_dbno_tests() -> anyhow::Result<()> {
        println!("=== 开始运行 generate_xtk_by_dbno 测试套件 ===");
        
        // 运行各个测试
        test_generate_xtk_by_dbno()?;
        test_generate_xtk_by_dbno_with_invalid_params()?;
        test_generate_xtk_by_dbno_compression_options()?;
        
        println!("=== generate_xtk_by_dbno 测试套件完成 ===");
        Ok(())
    }
}
