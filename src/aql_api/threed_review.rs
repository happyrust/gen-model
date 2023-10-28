use arangors_lite::AqlQuery;
use crate::arangodb::ArDatabase;

pub async fn remove_threed_review_with_key(database: &ArDatabase, key: String) -> anyhow::Result<bool> {
    let aql = AqlQuery::new(
        "
           with threed_review
                    REMOVE @key IN threed_review")
        .bind_var("key", key)
        ;
    let result = database.aql_query::<Vec<()>>(aql).await;
    dbg!(&result.as_ref().err());
    Ok(!result.is_err())
}