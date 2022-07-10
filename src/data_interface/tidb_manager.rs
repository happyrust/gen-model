use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::default::default;
use std::default::Default;
use std::env;
use std::mem::take;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use aios_core::consts::*;
use aios_core::db_number::DbNumMgr;
use aios_core::parsed_data::{CateAxisParam, GeomsInfo};
// use simple_process_stats::ProcessStats;
use aios_core::pdms_types::*;
use aios_core::prim_geo::category::{CateBrepShape, convert_to_brep_shapes};
use aios_core::prim_geo::extrusion::{CurveType, Extrusion};
use aios_core::prim_geo::facet::{Contour, Facet, Polygon};
use aios_core::prim_geo::revolution::Revolution;
use aios_core::prim_geo::tubing::PdmsTubing;
use aios_core::shape::pdms_shape::{BrepShapeTrait, PdmsMesh, VerifiedShape};
use anyhow::anyhow;
use append_only_vec::AppendOnlyVec;
use approx::{abs_diff_eq, abs_diff_ne};
use async_trait::async_trait;
use config::{Config, ConfigError, Environment, File};
use dashmap::{DashMap, DashSet};
use dashmap::mapref::one::Ref;
use glam::{Quat, quat, TransformRT, TransformSRT, Vec3};
use id_tree::{Node, NodeId};
use lazy_static::lazy_static;
use once_cell::sync::Lazy;
use smol_str::SmolStr;
use sqlx::{MySql, MySqlPool, Pool};

use crate::api::attr::*;
use crate::api::children::cache_site_node;
use crate::api::element::*;
use crate::api::refno_info::{get_ref0_map, sync_refno_basic_map};
use crate::ATTR_INFO_MAP;
use crate::cata::query_cata::resolve_desi_comp;
use crate::cata::sctn;
use crate::cata::sctn::geo::create_st_geos;
use crate::consts::*;
use crate::data_interface::cache::{CACHED_MDB_SITE_MAP, CACHED_REFNO_BASIC_MAP, PDMS_ATT_MAP_CACHE, PDMS_IMPLICIT_ATT_MAP_CACHE};
use crate::data_interface::defines::CachedRefBasic;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::structs::AIOSAxisMap;
use crate::defines::AiosString;
use crate::helper::qualified_table_name;
use crate::options::DbOption;

pub const TUBI_TOL: f32 = 10.0f32;
pub const BATCH_COUNT: usize = 50;


pub type CateBrepShapeMap = DashMap<RefU64, Vec<CateBrepShape>>;
// static GLOBAL_COLLISION_WORLD: Lazy<Mutex<CollisionWorld<f32, (RefU64, RefU64)>>> = Lazy::new(|| {
//     let mut world = CollisionWorld::<f32, (RefU64, RefU64)>::new(0.001f32);
//     Mutex::new(world)
// });

// static PRIM_HASH_NOUNS: Lazy<Vec<u32>> = Lazy::new(|| {
//     vec![BOX_HASH, CYLI_NOUN, SPHE_NOUN, CONE_NOUN, CTOR_NOUN, DISH_NOUN,
//          LOOP_NOUN, PYRA_NOUN, RTOR_NOUN, REVO_NOUN, POHE_NOUN, PLOO_NOUN, SPINE_NOUN]
// });


//"SPINE", "GENS",
static GNERAL_PRIM_NOUN_NAMES: Lazy<Vec<&'static str>> = Lazy::new(|| {
    vec!["BOX", "CYLI", "SPHE", "CONE", "DISH", "CTOR", "RTOR", "PYRA"]
});

static PDMS_GNERAL_TYPE_NAMES_MAP: Lazy<HashMap<&'static str, PdmsGenericType>> = Lazy::new(|| {
    // vec!["EQUI", "PIPE", "STRU", "ELEC", ""]
    let mut m = HashMap::new();
    m.insert("EQUI", PdmsGenericType::EQUI);
    m.insert("PIPE", PdmsGenericType::PIPE);
    m.insert("ROOM", PdmsGenericType::ROOM);
    m.insert("STRU", PdmsGenericType::STRU);
    m
});

static GENRIC_NOUN_NAMES: Lazy<Vec<SmolStr>> = Lazy::new(|| {
    vec!["EQUI".into(), "PIPE".into(), "STRU".into(), "ROOM".into(), "STWALL".into(), "FLOOR".into()]
});


#[derive(Debug)]
pub struct AiosDBManager {
    pub project_map: DashMap<String, Pool<MySql>>,

    pub ref0_map: DashMap<u32, String>,

    pub info_pool: Pool<MySql>,

    pub projects: Vec<String>,

    pub needed_parse_files: Option<Vec<String>>,

    pub project_path: String,  //整个项目的路径

    pub db_option: DbOption,

    pub dbno_mgr: DbNumMgr,

    pub cached_mesh_mgr: Arc<CachedMeshesMgr>,

    pub mesh_instance_mgr: Arc<DashMap<i32, PdmsMeshInstanceMgr>>,

    cached_world_transforms_map: Arc<DashMap<RefU64, TransformRT>>,
}

#[async_trait]
impl PdmsDataInterface for AiosDBManager {
    /// 获得最全的数据
    async fn get_attr(&self, refno: RefU64) -> anyhow::Result<AttrMap> {
        if let Some(k) = PDMS_ATT_MAP_CACHE.get(&refno) {
            return Ok(k.value().clone());
        }
        if let Some(project_pool) = self.get_project_pool(refno) {
            if let Some(ref_basic) = self.get_refno_basic(refno) {
                let attr = query_full_attr(refno, ref_basic.value(), &project_pool, None).await?;
                if PDMS_ATT_MAP_CACHE.use_sled() {
                    PDMS_ATT_MAP_CACHE.insert(refno, attr.clone());
                }
                return Ok(attr);
            }
        }
        Ok(AttrMap::default())
    }

    ///获取owner的参考号，从缓存读取
    #[inline]
    fn get_owner(&self, refno: RefU64) -> RefU64 {
        CACHED_REFNO_BASIC_MAP.get(&refno)
            .map(|x| x.value().get_owner()).unwrap_or_default()
    }

