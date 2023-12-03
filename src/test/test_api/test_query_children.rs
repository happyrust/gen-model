use aios_core::pdms_types::RefU64;
use crate::aql_api::children::SearchAlongParam;
use crate::data_interface::interface::PdmsDataInterface;

use crate::test::test_helper::{get_test_ams_db_manager, get_test_ams_db_manager_async};

///  测试沿着路径搜索目标节点
#[tokio::test]
async fn test_search_along_path() -> anyhow::Result<()> {
    let mgr = get_test_ams_db_manager_async().await;
    let refno: RefU64 = "25688/4595".into();
    let param = SearchAlongParam {
        refnos: vec![refno],
        fuzzy: vec!["1AR".to_owned(), "WF".to_owned()],
        path_nouns: vec!["SITE".to_owned(), "ZONE".to_owned()],
        children_nouns: vec![],
        ancestor_nouns: vec!["ZONE".to_owned()],
        only_path_nodes: true,
        include_path_nodes: true,
    };
    dbg!(serde_json::to_string(&param).unwrap());
    let result = mgr.search_refnos_along_path_by_param(&param).await?;
    dbg!(result.0.len());
    // dbg!(result
    //     .iter()
    //     .take(50)
    //     .map(|x| { aios_core::get_named_attmap(x.0 .0).unwrap().get_name_or_default() })
    //     .collect::<Vec<_>>());

    Ok(())
}
