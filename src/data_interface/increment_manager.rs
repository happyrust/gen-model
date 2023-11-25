use aios_core::cache::mgr::BytesTrait;
use aios_core::cache::refno::CachedRefBasic;
use aios_core::consts::NAME_HASH;
use aios_core::helper::qualified_table_name;
use aios_core::pdms_types::*;
use anyhow::anyhow;
use indexmap::{IndexMap, IndexSet};
use notify::{RecursiveMode, Watcher};
use pdms_io::io::PdmsIO;
use pdms_io::watch::PdmsWatcher;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
// use pdms_io::watch::PdmsWatcher;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::database::sync_total_async_threaded;
use futures::{
    channel::mpsc::{channel, Receiver},
    future::ok,
    SinkExt, StreamExt,
};
// use pdms_io::io::PdmsIO;
use crate::consts::*;
use crate::data_interface::gen_model::gen_all_geos_data;
use crate::data_interface::increment_record::{IncrEleUpdateLog, IncrGeoUpdateLog};
use crate::defines::CACHED_REFNO_BASIC_MAP;
use crate::graph_db::pdms_arango::{remove_edges_arangodb, save_arangodb_with_db_option};
use crate::graph_db::structs::{PdmsEleData, PdmsEleEdge};
use crate::surreal_service;
use crate::surreal_service::SUL_DB;
use aios_core::orm::pdms_element;
use aios_core::pe::SPdmsElement;
use aios_core::tool::db_tool::db1_dehash;
use aios_core::{AttrMap, AttrVal, get_db_option, RefU64Vec};
use itertools::Itertools;
use pdms_io::defines::DbPageBasicInfo;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;
use pdms_io::sync::compress::{CompressOptions, execute_compress};
use rumqttc::QoS;
use surrealdb::sql::Thing;
use tokio::task::JoinSet;
use walkdir::WalkDir;
use crate::mqtt_service::SyncE3dFileMsg;

#[derive(Debug, Default, Clone)]
pub struct IncrementInfo {
    pub refno: RefU64,
    pub db_no: i32,
    pub attr: NamedAttrMap,
    pub children: RefU64Vec,
    pub operation: EleOperation,
}



