use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aios_core::SUL_DB;
use aios_core::accel_tree::acceleration_tree::{AccelerationTree, RStarBoundingBox};
use aios_core::file_helper::collect_db_dirs;
use aios_core::get_db_option;
use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use dashmap::DashMap;
use futures::StreamExt;
use glam::Vec3;
use indexmap::IndexMap;
use itertools::Itertools;
use log::{error, info};
use once_cell::sync::Lazy;
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use parry3d::math::Vector;
use parry3d::query::{Ray, RayCast};
use pdms_io::sync::clone::{CloneOptions, execute_clone};
// use pdms_io::sync::clone::{execute_clone, CloneOptions};
use pdms_io::watch::PdmsWatcher;
use rayon::prelude::*;
use rumqttc::Event::Incoming;
use rumqttc::{Packet, QoS};
#[cfg(feature = "sql")]
use sqlx::pool::PoolOptions;
#[cfg(feature = "sql")]
use sqlx::{Executor, MySql, MySqlPool, Pool, Row};
use tokio::sync::Mutex;

use crate::consts::*;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::defines::CACHED_MDB_SITE_MAP;
use crate::mqtt_service::{SyncE3dFileMsg, new_mqtt_inst};

pub const TUBI_TOL: f32 = 0.1f32;

// project + mdb + module
pub static GLOBAL_MDB_WORLD_MAP: Lazy<DashMap<String, PdmsElement>> = Lazy::new(DashMap::new);

static PDMS_GNERAL_TYPE_NAMES_MAP: Lazy<HashMap<&'static str, PdmsGenericType>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert("EQUI", PdmsGenericType::EQUI);
    m.insert("PIPE", PdmsGenericType::PIPE);
    m.insert("ROOM", PdmsGenericType::ROOM);
    m.insert("STRU", PdmsGenericType::STRU);
    m.insert("PANE", PdmsGenericType::PANE);
    m.insert("HANG", PdmsGenericType::HANG);
    m.insert("WALL", PdmsGenericType::WALL);
    m.insert("GWALL", PdmsGenericType::WALL);
    m.insert("CWALL", PdmsGenericType::WALL);
    m.insert("STWALL", PdmsGenericType::WALL);
    m.insert("CFLOOR", PdmsGenericType::CFLOOR);
    m.insert("FLOOR", PdmsGenericType::FLOOR);
    m.insert("EXTR", PdmsGenericType::EXTR);
    m.insert("REVO", PdmsGenericType::REVO);
    m
});

//创建一个监控mqtt是否连接的全局变量,使用Mutex<bool>
pub static MQTT_CONNECT_STATUS: Lazy<Mutex<Option<bool>>> = Lazy::new(|| Mutex::new(None));

impl AiosDBManager {
    /// 从默认配置文件初始化
    pub async fn init_form_config() -> anyhow::Result<Self> {
        let db_option = get_db_option();
        let mut mgr = Self::init(&db_option).await?;
        Ok(mgr)
    }

    //初始化watcher
    pub async fn exec_watcher(mgr: Arc<AiosDBManager>) -> anyhow::Result<()> {
        mgr.init_watcher().await.unwrap();
        mgr.async_watch().await.unwrap();
        Ok(())
    }

    //开启定时同步更新任务
    pub async fn run_e3d_clone_bg_task(mgr: Arc<AiosDBManager>) -> anyhow::Result<()> {
        dbg!("定时同步数据任务开启");
        let forever = tokio::spawn(async move {
            //10分钟强制刷一遍
            let mut interval = tokio::time::interval(Duration::from_secs(60 * 10));
            loop {
                interval.tick().await;
                //todo，需要配置各个db对应的映射, 不同区域对应不同的db
                // Self::exec_delta_clone_remotes(&mgr.watcher, &[]).await.unwrap();
            }
        });
        forever.await?
    }

