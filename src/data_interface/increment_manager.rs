use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use aios_core::pdms_types::*;
use aios_core::pe::SPdmsElement;
use aios_core::tool::db_tool::db1_dehash;
use aios_core::{clear_all_caches, SUL_DB};
use aios_core::{get_db_option, RefU64Vec};
use aios_core::version::backup_att_and_pe_to_history_tables;
use futures::StreamExt;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use notify::{RecursiveMode, Watcher};
use parse_pdms_db::parse::parse_db_basic_info;
use pdms_io::defines::DbPageBasicInfo;
use pdms_io::io::PdmsIO;
use pdms_io::sync::compress::{execute_compress, CompressOptions};
use pdms_io::watch::PdmsWatcher;
use petgraph::visit::Walker;
use rumqttc::QoS;
use surrealdb::sql::Thing;
use tokio::fs::create_dir_all;
use tokio::task::JoinSet;
use walkdir::WalkDir;

use crate::data_interface::increment_record::IncrGeoUpdateLog;
use crate::fast_model::{gen_all_geos_data, process_meshes_update_db};
// use pdms_io::watch::PdmsWatcher;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::mqtt_service::SyncE3dFileMsg;

#[derive(Debug, Default, Clone)]
pub struct IncrementInfo {
    pub refno: RefU64,
    pub db_no: i32,
    pub attr: NamedAttrMap,
    pub children: RefU64Vec,
    pub operation: EleOperation,
}

impl IncrementInfo {
    #[inline]
    pub fn is_modified(&self) -> bool {
        matches!(self.operation, EleOperation::Modified)
    }

    #[inline]
    pub fn is_deleted(&self) -> bool {
        matches!(self.operation, EleOperation::Deleted)
    }

    #[inline]
    pub fn is_added(&self) -> bool {
        matches!(self.operation, EleOperation::Add)
    }
}

const JSON_CHUNK_COUNT: usize = 200;


