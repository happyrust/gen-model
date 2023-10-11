use aios_core::pdms_types::RefU64;
use arangors_lite::AqlQuery;
use crate::aql_api::PdmsRefnoNameAql;
use crate::consts::AQL_PDMS_ELES_COLLECTION;
use crate::arangodb::ArDatabase;

/// 通过 name集合 查找对应的参考号
pub async fn query_id_from_names_aql(names: Vec<String>, att_type: Option<&str>, database: &ArDatabase) -> anyhow::Result<Vec<PdmsRefnoNameAql>> {
    let names = names.into_iter()
        .map(|name| if name.starts_with("/") { name } else { format!("/{}", name) })
        .collect::<Vec<String>>();
    let aql = if att_type.is_some() {
        AqlQuery::new("\
        With @@pdms_eles
        for v in pdms_eles
        filter v.noun == @noun
        filter v.name in @names
        return {
            'refno':v._key,
            'name': v.name,
        }
        ").bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
            .bind_var("noun", att_type.unwrap())
            .bind_var("names", names)
    } else {
        AqlQuery::new("
        With @@pdms_eles
        for v in pdms_eles
            filter v.name in @names
            return {
                'refno':v._key,
                'name': v.name,
       }").bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
            .bind_var("names", names)
    };
    let result = database.aql_query::<PdmsRefnoNameAql>(aql).await?;
    Ok(result)
}


