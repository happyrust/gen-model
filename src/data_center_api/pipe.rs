use std::array::from_ref;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::File;
use std::io::Write;
use std::sync::Arc;
use aios_core::data_center::{DataCenterAttr, DataCenterInstance, DataCenterProject, DataCenterProjectWithRelations, DataCenterRelations};
use aios_core::data_center::AttrValue::AttrString;
use aios_core::pdms_types::{PdmsElement, RefU64};
use arangors_lite::Database;
use bevy::render::render_resource::encase::private::RuntimeSizedArray;
use sqlx::{MySql, Pool};
use crate::api::attr::query_implicit_attr;
use crate::api::element::{query_children, query_children_eles, query_children_eles_without_children_count, query_name, query_refno_type};
use crate::api::metadata_manage::{query_metadata_table_code_sql, query_metadata_table_sql};
use crate::aql_api::children::{query_children_aql, query_children_refnos_aql};
use crate::aql_api::tubi::{query_bran_info, query_tubi_from_bran};
use crate::data_center_api::bran::get_data_center_bran_attr;
use crate::data_center_api::elbo::get_data_center_elbo_attr;
use crate::data_center_api::flan::get_data_center_flan_attr;
use crate::data_center_api::redu::get_data_center_redu_attr;
use crate::data_center_api::tee::get_data_center_tee_attr;
use crate::data_center_api::tubi::get_data_center_tubi_attr;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::{AiosDBManager, TUBI_TOL};
use crate::metadata::{convert_str_to_hash, get_characters_in_str};

macro_rules! query_metadata {
    ($item:expr,$att_type:expr, $pool:expr,$map:expr) => {{
        let hash = convert_str_to_hash($item);
        let attr = query_metadata_table_code_sql(hash, $pool).await?;
        $map.insert($att_type,attr);
    }};
}

/// 找到 管段 所有需要统计的数据
pub async fn get_all_metadata_pipe(pool: &Pool<MySql>) -> anyhow::Result<HashMap<String, Vec<String>>> {
    let mut pipe_metadata_map = HashMap::new();
    query_metadata!("HSEGMA","BRAN".to_string(),&pool,pipe_metadata_map);
    query_metadata!("IITEMAA","TUBE".to_string(),&pool,pipe_metadata_map);
    query_metadata!("IITEMAB","TEE".to_string(),&pool,pipe_metadata_map);
    query_metadata!("IITEMAC","CROS".to_string(),&pool,pipe_metadata_map);
    query_metadata!("IITEMAD","ELBO".to_string(),&pool,pipe_metadata_map);
    query_metadata!("IITEMAE","FLAN".to_string(),&pool,pipe_metadata_map);
    query_metadata!("IITEMAG","COUP".to_string(),&pool,pipe_metadata_map);
    query_metadata!("IITEMAH","OLET".to_string(),&pool,pipe_metadata_map);
    query_metadata!("IITEMAJ","REDU".to_string(),&pool,pipe_metadata_map);
    query_metadata!("IITEMAK","CAP".to_string(),&pool,pipe_metadata_map);
    query_metadata!("IITEMAL","GASK".to_string(),&pool,pipe_metadata_map);
    query_metadata!("FCOMPAA","EQUI".to_string(),&pool,pipe_metadata_map);
    Ok(pipe_metadata_map)
}

pub async fn get_data_center_from_pipe(aios_mgr: &AiosDBManager, pipe_refno: RefU64) -> anyhow::Result<()> {
    let pool = aios_mgr.get_project_pool_by_refno(pipe_refno).await;
    if pool.is_none() { return Ok(()); }
    let (_, pool) = pool.unwrap();
    let database = aios_mgr.get_arangodb_conn().await?;
    let bran_refnos = query_children_eles_without_children_count(pipe_refno, &pool).await?;
    let (need_compute_bran_refnos, ref_map) = get_bran_data(&bran_refnos, aios_mgr).await?;
    let metadata_map = get_all_metadata_pipe(&pool).await?;
    let (instance_map, element_map, bran_children_map) =
        get_instances_data(need_compute_bran_refnos, metadata_map, &pool, &database).await?;
    let mut relations = get_relations_data(&bran_refnos, &instance_map, &ref_map, element_map, bran_children_map);
    // 统一给 relations 赋上流水号
    for (idx, relation) in relations.iter_mut().enumerate() {
        relation.instance_code = format!("{}{}", &relation.object_model_code, idx);
    }
    let instances = instance_map.into_iter().map(|x| x.1).collect::<Vec<DataCenterInstance>>();
    let data = DataCenterProjectWithRelations {
        project_code: "1516".to_string(),
        owner: "KY1801-208".to_string(),
        instances,
        relations,
    };
    let mut file = File::create("管段.json")?;
    let json = serde_json::to_string(&data).unwrap();
    file.write_all(&json.into_bytes())?;
    Ok(())
}

