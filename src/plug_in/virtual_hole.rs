use std::sync::Arc;
use aios_core::create_attas_structs::{VirtualEmbedGraphNodeQuery, VirtualHoleGraphNodeQuery};
use aios_core::data_center::{DataCenterProject, SendHoleData};
use aios_core::options::DbOption;
use arangors_lite::AqlQuery;
use bitvec::macros::internal::funty::Fundamental;
use serde::{Serialize, Deserialize};

use crate::api::virtual_hole::query_hole_detail_data_by_code;
use crate::api::virtual_hole::query_embed_detail_data_by_code;
use crate::consts::{AQL_EMBED_DATA_COLLECTION, AQL_HOLE_DATA_COLLECTION, AQL_VIRTUAL_HOLE_COLLECTION};
use crate::data_center_api::embed::create_embed_data_aql;
use crate::data_center_api::hole::gen_hole_datacenter_instance_aql;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::arangodb::ArDatabase;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;

pub async fn get_audit_data(aios_mgr: &AiosDBManager, data: &mut SendHoleData) {
    let mut agree = false;
    //获取project_code
    data.form_data.project_code = aios_mgr.db_option.project_code.to_string();

    // //如果设定人全部同意(流程结束)发送元数据包
    //操作人与审定人对比，
    let sz_name = data.form_data.sz_name.split('/').next().unwrap_or_default().to_string();
    if data.form_data.human_code == sz_name {
        agree = true;
        for i in &data.form_data.model_body {
            if !i.status {
                agree = false;
                break;
            }
        }
    }
    if agree {
        //发送元数据包
        let mut hole_keys = vec![];
        let mut embed_keys = vec![];
        let mut instances_vec = vec![];
        for i in &data.form_data.detail {
            if i.is_hole {
                hole_keys.push(i.key.clone());
            } else {
                embed_keys.push(i.key.clone());
            }
        }
        if let Ok(database) = aios_mgr.get_arango_db().await {
            if let Some(mut instances) = gen_hole_datacenter_instance_aql(hole_keys, &aios_mgr.db_option.project_code, &database).await {
                instances_vec.append(&mut instances);
            }
            if let Ok(Some(mut instances)) = create_embed_data_aql(embed_keys, &aios_mgr.db_option.project_code, &database).await {
                instances_vec.append(&mut instances);
            }
        }
        data.form_data.data_body = DataCenterProject {
            package_code: DataCenterProject::convert_package_code(),
            project_code: aios_mgr.db_option.project_code.to_string(),
            owner: "KY1801-208".to_string(),
            instances: instances_vec,
        };
        //流程结束，对应的孔洞/埋件数据需要进行版本升级
        //遍历detail,通过key找到图数据库对应的数据
        if let Ok(database) = aios_mgr.get_arango_db().await {
            //保存需要更改version的孔洞
            let mut update_holes_version = vec![];
            //保存需要要更改version的埋件
            let mut update_embeds_version = vec![];
            //读取detail中对应的数据
            for i in &data.form_data.detail {
                if i.is_hole {
                    if let Ok(Some(mut result)) = query_hole_detail_data_by_code(&database, &i.key).await {
                        update_holes_version.append(&mut result);
                    }
                } else {
                    if let Ok(Some(mut result)) = query_embed_detail_data_by_code(&database, &i.key).await {
                        update_embeds_version.append(&mut result);
                    }
                }
            }
            //修改对应的孔洞version
            for i in update_holes_version {
                match i.version {
                    //若为空，则将version置为A
                    ' ' => {
                        if let Ok(Some(_)) = update_virtual_hole_data_version_aql(&database, i._key, 'A').await{}

                    }
                    //若为A-Y,则version+1
                    'A'..='Y' => {
                        if let Ok(Some(_)) = update_virtual_hole_data_version_aql(&database, i._key, (i.version as u8 + 1) as char).await{}
                    }
                    _ => {}
                }
            }

            //修改对应的埋件version
            for i in update_embeds_version {
                match i.version {
                    //若为空，则将version置为A
                    ' ' => {
                        if let Ok(Some(_)) = update_virtual_embed_data_version_aql(&database, i._key, 'A').await {}
                    }
                    //若为A-Y,则version+1
                    'A'..='Y' => {
                        if let Ok(Some(_)) = update_virtual_embed_data_version_aql(&database, i._key, (i.version as u8 + 1) as char).await {}
                    }
                    _ => {}
                }
            }
        }
        dbg!("****");
    }
}