    //增量从服务器里的数据clone到本地
    pub async fn exec_delta_clone_remotes(
        watcher: &PdmsWatcher,
        sync_msg: SyncE3dFileMsg,
    ) -> anyhow::Result<bool> {
        if sync_msg.file_names.is_empty() {
            return Ok(false);
        }
        let loc_dbs = &get_db_option().location_dbs;
        let remote_url = sync_msg.file_server_host.as_str();
        for file_name in &sync_msg.file_names {
            let url = format!("{}/{}.cba", remote_url, file_name);
            dbg!(&file_name);
            //todo 如果没有需要新加数据
            let Some(pb) = watcher.file_name_full_path_map.get(file_name) else {
                continue;
            };
            dbg!(&pb);

            //还需要检查location dbnum，如果不一致，就需要clone
            //必须不是当前区域的db 才能clone, 只能clone别的区域的数据
            if let Some(dbno) = watcher.get_dbno(&pb) {
                dbg!(dbno);
                //跳过当前区域的dbnos
                if let Some(dbs) = loc_dbs {
                    if dbs.contains(&dbno) {
                        continue;
                    }
                }
            }

            println!(
                "Start delta clone db files num: {} from {}",
                sync_msg.file_names.len(),
                &url
            );
            let e3d_file: PathBuf = pb.value().clone();
            let mut clone_time = Instant::now();
            let remote_clone_opt = CloneOptions::new_remote(url.as_str(), e3d_file);
            if let Ok(r) = execute_clone(remote_clone_opt).await {
                if r {
                    //需要保存更新记录
                    println!(
                        "Clone {} cost: {:?}s",
                        file_name,
                        clone_time.elapsed().as_secs_f64()
                    );
                    //clone完了,再执行增量更新
                }
            }
        }

        Ok(true)
    }

    pub async fn spawn_exec_watcher(mgr: Arc<AiosDBManager>) -> anyhow::Result<()> {
        let f = tokio::spawn(async move {
            mgr.init_watcher().await.unwrap();
            mgr.async_watch().await.unwrap();
        });
        Ok(f.await?)
    }

