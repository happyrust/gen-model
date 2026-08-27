use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aios_core::SUL_DB;
use aios_core::accel_tree::acceleration_tree::{AccelerationTree, RStarBoundingBox};
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
use crate::defines::{CACHED_MDB_SITE_MAP, CACHED_REFNO_BASIC_MAP};
use crate::mqtt_service::{SyncE3dFileMsg, new_mqtt_inst};

pub const TUBI_TOL: f32 = 0.1f32;

/// 隐式直管段的连接容差(mm):相邻构件的 leave→arrive 缝隙短于它时视为「已连接的
/// 建模余量」,不再合成一段填充直管——与 E3D 的行为对齐。
///
/// 背景(2026-08-12 db8000 BRAN 增量取证,RVM 真值对拍):gen-model 的构件坐标与
/// E3D 逐一吻合(≤1mm),但相邻显式管件在源数据里本就带亚毫米~几毫米的关节余量
/// (实测 `/C-OR-1R345-C`:FTUB2 `POS+HEIG` 距 FTUB3 `POS` 为 1403 vs HEIG 1400,
/// ~3mm)。E3D 对这类缝**零产管**(RVM 导出零隐式管容器);而 `TUBI_TOL=0.1mm`
/// 太紧,gen-model 给每个关节都合成一段 0.66~2.70mm 的「薄饼管」,再以 leave_refno
/// 为键覆盖掉构件自身几何(见 ADR / 审查记录的 D1 覆盖缺陷)。
///
/// 取值 5.0mm 的依据(非拍脑袋):db8000 全部 91 条 tubi 的长度分布在 4.18mm 与
/// 6.70mm 之间有一条干净断层——54 条 ≤4.18mm 是薄饼,6.70mm 起才是成片真实管段;
/// 5.0mm 落在断层内,且 > 本支管实测被 E3D 容忍的最大缝 2.70mm,既滤掉薄饼又不误杀
/// 真管。E3D 的连接容差本身不是分支属性(实测 BRAN 的 121 个属性里无 BTOL/CBTOLE),
/// 是 core.dll 内部规则;此处按实证取值,若日后从 core.dll 逆出权威阈值再精化。
pub const TUBI_CONNECT_TOL: f32 = 5.0f32;

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
        let db_option = crate::get_db_option_ext().inner;
        let mut mgr = Self::init(&db_option).await?;
        Ok(mgr)
    }

    //初始化watcher
    /// 看门狗的失败必须向上传播（T903）：过去这里 `.unwrap()`，init 失败直接 panic，
    /// 而 `async_watch` 静默返回 Ok 时又看不出任何异常。
    ///
    /// 合流后两条自动路径都只「发现即入队」，这里必须确保队列消费者在跑，
    /// 否则批次入了队没人执行（`ensure_batch_worker` 幂等，重复调用无害）。
    pub async fn exec_watcher(mgr: Arc<AiosDBManager>) -> anyhow::Result<()> {
        crate::data_interface::batch_worker::ensure_batch_worker(mgr.clone());
        mgr.init_watcher().await?;
        mgr.async_watch().await?;
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
            // 后台任务里 panic 只会毒死这一个 task 且往往无人查看，改为显式告警（T903）。
            crate::data_interface::batch_worker::ensure_batch_worker(mgr.clone());
            if let Err(e) = mgr.init_watcher().await {
                log::error!("init_watcher 失败，增量看门狗未启动: {e:?}");
                eprintln!("init_watcher 失败，增量看门狗未启动: {e:?}");
                return;
            }
            if let Err(e) = mgr.async_watch().await {
                log::error!("async_watch 退出，增量看门狗已停止: {e:?}");
                eprintln!("async_watch 退出，增量看门狗已停止: {e:?}");
            }
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
            mqtt_inst.el.network_options.set_connection_timeout(10000);
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

    ///快速获得table名称
    pub fn get_table_name(&self, refno: RefU64) -> String {
        CACHED_REFNO_BASIC_MAP
            .get(&refno)
            .map(|x| x.get_table_name().to_string())
            .unwrap_or("UNSET".to_string())
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

    /// 初始化 mdb：把这个 MDB 声明了哪些库解出来并登记，供后续按类型取用。
    ///
    /// 之前这里是个空壳（只有 `Ok(())`），于是「哪些库属于本 MDB」在初始化期
    /// 无人回答，字典库只能靠扫项目目录去猜——而 MDB 是**跨项目**的：
    /// `AvevaMarineSample /ALL` 声明的六个字典库有四个在 `AvevaCatalogue`
    /// 与 `SCB` 底下。猜错不报错，少一个字典库跟「这个 UDA 没值」长得一模一样。
    ///
    /// `module` 暂时只入日志。E3D 里 UDA 看起来是按模块过滤的（Design 下
    /// `q att` 不打印 `PSI` / `MDS` 那几个应用的 UDA），但字典元素里目前解不出
    /// 任何字段支持这个说法，没证据就不按它筛。
    pub async fn init_mdb(&mut self, project: &str, mdb: &str, module: &str) -> anyhow::Result<()> {
        use crate::data_interface::mdb_membership;

        let membership = mdb_membership::resolve(&self.db_option, project, mdb)?;
        let counts = membership.counts_by_type();
        let dictionaries = membership.dictionary_paths();
        println!(
            "MDB {} / {project} / module={module}：声明 {} 个库（按 STYP {counts:?}），\
             其中字典库 {} 个",
            membership.mdb(),
            membership.databases().len(),
            dictionaries.len()
        );
        for path in &dictionaries {
            println!("  字典库 {}", path.display());
        }
        // 声明了却不在盘上的库要出声。它不是「本 MDB 没有这个库」，是部署缺件，
        // 两者对下游的意思完全不同。
        for database in membership.unresolved() {
            log::warn!(
                "MDB {} 声明了 dbnum={} ({})，但配置的项目目录里找不到对应文件",
                membership.mdb(),
                database.dbnum,
                database.name
            );
        }
        mdb_membership::install(membership);
        Ok(())
    }

    ///初始化db manager
    pub async fn init(db_option: &DbOption) -> anyhow::Result<Self> {
        let dir = db_option.project_path.to_string();
        #[cfg(feature = "sql")]
        let mut project_map = DashMap::new();
        let default_conn = AiosDBManager::get_default_conn_str(&db_option);
        let projects = db_option.get_project_dir_names().clone();

        // 监控目录逐项目解析：本地盘与共享目录（UNC / 映射盘）混排都要认，而且一个
        // 项目解析失败不能带走其余项目。解析结论无论成败都打出来——这份目录集合同时
        // 是自动看门狗的监听面和手动摄入的候选面，它空着就等于「服务在跑但什么都不更新」，
        // 而过去这里是 `collect_db_dirs(..).unwrap_or_default()`，失败连一个字都不留。
        let plan = crate::data_interface::project_paths::plan_watch_dirs(db_option);
        // 「这个目录属于哪个项目」只有解析这一刻知道，摄入侧靠它给批次定归属。
        crate::data_interface::project_paths::record_watch_dir_owners(&plan);
        let db_paths = plan.dirs();
        println!(
            "监控目录解析（共 {} 个库目录）:\n{}",
            db_paths.len(),
            plan.describe()
        );
        for problem in plan.problems() {
            log::warn!("监控目录解析失败: {problem}");
            eprintln!("监控目录解析失败: {problem}");
        }
        // 逐项目归档：一个项目解析不出库目录，外在表现就是「服务在跑但这个项目什么都
        // 不更新」，而启动时那两行早就滚走了。按项目名归行，修好之后下次启动自动销账。
        for project in &plan.projects {
            match project.problem.as_deref() {
                Some(problem) => {
                    crate::data_interface::parse_error::note_dir_failure(&project.project, problem)
                }
                None => crate::data_interface::parse_error::note_dir_success(&project.project),
            }
        }
        // 这份名单来自配置、就是全集，所以不在名单里的在册项目一律销账：从配置里
        // 删掉一个项目之后它再也不会被 note 到，逐目标的成功销账永远够不着它。
        crate::data_interface::parse_error::note_dir_scope(
            &plan
                .projects
                .iter()
                .map(|project| project.project.clone())
                .collect(),
        )
        .await;
        if let Err(error) = crate::data_interface::parse_error::flush().await {
            log::warn!("{error:#}");
        }
        let mut watcher = PdmsWatcher::new(db_paths);
        #[cfg(feature = "debug_watch")]
        {
            dbg!(&watcher.watch_dirs);
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
