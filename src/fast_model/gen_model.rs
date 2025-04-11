use crate::data_interface::increment_record::IncrGeoUpdateLog;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::pdms_inst::{save_instance_data};
use crate::fast_model::{
    booleans_meshes_in_db, cata_model, gen_meshes_in_db, loop_model, prim_model,
    process_meshes_update_db_deep, resolve_desi_comp, shared,
};
#[cfg(feature = "gen_model")]
use aios_core::csg::manifold::ManifoldRust;
use aios_core::geometry::{PlantGeoData, ShapeInstancesData};
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::CateGeoParam::{BoxImplied, TubeImplied};
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::prim_geo::tubing::TubiSize;
use aios_core::room::room::GLOBAL_AABB_TREE;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::tool::hash_tool::hash_two_str;
use aios_core::{pdms_types::*, RefnoEnum};
use aios_core::{prim_geo::*, DBType};
use aios_core::{
    query_multi_children_refnos, query_type_refnos_by_dbnum, query_use_cate_refnos_by_dbnum, SUL_DB,
};
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
                let target_refnos = query_multi_children_refnos(&bran_refnos).await.unwrap();
                gen_meshes_in_db(db_option_clone, &target_refnos)
                    .await
                    .expect("更新bran_hanger模型数据失败");
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
                let target_refnos = query_multi_children_refnos(&chunk).await.unwrap();
                booleans_meshes_in_db(db_option_clone, &target_refnos)
                    .await
                    .expect("布尔运算bran_hanger模型数据失败");
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
///
/// # 返回值
/// * `anyhow::Result<bool>` - 返回生成结果，成功返回true，失败返回错误
pub async fn gen_all_geos_data(
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOption,
    incr_updates: Option<IncrGeoUpdateLog>,
) -> anyhow::Result<bool> {
    const CHUNK_SIZE: usize = 100;
    let is_incr_update = incr_updates.is_some();
    let has_manual_refnos = !manual_refnos.is_empty();
    let has_debug = db_option.debug_root_refnos.is_some();
    let time = Instant::now();

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
            incr_updates.clone(),
            sender.clone(),
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
                gen_geos_data_by_dbnum(dbno, db_option_arc.clone(), sender.clone()).await?;
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
pub async fn gen_geos_data(
    dbno: Option<u32>,
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOption,
    incr_updates: Option<IncrGeoUpdateLog>,
    sender: flume::Sender<ShapeInstancesData>,
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
