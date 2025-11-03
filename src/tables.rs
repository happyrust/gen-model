use crate::consts::*;
use aios_core::AttrVal;
use aios_core::helper::table::{qualified_column_name, qualified_table_name};
use aios_core::helper::*;
use aios_core::helper::*;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

#[inline]
pub fn gen_create_explicit_tables_sql() -> String {
    let mut sql = String::new();
    //后续可以创建一个owner表
    sql.push_str(&format!(
        r#"CREATE TABLE IF NOT EXISTS {PDMS_EXPLICIT_TABLE} ("#
    ));
    sql.push_str(&format!(r#"{} BIGINT NOT NULL PRIMARY KEY,"#, "ID")); //refno 的64位
    sql.push_str(&format!(r#"{} VARCHAR(30),"#, "REFNO"));
    sql.push_str(&format!(r#"{} VARCHAR(8),"#, "TYPE"));
    sql.push_str(&format!(r#"{} BIGINT,"#, "OWNER"));
    sql.push_str(&format!(r#"{} BLOB"#, "DATA"));
    sql.push_str(");");

    sql
}

#[inline]
pub fn gen_create_uda_tables_sql() -> String {
    let mut sql = String::new();
    //后续可以创建一个owner表
    sql.push_str(&format!(
        r#"CREATE TABLE IF NOT EXISTS {PDMS_UDA_ATT_TABLE} ("#
    ));
    sql.push_str(&format!(r#"{} INT NOT NULL PRIMARY KEY,"#, "TYPE")); //refno 的64位
    sql.push_str(&format!(r#"{} BLOB"#, "DATA"));
    sql.push_str(");");

    sql
}

#[inline]
pub fn gen_create_dbno_infos_tables_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!(
        r#"CREATE TABLE IF NOT EXISTS {PDMS_DBNO_INFOS_TABLE} ("#
    ));
    sql.push_str(&format!(r#"{} VARCHAR(100) PRIMARY KEY,"#, "id"));
    sql.push_str(&format!(r#"{} INT,"#, "NUMBDB"));
    sql.push_str(&format!(r#"{} VARCHAR(30),"#, "FILENAME"));
    sql.push_str(&format!(r#"{} INT, "#, "VERSION"));
    sql.push_str(&format!(r#"{} VARCHAR(30) ,"#, "PROJECT"));
    sql.push_str(&format!(r#"{} VARCHAR(10) "#, "DB_TYPE"));
    sql.push_str(");");

    // sql.push_str(&format!("CREATE INDEX INFO_DB_TYPE_IDX ON {PDMS_DBNO_INFOS_TABLE} (DB_TYPE);"));
    // sql.push_str(&format!("CREATE INDEX INFO_DBNO_IDX ON {PDMS_DBNO_INFOS_TABLE} (NUMBDB);"));
    sql
}

#[inline]
pub fn gen_create_element_tables_sql() -> String {
    let mut sql = String::new();
    //后续可以创建一个owner表
    sql.push_str(&format!(
        r#"CREATE TABLE IF NOT EXISTS {PDMS_ELEMENTS_TABLE} ("#
    ));
    sql.push_str(&format!(r#"{} BIGINT NOT NULL PRIMARY KEY,"#, "ID")); //refno 的64位
    sql.push_str(&format!(r#"{} VARCHAR(30),"#, "REFNO"));
    sql.push_str(&format!(r#"{} VARCHAR(8),"#, "TYPE"));
    sql.push_str(&format!(r#"{} BIGINT,"#, "OWNER"));
    sql.push_str(&format!(r#"{} VARCHAR(100),"#, "NAME"));
    // world position 是否需要存储
    // sql.push_str(&format!(r#"{} INT ,"#, "NUMBDB"));
    sql.push_str(&format!(r#"{} INT ,"#, "NUMBDB"));
    sql.push_str(&format!(r#"{} INT ,"#, "ORDER_NUM"));
    sql.push_str(&format!(r#"{} INT ,"#, "CHILDREN_COUNT"));
    sql.push_str(&format!(r#"{} TINYINT(1) "#, "IS_DEL"));
    sql.push_str(");");

    sql.push_str(&format!(
        "CREATE INDEX  ELE_TYPE_IDX ON {PDMS_ELEMENTS_TABLE} (TYPE);"
    ));
    sql.push_str(&format!(
        "CREATE INDEX  ELE_DBNO_IDX ON {PDMS_ELEMENTS_TABLE} (NUMBDB);"
    ));
    sql.push_str(&format!(
        "CREATE INDEX  ELE_OWNER_IDX ON {PDMS_ELEMENTS_TABLE} (OWNER);"
    ));
    sql
}

#[inline]
pub fn gen_create_ssc_element_tables_sql() -> String {
    let mut sql = String::new();
    //后续可以创建一个owner表
    sql.push_str(&format!(
        r#"CREATE TABLE IF NOT EXISTS {PDMS_SSC_ELEMENTS_TABLE} ("#
    ));
    sql.push_str(&format!(r#"{} BIGINT AUTO_INCREMENT PRIMARY KEY,"#, "ID")); //refno 的64位
    sql.push_str(&format!(r#"{} VARCHAR(30),"#, "REFNO"));
    sql.push_str(&format!(r#"{} VARCHAR(8),"#, "TYPE"));
    sql.push_str(&format!(r#"{} BIGINT,"#, "OWNER"));
    sql.push_str(&format!(r#"{} VARCHAR(100),"#, "NAME"));
    sql.push_str(&format!(r#"{} BIGINT,"#, "REAL_PDMS_REFNO"));
    sql.push_str(&format!(r#"{} INT"#, "ORDER_NUM"));
    sql.push_str(");");

    // sql.push_str(&format!("CREATE INDEX  ELE_TYPE_IDX ON {PDMS_SSC_ELEMENTS_TABLE} (TYPE);"));
    // sql.push_str(&format!("CREATE INDEX  ELE_OWNER_IDX ON {PDMS_SSC_ELEMENTS_TABLE} (OWNER);"));
    sql
}

/// 创建 数据状态表
pub fn gen_create_data_state_tables_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!(
        r#"CREATE TABLE IF NOT EXISTS {PDMS_DATA_STATE} ("#
    ));
    sql.push_str(&format!(r#"{} BIGINT NOT NULL PRIMARY KEY,"#, "ID"));
    // sql.push_str(&format!(r#"{} VARCHAR(8),"#, "TYPE"));
    // sql.push_str(&format!(r#"{} VARCHAR(100),"#, "NAME"));
    sql.push_str(&format!(r#"{} VARCHAR(50)"#, "STATE"));
    sql.push_str(");");
    sql
}

#[inline]
pub fn gen_create_project_mdb_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!(
        r#"CREATE TABLE IF NOT EXISTS {PDMS_PROJECT_MDB_TABLE} ("#
    ));
    sql.push_str(&format!(r#"{} BIGINT NOT NULL PRIMARY KEY,"#, "ID"));
    sql.push_str(&format!(r#"{} INT,"#, "DB_NUM"));
    sql.push_str(&format!(r#"{} VARCHAR(100) ,"#, "MDB_NAME"));
    sql.push_str(&format!(r#"{} VARCHAR(50) ,"#, "REFNO"));
    sql.push_str(&format!(r#"{} VARCHAR(100) ,"#, "PROJECT"));
    sql.push_str(&format!(r#"{} VARCHAR(50) ,"#, "WORLD_REFNO"));
    sql.push_str(&format!(r#"{} VARCHAR(50) ,"#, "DB_TYPE"));
    sql.push_str(&format!(r#"{} INT"#, "ORDER_NUM"));
    sql.push_str(");");
    sql
}

#[inline]
pub fn gen_create_project_mdb_json_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!(
        r#"CREATE TABLE IF NOT EXISTS {PDMS_PROJECT_MDB_TABLE_JSON} ("#
    ));
    sql.push_str(&format!(r#"{} VARCHAR(20) ,"#, "MDB_NAME"));
    sql.push_str(&format!(r#"{} VARCHAR(10) ,"#, "DB_TYPE"));
    sql.push_str(&format!(r#"{} VARCHAR(1000) "#, "DATA"));
    sql.push_str(");");

    // sql.push_str(&format!("CREATE INDEX PROJ_MDB_DB_TYPE_IDX ON {PDMS_PROJECT_MDB_TABLE_JSON} (DB_TYPE);"));
    sql
}

#[inline]
pub fn gen_create_implicit_tables_sql(
    type_name: &str,
    att_map: &BTreeMap<u32, (String, AttrVal)>,
) -> String {
    let mut sql = String::new();
    let table_name = qualified_table_name(type_name);
    let table_name = table_name.as_str();
    //后续可以创建一个owner表
    sql.push_str(&format!(r#"CREATE TABLE IF NOT EXISTS {} ("#, table_name));
    sql.push_str(&format!(r#"{} BIGINT NOT NULL PRIMARY KEY,"#, "ID")); //refno 的64位
    sql.push_str(&format!(r#"{} VARCHAR(30),"#, "REFNO")); //refno
    sql.push_str(&format!(r#"{} VARCHAR(8),"#, "TYPE"));
    sql.push_str(&format!(r#"{} BIGINT NOT NULL,"#, "OWNER"));

    for (offset, (k, v)) in att_map {
        let att_name = qualified_column_name(k);

        match v {
            AttrVal::InvalidType => {}
            AttrVal::IntegerType(_) => {
                sql.push_str(&format!(r#"{} INT,"#, att_name));
            }
            AttrVal::StringType(_) => {
                //根据不同类型优化一下string的大小
                sql.push_str(&format!(r#"{} VARCHAR(500),"#, att_name));
            }
            AttrVal::DoubleType(_) => {
                sql.push_str(&format!(r#"{} DOUBLE,"#, att_name));
            }
            AttrVal::DoubleArrayType(_) => {
                sql.push_str(&format!(r#"{} BLOB,"#, att_name));
            }
            AttrVal::StringArrayType(_) => {
                sql.push_str(&format!(r#"{} VARCHAR(300),"#, att_name)); //暂时用blob来表示，至于需不需要分表，看情况
            }
            AttrVal::BoolArrayType(_) => {
                sql.push_str(&format!(r#"{} INT,"#, att_name));
            }
            AttrVal::IntArrayType(_) | AttrVal::RefU64Array(_) => {
                sql.push_str(&format!(r#"{} VARCHAR(100),"#, att_name));
            }
            AttrVal::BoolType(_) => {
                sql.push_str(&format!(r#"{} TINYINT(1),"#, att_name));
            }
            AttrVal::Vec3Type(_) => {
                sql.push_str(&format!(r#"{} VARCHAR(100),"#, att_name));
            }
            AttrVal::ElementType(_) => {
                sql.push_str(&format!(r#"{} BIGINT,"#, att_name));
            }
            AttrVal::WordType(_) => {
                sql.push_str(&format!(r#"{} VARCHAR(50),"#, att_name));
            }
            AttrVal::RefU64Type(_) => {
                sql.push_str(&format!(r#"{} BIGINT,"#, att_name));
            }
            AttrVal::StringHashType(_) => {}
            _ => {}
        }
    }

    sql.remove(sql.len() - 1);
    sql.push_str(");");

    // sql.push_str(&format!("CREATE INDEX {type_name}_OWNER_IDX ON {table_name} (owner);"));
    // sql.push_str(&format!("CREATE INDEX {type_name}_TYPE_IDX ON {table_name} (type);"));

    sql
}

pub fn gen_create_room_code_table_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("CREATE TABLE IF NOT EXISTS {ROOM_CODE} ("));
    sql.push_str(&format!("{} BIGINT ,", "REFNO"));
    sql.push_str(&format!("{} VARCHAR(50) ", "ROOM_NAME"));
    sql.push_str(");");
    sql
}

pub fn gen_create_version_info_table_sql(project_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("CREATE TABLE IF NOT EXISTS {PDMS_VERSION}("));
    sql.push_str(&format!("{} VARCHAR(20) ,", "PROJECT"));
    sql.push_str(&format!("{} INT", "VERSION"));
    sql.push_str(");");
    sql
}

/// 创建版本管理的数据表
pub fn gen_create_pdms_version_table_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("CREATE TABLE IF NOT EXISTS {VERSION_DATA} ("));
    sql.push_str(&format!("{} BIGINT PRIMARY KEY AUTO_INCREMENT ,", "ID"));
    sql.push_str(&format!("{} BIGINT ,", "REFNO"));
    sql.push_str(&format!("{} INT ,", "BIG_VERSION"));
    sql.push_str(&format!("{} INT ,", "SMALL_VERSION"));
    sql.push_str(&format!("{} INT ,", "PDMS_VERSION"));
    sql.push_str(&format!("{} SMALLINT ,", "OPERATE"));
    sql.push_str(&format!("{} BLOB", "DATA"));
    sql.push_str(");");
    sql
}

/// 创建存储每个文件的版本号表
pub fn gen_create_file_version_table_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!(
        "CREATE TABLE IF NOT EXISTS {PDMS_FILE_VERSION_TABLE} ("
    ));
    sql.push_str(&format!("{} VARCHAR(20) ,", "FILENAME"));
    sql.push_str(&format!("{} INT", "VERSION"));
    sql.push_str(");");
    sql
}

pub fn gen_create_pdms_mesh_table_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("CREATE TABLE IF NOT EXISTS {PDMS_MESH} ("));
    sql.push_str(&format!("{} BIGINT UNSIGNED  PRIMARY KEY ,", "HASH"));
    sql.push_str(&format!("{} BLOB ", "MESH"));
    sql.push_str(");");
    sql
}

#[test]
fn test_replace() {
    let mut r = "a '' b c a";
    let v = r.replace(r"'", "e");
    dbg!(&v);
}
