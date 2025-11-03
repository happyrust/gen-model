use anyhow::anyhow;

use calamine::{open_workbook, RangeDeserializerBuilder, Reader, Xlsx};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use crate::pcf::pcf_api::create_thickness_data;
use crate::ssc::SiteExcelData;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct PipeThicknessTable {
    dn: Option<String>,
    h: Option<String>,
    i: Option<String>,
    l: Option<String>,
}

impl PipeThicknessTable {
    fn is_null(&self) -> bool {
        if self.dn.is_none() || self.i.is_none() || self.l.is_none() || self.h.is_none() { return true; }
        false
    }
}

/// 获取管道外径壁厚表
pub fn get_pipe_thickness_table() -> anyhow::Result<DashMap<String, DashMap<String, String>>> {
    let mut map = DashMap::new();
    let mut workbook: Xlsx<_> = open_workbook("resource/管道外径壁厚表.xlsx")?;
    let range = workbook.worksheet_range("Sheet1")
        .ok_or(anyhow::anyhow!("Cannot find 'Sheet1'"))??;
    let mut iter = RangeDeserializerBuilder::new().from_range(&range)?;
    while let Some(result) = iter.next() {
        let v: PipeThicknessTable = result?;
        if !v.is_null() {
            map.entry(v.dn.clone().unwrap()).or_insert_with(DashMap::new).entry("H".to_string()).or_insert(v.h.unwrap());
            map.entry(v.dn.clone().unwrap()).or_insert_with(DashMap::new).entry("I".to_string()).or_insert(v.i.unwrap());
            map.entry(v.dn.unwrap()).or_insert_with(DashMap::new).entry("L".to_string()).or_insert(v.l.unwrap());
        }
    }
    Ok(map)
}

#[test]
fn test_get_pipe_thickness_table() -> anyhow::Result<()> {
    let pipe_name = "2ACAS-A6-806-6-LJ6";
    let map = get_pipe_thickness_table()?;
    let name = create_thickness_data(pipe_name, &map,true);
    Ok(())
}