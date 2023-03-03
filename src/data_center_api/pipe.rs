use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::Write;
use std::sync::Arc;
use aios_core::data_center::{DataCenterAttr, DataCenterInstance, DataCenterProject, DataCenterProjectWithRelations, DataCenterRelations};
use aios_core::data_center::AttrValue::AttrString;
use aios_core::pdms_types::{PdmsElement, RefU64};
use arangors_lite::Database;
use sqlx::{MySql, Pool};
use crate::api::attr::query_implicit_attr;
use crate::api::element::{query_children_eles, query_children_eles_without_children_count, query_refno_type};
use crate::api::metadata_manage::{query_metadata_table_code_sql, query_metadata_table_sql};
use crate::aql_api::children::query_children_aql;
use crate::aql_api::tubi::{query_bran_info, query_tubi_from_bran};
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
    let bran_refnos = query_children_aql(&database, pipe_refno).await?;
    let (need_compute_bran_refnos,ref_map) = get_bran_data(&bran_refnos, aios_mgr).await?;
    let metadata_map = get_all_metadata_pipe(&pool).await?;
    let data = get_instances_data(need_compute_bran_refnos, metadata_map, ref_map,&database).await?;

    let mut file = File::create("管段.json")?;
    let json = serde_json::to_string(&data).unwrap();
    file.write_all(&json.into_bytes())?;
    Ok(())
}

/// 找到所有需要收集的bran以及href tref 的参考号
async fn get_bran_data(bran_refnos: &Vec<PdmsElement>, aios_mgr: &AiosDBManager) -> anyhow::Result<(HashMap<String, Vec<RefU64>>,HashMap<RefU64, (RefU64, RefU64)>)> {
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
        if basic.is_none() { return Ok((map,ref_map)); }
        let basic = basic.unwrap();
        map.entry("BRAN".to_string()).or_insert_with(Vec::new).push(bran_refno);

        let bran_attr = query_implicit_attr(bran_refno, basic.value(), &pool, Some(vec!["HREF", "TREF"])).await?;
        let href = bran_attr.get_refu64("HREF");
        let tref = bran_attr.get_refu64("TREF");
        if href.is_none() || tref.is_none() { return Ok((map,ref_map)); }
        let href = href.unwrap();
        let tref = tref.unwrap();
        ref_map.entry(bran_refno).or_insert((href,tref));
        if href != RefU64(0) {
            let href_type = query_refno_type(href, &pool).await?;
            map.entry(href_type).or_insert_with(Vec::new).push(href);
        }
        if tref != RefU64(0) {
            let tref_type = query_refno_type(tref, &pool).await?;
            map.entry(tref_type).or_insert_with(Vec::new).push(tref);
        }
    }
    Ok((map,ref_map))
}