/// 找到所有需要收集的bran以及href tref 的参考号
async fn get_bran_data(bran_refnos: &Vec<PdmsElement>, aios_mgr: &AiosDBManager) -> anyhow::Result<(HashMap<String, HashSet<RefU64>>, HashMap<RefU64, (RefU64, RefU64)>)> {
    let mut map = HashMap::new();
    let mut ref_map = HashMap::new(); // 存放每个bran 的 href 和 tref
    for bran_refno in bran_refnos {
        let bran_refno = RefU64::from_refno_str(&bran_refno.refno);
        if bran_refno.is_err() { continue; }
        let bran_refno = bran_refno.unwrap();
        let pool = aios_mgr.get_project_pool_by_refno(bran_refno).await;
        if pool.is_none() { continue; }
        let (_, pool) = pool.unwrap();
        let basic = aios_mgr.get_refno_basic(bran_refno);
        if basic.is_none() { return Ok((map, ref_map)); }
        let basic = basic.unwrap();
        map.entry("BRAN".to_string()).or_insert_with(HashSet::new).insert(bran_refno);

        let bran_attr = query_implicit_attr(bran_refno, basic.value(), &pool, Some(vec!["HREF", "TREF"])).await?;
        let href = bran_attr.get_refu64("HREF");
        let tref = bran_attr.get_refu64("TREF");
        if href.is_none() || tref.is_none() { return Ok((map, ref_map)); }
        let href = href.unwrap();
        let tref = tref.unwrap();
        ref_map.entry(bran_refno).or_insert((href, tref));
        if href != RefU64(0) {
            let href_type = query_refno_type(href, &pool).await?;
            map.entry(href_type).or_insert_with(HashSet::new).insert(href);
        }
        if tref != RefU64(0) {
            let tref_type = query_refno_type(tref, &pool).await?;
            map.entry(tref_type).or_insert_with(HashSet::new).insert(tref);
        }
    }
    Ok((map, ref_map))
}

