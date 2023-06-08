use glam::Vec3;
use once_cell::sync::Lazy;
use smol_str::SmolStr;
use arangors::AqlQuery;
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use aios_core::pdms_types::*;
use aios_core::accel_tree::acceleration_tree::{AccelerationTree, RStarBoundingBox};
use parry3d::query::{Ray, RayCast};
use parry3d::math::{Isometry, Vector};
use anyhow::anyhow;
use std::collections::{HashMap, HashSet};
use aios_core::options::DbOption;
use sqlx::{Executor, MySql, MySqlPool, Pool, Row};
use sqlx::pool::PoolOptions;
use std::time::{Duration, Instant};
use log::{error, info};
use dashmap::DashMap;
use std::sync::{Arc, Mutex};
use aios_core::tool::db_tool::{db1_dehash, db1_hash, GLOBAL_UDA_NAME_MAP};
use tokio::sync::{mpsc, RwLock};
use aios_core::pdms_data::ScomInfo;
use aios_core::parsed_data::CateGeomsInfo;
use aios_core::prim_geo::category::convert_to_brep_shapes;
use aios_core::prim_geo::tubing::{PdmsTubing, TubiEdge};
use aios_core::parsed_data::geo_params_data::CateGeoParam::TubeImplied;
use std::default::default;
use bevy::prelude::Transform;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use std::mem::take;
use aios_core::prim_geo::cylinder::SCylinder;
use aios_core::prim_geo::TUBI_GEO_HASH;
use tokio_stream::wrappers::UnboundedReceiverStream;
use approx::abs_diff_eq;
use opencascade::{DsShape, OCCShape};
use aios_core::shape::pdms_shape::{BrepShapeTrait, PlantMesh, VerifiedShape};
use futures::StreamExt;
use nalgebra::Point3;
use rayon::prelude::*;
use crate::api::attr::{query_attr, query_uda_ukey_udna_all};
use crate::api::children::{cache_mdb_module_numbdbs, cache_mdb_site_map, query_mdb_all_dbnums};
use crate::api::element::{check_exist_refno, DbQuickInfo, MdbQuickInfoMap, query_name, query_types_refnos, query_world_refno_by_dbno};
use crate::api::project_mdb::{gen_insert_project_mdb_sql, query_db_nums_of_mdb};
use crate::api::refno_info::{cache_plin_plax, get_ref0_projects, sync_refno_basic_map};
use crate::aql_api::children::{query_children_order_aql, query_deep_children_refnos_fuzzy};
use crate::aql_api::foreign_refnos::query_foreign_refnos_fuzzy;
use crate::aql_api::pdms_mesh::query_pdms_mesh_aql;
use crate::cata::query_cata::resolve_desi_comp;
use crate::cata::resolve::CataExprContext;
use crate::cata::resolve_helper::eval_str_to_f32;
use crate::cata::sctn::geo::create_profile_geos;
use crate::consts::PDMS_INFO_DB;
use crate::data_interface::db_manager::GeoEnum;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::structs::{AIOSAxisMap, CateBrepShapeMap};
use crate::data_interface::tidb_manager::{AiosDBManager, CATAEXPRCONTEXT_MAP};
use crate::defines::{CACHED_MDB_SITE_MAP, CACHED_PLIN_MAP, CACHED_REFNO_BASIC_MAP};
use crate::graph_db::pdms_arango::{ArDatabase, connect_arangodb};
use crate::graph_db::pdms_inst_arango::{query_insts_shape_data, save_instance_to_graph_db};
use crate::graph_db::pdms_mesh_arango::save_mesh_to_arango_db;
use crate::tables::gen_create_project_mdb_sql;
use crate::consts::PDMS_DBNO_INFOS_TABLE;
use crate::consts::AQL_PDMS_ELES_COLLECTION;
use crate::data_interface::gen_model::{cache_cata_geos, cache_loop_geos, cache_prim_geos};

pub const TUBI_TOL: f32 = 10.0f32;

static PDMS_GNERAL_TYPE_NAMES_MAP: Lazy<HashMap<&'static str, PdmsGenericType>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert("EQUI", PdmsGenericType::EQUI);
    m.insert("PIPE", PdmsGenericType::PIPE);
    m.insert("ROOM", PdmsGenericType::ROOM);
    m.insert("STRU", PdmsGenericType::STRU);
    m.insert("PANE", PdmsGenericType::PANE);
    m.insert("CFLOOR", PdmsGenericType::CFLOOR);
    m.insert("FLOOR", PdmsGenericType::FLOOR);
    m.insert("EXTR", PdmsGenericType::EXTR);
    m.insert("REVO", PdmsGenericType::REVO);
    m
});

static GENRIC_NOUN_NAMES: Lazy<Vec<SmolStr>> = Lazy::new(|| {
    vec![
        "EQUI".into(),
        "PIPE".into(),
        "STRU".into(),
        "ROOM".into(),
        "STWALL".into(),
        "FLOOR".into(),
    ]
});

impl AiosDBManager {
    /// 从默认配置文件初始化
    pub async fn init_form_config() -> anyhow::Result<Self> {
        let db_option = Self::get_db_option()?;
        let mut mgr = Self::init(&db_option).await?;
        dbg!("正在初始化uda");
        mgr.init_uda_map().await?;
        mgr.init_mdb(
            &db_option.project_name,
            &db_option.mdb_name,
            &db_option.module,
        ).await?;
        if db_option.gen_spatial_tree {
            mgr.compute_aabb_tree().await?;
        }
        Ok(mgr)
    }

