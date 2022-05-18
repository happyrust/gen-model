use aios_core::pdms_types::RefU64;

pub fn gen_query_implicit_attr_sql(refno: RefU64, type_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select * from {} where id = {}", type_name, refno.0));
    sql
}