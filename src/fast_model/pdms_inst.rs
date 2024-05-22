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
) -> anyhow::Result<()> {
    let mut join_set = tokio::task::JoinSet::new();
    let mut aabb_map: HashMap<u64, String> = HashMap::new();
    let mut transform_map: HashMap<u64, String> = HashMap::new();
    //标识单位矩阵
    transform_map.insert(0, serde_json::to_string(&Transform::IDENTITY).unwrap());
    let mut param_map = HashMap::new();
    let mut vec3_map: HashMap<u64, String> = HashMap::new();

    let keys = inst_mgr.inst_geos_map.keys().collect::<Vec<_>>();
    let chunk_size = 100;
    for chunk in keys.chunks(chunk_size) {
        let mut json_vec = vec![];
        let mut geo_relate_vec = vec![];
        for &k in chunk {
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
                let cat_negs_str =  if !inst.cata_neg_refnos.is_empty() {
                    format!(
                        ", cata_neg=[{}]",
                        inst.cata_neg_refnos.iter().map(|x| x.to_pe_key()).join(",")
                    )
                } else {
                    "".to_string()
                };
                // dbg!(&v);
                let relate_sql = format!(
                    r#"
                    if inst_info:⟨{0}⟩.id == none {{
                        relate inst_info:⟨{0}⟩->geo_relate->inst_geo:⟨{1}⟩ set trans=trans:⟨{2}⟩,
                            geom_refno=pe:{3}, pts=[{4}], geo_type='{5}', visible={6} {7};
                    }};"#,
                    v.id(),
                    inst.geo_hash,
                    transform_hash,
                    inst.refno,
                    // param_hash,
                    pt_hashes.join(","),
                    inst.geo_type.to_string(),
                    inst.visible,
                    cat_negs_str
                );
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
            join_set.spawn(async move {
                SUL_DB.query(sql_string).await.unwrap();
            });

            //保存relate 关系
            if !geo_relate_vec.is_empty() {
                //使用surreal 保存NamedAttrMap
                // dbg!(&geo_relate_vec);
                join_set.spawn(async move {
                    let sql = geo_relate_vec.join("");
                    // println!("{}", &sql);
                    SUL_DB.query(sql).await.unwrap();
                });
            }
        }
    }
    while let Some(_) = join_set.join_next().await {}

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
    {
        let mut ngmr_relate_vec = vec![];
        for (k, refnos) in &inst_mgr.neg_relate_map {
            for r in refnos {
                ngmr_relate_vec.push(format!(
                    "relate {}->neg_relate:{}->{};",
                    r.to_pe_key(),
                    r.to_string(),
                    k.to_pe_key(),
                ));
            }
            // dbg!(&ngmr_relate_vec);
        }
        let ngmr_relate_sql = ngmr_relate_vec.join("");
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
            let mut sql = format!(
                "relate {}->{}->inst_info:⟨{}⟩ set world_trans=trans:⟨{}⟩, generic='{}', has_cata_neg={}, solid={}",
                // k.to_pe_versioned_key(v.version),
                k.to_pe_key(),
                // k.to_inst_relate_versioned_key(v.version),
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
    // dbg!("insert inst_relate, inst_info ok");

    // let mut join_set = tokio::task::JoinSet::new();
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
                // if k == 8440817253550486985u64{
                //     dbg!(&json);
                //     found = true;
                // }
                jsons.push(json);
            }
            let sql = format!("INSERT INTO aabb [{}];", jsons.join(","));
            // if found {
            //     println!("aabb sql is {}", &sql);
            // }
            // join_set.spawn(async move {
                SUL_DB.query(sql).await.unwrap();
            // });
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

    //保存param_map数据
    // if !param_map.is_empty() {
    //     let keys = param_map.keys().collect::<Vec<_>>();
    //     for chunk in keys.chunks(100) {
    //         let mut sql_string = "".to_string();
    //         for &&k in chunk {
    //             let v = param_map.get(&k).unwrap();
    //             let json = format!(
    //                 "INSERT IGNORE INTO param {{'id':param:⟨{}⟩, 'd':{}}};",
    //                 k, v
    //             );
    //             sql_string.push_str(&json);
    //         }
    //         // join_set.spawn(async move {
    //             SUL_DB.query(sql_string).await.unwrap();
    //         // });
    //     }
    // }

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

    // while let Some(_) = join_set.join_next().await {}


    //inst relate 放到最后保存, 因为是被监控的
    if !inst_relate_vec.is_empty() {
        //使用surreal 保存NamedAttrMap
        // dbg!(&inst_relate_vec);
        for chunk in inst_relate_vec.chunks(100) {
            SUL_DB.query(chunk.join(";")).await.unwrap();
        }
    }

    // dbg!("insert vec3, trans, param ok");

    Ok(())
}