/// ref_map: 每个 bran 对应的 href 和 tref
async fn get_instances_data(compute_refnos: HashMap<String, HashSet<RefU64>>, metadata_map: HashMap<String, Vec<String>>,
                            pool: &Pool<MySql>, database: &Database) -> anyhow::Result<(HashMap<RefU64, DataCenterInstance>, HashMap<RefU64, Vec<RefU64>>, HashMap<RefU64, Vec<RefU64>>)> {
    // let mut bran_instances_map = HashMap::new(); // 将 bran 及其 href tref 的 instance 存在 map 中
    let mut instances = HashMap::new();
    let mut bran_children_map = HashMap::new();
    // let mut relations = Vec::new();
    let mut bran_relations_map = HashMap::new();
    for (att_type, refnos) in compute_refnos.into_iter() {
        let mut b_bran = false;
        if att_type.as_str() == "BRAN" { b_bran = true; }
        for (index, refno) in refnos.into_iter().enumerate() {
            // bran 的 instance_code 是 name ，其他的是 refno
            let instance_code = if b_bran { query_name(refno, pool).await? } else { refno.to_refno_string() };
            let instance = get_instance_data_element(&metadata_map, refno, &att_type, instance_code);
            if instance.is_none() { continue; }
            let instance = instance.unwrap();
            instances.insert(refno, instance);
            // 将 bran 下得元件放进去
            if b_bran {
                let bran_elements = query_bran_info(refno, database).await?;
                let bran_children = query_children_refnos_aql(database, refno).await?;
                for idx in 0..bran_elements.len() {
                    // 放入元件数据
                    let bran_element = &bran_elements[idx];
                    let from_refno = &bran_element._from;
                    let from_refno = RefU64::from_arangodb_refno_str(&from_refno);
                    if from_refno.is_none() { continue; }
                    let from_refno = from_refno.unwrap();
                    bran_children_map.entry(refno).or_insert_with(Vec::new).push(from_refno);
                    if let Some(to_refno) = RefU64::from_arangodb_refno_str(&bran_element._to) {
                        if bran_children.contains(&to_refno) {
                            let element_type = &bran_element.att_type;
                            if let Some(instance) = get_instance_data_element(&metadata_map, to_refno, &element_type, to_refno.to_refno_string()) {
                                bran_relations_map.entry(to_refno).or_insert_with(Vec::new).push(to_refno);
                                instances.insert(to_refno, instance);
                            }
                        }
                    }
                }
                let mut tubi_from_idx = 0;
                let mut tubi_to_idx = 0;
                for idx in 0..bran_elements.len() {
                    if idx < tubi_from_idx { continue; }
                    let distance = bran_elements[tubi_from_idx].start_pt.distance(bran_elements[tubi_to_idx].end_pt);
                    while bran_elements[tubi_to_idx].att_type == "ATTA" && tubi_to_idx < bran_elements.len() - 1 && tubi_from_idx < bran_elements.len() - 1 {
                        tubi_to_idx += 1;
                    }
                    // 将 tube 的数据放进去
                    if distance >= TUBI_TOL {
                        // tube_refno 用 from 的 refno 和 to 的 refno 做一个 hash
                        let from_refno = RefU64::from_arangodb_refno_str(&bran_elements[tubi_from_idx]._from);
                        let to_refno = RefU64::from_arangodb_refno_str(&bran_elements[tubi_to_idx]._to);
                        if from_refno.is_none() || to_refno.is_none() { continue; }
                        let from_refno = from_refno.unwrap();
                        let to_refno = to_refno.unwrap();
                        let tube_refno = RefU64(from_refno.hash_with_another_refno(to_refno));
                        // let tube_code = tube_refno.to_refno_string();
                        let tube_code = format!("from:{} / to:{}", from_refno.to_refno_string(), to_refno.to_refno_string());
                        let instance = get_instance_data_element(&metadata_map, tube_refno, "TUBE", tube_code);
                        if instance.is_none() { continue; }
                        let instance = instance.unwrap();

                        bran_relations_map.entry(from_refno).or_insert_with(Vec::new).push(tube_refno);
                        instances.insert(tube_refno, instance);
                        if tubi_from_idx != tubi_to_idx {
                            tubi_from_idx = tubi_to_idx;
                        }
                    }
                    tubi_from_idx += 1;
                    tubi_to_idx += 1;
                }
            }
        }
    }
    Ok((instances, bran_relations_map, bran_children_map))
}