impl AiosDBManager {
    ///执行增量更新
    pub async fn execute_incr_update(
        &self,
        increment_ranges_map: IndexMap<PathBuf, (DbPageBasicInfo, u32)>,
    ) -> anyhow::Result<bool> {
        if increment_ranges_map.is_empty() {
            return Ok(true);
        }
        let mut modify_type_eles_map = HashMap::new();
        let mut added_type_eles_map = HashMap::new();
        let mut delete_keys = vec![];
        let mut delete_maps: HashMap<RefU64, IncrementInfo> = HashMap::new();
        let mut deleted_refnos_set = HashSet::new();

        let mut total_add_len = 0;
        let mut total_modify_len = 0;
        let mut total_deleted_len = 0;
        let mut geo_update_log = IncrGeoUpdateLog::default();
        let mut owner_children_map = IndexMap::new();
        let mut deleted_owner_set = IndexSet::new();
        let mut all_relate_sqls = vec![];
        for (path, (basic_info, last_pageno)) in increment_ranges_map {
            let mut io = PdmsIO::new(path, true);
            io.open()?;
            let eles = io.collect_increment_eles(&basic_info, Some(last_pageno))?;
            dbg!(eles.len());
            for ele in eles {
                let mut attmap: NamedAttrMap = ele.whole_attmap.merge().into();
                attmap.set_e3d_version(ele.version as _);
                let owner = attmap.get_owner();
                let refno = ele.refno;
                let type_name = attmap.get_type();
                let type_name = type_name.as_str();
                // dbg!(ele.refno);
                // dbg!(&attmap);
                let mut ele_op = EleOperation::Modified;
                // 删除只是owner的children变化了，但是需要记录删除的节点
                // 只要有返回，说明节点存在，返回为空，说明是叶子节点
                if let Ok(old_refnos) = surreal_service::get_children_refnos(ele.refno).await {
                    //检查是否有删除的
                    old_refnos
                        .iter()
                        .filter(|x| !ele.children.contains(*x))
                        .for_each(|&x| {
                            let key = x.hash_with_another_refno(ele.refno);
                            delete_keys.push(key.to_string());
                            deleted_refnos_set.insert(x);
                            //执行的是父节点的操作
                            ele_op = EleOperation::Deleted;
                            dbg!(x);
                            delete_maps.entry(x).or_insert(IncrementInfo {
                                refno: x,
                                db_no: basic_info.pdms_header.db_num,
                                attr: Default::default(),
                                children: Default::default(),
                                operation: EleOperation::Deleted,
                            });
                            //方便处理删除后模型更新的情况
                            deleted_owner_set.insert(owner);
                            // owner_children_map.entry(owner).or_insert_with(HashSet::new).insert(refno);
                        });
                } else {
                    ele_op = EleOperation::Add;
                }

                //暂时处理好新增的情况
                //按照owner 先排序
                if PRIMITIVE_NOUN_NAMES.contains(&type_name) {
                    geo_update_log.prim_refnos.insert(refno);
                } else if GNERAL_LOOP_NOUN_NAMES.contains(&type_name) {
                    geo_update_log.loop_refnos.insert(refno);
                } else if CATA_HAS_TUBI_GEO_NAMES.contains(&type_name) {
                    geo_update_log.bran_hanger_refnos.insert(refno);
                } else if CATA_GEO_NAMES.contains(&type_name) {
                    geo_update_log.basic_cata_refnos.insert(refno);
                    owner_children_map
                        .entry(attmap.get_owner())
                        .or_insert_with(HashSet::new)
                        .insert(refno);
                }else{
                    owner_children_map
                        .entry(attmap.get_owner())
                        .or_insert_with(HashSet::new)
                        .insert(refno);
                }

                //创建relate关系
                if ele_op == EleOperation::Deleted {
                    //如果是负实体这种发生修改，需要更新owner
                    //如果有cref这些，
                    //存储删除的语句
                } else {
                    //todo 添加overwrite模式，覆盖之前的数据
                    //提供一个channel传入，在指定的地方执行
                    //需要添加索引
                    let relate_sqls = ele
                        .children
                        .iter()
                        .enumerate()
                        .map(|(i, child)| {
                            format!(
                                "RELATE pe:{}->pe_owner->pe:{} set order_num = {}",
                                child.to_string(),
                                refno.to_string(),
                                i
                            )
                        })
                        .collect::<Vec<String>>();
                    all_relate_sqls.extend_from_slice(&relate_sqls);
                }

                let increment_info = IncrementInfo {
                    refno,
                    db_no: basic_info.pdms_header.db_num,
                    attr: attmap,
                    children: ele.children.clone(),
                    operation: ele_op,
                };
                if ele_op == EleOperation::Modified {
                    modify_type_eles_map
                        .entry(ele.noun)
                        .or_insert(Vec::new())
                        .push(increment_info);
                } else if ele_op == EleOperation::Add {
                    added_type_eles_map
                        .entry(ele.noun)
                        .or_insert(Vec::new())
                        .push(increment_info);
                }

                match ele_op {
                    EleOperation::None => {}
                    EleOperation::Add => {
                        dbg!(ele.refno);
                        total_add_len += 1;
                    }
                    EleOperation::Modified => {
                        total_modify_len += 1;
                    }
                    EleOperation::Deleted => {
                        dbg!(ele.refno);
                        total_deleted_len += 1;
                    }
                }
            }
        }

        //可以采用channel模式，发送更新的数据，然后更新数据
        //保存新增数据
        let mut join_set = tokio::task::JoinSet::new();
        let mut save_atts_time = Instant::now();
        for (&noun, v) in added_type_eles_map.iter() {
            let type_name = db1_dehash(noun as _);
            if type_name.is_empty() {
                continue;
            }
            let type_name = type_name.as_str();
            let mut pe_json_vec = vec![];
            let mut att_json_vec = vec![];
            for k in v {
                let refno = k.refno;
                let pe = SPdmsElement {
                    id: refno.to_string(),
                    refno,
                    owner: k.attr.get_owner(),
                    name: k.attr.get_name_or_default(),
                    noun: k.attr.get_type(),
                    dbnum: k.db_no,
                    e3d_version: k.attr.get_e3d_version(),
                    version_tag: None,
                    status_tag: None,
                    cata_hash: k.attr.cal_cata_hash().map(|x| x.to_string()),
                    lock: false,
                    deleted: false,
                };

                pe_json_vec.push(pe.gen_sur_json());
                if let Some(json) = k.attr.gen_sur_json() {
                    att_json_vec.push(json);
                }
            }
            //对新增的pe处理
            let pe_sql = format!("INSERT IGNORE INTO pe [{}]", pe_json_vec.join(","));
            //使用surreal 保存NamedAttrMap
            join_set.spawn(async move {
                SUL_DB.query(pe_sql).await.unwrap();
            });
            //对新增的属性处理
            let attmap_sql = format!(
                "INSERT IGNORE INTO {} [{}]",
                &type_name,
                att_json_vec.join(",")
            );
            // println!("attmap sql: {}", &attmap_sql);
            //使用surreal 保存NamedAttrMap
            join_set.spawn(async move {
                SUL_DB.query(attmap_sql).await.unwrap();
            });

            //还需要创建relate关系
        }
        //等待保存任务完成
        while let Some(_) = join_set.join_next().await {}
        println!(
            "保存新增属性数据完成，耗时: {} s",
            save_atts_time.elapsed().as_secs_f32()
        );

        let mut relate_join_set = tokio::task::JoinSet::new();
        let mut time = Instant::now();
        dbg!(all_relate_sqls.len());
        let mut chunks = all_relate_sqls.chunks(100);
        for mut s in chunks {
            let sql = s.into_iter().join(";");
            relate_join_set.spawn(async move {
                SUL_DB.query(sql).await.unwrap();
            });
        }
        while let Some(_) = relate_join_set.join_next().await {}
        println!("Relate pes task costs {} s", time.elapsed().as_secs_f32());

        //todo 批量查询types
        for (k, v) in owner_children_map {
            if let Ok(type_name) = surreal_service::get_type_name(k).await {
                if type_name == "BRAN" || type_name == "HANG" {
                    geo_update_log.bran_hanger_refnos.insert(k);
                }
            }
        }

        // let mut updated_sets = HashSet::new();
        // for (mut noun, mut incrs) in modify_type_eles_map {
        //     while let Some(mut incr) = incrs.pop() {
        //         let refno = incr.refno;
        //         updated_sets.insert(incr.refno);
        //         let owner = incr.attr.get_owner();
        //         if owner.is_unset() {
        //             continue;
        //         }
        //
        //         let type_name = incr.attr.get_type_str();
        //         if PRIMITIVE_NOUN_NAMES.contains(&type_name) {
        //             geo_update_log.prim_refnos.push(refno);
        //         } else if GNERAL_LOOP_NOUN_NAMES.contains(&type_name) {
        //             geo_update_log.loop_refnos.push(refno);
        //         } else if CATA_HAS_TUBI_GEO_NAMES.contains(&type_name) {
        //             geo_update_log.basic_cata_refnos.push(refno);
        //         } else if CATA_GEO_NAMES.contains(&type_name) {
        //             geo_update_log.basic_cata_refnos.push(refno);
        //         }
        //
        //         let pdms_element = SPdmsElement {
        //             id: refno.to_string(),
        //             refno,
        //             owner,
        //             name: incr.attr.get_string_or_default("NAME"),
        //             noun: db1_dehash(noun),
        //             dbnum: incr.db_no,
        //             e3d_version: incr.attr.get_e3d_version(),
        //             version_tag: None,
        //             status_tag: None,
        //             cata_hash: None,
        //             lock: false,
        //             deleted: false,
        //         };
        //         pdms_elements.push(pdms_element);
        //     }
        // }

        // let mut time = Instant::now();
        // let mut join_set = tokio::task::JoinSet::new();
        // //更新和插入的处理
        // for eles in pdms_elements.chunks(500) {
        //     let mut json_strs = Vec::new();
        //     for m in eles {
        //         json_strs.push(m.gen_sur_json());
        //     }
        //     let sql = format!("INSERT IGNORE INTO pe [{}]", json_strs.join(","));
        //     //手动修改，替换掉""
        //     join_set.spawn(async move {
        //         SUL_DB
        //             .query(sql)
        //             .await
        //             .unwrap();
        //     });
        // }
        // while let Some(_) = join_set.join_next().await {}
        // println!("Save incr pes task costs {} s", time.elapsed().as_secs_f32());

        // dbg!(geo_update_log.bran_hanger_refnos.len());
        dbg!(&geo_update_log);
        let r: Vec<IncrGeoUpdateLog> = SUL_DB
            .create("incr_model_log")
            .content(&geo_update_log)
            .await?;

        gen_all_geos_data(Arc::new(self.clone()), Some(geo_update_log))
            .await
            .unwrap();

        println!("增加:{total_add_len}，修改:{total_modify_len}，删除:{total_deleted_len}");

        Ok(true)
    }

