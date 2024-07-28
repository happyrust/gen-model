use std::collections::HashMap;

use aios_core::geometry::ShapeInstancesData;
use aios_core::pdms_types::*;
use aios_core::types::*;
use aios_core::SUL_DB;
use bevy_transform::prelude::Transform;
use itertools::Itertools;

use crate::data_interface::tidb_manager::AiosDBManager;

///保存instance 数据到数据库
pub async fn save_instance_data(
    inst_mgr: &ShapeInstancesData,
    replace_exist: bool,
) -> anyhow::Result<()> {
    // let mut join_set = tokio::task::JoinSet::new();
    let mut aabb_map: HashMap<u64, String> = HashMap::new();
    let mut transform_map: HashMap<u64, String> = HashMap::new();
    //标识单位矩阵
    transform_map.insert(0, serde_json::to_string(&Transform::IDENTITY).unwrap());
    let mut param_map = HashMap::new();
    let mut vec3_map: HashMap<u64, String> = HashMap::new();

    let chunk_size = 10;
    //把delete 提前，因为后面的插入都是异步的执行
    // dbg!(replace_exist);
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
    // dbg!(&keys);
    for chunk in keys.chunks(chunk_size) {
        let mut json_vec = vec![];
        let mut geo_relate_vec = vec![];
        for &k in chunk {
            // dbg!(k);
            let v = inst_mgr.inst_geos_map.get(k).unwrap();
            for inst in &v.insts {
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
                let relate_sql = format!(
                    r#"
                        {{
                            in: inst_info:⟨{0}⟩, out: inst_geo:⟨{1}⟩, trans: trans:⟨{2}⟩,
                            geom_refno: pe:{3}, pts: [{4}], geo_type: '{5}', visible: {6} {7}
                        }}
                    "#,
                    v.id(),
                    inst.geo_hash,
                    transform_hash,
                    inst.refno,
                    pt_hashes.join(","),
                    inst.geo_type.to_string(),
                    inst.visible,
                    cat_negs_str
                );
                // dbg!(&relate_sql);

                geo_relate_vec.push(relate_sql);
                //保存 unit shape 的几何参数
                json_vec.push(inst.gen_unit_geo_sur_json());
            }
        }

        if !json_vec.is_empty() {
            let mut sql_string = "".to_string();
            sql_string.push_str(&format!(
                "insert ignore into {} [{}];",
                stringify!(inst_geo),
                json_vec.join(",")
            ));
            #[cfg(feature = "debug_sql")]
            println!("insert inst_geo sql: {}", &sql_string);
            //使用surreal 保存NamedAttrMap
            // join_set.spawn(async move {
            SUL_DB.query(sql_string).await.unwrap();
        }
        // });
        //保存relate 关系
        // dbg!(geo_relate_vec.len());
        if !geo_relate_vec.is_empty() {
            // dbg!(&geo_relate_vec);
            let sql = format!("INSERT RELATION INTO geo_relate [{}];", geo_relate_vec.join(","));
            SUL_DB.query(sql).await.unwrap();
            // join_set.spawn(async move {
            //     SUL_DB.query(sql).await.unwrap();
            // });
        }
    }
    // while let Some(_) = join_set.join_next().await {}

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
    // dbg!(&keys);
    let mut join_set = tokio::task::JoinSet::new();
    // let mu tasks = vec![];
    let mut inst_relate_vec = vec![];

    // if let Some(refnos) = inst_mgr.ngmr_relate_map.get(k)
    if !inst_mgr.neg_relate_map.is_empty() {
        let mut neg_relate_vec = vec![];
        // dbg!(&inst_mgr.neg_relate_map);
        for (k, refnos) in &inst_mgr.neg_relate_map {
            //这里需要order
            for (indx, r) in refnos.into_iter().enumerate() {
                neg_relate_vec.push(format!(
                    "relate {}->neg_relate:[{}, {indx}]->{};",
                    r.to_pe_key(),
                    r.to_string(),
                    k.to_pe_key(),
                ));
            }
        }
        let neg_relate_sql = neg_relate_vec.join("");
        if !neg_relate_sql.is_empty() {
            SUL_DB.query(neg_relate_sql).await.unwrap();
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
                ngmr_relate_vec.push(format!("relate {0}->ngmr_relate:[{0}, {1}, {2}]->{1} set ngmr={2};", ele_pe, kpe, ngmr_pe));
            }
            // dbg!(&ngmr_relate_vec);
        }
        let ngmr_relate_sql = ngmr_relate_vec.join("");
        // dbg!(&ngmr_relate_sql);
        if !ngmr_relate_sql.is_empty() {
            SUL_DB.query(ngmr_relate_sql).await.unwrap();
        }
    }

    // dbg!(&inst_mgr.ngmr_relate_map);
    for chunk in keys.chunks(chunk_size) {
        let mut json_vec = vec![];
        for &k in chunk {
            let v = inst_mgr.inst_info_map.get(k).unwrap();
            if v.world_transform.is_nan() {
                continue;
            }
            json_vec.push(v.gen_sur_json(&mut vec3_map));

            let transform_hash = gen_bytes_hash::<_, 64>(&v.world_transform);
            if !transform_map.contains_key(&transform_hash) {
                transform_map.insert(
                    transform_hash,
                    serde_json::to_string(&v.world_transform).unwrap(),
                );
            }

            //arrive 和 leave 需要用 index
            //这里的 pts，存储的时点集信息
            let sql = format!(
                "relate {}->{}->inst_info:⟨{}⟩ set world_trans=trans:⟨{}⟩, generic='{}', has_cata_neg={}, solid={}",
                k.to_pe_key(),
                k.to_inst_relate_key(),
                v.id_str(),
                transform_hash,
                v.generic_type.to_string(),
                v.has_cata_neg,
                v.is_solid
            );
            inst_relate_vec.push(sql);
        }

        if !json_vec.is_empty() {
            let mut sql_string = "".to_string();
            sql_string.push_str(&format!(
                "insert ignore into {} [{}];",
                stringify!(inst_info),
                json_vec.join(",")
            ));
            join_set.spawn(async move {
                SUL_DB.query(sql_string).await.unwrap();
            });
        }
    }
    while let Some(_) = join_set.join_next().await {}

    //保存aabb
    if !aabb_map.is_empty() {
        // dbg!(aabb_map.len());
        let keys = aabb_map.keys().collect::<Vec<_>>();
        for chunk in keys.chunks(100) {
            let mut jsons = vec![];
            let mut found = false;
            for &&k in chunk {
                let v = aabb_map.get(&k).unwrap();
                let json = format!("{{'id':aabb:⟨{}⟩, 'd':{}}}", k, v);
                jsons.push(json);
            }
            let sql = format!("INSERT INTO aabb [{}];", jsons.join(","));
            SUL_DB.query(sql).await.unwrap();
        }
    }
    //保存transform
    if !transform_map.is_empty() {
        let keys = transform_map.keys().collect::<Vec<_>>();
        for chunk in keys.chunks(100) {
            let mut sql_string = "".to_string();
            for &&k in chunk {
                let v = transform_map.get(&k).unwrap();
                let json = format!(
                    "INSERT IGNORE INTO trans {{'id':trans:⟨{}⟩, 'd':{}}};",
                    k, v
                );
                sql_string.push_str(&json);
            }
            // join_set.spawn(async move {
            SUL_DB.query(sql_string).await.unwrap();
            // });
        }
    }

    if !vec3_map.is_empty() {
        let keys = vec3_map.keys().collect::<Vec<_>>();
        for chunk in keys.chunks(100) {
            let mut sql_string = "".to_string();
            for &&k in chunk {
                let v = vec3_map.get(&k).unwrap();
                let json = format!("INSERT IGNORE INTO vec3 {{'id':vec3:⟨{}⟩, 'd':{}}};", k, v);
                sql_string.push_str(&json);
            }
            // join_set.spawn(async move {
            SUL_DB.query(sql_string).await.unwrap();
            // });
        }
    }

    //inst relate 放到最后保存, 因为是被监控的
    if !inst_relate_vec.is_empty() {
        //使用surreal 保存NamedAttrMap
        for chunk in inst_relate_vec.chunks(100) {
            SUL_DB.query(chunk.join(";")).await.unwrap();
        }
    }

    Ok(())
}
