use aios_core::pdms_types::{EleTreeNode, PdmsElement, RefU64};
use bb8_arangodb::arangors_lite::{AqlQuery, Database};
use crate::consts::{AQL_SSC_EDGE_COLLECTION, AQL_SSC_ELES_COLLECTION};
use crate::arangodb::ArDatabase;

/// 通过图数据库查询 children
pub async fn query_ssc_children_aql(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Vec<EleTreeNode>> {
    let mut r = vec![];
    let refno_aql = format!("{AQL_SSC_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::new("\
    With @@ssc_eles,@@ssc_edges
    FOR z in 1 INBOUND @id ssc_edges
    filter z != null
    sort z.order
    return {
        '_key':z._key,
        'owner':z.owner,
        'name':z.name,
        'noun':z.noun,
        'version':0,
        'children_count':length(for c in 1 inbound z._id ssc_edges
                            return 1 ),
    }")
        .bind_var("id", refno_aql)
        .bind_var("@ssc_eles",AQL_SSC_ELES_COLLECTION)
        .bind_var("@ssc_edges",AQL_SSC_EDGE_COLLECTION);
    let result: Vec<PdmsElement> = database.aql_query(aql).await.unwrap();
    for v in result {
            r.push(EleTreeNode {
                refno: v.refno,
                owner: v.owner,
                name: v.name,
                noun: v.noun,
                children_count: v.children_count,
            })
    }
    Ok(r)
}
