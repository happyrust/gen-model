use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use aios_core::pdms_types::*;
use aios_core::pe::SPdmsElement;
use aios_core::tool::db_tool::db1_dehash;
use aios_core::version::{backup_data, backup_owner_relate};
use aios_core::{RefU64Vec, get_db_option};
use aios_core::{SUL_DB, clear_all_caches};
use futures::StreamExt;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use notify::{RecursiveMode, Watcher};
use parse_pdms_db::parse::parse_db_basic_info;
use pdms_io::defines::DbPageBasicInfo;
use pdms_io::io::{EleOperationData, EleOperationDetail, PdmsIO};
use pdms_io::sync::compress::{CompressOptions, execute_compress};
// use pdms_io::sync::compress::{execute_compress, CompressOptions};
use pdms_io::watch::PdmsWatcher;
use petgraph::visit::Walker;
use rumqttc::QoS;
use serde::{Deserialize, Serialize};
use tokio::fs::{self, create_dir_all};
use walkdir::WalkDir;

use crate::data_interface::increment_record::IncrGeoUpdateLog;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::*;
use crate::mqtt_service::SyncE3dFileMsg;
#[cfg(feature = "web_server")]
use crate::web_server::{
    remote_runtime::REMOTE_RUNTIME,
    remote_sync_handlers,
    sync_control_center::{NewSyncTaskParams, SYNC_CONTROL_CENTER},
};
use parse_pdms_db::parse::DbBasicInfo;

/// 增量更新信息结构体
///
/// 用于存储和跟踪数据库中元素的增量变化信息
#[derive(Debug, Default, Clone)]
pub struct IncrementInfo {
    /// 元素的引用编号
    pub refno: RefU64,
    /// 数据库编号
    pub db_no: i32,
    /// 元素的属性映射
    pub attr: NamedAttrMap,
    /// 子元素的引用编号列表
    pub children: RefU64Vec,
    /// 元素的操作类型(增加/修改/删除)
    pub operation: EleOperation,
}

impl IncrementInfo {
    /// 检查元素是否被修改
    ///
    /// # 返回值
    ///
    /// * `bool` - 如果元素被修改返回true，否则返回false
    #[inline]
    pub fn is_modified(&self) -> bool {
        matches!(self.operation, EleOperation::Modified)
    }

    /// 检查元素是否被删除
    ///
    /// # 返回值
    ///
    /// * `bool` - 如果元素被删除返回true，否则返回false
    #[inline]
    pub fn is_deleted(&self) -> bool {
        matches!(self.operation, EleOperation::Deleted)
    }

    /// 检查元素是否为新增
    ///
    /// # 返回值
    ///
    /// * `bool` - 如果元素是新增的返回true，否则返回false
    #[inline]
    pub fn is_added(&self) -> bool {
        matches!(self.operation, EleOperation::Add)
    }
}

#[cfg(feature = "web_server")]
#[derive(Debug)]
struct GeneratedSyncArtifact {
    path: PathBuf,
    file_name: String,
    file_size: u64,
    file_hash: Option<String>,
    record_count: Option<u64>,
}

