#![feature(let_chains)]
#![feature(duration_constructors)]
// 暂时屏蔽warnings
#![allow(warnings)]
#![recursion_limit = "256"]

#[macro_use]
extern crate clap;
#[macro_use]
extern crate nom;

extern crate strum;

#[macro_use]
extern crate strum_macros;

use aios_core::aios_db_mgr::aios_mgr::AiosDBMgr;
#[cfg(feature = "gui")]
use aios_database::gui;
use std::fs;
use std::fs::{File, OpenOptions};
use std::future::Future;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

// use aios_core::material::save_all_material_data;
use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use aios_core::room::room::{load_aabb_tree, GLOBAL_AABB_TREE};
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::ssc_setting::{
    set_pbs_fixed_node, set_pbs_node, set_pbs_room_major_node, set_pbs_room_node,
    set_pdms_major_code,
};
use aios_core::tool::db_tool::{db1_dehash, db1_hash};
use aios_core::SUL_DB;
use aios_core::{build_cate_relate, get_db_option};
use aios_database::data_interface::tidb_manager::AiosDBManager;
use aios_database::fast_model::cal_model::{update_cal_bran_component, update_cal_equip};
#[cfg(feature = "gen_model")]
use aios_database::fast_model::gen_all_geos_data;
use aios_database::fast_model::room_model::build_room_relations;
use aios_database::fast_model::{
    gen_inst_meshes, process_meshes_update_db_deep, EXIST_MESH_GEO_HASHES,
};
use aios_database::versioned_db::database::*;
// use aios_database::versioned_db::task::initialize_global_db_sender;
use aios_database::run_cli;
use aios_database::team_data::sync_system_db;
use bevy_reflect::List;
use chrono::{Datelike, Local, Timelike};
use futures::StreamExt;
use itertools::Itertools;
use log::{error, LevelFilter};
use simplelog::*;
use surrealdb::opt::auth::Root;

#[cfg(feature = "gui")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    gui::run_gui();
    Ok(())
}

#[cfg(not(feature = "gui"))]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use std::sync::mpsc;

    let db_option: DbOption = get_db_option().clone();
    let (tx, mut rx) = mpsc::channel::<i32>();
    run_cli(db_option, tx).await
}

#[test]
fn get_noun_hash() {
    let noun = "DB";
    let hash = db1_hash(noun);
    dbg!(hash);
    let hashes = [919309, 640481, 919399];
    for hash in hashes {
        let str = db1_dehash(hash);
        dbg!(&hash);
        dbg!(str);
    }
}

#[test]
fn test_time() {
    use chrono::prelude::*;
    let local: DateTime<Local> = Local::now();
    println!(
        "year:{} , month: {} , day: {}, week_day:{},hour:{} , min: {} , sec:{}",
        local.year(),
        local.month(),
        local.day(),
        local.weekday(),
        local.hour(),
        local.minute(),
        local.second()
    );
}

/// 将 all_attr_info.bin 文件转成 json
#[test]
fn test_turn_bin_into_json() {
    let mut file = File::open("all_attr_info.bin").unwrap();
    let mut data = vec![];
    file.read_to_end(&mut data).unwrap();
    let map = bincode::deserialize::<PdmsDatabaseInfo>(&data).unwrap();
    let json = serde_json::to_string(&map).unwrap();
    let mut new_file = File::create("all_attr_info_1.json").unwrap();
    new_file.write_all(&json.into_bytes()).unwrap();
}
