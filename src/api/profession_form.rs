use std::collections::HashMap;

fn gen_create_table_sql(map:&HashMap<String,String>,form_name:&str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("CREATE TABLE IF NOT EXISTS {}",form_name));
    for key in map.keys() {
        sql.push_str(&format!("{} varchar(100),",key));
    }
    sql.remove(sql.len() -1);
    sql.push_str(");");
    sql
}

fn gen_insert_to_db_sql(map:Vec<HashMap<String,String>>,form_name:&str) -> String {
    "unset".to_string()
}