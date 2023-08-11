use std::collections::hash_map::DefaultHasher;
use aios_core::pdms_types::RefU64;
use crate::data_interface::tidb_manager::AiosDBManager;
use std::collections::HashSet;
use aios_core::pdms_types::AttrMap;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use crate::api::children::travel_children_with_type;
use crate::data_interface::interface::PdmsDataInterface;
use aios_core::water_calculation::*;
use aios_core::water_calculation::ExportFloodingStpEvent;
#[cfg(feature = "opencascade_rs")]
use opencascade::primitives::*;
use crate::api::attr::query_attr;
use crate::rvm::data_api::query_rvm_geo_instance_aql;
use crate::consts::AQL_WATER_CALCULATION_COLLECTION;
use crate::graph_db::pdms_arango::{ArDatabase, save_arangodb_doc};
use aios_core::water_calculation::FloodingStpToArangodb;
use arangors_lite::AqlQuery;

/// 将数据保存至图数据库
pub async fn save_stp_data_to_arangodb(aios_mgr: &AiosDBManager, mut stp: ExportFloodingStpEvent) -> String {
    if let Ok(database) = aios_mgr.get_arango_db().await {
        let mut hasher = DefaultHasher::new();
        stp.file_name.hash(&mut hasher);
        let key = hasher.finish();
        let json_data = vec![stp.to_arango_struct()];
        let Ok(send_value) = serde_json::to_value(&json_data) else { return "数据结构反序列化失败".to_string(); };
        if let Ok(_result) = query_water_calculation_data(&database, &key.to_string()).await {
            let _ = save_arangodb_doc(send_value, AQL_WATER_CALCULATION_COLLECTION, &database, true).await.unwrap();
        } else {
            let _ = save_arangodb_doc(send_value, AQL_WATER_CALCULATION_COLLECTION, &database, false).await.unwrap();
        }
    }
    "Ok".to_string()
}


#[cfg(feature = "opencascade_rs")]
///导出水淹计算stp
pub async fn export_stp(mgr: &AiosDBManager, stp_packet: ExportFloodingStpEvent) -> anyhow::Result<bool> {
    // let pos_refnos: Vec<RefU64> = stp_packet.stp.iter()
    //     .map(|x| x.keys().cloned())
    //     .flatten()
    //     .collect();
    // let mut total_shapes = vec![];
    // for pos_refno in pos_refnos {
    //     dbg!(pos_refno);
    //     let rvm_infos = query_rvm_geo_instance_aql(vec![pos_refno], &mgr.get_arango_db().await?).await?;
    //     let refnos = rvm_infos.iter().map(|x| x.refno).collect::<Vec<_>>();
    //     dbg!(&refnos);
    //     let Some(mut final_shape) = rvm_infos.iter()
    //         .filter(|x| x.refno == pos_refno)
    //         .map(|x| x.gen_occ_shape())
    //         .flatten()
    //         .nth(0) else {
    //         continue;
    //     };
    //
    //     //过滤出找到ngrm的shapes
    //     let ngmr_shapes = rvm_infos.iter()
    //         .map(|x| x.gen_ngmr_occ_shape())
    //         .flatten()
    //         .collect::<Vec<_>>();
    //     for n in ngmr_shapes{
    //         final_shape = final_shape.subtract_shape(&n).0;
    //         // final_shape = final_shape.union_shape(&n).0;
    //     }
    //     total_shapes.push(final_shape);
    // }
    //
    // let mut final_compound_shape = Compound::from_shapes(&total_shapes);
    //
    // // final_compound_shape.write_step(&format!("walter_steps/{name}.step")).unwrap();
    // final_compound_shape.write_step(&format!("walter_steps/test.step")).unwrap();

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
pub async fn query_water_calculation_data_total_aql(database: &ArDatabase) -> anyhow::Result<Vec<FloodingStpToArangodb>> {
    let aql = AqlQuery::new("
    for c in @@collection
        return unset(c , '_id','_rev')").bind_var("@collection", AQL_WATER_CALCULATION_COLLECTION);
    let result = database.aql_query::<FloodingStpToArangodb>(aql).await?;
    Ok(result)
}