/// 生成 relations 信息
fn get_relations_data(bran_infos: &Vec<PdmsElement>, instance_map: &HashMap<RefU64, DataCenterInstance>,
                      ref_map: &HashMap<RefU64, (RefU64, RefU64)>, element_map: HashMap<RefU64, Vec<RefU64>>, bran_children_map: HashMap<RefU64, Vec<RefU64>>) -> Vec<DataCenterRelations> {
    let mut relations = Vec::new();
    // let mut bran_infos = HashMap::new();
    // 存 bran 与 bran 之间的关系
    for i in 0..bran_infos.len() - 1 {
        let start_refno = RefU64::from_refno_str(&bran_infos[i].refno);
        let end_refno = RefU64::from_refno_str(&bran_infos[i + 1].refno);
        if start_refno.is_err() || end_refno.is_err() { continue; }
        let start_refno = start_refno.unwrap();
        let end_refno = end_refno.unwrap();

        let start_instance = instance_map.get(&start_refno);
        let end_instance = instance_map.get(&end_refno);
        if start_instance.is_none() || end_instance.is_none() { continue; }
        let start_instance = start_instance.unwrap();
        let end_instance = end_instance.unwrap();

        relations.push(DataCenterRelations {
            version: "A版".to_string(),
            object_model_code: "RELAPOPC".to_string(),
            instance_code: "".to_string(),
            start_object_code: start_instance.object_model_code.clone(),
            start_instance_code: start_instance.instance_code.clone(),
            end_object_code: end_instance.object_model_code.clone(),
            end_instance_code: end_instance.instance_code.clone(),
            attributes: vec![],
        });
    }
    // 存 bran 与 元件之间的关系
    for (bran_refno, bran_children) in &bran_children_map {
        let bran_instances = instance_map.get(&bran_refno);
        if bran_instances.is_none() { continue; }
        let bran_instances = bran_instances.unwrap();
        // 包含 tube
        for child in bran_children {
            let elements = element_map.get(child);
            if elements.is_none() { continue; }
            let elements = elements.unwrap();
            for element in elements {
                let child_instance = instance_map.get(element);
                if child_instance.is_none() { continue; }
                let child_instance = child_instance.unwrap();

                relations.push(DataCenterRelations {
                    version: "A版".to_string(),
                    object_model_code: "RELAPOPC".to_string(),
                    instance_code: "".to_string(),
                    start_object_code: bran_instances.object_model_code.clone(),
                    start_instance_code: bran_instances.instance_code.clone(),
                    end_object_code: child_instance.object_model_code.clone(),
                    end_instance_code: child_instance.instance_code.clone(),
                    attributes: vec![],
                });
            }
        }
    }

    // 第一个元件和 href  最后一个元件和 tref
    for (bran_refno, _bran_children) in &bran_children_map {
        // 获取 bran 的 href 和 tref
        let result = ref_map.get(bran_refno);
        if result.is_none() { continue; }
        let (href, tref) = result.unwrap();
        let href_instance = instance_map.get(href);
        let tref_instance = instance_map.get(tref);
        if href_instance.is_none() || tref_instance.is_none() { continue; }
        let href_instance = href_instance.unwrap();
        let tref_instance = tref_instance.unwrap();
        // 获取 bran 的 children
        let children = bran_children_map.get(bran_refno);
        if children.is_none() { continue; }
        let children = children.unwrap();
        let mut first_idx = 0;
        let mut last_idx = children.len() - 1;
        let mut first_element_opt = None;
        let mut last_element_opt = None;
        while first_element_opt.is_none() || last_element_opt.is_none() {
            let first_element_refno = children.get(first_idx);
            let last_element_refno = children.get(last_idx);
            if first_element_refno.is_none() || last_element_refno.is_none() { break; }
            let first_element_refno = first_element_refno.unwrap();
            let last_element_refno = last_element_refno.unwrap();
            let first_element = element_map.get(first_element_refno);
            let last_element = element_map.get(last_element_refno);
            if first_element_opt.is_none() {
                first_element_opt = first_element;
            }
            if last_element_opt.is_none() {
                last_element_opt = last_element;
            }
            first_idx += 1;
            last_idx -= 1;
            if first_idx == last_idx { break; }
        }

        // 分别赋上 relations
        if let Some(first_elements) = first_element_opt {
            let first_element_instance = instance_map.get(&first_elements[0]);
            if let Some(first_element_instance) = first_element_instance {
                relations.push(DataCenterRelations {
                    version: "A版".to_string(),
                    object_model_code: "RELAPOPC".to_string(),
                    instance_code: "".to_string(),
                    start_object_code: href_instance.object_model_code.clone(),
                    start_instance_code: href_instance.instance_code.clone(),
                    end_object_code: first_element_instance.object_model_code.clone(),
                    end_instance_code: first_element_instance.instance_code.clone(),
                    attributes: vec![],
                });
            }
        }
        if let Some(last_elements) = last_element_opt {
            let last_refno = if last_elements.len() > 1 { &last_elements[1] } else { &last_elements[0] };
            let last_element_instance = instance_map.get(last_refno);
            if let Some(last_element_instance) = last_element_instance {
                relations.push(DataCenterRelations {
                    version: "A版".to_string(),
                    object_model_code: "RELAPOPC".to_string(),
                    instance_code: "".to_string(),
                    start_object_code: last_element_instance.object_model_code.clone(),
                    start_instance_code: last_element_instance.instance_code.clone(),
                    end_object_code: tref_instance.object_model_code.clone(),
                    end_instance_code: tref_instance.instance_code.clone(),
                    attributes: vec![],
                });
            }
        }
    }
    // 元件与元件之间的关系
    for (bran_refno, children) in bran_children_map {
        // 将一个参考号有多个element数据先提出来(tubi)
        for child in &children {
            let element = element_map.get(child);
            if element.is_none() { continue; }
            let elements = element.unwrap();
            if elements.len() <= 1 { continue; }
            let start_element = elements[0];
            let end_element = elements[1];

            let start_instance = instance_map.get(&start_element);
            let end_instance = instance_map.get(&end_element);
            if start_instance.is_none() || end_instance.is_none() { continue; }
            let start_instance = start_instance.unwrap();
            let end_instance = end_instance.unwrap();

            let relation = DataCenterRelations {
                version: "A版".to_string(),
                object_model_code: "RELAPOPC".to_string(),
                instance_code: "".to_string(),
                start_object_code: start_instance.object_model_code.clone(),
                start_instance_code: start_instance.instance_code.clone(),
                end_object_code: end_instance.object_model_code.clone(),
                end_instance_code: end_instance.instance_code.clone(),
                attributes: vec![],
            };
            relations.push(relation);
        }

        let mut start = 0;
        let mut end = 1;
        for idx in 0..children.len() - 1 {
            let start_element = &children.get(start);
            let end_element = &children.get(end);
            if start_element.is_none() || end_element.is_none() { continue; }
            let start_element = start_element.unwrap();
            let end_element = end_element.unwrap();
            let start_element = element_map.get(start_element);
            if start_element.is_none() {
                start += 1;
                end += 1;
                continue;
            }
            let end_element = element_map.get(end_element);
            if end_element.is_none() {
                end += 1;
                continue;
            }
            let start_element = start_element.unwrap().last().unwrap();
            let end_element = end_element.unwrap().first().unwrap();
            let first_instance = instance_map.get(start_element);
            let end_instance = instance_map.get(end_element);
            if first_instance.is_some() && end_instance.is_some() {
                let first_instance = first_instance.unwrap();
                let end_instance = end_instance.unwrap();
                let relation = DataCenterRelations {
                    version: "A版".to_string(),
                    object_model_code: "RELAPOPC".to_string(),
                    instance_code: "".to_string(),
                    start_object_code: first_instance.object_model_code.clone(),
                    start_instance_code: first_instance.instance_code.clone(),
                    end_object_code: end_instance.object_model_code.clone(),
                    end_instance_code: end_instance.instance_code.clone(),
                    attributes: vec![],
                };
                relations.push(relation);
                if start != end {
                    start = end;
                    end += 1;
                } else {
                    start += 1;
                    end += 1;
                }
            }
        }
    }
    relations
}

