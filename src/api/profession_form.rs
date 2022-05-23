use std::collections::HashMap;

fn gen_create_table_sql(map: &HashMap<String, String>, form_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("CREATE TABLE IF NOT EXISTS {}", form_name));
    for key in map.keys() {
        sql.push_str(&format!("{} varchar(100),", key));
    }
    sql.remove(sql.len() - 1);
    sql.push_str(");");
    sql
}

fn gen_insert_to_db_sql(map: Vec<HashMap<String, String>>, form_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("insert ignore into {} ( ", form_name));
    let mut key_vec = vec![];
    for keys in map[0].clone().keys() {
        key_vec.push(keys.clone());
        sql.push_str(&format!("{} ,", keys));
    }
    sql.remove(sql.len() - 1);
    sql.push_str(") Values");
    for vals in map {
        sql.push_str("(");
        for key in &key_vec {
            if let Some(val) = vals.get(key) {
                sql.push_str(&format!("{} ,", val));
            }
        }
        sql.remove(sql.len() - 1);
        sql.push_str("),");
    }
    sql.remove(sql.len() - 1);
    sql.push_str(";");
    sql
}

#[test]
fn test_gen_insert_to_db_sql() {
    let mut val = vec![];
    let mut map = HashMap::new();
    map.insert("a".to_string(), "b".to_string());
    map.insert("c".to_string(), "d".to_string());
    val.push(map);
    let mut map = HashMap::new();
    map.insert("a".to_string(), "f".to_string());
    map.insert("c".to_string(), "h".to_string());
    val.push(map);
    let sql = gen_insert_to_db_sql(val, "test");
    println!("sql={}", sql);
}