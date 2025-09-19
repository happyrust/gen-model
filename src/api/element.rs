use crate::data_interface::tidb_manager::AiosDBManager;
use aios_core::{NamedAttrMap, pdms_types::*};
use aios_core::{RefU64, RefU64Vec};
use std::collections::HashMap;

pub const ATT_DIVCO: i32 = 688051937;

/// 生成树结构的sql
pub fn gen_pdms_element_insert_sql(
    att: &NamedAttrMap,
    dbno: i32,
    children_map: &HashMap<RefU64, Vec<RefU64>>,
) -> String {
    let Some(refno) = att.get_refno().map(|x| x.refno()) else {
        return "".to_string();
    };
    let type_name = att.get_type();
    let owner = att.get_owner().refno();
    let name = cal_default_name(refno, &att, children_map);
    let order = get_order(refno, att, children_map);
    let children_count = children_map
        .get(&refno)
        .map(|x| x.len())
        .unwrap_or_default();

    let mut sql = String::new();
    let name = name.replace(r#"""#, "'");
    sql.push_str(&format!(
        r#"({}, "`{}`", "`{}`", {},"`{}`" , {} , {} , {} ,0 ) ,"#,
        refno.0,
        refno.to_pdms_str(),
        type_name,
        owner.0,
        name,
        dbno,
        order,
        children_count
    ));
    sql
}

///如果名称未给定，根据属性列表和children列表获得当前的元素的名称
pub fn cal_default_name(
    refno: RefU64,
    attr: &NamedAttrMap,
    children_map: &HashMap<RefU64, Vec<RefU64>>,
) -> String {
    let type_name = attr.get_type_str();
    return if let Some(name) = attr.get_name() {
        name
    } else {
        let owner = attr.get_owner().refno();
        let mut idx = 1;
        if let Some(children) = children_map.get(&owner) {
            idx = children
                .iter()
                .position(|node| *node == refno)
                .unwrap_or_default()
                + 1;
        }
        format!("{} {}", type_name, idx)
    };
}

///获得顺序值
#[inline]
pub fn get_order(
    refno: RefU64,
    attr: &NamedAttrMap,
    children_map: &HashMap<RefU64, Vec<RefU64>>,
) -> usize {
    let owner = attr.get_owner().refno();
    if let Some(children) = children_map.get(&owner) {
        return children
            .iter()
            .position(|child| *child == refno)
            .unwrap_or_default();
    }
    0
}
