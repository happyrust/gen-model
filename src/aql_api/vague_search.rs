use aios_core::pdms_types::RefU64;
use aios_core::three_dimensional_review::VagueSearchExportCsvData;
use crate::aql_api::children::query_refnos_belong_level_aql;
// use crate::data_to_excel::export_csv::create_csv_file;
use crate::arangodb::ArDatabase;

/// 将查询的结果导出为csv
pub async fn export_vague_search_result(refnos: Vec<RefU64>, filter_condition: &str, database: &ArDatabase) -> anyhow::Result<Vec<u8>> {
    let search_results = query_refnos_belong_level_aql(refnos, "SITE", database).await?;
    let mut data = Vec::new();
    for result in search_results {
        let level = result.level.into_iter()
            .map(|level| if level.starts_with("/") { level[1..].to_string() } else { level }).collect::<Vec<_>>().join(" /");
        data.push(VagueSearchExportCsvData {
            key_word: filter_condition.to_string(),
            result: result.name,
            belong_level: level,
            att_type: result.att_type,
        }.into_vec_string());
    }
    Ok(vec![])
    // Ok(create_csv_file(vec!["关键词".to_string(), "命中目标".to_string(), "所属层级".to_string(), "目标类型".to_string()], data))
}