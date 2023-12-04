use std::str::FromStr;
use crate::api::element::*;
use crate::api::refno_info::*;
use crate::aql_api::children::query_deep_children_refnos_fuzzy;
use crate::aql_api::pdms_mesh::query_pdms_mesh_aql;
use crate::aql_api::pdms_room::{RoomElement, RoomPanelElement};
use crate::arangodb::ArDatabase;
use crate::cata::resolve_helper::eval_str_to_f32;
use crate::consts::*;
use crate::consts::*;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::defines::{CACHED_MDB_SITE_MAP, CACHED_REFNO_BASIC_MAP};
use crate::graph_db::pdms_arango::{connect_arangodb, save_arangodb_with_db_option};
use crate::graph_db::pdms_inst_arango::query_insts_shape_data;
use crate::graph_db::structs::{PdmsEleData, PdmsEleEdge, PdmsMdbEdge};
use crate::mqtt_service::{new_mqtt_inst, SyncE3dFileMsg};

use aios_core::accel_tree::acceleration_tree::{AccelerationTree, RStarBoundingBox};
use aios_core::file_helper::collect_db_dirs;
use aios_core::get_db_option;
use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use aios_core::SUL_DB;
use aios_core::tool::db_tool::{GLOBAL_UDA_NAME_MAP, GLOBAL_UDA_UKEY_MAP};
use arangors_lite::AqlQuery;
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
use pdms_io::sync::clone::{execute_clone, CloneOptions};
use pdms_io::watch::PdmsWatcher;
use rayon::prelude::*;
use rumqttc::Event::Incoming;
use rumqttc::{Client, ConnectionError, Event, MqttOptions, Packet, QoS};
use sqlx::pool::PoolOptions;
use sqlx::{Executor, MySql, MySqlPool, Pool, Row};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const TUBI_TOL: f32 = 10.0f32;

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

