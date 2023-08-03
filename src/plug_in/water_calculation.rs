use std::collections::hash_map::DefaultHasher;
use aios_core::pdms_types::RefU64;
use crate::data_interface::tidb_manager::AiosDBManager;
use std::collections::HashSet;
use aios_core::pdms_types::AttrMap;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use crate::api::children::travel_children_with_type;
use crate::data_interface::interface::PdmsDataInterface;
use aios_core::water_calculation::{CivilEngineeringStp, FloodingHole, FloodingHoleVec, WaterComputeStp};
use aios_core::water_calculation::ExportFloodingStpEvent;
// use opencascade::primitives::Compound;
use arangors_lite::AqlQuery;
use crate::api::attr::query_attr;
use crate::rvm::data_api::query_rvm_geo_instance_aql;
use crate::consts::AQL_WATER_CALCULATION_COLLECTION;
use crate::graph_db::pdms_arango::{ArDatabase, save_arangodb_doc};
use aios_core::water_calculation::FloodingStpToArangodb;


///得到导出stp的选中节点的参考号
pub async fn get_hole_refno(aios_mgr: &AiosDBManager, types: &HashSet<&str>, water_compute: &mut WaterComputeStp, flooding_hole_vec: &mut FloodingHoleVec, i: &(RefU64, String)) {
    let mut map = HashMap::new();
    map.insert(i.0.clone(), vec![]);
    water_compute.civil_engineering.push(map);
    if let Some((_, project_db)) = aios_mgr.get_project_pool_by_refno(i.0.clone()).await {
        for k in types {
            if let Ok(val) = travel_children_with_type(i.0.clone(), k.to_string(), &project_db).await {
                let mut result = val.into_iter().map(|x| (x.refno, x.name)).collect::<Vec<(RefU64, String)>>();
                for j in result {
                    let mut flooding_hole = FloodingHole::default();
                    flooding_hole.owner_refno = i.0.clone();
                    flooding_hole.refno = j.0.clone();
                    flooding_hole.name = j.1.clone();
                    flooding_hole_vec.data.push(flooding_hole);
                }
            }
        }
    }
}

///得到导出stp所需孔洞数据
pub async fn get_detail_data_for_export_stp(aios_mgr: &AiosDBManager, mut data: ExportFloodingStpEvent) -> ExportFloodingStpEvent {
    //向上找到对应的wall
    let att_type = HashSet::from(["CWALL", "STWALL", "GWALL", "WALL", "CFLOOR", "FLOOR"]);
    for i in &data.refnos {
        let mut refno = i.1.clone();
        while let Some(basic) = aios_mgr.get_refno_basic(refno) {
            if att_type.contains(&basic.get_type()) {
                let mut civil = CivilEngineeringStp::default();
                civil.wall_refno = refno;
                //判断是孔洞还是门洞
                let mut is_door = false;
                if let Ok(mut val) = query_attr(i.1.clone(), &aios_mgr, None).await {
                    if val.get_type() == "FITT" {
                        if let Some(info) = val.map.get(&397059875) {
                            if info.get_val_as_string().contains("门洞") {
                                is_door = true;
                            }
                        }
                    }
                }
                if is_door {
                    civil.door_refno = Some(i.1.clone());
                } else {
                    civil.hole_refno = Some(i.1.clone());
                }
                for mut j in &mut data.stp.civil_engineering {
                    if let Some(value) = j.get_mut(&i.0) {
                        value.push(civil);
                        break;
                    }
                }
                break;
            }
            refno = basic.get_owner();
        }
    }
    data
}

pub async fn save_stp_data_to_arangodb(aios_mgr: &AiosDBManager, mut stp: ExportFloodingStpEvent) -> String {
    // 将数据保存至图数据库
    if let Ok(database) = aios_mgr.get_arango_db().await {
        let mut hasher = DefaultHasher::new();
        stp.file_name.hash(&mut hasher);
        let key = hasher.finish();
        let json_data = vec![stp.to_arango_struct()];
        let Ok(send_value) = serde_json::to_value(&json_data) else { return "数据结构反序列化失败".to_string(); };
        if let Ok(_result) = query_water_calculation_data(&database, key.to_string()).await {
            let _ = save_arangodb_doc(send_value, AQL_WATER_CALCULATION_COLLECTION, &database, true).await.unwrap();
        } else {
            let _ = save_arangodb_doc(send_value, AQL_WATER_CALCULATION_COLLECTION, &database, false).await.unwrap();
        }
    }
    "Ok".to_string()
}


#[cfg(feature = "opencascade_rs")]
pub async fn export_stp(mgr: &AiosDBManager, stp_packet: &WaterComputeStp) -> anyhow::Result<bool> {
    let pos_refnos: Vec<RefU64> = stp_packet.civil_engineering.iter()
        .map(|x| x.keys().cloned())
        .flatten()
        .collect();
    let wall_refno = RefU64::from_refno_str("17496/106683").unwrap();
    let mut wall_refnos = vec![wall_refno];
    dbg!(&wall_refnos);
    let rvm_infos = query_rvm_geo_instance_aql(wall_refnos, &mgr.get_arango_db().await?).await?;
    let refnos = rvm_infos.iter().map(|x| x.refno).collect::<Vec<_>>();
    dbg!(&refnos.len());
    let mut wall_shape = rvm_infos.iter()
        .filter(|x| x.refno == wall_refno )
        .map(|x| x.gen_occ_shape())
        .flatten()
        .nth(0).unwrap();

    //过滤出找到ngrm的shapes
    let ngmr_shapes = rvm_infos.iter()
        .map(|x| x.gen_ngmr_occ_shape())
        .flatten()
        .collect::<Vec<_>>();
    for n in ngmr_shapes{
        wall_shape = wall_shape.subtract_shape(&n).0;
    }
    // let range = &pos_shapes[..];
    // // let range = &shapes[2..3];
    // let compound_shape = Compound::from_shapes(range);
    wall_shape.write_step("walter_steps/wall_shape.step").unwrap();
    Ok(true)
}


pub async fn query_water_calculation_data(database: &ArDatabase, key_value: String) -> anyhow::Result<Option<Vec<FloodingStpToArangodb>>> {
    let aql = AqlQuery::new("let v = document('water_calculaion',@_key)\
        return unset(v , '_id','_rev') ")
        .bind_var("_key", key_value);
    let data_vec: Vec<FloodingStpToArangodb> = database.aql_query(aql).await?;
    return Ok(Some((data_vec)));
}

pub async fn query_water_calculation_data_total_aql(database: &ArDatabase) -> anyhow::Result<Vec<FloodingStpToArangodb>> {
    let aql = AqlQuery::new("
    for c in @@collection
        return unset(c , '_id','_rev')").bind_var("@collection", AQL_WATER_CALCULATION_COLLECTION);
    let result = database.aql_query::<FloodingStpToArangodb>(aql).await?;
    Ok(result)
}


