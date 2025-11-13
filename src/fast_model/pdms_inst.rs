use std::collections::{HashMap, hash_map::Entry};

use aios_core::geometry::ShapeInstancesData;
use aios_core::pdms_types::*;
use aios_core::rs_surreal::delete_inst_relate_cascade;
use aios_core::types::*;
use aios_core::{SUL_DB, SurrealQueryExt, get_db_option};
use bevy_transform::prelude::Transform;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use itertools::Itertools;
use rkyv::vec;
use tokio::task::JoinHandle;

use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::debug_model_debug;
// use crate::fast_model::EXIST_MESH_GEOS;

///保存instance 数据到数据库（并行优化版本）
pub async fn save_instance_data(
    inst_mgr: &ShapeInstancesData,
    replace_exist: bool,
) -> anyhow::Result<()> {
    let mut aabb_map: HashMap<u64, String> = HashMap::new();
    let mut transform_map: HashMap<u64, String> = HashMap::new();
    //标识单位矩阵
    transform_map.insert(0, serde_json::to_string(&Transform::IDENTITY).unwrap());
    let mut param_map = HashMap::new();
    let mut vec3_map: HashMap<u64, String> = HashMap::new();
    let test_refno = get_db_option().get_test_refno();

    let chunk_size = 300;

    // 创建一个任务集合来管理并发操作
    let mut db_futures = FuturesUnordered::new();

    //把delete 提前，因为后面的插入都是异步的执行
    if replace_exist {
        let refnos: Vec<RefnoEnum> = inst_mgr.inst_info_map.keys().copied().collect();
        delete_inst_relate_cascade(&refnos, chunk_size).await?;
    }

    let keys = inst_mgr.inst_geos_map.keys().collect::<Vec<_>>();
    let mut inst_geo_vec = vec![];
    let mut geo_relate_vec = vec![];

    // 准备inst_geo和geo_relate数据
    for k in keys {
        let v = inst_mgr.inst_geos_map.get(k).unwrap();
        for inst in &v.insts {
            if inst.transform.translation.is_nan()
                || inst.transform.rotation.is_nan()
                || inst.transform.scale.is_nan()
            {
                dbg!(&inst);
                continue;
            }
            let transform_hash = gen_bytes_hash(&inst.transform);
            if !transform_map.contains_key(&transform_hash) {
                transform_map.insert(
                    transform_hash,
                    serde_json::to_string(&inst.transform).unwrap(),
                );
            }
            let param_hash = gen_bytes_hash(&inst.geo_param);
            if !param_map.contains_key(&param_hash) {
                param_map.insert(param_hash, serde_json::to_string(&inst.geo_param).unwrap());
            }
            let key_pts = inst.geo_param.key_points();
            let mut pt_hashes = vec![];
            for k in key_pts {
                let pts_hash = k.gen_hash();
                pt_hashes.push(format!("vec3:⟨{}⟩", pts_hash));
                if !vec3_map.contains_key(&pts_hash) {
                    vec3_map.insert(pts_hash, serde_json::to_string(&k).unwrap());
                }
            }
            //还需要加入geo_param的指向，param 是否填原始参数？ param=param:{}
            //使用cata_key -> inst_geos
            let cat_negs_str = if !inst.cata_neg_refnos.is_empty() {
                format!(
                    ", cata_neg: [{}]",
                    inst.cata_neg_refnos.iter().map(|x| x.to_pe_key()).join(",")
                )
            } else {
                "".to_string()
            };
            //如果是replace, 直接这里需要先删除之前的sql语句
            let mut relate_json = format!(
                r#"in: inst_info:⟨{0}⟩, out: inst_geo:⟨{1}⟩, trans: trans:⟨{2}⟩, geom_refno: pe:{3}, pts: [{4}], geo_type: '{5}', visible: {6} {7}"#,
                v.id(),
                inst.geo_hash,
                transform_hash,
                inst.refno,
                pt_hashes.join(","),
                inst.geo_type.to_string(),
                inst.visible,
                cat_negs_str
            );
            //将 string 转成一个 hash id
            let id = gen_bytes_hash(&relate_json);
            let final_json = format!("{{ {relate_json}, id: '{id}' }}");
            geo_relate_vec.push(final_json);
            //保存 unit shape 的几何参数
            inst_geo_vec.push(inst.gen_unit_geo_sur_json());
        }
    }

    // 并发保存inst_geo数据
    if !inst_geo_vec.is_empty() {
        for chunk in inst_geo_vec.chunks(chunk_size) {
            let sql_string = format!(
                "insert ignore into {} [{}];",
                stringify!(inst_geo),
                chunk.join(",")
            );
            let db = SUL_DB.clone();
            let future = tokio::spawn(async move { db.query(sql_string).await });
            db_futures.push(future);
        }
    }

    // 并发保存geo_relate数据
    if !geo_relate_vec.is_empty() {
        for chunk in geo_relate_vec.chunks(chunk_size) {
            let sql = format!("INSERT RELATION INTO geo_relate [{}];", chunk.join(","));
            let db = SUL_DB.clone();
            let future = tokio::spawn(async move { db.query(sql).await });
            db_futures.push(future);
        }
    }

    // 处理tubi数据
    let keys = inst_mgr.inst_tubi_map.keys().collect::<Vec<_>>();
    for chunk in keys.chunks(chunk_size) {
        for &k in chunk {
            let v = inst_mgr.inst_tubi_map.get(k).unwrap();
            //更新aabb 和 transform，保存relate已经在别的地方加了，这里后面需要重构
            let aabb = v.aabb.unwrap();
            let aabb_hash = gen_bytes_hash(&aabb);
            let transform_hash = gen_bytes_hash(&v.world_transform);
            if !aabb_map.contains_key(&aabb_hash) {
                aabb_map.insert(aabb_hash, serde_json::to_string(&aabb).unwrap());
            }
            if !transform_map.contains_key(&transform_hash) {
                transform_map.insert(
                    transform_hash,
                    serde_json::to_string(&v.world_transform).unwrap(),
                );
            }
        }
    }

    // 处理负关系数据并并发保存
    if !inst_mgr.neg_relate_map.is_empty() {
        let mut neg_relate_vec = vec![];
        for (k, refnos) in &inst_mgr.neg_relate_map {
            for (indx, r) in refnos.into_iter().enumerate() {
                neg_relate_vec.push(format!(
                    "{{ in: {}, id: [{}, {indx}], out: {} }}",
                    r.to_pe_key(),
                    r.to_string(),
                    k.to_pe_key(),
                ));
            }
        }
        if !neg_relate_vec.is_empty() {
            for chunk in neg_relate_vec.chunks(chunk_size) {
                let neg_relate_sql =
                    format!("INSERT RELATION INTO neg_relate [{}];", chunk.join(","));
                let db = SUL_DB.clone();
                let future = tokio::spawn(async move { db.query(neg_relate_sql).await });
                db_futures.push(future);
            }
        }
    }

    // 处理ngmr负关系数据并并发保存
    if !inst_mgr.ngmr_neg_relate_map.is_empty() {
        let mut ngmr_relate_vec = vec![];
        for (k, refnos) in &inst_mgr.ngmr_neg_relate_map {
            let kpe = k.to_pe_key();
            for (ele_refno, ngmr_geom_refno) in refnos {
                let ele_pe = ele_refno.to_pe_key();
                let ngmr_pe = ngmr_geom_refno.to_pe_key();
                ngmr_relate_vec.push(format!(
                    "{{ in: {0}, id: [{0}, {1}, {2}], out: {1}, ngmr: {2}}}",
                    ele_pe, kpe, ngmr_pe
                ));
            }
        }
        if !ngmr_relate_vec.is_empty() {
            for chunk in ngmr_relate_vec.chunks(chunk_size) {
                let ngmr_relate_sql =
                    format!("INSERT RELATION INTO ngmr_relate [{}];", chunk.join(","));
                let db = SUL_DB.clone();
                let future = tokio::spawn(async move { db.query(ngmr_relate_sql).await });
                db_futures.push(future);
            }
        }
    }

    // 处理inst_info数据
    let keys = inst_mgr.inst_info_map.keys().collect::<Vec<_>>();
    let mut inst_info_vec = vec![];
    let mut inst_relate_vec = vec![];

    for k in keys.clone() {
        let v = inst_mgr.inst_info_map.get(k).unwrap();
        if v.world_transform.translation.is_nan()
            || v.world_transform.rotation.is_nan()
            || v.world_transform.scale.is_nan()
        {
            continue;
        }
        inst_info_vec.push(v.gen_sur_json(&mut vec3_map));

        let transform_hash = gen_bytes_hash(&v.world_transform);
        if !transform_map.contains_key(&transform_hash) {
            transform_map.insert(
                transform_hash,
                serde_json::to_string(&v.world_transform).unwrap(),
            );
        }

        // 获取所属参考号和类型
        let (belong_refno, belong_type) = if let Some(owner_refno) = find_owner_refno(k) {
            (owner_refno.to_string(), get_owner_type(&owner_refno).to_string())
        } else {
            ("".to_string(), "".to_string())
        };

        let relate_sql = format!(
            "{{id: {0}, in: {1}, out: inst_info:⟨{2}⟩, world_trans: trans:⟨{3}⟩, generic: '{4}', zone_refno: fn::find_ancestor_type({1}, 'ZONE'), dt: fn::ses_date({1}), has_cata_neg: {5}, solid: {6}, belong_refno: '{7}', belong_type: '{8}'}}",
            k.to_inst_relate_key(),
            k.to_pe_key(),
            v.id_str(),
            transform_hash,
            v.generic_type.to_string(),
            v.has_cata_neg,
            v.is_solid,
            belong_refno,
            belong_type,
        );
        if let Some(t_refno) = test_refno {
            if *k == t_refno.into() {
                dbg!(v);
                println!("inst relate sql: {}", &relate_sql);
            }
        }
        inst_relate_vec.push(relate_sql);
    }

    if !inst_relate_vec.is_empty() {
        for chunk in inst_relate_vec.chunks(chunk_size) {
            let inst_relate_sql =
                format!("INSERT RELATION INTO inst_relate [{}];", chunk.join(","));
            let db = SUL_DB.clone();
            let future = tokio::spawn(async move { db.query(inst_relate_sql).await });
            db_futures.push(future);
        }

        // 更新PE表的has_inst字段，标记哪些元素有几何体
        for chunk in keys.chunks(chunk_size) {
            let pe_keys = chunk
                .iter()
                .map(|k| k.to_pe_key())
                .collect::<Vec<_>>()
                .join(",");
            let update_pe_sql = format!("UPDATE [{}] SET has_inst = true;", pe_keys);
            let db = SUL_DB.clone();
            let future = tokio::spawn(async move { db.query(update_pe_sql).await });
            db_futures.push(future);
        }
    }

    // 并发保存inst_info数据
    if !inst_info_vec.is_empty() {
        for chunk in inst_info_vec.chunks(chunk_size) {
            let sql_string = format!(
                "insert ignore into {} [{}];",
                stringify!(inst_info),
                chunk.join(",")
            );
            let db = SUL_DB.clone();
            let future = tokio::spawn(async move { db.query(sql_string).await });
            db_futures.push(future);
        }
    }

    // 并发保存aabb数据
    if !aabb_map.is_empty() {
        let keys = aabb_map.keys().collect::<Vec<_>>();
        for chunk in keys.chunks(chunk_size) {
            let mut jsons = vec![];
            for &&k in chunk {
                let v = aabb_map.get(&k).unwrap();
                let json = format!("{{'id':aabb:⟨{}⟩, 'd':{}}}", k, v);
                jsons.push(json);
            }
            let sql = format!("INSERT IGNORE INTO aabb [{}];", jsons.join(","));
            let db = SUL_DB.clone();
            let future = tokio::spawn(async move { db.query(sql).await });
            db_futures.push(future);
        }
    }

    // 并发保存transform数据（优化批量插入语法）
    if !transform_map.is_empty() {
        let keys = transform_map.keys().collect::<Vec<_>>();
        for chunk in keys.chunks(chunk_size) {
            let mut jsons = vec![];
            for &&k in chunk {
                let v = transform_map.get(&k).unwrap();
                jsons.push(format!("{{'id':trans:⟨{}⟩, 'd':{}}}", k, v));
            }
            let sql = format!("INSERT IGNORE INTO trans [{}];", jsons.join(","));
            let db = SUL_DB.clone();
            let future = tokio::spawn(async move { db.query(sql).await });
            db_futures.push(future);
        }
    }

    // 并发保存vec3数据（优化批量插入语法）
    if !vec3_map.is_empty() {
        let keys = vec3_map.keys().collect::<Vec<_>>();
        for chunk in keys.chunks(chunk_size) {
            let mut jsons = vec![];
            for &&k in chunk {
                let v = vec3_map.get(&k).unwrap();
                jsons.push(format!("{{'id':vec3:⟨{}⟩, 'd':{}}}", k, v));
            }
            let sql = format!("INSERT IGNORE INTO vec3 [{}];", jsons.join(","));
            let db = SUL_DB.clone();
            let future = tokio::spawn(async move { db.query(sql).await });
            db_futures.push(future);
        }
    }

    // 等待所有并发任务完成
    while let Some(result) = db_futures.next().await {
        if let Err(e) = result {
            debug_model_debug!("Task join error: {:?}", e);
            // 这里可以选择继续或者返回错误
        } else if let Ok(query_result) = result {
            if let Err(db_err) = query_result {
                debug_model_debug!("Database query error: {:?}", db_err);
                // 处理数据库错误
            }
        }
    }

    Ok(())
}

