use aios_core::pdms_types::RefU64;
use arangors_lite::AqlQuery;
use config::{Config, ConfigError, Environment, File};
use crate::graph_db::pdms_arango::get_arangodb_conn_from_db_option;
use crate::graph_db::structs::VirtualHoleGraphNode;
use crate::options::DbOption;


pub async fn query_virtual_hole_value(refnos: Vec<RefU64>) -> anyhow::Result<Option<Vec<VirtualHoleGraphNode>>> {
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option(&db_option).await?;
    let mut result: Vec<VirtualHoleGraphNode> = Vec::new();
    for refno in refnos {
        let aql = AqlQuery::new("
        let a =  document(@collection,@refno)
        return {
            '_key':a._key,
            'bankheight':a.bankheight,
            'bankwidth':a.bankwidth,
            'code':a.code,
            'extentlength1':a.extentlength1,
            'extentlength2':a.extentlength2,
            'fittrefno':a.fittrefno,
            'heatthick':a.heatthick,
            'holework':a.holework,
            'hotdis':a.hotdis,
            'icreate':a.icreate,
            'intelld':a.intelld,
            'itemref':a.itemref,
            'mainitem':a.mainitem,
            'mainitemref':a.mainitemref,
            'note':a.note,
            'openitem':a.openitem,
            'ori':a.ori,
            'plugtype':a.plugtype,
            'position':a.position,
            'refno':a.refno,
            'rehole':a.rehole,
            'relyitem':a.relyitem,
            'second':a.second,
            'shape':a.shape,
            'sizeheigh':a.sizeheigh,
            'sizewidth':a.sizewidth,
            'speciality':a.speciality,
            'subsmeterial':a.subsmeterial,
            'substhickness':a.substhickness,
            'substype':a.substype,
            'time':a.time,
            'workby':a.workby,
}
")
            .bind_var("collection", "virtual_hole")
            .bind_var("refno", refno.to_url_refno());
        result.append(&mut database.aql_query(aql).await?);
    }
    return if result.len() == 0 { Ok(None) } else { Ok(Some(result)) };
}

