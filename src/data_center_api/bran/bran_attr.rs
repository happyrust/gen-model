use std::collections::HashMap;
use std::io::Write;
use std::process::id;
use std::vec;
use aios_core::data_center::AttrValue::{AttrIntArray, AttrMap, AttrString};
use aios_core::data_center::{AttrValue, CableWeight, DataCenterAttr, DataCenterInstance, DataCenterProject};
use aios_core::options::DbOption;
use aios_core::pdms_pluggin::heat_dissipation::InstPointMap;
use aios_core::pdms_types::{NamedAttrValue, PdmsElement, RefU64};
use aios_core::pdms_user::RefnoMajor;
use anyhow::anyhow;
use calamine::{open_workbook, RangeDeserializerBuilder, Reader, Xlsx};
use glam::Vec3;
use parry2d::simba::scalar::SupersetOf;
use regex::Regex;
use serde::{Serialize, Deserialize};
use crate::api::attr::query_explicit_attr;
use crate::api::children::query_ancestor_refnos_till_type_aql;
use crate::aql_api::attr_map::query_refnos_point_map_aql;
use crate::aql_api::children::{query_children_eles, query_children_order_aql, query_children_refnos, query_refnos_belong_major, query_refnos_travel_children_with_type_aql, query_travel_children_with_type_aql};
use crate::aql_api::foreign_refnos::{query_foreign_name_aql, query_foreign_refno_aql, query_foreign_refnos_aql};
use crate::aql_api::pdms_room::{query_room_codes_from_owner, query_room_name_from_owner_aql, query_room_name_from_refno_aql, query_room_name_from_refnos_aql};
use crate::data_center_api::bran::atta::get_data_center_atta_attr;
use crate::data_center_api::bran::bend::get_dq_bend_data;
use crate::data_center_api::bran::cap::get_data_center_cap_attr;
use crate::data_center_api::bran::coup::get_data_center_coup_attr;
use crate::data_center_api::bran::cros::{get_data_center_cros_attr, get_dq_cros_data};
use crate::data_center_api::bran::elbo::get_data_center_elbo_attr;
use crate::data_center_api::bran::flan::get_data_center_flan_attr;
use crate::data_center_api::bran::ftub::*;
use crate::data_center_api::bran::gask::get_data_center_gask_attr;
use crate::data_center_api::bran::olet::get_data_center_olet_attr;
use crate::data_center_api::bran::redu::{get_data_center_redu_attr, get_dq_redu_data};
use crate::data_center_api::bran::tee::{get_data_center_tee_attr, get_dq_tee_data};
use crate::data_center_api::bran::tubi::get_data_center_tubi_attr;
use crate::data_center_api::bran::weld::get_data_center_weld_attr;
use crate::data_center_api::data_api::*;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::ArDatabase;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;