/// 保存 instance 数据到数据库（事务化批处理版本）
pub async fn save_instance_data_optimize(
    inst_mgr: &ShapeInstancesData,
    replace_exist: bool,
) -> anyhow::Result<()> {
    debug_model_debug!(
        "save_instance_data_optimize start: inst_info={}, inst_geo_keys={}, tubi_keys={}, replace_exist={}",
        inst_mgr.inst_info_map.len(),
        inst_mgr.inst_geos_map.len(),
        inst_mgr.inst_tubi_map.len(),
        replace_exist
    );
    const CHUNK_SIZE: usize = 300;
    const MAX_TX_STATEMENTS: usize = 4;
    const MAX_CONCURRENT_TX: usize = 6;

    let mut aabb_map: HashMap<u64, String> = HashMap::new();
    let mut transform_map: HashMap<u64, String> = HashMap::new();
    if let Entry::Vacant(entry) = transform_map.entry(0) {
        entry.insert(serde_json::to_string(&Transform::IDENTITY)?);
    }
    let mut vec3_map: HashMap<u64, String> = HashMap::new();
    if replace_exist {
        let refnos: Vec<RefnoEnum> = inst_mgr.inst_info_map.keys().copied().collect();
        debug_model_debug!(
            "save_instance_data_optimize deleting existing inst_relate for {} refnos",
            refnos.len()
        );
        delete_inst_relate_cascade(&refnos, CHUNK_SIZE).await?;
    }

    // inst_geo & geo_relate
    let mut geo_batcher = TransactionBatcher::new(MAX_TX_STATEMENTS, MAX_CONCURRENT_TX);
    let mut inst_geo_buffer: Vec<String> = Vec::with_capacity(CHUNK_SIZE);
    let mut geo_relate_buffer: Vec<String> = Vec::with_capacity(CHUNK_SIZE);

    for inst_geo_data in inst_mgr.inst_geos_map.values() {
        for inst in &inst_geo_data.insts {
            if inst.transform.translation.is_nan()
                || inst.transform.rotation.is_nan()
                || inst.transform.scale.is_nan()
            {
                debug_model_debug!(
                    "[WARN] skip inst geo due to NaN transform: refno={:?}, geo_hash={}",
                    inst.refno,
                    inst.geo_hash
                );
                continue;
            }

            let transform_hash = gen_bytes_hash(&inst.transform);
            if let Entry::Vacant(entry) = transform_map.entry(transform_hash) {
                entry.insert(serde_json::to_string(&inst.transform)?);
            }

            let key_pts = inst.geo_param.key_points();
            let mut pt_hashes = Vec::with_capacity(key_pts.len());
            for key_pt in key_pts {
                let pts_hash = key_pt.gen_hash();
                pt_hashes.push(format!("vec3:⟨{}⟩", pts_hash));
                if let Entry::Vacant(entry) = vec3_map.entry(pts_hash) {
                    entry.insert(serde_json::to_string(&key_pt)?);
                }
            }

            let cat_negs_str = if !inst.cata_neg_refnos.is_empty() {
                format!(
                    ", cata_neg: [{}]",
                    inst.cata_neg_refnos.iter().map(|x| x.to_pe_key()).join(",")
                )
            } else {
                String::new()
            };

            let relate_json = format!(
                r#"in: inst_info:⟨{0}⟩, out: inst_geo:⟨{1}⟩, trans: trans:⟨{2}⟩, geom_refno: pe:{3}, pts: [{4}], geo_type: '{5}', visible: {6} {7}"#,
                inst_geo_data.id(),
                inst.geo_hash,
                transform_hash,
                inst.refno,
                pt_hashes.join(","),
                inst.geo_type.to_string(),
                inst.visible,
                cat_negs_str
            );
            let relate_id = gen_bytes_hash(&relate_json);
            geo_relate_buffer.push(format!("{{ {relate_json}, id: '{relate_id}' }}"));

            inst_geo_buffer.push(inst.gen_unit_geo_sur_json());

            if inst_geo_buffer.len() >= CHUNK_SIZE {
                let statement = format!(
                    "INSERT IGNORE INTO {} [{}];",
                    stringify!(inst_geo),
                    inst_geo_buffer.join(",")
                );
                geo_batcher.push(statement).await?;
                inst_geo_buffer.clear();
            }

            if geo_relate_buffer.len() >= CHUNK_SIZE {
                let statement = format!(
                    "INSERT RELATION INTO geo_relate [{}];",
                    geo_relate_buffer.join(",")
                );
                geo_batcher.push(statement).await?;
                geo_relate_buffer.clear();
            }
        }
    }

    if !inst_geo_buffer.is_empty() {
        let statement = format!(
            "INSERT IGNORE INTO {} [{}];",
            stringify!(inst_geo),
            inst_geo_buffer.join(",")
        );
        geo_batcher.push(statement).await?;
        debug_model_debug!(
            "save_instance_data_optimize flushing remaining inst_geo records: {}",
            inst_geo_buffer.len()
        );
    }

    if !geo_relate_buffer.is_empty() {
        let statement = format!(
            "INSERT RELATION INTO geo_relate [{}];",
            geo_relate_buffer.join(",")
        );
        geo_batcher.push(statement).await?;
        debug_model_debug!(
            "save_instance_data_optimize flushing remaining geo_relate records: {}",
            geo_relate_buffer.len()
        );
    }

    geo_batcher.finish().await?;

    // tubi -> aabb & transform maps
    for tubi in inst_mgr.inst_tubi_map.values() {
        if let Some(aabb) = tubi.aabb {
            let aabb_hash = gen_bytes_hash(&aabb);
            if let Entry::Vacant(entry) = aabb_map.entry(aabb_hash) {
                entry.insert(serde_json::to_string(&aabb)?);
            }
        }

        let transform_hash = gen_bytes_hash(&tubi.world_transform);
        if let Entry::Vacant(entry) = transform_map.entry(transform_hash) {
            entry.insert(serde_json::to_string(&tubi.world_transform)?);
        }
    }

    // neg_relate
    if !inst_mgr.neg_relate_map.is_empty() {
        let mut neg_batcher = TransactionBatcher::new(MAX_TX_STATEMENTS, MAX_CONCURRENT_TX);
        let mut neg_buffer: Vec<String> = Vec::with_capacity(CHUNK_SIZE);

        for (target, refnos) in &inst_mgr.neg_relate_map {
            for (index, refno) in refnos.iter().enumerate() {
                neg_buffer.push(format!(
                    "{{ in: {}, id: [{}, {index}], out: {} }}",
                    refno.to_pe_key(),
                    refno.to_string(),
                    target.to_pe_key(),
                ));

                if neg_buffer.len() >= CHUNK_SIZE {
                    let statement = format!(
                        "INSERT RELATION INTO neg_relate [{}];",
                        neg_buffer.join(",")
                    );
                    neg_batcher.push(statement).await?;
                    neg_buffer.clear();
                }
            }
        }

        if !neg_buffer.is_empty() {
            let statement = format!(
                "INSERT RELATION INTO neg_relate [{}];",
                neg_buffer.join(",")
            );
            neg_batcher.push(statement).await?;
        }

        neg_batcher.finish().await?;
    }

    // ngmr_relate
    if !inst_mgr.ngmr_neg_relate_map.is_empty() {
        let mut ngmr_batcher = TransactionBatcher::new(MAX_TX_STATEMENTS, MAX_CONCURRENT_TX);
        let mut ngmr_buffer: Vec<String> = Vec::with_capacity(CHUNK_SIZE);

        for (k, refnos) in &inst_mgr.ngmr_neg_relate_map {
            let kpe = k.to_pe_key();
            for (ele_refno, ngmr_geom_refno) in refnos {
                let ele_pe = ele_refno.to_pe_key();
                let ngmr_pe = ngmr_geom_refno.to_pe_key();
                ngmr_buffer.push(format!(
                    "{{ in: {0}, id: [{0}, {1}, {2}], out: {1}, ngmr: {2}}}",
                    ele_pe, kpe, ngmr_pe
                ));

                if ngmr_buffer.len() >= CHUNK_SIZE {
                    let statement = format!(
                        "INSERT RELATION INTO ngmr_relate [{}];",
                        ngmr_buffer.join(",")
                    );
                    ngmr_batcher.push(statement).await?;
                    ngmr_buffer.clear();
                }
            }
        }

        if !ngmr_buffer.is_empty() {
            let statement = format!(
                "INSERT RELATION INTO ngmr_relate [{}];",
                ngmr_buffer.join(",")
            );
            ngmr_batcher.push(statement).await?;
        }

        ngmr_batcher.finish().await?;
    }

    // inst_info & inst_relate
    let mut inst_keys: Vec<RefnoEnum> = Vec::with_capacity(inst_mgr.inst_info_map.len());
    debug_model_debug!(
        "🔍 [DEBUG] inst_info_map keys: {:?}",
        inst_mgr.inst_info_map.keys().collect::<Vec<&RefnoEnum>>()
    );
    let mut inst_info_batcher = TransactionBatcher::new(MAX_TX_STATEMENTS, MAX_CONCURRENT_TX);
    let mut inst_info_buffer: Vec<String> = Vec::with_capacity(CHUNK_SIZE);
    let mut inst_relate_batcher = TransactionBatcher::new(MAX_TX_STATEMENTS, MAX_CONCURRENT_TX);
    let mut inst_relate_buffer: Vec<String> = Vec::with_capacity(CHUNK_SIZE);

    for (key, info) in &inst_mgr.inst_info_map {
        inst_keys.push(*key);

        if info.world_transform.translation.is_nan()
            || info.world_transform.rotation.is_nan()
            || info.world_transform.scale.is_nan()
        {
            continue;
        }

        inst_info_buffer.push(info.gen_sur_json(&mut vec3_map));
        if inst_info_buffer.len() >= CHUNK_SIZE {
            let statement = format!(
                "INSERT IGNORE INTO {} [{}];",
                stringify!(inst_info),
                inst_info_buffer.join(",")
            );
            inst_info_batcher.push(statement).await?;
            inst_info_buffer.clear();
        }

        let transform_hash = gen_bytes_hash(&info.world_transform);
        if let Entry::Vacant(entry) = transform_map.entry(transform_hash) {
            entry.insert(serde_json::to_string(&info.world_transform)?);
        }

        // 获取所属参考号和类型
        let (belong_refno, belong_type) = if let Some(owner_refno) = find_owner_refno(key) {
            (owner_refno.to_string(), get_owner_type(&owner_refno).to_string())
        } else {
            ("".to_string(), "".to_string())
        };

        let relate_sql = format!(
            "{{id: {0}, in: {1}, out: inst_info:⟨{2}⟩, world_trans: trans:⟨{3}⟩, generic: '{4}', zone_refno: fn::find_ancestor_type({1}, 'ZONE'), dt: fn::ses_date({1}), has_cata_neg: {5}, solid: {6}, belong_refno: '{7}', belong_type: '{8}'}}",
            key.to_inst_relate_key(),
            key.to_pe_key(),
            info.id_str(),
            transform_hash,
            info.generic_type.to_string(),
            info.has_cata_neg,
            info.is_solid,
            belong_refno,
            belong_type,
        );

        inst_relate_buffer.push(relate_sql);
        if inst_relate_buffer.len() >= CHUNK_SIZE {
            let statement = format!(
                "INSERT RELATION INTO inst_relate [{}];",
                inst_relate_buffer.join(",")
            );
            inst_relate_batcher.push(statement).await?;
            inst_relate_buffer.clear();
        }
    }

    if !inst_relate_buffer.is_empty() {
        debug_model_debug!(
            "🔍 [DEBUG] save_instance_data_optimize flushing remaining inst_relate records: {}",
            inst_relate_buffer.len()
        );

        // 打印第一条 inst_relate 记录用于调试
        if let Some(first) = inst_relate_buffer.first() {
            debug_model_debug!("🔍 [DEBUG] First inst_relate record: {}", first);
        }

        let statement = format!(
            "INSERT RELATION INTO inst_relate [{}];",
            inst_relate_buffer.join(",")
        );
        debug_model_debug!("🔍 [DEBUG] Executing inst_relate INSERT SQL: {}", statement);
        debug_model_debug!(
            "🔍 [DEBUG] Executing inst_relate INSERT with {} records",
            inst_relate_buffer.len()
        );
        inst_relate_batcher.push(statement).await?;
        debug_model_debug!("✅ [DEBUG] inst_relate INSERT pushed successfully");
    }

    if !inst_info_buffer.is_empty() {
        let statement = format!(
            "INSERT IGNORE INTO {} [{}];",
            stringify!(inst_info),
            inst_info_buffer.join(",")
        );
        inst_info_batcher.push(statement).await?;
        debug_model_debug!(
            "save_instance_data_optimize flushing remaining inst_info records: {}",
            inst_info_buffer.len()
        );
    }

    // NOTE: 暂时跳过 has_inst 标记更新，后续单独处理以避免阻塞调试

    debug_model_debug!("🔍 [DEBUG] Finishing inst_relate_batcher...");
    inst_relate_batcher.finish().await?;
    debug_model_debug!("✅ [DEBUG] inst_relate_batcher finished successfully");

    // 调试：确认 inst_relate 是否已写入数据库
    if !inst_keys.is_empty() {
        let pe_list = inst_keys.iter().map(|k| k.to_pe_key()).join(",");
        let verify_sql = format!(
            "SELECT count() AS cnt FROM inst_relate WHERE in IN [{}];",
            pe_list
        );
        match SUL_DB.query_response(&verify_sql).await {
            Ok(mut resp) => match resp.take::<Vec<serde_json::Value>>(0) {
                Ok(counts) => debug_model_debug!(
                    "🔍 [DEBUG] inst_relate verify counts for [{}]: {:?}",
                    pe_list,
                    counts
                ),
                Err(err) => debug_model_debug!(
                    "❌ [DEBUG] inst_relate verify take failed (sql: {}): {}",
                    verify_sql,
                    err
                ),
            },
            Err(e) => {
                debug_model_debug!(
                    "❌ [DEBUG] inst_relate verify query failed (sql: {}): {}",
                    verify_sql,
                    e
                );
            }
        }
    }

    debug_model_debug!("🔍 [DEBUG] Finishing inst_info_batcher...");
    inst_info_batcher.finish().await?;
    debug_model_debug!("✅ [DEBUG] inst_info_batcher finished successfully");

    // aabb
    if !aabb_map.is_empty() {
        let mut aabb_batcher = TransactionBatcher::new(MAX_TX_STATEMENTS, MAX_CONCURRENT_TX);
        let mut json_buffer: Vec<String> = Vec::with_capacity(CHUNK_SIZE);

        for (&hash, value) in &aabb_map {
            json_buffer.push(format!("{{'id':aabb:⟨{}⟩, 'd':{}}}", hash, value));
            if json_buffer.len() >= CHUNK_SIZE {
                let statement = format!("INSERT IGNORE INTO aabb [{}];", json_buffer.join(","));
                aabb_batcher.push(statement).await?;
                json_buffer.clear();
            }
        }

        if !json_buffer.is_empty() {
            let statement = format!("INSERT IGNORE INTO aabb [{}];", json_buffer.join(","));
            aabb_batcher.push(statement).await?;
        }

        aabb_batcher.finish().await?;
    }

    // transform
    if !transform_map.is_empty() {
        let mut transform_batcher = TransactionBatcher::new(MAX_TX_STATEMENTS, MAX_CONCURRENT_TX);
        let mut json_buffer: Vec<String> = Vec::with_capacity(CHUNK_SIZE);

        for (&hash, value) in &transform_map {
            json_buffer.push(format!("{{'id':trans:⟨{}⟩, 'd':{}}}", hash, value));
            if json_buffer.len() >= CHUNK_SIZE {
                let statement = format!("INSERT IGNORE INTO trans [{}];", json_buffer.join(","));
                transform_batcher.push(statement).await?;
                json_buffer.clear();
            }
        }

        if !json_buffer.is_empty() {
            let statement = format!("INSERT IGNORE INTO trans [{}];", json_buffer.join(","));
            transform_batcher.push(statement).await?;
        }

        transform_batcher.finish().await?;
    }

    // vec3
    if !vec3_map.is_empty() {
        let mut vec3_batcher = TransactionBatcher::new(MAX_TX_STATEMENTS, MAX_CONCURRENT_TX);
        let mut json_buffer: Vec<String> = Vec::with_capacity(CHUNK_SIZE);

        for (&hash, value) in &vec3_map {
            json_buffer.push(format!("{{'id':vec3:⟨{}⟩, 'd':{}}}", hash, value));
            if json_buffer.len() >= CHUNK_SIZE {
                let statement = format!("INSERT IGNORE INTO vec3 [{}];", json_buffer.join(","));
                vec3_batcher.push(statement).await?;
                json_buffer.clear();
            }
        }

        if !json_buffer.is_empty() {
            let statement = format!("INSERT IGNORE INTO vec3 [{}];", json_buffer.join(","));
            vec3_batcher.push(statement).await?;
        }

        vec3_batcher.finish().await?;
    }

    debug_model_debug!(
        "save_instance_data_optimize finish: inst_info={}, inst_geo={}, tubi={}, neg={}, ngmr={}",
        inst_mgr.inst_info_map.len(),
        inst_mgr.inst_geos_map.len(),
        inst_mgr.inst_tubi_map.len(),
        inst_mgr.neg_relate_map.len(),
        inst_mgr.ngmr_neg_relate_map.len()
    );

    Ok(())
}