    pub async fn compute_aabb_tree(&mut self) -> anyhow::Result<bool> {
        //测试分页查询
        let mut rstar_objs = vec![];
        let mut offset = 0;
        let database = self.get_arango_db().await?;
        loop {
            //需要排除负实体
            let aql = AqlQuery::builder().query(r#"
            FOR doc IN pdms_inst_infos
                SORT doc._key
                LIMIT @offset, @batch_size
                filter doc.aabb != null
                filter LENGTH(doc.geo_insts) > 1 or (LENGTH(doc.geo_insts) == 1 and !doc.geo_insts[0].is_neg)
                RETURN [
                    doc._key,
                    doc.aabb,
                ]
        "#)
                .bind_var("offset", offset)
                .bind_var("batch_size", 5000)
                .build();
            offset += 5000;
            if let Ok(refno_aabbs) = database.aql_query::<(String, Aabb)>(aql).await {
                if refno_aabbs.is_empty() {
                    break;
                }
                for (refno_str, aabb) in refno_aabbs {
                    if aabb.extents().magnitude().is_finite() {
                        let refno = RefU64::from_url_refno(&refno_str).unwrap();
                        rstar_objs.push(RStarBoundingBox::from_aabb(&aabb, refno));
                    }
                }
            } else {
                break;
            }
        }

        dbg!(offset);

        self.rtree = Some(AccelerationTree::load(rstar_objs));
        dbg!(self.rtree.as_ref().unwrap().size());

        Ok(true)
    }

    ///计算房间数据
    async fn calculate_room(&self, inst: &EleGeosInfo, inst_geo: &EleInstGeo, rtree: &AccelerationTree) -> anyhow::Result<Vec<RefU64>> {
        // let mut withing_room_items = vec![];
        // let room_refno = inst.refno;
        // let database = self.get_arango_db().await?;
        // if let Some(room_abb) = inst.aabb {
        //     // dbg!(&room_abb);
        //     withing_room_items = rtree
        //         .locate_intersecting_bounds(&room_abb)
        //         .collect::<Vec<_>>();
        //     let hashes = inst.geo_basics.iter().map(|x| x.geo_hash).collect::<Vec<_>>();
        //     let room_mesh_mgr = query_pdms_mesh_aql(&database, &hashes).await.unwrap_or_default();
        //     for hash in hashes {
        //         if let Some(room_mesh) = room_mesh_mgr.get_mesh(hash) {
        //             let t = inst.get_geo_world_transform(inst_geo);
        //             // dbg!(&t);
        //             let collider_mesh = room_mesh.get_tri_mesh(t.compute_matrix());
        //             // let local_aabb = collider_mesh.local_aabb();
        //             // dbg!(collider_mesh.local_aabb());
        //             let mut outer_refnos = vec![];
        //             //需要批量去获取数据
        //
        //             for (refno, world_point) in &withing_room_items {
        //                 // let world_trans = self.get_world_transform(*refno).await?.unwrap_or_default();
        //                 // let world_point: parry3d::math::Point<f32> = world_trans.translation.into();
        //
        //                 //检查目标的坐标点不在它自身包围盒的情况，这种就需要用相交的算法去计算
        //
        //                 //check 是否包含在房间内
        //                 let contain_point = match collider_mesh.cast_local_ray_and_get_normal(
        //                     &Ray::new(Point3::from_slice(world_point), Vector::new(0.0, 0.0, 1.0)),
        //                     100000.0,
        //                     false,
        //                 ) {
        //                     Some(intersection) => {
        //                         collider_mesh.is_backface(intersection.feature)
        //                     }
        //                     None => false,
        //                 };
        //                 // dbg!(contain_point);
        //                 // dbg!(outer_refnos.len());
        //                 if !contain_point {
        //                     outer_refnos.push(*refno);
        //                 }
        //                 //如果是风管，就需要这么去检测是否发生碰撞
        //                 //后续需要用包围盒再去判断一次
        //                 // collider_mesh.intersection_with_aabb();
        //             }
        //
        //             withing_room_items.retain(|(refno, _)| {
        //                 !outer_refnos.contains(refno) && *refno != room_refno
        //             });
        //
        //             // dbg!(&withing_room_refnos);
        //         }
        //     }
        //     //再次过滤room，通过判断位置是否在room的mesh里来判断
        // }
        //
        // return Ok(withing_room_items.iter().map(|x| x.0).collect());

        return Ok(vec![]);
    }

    ///计算所有房间包含的其他参考号
    pub async fn calculate_rooms(&self) -> anyhow::Result<()> {
        let rtree = self.rtree.as_ref().ok_or(anyhow!("空间树未生成。"))?;
        let database = self.get_arango_db().await?;
        //指定哪个site下有房间节点
        let Some(room_root_refnos) = &self.db_option.room_root_refnos else {
            return Ok(());
        };

        let mut room_hashmap = HashMap::new();
        for r in room_root_refnos {
            let Ok(room_root_refno) = RefU64::from_refno_str(r) else {
                continue;
            };
            let panes = query_deep_children_refnos_fuzzy(&database, room_root_refno, &["PANE"]).await?;
            // dbg!(&panes);
            println!("房间下的panel数量为: {}", panes.len());
            let inst_data = query_insts_shape_data(&database, &panes).await?;
            // dbg!(&instances);
            let mut final_within_room_refnos = vec![];
            for (_, info) in &inst_data.inst_info_map {
                //todo 需要使用图数据库来处理
                // let Some(Some(inst_geo)) = inst_data.get_inst_geo(info).into_iter().next() else{
                //     continue;
                // };
                // let r = self.calculate_room(info, inst_geo, rtree).await?;
                // final_within_room_refnos.extend_from_slice(&r);
            }

            // dbg!(&final_within_room_refnos);
            println!("房间内元件的数量为：{}", final_within_room_refnos.len());
            room_hashmap.insert(room_root_refno, final_within_room_refnos);
        }

        self.save_room_info_to_arangodb(room_hashmap).await?;


        Ok(())
    }


    ///快速获得table名称
    pub fn get_table_name(&self, refno: RefU64) -> String {
        CACHED_REFNO_BASIC_MAP
            .get(&refno)
            .map(|x| x.get_table_name().to_string())
            .unwrap_or("UNSET".to_string())
    }


    ///获得db option
    #[inline]
    pub fn get_db_option() -> anyhow::Result<DbOption> {
        use config::{Config, ConfigError, Environment, File};
        let s = Config::builder()
            .add_source(File::with_name("DbOption"))
            .build()?;
        s.try_deserialize::<DbOption>()
            .map_err(|x| anyhow!(x.to_string()))
    }

    ///获得默认的连接字符串
    #[inline]
    pub fn get_default_conn_str(d: &DbOption) -> String {
        let user = d.user.as_str();
        let pwd = d.password.as_str();
        let ip = d.ip.as_str();
        let port = d.port.as_str();
        format!("mysql://{user}:{pwd}@{ip}:{port}")
    }

    ///获得默认的连接字符串
    #[inline]
    pub fn default_conn_str(&self) -> String {
        let d = &self.db_option;
        let user = d.user.as_str();
        let pwd = d.password.as_str();
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
            .map_err({ |x| anyhow!(x.to_string()) })
    }

    #[inline]
    pub async fn get_arango_db(&self) -> anyhow::Result<ArDatabase> {
        Ok(self.arango_pool.get().await?.db(&self.db_option.arangodb_database).await?)
    }


    ///获得默认的pool
    #[inline]
    pub async fn get_default_pool(conn_str: &str) -> anyhow::Result<Pool<MySql>> {
        MySqlPool::connect(conn_str)
            .await
            .map_err(|x| anyhow!(x.to_string()))
    }


    /// 初始化mdb
    pub async fn init_mdb(&mut self, project: &str, mdb: &str, module: &str) -> anyhow::Result<()> {
        let project_pool = self.get_project_pool(project).ok_or(anyhow!("Unknown project pool"))?;
        info!("正在初始化mdb: {mdb}");
        let mut conn = project_pool.acquire().await?;
        let time = Instant::now();
        let need_sync_refno_basic = self.db_option.need_sync_refno_basic;
        if need_sync_refno_basic {
            for project in &self.db_option.included_projects {
                if let Some(kv) = self.project_map.get(project) {
                    sync_refno_basic_map(kv.value()).await.unwrap();
                }
            }
        }
        // 将对应mdb module 下所有的 numbdb 存下来
        //创建table, 如果已经存在，可以忽略
        if self.db_option.reset_mdb_project.unwrap_or(false) {
            let create_sql = gen_create_project_mdb_sql();
            let _ = conn.execute(create_sql.as_str()).await;
            println!("正在插入mdb数据");
            let _ = self.insert_project_mdb(&project_pool, &self.info_pool).await;
        }
        cache_mdb_site_map(mdb, module, &project_pool).await;
        self.mdb_dbnums = query_mdb_all_dbnums(mdb, &project_pool).await?;
        let database = self.get_arango_db().await?;
        if need_sync_refno_basic {
            for project in &self.db_option.included_projects {
                if let Some(kv) = self.project_map.get(project) {
                    let dbnums = self.mdb_dbnums.iter().cloned().collect::<Vec<_>>();
                    if let Ok(m) = cache_plin_plax(
                        kv.value(),
                        &dbnums,
                        &database,
                    ).await {
                        for (k, v) in m {
                            CACHED_PLIN_MAP.insert(k, &v.into());
                        }
                    }
                }
            }
        }
        if need_sync_refno_basic {
            CACHED_REFNO_BASIC_MAP.save_to_file(stringify!(CACHED_REFNO_BASIC_MAP))?;
            CACHED_PLIN_MAP.save_to_file(stringify!(CACHED_PLIN_MAP))?;
        } else {
            CACHED_REFNO_BASIC_MAP.load_map_from_file(stringify!(CACHED_REFNO_BASIC_MAP))?;
            CACHED_PLIN_MAP.load_map_from_file(stringify!(CACHED_PLIN_MAP))?;
        }

        // 将 mdb对应的 module 下的所有 numbdb保存下来
        let results = cache_mdb_module_numbdbs(mdb, module, &project_pool).await?;
        for r in results {
            self.cache_module_numbdbs.insert(r);
        }
        Ok(())
    }

    ///初始化db manager
    pub async fn init(db_option: &DbOption) -> anyhow::Result<Self> {
        let dir = db_option.project_path.to_string();
        let mut project_map = DashMap::new();
        let db_option = Self::get_db_option()?;
        let default_conn = AiosDBManager::get_default_conn_str(&db_option);
        for project in &db_option.included_projects {
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
        let info_conn = AiosDBManager::get_db_pool(
            &default_conn,
            &format!(
                "{}_{}",
                PDMS_INFO_DB,
                &db_option.project_name.to_uppercase()
            ),
        )
            .await?;
        let ref0_projects = get_ref0_projects(&info_conn).await?;
        // dbg!(&ref0_projects);
        let projects = db_option.included_projects.clone();
        println!("正在创建图数据库连接");
        let arango_pool = connect_arangodb(&db_option).await?;
        Ok(Self {
            project_map,
            ref0_projects,
            info_pool: info_conn,
            projects,
            needed_parse_files: None,
            project_path: dir,
            db_option,
            cached_mesh_mgr: Arc::new(Default::default()),
            arango_pool,
            cached_world_transforms_map: Arc::new(Default::default()),
            cache_module_numbdbs: Default::default(),
            mdb_dbnums: Default::default(),
            rtree: None,
        })
    }

    /// 初始化 uda_map
    pub async fn init_uda_map(&self) -> anyhow::Result<()> {
        for pool in &self.project_map {
            if let Ok(uda_map) = query_uda_ukey_udna_all(pool.value()).await {
                for (ukey, udna) in uda_map {
                    let udna = format!(":{}", udna);
                    GLOBAL_UDA_NAME_MAP.entry(ukey).or_insert(udna);
                }
            }
        }
        Ok(())
    }

    /// 根据project获取连接池
    #[inline]
    pub fn get_project_pool(&self, project: &str) -> Option<Pool<MySql>> {
        self.project_map.get(project).map(|x| x.value().clone())
    }

    ///获得project 的db
    #[inline]
    pub async fn get_project_pool_by_refno(&self, refno: RefU64) -> Option<(String, Pool<MySql>)> {
        if let Some(projects) = self.ref0_projects.get(&refno.get_0()) {
            ///只有一个的时候
            if projects.len() == 1 {
                let project = projects.value().iter().next().as_ref().unwrap().clone();
                if let Some(project_pool) = self.project_map.get(project) {
                    return Some((project.clone(), project_pool.value().clone()));
                }
            } else {
                for project in &self.db_option.included_projects {
                    if let Some(pool) = self.get_project_pool(project) {
                        if check_exist_refno(refno, &pool, &self.mdb_dbnums).await.ok()? {
                            return Some((project.clone(), pool.clone()));
                        }
                    }
                }
            }
        }
        (None)
    }

    /// 获得dbnum 对应的 dbtype 和 world refno
    pub async fn query_quick_info_by_dbno(&self, db_refno: RefU64, db_num: i32, pool: &Pool<MySql>) -> anyhow::Result<Option<DbQuickInfo>> {
        let mut sql = String::new();
        //todo 参考号相同的情况，导致refno获取出来的不准
        sql.push_str(&format!(r#"SELECT DB_TYPE, PROJECT  FROM {PDMS_DBNO_INFOS_TABLE} WHERE NUMBDB = {}"#, db_num));
        let result = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
        for v in result {
            if let project = v.get::<String, _>(1) {
                let project_pool = self.get_project_pool(&project).ok_or(anyhow!("Unknown project pool"))?;
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
        for mdb_refno in mdbs {
            // let Ok(mdb_attr) = query_attr(mdb_refno, self, None).await else {
            //     continue;
            // };
            let Ok(mdb_attr) = self.get_attr(mdb_refno).await else {
                continue;
            };
            let mdb_name = mdb_attr.get_name().to_string();
            // let Ok(mdb_name) = query_name(mdb_refno, &project_pool).await else {
            //     continue;
            // };
            // dbg!(&mdb_name);
            // dbg!(&mdb_attr);
            if let Some(dbs) = mdb_attr.get_refu64_vec("CURD") {
                let mut map = HashMap::new();
                for (i, db_refno) in dbs.iter().enumerate() {
                    if let Ok(att) = self.get_implicit_attr(*db_refno, Some(vec!["NUMBDB"])).await {
                        let db_num = att.get_i32("NUMBDB").unwrap_or_default();
                        // dbg!(&db_num);
                        if let Ok(Some(mut quick_info)) = self.query_quick_info_by_dbno(*db_refno, db_num, info_pool).await {
                            // dbg!(&quick_info.db_type);
                            quick_info.order_number = i as _;
                            map.entry(quick_info.db_type.clone())
                                .or_insert_with(Vec::new).push(quick_info);
                        }
                    }
                }
                mdb_map.entry(mdb_name).or_insert(map);
            }
        }
        Ok(mdb_map)
    }

    /// save project mdb info to database
    pub async fn insert_project_mdb(
        &self,
        project_pool: &Pool<MySql>,
        info_pool: &Pool<MySql>,
    ) -> anyhow::Result<()> {
        let project_mdb_map = self.query_mdb_quickinfo_map(project_pool, info_pool).await?;
        if !project_mdb_map.is_empty() {
            let sql = gen_insert_project_mdb_sql(&project_mdb_map);
            let mut conn = project_pool.acquire().await?;
            let result = conn.execute(sql.as_str()).await;
            match result {
                Ok(_) => {}
                Err(e) => {
                    dbg!(&e);
                    dbg!(sql.as_str());
                }
            }
        }
        Ok(())
    }

    ///获得参考号对应的一般类型
    pub fn get_generic_type(&self, refno: RefU64) -> PdmsGenericType {
        let mut cur_refno = refno;
        while let Some(b) = CACHED_REFNO_BASIC_MAP.get(&cur_refno) {
            let type_name = b.get_type();
            if PDMS_GNERAL_TYPE_NAMES_MAP.contains_key(&type_name) {
                return *PDMS_GNERAL_TYPE_NAMES_MAP.get(type_name).unwrap();
            }
            cur_refno = b.owner;
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
        let database = self.get_arango_db().await?;
        let cata_context = if let Some(cata) = CATAEXPRCONTEXT_MAP.get(&desi_refno) {
            cata.value().clone()
        } else {
            let cata = CataExprContext::create(desi_refno, &database)
                .await
                .unwrap_or_default()
                .unwrap_or_default();
            CATAEXPRCONTEXT_MAP.insert(desi_refno, cata.clone());
            cata
        };
        let context = cata_context.build(self, desi_refno).await;
        eval_str_to_f32(expr, &context, Some(self))
    }


    pub async fn cache_pohe_geos(mgr: Arc<AiosDBManager>, project: &str) -> anyhow::Result<bool> {
        let pohe_refnos = mgr
            .get_refnos_by_types(project, &vec!["POHE"], &[1])
            .await?;
        let pohe_cnt = pohe_refnos.len();
        dbg!(pohe_cnt);
        // let mut handles = vec![];
        // for (i, refno) in pohe_refnos.into_iter().enumerate() {
        //     let mgr = mgr.clone();
        //     let handle = tokio::spawn(async move {
        //         let inst_map = &mgr.mesh_mgr.inst_data;
        //         let cached_mesh_mgr = &mgr.mesh_mgr.cached_mesh_mgr;
        //         //在这里直接处理完所有需要处理的transform
        //         let transform = mgr.get_world_transform(refno).await.unwrap_or_default().unwrap_or_default();
        //         let mut geo_hash = None;
        //         let mut item_trans = TransformSRT::default();
        //         let mut facet = Facet::default();
        //         if let Ok(children_refs) = mgr.get_children_refs(refno).await {
        //             for pogo_ref in children_refs {
        //                 let mut vertices: Vec<[f32; 3]> = vec![];
        //                 let mut tv = vec![];
        //                 if let Ok(p_refs) = mgr.get_children_refs(pogo_ref).await {
        //                     let v_cnt = p_refs.len();
        //                     if v_cnt >= 3 {
        //                         for r in p_refs {
        //                             let att = mgr.get_attr(r).await.unwrap_or_default();
        //                             let v = att.get_position().unwrap_or_default();
        //                             vertices.push([v[0], v[1], v[2]]);
        //                             if tv.len() < 3 {
        //                                 tv.push(v);
        //                             }
        //                         }
        //                         let n = (tv[1] - tv[0]).cross(tv[2] - tv[1]).normalize();
        //                         let mut polygon = Polygon {
        //                             contours: vec![Contour {
        //                                 vertices,
        //                                 normals: vec![n.into(); v_cnt],
        //                             }]
        //                         };
        //                         facet.polygons.push(polygon);
        //                     }
        //                 }
        //             }
        //         }
        //         if facet.check_valid() {
        //             item_trans = facet.get_trans();
        //             let r = cached_mesh_mgr.get_pdms_mesh_hash_key(Box::new(facet));
        //             geo_hash = Some(r);
        //         }
        //
        //         let parent_refno = mgr.get_owner(refno);
        //         let mut parent_att = mgr.get_implicit_attr(parent_refno, Some(vec!["LEVE"])).await.unwrap_or_default();
        //         if let Some(geo_hash) = geo_hash {
        //             let visible = parent_att.is_visible_by_level(None).unwrap_or(true);
        //             let tr: TransformSRT = item_trans * transform;
        //             let mut bbox = cached_mesh_mgr.get_bbox(&geo_hash).unwrap();
        //             bbox.scaled(&tr.scale);
        //             let geom_data = EleGeoInstance {
        //                 geo_hash,
        //                 bbox,
        //                 global_transform: (tr.rotation, tr.translation, tr.scale),
        //                 visible,
        //                 generic_type: "STRU".to_string(),  //todo add generic type
        //                 zone_refno: refno,
        //             };
        //             inst_map.entry(parent_refno).or_insert(Vec::new()).push(geom_data);
        //         }
        //     });
        //     handles.push(handle);
        //     if i == pohe_cnt - 1 || handles.len() == 100 {
        //         futures::future::join_all(take(&mut handles)).await;
        //     }
        // }
        // println!("处理POHE几何体: {} 花费时间: {} ms", pohe_cnt, t.elapsed().as_millis());
        Ok(true)
    }


    // 需要区分project，不同project的mesh，是不同的
    pub async fn cache_geos_data(
        mut mgr: Arc<AiosDBManager>,
        db_option: DbOption,
    ) -> anyhow::Result<bool> {
        let time = Instant::now();
        let project = &db_option.project_name;
        let mdb = &db_option.mdb_name;
        let mut db_nos = db_option.manual_db_nums.clone().unwrap_or_default();

        if db_nos.is_empty() {
            let url = AiosDBManager::get_default_conn_str(&mgr.db_option);
            let pool = AiosDBManager::get_db_pool(&url, project).await?;
            db_nos = query_db_nums_of_mdb(mdb, &db_option.module, &pool).await?;
            db_nos.sort();
            info!("当前mdb的所有dbnos: {:?}", db_nos);
        }
        // std::fs::create_dir_all("./assets/mesh").unwrap();
        // std::fs::create_dir_all("./assets/instance").unwrap();

        let adb = mgr.get_arango_db().await?;

        dbg!(&db_nos);
        let scom_info_map: Arc<RwLock<HashMap<RefU64, ScomInfo>>> = Arc::new(RwLock::new(HashMap::new()));

        for db_no in db_nos {
            println!("开始处理db: {db_no}");
            let d_types = &db_option.debug_refno_types;
            let not_debug = db_option.debug_refno_types.is_empty();
            let mut run_cache_cata = d_types.iter().any(|x| x == "CATA");
            let mut run_cache_loop = d_types.iter().any(|x| x == "LOOP");
            let mut run_cache_prim = d_types.iter().any(|x| x == "PRIM");

            let mut shape_insts_data = ShapeInstancesData::default();
            let unit_cyli_aabb = Aabb::new(Point3::new(-0.5, -0.5, 0.0), Point3::new(0.5, 0.5, 1.0));
            shape_insts_data.insert_geos_data(TUBI_GEO_HASH, EleInstGeosData {
                inst_key: TUBI_GEO_HASH,
                refno: Default::default(),
                insts: vec![EleInstGeo {
                    geo_hash: TUBI_GEO_HASH,
                    refno: Default::default(),
                    geo_param: PdmsGeoParam::PrimSCylinder(SCylinder::default()),
                    pts: vec![],
                    aabb: Some(unit_cyli_aabb),
                    transform: Default::default(),
                    visible: true,
                    is_tubi: true,
                    geo_type: GeoBasicType::Pos,
                }],
                aabb: Some(unit_cyli_aabb),
                type_name: "TUBI".to_string(),
                ptset_map: Default::default(),
                flow_pt_indexs: vec![],
            });
            let instance_mgr = Arc::new(RwLock::new(shape_insts_data));

            let instance_mgr_clone = instance_mgr.clone();

            let db_option_clone = db_option.clone();
            let mgr_clone = mgr.clone();
            let mgr_clone_new = mgr.clone();

            let target_dbnos = [db_no];
            let root_refnos = mgr.get_gen_model_root_refnos(&target_dbnos).await?;
            dbg!(&root_refnos);
            if root_refnos.is_empty() {
                println!("输入的调试参考号或者db号不正确");
                continue;
            }

            //元件库的模型计算
            //求出有多少个是一样的模型
            let target_cata_refnos = mgr.get_gen_model_target_refnos(GeoEnum::CATA_ONLY_TUBI, &target_dbnos, false).await?;
            println!("使用管道元件库数量: {}", target_cata_refnos.len());
            //查询出branch 和 branch 下的子节点
            let mut branch_refnos_map = DashMap::new();
            let mut refno_lstube_map = DashMap::new();
            let mut lstube_bores_map = DashMap::new();
            let mut bran_comp_eles = vec![];
            for refno in target_cata_refnos {
                let children = query_children_order_aql(&adb, refno).await?;
                if children.is_empty() { continue; }
                bran_comp_eles.extend(children.iter().map(|x| x.refno));
                //求出元件对应的outside bore
                branch_refnos_map.insert(refno, children);
            }

            let lstube_refnos = mgr.query_foreign_refnos(&bran_comp_eles,
                                                         &[&["LSRO", "LSTU"]], &["CATR"],
                                                         &[], 2).await?;
            // dbg!(&bran_comp_eles);
            // dbg!(&lstube_refnos);
            for c in 0..bran_comp_eles.len() {
                refno_lstube_map.insert(bran_comp_eles[c], lstube_refnos[c]);
            }
            let lstube_set = lstube_refnos.into_iter()
                .collect::<HashSet<_>>()
                .into_iter();
            for l in lstube_set {
                let att = mgr.get_attr(l).await?;
                let params = att.get_f64_vec("PARA").unwrap_or_default();
                let gtype = att.get_as_string("GTYP").unwrap_or_default();
                if params.len() >= 2 {
                    // let type_name = db1_dehash(params[2] as u32);
                    // dbg!(type_name);
                    let bore = params[if gtype.as_str() == "TUBE" { 1 } else { 0 }] as f32;
                    lstube_bores_map.insert(l, bore);
                }
            }
            // dbg!(&lstube_bores_map);
            let target_bran_cata_map = mgr.get_gen_model_target_refnos_by_cata_hash(GeoEnum::CATA_ONLY_TUBI, &target_dbnos, true, false).await?;
            let target_single_cata_map = mgr.get_gen_model_target_refnos_by_cata_hash(GeoEnum::CATA, &target_dbnos, false, false).await?;
            // dbg!(&target_bran_cata_map);
            if run_cache_cata {
                let mut handles = vec![];
                if !target_bran_cata_map.is_empty() {
                    let scom_info_map_clone = scom_info_map.clone();
                    let mgr_clone = mgr.clone();
                    let instance_mgr_clone = instance_mgr.clone();
                    let db_option_clone = db_option.clone();
                    let handle = tokio::spawn(async move {
                        cache_cata_geos(
                            mgr_clone,
                            instance_mgr_clone,
                            scom_info_map_clone,
                            &db_option_clone,
                            Arc::new(target_bran_cata_map),
                            Arc::new(branch_refnos_map),
                            Arc::new(refno_lstube_map),
                            Arc::new(lstube_bores_map),
                        )
                            .await
                            .unwrap();
                    });
                    handles.push(handle);
                }

                if !target_single_cata_map.is_empty() {
                    let mgr_clone = mgr.clone();
                    let scom_info_map_clone = scom_info_map.clone();
                    let instance_mgr_clone = instance_mgr.clone();
                    let db_option_clone = db_option.clone();
                    let handle = tokio::spawn(async move {
                        cache_cata_geos(
                            mgr_clone,
                            instance_mgr_clone,
                            scom_info_map_clone,
                            &db_option_clone,
                            Arc::new(target_single_cata_map),
                            Arc::new(Default::default()),
                            Arc::new(Default::default()),
                            Arc::new(Default::default()),
                        )
                            .await
                            .unwrap();
                    });
                    handles.push(handle);
                }

                futures::future::join_all(handles).await;
                {
                    let mesh_mgr = mgr.cached_mesh_mgr.read().await;
                    let inst_data = instance_mgr.read().await;
                    println!("当前db下的元件库生成统计：");
                    dbg!(mesh_mgr.len());
                    dbg!(inst_data.inst_info_map.len());
                    // dbg!(&inst_data.inst_info_map);
                    dbg!(inst_data.inst_tubi_map.len());
                    save_instance_to_graph_db(&mgr, &inst_data).await?;
                    save_mesh_to_arango_db(&mgr, &mesh_mgr).await?;
                }
                mgr.cached_mesh_mgr.write().await.clear();
                instance_mgr.write().await.clear();
            }

            let mut has_geom_refnos = vec![];
            for root_refno in root_refnos.clone() {
                let refnos = mgr.query_refnos_has_geos(root_refno).await?;
                has_geom_refnos.extend_from_slice(&refnos);
            }
            dbg!(has_geom_refnos.len());
            if has_geom_refnos.is_empty() {
                println!("当前节点下面没有要继续生成的基本体几何节点");
                continue;
            }

            let target_loop_refnos = mgr.get_gen_model_target_refnos(GeoEnum::LOOP, &target_dbnos, false).await?;
            println!("使用LOOP的数量: {}", target_loop_refnos.len());
            if run_cache_loop && !target_loop_refnos.is_empty() {
                let instance_mgr_clone = instance_mgr.clone();
                let db_option_clone = db_option.clone();
                let mgr_clone = mgr.clone();
                let handle = tokio::spawn(async move {
                    cache_loop_geos(
                        mgr_clone.clone(),
                        instance_mgr_clone.clone(),
                        &db_option_clone,
                        &target_loop_refnos,
                    )
                        .await
                        .unwrap();
                });
                futures::future::join_all(vec![handle]).await;
            }

            let target_prim_refnos = mgr.get_gen_model_target_refnos(GeoEnum::PRIM, &target_dbnos, false).await?;
            println!("使用基本体数量: {}", target_prim_refnos.len());
            if run_cache_prim && !target_prim_refnos.is_empty() {
                let instance_mgr_clone = instance_mgr.clone();
                let db_option_clone = db_option.clone();
                let mgr_clone = mgr.clone();
                let handle = tokio::spawn(async move {
                    cache_prim_geos(
                        mgr_clone.clone(),
                        instance_mgr_clone.clone(),
                        &db_option_clone,
                        target_prim_refnos.as_slice(),
                    )
                        .await
                        .unwrap();
                });
                futures::future::join_all(vec![handle]).await;
            }

            println!("开始处理负实体计算");
            let (tx, rx) =
                mpsc::unbounded_channel::<(RefU64, Arc<AiosDBManager>, Arc<RwLock<ShapeInstancesData>>)>();
            let rx_stream = UnboundedReceiverStream::new(rx);

            //todo 优化负实体的计算
            let has_pos_neg_map = mgr.query_refnos_has_pos_neg_map(&root_refnos).await.unwrap_or_default();
            dbg!(has_pos_neg_map.len());
            if has_pos_neg_map.is_empty() {
                println!("当前节点下面没有需要参与负实体计算的几何体");
                continue;
            }

            // Spawn a separate task to send messages

            // tokio::spawn(async move {
            //     for refno in has_neg_refnos {
            //         tx.send((refno, mgr_clone_new.clone(), instance_mgr_new.clone())).unwrap();
            //     }
            // });


            if db_option.apply_boolean_operation {
                let now = Instant::now();
                let mut trans_map = DashMap::new();
                let mut mesh_result_map: Arc<DashMap<u64, PlantMesh>> = Arc::new(DashMap::new());
                let mut inst_info_result_map = Arc::new(DashMap::new());
                let mut inst_geos_result_map = Arc::new(DashMap::new());
                {
                    let inst_data = Arc::new(instance_mgr.read().await);
                    let mesh_mgr = Arc::new(mgr.cached_mesh_mgr.read().await);
                    for comp_refno in has_pos_neg_map.keys().cloned() {
                        let trans = mgr.get_world_transform(comp_refno).await.unwrap_or_default().unwrap_or_default();
                        trans_map.insert(comp_refno, trans);
                    }
                    has_pos_neg_map.into_par_iter().for_each(|(comp_refno, (pos_refnos, neg_refnos))| {
                        println!("正在处理: {} 下的负实体", comp_refno);
                        let inst_data_clone = inst_data.clone();
                        let mut mesh_mgr_clone = mesh_mgr.clone();
                        let trans_map_clone = trans_map.clone();
                        let mut mesh_result_map_clone = mesh_result_map.clone();
                        let mut inst_info_result_map_clone = inst_info_result_map.clone();
                        let mut inst_geos_result_map_clone = inst_geos_result_map.clone();

                        let mut pos_meshes = vec![];
                        let mut neg_meshes = vec![];
                        let mut w_aabb: Option<Aabb> = None;
                        //没有正实体的情况，直接跳过
                        if pos_refnos.is_empty() { return; }
                        let Some(w_trans) = trans_map.get(&comp_refno).map(|x| x.value().clone()) else {
                            return;
                        };
                        // dbg!(w_trans);
                        let mut total_refnos = pos_refnos.clone();
                        total_refnos.extend_from_slice(&neg_refnos);
                        let inverse_mat = w_trans.compute_matrix().inverse();

                        for t_refno in total_refnos {
                            let Some(geos_info) = inst_data.get_info(&t_refno) else {
                                continue;
                            };
                            // dbg!(geos_info);
                            if let Some(mut w_aabb) = w_aabb {
                                w_aabb.merge(&geos_info.aabb.unwrap());
                            } else {
                                w_aabb = geos_info.aabb;
                            }
                            let Some(inst_geos) = inst_data.get_inst_geos(geos_info) else {
                                continue;
                            };
                            for geo_inst in inst_geos {
                                let geo_refno = geo_inst.refno;
                                // dbg!(geo_refno);
                                let Some(mesh) = mesh_mgr_clone.get_mesh(geo_inst.geo_hash) else {
                                    continue;
                                };
                                // let Ok(Some(geo_mat)) = mgr.get_world_transform(geo_refno).await else {
                                //     continue;
                                // };
                                let geo_mat = geo_inst.transform * geos_info.world_transform;
                                let ele_mat = inverse_mat * geo_mat.compute_matrix();
                                let local_mat = ele_mat * geo_inst.transform.compute_matrix();
                                let csg_mesh = mesh.into_csg_mesh(&local_mat);
                                if pos_refnos.contains(&t_refno) {
                                    pos_meshes.push(csg_mesh)
                                } else {
                                    neg_meshes.push(csg_mesh);
                                }
                            }
                        }
                        let geo_hash = *comp_refno;
                        if pos_meshes.is_empty() { return; }
                        let mut final_mesh = pos_meshes.pop().unwrap();
                        for pos_mesh in pos_meshes {
                            final_mesh = final_mesh + pos_mesh;
                        }
                        for neg_mesh in neg_meshes {
                            final_mesh = final_mesh - neg_mesh;
                        }
                        mesh_result_map_clone.insert(geo_hash, final_mesh.into());
                        let geom_inst = EleInstGeo {
                            geo_hash,
                            refno: comp_refno,
                            pts: vec![],
                            aabb: None,
                            transform: Transform::IDENTITY,
                            geo_param: PdmsGeoParam::CompoundShape,
                            visible: true,
                            is_tubi: false,
                            geo_type: GeoBasicType::Compound,
                        };


                        let mut geos_info = EleGeosInfo {
                            refno: comp_refno,
                            visible: true,
                            generic_type: mgr.get_generic_type(comp_refno),
                            aabb: w_aabb,
                            world_transform: w_trans,
                            cata_hash: None,
                        };
                        // dbg!(&geos_info);
                        inst_info_result_map_clone.insert(comp_refno, geos_info);
                        let comp_type = mgr.get_refno_basic(comp_refno).unwrap().get_type().to_string();
                        inst_geos_result_map_clone.insert(*comp_refno, EleInstGeosData {
                            inst_key: *comp_refno,
                            refno: comp_refno,
                            insts: vec![geom_inst],
                            aabb: None,
                            type_name: comp_type,
                            ptset_map: Default::default(),
                            flow_pt_indexs: vec![],
                        });
                    });

                    println!("布尔运算实体耗时 {} ms", now.elapsed().as_millis());
                }

                {
                    let mut inst_data = instance_mgr.write().await;
                    dbg!(inst_geos_result_map.len());
                    let inst_geos_result_map_inner = Arc::try_unwrap(inst_geos_result_map).unwrap();
                    for (k, v) in inst_geos_result_map_inner {
                        inst_data.insert_geos_data(k, v);
                    }
                    let inst_info_result_map_inner = Arc::try_unwrap(inst_info_result_map).unwrap();
                    for (k, v) in inst_info_result_map_inner {
                        inst_data.insert_info(k, v);
                    }
                    let mut mesh_mgr = mgr.cached_mesh_mgr.write().await;
                    let mesh_result_map_inner = Arc::try_unwrap(mesh_result_map).unwrap();
                    for (k, v) in mesh_result_map_inner {
                        mesh_mgr.insert(k, v);
                    }
                }
            }
            {
                let inst_data = instance_mgr.read().await;
                println!("当前db下的基本体生成统计：");
                dbg!(inst_data.inst_geos_map.len());
                save_instance_to_graph_db(&mgr, &inst_data).await?;
            }
            println!("{db_no} 生成完毕。");
        }

        {
            let mesh_mgr = mgr.cached_mesh_mgr.read().await;
            dbg!(mesh_mgr.len());
            save_mesh_to_arango_db(&mgr, &mesh_mgr).await?;
        }

        println!("生成所有模型时间: {}ms", time.elapsed().as_millis());
        Ok(true)
    }

    async fn process_csg_boolean_operations(has_geom_refno: RefU64, mgr: Arc<AiosDBManager>,
                                            instance_mgr: Arc<RwLock<ShapeInstancesData>>) -> anyhow::Result<bool> {
        let pos_neg_map = mgr.query_refnos_has_pos_neg_map(&[has_geom_refno]).await.unwrap_or_default();
        // dbg!(&pos_neg_map);
        let has_neg = !pos_neg_map.is_empty();
        // dbg!(has_neg);
        //如果有负实体，直接合在一起，不需要再拆分
        //有点太慢了，todo 改用manifold 库试试
        for (comp_refno, (pos_refnos, neg_refnos)) in pos_neg_map {
            // dbg!(comp_refno);
            println!("正在处理: {} 下的负实体", comp_refno);
            let mut pos_meshes = vec![];
            let mut neg_meshes = vec![];
            let mut w_aabb: Option<Aabb> = None;
            //没有正实体的情况，直接跳过
            if pos_refnos.is_empty() { continue; }
            let Ok(Some(w_trans)) = mgr.get_world_transform(comp_refno).await else {
                continue;
            };
            let mut total_refnos = pos_refnos.clone();
            total_refnos.extend_from_slice(&neg_refnos);
            // dbg!(&total_refnos);
            // dbg!(&pos_refnos);
            let inverse_mat = w_trans.compute_matrix().inverse();
            {
                let inst_data = instance_mgr.read().await;
                let mesh_mgr = mgr.cached_mesh_mgr.read().await;
                for t_refno in total_refnos {
                    let Some(geos_info) = inst_data.get_info(&t_refno) else {
                        continue;
                    };
                    // dbg!(geos_info);
                    if let Some(mut w_aabb) = w_aabb {
                        w_aabb.merge(&geos_info.aabb.unwrap());
                    } else {
                        w_aabb = geos_info.aabb;
                    }
                    let Some(inst_geos) = inst_data.get_inst_geos(geos_info) else {
                        continue;
                    };
                    for geo_inst in inst_geos {
                        let geo_refno = geo_inst.refno;
                        // dbg!(geo_refno);
                        let Some(mesh) = mesh_mgr.get_mesh(geo_inst.geo_hash) else {
                            // dbg!(geo_inst);
                            continue;
                        };
                        let Ok(Some(geo_mat)) = mgr.get_world_transform(geo_refno).await else {
                            continue;
                        };
                        let ele_mat = inverse_mat * geo_mat.compute_matrix();
                        let local_mat = ele_mat * geo_inst.transform.compute_matrix();
                        let csg_mesh = mesh.into_csg_mesh(&local_mat);
                        if pos_refnos.contains(&t_refno) {
                            pos_meshes.push(csg_mesh)
                        } else {
                            neg_meshes.push(csg_mesh);
                        }
                    }
                }
            }
            let geo_hash = *comp_refno;
            // let mut inst_data = instance_mgr.write().await;
            // let mut mesh_mgr = mgr.cached_mesh_mgr.write().await;
            if pos_meshes.is_empty() { return Ok(false); }
            let mut final_mesh = pos_meshes.pop().unwrap();
            for pos_mesh in pos_meshes {
                final_mesh = final_mesh + pos_mesh;
            }
            for neg_mesh in neg_meshes {
                final_mesh = final_mesh - neg_mesh;
            }
            // mesh_mgr.insert(geo_hash, final_mesh.into());
            let geom_inst = EleInstGeo {
                geo_hash,
                refno: comp_refno,
                pts: vec![],
                aabb: None,
                transform: Transform::IDENTITY,
                geo_param: PdmsGeoParam::CompoundShape,
                visible: true,
                is_tubi: false,
                geo_type: GeoBasicType::Compound,
            };


            let mut geos_info = EleGeosInfo {
                refno: comp_refno,
                visible: true,
                generic_type: mgr.get_generic_type(comp_refno),
                aabb: w_aabb,
                world_transform: w_trans,
                cata_hash: None,
            };
            // dbg!(&geos_info);
            // inst_data.insert_info(comp_refno, geos_info);
            let comp_type = mgr.get_refno_basic(comp_refno).unwrap().get_type().to_string();
            // inst_data.insert_geos_data(*comp_refno, EleInstGeosData{
            //     inst_key: *comp_refno,
            //     refno: comp_refno,
            //     insts: vec![geom_inst],
            //     aabb: None,
            //     type_name: comp_type,
            //     ptset_map: Default::default(),
            //     flow_pt_indexs: vec![],
            // });
        }

        return Ok(true);
    }

    async fn process_occ_boolean_operations(has_geom_refno: RefU64, mgr: Arc<AiosDBManager>, instance_mgr: Arc<RwLock<ShapeInstancesData>>) -> anyhow::Result<bool> {
        // let pos_neg_map = mgr.query_refnos_has_pos_neg_map(has_geom_refno).await.unwrap_or_default();
        // // dbg!(&pos_neg_map);
        // let has_neg = !pos_neg_map.is_empty();
        // // dbg!(has_neg);
        // //如果有负实体，直接合在一起，不需要再拆分
        // //有点太慢了，todo 改用manifold 库试试
        // for (comp_refno, (pos_refnos, neg_refnos)) in pos_neg_map {
        //     dbg!(comp_refno);
        //     let mut pos_shapes = vec![];
        //     let mut neg_shapes = vec![];
        //     let mut w_aabb: Option<Aabb> = None;
        //     //没有正实体的情况，直接跳过
        //     if pos_refnos.is_empty() { continue; }
        //     let Ok(Some(w_trans)) = mgr.get_world_transform(comp_refno).await else {
        //         continue;
        //     };
        //     let mut total_refnos = pos_refnos.clone();
        //     total_refnos.extend_from_slice(&neg_refnos);
        //     // dbg!(&total_refnos);
        //     dbg!(&pos_refnos);
        //     let inverse_mat = w_trans.compute_matrix().inverse();
        //     {
        //         let inst_data = instance_mgr.read().await;
        //         let mesh_mgr = mgr.cached_mesh_mgr.read().await;
        //         let mut neg_need_offset = false;
        //         'outer: for t_refno in total_refnos {
        //             let Some(geos_info) = inst_data.get_info(&t_refno) else {
        //                 continue;
        //             };
        //             // dbg!(geos_info);
        //             if let Some(mut w_aabb) = w_aabb {
        //                 w_aabb.merge(&geos_info.aabb.unwrap());
        //             } else {
        //                 w_aabb = geos_info.aabb;
        //             }
        //             for geo_inst in &geos_info.geo_basics {
        //                 let geo_refno = geo_inst.refno;
        //                 // dbg!(geo_refno);
        //                 let Some(occ_shape) = mesh_mgr.get_occ_shape(geo_inst.geo_hash) else {
        //                     dbg!(geo_inst);
        //                     continue;
        //                 };
        //                 // dbg!("Get shape");
        //                 let Ok(Some(geo_mat)) = mgr.get_world_transform(geo_refno).await else {
        //                     continue;
        //                 };
        //                 let ele_mat = inverse_mat * geo_mat.compute_matrix();
        //
        //                 // dbg!(ele_mat.to_scale_rotation_translation());
        //                 let local_mat = ele_mat * geo_inst.transform.compute_matrix();
        //                 // dbg!(&local_mat);
        //                 //如果scale都是一样的，只需要用transform
        //                 let (s, r, t) = local_mat.to_scale_rotation_translation();
        //                 let is_scale_same = abs_diff_eq!(s.max_element(), s.min_element(), epsilon=0.01);
        //                 // dbg!(is_scale_same);
        //                 let shape = if is_scale_same {
        //                     occ_shape.transform(&local_mat.as_dmat4()).unwrap()
        //                 } else {
        //                     occ_shape.g_transform(&local_mat.as_dmat4()).unwrap()
        //                 };
        //
        //                 if pos_refnos.contains(&t_refno) {
        //                     // neg_need_offset = matches!(geo_inst.geo_param, PrimExtrusion(_));
        //                     // dbg!(neg_need_offset);
        //                     pos_shapes.push(shape)
        //                 } else {
        //                     // if geo_refno == RefU64::from_two_nums(24381, 35205) {
        //                     //     continue;
        //                     // }
        //                     dbg!(t_refno);
        //                     //说明，这里特殊处理一下，如果被切割的是 extrusion,，需要将负实体扩张一下，不然生成的不对
        //                     // if neg_need_offset {
        //                     //     let cut_shape = shape.offset(1.0).expect("Offset shape error.");
        //                     //     neg_shapes.push(cut_shape);
        //                     // } else {
        //                     neg_shapes.push(shape);
        //                     // }
        //                 }
        //             }
        //         }
        //     }
        //     let geo_hash = *comp_refno;
        //     let mut inst_data = instance_mgr.write().await;
        //     let mut mesh_mgr = mgr.cached_mesh_mgr.write().await;
        //     // dbg!(pos_shapes.len());
        //     // dbg!(neg_shapes.len());
        //     let mut final_shape = None;
        //     // if let Ok(mut pos_compound_shape) = OCCShape::fuse_shapes(&pos_shapes)
        //     //     && let Ok(neg_compound_shape) = OCCShape::fuse_shapes(&neg_shapes)
        //     // {
        //     //     println!("Cut by merged.");
        //     //     if let Ok(s) = pos_compound_shape.cut(&neg_compound_shape, 1.0) {
        //     //         final_shape = Some(s);
        //     //     }
        //     // }
        //     if final_shape.is_none() {
        //         if let Ok(mut pos_compound_shape) = OCCShape::fuse_shapes(&pos_shapes) {
        //             println!("Cut by merged failed, so by each one.");
        //             for neg_shape in &neg_shapes {
        //                 pos_compound_shape = pos_compound_shape.cut(neg_shape, 1.0).unwrap();
        //             }
        //             final_shape = Some(pos_compound_shape);
        //         }
        //     }
        //
        //     if let Some(s) = final_shape {
        //         let size = w_aabb.unwrap().bounding_sphere().radius as f64;
        //         dbg!(size);
        //         let mesh: PlantMesh = s.mesh(0.01 * size).unwrap().into();
        //         mesh_mgr.insert(geo_hash, mesh);
        //     } else {
        //         println!("Cut 失败.");
        //     }
        //
        //     let geom_inst = EleInstGeo {
        //         geo_hash,
        //         refno: comp_refno,
        //         pts: vec![],
        //         aabb: None,
        //         transform: Transform::IDENTITY,
        //         geo_param: PdmsGeoParam::CompoundShape,
        //         visible: true,
        //         is_tubi: false,
        //     };
        //
        //     let mut geos_info = EleGeosInfo {
        //         refno: comp_refno,
        //         geo_basics: vec![geom_inst],
        //         visible: true,
        //         generic_type: mgr.get_generic_type(comp_refno),
        //         aabb: w_aabb,
        //         world_transform: w_trans,
        //         ptset_map: default(),
        //         flow_pt_indexs: default(),
        //     };
        //     // dbg!(&geos_info);
        //     inst_data.insert(comp_refno, geos_info);
        // }
        //
        return Ok(true);
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
    let x = Vec3::new(3460.0, 9230.0, 5013.23);
    let y = Vec3::new(3460.0, 9230.0, 5081.305);
    let distance = x.distance(y);
    dbg!(&distance);
}