    pub async fn init_watcher(&self) -> anyhow::Result<()> {
        let mut params = IndexMap::new();
        let mut latest_need_update_headers = IndexMap::new();
        fs::create_dir_all("asset/archives")?;
        let mut time = Instant::now();
        for watch_dir in &self.watcher.watch_dirs {
            let mut join_set = JoinSet::new();
            for entry in WalkDir::new(watch_dir).sort_by(|a, b| {
                b.path()
                    .metadata()
                    .unwrap()
                    .len()
                    .cmp(&a.path().metadata().unwrap().len())
            }) {
                let dir_entry = entry.unwrap();
                let path = dir_entry.path();
                //只处理0001 结尾的文件
                let file_name = path.file_stem().unwrap().to_str().unwrap();
                if path.is_dir() || !file_name.ends_with("0001"){
                    continue;
                }
                self.watcher.file_name_full_path_map.insert(file_name.to_owned(), path.to_path_buf());
                //初始化CBA的Archive文件，来保证后续增量下载
                let input= path.to_path_buf();
                let output: PathBuf = format!("asset/archives/{}.cba", file_name).into();
                join_set.spawn(async move {
                    // let compress_opt = CompressOptions::new(input, output);
                    // execute_compress(compress_opt).await.unwrap();
                });

                let mut io = PdmsIO::new(path, true);
                io.open()?;
                if let Ok(basic_info) = io.get_page_basic_info() {
                    if let Some(mut old) = self.watcher.headers.get_mut(&path.to_path_buf()) {
                        //未发生修改，直接跳过
                        if old.pdms_header.page_no == basic_info.pdms_header.page_no {
                            continue;
                        }
                        params.insert(
                            path.to_path_buf(),
                            (basic_info.clone(), old.pdms_header.page_no),
                        );
                        //在old里有出现，但是版本号不一致，需要更新
                        latest_need_update_headers.insert(path.to_path_buf(), basic_info);
                    } else {
                        //在old里面没有出现，需要更新进来
                        self.watcher.headers.insert(path.to_path_buf(), basic_info);
                    }
                }
                // break;
            }
            while let Some(_) = join_set.join_next().await {}
            // break;
        }
        println!("初始化增量更新耗时: {} s", time.elapsed().as_secs_f32());
        match self.execute_incr_update(params).await {
            Ok(_) => {
                //执行没问题了，再更新当前的版本记录，headers直接存本地json
                for (path, new_header) in latest_need_update_headers {
                    if let Some(mut old) = self.watcher.headers.get_mut(&path) {
                        //未发生修改，直接跳过
                        if old.pdms_header.page_no == new_header.pdms_header.page_no {
                            continue;
                        }
                        *old.value_mut() = new_header;
                    }
                }
                //now save the watch.json
                self.watcher.save(None)?;
                println!("执行启动后的自动增量完成。")
            }
            Err(e) => {
                println!("Execute increment update error: {:?}", e);
            }
        }

        anyhow::Ok(())
    }

