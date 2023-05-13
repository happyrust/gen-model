use aios_core::three_dimensional_review::{ThreeDimensionalModelDataCrate, ThreeDimensionalModelDataToArango};
use crate::consts::ARANGODB_SAVE_AMOUNT;
use crate::graph_db::pdms_arango::{get_arangodb_conn_from_db_option, save_arangodb_with_database};
use arangors_lite::Database;
// use crate::options::DbOption;
use arangors_lite::AqlQuery;


//编校审数据存入图数据库
pub async fn save_three_dimensional_review_data_to_arango(
    database: Database,
    review_data: ThreeDimensionalModelDataCrate,
) -> anyhow::Result<()>
{
    let data = insert_three_dimensional_review_data(review_data);
    for i in data.chunks(ARANGODB_SAVE_AMOUNT) {
        let json = serde_json::to_value(i)?;
        save_arangodb_with_database(json, "review_data", &database, false).await?;
    }
    Ok(())
}

//保存来自普华的数据
pub async fn save_threed_review_data_to_arango(
    database: Database,
    review_data: ThreeDimensionalModelDataCrate,
) -> anyhow::Result<()>
{
    let data = insert_three_dimensional_review_data(review_data);
    for i in data.chunks(ARANGODB_SAVE_AMOUNT) {
        let json = serde_json::to_value(i)?;
        save_arangodb_with_database(json, "threed_review", &database).await?;
    }
    Ok(())
}

fn insert_three_dimensional_review_data(review_data: ThreeDimensionalModelDataCrate) -> Vec<ThreeDimensionalModelDataToArango> {
    let mut review_data_vec = Vec::new();
    let data = ThreeDimensionalModelDataToArango {
        _key: review_data.key_value,
        proj_code: review_data.proj_code,
        user_code: review_data.user_code,
        site_code: review_data.site_code,
        site_name: review_data.site_name,
        user_role: review_data.user_role,
        model_data: review_data.model_data,
        flow_pic_data: review_data.flow_pic_data,
    };
    review_data_vec.push(data);
    review_data_vec
}

pub async fn query_three_dimensional_review_data(database: &Database, key_value: &str) -> anyhow::Result<Option<Vec<ThreeDimensionalModelDataToArango>>> {
    let aql = AqlQuery::new("return document('review_data',@_key)")
        .bind_var("_key", key_value);
    let data_vec: Vec<ThreeDimensionalModelDataToArango> = database.aql_query(aql).await?;
    return Ok(Some((data_vec)));
}


pub async fn query_threed_review_data(database: &Database, key_value: &str) -> anyhow::Result<Option<Vec<ThreeDimensionalModelDataToArango>>> {
    let aql = AqlQuery::new("return document('threed_review',@_key)")
        .bind_var("_key", key_value);
    let data_vec: Vec<ThreeDimensionalModelDataToArango> = database.aql_query(aql).await?;
    return Ok(Some((data_vec)));
}


pub async fn query_threed_review_data_by_name(database: &Database, name: &str) -> anyhow::Result<Option<Vec<ThreeDimensionalModelDataToArango>>> {
    let aql = AqlQuery::new("return document('threed_review',@UserCode)")
        .bind_var("UserCode", name);
    let data_vec: Vec<ThreeDimensionalModelDataToArango> = database.aql_query(aql).await?;
    return Ok(Some((data_vec)));
}