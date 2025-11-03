use aios_core::water_calculation::*;
#[cfg(feature = "opencascade_rs")]
use opencascade::adhoc::AdHocShape;
use bevy_transform::prelude::Transform;
use glam::Vec3;
use crate::consts::AQL_WATER_CALCULATION_COLLECTION;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::save_arangodb_doc;
use crate::graph_db::pdms_inst_arango::query_insts_shape_data;
use aios_core::pdms_types::*;
use aios_core::water_calculation::ExportFloodingStpEvent;
use aios_core::water_calculation::FloodingStpToArangodb;
use aios_core::water_calculation::*;
use arangors_lite::AqlQuery;
use itertools::Itertools;
#[cfg(feature = "opencascade_rs")]
use opencascade::primitives::*;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use crate::arangodb::ArDatabase;
use std::fs::File;
use crate::consts::*;
use std::io::Write;

/// 将数据保存至图数据库
pub async fn save_stp_data_to_arangodb(
    aios_mgr: &AiosDBManager,
    mut stp: ExportFloodingStpEvent,
) -> String {
    if let Ok(database) = aios_mgr.get_arango_db().await {
        let mut hasher = DefaultHasher::new();
        stp.file_name.hash(&mut hasher);
        let key = hasher.finish();
        let json_data = vec![stp.to_arango_struct()];
        let Ok(send_value) = serde_json::to_value(&json_data) else {
            return "数据结构反序列化失败".to_string();
        };
        if let Ok(_result) = query_water_calculation_data(&database, &key.to_string()).await {
            let _ = save_arangodb_doc(
                send_value,
                AQL_WATER_CALCULATION_COLLECTION,
                &database,
                true,
            )
                .await
                .unwrap();
        } else {
            let _ = save_arangodb_doc(
                send_value,
                AQL_WATER_CALCULATION_COLLECTION,
                &database,
                false,
            )
                .await
                .unwrap();
        }
    }
    "Ok".to_string()
}

#[cfg(not(feature = "opencascade_rs"))]
///导出水淹计算stp
pub async fn export_stp(
    mgr: &AiosDBManager,
    stp_packet: ExportFloodingStpEvent,
) -> anyhow::Result<bool> {
    let mut file = File::create(format!(
        "./assets/walter_steps/{}.stp",
        stp_packet.file_name.as_str()
    ))?;
    let mut test_str = "测试STP文件下载";
    file.write_all(test_str.as_bytes())?;

    Ok(true)
}


