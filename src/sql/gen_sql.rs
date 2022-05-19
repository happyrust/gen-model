use aios_core::pdms_types::RefU64;

pub fn gen_query_implicit_attr_sql(refno: RefU64, type_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select * from {} where id = {}", type_name, refno.0));
    sql
}

pub fn gen_query_refno_type_sql(refno:RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select type from pdms_elements where id = {}", refno.0));
    sql
}

pub fn gen_query_type_refnos_sql(type_name:&str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select id from pdms_elements where type = '{}' ;", type_name));
    sql
}

pub fn gen_query_explicit_attr_sql(refno:RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select * from explicit_att where id = {} ;", refno.0));
    sql
}