    pub async fn demo_mqtt_requests() {
        let mut mqtt_inst = new_mqtt_inst("test-1");
        let client = mqtt_inst.client.clone();
        let f = tokio::spawn(async move {
            for i in 1..=10000 {
                let test_data = SyncE3dFileMsg {
                    file_names: vec![format!("Hello-{}", i)],
                    file_hashes: vec![],
                    file_server_host: "http://50c170h624.zicp.vip:56785/assets/archives"
                        .to_string(),
                    location: "bj".to_string(),
                    timestamp: Default::default(),
                };
                let _ = client
                    .publish("Sync/E3d", QoS::ExactlyOnce, false, test_data)
                    .await
                    .unwrap();

                dbg!(i);

                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            // tokio::time::sleep(Duration::from_secs(120)).await;
        });

        loop {
            let event = mqtt_inst.el.poll().await;
        }

        f.await.expect("demo_mqtt_requests panic");
    }

    ///另外将里面可能有关联的db，也要同步检查后一下？？
    ///处理mqtt的消息, 通知需要处理的db 文件名，然后对应的归属地也需要发送
    pub async fn poll_sync_e3d_mqtt_events(watcher: Arc<PdmsWatcher>) {
        let db_option = get_db_option();
        let location = db_option.location.clone();
        let f = tokio::spawn(async move {
            //订阅消息处理更新
            let mut mqtt_inst = new_mqtt_inst(&format!(
                "{}-{}-sub",
                db_option.location.as_str(),
                db_option.project_code
            ));
            mqtt_inst
                .client
                .subscribe("Sync/E3d", QoS::ExactlyOnce)
                .await
                .unwrap();
            // 连接超时已在 MqttOptions 上配置，无需再从 EventLoop 访问网络选项
            loop {
                let event = mqtt_inst.el.poll().await;
                match &event {
                    Ok(v) => {
                        match v {
                            Incoming(Packet::Publish(p)) => {
                                let sync_e3d = SyncE3dFileMsg::from(p.payload.to_vec());
                                // println!("payload = {:?}", &sync_e3d);
                                //检查是否和本地的location一致，如果不一致，才发生更新
                                if sync_e3d.location != location {
                                    //自己本地也要保存, todo 后续还是要配置哪些dbs，哪个地方能修改，哪个地方是不能改的
                                    SUL_DB
                                        .query(format!(
                                            "INSERT IGNORE INTO e3d_sync {} ",
                                            serde_json::to_string(&sync_e3d).unwrap()
                                        ))
                                        .await
                                        .unwrap();
                                    //执行指定文件的clone
                                    Self::exec_delta_clone_remotes(&watcher, sync_e3d)
                                        .await
                                        .unwrap();
                                }
                            }
                            _ => {
                                // dbg!(v);
                            }
                        }
                    }
                    Err(e) => {
                        // println!("Error = {e:?}");
                        // return Ok(());
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    _ => {}
                }
            }
        });
        f.await.expect("demo_mqtt_requests panic");
    }

    ///处理mqtt的消息，带重连退避（单位ms）
    pub async fn poll_sync_e3d_mqtt_events_with_backoff(
        watcher: Arc<PdmsWatcher>,
        initial_backoff_ms: u64,
        max_backoff_ms: u64,
    ) {
        let db_option = get_db_option();
        let location = db_option.location.clone();
        let mut backoff = initial_backoff_ms.max(100);
        let max_backoff = max_backoff_ms.max(backoff);

        loop {
            // 构造新的连接实例
            let mut mqtt_inst = new_mqtt_inst(&format!(
                "{}-{}-sub",
                db_option.location.as_str(),
                db_option.project_code
            ));
            let _ = mqtt_inst
                .client
                .subscribe("Sync/E3d", QoS::ExactlyOnce)
                .await;

            // 轮询事件，直到错误发生
            loop {
                let event = mqtt_inst.el.poll().await;
                match &event {
                    Ok(v) => match v {
                        Incoming(Packet::Publish(p)) => {
                            let sync_e3d = SyncE3dFileMsg::from(p.payload.to_vec());
                            if sync_e3d.location != location {
                                let _ = SUL_DB
                                    .query(format!(
                                        "INSERT IGNORE INTO e3d_sync {} ",
                                        serde_json::to_string(&sync_e3d).unwrap()
                                    ))
                                    .await;
                                let _ = Self::exec_delta_clone_remotes(&watcher, sync_e3d).await;
                            }
                            // 收到消息，视为连接正常，重置退避
                            backoff = initial_backoff_ms.max(100);
                        }
                        _ => {}
                    },
                    Err(e) => {
                        {
                            let mut mqtt_connect_status = MQTT_CONNECT_STATUS.lock().await;
                            if mqtt_connect_status.is_none() {
                                *mqtt_connect_status = Some(false);
                                error!("Init MQTT Connection error encountered: {}", e);
                            } else if (*mqtt_connect_status).unwrap() {
                                *mqtt_connect_status = Some(false);
                                error!("MQTT Connection error encountered: {}", e);
                            }
                        }
                        // 发生错误，退出内层循环以重建连接
                        break;
                    }
                }
            }
            // 退避等待后重建连接
            tokio::time::sleep(Duration::from_millis(backoff)).await;
            backoff = (backoff.saturating_mul(2)).min(max_backoff);
        }
    }

    ///快速获得table名称
    // 已废弃: cache 模块已移除
    pub fn get_table_name(&self, refno: RefU64) -> String {
        "UNSET".to_string()
    }

    ///获得默认的连接字符串
    #[inline]
    pub fn get_default_conn_str(d: &DbOption) -> String {
        let user = d.user.as_str();
        let pwd = urlencoding::encode(d.password.as_str());
        let ip = d.ip.as_str();
        let port = d.port.as_str();
        format!("mysql://{user}:{pwd}@{ip}:{port}")
    }

    #[cfg(feature = "sql")]
    #[inline]
    pub async fn get_global_pool(&self) -> anyhow::Result<Pool<MySql>> {
        let connection_str = self.default_conn_str();
        let url = &format!("{connection_str}/{}", GLOBAL_DATABASE);
        PoolOptions::new()
            .max_connections(500)
            .acquire_timeout(Duration::from_secs(10 * 60))
            .connect(url)
            .await
            .map_err({ |x| anyhow::anyhow!(x.to_string()) })
    }

    ///获得默认的连接字符串
    #[inline]
    pub fn default_conn_str(&self) -> String {
        let d = &self.db_option;
        let user = d.user.as_str();
        let pwd = urlencoding::encode(&d.password);
        let ip = d.ip.as_str();
        let port = d.port.as_str();
        format!("mysql://{user}:{pwd}@{ip}:{port}")
    }
    /// 获得pool
    #[cfg(feature = "sql")]
    #[inline]
    pub async fn get_db_pool(connection_str: &str, project: &str) -> anyhow::Result<Pool<MySql>> {
        let url = &format!("{connection_str}/{}", project);
        PoolOptions::new()
            .max_connections(500)
            .acquire_timeout(Duration::from_secs(10 * 60))
            .connect(url)
            .await
            .map_err({ |x| anyhow::anyhow!(x.to_string()) })
    }

    #[inline]
    pub fn puhua_conn_str(&self) -> String {
        let d = &self.db_option;
        let user = d.puhua_database_user.as_str();
        let pwd = d.puhua_database_password.as_str();
        let ip = d.puhua_database_ip.as_str();
        format!("mysql://{user}:{pwd}@{ip}")
    }

    ///获取普华mysql数据库的连接pool
    #[cfg(feature = "sql")]
    #[inline]
    pub async fn get_puhua_pool(&self) -> anyhow::Result<Pool<MySql>> {
        let conn = self.puhua_conn_str();
        let url = &format!("{conn}/{}", PUHUA_MATERIAL_DATABASE);
        PoolOptions::new()
            .max_connections(500)
            .acquire_timeout(Duration::from_secs(10 * 60))
            .connect(url)
            .await
            .map_err({ |x| anyhow::anyhow!(x.to_string()) })
    }

    ///获取mysql数据库模糊查询的连接pool
    #[cfg(feature = "sql")]
    #[inline]
    pub async fn get_fuzzy_query_pool(&self) -> anyhow::Result<Pool<MySql>> {
        let connection_str = self.default_conn_str();
        let url = &format!("{connection_str}/{}", FUZZY_QUERT);
        PoolOptions::new()
            .max_connections(500)
            .acquire_timeout(Duration::from_secs(10 * 60))
            .connect(url)
            .await
            .map_err({ |x| anyhow::anyhow!(x.to_string()) })
    }

    ///获得默认的pool
    #[cfg(feature = "sql")]
    #[inline]
    pub async fn get_default_pool(conn_str: &str) -> anyhow::Result<Pool<MySql>> {
        MySqlPool::connect(conn_str)
            .await
            .map_err(|x| anyhow::anyhow!(x.to_string()))
    }

    /// 初始化mdb
    pub async fn init_mdb(&mut self, project: &str, mdb: &str, module: &str) -> anyhow::Result<()> {
        Ok(())
    }

    ///初始化db manager
    pub async fn init(db_option: &DbOption) -> anyhow::Result<Self> {
        let dir = db_option.project_path.to_string();
        #[cfg(feature = "sql")]
        let mut project_map = DashMap::new();
        let default_conn = AiosDBManager::get_default_conn_str(&db_option);
        let projects = db_option.get_project_dir_names().clone();

        let db_paths =
            collect_db_dirs(&db_option.project_path, projects.iter().map(|x| x.as_ref()))
                .unwrap_or_default();
        let mut watcher = PdmsWatcher::new(db_paths);
        #[cfg(feature = "debug_watch")]
        {
            dbg!(&db_paths);
            dbg!(watcher.headers.len());
            dbg!(watcher.file_name_full_path_map.len());
        }
        let mut mqtt_inst = new_mqtt_inst(&format!(
            "{}-{}-pub",
            db_option.location.as_str(),
            db_option.project_code
        ));
        let mqtt_client = Arc::new(mqtt_inst.client);
        #[cfg(feature = "mqtt")]
        tokio::task::spawn(async move {
            loop {
                let event = mqtt_inst.el.poll().await;
                match event {
                    Ok(event) => match event {
                        rumqttc::Event::Incoming(Packet::Publish(_)) => {
                            // Currently unused, but we can subscribe to topics to get messages here
                        }
                        rumqttc::Event::Incoming(Packet::ConnAck(_)) => {
                            // Connection was established. Notify the client to send all discovery messages
                            // info!("Connected to MQTT broker.");

                            //判断MQTT_CONNECT_STATUS,如果为false,则发送连接成功的消息,修改为true
                            let mut mqtt_connect_status = MQTT_CONNECT_STATUS.lock().await;
                            if mqtt_connect_status.is_none() {
                                *mqtt_connect_status = Some(true);
                                info!("Init connected to MQTT broker.");
                            } else {
                                if !(*mqtt_connect_status).unwrap() {
                                    *mqtt_connect_status = Some(true);
                                    info!("Connected to MQTT broker.");
                                }
                            }
                        }
                        _ => {}
                    },
                    Err(e) => {
                        let mut mqtt_connect_status = MQTT_CONNECT_STATUS.lock().await;
                        if mqtt_connect_status.is_none() {
                            *mqtt_connect_status = Some(false);
                            error!("Init MQTT Connection error encountered: {}", e);
                        } else {
                            if (*mqtt_connect_status).unwrap() {
                                *mqtt_connect_status = Some(false);
                                error!("MQTT Connection error encountered: {}", e);
                            }
                        }

                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        });
        Ok(Self {
            #[cfg(feature = "sql")]
            project_map,
            projects,
            needed_parse_files: None,
            project_path: dir,
            db_option: db_option.clone(),
            watcher: Arc::new(watcher),
            mqtt_client,
            rtree: None,
        })
    }

    /// 根据project获取连接池
    #[cfg(feature = "sql")]
    #[inline]
    pub fn get_project_pool(&self, project: &str) -> Option<Pool<MySql>> {
        self.project_map.get(project).map(|x| x.value().clone())
    }

    /// 根据project获取连接池
    #[cfg(feature = "sql")]
    #[inline]
    pub fn get_cur_project_pool(&self) -> Option<Pool<MySql>> {
        self.project_map
            .get(self.get_cur_project())
            .map(|x| x.value().clone())
    }

    ///获得project 的db
    #[cfg(feature = "sql")]
    #[inline]
    pub async fn get_project_pool_by_refno(&self, refno: RefU64) -> Option<(String, Pool<MySql>)> {
        // if let Some(projects) = self.ref0_projects.get(&refno.get_0()) {
        //     ///只有一个的时候
        //     if projects.len() == 1 {
        //         let project = projects.value().iter().next().as_ref().unwrap().clone();
        //         if let Some(project_pool) = self.project_map.get(project) {
        //             return Some((project.clone(), project_pool.value().clone()));
        //         }
        //     } else {
        //         for project in &self.db_option.included_projects {
        //             if let Some(pool) = self.get_project_pool(project) {
        //                 // if check_exist_refno(refno, &pool, &self.mdb_dbnums)
        //                 //     .await
        //                 //     .ok()?
        //                 // {
        //                     return Some((project.clone(), pool.clone()));
        //                 // }
        //             }
        //         }
        //     }
        // }
        None
    }

    fn match_stype(input: i32) -> String {
        match input {
            1 => "DESI".to_string(),
            2 => "CATA".to_string(),
            4 => "PROP".to_string(),
            6 => "ISOD".to_string(),
            7 => "PADD".to_string(),
            8 => "DICT".to_string(),
            9 => "ENGI".to_string(),
            14 => "SCHE".to_string(),
            _ => "".to_string(),
        }
    }

    ///获得当前mdb下的site参考号
    pub async fn get_site_refnos(&self) -> anyhow::Result<Vec<RefU64>> {
        // let world_refno = self.get_desi_world().await?.refno;
        // let r = self
        //     .get_cached_site_nodes(world_refno)
        //     .await?
        //     .unwrap_or_default()
        //     .iter()
        //     .map(|x| x.refno)
        //     .collect();
        Ok(vec![])
    }
}

#[tokio::test]
async fn test_get_attr() -> anyhow::Result<()> {
    // let mut mgr = AiosDBManager::init_form_config().await?;
    // let refno: RefU64 = RefI32Tuple((23584, 8)).into();
    // let v = mgr.get_attr(refno).await?;
    // println!("v={:?}", v.to_string_hashmap());

    // mgr.cache_geos_data("Sample", "SAMPLE").await?;

    Ok(())
}

#[test]
fn test_compute_distance() {
    let x = Vec3::new(19373.929, -2923.338, 15286.0);
    let y = Vec3::new(19381.39, -2894.83, 15286.0);
    let arrive = x.distance(y);
    let z = Vec3::new(19381.39, -2865.362, 15286.0);
    let leave = z.distance(y);
    let inst_a = Vec3::new(28.508010864257812, 7.4603271484375, 0.0);
    let inst_b = Vec3::new(0.0, 0.0, 0.0);
    let inst_dis = inst_a.distance(inst_b);
    dbg!(&inst_dis);
    dbg!(&arrive);
    dbg!(&leave);
}