struct TransactionBatcher {
    max_statements: usize,
    max_concurrent: usize,
    pending: Vec<String>,
    tasks: FuturesUnordered<JoinHandle<anyhow::Result<()>>>,
}

impl TransactionBatcher {
    fn new(max_statements: usize, max_concurrent: usize) -> Self {
        let max_statements = max_statements.max(1);
        let max_concurrent = max_concurrent.max(1);
        Self {
            max_statements,
            max_concurrent,
            pending: Vec::with_capacity(max_statements),
            tasks: FuturesUnordered::new(),
        }
    }

    async fn push(&mut self, statement: String) -> anyhow::Result<()> {
        if statement.trim().is_empty() {
            return Ok(());
        }

        self.pending.push(statement);
        if self.pending.len() >= self.max_statements {
            self.flush().await?;
        }
        Ok(())
    }

    async fn flush(&mut self) -> anyhow::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }

        let statements = std::mem::take(&mut self.pending);
        let query = build_transaction_block(&statements);
        let db = SUL_DB.clone();
        let debug_query = query.clone();
        // debug_model_debug!(
        //     "🔍 [DEBUG] TransactionBatcher flushing {} statements:\n{}",
        //     statements.len(),
        //     debug_query
        // );

        self.tasks.push(tokio::spawn(async move {
            match db.query(query).await {
                Ok(resp) => {
                    // debug_model_debug!("✅ [DEBUG] TransactionBatcher query executed successfully: {:?}", resp);
                    Ok(())
                }
                Err(err) => {
                    debug_model_debug!("❌ [DEBUG] TransactionBatcher query error: {}", err);
                    Err(anyhow::Error::from(err))
                }
            }
        }));

        self.await_if_needed().await
    }

    async fn await_if_needed(&mut self) -> anyhow::Result<()> {
        while self.tasks.len() >= self.max_concurrent {
            if let Some(result) = self.tasks.next().await {
                match result {
                    Ok(inner) => inner?,
                    Err(join_err) => return Err(join_err.into()),
                }
            }
        }
        Ok(())
    }

    async fn finish(mut self) -> anyhow::Result<()> {
        if !self.pending.is_empty() {
            self.flush().await?;
        }

        while let Some(result) = self.tasks.next().await {
            match result {
                Ok(inner) => inner?,
                Err(join_err) => return Err(join_err.into()),
            }
        }

        Ok(())
    }
}

