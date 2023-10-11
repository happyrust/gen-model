use std::collections::HashMap;
use aios_core::options::DbOption;
use aios_core::pdms_types::RefU64;
use aios_core::pdms_types::UdaMajorType::P;
use bb8_arangodb::arangors_lite::{AqlQuery, Database};
use dashmap::DashMap;
use futures::future;
use futures::future::ok;
use once_cell::sync::Lazy;
use crate::api::attr::query_attr;
use crate::api::children::travel_children_with_type;
use crate::aql_api::change_vec_refnos_into_vec_string;
use crate::aql_api::children::{query_children_eles, query_travel_children_with_types_aql};
use crate::aql_api::foreign_refnos::query_foreign_refno_aql;
use crate::aql_api::para_value::query_des_para_value;
use crate::consts::AQL_DATA_ELES_COLLECTION;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::DataDocument;
use crate::arangodb::ArDatabase;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;

/// 查询 catr refno引用的 dtse下 data 的 ppro和 dpro数据
pub async fn query_dtse_ppro_from_catr_refno(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Option<DashMap<String, DataDocument>>> {
    let dtre_refno = query_foreign_refno_aql(&database, refno, &["DTRE", "DTRE"]).await?;
    if dtre_refno.is_none() { return Ok(None); }
    let data_refnos = query_children_eles(&database, dtre_refno.unwrap()).await?;
    let mut children = vec![];
    for data_refno in data_refnos.into_iter() {
        children.push(data_refno.refno);
    }
    let result = query_data_attr_from_refnos(children, &database).await?;
    Ok(Some(result))
}

/// 返回data下对应的ppro数据 -> k: dkey
async fn query_data_attr_from_refnos(refnos: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<DashMap<String, DataDocument>> {
    let children = change_vec_refnos_into_vec_string(refnos);
    let aql = AqlQuery::new("
    With @@data_eles
    let data = @element
    for v in data
    let e = document(@@data_eles,v)
        return {
            '_key':e._key,
            'dkey':e.dkey,
            'ppro':e.ppro,
            'dpro':e.dpro,
        } "
    )
        .bind_var("element", children)
        .bind_var("@data_eles", AQL_DATA_ELES_COLLECTION);
    let result: Vec<DataDocument> = database.aql_query(aql).await?;
    let mut data_map = DashMap::new();
    for r in result {
        data_map.entry(r.dkey.clone()).or_insert(r);
    }
    Ok(data_map)
}

pub static BRAN_IPARAM_MAP: Lazy<DashMap<RefU64, Vec<f64>>> = Lazy::new(DashMap::new);

impl AiosDBManager{

    /// 查询保温层参数
    pub fn query_ipara_from_ele(&self, ele_refno: RefU64) -> anyhow::Result<Vec<f64>> {
        let owner = self.get_owner(ele_refno);
        if BRAN_IPARAM_MAP.contains_key(&owner) {
            return Ok(BRAN_IPARAM_MAP.get(&owner).unwrap().clone());
        }
        let owner_type = self.get_type_name(owner);
        if owner_type.as_str() == "BRAN" {
            // let mgr = self.clone();
            let s = futures::executor::block_on(async {
                self.query_ipara_from_bran(owner).await.unwrap_or_default()
            });
            BRAN_IPARAM_MAP.insert(ele_refno, s);
        }
        Ok(vec![])
    }

    /// 查询保温层参数
    pub async fn query_ipara_from_bran(&self, bran_refno: RefU64) -> anyhow::Result<Vec<f64>> {
        let database = self.get_arango_db().await?;
        let bran_attr = self.get_full_attr_from_localdb(bran_refno)?;
        let temp = bran_attr.get_f32("TEMP").unwrap_or(-100000.0);
        let h_bore = bran_attr.get_f32("HBOR").unwrap_or(0.0);
        let Some(ispec) = bran_attr.get_refu64("ISPE") else { return Ok(vec![]); };
        if *ispec == 0 { return Ok(vec![0.0]); };
        // 找到ispec下所有的 bore 范围，并将其分类
        let bore_node = query_travel_children_with_types_aql(&database, ispec, &vec!["SPCO"], false).await?;
        // key 为 温度范围， value 为 外径范围
        let mut bore_map = HashMap::new();
        for node in bore_node {
            bore_map.entry(node.owner).or_insert_with(Vec::new).push(node);
        }
        // 根据温度节点和外径节点查询到具体的范围数值
        // ispec_vec : 0 : 温度范围 , 1 : 外径范围以及引用的catr
        let mut ispec_vec = Vec::new();
        for (t_ref, bore_vec) in bore_map {
            let Ok(temp_attr) = self.get_full_attr_from_localdb(t_ref) else { continue; };
            let Some(temp_answer) = temp_attr.get_f32("ANSW") else { continue; };
            let Some(temp_max_answer) = temp_attr.get_f32("MAXA") else { continue; };
            let mut bore_value_vec = Vec::new();
            for bore in bore_vec {
                let Ok(bore_attr) = self.get_full_attr_from_localdb(bore.refno) else { continue; };
                let Some(bore_answer) = bore_attr.get_f32("ANSW") else { continue; };
                let Some(bore_max_answer) = bore_attr.get_f32("MAXA") else { continue; };
                let Some(catr_refno) = bore_attr.get_refu64("CATR") else { continue; };
                if !catr_refno.is_valid() { continue; };
                bore_value_vec.push((bore_answer, bore_max_answer, catr_refno));
            }
            ispec_vec.push(((temp_answer, temp_max_answer), bore_value_vec));
        }
        // 根据bran的temp和bore找到具体的保温层
        for ispec in &ispec_vec {
            if temp >= ispec.0.0 && temp <= ispec.0.1 {
                for bore in &ispec.1 {
                    if h_bore >= bore.0 && h_bore <= bore.1 {
                        let Ok(catr_attr) = self.get_attr_from_localdb(bore.2) else { break; };
                        let Some(para) = catr_attr.get_f64_vec("PARA") else { return Ok(vec![]); };
                        return Ok(para);
                    }
                }
                break;
            }
        }
        Ok(vec![])
    }
}



#[tokio::test]
async fn test_query_dtse_ppro_from_catr_refno() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let refno = RefU64::from_refno_str("15193/14606").unwrap();
    let result = query_dtse_ppro_from_catr_refno(refno, &database).await?;
    dbg!(&result);
    Ok(())
}

#[tokio::test]
async fn test_query_ipara_from_bran() -> anyhow::Result<()> {
    let aios_mgr = AiosDBManager::init_form_config().await?;
    let bran_refno = RefU64::from_url_refno("24383_74374").unwrap();
    let result = aios_mgr.query_ipara_from_bran(bran_refno).await?;
    dbg!(&result);
    Ok(())
}