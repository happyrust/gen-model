use aios_core::pdms_types::{EleTreeNode, PdmsElement, RefU64};
use bb8_arangodb::arangors::{AqlQuery, Database};
use crate::graph_db::pdms_arango::ArDatabase;

/// 通过图数据库查询 children
pub async fn query_ssc_children_aql(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Vec<EleTreeNode>> {
    let mut r = vec![];
    let refno_aql = format!("ssc_eles/{}", refno.to_url_refno());
    let aql = AqlQuery::builder().query("\
    FOR z in 1 INBOUND @id ssc_edges
    return {
        'refno':z._key,
        'owner':z.owner,
        'name':z.name,
        'noun':z.noun,
        'version':0,
        'children_count':length(for c in 1 inbound z._id ssc_edges
                            return 1 ),
    }
    ").bind_var("id", refno_aql).build();
    let result: Vec<PdmsElement> = database.aql_query(aql).await?;
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