#[cfg(feature = "opencascade_rs")]
///导出水淹计算stp
pub async fn export_stp(
    mgr: &AiosDBManager,
    stp_packet: ExportFloodingStpEvent,
) -> anyhow::Result<bool> {
    use std::collections::BTreeMap;
    let all_plugged_hole_refnos: HashSet<RefU64> = stp_packet.all_plugged_hole_refnos().collect();
    let all_plugged_door_refnos: HashSet<RefU64> = stp_packet.all_plugged_door_refnos().collect();
    let export_refnos: Vec<RefU64> = stp_packet.export_refnos().cloned().collect();
    let mut shapes_data = query_insts_shape_data(
        &mgr.get_arango_db().await?,
        &export_refnos,
        Some(&[
            GeoBasicType::Pos,
            GeoBasicType::CateNeg,
            GeoBasicType::Neg,
            GeoBasicType::CataCrossNeg,
        ]),
    ).await?;

    let mut total_shapes_map: HashMap<RefU64, Shape> = HashMap::default();
    //one to many relationship

    //针对所有的门，计算一下新的模型信息

    let mut boolean_map: BTreeMap<RefU64, Vec<(RefU64, Rc<Shape>)>> = BTreeMap::new();
    for (refno, geos_info) in &shapes_data.inst_info_map {
        //被封堵了的，相当于没有出现过，直接忽略
        if all_plugged_hole_refnos.contains(refno) {
            continue;
        }
        let is_door = all_plugged_door_refnos.contains(refno);
        //如果是门，直接换成新的geos_info
        let Some(insts_data) = shapes_data.get_inst_geos_data(geos_info) else {
            continue;
        };

        let mut transform = geos_info.world_transform;
        if let Ok((shape, own_pos_refnos)) = insts_data.gen_occ_shape(&transform) {
            if !own_pos_refnos.is_empty() {
                let t_shape = if is_door {
                    let mut box_shape = AdHocShape::make_box(100.0, 100.0, 100.0).0;
                    box_shape.transform_by_mat(&transform.to_matrix().as_dmat4());
                    Rc::new(box_shape)
                } else {
                    Rc::new(shape)
                };
                //todo，更改门的角度参数，重新生成men的mesh - 开孔的长方体mesh


                for o in own_pos_refnos {
                    if !o.is_valid() {
                        continue;
                    }
                    boolean_map.entry(o).or_default().push((*refno, t_shape.clone()));
                }
                if is_door {
                    //door 已经处理，不需要处理第二次
                    continue;
                }
            } else {
                total_shapes_map.insert(*refno, shape);
            }
        }


        let mut ngmr_shapes = insts_data.gen_ngmr_occ_shapes(&transform);
        for (mut refnos, mut shape) in ngmr_shapes {
            //需要进行缩放处理，宽度为门的1/10，高度固定为100
            if is_door {
                //2150 1000 700
                dbg!("处理门: {d}");
                let inst = &insts_data.insts[0];
                let extents = insts_data.aabb.unwrap().extents();

                transform = Transform::from_translation(Vec3::new(0.0, 0.0, -extents.x / 2.0)) * transform * inst.transform;
                let mut box_shape = AdHocShape::make_box(100.0, extents.y as f64 / 10.0, extents.z as f64).0;
                box_shape.transform_by_mat(&transform.to_matrix().as_dmat4());
                let t_shape = Rc::new(box_shape);
                refnos.into_iter().for_each(|o| {
                    boolean_map.entry(o).or_default().push((*refno, t_shape.clone()));
                });
                break;
            } else {
                let t_shape = Rc::new(shape);
                refnos.into_iter().for_each(|o| {
                    boolean_map.entry(o).or_default().push((*refno, t_shape.clone()));
                });
            }
        }
    }

    total_shapes_map
        .iter_mut()
        .filter(|(k, _)| boolean_map.contains_key(k))
        .for_each(|(k, v)| {
            let neg_shapes = boolean_map.get(k).unwrap();
            neg_shapes.into_iter().for_each(|t| {
                //对于负实体要统一做一个延伸处理，否则负实体会出现薄片
                *v = v.subtract_shape(&t.1).0;
            });
        });

    let mut final_compound_shape = Compound::from_shapes(total_shapes_map.values());
    fs::create_dir_all("./assets/water_steps")?;
    final_compound_shape
        .write_step(&format!(
            "./assets/water_steps/{}.step",
            &stp_packet.file_name
        ))
        .unwrap();

    Ok(true)
}


///删除水淹计算的指定key的记录
pub async fn delete_water_calculation_data(
    database: &ArDatabase,
    key: String,
) -> anyhow::Result<Option<Vec<FloodingStpToArangodb>>> {
    let aql = format!("With {AQL_HOLE_DATA_COLLECTION} remove {{'_key':'{}'}} in {}", key, AQL_WATER_CALCULATION_COLLECTION);
    let result = database.aql_query::<FloodingStpToArangodb>(AqlQuery::new(aql.as_str())).await?;
    return Ok(Some(result));
}

///清空水淹计算的指定key的记录
pub async fn truncate_water_calculation_data(
    database: &ArDatabase,
) -> anyhow::Result<Option<Vec<FloodingStpToArangodb>>> {
    let aql = AqlQuery::new("\
    With water_calculation
    for data in water_calculation
            REMOVE data IN water_calculation
    ");
    let result = database.aql_query::<FloodingStpToArangodb>(aql).await?;
    // let result = database.aql_query::<FloodingStpToArangodb>(AqlQuery::new(aql.as_str())).await?;
    return Ok(Some(result));
}


///查询数据库中是否已有当前名称的文件
pub async fn query_water_calculation_data(
    database: &ArDatabase,
    key_value: &str,
) -> anyhow::Result<Option<Vec<FloodingStpToArangodb>>> {
    let aql = AqlQuery::new(
        "let v = document('water_calculation',@_key)\
        return unset(v , '_id','_rev') ",
    )
        .bind_var("_key", key_value);
    let data_vec: Vec<FloodingStpToArangodb> = database.aql_query(aql).await?;
    return Ok(Some((data_vec)));
}

///查询数据库中所有记录
pub async fn query_water_calculation_data_total_aql(database: &ArDatabase) -> anyhow::Result<Vec<FloodingStpToArangodb>> {
    let aql = AqlQuery::new("
    for c in @@collection
        return unset(c , '_id','_rev')").bind_var("@collection", AQL_WATER_CALCULATION_COLLECTION);

    let result = database.aql_query::<FloodingStpToArangodb>(aql).await?;
    Ok(result)
}