/// 获取 管段元数据
pub fn get_data_center_bran_attr(refno: RefU64) -> Vec<DataCenterAttr> {
    let mut result = Vec::new();
    let segma_1 = vec![1, 2, 3, 4];
    let segma_2 = "TEST".to_string();
    let segma_3 = "安装基准点".to_string();
    let segma_4 = "TEST".to_string();
    let segma_5 = "TEST_TEST_TEST_TEST".to_string();
    let segma_6 = "TEST_TEST_TEST_TEST".to_string();
    let segma_7 = "TEST_TEST_TEST_TEST".to_string();
    let mut map = HashMap::new();
    map.insert("流向1".to_string(), vec!["支吊架编号1".to_string(), "支吊架编号2".to_string()]);
    let segma_8 = map;
    let segma_9 = "TEST".to_string();
    let segma_10 = "TEST_TEST_TEST_TEST".to_string();

    result.push(DataCenterAttr {
        attribute_model_code: "SEGMA1".to_string(),
        value: AttrIntArray(segma_1).into(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: "SEGMA2".to_string(),
        value: AttrString(segma_2).into(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: "SEGMA3".to_string(),
        value: AttrString(segma_3).into(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: "SEGMA4".to_string(),
        value: AttrString(segma_4).into(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: "SEGMA5".to_string(),
        value: AttrString(segma_5).into(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: "SEGMA6".to_string(),
        value: AttrString(segma_6).into(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: "SEGMA7".to_string(),
        value: AttrString(segma_7).into(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: "SEGMA8".to_string(),
        value: AttrMap(segma_8).into(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: "SEGMA9".to_string(),
        value: AttrString(segma_9).into(),
    });
    result.push(DataCenterAttr {
        attribute_model_code: "SEGMA10".to_string(),
        value: AttrString(segma_10).into(),
    });
    result
}

/// 给排水专业 获取 bran和pipe的name
pub async fn get_sg_pipe_bran_name(refnos: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<DataCenterProject> {
    let mut result = Vec::new();
    if let Ok(children) = query_refnos_travel_children_with_type_aql(&database, &refnos,
                                                                     vec!["PIPE".to_string()]).await {
        for pipe in children {
            let brans = query_children_eles(&database, pipe.refno).await?;
            for bran in brans {
                if bran.noun != "BRAN" { continue; };
                let mut attr = Vec::new();
                attr.push(DataCenterAttr {
                    attribute_model_code: "SEGMA1".to_string(),
                    value: bran.name.clone(),
                });
                attr.push(DataCenterAttr {
                    attribute_model_code: "SEGMA2".to_string(),
                    value: pipe.name.clone(),
                });
                result.push(DataCenterInstance {
                    object_model_code: "SEGMA".to_string(),
                    project_code: "1516".to_string(),
                    instance_code: bran.name,
                    version: get_refno_latest_version(),
                    attributes: attr,
                })
            }
        }
    }
    Ok(DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: "1516".to_string(),
        owner: "KY1801".to_string(),
        instances: result,
    })
}

/// 获取电气专业的 bran 下面的元件信息
pub async fn get_dq_bran_data(refnos: &[RefU64], aios_mgr: &AiosDBManager) -> anyhow::Result<DataCenterProject> {
    let mut result = Vec::new();
    let database = aios_mgr.get_arango_db().await?;

    if let Ok(children) = query_refnos_travel_children_with_type_aql(&database, &refnos,
                                                                     vec!["BRAN".to_string()]).await {
        let bran_refnos = children.iter().map(|c| c.refno).collect::<Vec<RefU64>>();
        // 通过 owner返回房间号（只返回一个）
        let room_name_map = query_room_name_from_owner_aql(bran_refnos.clone(), &database).await?;
        let room_name_map = room_name_map.into_iter()
            .map(|r| (r.refno, r.room_name))
            .collect::<HashMap<RefU64, String>>();
        // 查找bran所属专业
        let major_map = query_refnos_belong_major(bran_refnos, &database).await?;
        let major_map = major_map.into_iter().map(|x| (x.refno.clone(), x)).collect::<HashMap<String, RefnoMajor>>();
        // 找到每个bran所属的site的name
        for bran in children {
            let bran_children = aios_mgr.query_children_eles_order(bran.refno, &vec![], &vec![]).await?;
            let bran_children_refnos = bran_children.iter().map(|x| x.refno).collect::<Vec<_>>();
            let room_name = query_room_name_from_refnos_aql(bran_children_refnos, &database).await.unwrap_or(vec![]);
            let bran_children_room_map = room_name
                .into_iter()
                .map(|x| (x.refno, x.room_name))
                .collect::<HashMap<RefU64, String>>();

            let bran_attr = aios_mgr.get_attr(bran.refno).await?;
            let room_name = room_name_map.get(&bran.refno).unwrap_or(&"".to_string()).clone();
            let pspe_name = query_foreign_name_aql(bran.refno, vec!["PSPE", "PSPE"], &database).await?;
            let mut kind = "".to_string();
            if let Some(ref pspe_name) = pspe_name {
                match pspe_name {
                    s if s.contains("Ladder") => { kind = "梯架".to_string() }
                    s if s.contains("Ventilated") => { kind = "带孔托盘".to_string() }
                    s if s.contains("Trough") => { kind = "实底托盘".to_string() }
                    s if s.contains("Riser") => { kind = "竖梯".to_string() }
                    s if s.contains("Divider") => { kind = "分隔板".to_string() }
                    _ => {}
                }
            }

            let mut tray_width = "".to_string();
            let mut tray_height = "".to_string();
            let mut bridge_dir = "".to_string();
            let mut b_climbing = false;
            let mut b_wheel = false;
            let mut b_find_ftub = false;
            let mut b_cover = false;
            let mut ftub_paras = vec![];
            let mut bend_para_11 = 0.0;
            // 找到bran下的第一个ftub
            for (idx, child) in bran_children.iter().enumerate() {
                if child.noun == "ATTA" { continue; }
                // BRAN下第一个元件（去除ATTA）的DESP[3]为1,则为是，否则为否。
                if idx == 0 {
                    let desp = get_refno_desp(child.refno, aios_mgr).await.unwrap_or(vec![]);
                    if desp.len() > 4 {
                        if desp[3] == 1.0 {
                            b_cover = true;
                        }
                    }
                }
                if bridge_dir.is_empty() {
                    let spre_name = query_foreign_name_aql(child.refno, vec!["SPRE", "SPRE"], &database).await?;
                    if let Some(spre_name) = spre_name {
                        if spre_name.contains("Riser") || spre_name.contains("RDivider") {
                            bridge_dir = "竖向".to_string();
                        } else {
                            bridge_dir = "水平".to_string();
                        }
                    }
                }

                if child.noun == "FTUB" {
                    if !b_find_ftub {
                        let paras = get_refno_paras(child.refno, aios_mgr)?;
                        tray_width = paras.get(0).unwrap_or(&0.0).to_string();
                        tray_height = paras.get(1).unwrap_or(&0.0).to_string();
                        b_find_ftub = true;
                        ftub_paras = paras;
                    }
                }
                if !b_climbing {
                    if child.noun == "ELBO" {
                        b_climbing = true;
                    }
                }
                if !b_wheel {
                    if child.noun == "BEND" {
                        // 找BRAN下面的BEND取PARA11
                        if b_wheel == false {
                            let paras = get_refno_paras(child.refno, aios_mgr)?;
                            let para_11 = paras.get(10).unwrap_or(&0.0);
                            bend_para_11 = *para_11;
                        }
                        b_wheel = true;
                    }
                }
            }

            let bran_name_split = bran.name.split("-").collect::<Vec<_>>();
            let mut color = "".to_string();
            let mut b_paint = false;
            if bran_name_split.len() >= 2 {
                color = bran_name_split[1].to_string();
                if color == "CO" { b_paint = true }
            }
            let desc = get_refno_desc(bran.refno, aios_mgr).await?;
            let b_partition = if desc.is_empty() { false } else { true };

            let mut erecb_attr = Vec::new();
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB1".to_string(),
                value: AttrValue::AttrString(bran.name.clone()).into(),
            });
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB2".to_string(),
                value: AttrValue::AttrString("".to_string()).into(),
            });
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB3".to_string(),
                value: AttrValue::AttrString(room_name.clone()).into(),
            });
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB4".to_string(),
                value: AttrValue::AttrString("".to_string()).into(),
            });
            // 获取site的name和desc
            let site_refno = aios_mgr.get_ancestor_refno_of_type_data(bran.refno, "SITE")?;
            let site_attr = aios_mgr.get_attr(site_refno).await?;
            let site_name = site_attr.get_name().unwrap_or("".to_string());
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB7".to_string(),
                value: AttrValue::AttrString(site_name.split("-").collect::<Vec<_>>().get(0).unwrap_or(&"").to_string()).into(),
            });
            let desc = site_attr.get_str("DESC").unwrap_or("").to_string();
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB8".to_string(),
                value: AttrValue::AttrString(desc).into(),
            });

            let major_info = major_map.get(&bran.refno.to_refno_string()).map_or(RefnoMajor::default(), |x| x.clone());
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB9".to_string(),
                value: AttrValue::AttrString(major_info.major.to_string()).into(),
            });

            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB10".to_string(),
                value: AttrValue::AttrString(major_info.major_classify.to_string()).into(),
            });
            let bran_name_split = bran.name.split("-").collect::<Vec<_>>();
            // 判断BRAN的NAME按-分割从0开始数第0位，LV：低压，MV：中压，M：测量，C：控制，IED：IED
            let bran_name_first = bran_name_split.get(0).map_or("", |x| x);
            let bran_type = match_bran_type(bran_name_first);
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB16".to_string(),
                value: AttrValue::AttrString(bran_type).into(),
            });
            // NAME按“-”分割取第1个字符，可配置。GR,IY:B序列,OR,CO:A序列,YE:保护组I,BL:保护组II，RE：保护组III，BR:保护组VI，PU:PAMS1，TU:PAMS2
            let bran_name_first = bran_name_split.get(1).map_or("", |x| x);
            let bran_series = match_bran_series(bran_name_first);
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB17".to_string(),
                value: AttrValue::AttrString(bran_series).into(),
            });
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB21".to_string(),
                value: AttrValue::AttrString(format!("{}mm{}", tray_width, kind)).into(),
            });
            // HPOS TPOS
            let hpos = bran_attr.get_f64_vec("HPOS").unwrap_or(vec![]);
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB23".to_string(),
                value: AttrValue::AttrFloatArray(hpos.into_iter().map(|x| x as f32).collect()).into(),
            });
            let tpos = bran_attr.get_f64_vec("TPOS").unwrap_or(vec![]);
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB24".to_string(),
                value: AttrValue::AttrFloatArray(tpos.into_iter().map(|x| x as f32).collect()).into(),
            });

            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB25".to_string(),
                value: AttrValue::AttrBool(b_climbing).into(),
            });
            // 求两个ELBO之间FTUB的长度
            let mut distance = 0.0;
            if b_climbing {
                let ftub = match_ftub_between_elbo(&bran_children);
                if let Some(ftub) = ftub {
                    let arrive_leave_map = get_refnos_arrive_leave_info(vec![ftub], false, aios_mgr).await.unwrap_or_default();
                    if let Some(arrive_leave_info) = arrive_leave_map.get(&ftub) {
                        let arrive_pt = arrive_leave_info.get(&"ARRIVE_POINT".to_string())
                            .map_or(NamedAttrValue::Vec3Type(Vec3::ZERO), |pt| pt.clone());
                        let leave_pt = arrive_leave_info.get(&"LEAVE_POINT".to_string())
                            .map_or(NamedAttrValue::Vec3Type(Vec3::ZERO), |pt| pt.clone());

                        if let NamedAttrValue::Vec3Type(arrive) = arrive_pt {
                            if let NamedAttrValue::Vec3Type(leave) = leave_pt {
                                distance = arrive.distance(leave);
                            }
                        }
                    }
                }
            }
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB26".to_string(),
                value: AttrValue::AttrFloat(distance).into(),
            });
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB27".to_string(),
                value: AttrValue::AttrBool(b_wheel).into(),
            });
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB28".to_string(),
                value: AttrValue::AttrFloat(bend_para_11 as f32).into(),
            });
            let cable_weight = math_cable_weight(&kind, &tray_width);
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB29".to_string(),
                value: AttrValue::AttrString(cable_weight).into(),
            });
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB31".to_string(),
                value: AttrValue::AttrString(kind.clone()).into(),
            });
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB32".to_string(),
                value: AttrValue::AttrString(color.to_string()).into(),
            });
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB33".to_string(),
                value: AttrValue::AttrBool(b_paint).into(),
            });
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB34".to_string(),
                value: AttrValue::AttrBool(b_partition).into(),
            });
            erecb_attr.push(DataCenterAttr {
                attribute_model_code: "ERECB35".to_string(),
                value: AttrValue::AttrString(bridge_dir.to_string()).into(),
            });
            result.push(DataCenterInstance {
                object_model_code: "ERECB".to_string(),
                project_code: aios_mgr.db_option.project_code.to_string(),
                instance_code: bran.name.clone(),
                version: get_refno_latest_version(),
                attributes: erecb_attr,
            });

            // 获取 bran下元件的数据
            let mut point_type = vec!["TEE".to_string(), "BEND".to_string()];
            let need_query_point = bran_children.iter()
                .filter(|child| point_type.contains(&child.noun))
                .map(|child| child.refno).collect::<Vec<RefU64>>();
            let points_map = query_refnos_point_map_aql(need_query_point, &database).await.unwrap_or(vec![]);
            let points_map = points_map.into_iter().map(|x| (x.refno, x)).collect::<HashMap<RefU64, InstPointMap>>();

            let bran_refnos = bran_children.iter().map(|child| child.refno).collect::<Vec<RefU64>>();
            let bran_children_spre = query_foreign_refnos_aql(&database, bran_refnos, vec!["SPRE".to_string(), "SPRE".to_string()]).await?;
            let children_spre_map = bran_children_spre.into_iter()
                .filter(|c| RefU64::from_url_refno(&c.refno).is_some())
                .map(|e| (RefU64::from_url_refno(&e.refno).unwrap(), e.name))
                .collect::<HashMap<RefU64, String>>();
            let regex = Regex::new(r"(\d+):")?; // 判断字符串是否包含有多个数字加一个:
            for child in bran_children {
                // let spre_name = query_foreign_name_aql(child.refno, vec!["SPRE", "SPRE"], &database).await?.unwrap_or_default();
                let spre_name = children_spre_map.get(&child.refno).unwrap_or(&"".to_string()).clone();
                let room_name = bran_children_room_map.get(&child.refno).map_or("".to_string(), |x| x.to_string());
                match child.noun.as_str() {
                    "FTUB" => {
                        if spre_name.contains("RISER") {
                            let Ok(r) = get_dq_ftub_contains_riser_data(&child, &bran.name, &spre_name, &room_name, &ftub_paras,
                                                                            aios_mgr).await else { continue; };
                            result.push(r);
                        } else if spre_name.contains("RDivider") {
                            let Ok(r) = get_dq_ftub_contains_rdivider_data(&child, &bran.name, &spre_name, &room_name, &ftub_paras,
                                                                            &points_map, aios_mgr).await else { continue; };
                            result.push(r);
                        } else {
                            let Ok(r) = get_dq_ftub_data(&child, &bran.name, &spre_name, &room_name,
                                                         &kind, b_cover, &points_map, aios_mgr).await else { continue; };
                            result.push(r);
                        }
                    }
                    "TEE" => {
                        let Ok(r) = get_dq_tee_data(&child, &bran.name, &spre_name, &room_name, &ftub_paras,
                                                    &kind, b_cover, &points_map, aios_mgr).await else { continue; };
                        result.push(r);
                    }
                    "CROS" => {
                        let Ok(r) = get_dq_cros_data(&child, &bran.name, &spre_name, &room_name, &ftub_paras,
                                                    &kind, b_cover, &points_map, aios_mgr).await else { continue; };
                        result.push(r);
                    }
                    "REDU" => {
                        let Ok(r) = get_dq_redu_data(&child, &bran.name, &spre_name, &room_name, &ftub_paras,
                                                     &kind, b_cover, &points_map, aios_mgr).await else { continue; };
                        result.push(r);
                    }
                    "BEND" => {
                        if regex.is_match(&spre_name) {
                            let Ok(r) = get_dq_bend_data(&child, &bran.name, &spre_name, &room_name, &ftub_paras,
                                                         &kind, b_cover, &points_map, aios_mgr).await else { continue; };
                            result.push(r);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        owner: "KY1801".to_string(),
        instances: result,
    })
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct CableWeightExcel {
    pub types: Option<String>,
    pub width: Option<String>,
    /// 托盘重量
    pub tray_weight: Option<String>,
    /// 电缆线重
    pub cable_weight: Option<String>,
}

impl CableWeightExcel {
    pub fn is_null(&self) -> bool {
        if self.types.is_none() || self.width.is_none() || self.tray_weight.is_none() || self.cable_weight.is_none() {
            true
        } else {
            false
        }
    }
}

/// 读取 电缆桥架及电缆线重 表
pub async fn read_cable_weight_excel() -> anyhow::Result<HashMap<String, HashMap<String, CableWeight>>> {
    let mut map = HashMap::new();
    let mut workbook: Xlsx<_> = open_workbook("./resource/电缆桥架及电缆线重.xlsx")?;
    let range = workbook.worksheet_range("Sheet1")
        .ok_or(anyhow::anyhow!("Cannot find Sheet 'Sheet1'"))??;

    let mut iter = RangeDeserializerBuilder::new().from_range(&range)?;
    while let Some(result) = iter.next() {
        let v: CableWeightExcel = result?;
        if v.is_null() { break; }
        let types = v.types.unwrap();
        let width = v.width.unwrap();
        let tray_weight = v.tray_weight.unwrap();
        let cable_weight = v.cable_weight.unwrap();
        let cable = CableWeight {
            types: types.clone(),
            width: width.clone(),
            tray_weight,
            cable_weight,
        };
        map.entry(types).or_insert_with(HashMap::new).entry(width).or_insert(cable);
    }
    Ok(map)
}

/// 获取工艺管件数据(数据中台)
pub async fn query_gy_bran_data_datacenter(select_refno: RefU64, aios_mgr: &AiosDBManager) -> anyhow::Result<DataCenterProject> {
    let mut instances = Vec::new();
    let database = aios_mgr.get_arango_db().await?;
    let brans = query_travel_children_with_type_aql(&database, select_refno, "BRAN").await?;
    for bran in brans {
        let children = query_children_order_aql(&database, bran.refno).await?;
        //  bran 下的元件
        for child in children {
            match child.noun.clone().as_str() {
                "ATTA" => {
                    let instance = get_data_center_atta_attr(child, &bran.name, &database, aios_mgr).await;
                    instances.push(instance);
                }
                "ELBO" => {
                    let instance = get_data_center_elbo_attr(child, &bran.name, &database, aios_mgr).await;
                    instances.push(instance);
                }
                "CAP" => {
                    let instance = get_data_center_cap_attr(child, &bran.name, &database, aios_mgr).await;
                    instances.push(instance);
                }
                "COUP" => {
                    let instance = get_data_center_coup_attr(child, &bran.name, &database, aios_mgr).await;
                    instances.push(instance);
                }
                "CROS" => {
                    let instance = get_data_center_cros_attr(child, &bran.name, &database, aios_mgr).await;
                    instances.push(instance);
                }
                "FLAN" => {
                    let instance = get_data_center_flan_attr(child, &bran.name, &database, aios_mgr).await;
                    instances.push(instance);
                }
                "GASK" => {
                    let instance = get_data_center_gask_attr(child, &bran.name, &database, aios_mgr).await;
                    instances.push(instance);
                }
                "OLET" => {
                    let instance = get_data_center_olet_attr(child, &bran.name, &database, aios_mgr).await;
                    instances.push(instance);
                }
                "REDU" => {
                    let instance = get_data_center_redu_attr(child, &bran.name, &database, aios_mgr).await;
                    instances.push(instance);
                }
                "TEE" => {
                    let instance = get_data_center_tee_attr(child, &bran.name, &database, aios_mgr).await;
                    instances.push(instance);
                }
                "WELD" => {
                    let instance = get_data_center_weld_attr(child, &bran.name, &database, aios_mgr).await;
                    instances.push(instance);
                }
                _ => {}
            }
        }
        // tubi
        let mut tubi_instances = get_data_center_tubi_attr(bran.refno, &bran.name, &database, aios_mgr).await;
        instances.append(&mut tubi_instances);
    }
    Ok(DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        owner: "KY1801".to_string(),
        instances,
    })
}

/// 返回bran中 两个elbo之间相邻的ftub（排除atta），且只返回一个
fn match_ftub_between_elbo(bran_children: &Vec<PdmsElement>) -> Option<RefU64> {
    let children = bran_children.iter().filter(|child| &child.noun != "ATTA").collect::<Vec<_>>();
    let mut idx = 0;
    let children_len = children.len();
    for child in &children {
        if child.noun == "ELBO".to_string() || child.noun == "BEND".to_string() {
            if idx + 2 > children_len { return None; };
            let next_node = children[idx + 1].noun == "FTUB".to_string();
            let next_next_node = children[idx + 2].noun == "ELBO".to_string() || children[idx + 2].noun == "BEND".to_string();
            if next_node && next_next_node {
                return Some(children[idx + 1].refno);
            }
        }
        idx += 1;
    }
    return None;
}

/// 匹配桥架类型
fn match_bran_type(input: &str) -> String {
    match input {
        s if s.starts_with("/LV") => { "低压".to_string() }
        s if s.starts_with("/MV") => { "中压".to_string() }
        s if s.starts_with("/M") => { "测量".to_string() }
        s if s.starts_with("/C") => { "控制".to_string() }
        s if s.starts_with("/IED") => { "IED".to_string() }
        _ => { "".to_string() }
    }
}

/// 匹配bran的系列
fn match_bran_series(bran_name: &str) -> String {
    let bran_name_split = bran_name.split("-").collect::<Vec<_>>();
    if bran_name_split.len() < 2 { return "".to_string(); }
    let split = bran_name_split[1];
    match split {
        "GR" | "IY" => { "B序列".to_string() }
        "OR" | "CO" => { "A序列".to_string() }
        "YE" => { "保护组I".to_string() }
        "BL" => { "保护组II".to_string() }
        "RE" => { "保护组III".to_string() }
        "BR" => { "保护组VI".to_string() }
        "PU" => { "PAMS1".to_string() }
        "TU" => { "PAMS2".to_string() }
        _ => { "".to_string() }
    }
}

fn math_cable_weight(tray_name: &str, tray_width: &str) -> String {
    match tray_name {
        "梯架" => {
            match tray_width {
                "600" => "100".to_string(),
                "500" => "75".to_string(),
                "300" => "45".to_string(),
                _ => "0.0".to_string()
            }
        }
        "带孔托盘" => {
            match tray_width {
                "200" => "30".to_string(),
                "100" => "15".to_string(),
                _ => "0.0".to_string()
            }
        }
        "实底托盘" => {
            match tray_width {
                "600" => "100".to_string(),
                "500" => "75".to_string(),
                "300" => "45".to_string(),
                "100" => "15".to_string(),
                "50" => "7.5".to_string(),
                _ => "0.0".to_string()
            }
        }
        _ => "0.0".to_string()
    }
}

#[tokio::test]
async fn test_query_gy_bran_data_datacenter() -> anyhow::Result<()> {
    let aios_mgr = AiosDBManager::init_form_config().await?;
    let tee_refno = RefU64::from_refno_str("24383/66761").unwrap();
    let result = query_gy_bran_data_datacenter(tee_refno, &aios_mgr).await?;
    let mut file = std::fs::File::create("bran.json")?;
    let json = serde_json::to_vec(&result)?;
    file.write_all(&json)?;
    Ok(())
}

#[tokio::test]
async fn test_query_dq_bran_data_datacenter() -> anyhow::Result<()> {
    let aios_mgr = AiosDBManager::init_form_config().await?;
    let bran_refno = vec![RefU64::from_refno_str("24383/84157").unwrap()];
    let result = get_dq_bran_data(&bran_refno, &aios_mgr).await?;
    let mut file = std::fs::File::create("data_center_test/dq_bran.json")?;
    let json = serde_json::to_vec(&result)?;
    file.write_all(&json)?;
    Ok(())
}

#[tokio::test]
async fn test_match_ftub_between_elbo() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let refno = RefU64::from_refno_str("24383/84088").unwrap();
    let children = query_children_order_aql(&database, refno).await?;
    let ftub = match_ftub_between_elbo(&children);
    dbg!(&ftub);
    let error_refno = RefU64::from_refno_str("24383/84151").unwrap();
    let children = query_children_order_aql(&database, error_refno).await?;
    let ftub = match_ftub_between_elbo(&children);
    dbg!(&ftub);
    Ok(())
}