use aios_core::pdms_types::RefU64;

pub fn gen_pdms_element_insert_sql(refno: RefU64, type_name: &str, owner: RefU64, name: Option<String>, dbno: u32, project: &str) -> Option<String> {
    let mut sql = String::new();
    let mut table_columns_sql = String::new();
    if let Some(_) = name {
        table_columns_sql.push_str("insert ignore into pdms_elements (id, refno, type, owner, name, dbno,project)");
    } else {
        table_columns_sql.push_str("insert ignore into pdms_elements (id, refno, type, owner, dbno,project)");
    }

    let mut table_vals_sql = String::new();
    if let Some(name) = name {
        table_vals_sql.push_str(&format!(r#"({}, '{}', '{}', {},'{}' ,{},'{}')"#,
                                         refno.0, refno.to_refno_str(), type_name, owner.0, name, dbno, project));
    } else {
        table_vals_sql.push_str(&format!(r#"({}, '{}', '{}', {},{},'{}')"#,
                                         refno.0, refno.to_refno_str(), type_name, owner.0, dbno, project));
    }

    sql.push_str(&table_columns_sql);
    sql.push_str(" values ");
    sql.push_str(&table_vals_sql);

    Some(sql)
}

pub fn gen_refno_infos_insert_sql(refno:RefU64,project:&str) -> Option<String> {
    let mut sql = String::new();
    let mut table_columns_sql = String::new();
    table_columns_sql.push_str("insert ignore into refno_infos (ref0,project)");

    let mut table_vals_sql = String::new();
    table_vals_sql.push_str(&format!(r#"({},'{}')"#,refno.get_0(),project));

    sql.push_str(&table_columns_sql);
    sql.push_str(" values ");
    sql.push_str(&table_vals_sql);

    Some(sql)
}

pub fn gen_dbno_filename_insert_sql(dbno:u32,filename:&str,version:u32) -> Option<String> {
    let mut sql = String::new();
    let mut table_columns_sql = String::new();
    table_columns_sql.push_str("insert ignore into dbno_filename (dbno,filename,version)");

    let mut table_vals_sql = String::new();
    table_vals_sql.push_str(&format!(r#"({},'{}',{});"#,dbno,filename,version));

    sql.push_str(&table_columns_sql);
    sql.push_str(" values ");
    sql.push_str(&table_vals_sql);

    Some(sql)
}