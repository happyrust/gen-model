use std::collections::HashMap;
use aios_core::pdms_types::{NounHash, RefU64, RefU64Vec};
use dashmap::DashMap;
use parse_pdms_db::db_tool::db1_hash;
use parse_pdms_db::parse::WholeAttMap;
use smol_str::SmolStr;

pub fn gen_pdms_element_insert_sql(att: &WholeAttMap,name:&str, dbno: u32, project: &str) -> String {
    let implicit = &att.implicit_attmap;
    let refno = implicit.get_refno().unwrap();
    let type_name = implicit.get_type();
    let owner = implicit.get_owner().unwrap();

    let mut sql = String::new();
    sql.push_str(&format!(r#"({}, '{}', '{}', {},'{}' , {},'{}') ,"#,
                          refno.0, refno.to_refno_str(), type_name, owner.0, name, dbno, project));
    sql
}

pub fn gen_refno_infos_insert_sql(refno: RefU64, project: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!(r#"({},'{}') ,"#, refno.get_0(), project));
    sql
}

pub fn gen_dbno_filename_insert_sql(dbno: u32, filename: &str, version: u32) -> String {
    let mut sql = String::new();
    sql.push_str(&format!(r#"({},'{}',{}) ,"#, dbno, filename, version));
    sql
}

pub fn get_name(whole_attr: &DashMap<RefU64, WholeAttMap>, children_map: &HashMap<RefU64, RefU64Vec>, refno: RefU64) -> String {
    let attr = whole_attr.get(&refno).unwrap();
    let type_name = attr.implicit_attmap.get_type();
    return if let Some(name) = attr.explicit_attmap.get(&NounHash(db1_hash("NAME"))) {
        name.string_value().to_string()
    } else {
        let owner = attr.implicit_attmap.get_owner().unwrap();
        let mut idx = 1;
        if let Some(children) = children_map.get(&owner) {
            idx = children.iter().filter(|child| {
                if let Some(v) = whole_attr.get(child) {
                    whole_attr.get(child).unwrap().implicit_attmap.get_type() == type_name
                } else {
                    false
                }
            }).position(|node| node == &refno).unwrap_or_default() + 1;
        }
        format!("{} {}", type_name, idx)
    }
}