    //开始监测数据文件夹
    pub async fn async_watch(&self) -> notify::Result<()> {
        let (mut watcher, mut rx) = PdmsWatcher::async_watcher()?;
        self.watcher.watch_dirs.iter().for_each(|x| {
            watcher
                .watch(x.as_path(), RecursiveMode::NonRecursive)
                .expect("watch files failed");
        });

        while let Some(res) = rx.next().await {
            match res {
                Ok(event) => {
                    //跳过只是meta data变动的情况
                    if matches!(event.kind, notify::EventKind::Modify(notify::event::ModifyKind::Metadata(_))) {
                        continue;
                    }
                    // println!("changed: {:?}", &event);
                    if let Ok(new_headers) = PdmsWatcher::scan_db_headers(event.paths) {
                        // dbg!(&new_headers);
                        let mut params = IndexMap::new();
                        for (path, new_header) in &new_headers {
                            if let Some(mut old) = self.watcher.headers.get_mut(path) {
                                //未发生修改，直接跳过
                                if old.pdms_header.page_no == new_header.pdms_header.page_no {
                                    continue;
                                }
                                params.insert(
                                    path.clone(),
                                    (new_header.clone(), old.pdms_header.page_no),
                                );
                            }
                        }
                        // dbg!(&params);
                        let mut notify_file_names = vec![];
                        let mut notify_file_hashes = vec![];

                        match self.execute_incr_update(params).await {
                            Ok(_) => {
                                //执行没问题了，再更新当前的版本记录，headers直接存本地json
                                for (path, new_header) in new_headers {
                                    let file_name = path.file_stem().unwrap().to_str().unwrap();
                                    // dbg!(&file_name);
                                    if path.is_dir() || !file_name.ends_with("0001"){
                                        continue;
                                    }
                                    if let Some(mut old) = self.watcher.headers.get_mut(&path) {
                                        //未发生修改，直接跳过
                                        if old.pdms_header.page_no >= new_header.pdms_header.page_no {
                                            continue;
                                        }
                                        *old.value_mut() = new_header;

                                        //发生修改的文件，重新生成archive
                                        // dbg!(&path);
                                        let output: PathBuf = format!("asset/archives/{}.cba", file_name).into();
                                        // dbg!(&output);
                                        let compress_opt = CompressOptions::new(path.clone(), output);
                                        let file_hash = execute_compress(compress_opt).await.unwrap().to_string();

                                        //数据库里不存在这个file hash的记录，才需要
                                        let mut response  = SUL_DB
                                            // .query("select value id from (select * from e3d_sync where location != $loc and $name in file_names order by timestamp desc limit 1) where $hash in file_hashes")
                                            .query("select value id from (select * from e3d_sync where location != $loc and $name in file_names order by timestamp desc) where $hash in file_hashes")
                                            .bind(("loc", get_db_option().location.as_str()))
                                            .bind(("hash", &file_hash))
                                            .bind(("name", file_name))
                                            .await.unwrap();
                                        let id = response.take::<Option<String>>("id").unwrap();
                                        dbg!(&id);
                                        if id.is_none() {
                                            notify_file_hashes.push(file_hash);
                                            notify_file_names.push(file_name.to_owned());
                                        }
                                    }
                                }
                                //now save the watch.json
                                self.watcher.save(None);
                            }
                            Err(e) => {
                                println!("Execute increment update error: {:?}", e);
                            }
                        }
                        //publish notify db file updates
                        dbg!(&notify_file_names);
                        let payload = SyncE3dFileMsg::new(notify_file_names, notify_file_hashes);
                        //自己本地也要保存, todo 后续还是要配置哪些dbs，哪个地方能修改，哪个地方是不能改的
                        SUL_DB.query(format!("INSERT INTO e3d_sync {} "
                                             , serde_json::to_string(&payload).unwrap())).await.unwrap();
                        //todo 检查是否只是发生了claim page的变化，如果只是claim修改，是需要每次都同步？
                        //会导致出现循环
                        self.mqtt_client.clone().publish("Sync/E3d",
                                                 QoS::ExactlyOnce, true, payload).await.unwrap();

                    }
                }
                Err(e) => println!("watch error: {:?}", e),
            }
        }

        Ok(())
    }
}