///更新虚拟孔洞版本
pub async fn update_virtual_hole_data_version_aql(
    database: &ArDatabase,
    key: String,
    version: char,
) -> anyhow::Result<Option<Vec<VirtualHoleGraphNodeQuery>>> {
    let aql = format!("With {AQL_HOLE_DATA_COLLECTION} update {{'_key':'{}' , 'Version':'{}'}} in {}", key, version.to_string(), AQL_HOLE_DATA_COLLECTION);
    let result = database.aql_query::<VirtualHoleGraphNodeQuery>(AqlQuery::new(aql.as_str())).await?;
    return Ok(Some(result));
}

///更新虚拟埋件版本
pub async fn update_virtual_embed_data_version_aql(
    database: &ArDatabase,
    key: String,
    version: char,
) -> anyhow::Result<Option<Vec<VirtualEmbedGraphNodeQuery>>> {
    let aql = format!("With {AQL_EMBED_DATA_COLLECTION} update {{'_key':'{}' , 'Version':'{}'}} in {}", key, version.to_string(), AQL_EMBED_DATA_COLLECTION);
    let reuslt = database.aql_query::<VirtualEmbedGraphNodeQuery>(AqlQuery::new(aql.as_str())).await?;
    return Ok(Some(reuslt));
    let _ = database.aql_query::<VirtualEmbedGraphNodeQuery>(AqlQuery::new(aql.as_str())).await;
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct VirtualHoleKey {
    pub is_hole: bool,
    pub key: String,
}

/// 通过虚拟孔洞提资流程的key，查询本次提资中用到了哪些孔洞埋件的key
async fn query_virtual_hole_detail_key(document_key: &str, database: &ArDatabase) -> anyhow::Result<Vec<VirtualHoleKey>> {
    let aql = AqlQuery::new("\
    With @@virtual_hole
    let data = document(@@virtual_hole,@key)
    for d in data.formdata.Detail
    return {
        'is_hole':d.is_hole,
        'key':d.key,
    }").bind_var("@virtual_hole", AQL_VIRTUAL_HOLE_COLLECTION)
        .bind_var("key", document_key);
    let result = database.aql_query::<VirtualHoleKey>(aql).await?;
    Ok(result)
}

/// 更新孔洞或者埋件校审状态
///
/// is_hole  true : 孔洞  false ： 埋件
async fn update_hole_embed_js_status(keys: Vec<String>, status: &str, is_hole: bool, database: &ArDatabase) -> anyhow::Result<()> {
    let collection = if is_hole { AQL_HOLE_DATA_COLLECTION } else { AQL_EMBED_DATA_COLLECTION };
    let aql = AqlQuery::new("\
    with @@hole_data
    for key in @keys
    update {'_key':key , 'JSStatus':@status} in @@hole_data
    ").bind_var("@hole_data", collection)
        .bind_var("keys", keys)
        .bind_var("status", status);
    let _ = database.aql_query::<String>(aql).await?;
    Ok(())
}

/// 根据提资表单，更新虚拟孔洞埋件中的校审状态
pub async fn update_virtual_hole_status(document_key: &str, status: &str, database: &ArDatabase) -> anyhow::Result<String> {
    let Ok(keys) = query_virtual_hole_detail_key(document_key, database).await else { return Ok("单据不存在".to_string()); };
    if keys.is_empty() { return Ok("单据不存在".to_string()); }
    // 孔洞
    let hole_keys = keys.iter()
        .filter(|k| k.is_hole)
        .map(|x| x.key.clone())
        .collect::<Vec<String>>();
    // 埋件
    let embed_keys = keys.into_iter()
        .filter(|k| !k.is_hole)
        .map(|x| x.key)
        .collect::<Vec<String>>();
    // 分别更新状态
    if !hole_keys.is_empty() {
        update_hole_embed_js_status(hole_keys, status, true, database).await?;
    }
    if !embed_keys.is_empty() {
        update_hole_embed_js_status(embed_keys, status, false, database).await?;
    }
    Ok("完成赋值".to_string())
}






#[tokio::test]
async fn test_update_virtual_hole_data_version_aql() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    if let Ok(Some(_)) = update_virtual_hole_data_version_aql(&database, "bca176a3-a8cf-4e1f-b21e-50ac7f56ab5d12".to_string(), 'D').await{}
    Ok(())
}