#[cfg(feature = "web_server")]
async fn enqueue_generated_sync_tasks(artifacts: Vec<GeneratedSyncArtifact>) {
    if artifacts.is_empty() {
        return;
    }

    let runtime_guard = REMOTE_RUNTIME.read().await;
    let runtime_state = match runtime_guard.as_ref() {
        Some(state) => state.clone(),
        None => return,
    };
    drop(runtime_guard);

    let env_id = runtime_state.env_id.clone();
    let env_id_for_query = env_id.clone();

    let query_result = tokio::task::spawn_blocking(
        move || -> anyhow::Result<(Option<String>, Vec<(String, Option<String>)>)> {
            let conn = remote_sync_handlers::open_sqlite()?;

            let env_name = conn
                .prepare("SELECT name FROM remote_sync_envs WHERE id = ?1 LIMIT 1")?
                .query_row([env_id_for_query.as_str()], |row| row.get::<_, String>(0))
                .ok();

            let mut stmt_sites =
                conn.prepare("SELECT id, name FROM remote_sync_sites WHERE env_id = ?1")?;
            let site_iter = stmt_sites.query_map([env_id_for_query.as_str()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?;
            let mut sites = Vec::new();
            for item in site_iter {
                sites.push(item?);
            }

            Ok((env_name, sites))
        },
    )
    .await;

    let (env_name, site_entries) = match query_result {
        Ok(Ok(data)) => data,
        Ok(Err(err)) => {
            eprintln!("查询远程同步站点失败: {}", err);
            return;
        }
        Err(err) => {
            eprintln!("查询远程同步站点失败: {}", err);
            return;
        }
    };

    let source_env = get_db_option().location.clone();

    let targets: Vec<(Option<String>, Option<String>)> = if site_entries.is_empty() {
        vec![(None, None)]
    } else {
        site_entries
            .into_iter()
            .map(|(id, name)| (Some(id), name))
            .collect()
    };

    let mut center = SYNC_CONTROL_CENTER.write().await;
    for artifact in artifacts {
        let Some(path_str) = artifact.path.to_str().map(|s| s.to_string()) else {
            continue;
        };
        for (site_id_opt, site_name_opt) in &targets {
            center.add_task(NewSyncTaskParams {
                file_path: path_str.clone(),
                file_size: artifact.file_size,
                priority: 5,
                record_count: artifact.record_count,
                file_name: Some(artifact.file_name.clone()),
                file_hash: artifact.file_hash.clone(),
                env_id: Some(env_id.clone()),
                source_env: source_env.clone(),
                target_site: site_id_opt.clone(),
                direction: Some("UPLOAD".to_string()),
                notes: site_name_opt
                    .clone()
                    .or_else(|| env_name.clone())
                    .map(|name| format!("自动同步 - {}", name)),
            });
        }
    }
}

const JSON_CHUNK_COUNT: usize = 200;

pub const CHECK_DB_TYPES: [&'static str; 6] = ["CATA", "DESI", "DICT", "SYST", "GLB", "GLOB"];

impl AiosDBManager {
    /// 执行增量更新
    /// 执行增量更新操作
    ///
    /// 该函数处理多个数据库文件的增量更新。
    ///
    /// # 参数
    ///
    /// * `increment_ranges_map` - 包含路径和对应的数据库页面基本信息及会话号范围的映射
    ///   键为数据库文件路径，值为元组，包含数据库页面基本信息和需要更新的会话号范围
    ///
    /// # 返回值
    ///
    /// * `anyhow::Result<bool>` - 成功返回Ok(true)，失败返回错误
    ///
    /// # 错误
    ///
    /// 当数据库操作失败时会返回错误
    pub async fn execute_incr_update(
        &self,
        increment_ranges_map: IndexMap<PathBuf, (DbPageBasicInfo, RangeInclusive<i32>)>,
    ) -> anyhow::Result<bool> {
        for (path, (basic_info, sesno_range)) in increment_ranges_map {
            println!("Path: {:?}, Sesno Range: {:?}", path, &sesno_range);
            let mut io = PdmsIO::new("", path.clone(), true);
            io.open()
                .map_err(|e| anyhow::anyhow!("Failed to open PdmsIO: {}", e))?;
            let end_sesno = sesno_range.end().clone();
            let range_update_eles = io.collect_increment_eles(Some(sesno_range))?;
            io.update_elements_to_database(&range_update_eles, true)
                .await?;

            //执行逻辑

            //更新 sesno 到 db_file_info 中
            let file_name = path.file_stem().unwrap().to_str().unwrap();
            // dbg!(&file_name);
            //更新 sesno 到 db_file_info 中的sql
            let sql = format!("UPDATE db_file_info:{} SET sesno={};", file_name, end_sesno);
            //执行更新
            SUL_DB.query(sql).await.unwrap();
        }

        Ok(true)
    }

    /// 通过文件名查询数据库中最新的会话号
    ///
    /// # 参数
    ///
    /// * `file_name` - 要查询的数据库文件名
    ///
    /// # 返回值
    ///
    /// * `anyhow::Result<u32>` - 成功则返回最新会话号,失败返回错误
    ///
    /// # 错误
    ///
    /// 当数据库查询失败时会返回错误
    async fn query_latest_sesno_by_file_name(file_name: &str) -> anyhow::Result<u32> {
        let mut response = SUL_DB
            .query(format!(
                r#"
                select value sesno from only db_file_info:{} limit 1;
                "#,
                file_name
            ))
            .await?;
        let sesno: Option<u32> = response.take(0)?;
        Ok(sesno.unwrap_or_default())
    }

    /// 通过数据库编号查询数据库中最新的会话号
    ///
    /// # 参数
    ///
    /// * `dbnum` - 要查询的数据库编号
    ///
    /// # 返回值
    ///
    /// * `anyhow::Result<u32>` - 成功则返回最新会话号,失败返回错误
    ///
    /// # 错误
    ///
    /// 当数据库查询失败时会返回错误
    async fn query_latest_sesno_by_dbnum(dbnum: u32) -> anyhow::Result<u32> {
        // 从dbnum_info_table中查询对应dbnum的最大sesno
        // 使用更高效的查询，直接获取该dbnum的最大sesno值
        let mut response = SUL_DB
            .query(format!(
                r#"
                math::max(array::flatten([
                    SELECT VALUE sesno FROM dbnum_info_table WHERE dbnum = {}
                ]));
                "#,
                dbnum
            ))
            .await?;
        let sesno: Option<u32> = response.take(0)?;
        Ok(sesno.unwrap_or_default())
    }

    ///初始化监测
    /// 启动时监测数据文件夹里的文件变化
    pub async fn init_watcher(&self) -> anyhow::Result<()> {
        let mut params = IndexMap::new();
        fs::create_dir_all("assets/archives")?;
        let mut time = Instant::now();
        dbg!(&self.watcher.watch_dirs);
        let db_option = get_db_option();
        let manual_dbnums = db_option.manual_db_nums.clone().unwrap_or_default();
        let exclude_dbnums = db_option.exclude_db_nums.clone().unwrap_or_default();

        for watch_dir in &self.watcher.watch_dirs {
            for entry in WalkDir::new(watch_dir).sort_by(|a, b| {
                let a_len = a.path().metadata().map(|m| m.len()).unwrap_or_default();
                let b_len = b.path().metadata().map(|m| m.len()).unwrap_or_default();
                b_len.cmp(&a_len)
            }) {
                let dir_entry =
                    entry.map_err(|e| anyhow::anyhow!("Failed to get directory entry: {}", e))?;
                let path = dir_entry.path();
                let file_name = path
                    .file_stem()
                    .ok_or_else(|| {
                        anyhow::anyhow!("Failed to get file stem from path: {}", path.display())
                    })?
                    .to_str()
                    .ok_or_else(|| {
                        anyhow::anyhow!("Failed to convert file stem to string: {}", path.display())
                    })?;
                if path.is_dir() {
                    continue;
                }

                let DbBasicInfo {
                    db_type,
                    ses_pgno,
                    db_no,
                } = parse_db_basic_info(path.to_path_buf());
                //是否调试里有筛选
                if !manual_dbnums.is_empty() && !manual_dbnums.contains(&db_no) {
                    continue;
                }
                //过滤掉排除的数据库编号
                if !exclude_dbnums.is_empty() && exclude_dbnums.contains(&db_no) {
                    continue;
                }
                let project = get_db_option().project_name.clone();
                let file_latest_sesno = PdmsIO::new(&project, path.to_path_buf(), true)
                    .get_latest_sesno()
                    .unwrap_or_default();
                dbg!((db_no, file_latest_sesno));

                if !CHECK_DB_TYPES.contains(&db_type.as_str()) {
                    continue;
                }
                //TODO 这种情况，需要全新的解析
                let Ok(db_latest_sesno) = Self::query_latest_sesno_by_dbnum(db_no).await else {
                    //先暂时跳过数据库里没有的文件，todo 考虑自动追加文件全新解析
                    continue;
                };
                dbg!((db_no, db_latest_sesno));
                if db_latest_sesno == 0 {
                    continue;
                }
                self.watcher
                    .file_name_full_path_map
                    .insert(file_name.to_owned(), path.to_path_buf());
                // dbg!(db_latest_sesno);
                //只有开启异地同步时，才需要初始化异地更新压缩数据包
                #[cfg(feature = "mqtt")]
                {
                    // 初始化CBA的Archive文件，来保证后续增量下载, 后面是否需要加一个环境变量，来控制是否需要重新生成archive文件
                    // 是否需要完全初始化
                    let input = path.to_path_buf();
                    let output: PathBuf = format!("assets/archives/{}.cba", file_name).into();
                    // join_set.spawn(async move {
                    let compress_opt = CompressOptions::new(input, output, "assets/temp");
                    execute_compress(compress_opt)
                        .await
                        .expect("compress failed");
                    // });
                }

                //每个path 都要检查一遍
                // if db_latest_sesno != 0
                {
                    // #[cfg(feature = "debug_parse")]
                    // dbg!((db_no, db_latest_sesno));
                    //暂时先跳过更新比较大的
                    //
                    {
                        let mut io = PdmsIO::new(&project, path, true);
                        io.open()?;
                        if let Ok(basic_info) = io.get_page_basic_info() {
                            if file_latest_sesno > db_latest_sesno {
                                println!(
                                    "发现需要增量更新的文件: {:?}, 当前数据库属性最大sesno: {db_latest_sesno},\
                                        文件属性对应sesno: {file_latest_sesno}",
                                    &file_name
                                );
                                let nearest_sesno = io
                                    .get_nearest_large_sesno(db_latest_sesno as i32 + 1)
                                    .unwrap_or_default();
                                params.insert(
                                    path.to_path_buf(),
                                    (
                                        basic_info.clone(),
                                        //warning : db_latest_sesno as i32 + 1 不一定存在，需要找离他最近的sesno
                                        nearest_sesno..=file_latest_sesno as i32,
                                    ),
                                );
                            }
                            // 初始化监听的headers
                            self.watcher.headers.insert(path.to_path_buf(), basic_info);
                        }
                    }
                }
            }
        }

        //等所有的文件都检查同步完毕，才执行更新
        //按每个单独的 sesno
        if !params.is_empty() {
            dbg!(params.len());
        }
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

        create_dir_all("assets/archives")
            .await
            .map_err(|e| notify::Error::io(e))?;
        create_dir_all("assets/temp")
            .await
            .map_err(|e| notify::Error::io(e))?;
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
                    // 添加调试信息
                    println!("开始扫描数据库头部信息，路径: {:?}", &event.paths);
                    // dbg!(&self.watcher.headers);
                    if let Ok(new_headers) = PdmsWatcher::scan_db_headers(&event.paths) {
                        println!("成功扫描到 {} 个数据库头部", new_headers.len());
                        #[cfg(feature = "web_server")]
                        let mut generated_artifacts: Vec<
                            GeneratedSyncArtifact,
                        > = Vec::new();
                        let mut params = IndexMap::new();
                        for (path, new_header) in &new_headers {
                            println!("正在处理路径: {:?}", path);
                            // dbg!(&new_header.pdms_header);
                            // dbg!(path);
                            if let Some(mut old) = self.watcher.headers.get_mut(path) {
                                dbg!(path);
                                dbg!(new_header.latest_ses_data.sesno);
                                #[cfg(feature = "web_server")]
                                let prev_sesno = old.latest_ses_data.sesno;
                                let new_sesno = new_header.latest_ses_data.sesno;

                                // 从数据库获取最新的sesno，而不是使用缓存的值
                                let db_num = new_header.pdms_header.db_num;
                                let db_latest_sesno =
                                    match Self::query_latest_sesno_by_dbnum(db_num as _).await {
                                        Ok(sesno) => sesno,
                                        Err(e) => {
                                            println!("查询数据库最新sesno失败: {:?}", e);
                                            continue;
                                        }
                                    };

                                // dbg!(&old.pdms_header);
                                //未发生修改，直接跳过
                                if db_latest_sesno as i32 == new_sesno {
                                    continue;
                                }
                                //比如给出准确的范围next_sesno..=end_sesno
                                params.insert(
                                    path.clone(),
                                    (new_header.clone(), (db_latest_sesno as i32 + 1)..=new_sesno),
                                );
                            } else {
                                println!("watcher.headers: {:?}", self.watcher.headers);
                                println!("在 watcher.headers 中找不到路径: {:?}", path);
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
                                    let dbno = new_header.pdms_header.db_num as u32;
                                    if path.is_dir() {
                                        continue;
                                    }
                                    // dbg!(&file_name);
                                    //这个地方是不是需要直接去读取文件，然后更新headers，不能太依赖json数据
                                    //或者每次启动都重新更新这个文件？
                                    if let Some(mut old) = self.watcher.headers.get_mut(&path) {
                                        // dbg!((
                                        //     old.latest_ses_data.sesno,
                                        //     new_header.latest_ses_data.sesno
                                        // ));
                                        //未发生修改，直接跳过
                                        let prev_sesno = old.latest_ses_data.sesno;
                                        let new_sesno = new_header.latest_ses_data.sesno;
                                        if old.latest_ses_data.sesno >= new_sesno {
                                            continue;
                                        }
                                        *old.value_mut() = new_header;

                                        //发生修改的文件，重新生成archive
                                        // dbg!(&path);
                                        let output: PathBuf =
                                            format!("assets/archives/{}.cba", file_name).into();
                                        // dbg!(&output);

                                        let compress_opt = CompressOptions::new(
                                            path.clone(),
                                            output,
                                            "assets/temp",
                                        );
                                        let file_hash = execute_compress(compress_opt)
                                            .await
                                            .unwrap()
                                            .to_string();
                                        // dbg!(&file_hash);
                                        #[cfg(feature = "web_server")]
                                        {
                                            let archive_size = fs::metadata(&output)
                                                .await
                                                .map(|m| m.len())
                                                .unwrap_or(0);
                                            let delta = new_sesno.saturating_sub(prev_sesno) as u64;
                                            generated_artifacts.push(GeneratedSyncArtifact {
                                                path: output.clone(),
                                                file_name: format!("{file_name}.cba"),
                                                file_size: archive_size,
                                                file_hash: Some(file_hash.clone()),
                                                record_count: if delta > 0 {
                                                    Some(delta)
                                                } else {
                                                    None
                                                },
                                            });
                                        }

                                        //如果location_dbs为空，则不进行筛选
                                        //说明是所有地区都推送，跳过检查
                                        //必须要是地区对应的dbnos才能推送
                                        if let Some(location_dbs) = &get_db_option().location_dbs {
                                            if !location_dbs.contains(&dbno) {
                                                continue;
                                            }
                                        }

                                        //数据库里不存在这个file hash的记录，才需要发送
                                        //是自己创建的，在记录里还没有的，才能发送消息出去
                                        //如果是别的创建的，就应该调过
                                        let sql = format!(
                                            "select value <string>\
                                            id from (select * from e3d_sync where location != '{}' and '{}' in file_names and '{}' in file_hashes order by timestamp desc) ",
                                            get_db_option().location.as_str(),
                                            file_name,
                                            &file_hash
                                        );
                                        // dbg!(&sql);
                                        // println!("sql is {}", &sql);
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
                        #[cfg(feature = "mqtt")]
                        if !notify_file_names.is_empty() {
                            let payload =
                                SyncE3dFileMsg::new(notify_file_names, notify_file_hashes);
                            //自己本地也要保存
                            // todo 后续还是要配置哪些dbs，哪个地方能修改，哪个地方是不能改的
                            SUL_DB
                                .query(format!(
                                    "INSERT IGNORE INTO e3d_sync {} ",
                                    serde_json::to_string(&payload).unwrap()
                                ))
                                .await
                                .unwrap();
                            //todo 检查是否只是发生了claim page的变化，如果只是claim修改，是需要每次都同步？
                            //会导致出现循环
                            self.mqtt_client
                                .clone()
                                .publish("Sync/E3d", QoS::ExactlyOnce, true, payload)
                                .await
                                .unwrap();
                        }
                        #[cfg(feature = "web_server")]
                        enqueue_generated_sync_tasks(generated_artifacts).await;
                    } else {
                        println!("扫描数据库头部失败，错误路径: {:?}", &event.paths);
                    }
                }
                Err(e) => println!("watch error: {:?}", e),
            }
        }

        Ok(())
    }
}