/// ref_map: 每个 bran 对应的 href 和 tref
async fn get_instances_data(compute_refnos: HashMap<String, Vec<RefU64>>, metadata_map: HashMap<String, Vec<String>>, ref_map: HashMap<RefU64, (RefU64, RefU64)>,database: &Database) -> anyhow::Result<DataCenterProjectWithRelations> {
    let mut bran_instances_map = HashMap::new(); // 将 bran 及其 href tref 的 instance 存在 map 中
    let mut instances = Vec::new();
    let mut relations = Vec::new();
    for (att_type, refnos) in compute_refnos.into_iter() {
        for (index, refno) in refnos.into_iter().enumerate() {
            let instance = get_instance_data_element(&metadata_map, &att_type, index);
            if instance.is_none() { continue; }
            let mut instance = instance.unwrap();
            instance.instance_code = refno.to_refno_string();
            instances.push(instance.clone());
            bran_instances_map.entry(refno).or_insert(instance);
            // 将 bran 下得元件放进去
            if att_type.as_str() == "BRAN" {
                let bran_elements = query_bran_info(refno, database).await?;
                let mut index = 0;
                let mut bran_instance = Vec::new();
                for bran_element in bran_elements {
                    let distance = bran_element.start_pt.distance(bran_element.end_pt);
                    // 将 tube 的数据放进去
                    if distance >= TUBI_TOL {
                        let instance = get_instance_data_element(&metadata_map, "TUBE", index);
                        if instance.is_none() { continue; }
                        let instance = instance.unwrap();
                        instances.push(instance.clone());
                        bran_instance.push(instance);
                        index += 1;
                    }
                    // 放入元件数据
                    let element_type = bran_element.att_type;
                    let instance = get_instance_data_element(&metadata_map, &element_type, index);
                    if instance.is_none() { continue; }
                    let instance = instance.unwrap();
                    instances.push(instance.clone());
                    bran_instance.push(instance);
                    index += 1;
                }
                // 存储一个bran下 relations 的信息
                if bran_instance.len() > 1 {
                    for i in 1..bran_instance.len() - 1 {
                        let from_code = &bran_instance[i - 1];
                        let to_code = &bran_instance[i + 1];
                        relations.push(DataCenterRelations {
                            version: "A版".to_string(),
                            object_model_code: "RELAPOPC".to_string(),
                            instance_code: "".to_string(),
                            start_object_code: from_code.object_model_code.to_string(),
                            start_instance_code: from_code.instance_code.to_string(),
                            end_object_code: to_code.object_model_code.to_string(),
                            end_instance_code: to_code.instance_code.to_string(),
                            attributes: vec![],
                        })
                    }
                }
            }
        }
    }
    for (_bran_refno,(href,tref)) in ref_map {
        let mut start_object_code = "".to_string();
        let mut start_instance_code = "".to_string();
        let mut end_object_code = "".to_string();
        let mut end_instance_code = "".to_string();

        if let Some(start_inst) = bran_instances_map.get(&href){
            start_object_code = start_inst.object_model_code.clone();
            start_instance_code = start_inst.instance_code.clone();
        }
        if let Some(end_inst) = bran_instances_map.get(&tref){
            end_object_code = end_inst.object_model_code.clone();
            end_instance_code = end_inst.instance_code.clone();
        }
        relations.push(DataCenterRelations{
            version: "A版".to_string(),
            object_model_code: "RELAPOPC".to_string(),
            instance_code: "".to_string(),
            start_object_code,
            start_instance_code,
            end_object_code,
            end_instance_code,
            attributes: vec![],
        })
    }
    // 将 relations 排序
    let mut relations_end = Vec::new();
    for (idx,mut relation) in relations.into_iter().enumerate() {
        relation.instance_code = format!("{}{}",relation.object_model_code.clone(),idx.to_string());
        relations_end.push(relation);
    }
    let result = DataCenterProjectWithRelations{
        project_code: "KY1801-208".to_string(),
        owner: "布置".to_string(),
        instances,
        relations:relations_end,
    };
    Ok(result)
}

fn get_instance_data_element(metadata_map: &HashMap<String, Vec<String>>, att_type: &str, index: usize) -> Option<DataCenterInstance> {
    let mut result = Vec::new();
    let metadata_value = metadata_map.get(att_type);
    if metadata_value.is_none() { return None; }
    let metadata_values = metadata_value.unwrap();
    if metadata_values.is_empty() { return None; }
    let code = get_characters_in_str(&metadata_values[0]);
    for value in metadata_values {
        result.push(DataCenterAttr {
            attribute_model_code: value.to_string(),
            value: AttrString("TEST".to_string()),
        });
    }
    Some(DataCenterInstance {
        object_model_code: code,
        instance_code: index.to_string(),
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
    let pipe_refno = RefU64::from_refno_str("24383/66469")?;
    let mut mgr = Arc::new(AiosDBManager::init_form_config().await?);
    get_data_center_from_pipe(&mgr, pipe_refno).await?;
    Ok(())
}