fn get_instance_data_element(metadata_map: &HashMap<String, Vec<String>>, refno: RefU64, att_type: &str, instance_code: String) -> Option<DataCenterInstance> {
    let metadata_value = metadata_map.get(att_type);
    if metadata_value.is_none() { return None; }
    let metadata_values = metadata_value.unwrap();
    if metadata_values.is_empty() { return None; }
    let code = get_characters_in_str(&metadata_values[0]);
    let result = match att_type.to_uppercase().as_str() {
        "BRAN" => { get_data_center_bran_attr(refno) }
        "ELBO" => { get_data_center_elbo_attr(refno) }
        "FLAN" => { get_data_center_flan_attr(refno) }
        "REDU" => { get_data_center_redu_attr(refno) }
        "TEE" => { get_data_center_tee_attr(refno) }
        "TUBE" | "TUBI" => { get_data_center_tubi_attr(refno) }
        &_ => { Vec::new() }
    };
    Some(DataCenterInstance {
        object_model_code: code,
        instance_code,
        attributes: result,
    })
}

#[tokio::test]
async fn test_get_all_metadata_pipe() {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL").unwrap();
    let pool = AiosDBManager::get_db_pool(&url, "AvevaMarineSample").await.unwrap();
    let map = get_all_metadata_pipe(&pool).await.unwrap();
    dbg!(&map);
}

#[tokio::test]
async fn test_get_data_center_from_pipe() -> anyhow::Result<()> {
    // let pipe_refno = RefU64::from_refno_str("24383/67155")?;
    let pipe_refno = RefU64::from_refno_str("24383/67116")?;
    let mut mgr = Arc::new(AiosDBManager::init_form_config().await?);
    get_data_center_from_pipe(&mgr, pipe_refno).await?;
    Ok(())
}