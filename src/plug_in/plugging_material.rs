use std::default;
use std::sync::Arc;
use aios_core::pdms_types::RefU64;
use crate::api::children::travel_children_with_type;
use crate::data_interface::tidb_manager::AiosDBManager;
use aios_core::plugging_material::PluggingVec;
use aios_core::plugging_material::PluggingData;
use crate::data_interface::interface::PdmsDataInterface;

pub async fn get_plugging_data_detail(aios_mgr: &AiosDBManager, refnos: Vec<(RefU64, String)>) -> anyhow::Result<PluggingVec> {
    let mut hole_data_vec = PluggingVec::default();
    let mut refno_vec = Vec::new();
    let mut wall_vec = Vec::new();
//得到对应类型的参考号
    for i in refnos {
        // let refno_url = i.0.to_url_refno();
        let mut attr_type = "".to_string();
        match i.1.as_str() {
            "STWALL" => {
                attr_type = "FITT".to_string();
            }
            "GWALL" => {
                attr_type = "PFIT".to_string();
            }
            "WALL" => {
                attr_type = "JLDATU".to_string();
            }
            (_) => {}
        }
        if attr_type != "".to_string() {
            if let Some((_, project_db)) = aios_mgr.get_project_pool_by_refno(i.0.clone()).await {
                if let Ok(val) = travel_children_with_type(i.0.clone(), attr_type.clone(), &project_db).await {
                    let mut result = val.into_iter().map(|x| x.refno).collect::<Vec<RefU64>>();
                    if attr_type == "JLDATU".to_string() {
                        for j in result {
                            wall_vec.push((j, i.0.clone()));
                        }
                    } else {
                        for j in result {
                            refno_vec.push((j, i.0.clone()));
                        }
                    }
                }
            }
        }
    }
    //通过得到的参考号取对应的name和para1(除Wall)
    for i in refno_vec {
        if let Ok(attr) = aios_mgr.get_attr(i.0.clone()).await {
            let mut desp = "".to_string();
            let mut materials = "".to_string();
            if let Some(data) = attr.get_f64_vec("DESP") {
                if data.len() != 0 {
                    desp = data[0].to_string();
                }
            }
            // if let Some(data) = attr.get_as_string("JGOBJNOTE") {
            //     materials = data.;
            // }
            if attr.get_name().to_string().contains("LL") || attr.get_name().to_string().contains("EE") || attr.get_name().to_string().contains("KK") {
                hole_data_vec.data.push(PluggingData {
                    name: attr.get_name().to_string(),
                    size: desp,
                    own_refno: i.1.clone(),
                    materials,
                    ..Default::default()
                });
            }
        }
    }
    //取Wall
    for i in wall_vec {
        let mut name = "".to_string();
        let mut desp = "".to_string();
        let mut materials = "".to_string();
        //取name
        if let Ok(attr) = aios_mgr.get_attr(i.0.clone()).await {
            name = attr.get_name().to_string();
            // if let Some(data) = attr.get_as_string("JGOBJNOTE") {
            //     materials = data;
            // }
        }
        //取尺寸
        if let Some((_, project_db)) = aios_mgr.get_project_pool_by_refno(i.0.clone()).await {
            if let Ok(val) = travel_children_with_type(i.0.clone(), "FIXING".to_string(), &project_db).await {
                let mut result = val.into_iter().map(|x| x.refno).collect::<Vec<RefU64>>();
                if result.len() > 0 {
                    if let Ok(attr) = aios_mgr.get_attr(result[0]).await {
                        if let Some(data) = attr.get_f64_vec("DESP") {
                            if attr.len() > 0 {
                                desp = data[0].to_string();
                            }
                        }
                    }
                }
            }
        }
        if name.contains("LL") || name.contains("EE") || name.contains("KK") {
            hole_data_vec.data.push(PluggingData {
                name,
                size: desp,
                own_refno: i.1.clone(),
                materials,
                ..Default::default()
            });
        }
    }
    return Ok(hole_data_vec);

}
