use crate::api::attr::query_attr;
use crate::api::children::travel_children_with_type;
use crate::consts::AQL_WATER_CALCULATION_COLLECTION;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::{save_arangodb_doc, ArDatabase};
use crate::graph_db::pdms_inst_arango::query_insts_shape_data;
use crate::rvm::data_api::query_rvm_geo_instance_aql;
use aios_core::pdms_types::AttrMap;
use aios_core::pdms_types::{GeoBasicType, RefU64};
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
use std::fs::File;
use std::hash::{Hash, Hasher};
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
        let Ok(send_value) = serde_json::to_value(&json_data) else { return "数据结构反序列化失败".to_string(); };
        if let Ok(_result) = query_water_calculation_data(&database, &key.to_string()).await {
            let _ = save_arangodb_doc(send_value, AQL_WATER_CALCULATION_COLLECTION, &database, true).await.unwrap();
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
pub async fn export_stp_(
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

    let all_plugged_refnos: HashSet<RefU64> = stp_packet.all_plugged_hole_refnos().collect();
    let export_refnos: Vec<RefU64> = stp_packet.export_refnos().cloned().collect();
    let shapes_data = query_insts_shape_data(
        &mgr.get_arango_db().await?,
        &export_refnos,
        &[
            GeoBasicType::Pos,
            GeoBasicType::CateNeg,
            GeoBasicType::Neg,
            GeoBasicType::CateCrossNeg,
        ],
    )
    .await?;

    let mut total_shapes_map: HashMap<RefU64, Shape> = HashMap::default();
    //one to many relationship
    let mut boolean_map: BTreeMap<RefU64, Vec<(RefU64, Shape)>> = BTreeMap::new();
    for (refno, geos_info) in &shapes_data.inst_info_map {
        //被封堵了的，相当于没有出现过，直接忽略
        if all_plugged_refnos.contains(refno) {
            continue;
        }
        let Some(insts_data) = shapes_data.get_inst_geos_data(geos_info) else {
            continue;
        };
        if let Some((shape, own_pos_refno)) = insts_data.gen_occ_shape(&geos_info.world_transform) {
            if let Some(o) = own_pos_refno && o.is_valid(){
                boolean_map.entry(o).or_default().push((*refno, shape));
            } else{
                total_shapes_map.insert(*refno, shape);
            }
        }
        let ngmr_shapes = insts_data.gen_ngmr_occ_shapes(&geos_info.world_transform);
        for (o, shape) in ngmr_shapes {
            dbg!(o);
            boolean_map.entry(o).or_default().push((*refno, shape));
        }
    }
    // boolean_map.values_mut().for_each(|x| {
    //     x.sort_by(|a, b| a.0.cmp(&b.0));
    // });

    let refnos = boolean_map
        .values()
        .flat_map(|x| x.iter().map(|t| t.0))
        .collect::<Vec<_>>();
    // dbg!(&refnos);

    total_shapes_map
        .iter_mut()
        .filter(|(k, _)| boolean_map.contains_key(k))
        .for_each(|(k, v)| {
            let neg_shapes = boolean_map.get(k).unwrap();
            neg_shapes.into_iter().foreach(|t| {
                //对于负实体要统一做一个延伸处理，否则负实体会出现薄片
                *v = v.subtract_shape(&t.1).0;
            });
        });

    let mut final_compound_shape = Compound::from_shapes(total_shapes_map.values());
    fs::create_dir_all("./assets/walter_steps")?;
    final_compound_shape
        .write_step(&format!(
            "./assets/walter_steps/{}.step",
            &stp_packet.file_name
        ))
        .unwrap();
dbg!("***");
    Ok(true)
}

///查询数据库中是否已有当前名称的文件
pub async fn query_water_calculation_data(database: &ArDatabase, key_value: &str) -> anyhow::Result<Option<Vec<FloodingStpToArangodb>>> {
    let aql = AqlQuery::new("let v = document('water_calculation',@_key)\
        return unset(v , '_id','_rev') ").bind_var("_key", key_value);
    let data_vec: Vec<FloodingStpToArangodb> = database.aql_query(aql).await?;
    return Ok(Some((data_vec)));
}

///查询数据库中所有记录
pub async fn query_water_calculation_data_total_aql(
    database: &ArDatabase,
) -> anyhow::Result<Vec<FloodingStpToArangodb>> {
    let aql = AqlQuery::new(
        "
    for c in @@collection
        return unset(c , '_id','_rev')",
    )
    .bind_var("@collection", AQL_WATER_CALCULATION_COLLECTION);
    let result = database.aql_query::<FloodingStpToArangodb>(aql).await?;
    Ok(result)
}
