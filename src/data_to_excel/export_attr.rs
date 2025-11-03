use std::collections::HashMap;
use std::io::Write;
use std::str::FromStr;
use aios_core::AttrVal::RefU64Type;
use aios_core::pdms_types::RefU64;
use aios_core::tool::db_tool::{db1_dehash, db1_hash};
use itertools::all;
use crate::aql_api::children::query_children_order_aql;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::data_to_excel::export_csv::create_csv_file;

/// 将节点的attr导出为csv
// pub async fn get_attrs_to_csv(refnos: Vec<RefU64>, aios_mgr: &AiosDBManager) -> anyhow::Result<()> {
//     let mut type_attrs = HashMap::new();
//     let mut all_refnos = Vec::new();
//     for refno in refnos {
//         all_refnos.push(refno);
//         let attr = aios_mgr.get_attr(refno).await?;
//         // 将元件库也导出
//         if let Some(RefU64Type(spre)) = attr.get_val("SPRE") {
//             all_refnos.push(*spre);
//             let spre_attr = aios_mgr.get_attr(*spre).await?;
//             if let Some(RefU64Type(catr)) = spre_attr.get_val("CATR") {
//                 all_refnos.push(*catr);
//                 let catr_attr = aios_mgr.get_attr(*catr).await?;
//                 type_attrs.entry(catr_attr.get_type_str().to_string()).or_insert_with(Vec::new).push(catr_attr);
//             }
//             if let Some(RefU64Type(detr)) = spre_attr.get_val("DETR") {
//                 all_refnos.push(*detr);
//                 let detr_attr = aios_mgr.get_attr(*detr).await?;
//                 type_attrs.entry(detr_attr.get_type_str().to_string()).or_insert_with(Vec::new).push(detr_attr);
//             }
//             type_attrs.entry(spre_attr.get_type_str().to_string()).or_insert_with(Vec::new).push(spre_attr);
//         }
//         type_attrs.entry(attr.get_type_str().to_string()).or_insert_with(Vec::new).push(attr);
//     }
//     dbg!(&all_refnos);
//     dbg!(&all_refnos.len());
//     // 将 attr 导出
//     for (att_type, mut attrs) in type_attrs {
//         let mut csv_value = Vec::new();
//         // 表头
//         let mut value_sort = Vec::new();
//         // 第一行数据 固定表头 ，后面的数据按第一行数据的表头排序
//         let mut single_csv_value = Vec::new(); // 一行数据
//         let first_attr = attrs.remove(0);
//         let Some(refno) = first_attr.get_refno() else { continue; };
//
//         value_sort.push(db1_hash("REFNO"));
//         single_csv_value.push(refno.to_string());
//         for (k, v) in first_attr.map {
//             if k == db1_hash("REFNO") { continue; }
//             value_sort.push(k);
//             single_csv_value.push(v.get_val_as_string_csv());
//         }
//         csv_value.push(single_csv_value);
//         // 将后面的attr按第一行的顺序进行排列
//         for attr in attrs {
//             let mut single_csv_value = Vec::new();
//             for k in &value_sort {
//                 if let Some(value) = attr.map.get(&k) {
//                     single_csv_value.push(value.get_val_as_string_csv());
//                 } else {
//                     single_csv_value.push(" ".to_string());
//                 };
//
//             }
//             csv_value.push(single_csv_value);
//         }
//         // 导出csv
//         let headers = value_sort
//             .into_iter()
//             .map(|x| db1_dehash(x))
//             .collect::<Vec<_>>();
//         let csv_file = create_csv_file(headers,csv_value);
//         let mut file = std::fs::File::create(format!("{}.csv",att_type))?;
//         file.write_all(&csv_file)?;
//     }
//     Ok(())
// }

#[tokio::test]
async fn test_get_attrs_to_csv() -> anyhow::Result<()> {
    let aios_mgr = AiosDBManager::init_form_config().await.unwrap();
    let database = aios_mgr.get_arango_db().await?;
    let children = query_children_order_aql(&database,RefU64::from_str("24383/66748").unwrap()).await?;
    // let mut refnos = children.into_iter().map(|x| x.refno).collect();
    // push spre 和 catr
    // get_attrs_to_csv(refnos,&aios_mgr).await

    Ok(())
}

#[test]
fn test_replace() {
    let value = [0.0,1.0,2.0];
    let json = serde_json::to_string(&value).unwrap().replace(",",";");
    dbg!(&json);
}
