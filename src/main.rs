use std::collections::{BTreeMap, HashSet};
use std::fmt::format;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use aios_core::pdms_types::{AttrMap, AttrVal, NounHash, PdmsDatabaseInfo, RefI32Tuple, RefU64};
use aios_core::pdms_types::AttrVal::StringType;
use aios_core::tool::db_tool::db1_hash;
use parse_pdms_db::local_db::DbOption;
use parse_pdms_db::parse::PdmsDbData;
use aios_database::tables;
use parse_pdms_db::{db1_dehash, parse_file};
use parse_pdms_db::tool::hash_tool::{f32_round_2, f64_round_2, f64_round_3};
use sqlx::{MySql, MySqlPool, Pool};
use sqlx::pool::PoolConnection;
use aios_database::consts::URL;
use aios_database::database::{get_tidb_pool, init_database, init_info_database, set_connect_url};
use aios_database::insert_sql::{gen_dbno_filename_insert_sql, gen_pdms_element_insert_sql, gen_refno_infos_insert_sql};

pub const TYPE_HASH: u32 = db1_hash("TYPE");




#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    dbg!(&db_option);
    let mut time = Instant::now();

    let url = set_connect_url(&db_option.ip,&db_option.user,&db_option.password,"",&db_option.port);
    let info_pool = get_tidb_pool(&format!("{}/{}", url, "refno_infos")).await;
    dbg!(&url);
    init_info_database(&url).await;
    for project in &db_option.included_projects {
        init_database(project,&url).await;
        let project_pool = get_tidb_pool(&format!("{}/{}", url, project)).await;
        // let mut conn = project_pool.try_acquire().unwrap();
        println!("初始化数据库时间: {} ms", time.elapsed().as_millis());
        let connection_string = format!("{url}/{project}");
        if let Ok(db_info) = bincode::deserialize::<PdmsDatabaseInfo>(include_bytes!("../all_attr_info.bin")) {
            for (k, v) in db_info.noun_attr_info_map {
                let mut attr_map = BTreeMap::new();
                let type_name = db1_dehash(k as u32).to_lowercase();
                if type_name.is_empty() {
                    continue;
                }
                let mut tmp_sets = HashSet::new();
                for (kk, vv) in v {
                    let att_name = vv.name.to_lowercase();
                    if att_name.starts_with(":") || vv.offset == 0 {
                        continue;
                    }
                    if !tmp_sets.contains(&att_name) {
                        tmp_sets.insert(att_name.clone());
                    } else {
                        continue;
                    }
                    if kk == TYPE_HASH as i32 {
                        attr_map.insert(vv.offset, (att_name, StringType(db1_dehash(k as u32).to_lowercase().into())));
                    } else {
                        attr_map.insert(vv.offset, (att_name, vv.default_val));
                    }
                }
                // tables::create_implicit_tables(connection_string.as_str(), type_name.as_str(), &attr_map).await;
                // let mut conn = pool.try_acquire().unwrap();
                tables::create_implicit_tables(&mut project_pool.acquire().await.unwrap(), type_name.as_str(), &attr_map).await;
                tables::create_explicit_data_tables( &mut project_pool.acquire().await.unwrap()).await;
                tables::create_uda_data_tables(&mut project_pool.acquire().await.unwrap()).await;
                tables::create_dbno_filename_tables(&mut project_pool.acquire().await.unwrap()).await;
            }
        }
        tables::create_element_tables(&mut project_pool.acquire().await.unwrap()).await;
        sync_total(&db_option, project, &None, project_pool,info_pool.clone()).await;
    }
    Ok(())
}

