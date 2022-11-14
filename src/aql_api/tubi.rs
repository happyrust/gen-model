use aios_core::pdms_types::RefU64;
use aios_core::prim_geo::tubing::TubiEdgeAql;
use arangors_lite::{AqlQuery, Database};
use smol_str::SmolStr;
use crate::api::children::travel_children_with_type;
use crate::aql_api::children::query_travel_children_with_type_aql;
use crate::data_interface::tidb_manager::TUBI_TOL;
use crate::graph_db::pdms_arango::get_arangodb_conn_from_db_option;
use crate::options::DbOption;

/// 找到某个节点下所有的 bran 中的 tubi
pub async fn query_all_tubi_from_node(refno: RefU64, database: &Database) -> anyhow::Result<()> {
    let brans = query_travel_children_with_type_aql(database, refno, "BRAN").await?;
    let mut total_distance = 0.0;
    for bran in brans {
        let tubis = query_tubi_from_bran(bran.refno, database).await?;
        for tubi in tubis {
            let distance = tubi.start_pt.distance(tubi.end_pt);
            total_distance += distance;
        }
    }
    dbg!(&total_distance);
    Ok(())
}

/// 找到 bran 下所有的 tubi
pub async fn query_tubi_from_bran(bran_refno: RefU64, database: &Database) -> anyhow::Result<Vec<TubiEdgeAql>> {
    let key = format!("pdms_eles/{}", bran_refno.to_url_refno());
    let aql = AqlQuery::new("
    let bran_name = ( return document('pdms_eles',@bran_refno).name )
    for v,e in 0..100 outbound @id tubi_edges
    filter bran_name[0] != null
    filter bran_name[0] == e.bran_name
    filter e != null
    return {
        '_key': e._key,
        '_from': e._from,
        '_to':e._to,
        'start_pt': e.start_pt,
        'end_pt': e.end_pt,
        'att_type': e.att_type,
        'bran_name': e.bran_name,
        'extra_type': e.extra_type,
        'bore': e.bore
    }")
        .bind_var("id", key)
        .bind_var("bran_refno",bran_refno.to_url_refno());
    let mut results: Vec<TubiEdgeAql> = database.aql_query(aql).await?;
    // 过滤不是 tubi 的数据
    results.retain(|r| {
        let distance = r.start_pt.distance(r.end_pt);
        distance > TUBI_TOL
    });
    Ok(results)
}

#[tokio::test]
async fn test_query_all_tubi_from_node() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option(&db_option).await?;
    query_all_tubi_from_node(RefU64::from_two_nums(23584,5652),&database).await
}

#[test]
fn test_small_str() {
    let smol_str = SmolStr::new("/100-B-1");
    dbg!(&smol_str);
    let str = smol_str.to_string();
    dbg!(&str);
}