    /// 获得隐含数据的属性
    async fn get_implicit_attr(&self, refno: RefU64, columns: Option<Vec<&str>>) -> anyhow::Result<AttrMap> {
        if let Some(k) = PDMS_IMPLICIT_ATT_MAP_CACHE.get(&refno) {
            return Ok(k.value().clone());
        }
        if let Some(project_pool) = self.get_project_pool(refno) {
            if let Some(ref_basic) = self.get_refno_basic(refno) {
                let attr = query_implicit_attr(refno, ref_basic.value(), &project_pool, columns).await?;
                if PDMS_IMPLICIT_ATT_MAP_CACHE.use_sled() {
                    PDMS_IMPLICIT_ATT_MAP_CACHE.insert(refno, attr.clone());
                }
                return Ok(attr);
            }
        }
        Ok(AttrMap::default())
    }

    /// 获得OWNER隐含数据的属性
    async fn get_implicit_attrs_by_owner(&self, owner: RefU64, type_name: &str, columns: Option<Vec<&str>>) -> anyhow::Result<Vec<AttrMap>> {
        if let Some(project_pool) = self.get_project_pool(owner) {
            let attr = query_implicit_attrs_by_owner(owner, type_name, &project_pool, columns).await?;
            return Ok(attr);
        }
        Ok(vec![])
    }

    /// 获取parent的attr数据
    async fn get_parent_attr(&self, refno: RefU64) -> anyhow::Result<AttrMap> {
        // if let Some(project_pool) = self.get_project_pool(refno) {
        //     let attr = query_parent_attr(refno, &project_pool, None).await?;
        //     return Ok(attr);
        // }
        Ok(AttrMap::default())
    }

    /// 获得缓存的refno基本信息
    #[inline]
    fn get_refno_basic(&self, refno: RefU64) -> Option<Ref<RefU64, CachedRefBasic>> {
        if !refno.is_valid() {
            None
        } else {
            CACHED_REFNO_BASIC_MAP.get(&refno)
        }
    }

    /// 获得owner缓存的refno基本信息
    #[inline]
    fn get_owner_ref_basic(&self, refno: RefU64) -> Option<Ref<RefU64, CachedRefBasic>> {
        let owner_ref = self.get_owner(refno);
        self.get_refno_basic(owner_ref)
    }

    /// 获得节点数据
    async fn get_ele_node(&self, refno: RefU64) -> anyhow::Result<Option<EleTreeNode>> {
        if let Some(project_pool) = self.get_project_pool(refno) {
            if let Ok(node) = query_ele_node(refno, &project_pool).await {
                return Ok(Some(node));
            }
        }
        Ok(None)
    }

    async fn get_owner_ele_node(&self, refno: RefU64) -> anyhow::Result<Option<EleTreeNode>> {
        let mut node = None;
        if let Some(project_pool) = self.get_project_pool(refno) {
            let parent = self.get_owner(refno);
            if parent.is_valid() {
                node = Some(query_ele_node(parent, &project_pool).await?);
            }
        }
        Ok(node)
    }

    async fn get_world(&self, project: &str, mdb_name: &str, module: &str) -> anyhow::Result<EleTreeNode> {
        if let Some(project_pool) = self.project_map.get(project) {
            let v = query_world("SAMPLE", "DESI", project_pool.value()).await?;
            return Ok(v);
        }
        return Err(anyhow!("World not found".to_string()));
    }

    async fn get_children_nodes(&self, refno: RefU64) -> anyhow::Result<Vec<EleTreeNode>> {
        let mut r = vec![];
        if let Some(project_pool) = self.get_project_pool(refno) {
            let children = query_children(refno, &project_pool).await?;
            for (refno, _) in children {
                let node = query_ele_node(refno, &project_pool).await?;
                r.push(node);
            }
        }
        Ok(r)
    }

    async fn get_children_attrs(&self, refno: RefU64) -> anyhow::Result<Vec<AttrMap>> {
        let mut r = vec![];
        if let Some(project_pool) = self.get_project_pool(refno) {
            let children = query_children(refno, &project_pool).await?;
            for child in children {
                let attr = self.get_attr(child.0).await?;
                r.push(attr);
            }
        }
        Ok(r)
    }

    async fn get_children_refs(&self, refno: RefU64) -> anyhow::Result<RefU64Vec> {
        let mut result = RefU64Vec::default();
        if let Some(project_pool) = self.get_project_pool(refno) {
            let children = query_children(refno, &project_pool).await?;
            children.into_iter().for_each(|child| {
                result.push(child.0);
            });
        }
        Ok(result)
    }

    async fn get_name(&self, refno: RefU64) -> anyhow::Result<SmolStr> {
        if let Some(project_pool) = self.get_project_pool(refno) {
            let name = query_name(refno, &project_pool).await?;
            return Ok(SmolStr::new(name));
        }
        Ok(SmolStr::new(""))
    }

