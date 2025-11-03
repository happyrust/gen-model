use std::fs::File;
use std::io::Write;
use aios_core::options::DbOption;
use chrono::{Datelike, DateTime, Local, Timelike};
use crate::rvm::data_api::{gen_cnte_data, gen_end_data};

/// 生成 rvm 头部信息
pub fn create_head_data(db_option:&DbOption) -> Vec<u8> {
    let mut data = vec![];
    let project_name = &db_option.project_name;
    let mdb_name = &db_option.mdb_name;
    data.append(&mut create_head_version_data());
    data.append(&mut create_head_time_data());
    data.append(&mut create_computer_user_data());
    data.append(&mut create_project_info_data(project_name,mdb_name));
    data
}

// 在文件末尾将 ancestor 得 cntb 数量对齐
pub fn create_tail_data(cntb_count:usize) -> Vec<u8> {
    let mut data = vec![];
    for i in 0..cntb_count {
        data.append(&mut gen_cnte_data());
    }
    data.append(&mut gen_end_data());
    data
}

/// 生成版号信息
fn create_head_version_data() -> Vec<u8> {
    "HEAD\r\n     1     2\r\nAVEVA PDMS Design Mk12.1.SP4.0[4074]  (WINDOWS-NT 6.1)  (25 Jun 2013 : 20:47)\r\n\r\n".to_string().into_bytes()
}


fn create_head_time_data() -> Vec<u8> {
    let time: DateTime<Local> = Local::now();
    let current_time = time.format("%a %b %d %T %Y\r\n").to_string();
    current_time.into_bytes()
}

fn create_computer_user_data() -> Vec<u8> {
    "admin@admin \r\nUnicode UTF-8 \r\n".to_string().into_bytes()
}

fn create_project_info_data(project_name: &str, mdb: &str) -> Vec<u8> {
    format!("MODL\r\n     1     1\r\n{}\r\n/{}\r\n", project_name, mdb).into_bytes()
}

#[test]
fn test_create_head_data() -> anyhow::Result<()>{
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();

    let mut file = std::fs::File::create("test_rvm.txt").unwrap();
    let data = create_head_data(&db_option);
    file.write_all(&data).unwrap();
    Ok(())
}