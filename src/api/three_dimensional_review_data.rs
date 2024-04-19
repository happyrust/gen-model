use aios_core::three_dimensional_review::{ThreeDimensionalModelDataCrate, ThreeDimensionalModelDataToArango};
use crate::graph_db::pdms_arango::save_arangodb_doc;
// use crate::options::DbOption;

use crate::arangodb::ArDatabase;


//编校审数据存入图数据库
pub async fn save_three_dimensional_review_data_to_arango(
    database: &ArDatabase,
    review_data: ThreeDimensionalModelDataCrate,
) -> anyhow::Result<()> {
    let json = serde_json::to_value(review_data.to_arango_struct())?;
    save_arangodb_doc(json, "review_data", database, false).await?;
    Ok(())
}

//保存来自普华的数据
pub async fn save_threed_review_data_to_arango(
    database: &ArDatabase,
    review_data: ThreeDimensionalModelDataCrate,
) -> anyhow::Result<()> {
    let json = serde_json::to_value(review_data.to_arango_struct())?;
    save_arangodb_doc(json, "review_data", database, false).await?;
    Ok(())
}

fn insert_three_dimensional_review_data(review_data: ThreeDimensionalModelDataCrate) -> Vec<ThreeDimensionalModelDataToArango> {
    let mut review_data_vec = Vec::new();
    // let data = ThreeDimensionalModelDataToArango {
    //     _key: review_data.key_value,
    //     proj_code: review_data.proj_code,
    //     user_code: review_data.user_code,
    //     site_code: review_data.site_code,
    //     site_name: review_data.site_name,
    //     user_role: review_data.user_role,
    //     model_data: review_data.model_data,
    //     flow_pic_data: review_data.flow_pic_data,
    // };
    // review_data_vec.push(data);
    review_data_vec
}

pub async fn query_three_dimensional_review_data(database: &ArDatabase, key_value: &str) -> anyhow::Result<Option<Vec<ThreeDimensionalModelDataToArango>>> {
    let aql = AqlQuery::new("with review_data return document('review_data',@_key)")
        .bind_var("_key", key_value);
    let data_vec: Vec<ThreeDimensionalModelDataToArango> = database.aql_query(aql).await?;
    return Ok(Some((data_vec)));
}


pub async fn query_threed_review_data(database: &ArDatabase, key_value: &str) -> anyhow::Result<Option<Vec<ThreeDimensionalModelDataToArango>>> {
    let aql = AqlQuery::new("with threed_review let v = document('threed_review',@_key)\
        return unset(v , '_id','_rev') ")
        .bind_var("_key", key_value);
    let data_vec: Vec<ThreeDimensionalModelDataToArango> = database.aql_query(aql).await?;
    return Ok(Some((data_vec)));
}


pub async fn query_threed_review_data_by_name(database: &ArDatabase, name: &str) -> anyhow::Result<Option<Vec<ThreeDimensionalModelDataToArango>>> {
    let aql = AqlQuery::new("with @@collection FOR u IN @@collection
                                                FILTER u.UserCode==@name
                                                return unset(u , '_id','_rev')")
        .bind_var("@collection", "threed_review")
        .bind_var("name", name);
    let data_vec: Vec<ThreeDimensionalModelDataToArango> = database.aql_query(aql).await?;
    return Ok(Some((data_vec)));
}