fn build_transaction_block(statements: &[String]) -> String {
    let estimated_len = statements.iter().map(|s| s.len() + 2).sum::<usize>() + 32;
    let mut block = String::with_capacity(estimated_len);
    block.push_str("BEGIN TRANSACTION;\n");
    for stmt in statements {
        let trimmed = stmt.trim_end();
        block.push_str(trimmed);
        if !trimmed.ends_with(';') {
            block.push(';');
        }
        block.push('\n');
    }
    block.push_str("COMMIT TRANSACTION;");
    block
}

/// 查找所属参考号
/// 
/// 根据参考号查找其所属的参考号，例如对于 BRAN HANG 返回其所属的管道参考号
fn find_owner_refno(refno: &RefnoEnum) -> Option<RefnoEnum> {
    // 这里需要根据实际业务逻辑实现
    // 1. 查询数据库获取元素的 owner 信息
    // 2. 检查 owner 的类型，如果是 BRAN 或 EUIQ 类型，则返回其参考号
    // 3. 如果找不到或类型不匹配，则返回 None
    
    // 示例实现（需要根据实际数据库结构调整）
    /*
    let sql = format!(
        "SELECT owner FROM pe WHERE refno = '{}' LIMIT 1",
        refno.to_string()
    );
    
    if let Ok(Some(owner_refno)) = SUL_DB.query_take::<Option<String>>(&sql, 0).await {
        if let Some(owner_refno) = owner_refno {
            // 检查 owner 类型
            let type_sql = format!(
                "SELECT noun FROM pe WHERE refno = '{}' LIMIT 1",
                owner_refno
            );
            
            if let Ok(Some(noun)) = SUL_DB.query_take::<Option<String>>(&type_sql, 0).await {
                if let Some(noun) = noun {
                    if noun == "BRAN" || noun == "EUIQ" {
                        return Some(owner_refno.into());
                    }
                }
            }
        }
    }
    */
    
    None
}

/// 获取所有者类型
/// 
/// 根据参考号返回所有者类型，例如 "PIPE" 或 "EQUI"
fn get_owner_type(owner_refno: &RefnoEnum) -> &'static str {
    // 这里需要根据实际业务逻辑实现
    // 1. 查询数据库获取元素的类型
    // 2. 根据类型返回对应的字符串
    
    // 示例实现（需要根据实际数据库结构调整）
    /*
    let sql = format!(
        "SELECT noun FROM pe WHERE refno = '{}' LIMIT 1",
        owner_refno.to_string()
    );
    
    if let Ok(Some(noun)) = SUL_DB.query_take::<Option<String>>(&sql, 0).await {
        if let Some(noun) = noun {
            return match noun.as_str() {
                "BRAN" => "PIPE",
                "EUIQ" => "EQUI",
                _ => "",
            };
        }
    }
    */
    
    ""
}
