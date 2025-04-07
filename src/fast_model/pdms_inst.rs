use std::collections::HashMap;

use aios_core::geometry::ShapeInstancesData;
use aios_core::pdms_types::*;
use aios_core::types::*;
use aios_core::{get_db_option, SUL_DB};
use bevy_transform::prelude::Transform;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use itertools::Itertools;

use crate::data_interface::tidb_manager::AiosDBManager;
// use crate::fast_model::EXIST_MESH_GEOS;

/// 初始化数据库的 inst_relate 表的索引
pub async fn init_inst_relate_indices() -> anyhow::Result<()> {
    // 创建 zone_refno 字段的索引
    let create_index_sql = "
        DEFINE INDEX idx_inst_relate_zone_refno ON TABLE inst_relate COLUMNS zone_refno TYPE BTREE;
    ";
    let _ = SUL_DB.query(create_index_sql).await;
    Ok(())
}

///保存instance 数据到数据库
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
    //把delete 提前，因为后面的插入都是异步的执行
    if replace_exist {
        let keys = inst_mgr.inst_info_map.keys().collect::<Vec<_>>();
        for chunk in keys.chunks(chunk_size) {
            let mut delete_sql_vec = vec![];

            for &k in chunk {
                let v = inst_mgr.inst_info_map.get(k).unwrap();
                let delete_old_sql = format!(
                    r#"
                delete array::flatten(select value out->geo_relate.out from {0});
                delete array::flatten(select value out->geo_relate from {0});
                delete array::flatten(select value out from {0});
                delete {0};"#,
                    v.refno.to_inst_relate_key()
                );
                delete_sql_vec.push(delete_old_sql);
            }
            //如果需要删除之前的，先执行
            if !delete_sql_vec.is_empty() {
                let sql = delete_sql_vec.join("");
                // dbg!(&sql);
                SUL_DB.query(sql).await.unwrap();
            }
        }
        // return Ok(());
    }

    let keys = inst_mgr.inst_geos_map.keys().collect::<Vec<_>>();
    // let mut insert_handles = FuturesUnordered::new();
    let mut inst_geo_vec = vec![];
    let mut geo_relate_vec = vec![];

    // dbg!(&keys);
    for k in keys {
        let v = inst_mgr.inst_geos_map.get(k).unwrap();
        for inst in &v.insts {
            // dbg!(&inst);
            // if EXIST_MESH_GEOS.contains(&inst.geo_hash){
            //     continue;
            // }
            if inst.transform.is_nan() {
                dbg!(&inst);
                continue;
            }
            let transform_hash = gen_bytes_hash::<_, 64>(&inst.transform);
            if !transform_map.contains_key(&transform_hash) {
                transform_map.insert(
                    transform_hash,
                    serde_json::to_string(&inst.transform).unwrap(),
                );
            }
            let param_hash = gen_bytes_hash::<_, 64>(&inst.geo_param);
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
            let id = gen_bytes_hash::<_, 64>(&relate_json);
            let final_json = format!("{{ {relate_json}, id: '{id}' }}");
            // dbg!(&relate_sql);
            // println!("geo relate json: {}", &final_json);
            geo_relate_vec.push(final_json);
            //保存 unit shape 的几何参数
            inst_geo_vec.push(inst.gen_unit_geo_sur_json());
            // EXIST_MESH_GEOS.insert(inst.geo_hash);
        }
    }

    if !inst_geo_vec.is_empty() {
        for chunk in inst_geo_vec.chunks(chunk_size) {
            let sql_string = format!(
                "insert ignore into {} [{}];",
                stringify!(inst_geo),
                chunk.join(",")
            );
            // dbg!(&sql_string);
            // let handle = tokio::spawn(async move {
            SUL_DB.query(sql_string).await.unwrap();
            // });
            // insert_handles.push(handle);
        }
    }
    if !geo_relate_vec.is_empty() {
        // let handle = tokio::spawn(async move {
        for chunk in geo_relate_vec.chunks(chunk_size) {
            let sql = format!("INSERT RELATION INTO geo_relate [{}];", chunk.join(","));
            //
            // println!("geo relate sql: {}", &sql);
            let mut response = SUL_DB.query(sql).await.unwrap();
            // let mut error = response.take_errors();
            // if !error.is_empty() {
            //     dbg!(&error);
            // }
        }
        // });
        // insert_handles.push(handle);
    }

    //保存tubi的数据
    let keys = inst_mgr.inst_tubi_map.keys().collect::<Vec<_>>();
    for chunk in keys.chunks(chunk_size) {
        for &k in chunk {
            let v = inst_mgr.inst_tubi_map.get(k).unwrap();
            //更新aabb 和 transform，保存relate已经在别的地方加了，这里后面需要重构
            let aabb = v.aabb.unwrap();
            let aabb_hash = gen_bytes_hash::<_, 64>(&aabb);
            let transform_hash = gen_bytes_hash::<_, 64>(&v.world_transform);
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

    let keys = inst_mgr.inst_info_map.keys().collect::<Vec<_>>();
    if !inst_mgr.neg_relate_map.is_empty() {
        let mut neg_relate_vec = vec![];
        // dbg!(&inst_mgr.neg_relate_map);
        for (k, refnos) in &inst_mgr.neg_relate_map {
            //这里需要order
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
                SUL_DB.query(neg_relate_sql).await.unwrap();
            }
        }
    }

    // dbg!(&inst_mgr.ngmr_neg_relate_map);
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
                SUL_DB.query(ngmr_relate_sql).await.unwrap();
            }
        }
    }

    // dbg!(&inst_mgr.ngmr_relate_map);
    // for chunk in keys.chunks(chunk_size)
    {
        let mut inst_info_vec = vec![];
        let mut inst_relate_vec = vec![];
        for k in keys.clone() {
            let v = inst_mgr.inst_info_map.get(k).unwrap();
            if v.world_transform.is_nan() {
                continue;
            }
            inst_info_vec.push(v.gen_sur_json(&mut vec3_map));

            let transform_hash = gen_bytes_hash::<_, 64>(&v.world_transform);
            if !transform_map.contains_key(&transform_hash) {
                transform_map.insert(
                    transform_hash,
                    serde_json::to_string(&v.world_transform).unwrap(),
                );
            }
            
            let relate_sql = format!(
                "{{id: {},  in: {}, out: inst_info:⟨{}⟩, world_trans: trans:⟨{}⟩, generic: '{}', has_cata_neg: {}, solid: {}}}",
                k.to_inst_relate_key(),
                k.to_pe_key(),
                v.id_str(),
                transform_hash,
                v.generic_type.to_string(),
                v.has_cata_neg,
                v.is_solid,
                // v.dt.and_utc().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
            );
            if let Some(t_refno) = test_refno {
                if *k == t_refno.into() {
                    dbg!(v);
                    println!("inst relate sql: {}", &relate_sql);
                }
            }
            inst_relate_vec.push(relate_sql);
        }

        if !inst_info_vec.is_empty() {
            for chunk in inst_info_vec.chunks(chunk_size) {
                let sql_string = format!(
                    "insert ignore into {} [{}];",
                    stringify!(inst_info),
                    chunk.join(",")
                );
                SUL_DB.query(sql_string).await.unwrap();
            }
        }
        //inst relate 放到最后保存, 因为是被监控的
        if !inst_relate_vec.is_empty() {
            for chunk in inst_relate_vec.chunks(chunk_size) {
                let inst_relate_sql =
                    format!("INSERT RELATION INTO inst_relate [{}];", chunk.join(","));
                SUL_DB.query(inst_relate_sql).await.unwrap();
            }
            
            // 使用SQL函数更新zone_refno
            let update_zone_sql = "
                LET $records = SELECT * FROM inst_relate WHERE zone_refno = NONE;
                FOR $record IN $records {
                    LET $zone = fn::find_ancestor_type($record.in, 'ZONE');
                    IF $zone != NONE {
                        UPDATE $record SET zone_refno = $zone[0].refno;
                    }
                };
            ";
            SUL_DB.query(update_zone_sql).await.unwrap();
            
            for chunk in keys.to_vec().chunks(chunk_size) {
                let mut update_date_sql = String::new();
                for &k in chunk {
                    update_date_sql.push_str(&format!("update inst_relate:{k} set dt=fn::ses_date(pe:{k});"));
                }
                SUL_DB.query(update_date_sql).await.unwrap();
            }
        }
    }

    // while let Some(_) = insert_handles.next().await {}
    // dbg!("here");

    //保存aabb
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
            SUL_DB.query(sql).await.unwrap();
        }
    }
    //保存transform
    if !transform_map.is_empty() {
        let keys = transform_map.keys().collect::<Vec<_>>();
        for chunk in keys.chunks(chunk_size) {
            let mut sql_string = "".to_string();
            for &&k in chunk {
                let v = transform_map.get(&k).unwrap();
                let json = format!(
                    "INSERT IGNORE INTO trans {{'id':trans:⟨{}⟩, 'd':{}}};",
                    k, v
                );
                sql_string.push_str(&json);
            }
            SUL_DB.query(sql_string).await.unwrap();
        }
    }

    if !vec3_map.is_empty() {
        let keys = vec3_map.keys().collect::<Vec<_>>();
        for chunk in keys.chunks(chunk_size) {
            let mut sql_string = "".to_string();
            for &&k in chunk {
                let v = vec3_map.get(&k).unwrap();
                let json = format!("INSERT IGNORE INTO vec3 {{'id':vec3:⟨{}⟩, 'd':{}}};", k, v);
                sql_string.push_str(&json);
            }
            SUL_DB.query(sql_string).await.unwrap();
        }
    }

    Ok(())
}