    /// dbnos为空代表所有db都会去获取
    async fn get_refnos_by_types<'a>(&self, project: &'a str, att_types: &'a Vec<&str>, dbnos: Option<Vec<i32>>) -> anyhow::Result<RefU64Vec> {
        if let Some(project_pool) = self.project_map.get(project) {
            let r = query_types_refnos(att_types, project_pool.value(), dbnos).await?;
            return Ok(r);
        }
        Ok(RefU64Vec::default())
    }

    async fn get_db_world(&self, project: &str, db_no: u32) -> anyhow::Result<Option<(RefU64, String)>> {
        if let Some(project_pool) = self.project_map.get(project) {
            let r = query_id_name_from_dbno_type(db_no as i32, "WORL", project_pool.value()).await?;
            if let Some(mut r) = r {
                return Ok(Some(r.remove(0)));
            }
        }
        return Ok(None);
    }

    /// 获得参考号的祖先参考号
    fn get_ancestors_refnos(&self, refno: RefU64) -> Vec<RefU64> {
        let mut result = vec![refno]; //需要包含自己
        let mut cur_refno = refno;
        while let Some(b) = CACHED_REFNO_BASIC_MAP.get(&cur_refno) {
            cur_refno = b.owner;
            result.push(cur_refno);
        }
        result
    }

    fn get_ancestors_refnos_without_world(&self, refno: RefU64) -> Vec<RefU64> {
        let mut result = vec![refno]; //需要包含自己
        let mut cur_refno = refno;
        while let Some(b) = CACHED_REFNO_BASIC_MAP.get(&cur_refno) {
            if b.get_type() == "WORL" {
                break;
            }
            cur_refno = b.owner;
            result.push(cur_refno);
        }
        result
    }

    /// 获得参考号的祖先属性
    async fn get_ancestors_attrs(&self, refno: RefU64) -> Vec<AttrMap> {
        let mut cur_refno = refno;
        let mut r = vec![];
        let pool = self.get_project_pool(refno).unwrap();
        while let Ok(attr) = self.get_implicit_attr(cur_refno, None).await {
            //后面是不是要缓存这个层级结构
            if let Ok(Some(owner)) = query_owner_from_id(cur_refno, &pool).await {
                r.push(attr);
                cur_refno = owner;
            } else {
                break;
            }
        }
        r
    }

    /// 获得参考号的祖先节点
    async fn get_ancestor_nodes(&self, refno: RefU64) -> anyhow::Result<VecDeque<EleTreeNode>> {
        let mut cur_refno = refno;
        let mut ancestors = VecDeque::new();
        while let Some(node) = self.get_ele_node(cur_refno).await? {
            cur_refno = node.owner;
            ancestors.push_front(node);
        }
        Ok(ancestors)
    }

    ///获得世界坐标系, 需要缓存数据，如果已经存在数据了，直接获取
    async fn get_world_transform(&self, refno: RefU64) -> anyhow::Result<Option<glam::TransformRT>> {
        let mut ancestors = VecDeque::new();
        let mut rotation = Quat::IDENTITY;
        let mut translation = Vec3::ZERO;
        let mut cur_refno = refno;
        while let Some(ref_basic) = self.get_refno_basic(cur_refno) {
            //后面是不是要缓存这个层级结构
            if self.cached_world_transforms_map.contains_key(&cur_refno) {
                self.cached_world_transforms_map.get(&cur_refno).map(|x| {
                    rotation = x.rotation;
                    translation = x.translation;
                });
                break;
            }
            let tmp_owner = ref_basic.get_owner();
            ancestors.push_front((cur_refno, ref_basic));
            cur_refno = tmp_owner;
        }
        // dbg!(&new_ancestors);
        for (refno, ref_basic) in ancestors {
            let type_name = ref_basic.get_type();
            let (quat, pos) = if type_name == "SCTN" || type_name == "STWALL" {
                let mut quat = Quat::IDENTITY;
                let att = self.get_implicit_attr(refno, Some(vec!["POSS", "POSE", "BANG", "POS"])).await?;
                let poss = att.get_poss().unwrap();
                let pose = att.get_pose().unwrap();
                let extru_dir: Vec3 = (pose - poss).normalize();
                let bangle = att.get_f32("BANG").unwrap_or_default();
                //如果和Z轴平行，需要使用Y轴作为参考轴
                let d = extru_dir.dot(Vec3::Z).abs();

                let mut ref_axis = if abs_diff_eq!(1.0, d) {
                    Vec3::Y
                } else { Vec3::Z };

                let p_axis = ref_axis.cross(extru_dir).normalize();
                let y_axis = extru_dir.cross(p_axis).normalize();
                quat = Quat::from_mat3(&glam::f32::Mat3::from_cols_array_2d(
                    &[p_axis.to_array(), y_axis.to_array(), extru_dir.to_array()]
                )) * Quat::from_rotation_z(bangle.to_radians());
                let pos = att.get_position().unwrap_or_default();
                (quat, pos)
            } else {
                //这里可以直接判断有没有这两个属性，没有就直接返回
                let mut quat = Quat::IDENTITY;
                let mut pos = Vec3::default();
                let att_names = vec!["ORI", "POSS", "POS"];
                if ATTR_INFO_MAP.exist_least_one_att_by_names(type_name, &att_names) {
                    if let Ok(att) = self.get_implicit_attr(refno, Some(vec!["ORI", "POS", "POSS"])).await {
                        pos = att.get_position().unwrap_or_default();
                        quat = att.get_rotation().unwrap_or_default();
                    }
                }
                (quat, pos)
            };
            translation = translation + rotation * pos;
            rotation = rotation * quat;
            // println!("{} : {:?}", refno.to_refno_str(), (translation, rotation));
            self.cached_world_transforms_map.entry(refno).or_insert(TransformRT {
                rotation,
                translation,
            });
        }
        Ok(Some(glam::TransformRT {
            rotation,
            translation,
        }))
    }
}


impl AiosDBManager {
    pub fn get_table_name(&self, refno: RefU64) -> String {
        CACHED_REFNO_BASIC_MAP.get(&refno)
            .map(|x| x.get_table_name().to_string()).unwrap_or("UNSET".to_string())
    }

    #[inline]
    pub fn get_db_option() -> anyhow::Result<DbOption> {
        use config::{Config, ConfigError, Environment, File};
        let s = Config::builder()
            .add_source(File::with_name("DbOption"))
            .build()?;

        s.try_deserialize::<DbOption>().map_err(|x| anyhow!(x.to_string()))
    }