impl AiosDBManager {
    /// 从默认配置文件初始化
    pub async fn init_form_config() -> anyhow::Result<Self> {
        let db_option = get_db_option();
        let mut mgr = Self::init(&db_option).await?;
        println!("正在初始化uda");
        // mgr.init_mdb(
        //     &db_option.project_name,
        //     &db_option.mdb_name,
        //     &db_option.module,
        // ).await?;
        //初始化watcher
        //加载空间树
        if db_option.load_spatial_tree {
            mgr.compute_aabb_trees().await?;
        }
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
    ) -> anyhow::Result<()> {
        println!(
            "Start delta clone db files num: {}",
            sync_msg.file_names.len()
        );
        let remote_url = sync_msg.file_server_host.as_str();
        for file_name in &sync_msg.file_names {
            let url = format!("{}/{}.cba", remote_url, file_name);
            //todo 如果没有需要新加数据
            let Some(pb) = watcher.file_name_full_path_map.get(file_name) else {
                continue;
            };
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
                }
            }
        }

        Ok(())
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
                    file_server_host: "http://50c170h624.zicp.vip:56785/asset/archives".to_string(),
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
                                //检查是否和本地的location一致，如果一致，就不用更新
                                if sync_e3d.location != location {
                                    //自己本地也要保存, todo 后续还是要配置哪些dbs，哪个地方能修改，哪个地方是不能改的
                                    SUL_DB
                                        .query(format!(
                                            "INSERT INTO e3d_sync {} ",
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

    pub async fn compute_aabb_trees(&mut self) -> anyhow::Result<bool> {
        //测试分页查询
        let mut rstar_objs = vec![];
        let mut offset = 0;
        let database = self.get_arango_db().await?;
        loop {
            //需要排除负实体
            let aql = AqlQuery::new(
                r#"
            with pdms_inst_infos
            FOR doc IN pdms_inst_infos
                LIMIT @offset, @batch_size
                filter doc.aabb != null
                RETURN [
                    doc._key,
                    doc.aabb,
                ]
        "#,
            )
            .bind_var("offset", offset)
            .bind_var("batch_size", 5000);
            offset += 5000;
            if let Ok(refno_aabbs) = database.aql_query::<(String, Aabb)>(aql).await {
                if refno_aabbs.is_empty() {
                    break;
                }
                for (refno_str, aabb) in refno_aabbs {
                    if aabb.extents().magnitude().is_finite() {
                        let refno = RefU64::from_str(&refno_str).unwrap();
                        rstar_objs.push(RStarBoundingBox::from_aabb(&aabb, refno));
                    }
                }
            } else {
                break;
            }
        }

        dbg!(offset);
        dbg!(rstar_objs.len());

        self.rtree = Some(AccelerationTree::load(rstar_objs));
        dbg!(self.rtree.as_ref().unwrap().size());

        let aql = AqlQuery::new(
            r#"
            with room_eles
            FOR doc IN room_eles
                filter doc.aabb != null
                return doc
                // RETURN [
                //     doc._key,
                //     doc.aabb,
                // ]
        "#,
        );
        let mut room_rstar_objs = vec![];
        if let Ok(room_eles) = database.aql_query::<RoomElement>(aql).await {
            for room_ele in room_eles {
                if room_ele.aabb.is_some() {
                    for panel in &room_ele.panels {
                        room_rstar_objs.push(RStarBoundingBox::from_aabb(&panel.aabb, panel.refno));
                        self.room_panel_info_map.insert(panel.refno, panel.clone());
                    }
                }
                self.room_info_map.insert(room_ele.refno, room_ele);
            }
            self.room_panels_rtree = Some(AccelerationTree::load(room_rstar_objs));
            dbg!(self.room_panels_rtree.as_ref().unwrap().size());
        }

        Ok(true)
    }

    ///计算房间数据
    async fn calculate_room(
        &self,
        info: &EleGeosInfo,
        inst_geos: &Vec<EleInstGeo>,
        rtree: &AccelerationTree,
    ) -> anyhow::Result<Vec<RefU64>> {
        let mut withing_room_items = vec![];
        let room_refno = info.refno;
        let database = self.get_arango_db().await?;
        if let Some(room_abb) = info.aabb {
            withing_room_items = rtree
                .locate_intersecting_bounds(&room_abb)
                .collect::<Vec<_>>();

            let hashes = inst_geos.iter().map(|x| x.geo_hash).collect::<Vec<_>>();
            let room_mesh_mgr = query_pdms_mesh_aql(&database, hashes.iter())
                .await
                .unwrap_or_default();
            for (&hash, geo) in hashes.iter().zip(inst_geos) {
                if let Some(room_mesh) = room_mesh_mgr.get_mesh(hash) {
                    let t = info.get_geo_world_transform(geo);
                    let collider_mesh = room_mesh.get_tri_mesh(t.compute_matrix());
                    let mut outer_refnos = vec![];
                    //需要批量去获取数据

                    for (refno, aabb) in &withing_room_items {
                        //检查目标的坐标点不在它自身包围盒的情况，这种就需要用相交的算法去计算
                        //check 是否包含在房间内
                        let contain_point = match collider_mesh.cast_local_ray_and_get_normal(
                            &Ray::new(aabb.center(), Vector::new(0.0, 0.0, 1.0)),
                            100000.0,
                            false,
                        ) {
                            Some(intersection) => collider_mesh.is_backface(intersection.feature),
                            None => false,
                        };
                        if !contain_point {
                            outer_refnos.push(*refno);
                        }
                        //如果是风管，就需要这么去检测是否发生碰撞
                        //后续需要用包围盒再去判断一次
                        // collider_mesh.intersection_with_aabb();
                    }

                    //排除room的类型
                    withing_room_items
                        .retain(|(refno, _)| !outer_refnos.contains(refno) && *refno != room_refno);

                    // dbg!(&withing_room_refnos);
                }
            }
            //再次过滤room，通过判断位置是否在room的mesh里来判断
        }

        return Ok(withing_room_items.iter().map(|x| x.0).collect());
    }

    ///计算所有房间包含的其他参考号
    pub async fn calculate_rooms(&self) -> anyhow::Result<()> {
        let rtree = self
            .rtree
            .as_ref()
            .ok_or(anyhow::anyhow!("空间树未生成。"))?;
        let database = self.get_arango_db().await?;
        //指定哪个site下有房间节点
        let Some(room_root_refnos) = &self.db_option.room_root_refnos else {
            return Ok(());
        };

        let mut room_eles_map: HashMap<RefU64, (Aabb, Vec<RefU64>)> = HashMap::new();
        let mut room_panels_map: HashMap<RefU64, Vec<RoomPanelElement>> = HashMap::new();
        for r in room_root_refnos {
            let Ok(room_root_refno) = RefU64::from_str(r) else {
                continue;
            };
            let room_panels =
                query_deep_children_refnos_fuzzy(&database, &[room_root_refno], &["PANE"]).await?;
            //以panel的owner为房间的参考号
            println!("房间下的panel数量为: {}", room_panels.len());
            let inst_data = query_insts_shape_data(
                &database,
                &room_panels,
                Some(&[GeoBasicType::Pos, GeoBasicType::Compound]),
            )
            .await?;
            for (panel_refno, info) in &inst_data.inst_info_map {
                let Some(inst_geos) = inst_data.get_inst_geos(info) else {
                    continue;
                };
                let Some(aabb) = info.aabb else {
                    continue;
                };
                let r = self.calculate_room(info, inst_geos, rtree).await?;
                let room_refno = self.get_owner(info.refno);
                let room_panel_ele = RoomPanelElement {
                    refno: *panel_refno,
                    aabb,
                    inst_geo: inst_geos.first().cloned().unwrap_or_default(),
                    transform: info.world_transform,
                };
                if let Some((room_aabb, refnos)) = room_eles_map.get_mut(&room_refno) {
                    room_aabb.merge(&aabb);
                    refnos.extend_from_slice(&r);
                    room_panels_map
                        .get_mut(&room_refno)
                        .unwrap()
                        .push(room_panel_ele);
                } else {
                    room_eles_map.insert(room_refno, (aabb, r));
                    room_panels_map.insert(room_refno, vec![room_panel_ele]);
                }
            }
            println!("房间内元件的数量为：{}", room_eles_map.len());
        }

        self.save_room_info_to_arangodb(room_eles_map, room_panels_map)
            .await?;
        Ok(())
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

    ///获取图数据库的连接pool
    #[inline]
    pub async fn get_arango_db(&self) -> anyhow::Result<ArDatabase> {
        Ok(self
            .arango_pool
            .get()
            .await?
            .db(&self.db_option.arangodb_database)
            .await?)
    }

    ///获得默认的pool
    #[inline]
    pub async fn get_default_pool(conn_str: &str) -> anyhow::Result<Pool<MySql>> {
        MySqlPool::connect(conn_str)
            .await
            .map_err(|x| anyhow::anyhow!(x.to_string()))
    }

    /// 初始化mdb
    pub async fn init_mdb(&mut self, project: &str, mdb: &str, module: &str) -> anyhow::Result<()> {
        // let project_pool = self
        //     .get_project_pool(project)
        //     .ok_or(anyhow::anyhow!("Unknown project pool"))?;
        println!("正在初始化mdb: {mdb}");
        // let mut conn = project_pool;
        let need_sync_refno_basic = self.db_option.need_sync_refno_basic;
        if need_sync_refno_basic {
            for project in &self.db_option.included_projects {
                if let Some(kv) = self.project_map.get(project) {
                    sync_refno_basic_map(kv.value()).await.unwrap();
                }
                // if let Some(att_db) = self.local_attr_db_map.get(project).map(|x| x.value().clone()){
                //     sync_local_refno_basic_map();
                // }
            }
        }
        //todo 调整tidb，暂时不启用
        // if self.db_option.reset_mdb_project.unwrap_or(false) {
        // let create_sql = gen_create_project_mdb_sql();
        // let _ = conn.execute(create_sql.as_str()).await;
        // println!("正在插入mdb数据");
        // let _ = self
        //     .insert_project_mdb(&project_pool, &self.info_pool)
        //     .await;
        // }
        // cache_mdb_site_map(mdb, module, &project_pool).await;
        // self.mdb_dbnums = query_mdb_all_dbnums(mdb, &project_pool).await?;
        if need_sync_refno_basic {
            CACHED_REFNO_BASIC_MAP
                .save_to_file(stringify!(CACHED_REFNO_BASIC_MAP))
                .expect("CACHED_REFNO_BASIC_MAP 保存文件失败。");
        } else {
            println!("正在加载 CACHED_REFNO_BASIC_MAP");
            CACHED_REFNO_BASIC_MAP
                .load_map_from_file(stringify!(CACHED_REFNO_BASIC_MAP))
                .expect("CACHED_REFNO_BASIC_MAP 文件不存在。");
        }
        println!("加载 CACHED_REFNO_BASIC_MAP 成功");

        Ok(())
    }

    ///初始化db manager
    pub async fn init(db_option: &DbOption) -> anyhow::Result<Self> {
        let dir = db_option.project_path.to_string();
        let mut project_map = DashMap::new();
        let db_option = get_db_option().clone();
        let default_conn = AiosDBManager::get_default_conn_str(&db_option);
        for project in &db_option.included_projects {
            if db_option.use_tidb.unwrap_or(false) {
                let project_pool = AiosDBManager::get_db_pool(&default_conn, project).await;
                match project_pool {
                    Ok(pool) => {
                        println!("数据库连接成功 {project}");
                        project_map.entry(project.clone()).or_insert(pool.clone());
                    }
                    Err(_) => {
                        println!("项目: {} 连接创建失败", project);
                    }
                }
                println!("正在创建数据库连接 {project}");
            }
        }
        let projects = db_option.included_projects.clone();
        println!("正在创建图数据库连接");
        let arango_pool = connect_arangodb(&db_option).await?;

        let db_paths =
            collect_db_dirs(&db_option.project_path, projects.iter().map(|x| x.as_ref()));
        dbg!(&db_paths);
        let mut watcher = PdmsWatcher::load_from_json(None).unwrap_or(PdmsWatcher::new(db_paths));

        let mut mqtt_inst = new_mqtt_inst(&format!(
            "{}-{}-pub",
            db_option.location.as_str(),
            db_option.project_code
        ));
        let mqtt_client = Arc::new(mqtt_inst.client);
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
                            info!("Connected to MQTT broker.");
                            // let _ = connection_notify_tx.send(());
                        }
                        _ => {}
                    },
                    Err(e) => {
                        // error!("MQTT Connection error encountered: {}", e);
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        });
        Ok(Self {
            project_map,
            projects,
            needed_parse_files: None,
            project_path: dir,
            db_option,
            cached_mesh_mgr: Arc::new(Default::default()),
            arango_pool,
            watcher: Arc::new(watcher),
            mqtt_client,
            rtree: None,
            room_panels_rtree: None,
            room_info_map: Default::default(),
            room_panel_info_map: Default::default(),
            plin_params_map: Default::default(),
        })
    }

    /// 根据project获取连接池
    #[inline]
    pub fn get_project_pool(&self, project: &str) -> Option<Pool<MySql>> {
        self.project_map.get(project).map(|x| x.value().clone())
    }

    /// 根据project获取连接池
    #[inline]
    pub fn get_cur_project_pool(&self) -> Option<Pool<MySql>> {
        self.project_map
            .get(self.get_cur_project())
            .map(|x| x.value().clone())
    }

    ///获得project 的db
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

    /// 获得dbnum 对应的 dbtype 和 world refno
    pub async fn query_quick_info_by_dbno(
        &self,
        db_refno: RefU64,
        db_num: i32,
        pool: &Pool<MySql>,
    ) -> anyhow::Result<Option<DbQuickInfo>> {
        let mut sql = String::new();
        //todo 参考号相同的情况，导致refno获取出来的不准
        sql.push_str(&format!(
            r#"SELECT DB_TYPE, PROJECT  FROM {PDMS_DBNO_INFOS_TABLE} WHERE NUMBDB = {}"#,
            db_num
        ));
        let result = sqlx::query(&sql).fetch_all(pool).await?;
        for v in result {
            if let project = v.get::<String, _>(1) {
                dbg!(&project);
                let Some(project_pool) = self.get_project_pool(&project) else {
                    continue;
                };
                if let Some(world_refno) = query_world_refno_by_dbno(db_num, &project_pool).await? {
                    let db_type = v.get::<String, _>(0);
                    return Ok(Some(DbQuickInfo {
                        refno: db_refno,
                        world_refno,
                        db_num,
                        db_type,
                        project,
                        order_number: 0,
                    }));
                }
            }
        }
        Ok(None)
    }

    /// 获得mdb下所有的world的参考号
    pub async fn query_mdb_quickinfo_map(
        &self,
        project_pool: &Pool<MySql>,
        info_pool: &Pool<MySql>,
    ) -> anyhow::Result<MdbQuickInfoMap> {
        let mut mdb_map = HashMap::new();
        let mdbs = query_types_refnos(&vec!["MDB"], project_pool, &[]).await?;
        dbg!(&mdbs);
        for mdb_refno in mdbs {
            let Ok(mdb_attr) = aios_core::get_named_attmap(mdb_refno).await else {
                continue;
            };
            let mdb_name = mdb_attr.get_name_or_default();
            if let Some(dbs) = mdb_attr.get_refu64_vec("CURD") {
                // dbg!(&dbs);
                let mut map = HashMap::new();
                for (i, db_refno) in dbs.iter().enumerate() {
                    let att = aios_core::get_named_attmap(*db_refno)
                        .await
                        .unwrap_or_default();
                    let Some(db_num) = att.get_i32("NUMBDB") else {
                        continue;
                    };
                    dbg!(&db_num);
                    if let Ok(Some(mut quick_info)) = self
                        .query_quick_info_by_dbno(*db_refno, db_num, info_pool)
                        .await
                    {
                        quick_info.order_number = i as _;
                        map.entry(quick_info.db_type.clone())
                            .or_insert_with(Vec::new)
                            .push(quick_info);
                    }
                }
                mdb_map.entry(mdb_name).or_insert(map);
            }
        }
        Ok(mdb_map)
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

    /// save project mdb info to database
    pub async fn insert_project_mdb(
        &self,
        project_pool: &Pool<MySql>,
        info_pool: &Pool<MySql>,
    ) -> anyhow::Result<()> {
        //直接保存到图数据库，不要放在tidb里了
        let mdbs = self
            .query_ele_nodes_by_expression(r#"v.noun == "MDB""#)
            .await
            .unwrap();
        //直接在这里把mdb的信息加进去，创建这个节点
        let mut mdb_edges_map = IndexMap::new();
        let mut mdb_dbnums_map = IndexMap::new();
        let mut mdb_names_map = IndexMap::new();

        for mdb in &mdbs {
            let mdb_refno = mdb.refno;
            let Ok(mdb_attr) = aios_core::get_named_attmap(mdb_refno).await else {
                continue;
            };
            let name = mdb_attr.get_name_or_default();
            if let Some(dbs) = mdb_attr.get_refu64_vec("CURD") {
                for (i, db_refno) in dbs.into_iter().enumerate() {
                    let att = aios_core::get_named_attmap(db_refno)
                        .await
                        .unwrap_or_default();
                    let Some(db_num) = att.get_i32("NUMBDB") else {
                        continue;
                    };
                    let stype = att.get_i32("STYP").unwrap_or_default();
                    let db_type = Self::match_stype(stype);
                    let key = mdb_refno.hash_with_another_refno(db_refno).to_string();
                    let mdb_edge = PdmsMdbEdge {
                        key,
                        mdb_refno,
                        world_refno: Default::default(),
                        name: name.clone(),
                        order: i as _,
                        db_num: db_num as _,
                        db_refno,
                        db_type,
                    };
                    mdb_edges_map.insert(db_num, mdb_edge);
                    mdb_dbnums_map
                        .entry(mdb_refno)
                        .or_insert(Vec::new())
                        .push(db_num);
                }
            }
            mdb_names_map.entry(mdb_refno).or_insert(name);
        }

        dbg!(&mdb_names_map);

        let vec_str = mdb_edges_map.values().map(|x| x.db_num).join(",");
        let string = format!("v.dbnum in [{}] and v.noun== 'WORL'", vec_str);

        let mut pdms_edges = vec![];
        dbg!(&string);
        dbg!("hello");
        if let Ok(mut ele_nodes) = self.query_ele_nodes_by_expression(&string).await {
            if ele_nodes.is_empty() {
                return Ok(());
            }
            let database = self.get_arango_db().await?;

            for (k, (mdb_refno, dbnums)) in mdb_dbnums_map.into_iter().enumerate() {
                if dbnums.is_empty() {
                    continue;
                }
                let root_dbnum = dbnums[0];
                dbg!(root_dbnum);
                if !mdb_edges_map.contains_key(&root_dbnum) {
                    continue;
                }
                let Some(root_world) = ele_nodes.iter().find(|x| x.dbnum == root_dbnum) else {
                    continue;
                };

                //将mdb的关系也放入edges
                if let Some(mdb_data) = mdb_edges_map.get(&root_dbnum) {
                    let mdb_name = mdb_names_map.get(&mdb_refno).unwrap();
                    let edge = PdmsEleEdge {
                        key: root_world
                            .refno
                            .hash_with_another_refno(mdb_refno)
                            .to_string(),
                        refno: root_world.refno,
                        owner: mdb_refno,
                        order: k as _,
                        mdb_name: Some(mdb_name.clone()),
                        db_type: Some(mdb_data.db_type.clone()),
                    };
                    pdms_edges.push(edge);
                }
                // let children = aios_core::get_children_refnos(root_world.refno).unwrap_or_default();
                let mut order = 0;
                for dbnum in dbnums {
                    let Some(world) = ele_nodes.iter().find(|x| x.dbnum == dbnum) else {
                        continue;
                    };
                    mdb_edges_map
                        .entry(dbnum)
                        .and_modify(|x| x.world_refno = world.refno);
                    let site_refnos = aios_core::get_children_refnos(world.refno)
                        .await
                        .unwrap_or_default();
                    let Some(mdb_data) = mdb_edges_map.get(&dbnum) else {
                        continue;
                    };
                    //将site 和 第一个的 world 连在一起，而不是连world
                    for site_refno in site_refnos.into_iter() {
                        {
                            let edge = PdmsEleEdge {
                                key: site_refno
                                    .hash_with_another_refno(root_world.refno)
                                    .to_string(),
                                refno: site_refno,
                                owner: root_world.refno,
                                order: order as _,
                                mdb_name: None,
                                db_type: Some(mdb_data.db_type.clone()),
                            };
                            pdms_edges.push(edge);
                        }
                        order += 1;
                    }
                }
            }

            for result in mdb_edges_map
                .values()
                .into_iter()
                .collect::<Vec<_>>()
                .chunks(ARANGODB_SAVE_AMOUNT)
            {
                let json = serde_json::to_value(result)?;
                save_arangodb_with_db_option(&database, json, AQL_PDMS_MDBS_EDGES_COLLECTION)
                    .await?;
            }
            for edge in pdms_edges.chunks(ARANGODB_SAVE_AMOUNT) {
                let json = serde_json::to_value(edge)?;
                save_arangodb_with_db_option(&database, json, AQL_PDMS_EDGES_COLLECTION)
                    .await
                    .unwrap();
            }
        };

        Ok(())
    }

    //todo 获取类型直接采用edge上的查询
    ///获得参考号对应的一般类型
    pub async fn get_generic_type(&self, refno: RefU64) -> PdmsGenericType {
        let mut cur_refno = refno;
        while let Ok(b) = aios_core::get_named_attmap(cur_refno).await {
            if b.is_empty() {
                break;
            }
            let type_name = b.get_type_str();
            if PDMS_GNERAL_TYPE_NAMES_MAP.contains_key(&type_name) {
                return *PDMS_GNERAL_TYPE_NAMES_MAP.get(type_name).unwrap();
            }
            cur_refno = b.get_owner();
        }
        PdmsGenericType::UNKOWN
    }

    /// 通用的解析表达式的方法, 解析desi参考号下的 表达式值
    /// 如果 desi_refno 为空，代表design的数据不需要参与计算
    pub async fn resolve_expression_to_f32(
        &self,
        expr: &str,
        desi_refno: RefU64,
    ) -> anyhow::Result<f32> {
        let context = self.get_or_create_cata_context(desi_refno, None).await?;
        eval_str_to_f32(expr, &context, Some(self), "DIST")
    }

    ///查询单个element
    pub async fn query_element(&self, refno: RefU64) -> anyhow::Result<Option<PdmsEleData>> {
        let arango_db = self.get_arango_db().await?;
        let id = refno.format_url_name(AQL_PDMS_ELES_COLLECTION);
        let aql = AqlQuery::new(
            r#"
            with pdms_eles
                return document(pdms_eles, @id)"#,
        )
        .bind_var("id", id);
        let mut r = arango_db
            .aql_query::<PdmsEleData>(aql)
            .await
            .unwrap_or_default();
        Ok(r.pop())
    }

    /// 获取缓存好的site
    pub async fn get_cached_site_nodes(
        &self,
        world_refno: RefU64,
    ) -> anyhow::Result<Option<Vec<PdmsElement>>> {
        if let Some(k) = CACHED_MDB_SITE_MAP.read().await.get(&world_refno) {
            return Ok(Some(k.0.clone()));
        }
        Ok(None)
    }

    ///获得当前mdb下的site参考号
    pub async fn get_site_refnos(&self) -> anyhow::Result<Vec<RefU64>> {
        let world_refno = self.get_desi_world().await?.refno;
        let r = self
            .get_cached_site_nodes(world_refno)
            .await?
            .unwrap_or_default()
            .iter()
            .map(|x| x.refno)
            .collect();
        Ok(r)
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
