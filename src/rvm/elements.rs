use std::io::Write;
use std::sync::Arc;
use aios_core::pdms_types::RefU64;
use arangors_lite::Database;
use glam::Vec3;
use sqlx::{MySql, Pool};
use crate::api::attr::query_position_from_id;
use crate::api::element::query_children_eles;
use crate::aql_api::children::{query_ancestor_till_type_aql, query_ancestor_with_name_till_type_aql};
use crate::aql_api::PdmsRefnoNameAql;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_inst_arango::query_rvm_instance_data_from_refno_aql;
use crate::rvm::data_api::{gen_cntb_data, gen_cnte_data, gen_name_position_data, gen_prim_data, ShapeTypeData};

pub async fn create_owner_data(refno: RefU64, aios_mgr: &AiosDBManager, database: &Database) -> anyhow::Result<Vec<u8>> {
    let mut data = vec![];
    let ancestor = query_ancestor_with_name_till_type_aql(database, refno, "SITE").await?;
    if ancestor.is_empty() { return Ok(data); }
    data.append(&mut create_ancestor_data(ancestor, aios_mgr).await.unwrap_or(vec![]));
    data.append(&mut create_element_data(refno,aios_mgr,Vec3::ZERO,database).await?);
    Ok(data)
}

async fn create_ancestor_data(ancestor: Vec<PdmsRefnoNameAql>, aios_mgr: &AiosDBManager) -> anyhow::Result<Vec<u8>> {
    let mut data = vec![];
    let mut current_position = Vec3::ZERO;
    for refno_name in ancestor.into_iter().rev() {
        let refno = RefU64::from_url_refno(&refno_name.refno);
        if refno.is_none() { continue; }
        let refno = refno.unwrap();
        let pos = query_position_from_id(refno, aios_mgr).await?.unwrap_or(Vec3::ZERO);
        current_position = current_position + pos;
        data.append(&mut gen_ancestor_data_str(&refno_name.name, current_position));
    }
    Ok(data)
}

/// position: ancestor到本层级的相对坐标
async fn create_element_data(refno: RefU64, aios_mgr: &AiosDBManager,position:Vec3,database:&Database) -> anyhow::Result<Vec<u8>> {
    let mut data = vec![];
    let pool = aios_mgr.get_project_pool_by_refno(refno).await;
    if pool.is_none() { return Ok(data); }
    let (_, pool) = pool.unwrap();
    let children = query_children_eles(refno, &pool).await?;
    for child in children {
        let refno = RefU64::from_refno_str(&child.refno);
        if refno.is_err() { continue; }
        let refno = refno.unwrap();
        data.append(&mut gen_cntb_data());
        let pos = query_position_from_id(refno,aios_mgr).await?.unwrap_or(Vec3::ZERO) + position;
        data.append(&mut gen_name_position_data(&child.name,pos));
        let instance = query_rvm_instance_data_from_refno_aql(refno,database).await?;
        if instance.is_none() { continue; }
        let instance = instance.unwrap();
        data.append(&mut gen_prim_data(instance,ShapeTypeData::Box(460.0,460.0,460.0)));
        data.append(&mut gen_cnte_data());
    }
    Ok(data)
}



fn gen_ancestor_data_str(name: &str, pos: Vec3) -> Vec<u8> {
    format!("CNTB\r\n     1     2\r\n{}\r\n          {:.2}          {:.2}          {:.2}\r\n     1\r\n", name, pos.x, pos.y, pos.z).into_bytes()
}

#[tokio::test]
async fn test_create_owner_data() -> anyhow::Result<()> {
    let mgr = Arc::new(AiosDBManager::init_form_config().await?);
    let refno = RefU64::from_refno_str("23584/108").unwrap();
    let database = mgr.get_arangodb_conn().await?;
    let data = create_owner_data(refno, &mgr, &database).await?;
    let mut file = std::fs::File::create("test_rvm.txt").unwrap();
    file.write_all(&data).unwrap();
    Ok(())
}