pub const CHECK_DB_TYPES: [&'static str; 6] = ["CATA", "DESI", "DICT", "SYST", "GLB", "GLOB"];

impl AiosDBManager {
    ///执行增量更新
    pub async fn execute_incr_update(
        &self,
        increment_ranges_map: IndexMap<PathBuf, (DbPageBasicInfo, u32)>,
    ) -> anyhow::Result<bool> {
        //没有增量更新的数据，直接返回
        if increment_ranges_map.is_empty() {
            return Ok(false);
        }
        let mut update_type_eles_map = HashMap::new();
        let mut deleted_refnos_set = HashSet::new();

        let mut total_add_len = 0;
        let mut total_modify_len = 0;
        let mut total_deleted_len = 0;
        let mut geo_update_log = IncrGeoUpdateLog::default();
        let mut owner_children_map = IndexMap::new();
        let mut all_relate_sqls = vec![];
        let mut has_changed = false;
        //TODO: 如何鉴别只有claim page的变化，没有数据的更新，就不需要执行增量更新
        for (path, (basic_info, last_pageno)) in increment_ranges_map {
            let mut io = PdmsIO::new(path, true);
            io.open()?;
            let eles_map = io
                .collect_increment_eles(&basic_info, last_pageno)
                .await?;
            if eles_map.is_empty(){
                continue;
            }
            dbg!((last_pageno, eles_map.len()));
            if eles_map.is_empty() {
                continue;
            }
            //批量检测是否存在这些eles
            let mut exist_refnos = HashSet::new();
            let pes = eles_map.keys().map(|x| x.to_pe_key()).join(",");
            let mut resp = SUL_DB
                .query(format!("SELECT VALUE id FROM [{pes}];"))
                .await?;
            // dbg!(&resp);
            let refnos: Vec<RefU64> = resp.take(0).unwrap();
            exist_refnos.extend(refnos);
            for (&refno, _) in &eles_map {
                clear_all_caches(refno).await;
            }

            let mut need_delete_owner_set = HashSet::new();
            for (&refno, ele) in &eles_map {
                let mut attmap: NamedAttrMap = ele.whole_attmap.merge().into();
                attmap.set_e3d_version(ele.version as _);
                let owner = attmap.get_owner();
                let type_name = attmap.get_type();
                let type_name = type_name.as_str();
                has_changed = true;
                // dbg!(ele.refno);
                // dbg!(&attmap);
                let mut ele_op = EleOperation::Modified;
                let mut need_update_all_relate_after_delete = false;
                let mut need_update_all_relate_after_add = false;
                if exist_refnos.contains(&ele.refno) {
                    let old_children = aios_core::get_children_refnos(ele.refno).await?;
                    for (i, child) in old_children.into_iter().enumerate() {
                        if !ele.children.contains(&child) {
                            //index current delete refno, owner refno
                            let t = (i, child, refno);
                            println!("Delete: {:?}", t);
                            deleted_refnos_set.insert(t);
                            total_deleted_len += 1;
                            need_update_all_relate_after_delete = true;
                        }
                    }

                    if ele_op == EleOperation::Modified {
                        total_modify_len += 1;
                    }
                } else {
                    total_add_len += 1;
                    ele_op = EleOperation::Add;
                    let mut index = 0;
                    let mut is_last_add = false;
                    #[cfg(feature = "debug_parse")]
                    dbg!(ele.refno);
                    if let Some(owner_ele) = eles_map.get(&owner) {
                        index = owner_ele
                            .children
                            .iter()
                            .position(|&x| x == refno)
                            .unwrap_or(0);
                        // dbg!(owner_ele);
                        is_last_add = index == owner_ele.children.len() - 1;

                        let cp = refno.to_pe_key();
                        let op = owner.to_pe_key();
                        //如果是最后一个，啥子都不用管，直接插入到最后
                        if is_last_add {
                            all_relate_sqls
                                .push(
                                    format!("RELATE {0}->pe_owner:[{1}, {index}]->{1};", cp, op,),
                                );
                        } else {
                            need_update_all_relate_after_add = true;
                        }
                    }
                }

                if need_update_all_relate_after_delete ||
                    need_update_all_relate_after_add {
                    //优先使用更新的，因为可能是父节点发生修改，直接全刷
                    let (owner_ele, o_refno) = if need_update_all_relate_after_add {
                        (eles_map.get(&owner).unwrap(), owner)
                    } else {
                        (ele, ele.refno)
                    };

                    //不需要重复插入和执行，特别是多个的时候
                    if !need_delete_owner_set.contains(&o_refno) {
                        all_relate_sqls.push(format!(
                            "delete pe_owner:[{0}, 0]..[{0}, {1}];",
                            o_refno.to_pe_key(),
                            owner_ele.children.len()
                        ));


                        let relate_sqls = owner_ele
                            .children
                            .iter()
                            .enumerate()
                            .map(|(i, child)| {
                                let cp = child.to_pe_key();
                                format!("RELATE {0}->pe_owner:[{1}, {i}]->{1};", cp, o_refno.to_pe_key())
                            })
                            .collect::<Vec<String>>();
                        all_relate_sqls.extend_from_slice(&relate_sqls);

                        need_delete_owner_set.insert(o_refno);
                    }
                }

                #[cfg(feature = "debug_parse")]
                dbg!((refno, ele_op));

                if PRIMITIVE_NOUN_NAMES.contains(&type_name) {
                    geo_update_log.prim_refnos.insert(refno);
                } else if GNERAL_LOOP_OWNER_NOUN_NAMES.contains(&type_name) {
                    //TODO 如果修改的是顶点， 这里要考虑到最终的构件，也要添加进来，比如 vert -> GWALL/FLOOR 等
                    geo_update_log.loop_owner_refnos.insert(refno);
                    geo_update_log.loop_owner_refnos.insert(owner);
                } else if CATA_HAS_TUBI_GEO_NAMES.contains(&type_name) {
                    geo_update_log.bran_hanger_refnos.insert(refno);
                } else if CATA_GEO_NAMES.contains(&type_name) {
                    geo_update_log.basic_cata_refnos.insert(refno);
                    owner_children_map
                        .entry(attmap.get_owner())
                        .or_insert_with(HashSet::new)
                        .insert(refno);
                } else {
                    let owner_type = aios_core::get_type_name(owner).await?;
                    if CATA_HAS_TUBI_GEO_NAMES.contains(&owner_type.as_str()) {
                        geo_update_log.basic_cata_refnos.insert(refno);
                    }
                }
                let increment_info = IncrementInfo {
                    refno,
                    db_no: basic_info.pdms_header.db_num,
                    attr: attmap,
                    children: ele.children.clone(),
                    operation: ele_op,
                };
                if ele_op == EleOperation::Modified || ele_op == EleOperation::Add {
                    update_type_eles_map
                        .entry(ele.noun)
                        .or_insert(Vec::new())
                        .push(increment_info);
                };
            }
        }
        //如果没有发生变化，直接返回
        if !has_changed {
            return Ok(false);
        }

        //relate 优先处理
        let mut relate_join_set = tokio::task::JoinSet::new();
        let mut time = Instant::now();
        // dbg!(all_relate_sqls.len());
        let mut chunks = all_relate_sqls.chunks(100);
        for mut s in chunks {
            let sql = s.into_iter().join("");
            // dbg!(&sql);
            #[cfg(feature = "debug_parse")]
            println!("relates sql: {}", &sql);
            relate_join_set.spawn(async move {
                SUL_DB.query(sql).await.unwrap();
            });
        }
        while let Some(_) = relate_join_set.join_next().await {}
        println!("Relate pes task costs {} s", time.elapsed().as_secs_f32());

        //可以采用channel模式，发送更新的数据，然后更新数据
        //保存新增数据
        // let mut sql_join_set = tokio::task::JoinSet::new();
        let mut att_pe_handles = vec![];
        //新增模型的处理
        for (noun, v) in update_type_eles_map {
            let type_name = db1_dehash(noun as _);
            if type_name.is_empty() {
                continue;
            }
            let type_name = type_name.as_str();

            for chunk in v.chunks(JSON_CHUNK_COUNT) {
                let mut insert_pe_jsons_str = String::new();
                let mut update_pe_sql_str = String::new();
                let mut update_att_sql_str = String::new();
                let mut history_refnos_set = Vec::new();
                for k in chunk {
                    let refno = k.refno;
                    history_refnos_set.push(refno);
                    let name = k.attr.get_name();
                    let pe = SPdmsElement {
                        refno,
                        owner: k.attr.get_owner(),
                        name: name.unwrap_or_default(),
                        noun: k.attr.get_type(),
                        dbnum: k.db_no,
                        e3d_version: k.attr.get_e3d_version(),
                        cata_hash: k.attr.cal_cata_hash(),
                        ..Default::default()
                    };

                    let json = pe.gen_sur_json();
                    let att_json = k.attr.gen_sur_json_exclude(&["id"]);
                    if k.is_modified() {
                        update_pe_sql_str.push_str(
                            format!("UPDATE {} CONTENT {};", refno.to_pe_key(), json).as_str(),
                        );
                    } else {
                        insert_pe_jsons_str.push_str(&json);
                        insert_pe_jsons_str.push_str(",");
                    }

                    //不管怎样，update和add，都用update的方式
                    if let Some(att_json) = att_json {
                        update_att_sql_str.push_str(
                            format!(
                                "UPDATE {} CONTENT {};",
                                refno.to_table_key(&type_name),
                                att_json
                            )
                            .as_str(),
                        );
                    }
                }

                backup_att_and_pe_to_history_tables(&history_refnos_set).await.unwrap();

                //调用函数，将当前数据存储到版本表里
                // println!("{}", &update_pe_sql_str);
                let insert_pe_sql = if !insert_pe_jsons_str.is_empty() {
                    insert_pe_jsons_str.pop();
                    format!(
                        "INSERT IGNORE INTO pe [{}];",
                        insert_pe_jsons_str
                    )
                } else { "".to_owned() };


                let handle = tokio::task::spawn(async move {
                    if !update_att_sql_str.is_empty() {
                        // println!("update_att_sql_str: {}", &update_att_sql_str);
                        SUL_DB.query(update_att_sql_str).await.unwrap();
                    }
                    if !update_pe_sql_str.is_empty() {
                        // println!("update_pe_sql: {}", &update_pe_sql_str);
                        SUL_DB.query(update_pe_sql_str).await.unwrap();
                    }

                    //使用surreal 保存pe
                    if !insert_pe_sql.is_empty() {
                        // println!("insert_pe_sql: {}", &insert_pe_sql);
                        let response = SUL_DB.query(insert_pe_sql).await.unwrap();
                        // dbg!(&response);
                    }

                });

                att_pe_handles.push(handle);
            }
        }
        futures::future::join_all(att_pe_handles).await;
        //等待保存任务完成
        // while let Some(_) = sql_join_set.join_next().await {}

        //删除模型的处理
        let deleted_refnos: Vec<(usize, RefU64, RefU64)> = deleted_refnos_set.into_iter().collect::<Vec<_>>();
        for chunk in deleted_refnos.chunks(JSON_CHUNK_COUNT) {
            let del_sql = chunk
                .into_iter()
                .map(|(i, refno, owner)| {
                    format!(
                        "update {} set deleted=true;",
                        refno.to_pe_key()
                    )
                })
                .join("");
            // dbg!(&del_sql);
            SUL_DB.query(&del_sql).await.unwrap();
        }
        //todo 批量查询types
        for (k, v) in owner_children_map {
            if let Ok(type_name) = aios_core::get_type_name(k).await {
                if type_name == "BRAN" || type_name == "HANG" {
                    geo_update_log.bran_hanger_refnos.insert(k);
                }
            }
        }
        let r: Vec<IncrGeoUpdateLog> = SUL_DB
            .create("incr_model_log")
            .content(&geo_update_log)
            .await?;

        let all_refnos = geo_update_log.get_all_visible_refnos().into_iter().collect::<Vec<_>>();
        gen_all_geos_data(&self.db_option, Some(geo_update_log))
            .await
            .unwrap();

        dbg!(&all_refnos);
        //todo 把历史的数据 inst_relate 里的in 改成使用pe_history:[refno, version]
        process_meshes_update_db(None, &all_refnos)
            .await
            .unwrap();

        println!("增加:{total_add_len}，修改:{total_modify_len}，删除:{total_deleted_len}");

        Ok(true)
    }

    //直接通过数据库的查询，获得当前最新的version，不需要使用json的方式
    // pub fn scan_incr_updates(path_buf: PathBuf) -> anyhow::Result<IndexMap<PathBuf, (DbPageBasicInfo, u32)>> {
    //
    // }

    //初始化监测
    pub async fn init_watcher(&self) -> anyhow::Result<()> {
        let mut params = IndexMap::new();
        let mut latest_need_update_headers = IndexMap::new();
        fs::create_dir_all("asset/archives")?;
        let mut time = Instant::now();
        dbg!(&self.watcher.watch_dirs);
        for watch_dir in &self.watcher.watch_dirs {
            // let mut join_set = JoinSet::new();
            for entry in WalkDir::new(watch_dir).sort_by(|a, b| {
                b.path()
                    .metadata()
                    .unwrap()
                    .len()
                    .cmp(&a.path().metadata().unwrap().len())
            }) {
                let dir_entry = entry.unwrap();
                let path = dir_entry.path();
                let file_name = path.file_stem().unwrap().to_str().unwrap();
                if path.is_dir() {
                    continue;
                }

                let (db_type, file_version, db_num) = parse_db_basic_info(path.to_path_buf());
                // if db_num != 1112 {
                //     continue;
                // }
                let file_latest_max_pgno = PdmsIO::new(path.to_path_buf(), true)
                    .get_att_latest_pgno()
                    .unwrap_or_default();

                if !CHECK_DB_TYPES.contains(&db_type.as_str()) {
                    continue;
                }
                let Ok(max_pgno) = aios_core::query_db_max_version(db_num).await else{
                    //先暂时跳过数据库里没有的文件，todo 考虑自动追加文件全新解析
                    continue;
                };

                if file_latest_max_pgno <= max_pgno {
                    continue;
                }
                println!("发现需要增量更新的文件: {:?}, 当前数据库属性最大pgno: {max_pgno}, 文件属性对应pgno: {file_latest_max_pgno}", &file_name);
                //暂时先跳过更新比较大的
                if file_latest_max_pgno - max_pgno > 0x8000 {
                    continue;
                }

                // self.watcher
                //     .file_name_full_path_map
                //     .insert(file_name.to_owned(), path.to_path_buf());
                //初始化CBA的Archive文件，来保证后续增量下载, 后面是否需要加一个环境变量，来控制是否需要重新生成archive文件
                //是否需要完全初始化
                // let input = path.to_path_buf();
                // let output: PathBuf = format!("asset/archives/{}.cba", file_name).into();
                // join_set.spawn(async move {
                //     // let compress_opt = CompressOptions::new(input, output);
                //     // execute_compress(compress_opt).await.unwrap();
                // });

                let mut io = PdmsIO::new(path, true);
                io.open().unwrap();
                //每个path 都要检查一遍
                if let Ok(basic_info) = io.get_page_basic_info() {
                    if file_latest_max_pgno != 0 {
                        #[cfg(feature = "debug_parse")]
                        dbg!((db_num, file_latest_max_pgno));
                        params.insert(
                            path.to_path_buf(),
                            (basic_info.clone(), file_latest_max_pgno),
                        );
                        latest_need_update_headers.insert(path.to_path_buf(), basic_info.clone());
                        self.watcher.headers.insert(path.to_path_buf(), basic_info);
                    }
                }
            }
        }

        //等所有的文件都检查同步完毕，才执行更新
        match self.execute_incr_update(params).await {
            Ok(true) => {
                println!("执行启动后的自动增量完成。")
            }
            Ok(false) => {
                println!("没有发生增量更新。")
            }
            Err(e) => {
                println!("Execute increment update error: {:?}", e);
            }
        }

        println!("初始化增量更新耗时: {} s", time.elapsed().as_secs_f32());

        anyhow::Ok(())
    }

    //开始监测数据文件夹
    pub async fn async_watch(&self) -> notify::Result<()> {
        let (mut watcher, mut rx) = PdmsWatcher::async_watcher()?;
        dbg!(&self.watcher.watch_dirs);
        self.watcher.watch_dirs.iter().for_each(|x| {
            watcher
                .watch(x.as_path(), RecursiveMode::NonRecursive)
                .expect("watch files failed");
        });

        create_dir_all("assets/archives").await.unwrap();
        create_dir_all("assets/temp").await.unwrap();
        while let Some(res) = rx.next().await {
            match res {
                Ok(event) => {
                    // dbg!(&event);
                    //跳过只是meta data变动的情况
                    let data_changed = matches!(
                        event.kind,
                        notify::EventKind::Modify(notify::event::ModifyKind::Data(_))
                            | notify::EventKind::Modify(notify::event::ModifyKind::Any)
                            | notify::EventKind::Create(notify::event::CreateKind::File)
                            | notify::EventKind::Remove(notify::event::RemoveKind::File)
                    );
                    if !data_changed {
                        continue;
                    }
                    //后面用派发任务的方式,不要放在这里阻塞
                    println!("changed: {:?}", &event);
                    dbg!(&self.watcher.headers);
                    if let Ok(new_headers) = PdmsWatcher::scan_db_headers(&event.paths) {
                        let mut params = IndexMap::new();
                        for (path, new_header) in &new_headers {
                            // dbg!(new_header.pdms_header.page_no);
                            // dbg!(path);
                            if let Some(mut old) = self.watcher.headers.get_mut(path) {
                                // dbg!(path);
                                // dbg!(old.pdms_header.page_no);
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
                        if params.is_empty() {
                            continue;
                        }
                        let mut notify_file_names = vec![];
                        let mut notify_file_hashes = vec![];

                        //如果数据没有发生变化，则不需要推出变化，不需要执行增量
                        match self.execute_incr_update(params).await {
                            Ok(true) => {
                                //执行没问题了，再更新当前的版本记录，headers直接存本地json
                                for (path, new_header) in new_headers {
                                    let file_name = path.file_stem().unwrap().to_str().unwrap();
                                    if path.is_dir() {
                                        continue;
                                    }
                                    dbg!(&file_name);
                                    //这个地方是不是需要直接去读取文件，然后更新headers，不能太依赖json数据
                                    //或者每次启动都重新更新这个文件？
                                    if let Some(mut old) = self.watcher.headers.get_mut(&path) {
                                        dbg!((
                                            old.pdms_header.page_no,
                                            new_header.pdms_header.page_no
                                        ));
                                        //未发生修改，直接跳过
                                        if old.pdms_header.page_no >= new_header.pdms_header.page_no
                                        {
                                            continue;
                                        }
                                        *old.value_mut() = new_header;

                                        //发生修改的文件，重新生成archive
                                        // dbg!(&path);
                                        let output: PathBuf =
                                            format!("asset/archives/{}.cba", file_name).into();
                                        // dbg!(&output);
                                        let compress_opt = CompressOptions::new(
                                            path.clone(),
                                            output,
                                            "asset/temp",
                                        );
                                        let file_hash = execute_compress(compress_opt)
                                            .await
                                            .unwrap()
                                            .to_string();
                                        // dbg!(&file_hash);

                                        //数据库里不存在这个file hash的记录，才需要发送
                                        //是自己创建的，在记录里还没有的，才能发送消息出去
                                        //如果是别的创建的，就应该调过
                                        let sql = format!(
                                            "select value id from (select * from e3d_sync where location != '{}' and '{}' in file_names and '{}' in file_hashes order by timestamp desc) ",
                                            get_db_option().location.as_str(),
                                            file_name,
                                            &file_hash
                                        );
                                        // dbg!(&sql);
                                        let mut response = SUL_DB.query(&sql).await.unwrap();
                                        // dbg!(&response);
                                        let id = response.take::<Vec<String>>(0).unwrap();
                                        // dbg!(id.len());
                                        if id.is_empty() {
                                            println!("发生了增量更新，推送：{}", &file_name);
                                            notify_file_hashes.push(file_hash);
                                            notify_file_names.push(file_name.to_owned());
                                        }
                                    }
                                }
                                //now save the watch.json
                                // self.watcher.save(None).expect("save watch.json failed");
                            }
                            Ok(false) => {
                                println!("{:?} 文件发生修改，但是没有发生增量更新。", &event.paths);
                                continue;
                            }
                            Err(e) => {
                                println!("Execute increment update error: {:?}", e);
                            }
                        }
                        //publish notify db file updates
                        dbg!(&notify_file_names);
                        let payload = SyncE3dFileMsg::new(notify_file_names, notify_file_hashes);
                        //自己本地也要保存, todo 后续还是要配置哪些dbs，哪个地方能修改，哪个地方是不能改的
                        SUL_DB
                            .query(format!(
                                "INSERT INTO e3d_sync {} ",
                                serde_json::to_string(&payload).unwrap()
                            ))
                            .await
                            .unwrap();
                        //todo 检查是否只是发生了claim page的变化，如果只是claim修改，是需要每次都同步？
                        //会导致出现循环
                        // self.mqtt_client
                        //     .clone()
                        //     .publish("Sync/E3d", QoS::ExactlyOnce, true, payload)
                        //     .await
                        //     .unwrap();
                    }
                }
                Err(e) => println!("watch error: {:?}", e),
            }
        }

        Ok(())
    }
}