    #[inline]
    pub fn get_default_conn_str(d: &DbOption) -> String {
        let user = d.user.as_str();
        let pwd = d.password.as_str();
        let ip = d.ip.as_str();
        let port = d.port.as_str();
        format!("mysql://{user}:{pwd}@{ip}:{port}")
    }

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
        MySqlPool::connect(&format!("{connection_str}/{}", project)).await.map_err(
            {
                |x| anyhow!(x.to_string())
            }
        )
    }

    pub fn gen_pool_from_refno(self, refno: RefU64) -> anyhow::Result<Option<Pool<MySql>>> {
        if let Some(project) = self.ref0_map.get(&refno.get_0()) {
            if let Some(project_pool) = self.project_map.get(project.value()) {
                return Ok(Some(project_pool.value().clone()));
            }
        }
        Ok(None)
    }

    ///获得默认的pool
    #[inline]
    pub async fn get_default_pool(conn_str: &str) -> anyhow::Result<Pool<MySql>> {
        MySqlPool::connect(conn_str).await.map_err(|x| anyhow!(x.to_string()))
    }

    ///从默认配置文件初始化
    pub async fn init_form_config() -> anyhow::Result<Self> {
        let db_option = Self::get_db_option()?;
        Self::init(db_option).await
    }

    ///初始化
    pub async fn init(db_option: DbOption) -> anyhow::Result<Self> {
        let dir = db_option.project_path.to_string();
        let mut project_map = DashMap::new();

        let db_option = Self::get_db_option()?;
        let default_conn = AiosDBManager::get_default_conn_str(&db_option);
        // let mut cached_site = vec![];
        // let cached_refno_basic_map: Arc<DashMap<RefU64, CachedRefBasic>> = Arc::new(Default::default());
        // let process_stats = ProcessStats::get().await.expect("could not get stats for running process");
        // println!("{:?}", process_stats);
        let time = Instant::now();
        let mut dbno_mgr = DbNumMgr::default();
        // CACHED_REFNO_BASIC_MAP.load_all();
        // let need_sync = CACHED_REFNO_BASIC_MAP.is_empty();
        let need_sync = true;
        for project in &db_option.included_projects {
            let project_pool = AiosDBManager::get_db_pool(&default_conn, project).await;
            match project_pool {
                Ok(pool) => {
                    //暂时保存在内存，需要序列化到heed LMDB数据库
                    if need_sync {
                        sync_refno_basic_map(&pool, &mut dbno_mgr).await.unwrap();
                    }
                    project_map.entry(project.clone()).or_insert(pool.clone());
                    // 将树节点的site层提前缓存下来
                    cache_site_node(&db_option.mdb_name, &db_option.module, &pool).await;
                }
                Err(_) => { println!("project: {} init failed", project); }
            }
        }
        println!("缓存RefBasic数据花费：{}ms", time.elapsed().as_millis());

        let info_conn = AiosDBManager::get_db_pool(&default_conn, &format!("{}_{}",
                                                                           PDMS_INFO_DB, &db_option.project_name.to_uppercase())).await?;
        let ref0_map = get_ref0_map(&info_conn).await?;
        let projects = db_option.included_projects.clone();
        Ok(
            Self {
                project_map,
                ref0_map,
                info_pool: info_conn,
                projects,
                needed_parse_files: None,
                project_path: dir,
                db_option,
                dbno_mgr,
                cached_mesh_mgr: Arc::new(Default::default()),
                mesh_instance_mgr: Arc::new(Default::default()),
                cached_world_transforms_map: Arc::new(Default::default()),
            }
        )
    }

    ///获得 project 名称
    #[inline]
    pub fn get_project_name(&self, refno: RefU64) -> Option<String> {
        self.ref0_map.get(&refno.get_0()).map(|x| x.value().clone())
    }

    ///获得project 的db
    #[inline]
    pub fn get_project_db(&self, refno: RefU64) -> Option<Pool<MySql>> {
        if let Some(d) = self.ref0_map.get(&refno.get_0()) {
            self.project_map.get(d.value()).map(|x| x.value().clone())
        } else {
            None
        }
    }

    ///获得project 的mysql pool
    #[inline]
    pub fn get_project_pool(&self, refno: RefU64) -> Option<Pool<MySql>> {
        if let Some(d) = self.ref0_map.get(&refno.get_0()) {
            self.project_map.get(d.value()).map(|x| x.value().clone())
        } else {
            None
        }
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

    ///获取单个元件的模型数据
    pub async fn get_cata_single_geoms(mgr: Arc<AiosDBManager>, design_refno: RefU64, branch_att: &AttrMap,
                                       brep_shape_map: &CateBrepShapeMap, refno_ptset_map: &DashMap<RefU64, AIOSAxisMap>) -> anyhow::Result<bool> {
        let cur_ele = mgr.get_refno_basic(design_refno).unwrap();
        let type_name = cur_ele.get_type();
        let owner = mgr.get_owner_ref_basic(design_refno);
        if owner.is_none() {
            dbg!(design_refno);
            return Ok(false);
        }
        let owner = owner.unwrap();
        if type_name == "BRAN" {
            return Ok(false);
        }
        if owner.get_type() == "BRAN" {
            dbg!(design_refno);
            return Ok(false);
        }

        let desi_att = mgr.get_implicit_attr(design_refno, None).await?;

        let geoms = resolve_desi_comp(design_refno, mgr.as_ref()).await.unwrap_or_default();
        if type_name == "SCTN" || type_name == "STWALL" || type_name == "GENSEC" {
            create_st_geos(design_refno, &desi_att, &geoms, &brep_shape_map, mgr.as_ref()).await?;
        } else {
            let GeomsInfo {
                geometries,
                axis_map
            } = geoms;
            let len = geometries.len();
            for (i, geom) in geometries.into_iter().enumerate() {
                if let Some(cate_shape) = convert_to_brep_shapes(&geom) {
                    brep_shape_map.entry(design_refno).or_insert(Vec::new()).push(cate_shape);
                }
            }
            refno_ptset_map.insert(design_refno, axis_map);
        }

        Ok(true)
    }

    ///记录点集的信息
    ///获得branch的模型数据
    async fn get_cata_branch_geoms(mgr: Arc<AiosDBManager>, branch_refno: RefU64, branch_att: &AttrMap,
                                   brep_shape_map: &CateBrepShapeMap, refno_ptset_map: &DashMap<RefU64, AIOSAxisMap>) -> anyhow::Result<bool> {
        let bran_transform = mgr.get_world_transform(branch_refno).await?.unwrap_or_default();
        let bran_htube_pt = bran_transform.transform_point3(branch_att.get_vec3("HPOS")
            .ok_or(anyhow!("HPOS not exist".to_string()))?);
        let bran_ttube_pt = bran_transform.transform_point3(branch_att.get_vec3("TPOS")
            .ok_or(anyhow!("TPOS not exist".to_string()))?);
        let htube_ref = branch_att.get_foreign_refno("HSTU").unwrap_or_default();
        let hconnect = branch_att.get_as_string("HCON").unwrap_or_default();
        let mut has_tubi = true;
        if &hconnect == "DUCT" {
            has_tubi = false;
        }
        // dbg!(htube_ref);
        let mut bore = 0.0f32;
        if let Ok(hstube_att) = mgr.get_attr(htube_ref).await {
            // dbg!(&hstube_att);
            let hstube_cat_att = mgr.get_attr(hstube_att.get_foreign_refno("CATR").unwrap_or_default()).await?;
            // dbg!(&hstube_cat_att);
            let params = hstube_cat_att.get_f64_vec("PARA").unwrap_or_default();
            if params.len() >= 2 {
                bore = params[1] as f32;
            }
        }
        let mut current_tubing = PdmsTubing {
            start_pt: bran_htube_pt,
            end_pt: Vec3::ZERO,
            bore,
            finished: false,
        };
        let children = mgr.get_children_refs(branch_refno).await.unwrap_or_default();
        if children.len() == 0 {
            if !current_tubing.finished && bran_ttube_pt.distance(current_tubing.start_pt) > TUBI_TOL {
                current_tubing.end_pt = bran_ttube_pt;
                current_tubing.finished = true;
                brep_shape_map.entry(branch_refno).or_insert(Vec::new()).push(current_tubing.convert_to_shape());
            }
            return Ok(true);
        }
        //第一遍完成后，然后生成tubing
        let last_child = children.last().unwrap().clone();
        for refno in children {
            // if refno != RefU64::from_two_nums(23584, 7410) {
            //     continue;
            // }
            let world_trans = mgr.get_world_transform(refno).await?.unwrap_or_default();
            // dbg!(refno);
            let mut geoms = resolve_desi_comp(refno, mgr.as_ref()).await.unwrap_or_default();
            // dbg!(&geoms);
            let attr = mgr.get_attr(refno).await?;
            //有隐含管段
            if has_tubi {
                if let Some(arrive) = attr.get_i32("ARRI") {
                    if geoms.axis_map.contains_key(&arrive) {
                        let p = &geoms.axis_map[&arrive].pt;
                        let a_pos = world_trans.transform_point3(Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32));
                        if !current_tubing.finished && a_pos.distance(current_tubing.start_pt) > TUBI_TOL {
                            current_tubing.end_pt = a_pos;
                            current_tubing.finished = true;
                            brep_shape_map.entry(refno).or_insert(Vec::new()).push(current_tubing.convert_to_shape());
                        }
                    }
                }
                if let Some(lstube) = attr.get_foreign_refno("LSTU") {
                    if let Ok(lstube_att) = mgr.get_attr(lstube).await {
                        let lstube_cat_att = mgr.get_attr(lstube_att.get_foreign_refno("CATR").unwrap_or_default()).await?;
                        let params = lstube_cat_att.get_f64_vec("PARA").unwrap_or_default();
                        if params.len() >= 2 {
                            current_tubing.bore = params[1] as f32;
                        }
                    }
                }

                if let Some(leave) = attr.get_i32("LEAV") {
                    if geoms.axis_map.contains_key(&leave) {
                        let p = &geoms.axis_map[&leave].pt;
                        let l_pos = world_trans.transform_point3(Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32));
                        current_tubing.start_pt = l_pos;
                        current_tubing.finished = false;
                    }
                }
            }
            //管件的生成
            let GeomsInfo {
                geometries,
                axis_map
            } = geoms;
            let len = geometries.len();
            // let pt_map = &geoms.axis_map;
            for (i, geom) in geometries.into_iter().enumerate() {
                if let Some(cate_shape) = convert_to_brep_shapes(&geom) {
                    brep_shape_map.entry(refno).or_insert(Vec::new()).push(cate_shape);
                }
            }
            refno_ptset_map.insert(refno, axis_map);
            //有隐含管段
            if has_tubi {
                if refno == last_child {
                    if !current_tubing.finished && bran_ttube_pt.distance(current_tubing.start_pt) > TUBI_TOL {
                        current_tubing.end_pt = bran_ttube_pt;
                        current_tubing.finished = true;
                        brep_shape_map.entry(refno).or_insert(Vec::new()).push(current_tubing.convert_to_shape());
                    }
                }
            }
        }
        Ok(true)
    }

    /// 缓存使用元件库的几何体
    pub async fn cache_cata_geos(mgr: Arc<AiosDBManager>, instance_mgr: Arc<PdmsMeshInstanceMgr>, project: &str, mdb: &str,
                                 db_nos: Option<Vec<i32>>, debug_cata_refno: &Option<String>) -> anyhow::Result<bool> {
        let t = Instant::now();
        let mut att_types = vec!["BRAN"];
        att_types.extend_from_slice(&vec![
            "TP",
            // "SPLR",
            // "WELD",
            "FILT",
            "ELCONN",
            "HELE",
            "PCLA",
            // "PANE",
            "CMPF",
            "WALL",
            "SUBE",
            "FIXING",
            // "INST",
            "PJOI",
            "PFIT",
            // "CROS",
            "GWALL",
            // "OLET",
            // "BEND",
            "IDAM",
            // "CLOS",
            "FLOOR",
            "SCLA",
            // "SILE",
            "EQUI",
            // "COUP",
            "GENSEC",
            // "AHU",
            // "TAPE",
            "FLEX",
            // "HACC",
            // "VTWA",
            // "DUCT",
            // "TRNS",
            // "STRT",
            "STWALL",
            // "HFAN",
            // "DAMP",
            //"PAVE",
            "RNODE",
            "PRTELE",
            // "GRIL",
            // "PCOM",
            "FITT",
            "GPART",
            // "THRE",
            // "UNIO",
            "SCREED",
            "NOZZ",
            "PALJ",
            "SUBJ",
            "PLOO",
            "SJOI",
            "CABLE",
            "BATT",
            "CMFI",
            // "MESH",
            // "PLAT",
            "CNODE",
            "SCOJ",
            "SEVE",
            // "FBLI",
            // "STIF",
            "SBFI",
            // "OFST",
            // "BRCO",
            // "SELJ",
            // "CAP",
            "SCTN",
        ]);

        let has_cata_refnos = mgr.get_refnos_by_types(project, &att_types, db_nos).await?;
        let mut handles = vec![];
        // let has_cata_refnos = RefU64Vec(vec![RefU64::from_two_nums(23584, 7381)]);
        let has_cata_cnt = has_cata_refnos.len();
        for (i, refno) in has_cata_refnos.into_iter().enumerate() {
            if let Some(debug_refno) = debug_cata_refno {
                if let Ok(debug_refno) = RefU64::from_refno_str(debug_refno) {
                    if refno != debug_refno {
                        continue;
                    }
                }
            }

            let mgr = mgr.clone();
            let instance_mgr = instance_mgr.clone();
            let handle = tokio::spawn(async move {
                let inst_map = &instance_mgr.inst_mgr;
                let cached_mesh_mgr = &mgr.cached_mesh_mgr;
                let level_shape_mgr = &instance_mgr.level_shape_mgr;
                //在这里直接处理完所有需要处理的transform
                // let attr = mgr.get_implicit_attr(refno, None).await.unwrap_or_default();
                let brep_shapes = CateBrepShapeMap::new();
                let current_att = mgr.get_implicit_attr(refno, None).await.unwrap_or_default();
                let mut refno_ptset_map = DashMap::new();
                if current_att.get_type() == "BRAN" {
                    Self::get_cata_branch_geoms(mgr.clone(), refno, &current_att, &brep_shapes,
                                                &refno_ptset_map).await.unwrap_or_default();
                } else {
                    Self::get_cata_single_geoms(mgr.clone(), refno, &current_att, &brep_shapes,
                                                &refno_ptset_map).await.unwrap_or_default();
                }
                //todo refno_ptset_map 需要存入到数据库
                for (child_refno, shapes) in brep_shapes {
                    let trans_origin = mgr.get_world_transform(child_refno).await.unwrap_or_default().unwrap_or_default();
                    let ancestors = mgr.get_ancestors_refnos_without_world(child_refno);
                    for p_refno in ancestors {
                        level_shape_mgr.entry(p_refno).or_insert(RefU64Vec::default()).push(child_refno);
                    }
                    let mut geos_info = EleGeosInfo {
                        data: vec![],
                        visible: true,
                        generic_type: mgr.get_generic_type(child_refno),
                        world_transform: (trans_origin.rotation, trans_origin.translation, Vec3::ONE),
                        ptset_map: refno_ptset_map.remove(&child_refno).map(|x| x.1).unwrap_or_default(),
                    };
                    let mut geo_insts = &mut geos_info.data;
                    for shape in shapes {
                        //shape 的信息
                        let CateBrepShape {
                            refno,
                            brep_shape,
                            mut transform,
                            visible,
                            is_tubi,
                            pts,
                        } = shape;
                        if !visible || !brep_shape.check_valid() { continue; }
                        let scale = brep_shape.get_trans().scale;
                        if !brep_shape.check_valid() {
                            continue;
                        }
                        let geo_hash = cached_mesh_mgr.get_pdms_mesh_hash_key(brep_shape);
                        // let mut desi_trans = trans_origin.clone();
                        // if !is_tubi {
                        //     desi_trans.translation = desi_trans.translation + desi_trans.rotation * transform.translation;
                        //     desi_trans.rotation = desi_trans.rotation * transform.rotation;
                        // } else {
                        //     desi_trans.translation = transform.translation;
                        //     desi_trans.rotation = transform.rotation;
                        // }
                        // if is_tubi {
                        //     //求相对矩阵，即相对于owner的矩阵
                        //     transform = trans_origin.inverse() * transform;
                        // }
                        let mut bbox = cached_mesh_mgr.get_bbox(&geo_hash).unwrap();
                        bbox.scaled(&scale);
                        //tubi 需要特殊处理
                        let geom_inst = EleGeoInstance {
                            geo_hash,
                            refno,
                            pts,
                            bbox,
                            transform: (transform.rotation, transform.translation, scale),
                            visible,
                            is_tubi,
                        };
                        geo_insts.push(geom_inst);
                    }
                    inst_map.entry(child_refno).or_insert(geos_info);
                }
            });
            handles.push(handle);
            if i == has_cata_cnt - 1 || handles.len() == BATCH_COUNT {
                futures::future::join_all(take(&mut handles)).await;
            }
        }
        dbg!(instance_mgr.inst_mgr.len());
        println!("处理元件库几何体: {} 花费时间: {} ms", has_cata_cnt, t.elapsed().as_millis());
        Ok(true)
    }

    /// 生成基本体的几何数据
    pub async fn cache_prim_geos(mgr: Arc<AiosDBManager>, instance_mgr: Arc<PdmsMeshInstanceMgr>, project: &str, db_nos: Option<Vec<i32>>) -> anyhow::Result<bool> {
        let t = Instant::now();
        let mut prim_refnos = mgr.get_refnos_by_types(project, &GNERAL_PRIM_NOUN_NAMES, db_nos).await?;
        // let test_refno = RefU64::from_two_nums(17788, 18653);
        // prim_refnos = RefU64Vec(vec![test_refno]);
        let prim_cnt = prim_refnos.len();
        let mut handles = vec![];
        //todo 修改 batch 的方式
        for (i, refno) in prim_refnos.into_iter().enumerate() {
            let mgr = mgr.clone();
            let instance_mgr = instance_mgr.clone();
            let handle = tokio::spawn(async move {
                let inst_map = &instance_mgr.inst_mgr;
                let cached_mesh_mgr = &mgr.cached_mesh_mgr;
                let level_shape_mgr = &instance_mgr.level_shape_mgr;
                let transform = mgr.get_world_transform(refno).await.unwrap_or_default().unwrap_or_default();
                let ancestors = mgr.get_ancestors_refnos_without_world(refno);
                for p_refno in ancestors {
                    level_shape_mgr.entry(p_refno).or_insert(RefU64Vec::default()).push(refno);
                }
                let mut geos_info = EleGeosInfo {
                    data: vec![],
                    visible: true,
                    generic_type: mgr.get_generic_type(refno),
                    world_transform: (transform.rotation, transform.translation, Vec3::ONE),
                    ptset_map: default(),
                };
                let mut geo_insts = &mut geos_info.data;
                let mut geo_hash = None;
                let mut item_trans = TransformSRT::default();
                let attr = mgr.get_attr(refno).await.unwrap_or_default();
                if let Some(brep_obj) = attr.create_brep_shape() {
                    if brep_obj.check_valid() {
                        item_trans = brep_obj.get_trans();
                        let r = cached_mesh_mgr.get_pdms_mesh_hash_key(brep_obj);
                        geo_hash = Some(r);
                    }
                }
                let parent_refno = mgr.get_owner(refno);
                if let Some(geo_hash) = geo_hash {
                    let visible = attr.is_visible_by_level(None).unwrap_or(true);
                    let tr: TransformSRT = item_trans;
                    let mut bbox = cached_mesh_mgr.get_bbox(&geo_hash).unwrap();
                    bbox.scaled(&tr.scale);
                    let geom_inst = EleGeoInstance {
                        geo_hash,
                        refno,
                        pts: Default::default(),
                        bbox,
                        transform: (tr.rotation, tr.translation, tr.scale),
                        visible,
                        is_tubi: false,
                    };
                    geo_insts.push(geom_inst);
                    inst_map.entry(refno).or_insert(geos_info);
                }
            });
            handles.push(handle);
            if i == prim_cnt - 1 || handles.len() == BATCH_COUNT {
                futures::future::join_all(take(&mut handles)).await;
            }
        }
        dbg!(instance_mgr.inst_mgr.len());
        println!("处理常规基本几何体: {} 花费时间: {} ms", prim_cnt, t.elapsed().as_millis());
        Ok(true)
    }
    //
    // pub async fn cache_pohe_geos(mgr: Arc<AiosDBManager>, project: &str) -> anyhow::Result<bool> {
    //     let t = Instant::now();
    //     let pohe_refnos = mgr.get_refnos_by_types(project, &vec!["POHE"], Option::from(vec![1])).await?;
    //     let pohe_cnt = pohe_refnos.len();
    //     let mut handles = vec![];
    //     for (i, refno) in pohe_refnos.into_iter().enumerate() {
    //         let mgr = mgr.clone();
    //         let handle = tokio::spawn(async move {
    //             let inst_map = &mgr.mesh_mgr.inst_mgr;
    //             let cached_mesh_mgr = &mgr.mesh_mgr.cached_mesh_mgr;
    //             //在这里直接处理完所有需要处理的transform
    //             let transform = mgr.get_world_transform(refno).await.unwrap_or_default().unwrap_or_default();
    //             let mut geo_hash = None;
    //             let mut item_trans = TransformSRT::default();
    //             let mut facet = Facet::default();
    //             if let Ok(children_refs) = mgr.get_children_refs(refno).await {
    //                 for pogo_ref in children_refs {
    //                     let mut vertices: Vec<[f32; 3]> = vec![];
    //                     let mut tv = vec![];
    //                     if let Ok(p_refs) = mgr.get_children_refs(pogo_ref).await {
    //                         let v_cnt = p_refs.len();
    //                         if v_cnt >= 3 {
    //                             for x in p_refs {
    //                                 //todo 后面需要做错误处理
    //                                 let v = mgr.get_implicit_attr(x, Some(vec!["POS"])).await.unwrap_or_default().get_position().unwrap_or_default();
    //                                 vertices.push([v[0], v[1], v[2]]);
    //                                 if tv.len() < 3 {
    //                                     tv.push(v);
    //                                 }
    //                             }
    //                             let n = (tv[1] - tv[0]).cross(tv[2] - tv[1]).normalize();
    //                             let mut polygon = Polygon {
    //                                 contours: vec![Contour {
    //                                     vertices,
    //                                     normals: vec![n.into(); v_cnt],
    //                                 }]
    //                             };
    //                             facet.polygons.push(polygon);
    //                         }
    //                     }
    //                 }
    //             }
    //             if facet.check_valid() {
    //                 item_trans = facet.get_trans();
    //                 let r = cached_mesh_mgr.get_pdms_mesh_hash_key(Box::new(facet));
    //                 geo_hash = Some(r);
    //             }
    //
    //             let parent_refno = mgr.get_owner(refno);
    //             let mut parent_att = mgr.get_implicit_attr(parent_refno, Some(vec!["LEVE"])).await.unwrap_or_default();
    //             if let Some(geo_hash) = geo_hash {
    //                 let visible = parent_att.is_visible_by_level(None).unwrap_or(true);
    //                 let tr: TransformSRT = item_trans * transform;
    //                 let mut bbox = cached_mesh_mgr.get_bbox(&geo_hash).unwrap();
    //                 bbox.scaled(&tr.scale);
    //                 let geom_data = EleGeoInstance {
    //                     geo_hash,
    //                     bbox,
    //                     global_transform: (tr.rotation, tr.translation, tr.scale),
    //                     visible,
    //                     generic_type: "STRU".to_string(),  //todo add generic type
    //                     zone_refno: refno,
    //                 };
    //                 inst_map.entry(parent_refno).or_insert(Vec::new()).push(geom_data);
    //             }
    //         });
    //         handles.push(handle);
    //         if i == pohe_cnt - 1 || handles.len() == 100 {
    //             futures::future::join_all(take(&mut handles)).await;
    //         }
    //     }
    //     println!("处理POHE几何体: {} 花费时间: {} ms", pohe_cnt, t.elapsed().as_millis());
    //     Ok(true)
    // }
    //

    pub async fn cache_loop_geos(mgr: Arc<AiosDBManager>, instance_mgr: Arc<PdmsMeshInstanceMgr>, project: &str, db_nos: Option<Vec<i32>>) -> anyhow::Result<bool> {
        let t = Instant::now();
        let loop_refnos = mgr.get_refnos_by_types(project, &vec!["PLOO", "LOOP"], db_nos).await?;
        let loop_cnt = loop_refnos.len();
        //处理loop elements
        let mut handles = vec![];
        for (i, refno) in loop_refnos.into_iter().enumerate() {
            let mgr = mgr.clone();
            let instance_mgr = instance_mgr.clone();
            let handle = tokio::spawn(async move {
                let inst_map = &instance_mgr.inst_mgr;
                let cached_mesh_mgr = &mgr.cached_mesh_mgr;

                let level_shape_mgr = &instance_mgr.level_shape_mgr;
                let transform = mgr.get_world_transform(refno).await.unwrap_or_default().unwrap_or_default();
                let ancestors = mgr.get_ancestors_refnos_without_world(refno);
                for p_refno in ancestors {
                    level_shape_mgr.entry(p_refno).or_insert(RefU64Vec::default()).push(refno);
                }
                let mut geos_info = EleGeosInfo {
                    data: vec![],
                    visible: true,
                    world_transform: (transform.rotation, transform.translation, Vec3::ONE),
                    generic_type: mgr.get_generic_type(refno),
                    ptset_map: default(),
                };
                let mut geo_insts = &mut geos_info.data;

                if let Some(refno_basic) = mgr.get_refno_basic(refno) {
                    let parent_basic = mgr.get_owner_ref_basic(refno).unwrap();
                    let parent_type = parent_basic.get_type();
                    let parent_refno = refno_basic.get_owner();
                    let mut loop_verts: Vec<Vec3> = vec![];
                    let mut fradius_vec: Vec<f32> = vec![];
                    if let Ok(children_refs) = mgr.get_children_refs(refno).await {
                        for x in children_refs {
                            if let Ok(a) = mgr.get_implicit_attr(x, Some(vec!["POS", "FRAD"])).await {
                                loop_verts.push(a.get_position().unwrap_or_default());
                                fradius_vec.push(a.get_f32("FRAD").unwrap_or_default());
                            }
                        }
                    }
                    let mut parent_att = AttrMap::default();
                    let mut geo_hash = None;
                    let mut item_trans = TransformSRT::default();
                    match parent_type {
                        "REVO" => {
                            parent_att = mgr.get_implicit_attr(parent_refno, Some(vec!["ANGL", "LEVE"])).await.unwrap_or_default();
                            let angle = parent_att.get_f32("ANGL").unwrap_or_default();
                            if angle >= f32::EPSILON {
                                let revo = Box::new(Revolution {
                                    loop_verts,
                                    angle,
                                    ..Default::default()
                                });
                                if revo.check_valid() {
                                    item_trans = revo.get_trans();
                                    geo_hash = Some(cached_mesh_mgr.get_pdms_mesh_hash_key(revo));
                                }
                            }
                        }
                        "EXTR" | "PANE" | "FLOOR" | "SCREED" => {
                            let attr = mgr.get_implicit_attr(refno, Some(vec!["HEIG"])).await.unwrap_or_default();
                            parent_att = mgr.get_implicit_attr(parent_refno, Some(vec!["SJUS", "HEIG"])).await.unwrap_or_default();
                            let mut height = attr.get_f32("HEIG").unwrap_or(parent_att.get_f32("HEIG").unwrap_or_default());
                            let i: usize = 0;
                            let extrusion = Box::new(Extrusion {
                                verts: loop_verts,
                                height,
                                fradius_vec,
                                ..Default::default()
                            });
                            if extrusion.check_valid() {
                                item_trans = extrusion.get_trans();
                                if let Some(sjus) = attr.get_str("SJUS") {
                                    if sjus == "UTOP" || sjus == "DTOP" {
                                        item_trans.translation = item_trans.translation + Vec3::new(0.0, 0.0, -height);
                                    }
                                }
                                let r = cached_mesh_mgr.get_pdms_mesh_hash_key(extrusion);
                                geo_hash = Some(r);
                            }
                        }
                        _ => {}
                    }

                    if let Some(geo_hash) = geo_hash {
                        let visible = parent_att.is_visible_by_level(None).unwrap_or(true);
                        let tr: TransformSRT = item_trans;
                        let mut bbox = cached_mesh_mgr.get_bbox(&geo_hash).unwrap();
                        bbox.scaled(&tr.scale);

                        let geom_inst = EleGeoInstance {
                            geo_hash,
                            refno,
                            pts: Default::default(),
                            bbox,
                            transform: (tr.rotation, tr.translation, tr.scale),
                            visible,
                            is_tubi: false,
                        };
                        geo_insts.push(geom_inst);
                    }
                }
                inst_map.entry(refno).or_insert(geos_info);
            });
            handles.push(handle);
            if i == loop_cnt - 1 || handles.len() == BATCH_COUNT {
                futures::future::join_all(take(&mut handles)).await;
            }
        }
        dbg!(instance_mgr.inst_mgr.len());
        println!("处理loops几何体: {} 花费时间: {} ms", loop_cnt, t.elapsed().as_millis());
        Ok(true)
    }

    /// 生成模型
    pub async fn cache_geos_data(mgr: Arc<AiosDBManager>, db_option: DbOption) -> anyhow::Result<bool> {
        let mut time = Instant::now();
        let project = &db_option.project_name;
        let mdb = &db_option.mdb_name;
        let mut db_nos = db_option.manual_db_nums.clone().unwrap_or_default();
        let debug_cata_refno = db_option.debug_cata_refnos;
        if db_nos.is_empty() {
            let url = AiosDBManager::get_default_conn_str(&mgr.db_option);
            let info_pool = AiosDBManager::get_db_pool(
                &url, format!("PDMS_INFO_DB_{}", mgr.db_option.project_name.to_uppercase()).as_str()).await?;
            let pool = AiosDBManager::get_db_pool(&url, project).await?;
            let mdb_dbnos_map = query_mdb_dbnos(&pool, &info_pool).await?;
            let key_str = format!("/{mdb}");
            if mdb_dbnos_map.contains_key(&key_str) {
                db_nos = mdb_dbnos_map.get(&key_str).unwrap().get("DESI").cloned().unwrap_or_default();
            }
        }
        dbg!(&db_nos);
        for db_no in db_nos {

            let instance_mgr = Arc::new(PdmsMeshInstanceMgr::default());

            Self::cache_prim_geos(mgr.clone(), instance_mgr.clone(), project, Some(vec![db_no])).await?;
            Self::cache_loop_geos(mgr.clone(), instance_mgr.clone(), project, Some(vec![db_no])).await?;
            // Self::cache_pohe_geos(mgr.clone(), project).await?;
            Self::cache_cata_geos(mgr.clone(), instance_mgr.clone(), project, mdb, Some(vec![db_no]), &debug_cata_refno).await?;

            mgr.mesh_instance_mgr.insert(db_no, Arc::try_unwrap(instance_mgr).unwrap());
            println!("{db_no} 生成完毕。");
        }


        println!("cache all geoms costs: {}ms", time.elapsed().as_millis());
        Ok(true)
    }

    /// 获取缓存好的site
    pub fn get_cached_site_nodes(&self, world_refno: RefU64) -> anyhow::Result<Vec<PdmsElement>> {
        if let Some(k) = CACHED_MDB_SITE_MAP.get(&world_refno) {
            return Ok(k.value().0.clone());
        }
        Ok(vec![])
    }
}


#[tokio::test]
async fn test_get_attr() -> anyhow::Result<()> {
    let mut mgr = AiosDBManager::init_form_config().await?;
    let refno: RefU64 = RefI32Tuple((23584, 8)).into();
    let v = mgr.get_attr(refno).await?;
    println!("v={:?}", v.to_string_hashmap());

    // mgr.cache_geos_data("Sample", "SAMPLE").await?;

    Ok(())
}

#[tokio::test]
async fn test_get_children_attr() -> anyhow::Result<()> {
    let mgr = AiosDBManager::init_form_config().await?;
    let refno: RefU64 = RefI32Tuple((23584, 7)).into();
    let v = mgr.get_children_attrs(refno).await?;
    println!("v={:?}", v);
    Ok(())
}
