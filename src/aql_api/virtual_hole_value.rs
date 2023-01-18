use aios_core::pdms_types::RefU64;
use arangors_lite::AqlQuery;
use config::{Config, ConfigError, Environment, File};
use crate::graph_db::pdms_arango::get_arangodb_conn_from_db_option;
use crate::graph_db::structs::{VirtualEmbedGraphNode, VirtualHoleGraphNode};
use crate::options::DbOption;


pub async fn query_virtual_hole_value(refnos: Vec<RefU64>) -> anyhow::Result<Option<(Vec<VirtualHoleGraphNode>, Vec<VirtualEmbedGraphNode>)>> {
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option(&db_option).await?;

    let mut hole: Vec<VirtualHoleGraphNode> = Vec::new();
    for refno in &refnos {
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
            .bind_var("collection", "hole_data")
            .bind_var("refno", refno.to_url_refno());
        hole.append(&mut database.aql_query(aql).await?);
    }


    let mut embed: Vec<VirtualEmbedGraphNode> = Vec::new();
    for refno in &refnos {
        let aql = AqlQuery::new("
        let a =  document(@collection,@refno)
        return {
             '_key':a._key,
             'intelld':a. intelld,
             'code':a.code,
             'relyitem':a.relyitem,
             'relyitemref':a.relyitemref,
             'mainitem':a.mainitem,
             'speciality':a.speciality,
             'position':a.position,
             'ori':a.ori,
             'work':a.work,
             'workby':a.workby,
             'time':a.time,
             'standertype':a.standertype,
             'openitem':a.openitem,
             'holework':a.holework,
             'sizelength':a.sizelength,
             'sizewidth':a.sizewidth,
             'sizethickness':a.sizethickness,
             'minthickness':a.minthickness,
             'load': a.load,
             'mindistance':a.mindistance,
             'subsmeterial':a.subsmeterial,
             'fittid':a.fittid,
             '_ref':a._ref,
             'shape':a.shape,
             'note':a.note
}
")
            .bind_var("collection", "embed_data")
            .bind_var("refno", refno.to_url_refno());
        embed.append(&mut database.aql_query(aql).await?);
    }
    return Ok(Some((hole, embed)));
}