pub fn gen_explicit_att_insert_sql(refno: RefU64, type_name: &str, owner: RefU64, e_att: &AttrMap) -> Option<String> {
    let mut sql = String::new();
    let mut table_columns_sql = String::new();
    let table_name = type_name.replace("join", "joint");
    table_columns_sql.push_str("insert ignore into explicit_att (id, refno, type, owner, data)");

    let mut table_vals_sql = String::new();
    let data = hex::encode(bincode::serialize(e_att).unwrap());
    table_vals_sql.push_str(&format!(r#"({}, '{}', '{}', {}, 0x{})"#, refno.0, refno.to_refno_str(), type_name, owner.0, data));


    sql.push_str(&table_columns_sql);
    sql.push_str(" values ");
    sql.push_str(&table_vals_sql);


    Some(sql)
}

pub fn gen_implicit_att_insert_sql(refno: RefU64, type_name: &str, owner: RefU64, i_att: &AttrMap, columns_sql: &mut Option<String>) -> Option<String> {
    let mut sql = String::new();
    if columns_sql.is_none() {
        let mut table_columns_sql = String::new();
        let table_name = i_att.get_type().to_lowercase().replace("join", "joint");
        let table_name = table_name.replace("loop","loop_");
        table_columns_sql.push_str(&format!("insert ignore into {} (id, refno, type, owner,", table_name));
        for (k, v) in &i_att.map {
            let mut att_name_full = db1_dehash(k.0).to_lowercase();
            if att_name_full.as_str() == "numbdb" {
                att_name_full = "dbno".to_string();
            }
            let att_name = att_name_full.replace("desc", "desc_").replace("lock", "lock_").replace("char", "char_");
            if att_name.starts_with(":") || att_name.as_str() == "refno" || att_name.as_str() == "type" || att_name.as_str() == "owner" {
                continue;
            }
            match v {
                AttrVal::InvalidType => {}

                _ => {
                    table_columns_sql.push_str(&format!("{},", att_name));
                }
            }
        }
        table_columns_sql.remove(table_columns_sql.len() - 1);
        table_columns_sql.push_str(") ");
        *columns_sql = Some(table_columns_sql);
    }

    let mut table_vals_sql = String::new();

    table_vals_sql.push_str(&format!(r#"({}, '{}', '{}', {},"#, refno.0, refno.to_refno_str(), type_name, owner.0));
    for (k, v) in &i_att.map {
        let att_name = db1_dehash(k.0).to_lowercase()
            .replace("desc", "desc_")
            .replace("lock", "lock_")
            .replace("char", "char_");
        let att_name = att_name.as_str();
        if att_name.starts_with(":") || att_name == "refno" || att_name == "type" || att_name == "owner" {
            continue;
        }
        match v {
            AttrVal::InvalidType => {}
            AttrVal::IntegerType(d) => {
                table_vals_sql.push_str(&format!("{},", d.to_string()));
            }
            AttrVal::StringType(d) => {
                table_vals_sql.push_str(&format!(r#"'{}',"#, d));
            }
            AttrVal::DoubleType(d) => {
                table_vals_sql.push_str(&format!("{},", f64_round_3(*d)));
            }
            AttrVal::DoubleArrayType(d) => {
                table_vals_sql.push_str(&format!(r#"0x{},"#, hex::encode(bincode::serialize(d).unwrap().as_slice())));
            }
            AttrVal::StringArrayType(d) => {
                table_vals_sql.push_str(&format!(r#"'{}',"#, serde_json::to_string(d).unwrap()));
            }
            AttrVal::BoolArrayType(d) => {
                table_vals_sql.push_str(&format!(r#"'{}',"#, serde_json::to_string(d).unwrap()));
            }
            AttrVal::IntArrayType(d) => {
                table_vals_sql.push_str(&format!(r#"'{}',"#, serde_json::to_string(d).unwrap()));
            }
            AttrVal::BoolType(d) => {
                let b = if *d { 1 } else { 0 };
                table_vals_sql.push_str(&format!("{},", b));
            }
            AttrVal::Vec3Type(d) => {
                table_vals_sql.push_str(&format!(r#"'{}',"#, serde_json::to_string(d).unwrap()));
            }
            AttrVal::ElementType(d) => {
                table_vals_sql.push_str(&format!(r#"'{}',"#, d));
            }
            AttrVal::WordType(d) => {
                table_vals_sql.push_str(&format!(r#"'{}',"#, d));
            }
            AttrVal::RefU64Type(d) => {
                table_vals_sql.push_str(&format!("{},", d.0));
            }
            AttrVal::StringHashType(_) => {}
        }
    }

    table_vals_sql.remove(table_vals_sql.len() - 1);
    table_vals_sql.push_str(")");


    sql.push_str(columns_sql.as_ref().unwrap());
    sql.push_str(" values ");
    sql.push_str(&table_vals_sql);


    Some(sql)
}

pub async fn sync_total(db_option: &DbOption, project: &str, need_parsing_files: &Option<Vec<String>>,pool:Pool<MySql>,info_pool:Pool<MySql>) -> anyhow::Result<()> {
    let mut data_dir = Path::new(&db_option.project_path);
    let project_dir = data_dir.join(&project);
    let mut target_dir = fs::read_dir(&project_dir).unwrap().into_iter().map(|entry| {
        let entry = entry.unwrap();
        entry.path()
    }).find(|x| x.file_name().unwrap().to_str().unwrap().ends_with("000")).unwrap();

    let mut children_files = fs::read_dir(target_dir)?.into_iter().map(|entry| {
        let entry = entry.unwrap();
        entry.path()
    }).collect::<Vec<PathBuf>>();

    let mut handles = vec![];
    let project = Arc::new(project.to_string());
    let url = set_connect_url(&db_option.ip,&db_option.user,&db_option.password,"",&db_option.port);
    for path in children_files {
        let file_name = path.file_name().unwrap().to_str().unwrap().to_string();
        let file_name_clone = Arc::new(file_name.clone());
        if !file_name.ends_with("com") && !file_name.ends_with("mis") {
            if need_parsing_files.is_none() || need_parsing_files.as_ref().unwrap().contains(&file_name) {
                println!("path={:?}", &file_name);
                let project  = project.clone();
                let project_clone = project.to_string();
                let pool_clone = pool.clone();
                let info_pool_clone = info_pool.clone();
                let filename_clone = file_name_clone.clone();
                let handle = tokio::spawn(async move {
                    //后面再考虑成不同的table，如显示属性和隐藏属性
                    if let Ok(Ok(PdmsDbData {
                                  all_attr_map,
                                  total_attr_map,
                                  ele_id_tree,
                                  type_ele_map,
                                  refno_node_id_map,
                                  string_lookup,
                                  refno_info_map,
                                  children_map,
                                  db_no,
                                  field_no,
                                  version,
                                  room_code_map,
                                  ..
                              })) = tokio::task::spawn_blocking(move || {
                        parse_file(&path, &None, &file_name, &project_clone.as_str(), "")
                    }).await{

                        for kv in &total_attr_map {
                            let mut columns_sql = None;
                            let i_att = &kv.implicit_attmap;
                            let refno = i_att.get_refno().unwrap();
                            let type_name = i_att.get_type();
                            let owner = i_att.get_owner().unwrap();
                            let sql = gen_implicit_att_insert_sql(refno, type_name, owner, i_att, &mut columns_sql).unwrap_or_default();
                            let result = sqlx::query(&sql).execute(&mut pool_clone.clone().acquire().await.unwrap()).await;
                            match result {
                                Ok(_) => {}
                                Err(_) => {
                                    dbg!(i_att.to_string_hashmap());
                                    dbg!(sql.as_str());
                                }
                            }
                            let e_att = &kv.explicit_attmap;
                            let sql = gen_explicit_att_insert_sql(refno, type_name, owner, e_att).unwrap_or_default();
                            let result = sqlx::query(&sql).execute(&mut pool_clone.clone().acquire().await.unwrap()).await;
                            match result {
                                Ok(_) => {}
                                Err(_) => {
                                    dbg!(e_att.to_string_hashmap());
                                    dbg!(sql.as_str());
                                }
                            }
                            if let Some(name) = e_att.get(&NounHash(db1_hash("NAME"))) {
                                let name = name.string_value().to_string();
                                let sql = gen_pdms_element_insert_sql(refno, type_name, owner, Some(name), db_no.0, &project.clone()).unwrap_or_default();
                                let result = sqlx::query(&sql).execute(&mut pool_clone.clone().acquire().await.unwrap()).await;
                                match result {
                                    Ok(_) => {}
                                    Err(_) => {
                                        dbg!(sql.as_str());
                                    }
                                }
                            } else {
                                let sql = gen_pdms_element_insert_sql(refno, type_name, owner, None, db_no.0, &project.clone()).unwrap_or_default();
                                let result = sqlx::query(&sql).execute(&mut pool_clone.clone().acquire().await.unwrap()).await;
                                match result {
                                    Ok(_) => {}
                                    Err(_) => {
                                        dbg!(sql.as_str());
                                    }
                                }
                            }

                            let sql = gen_refno_infos_insert_sql(refno,&project).unwrap_or_default();
                            let result = sqlx::query(&sql).execute(&mut info_pool_clone.clone().acquire().await.unwrap()).await;
                            match result {
                                Ok(_) => {}
                                Err(_) => {
                                    dbg!(sql.as_str());
                                }
                            }
                        }
                        let sql = gen_dbno_filename_insert_sql(db_no.0,&filename_clone,version.0).unwrap_or_default();
                        let result = sqlx::query(&sql).execute(&mut pool_clone.clone().acquire().await.unwrap()).await;
                        match result {
                            Ok(_) => {}
                            Err(_) => {
                                dbg!(sql.as_str());
                            }
                        }
                    }
                });
                handles.push(handle);
            }
        }
    }

    futures::future::join_all(handles).await;

    Ok(())
}