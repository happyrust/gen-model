use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::mem::take;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use aios_core::consts::*;
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
use dashmap::{DashMap, DashSet};
use glam::{Quat, quat, TransformRT, TransformSRT, Vec3};
use id_tree::{Node, NodeId};
use smol_str::SmolStr;
use once_cell::sync::Lazy;
use sqlx::{MySql, MySqlPool, Pool};
use crate::api::attr::*;
use crate::api::element::*;
use crate::data_interface::interface::PdmsDataInterface;
use crate::consts::*;
use crate::options::DbOption;
use async_trait::async_trait;

pub const TUBI_TOL: f32 = 10.0f32;

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

    pub mesh_mgr: Arc<PdmsMeshMgr>,

    cached_refno_basic_map: Arc<DashMap<RefU64, CachedRefBasic>>,    //缓存到本地数据库

    cached_world_transforms_map: Arc<DashMap<RefU64, TransformRT>>,   //记录所有需要记录的world transform, need to flush to database
}

#[async_trait]
impl PdmsDataInterface for AiosDBManager {
    async fn get_attr(&self, refno: RefU64) -> anyhow::Result<AttrMap> {
        if let Some(project_pool) = self.get_project_pool(refno) {
            if let Some(ref_basic) = self.get_refno_basic(refno) {
                let attr = query_full_attr(refno, ref_basic.value(), &project_pool, None).await?;
                return Ok(attr);
            }
        }
        Ok(AttrMap::default())
    }

    ///获取owner的参考号，从缓存读取
    #[inline]
    fn get_owner(&self, refno: RefU64) -> RefU64 {
        self.cached_refno_basic_map.get(&refno)
            .map(|x| x.value().get_owner()).unwrap_or_default()
    }

    async fn get_implicit_attr(&self, refno: RefU64, columns: Option<Vec<&str>>) -> anyhow::Result<AttrMap> {
        if let Some(project_pool) = self.get_project_pool(refno) {
            if let Some(ref_basic) = self.get_refno_basic(refno) {
                let attr = query_implicit_attr(refno, ref_basic.value(), &project_pool, columns).await?;
                return Ok(attr);
            }
        }
        Ok(AttrMap::default())
    }

    async fn get_implicit_attrs_by_owner(&self, owner: RefU64, type_name: &str, columns: Option<Vec<&str>>) -> anyhow::Result<Vec<AttrMap>> {
        if let Some(project_pool) = self.get_project_pool(owner) {
            let attr = query_implicit_attrs_by_owner(owner, type_name, &project_pool, columns).await?;
            return Ok(attr);
        }
        Ok(vec![])
    }

    async fn get_parent_attr(&self, refno: RefU64) -> anyhow::Result<AttrMap> {
        // if let Some(project_pool) = self.get_project_pool(refno) {
        //     let attr = query_parent_attr(refno, &project_pool, None).await?;
        //     return Ok(attr);
        // }
        Ok(AttrMap::default())
    }

    #[inline]
    fn get_refno_basic(&self, refno: RefU64) -> Option<Ref<RefU64, CachedRefBasic>> {
        if !refno.is_valid() {
            None
        } else {
            self.cached_refno_basic_map.get(&refno)
        }
    }

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

    //包含自己
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
        self.cached_refno_basic_map.get(&refno)
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
        MySqlPool::connect(&format!("{connection_str}/{project}")).await.map_err(|x| anyhow!(x.to_string()))
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
        let cached_refno_basic_map: Arc<DashMap<RefU64, CachedRefBasic>> = Arc::new(Default::default());
        // let process_stats = ProcessStats::get().await.expect("could not get stats for running process");
        // println!("{:?}", process_stats);
        let time = Instant::now();
        for project in &db_option.included_projects {
            let project_pool = AiosDBManager::get_db_pool(&default_conn, project).await;
            match project_pool {
                Ok(pool) => {
                    //暂时保存在内存，需要序列化到heed LMDB数据库
                    sync_refno_basic_map(&pool, cached_refno_basic_map.clone()).await?;
                    project_map.entry(project.clone()).or_insert(pool);
                }
                Err(_) => { println!("project: {} init failed",project); }
            }
        }
        println!("缓存RefBasic数据花费：{}ms", time.elapsed().as_millis());
        dbg!(cached_refno_basic_map.len());
        // let process_stats = ProcessStats::get().await.expect("could not get stats for running process");
        // println!("{:?}", process_stats);

        let info_conn = AiosDBManager::get_db_pool(&default_conn, PDMS_INFO_DB).await?;
        let ref0_map = get_ref0_map(&info_conn).await?;
        // let cached_refno_type_map = get_refno_table_map(&project);
        let projects = db_option.included_projects.clone();
        Ok(
            Self {
                project_map,
                ref0_map,
                info_pool:info_conn,
                projects,
                needed_parse_files: None,
                project_path: dir,
                db_option,
                mesh_mgr: Arc::new(Default::default()),
                cached_refno_basic_map,
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

    ///获取单个元件的模型数据
    pub async fn get_cata_single_geoms(mgr: Arc<AiosDBManager>, design_refno: RefU64, result_map: &CateBrepShapeMap) -> anyhow::Result<bool> {
        let desi_att = mgr.get_implicit_attr(design_refno, None).await?;
        let type_name = desi_att.get_type();
        let is_bran = type_name == "BRAN";
        if !is_bran {
            return Ok(false);
        }
        let geoms = resolve_desi_comp(design_refno, mgr.as_ref()).await.unwrap_or_default();
        if type_name == "SCTN" || type_name == "STWALL" || type_name == "GENSEC" {
            create_st_geos(design_refno, &desi_att, &geoms, &result_map, mgr.as_ref()).await?;
        } else {
            for geom in geoms.geometries {
                if let Some(cate_shape) = convert_to_brep_shapes(&geom) {
                    result_map.entry(design_refno).or_insert(Vec::new()).push(cate_shape);
                }
            }
        }

        Ok(true)
    }

    ///获得branch的模型数据
    pub async fn get_cata_branch_geoms(mgr: Arc<AiosDBManager>, branch_refno: RefU64, result_map: &CateBrepShapeMap) -> anyhow::Result<bool> {
        let branch_att = mgr.get_implicit_attr(branch_refno, None).await?;
        let type_name = branch_att.get_type();
        if type_name != "BRAN" {
            return Ok(false);
        }
        // dbg!(design_refno.to_refno_str());
        let bran_transform = mgr.get_world_transform(branch_refno).await?.unwrap_or_default();
        // dbg!(bran_transform);
        let bran_htube_pt = bran_transform.transform_point3(branch_att.get_vec3("HPOS").ok_or(anyhow!("HPOS not exist".to_string()))?);
        let bran_ttube_pt = bran_transform.transform_point3(branch_att.get_vec3("TPOS").ok_or(anyhow!("TPOS not exist".to_string()))?);
        let htube_ref = branch_att.get_foreign_refno("HSTU").unwrap_or_default();
        // dbg!(htube_ref);
        // dbg!(mgr.ref0_map.get(&htube_ref.get_0()));
        let mut bore = 0.0f32;
        if let Ok(hstube_att) = mgr.get_attr(htube_ref).await {
            // dbg!(hstube_att.to_string_hashmap());
            let hstube_cat_att = mgr.get_attr(hstube_att.get_foreign_refno("CATR").unwrap_or_default()).await?;
            // dbg!(hstube_cat_att.to_string_hashmap());
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
                result_map.entry(branch_refno).or_insert(Vec::new()).push(current_tubing.convert_to_shape());
            }
            return Ok(true);
        }
        //第一遍完成后，然后生成tubing
        let last_child = children.last().unwrap().clone();
        for refno in children {
            let world_trans = mgr.get_world_transform(refno).await?.unwrap_or_default();
            let geoms = resolve_desi_comp(refno, mgr.as_ref()).await.unwrap_or_default();
            let attr = mgr.get_attr(refno).await?;
            if let Some(arrive) = attr.get_i32("ARRI") {
                if geoms.axis_map.contains_key(&arrive) {
                    let p = &geoms.axis_map[&arrive].pt;
                    let a_pos = world_trans.transform_point3(Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32));
                    if !current_tubing.finished && a_pos.distance(current_tubing.start_pt) > TUBI_TOL {
                        current_tubing.end_pt = a_pos;
                        current_tubing.finished = true;
                        result_map.entry(branch_refno).or_insert(Vec::new()).push(current_tubing.convert_to_shape());
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
            //管件的生成
            for geom in geoms.geometries {
                if let Some(cate_shape) = convert_to_brep_shapes(&geom) {
                    result_map.entry(refno).or_insert(Vec::new()).push(cate_shape);
                    // break;
                }
            } // end geoms.geometries
            if refno == last_child {
                if !current_tubing.finished && bran_ttube_pt.distance(current_tubing.start_pt) > TUBI_TOL {
                    current_tubing.end_pt = bran_ttube_pt;
                    current_tubing.finished = true;
                    result_map.entry(branch_refno).or_insert(Vec::new()).push(current_tubing.convert_to_shape());
                }
            }
        }
        Ok(true)
    }

    pub async fn cache_cata_geos(mgr: Arc<AiosDBManager>, project: &str, mdb: &str) -> anyhow::Result<bool> {
        let t = Instant::now();
        // let has_cata_types = ATTR_INFO_MAP.get_has_cat_ref_types().iter().map(|x| x.clone()).collect::<Vec<_>>();
        // dbg!(&has_cata_types);
        // let has_cata_types = has_cata_types.iter().map(|x| x.as_str()).collect();
        // dbg!(&has_cata_types);
        // let dbnos = query_mdb_dbnos_by_name("Sample").await?;
        let url = AiosDBManager::get_default_conn_str(&mgr.db_option);
        let info_pool = AiosDBManager::get_db_pool(&url,"PDMS_INFO_DB").await?;
        let pool = AiosDBManager::get_db_pool(&url,"SAMPLE").await?;
        let mdb_dbnos_map = query_mdb_dbnos(&pool, &info_pool).await?;

        let mut dbnos = None;

        // dbg!(&mdb_dbnos_map);
        let key_str = format!("/{mdb}");
        if mdb_dbnos_map.contains_key(&key_str) {
            dbnos = mdb_dbnos_map.get(&key_str).unwrap().get("DESI").cloned();
        }
        dbg!(&dbnos);

        let hash_cata_refnos = mgr.get_refnos_by_types(project, &vec!["BRAN"], dbnos).await?;
        let mut handles = vec![];
        // let hash_cata_refnos = RefU64Vec(vec![RefU64::from_two_nums(23584, 5495)]);
        let has_cata_cnt = hash_cata_refnos.len();
        for (i, refno) in hash_cata_refnos.into_iter().enumerate() {
            let mgr = mgr.clone();
            let handle = tokio::spawn(async move {
                let inst_map = &mgr.mesh_mgr.inst_mgr;
                let cached_mesh_mgr = &mgr.mesh_mgr.cached_mesh_mgr;
                //在这里直接处理完所有需要处理的transform


                let mut item_trans = TransformSRT::default();
                // let attr = mgr.get_implicit_attr(refno, None).await.unwrap_or_default();
                let brep_shapes = CateBrepShapeMap::new();
                Self::get_cata_branch_geoms(mgr.clone(), refno, &brep_shapes).await.unwrap_or_default();
                // Self::get_cata_single_geoms(mgr.clone(), refno, &brep_shapes).await.unwrap_or_default();
                // dbg!(&brep_shapes);
                for (child_refno, shapes) in brep_shapes {
                    let trans_origin = mgr.get_world_transform(child_refno).await.unwrap_or_default().unwrap_or_default();
                    //记录对应的不同颜色类型
                    // if let Some((noun_name, r)) = mgr.get_general_type_refno(d.refno) {
                    //     type_geom_refs_map.entry(r).or_insert(Vec::new()).push(d.refno);
                    //     type_refs_map.entry(noun_name.clone()).or_insert(HashSet::new()).insert(r);
                    //     color_type = Some(noun_name);
                    // }
                    //维护每个节点有那些几何实例
                    // let ancestors = tree.ancestors(&cur_node_id).unwrap();
                    // for ancestor in ancestors {
                    //     let p_refno = ancestor.data().refno;
                    //     level_shape_mgr.entry(p_refno).or_insert(RefU64Vec::default()).push(cur_refno);
                    // }
                    //当前自身也要加进去
                    // if d.refno != cur_refno {
                    //     level_shape_mgr.entry(d.refno).or_insert(RefU64Vec::default()).push(cur_refno);
                    // }
                    for shape in shapes {
                        let CateBrepShape {
                            brep_shape,
                            mut transform,
                            visible,
                            is_tubing,
                        } = shape;
                        if !visible || !brep_shape.check_valid() { continue; }
                        item_trans = brep_shape.get_trans();
                        if !brep_shape.check_valid() {
                            continue;
                        }
                        let geo_hash = cached_mesh_mgr.get_pdms_mesh_hash_key(brep_shape);
                        let mut desi_trans = trans_origin.clone();
                        if !is_tubing {
                            desi_trans.translation = desi_trans.translation + desi_trans.rotation * transform.translation;
                            desi_trans.rotation = desi_trans.rotation * transform.rotation;
                        } else {
                            desi_trans.translation = transform.translation;
                            desi_trans.rotation = transform.rotation;
                        }
                        let mut bbox = cached_mesh_mgr.get_bbox(&geo_hash).unwrap();
                        bbox.scaled(&item_trans.scale);
                        let geom_data = EleGeoInstData {
                            geo_hash,
                            bbox,
                            global_transform: (desi_trans.rotation, desi_trans.translation, item_trans.scale),
                            visible: true,
                            generic_type: "PIPE".to_string(), //color_type.clone().unwrap_or_default(),
                            zone_refno: refno,
                            //mgr.get_parent_att_by_type(cur_refno, "ZONE")?.map(|x| x.get_refno().unwrap_or_default()).unwrap_or_default(),
                        };
                        inst_map.entry(child_refno).or_insert(Vec::new()).push(geom_data);
                    }
                }
            });
            handles.push(handle);
            if i == has_cata_cnt - 1 || handles.len() == 100 {
                futures::future::join_all(take(&mut handles)).await;
            }
        }
        dbg!(mgr.mesh_mgr.inst_mgr.len());
        println!("处理常规基本几何体: {} 花费时间: {} ms", has_cata_cnt, t.elapsed().as_millis());
        Ok(true)
    }

    pub async fn cache_prim_geos(mgr: Arc<AiosDBManager>, project: &str) -> anyhow::Result<bool> {
        let t = Instant::now();
        let mut prim_refnos = mgr.get_refnos_by_types(project, &GNERAL_PRIM_NOUN_NAMES, Some(vec![7200])).await?;
        // let test_refno = RefU64::from_two_nums(23584, 2705);
        // prim_refnos = RefU64Vec(vec![test_refno]);
        let prim_cnt = prim_refnos.len();
        let mut handles = vec![];
        for (i, refno) in prim_refnos.into_iter().enumerate() {
            let mgr = mgr.clone();
            let handle = tokio::spawn(async move {
                let inst_map = &mgr.mesh_mgr.inst_mgr;
                let cached_mesh_mgr = &mgr.mesh_mgr.cached_mesh_mgr;
                //在这里直接处理完所有需要处理的transform
                let transform = mgr.get_world_transform(refno).await.unwrap_or_default().unwrap_or_default();
                // dbg!(&transform);
                let mut geo_hash = None;
                let mut item_trans = TransformSRT::default();
                let attr = mgr.get_implicit_attr(refno, None).await.unwrap_or_default();
                if let Some(brep_obj) = attr.create_brep_shape() {
                    if brep_obj.check_valid() {
                        item_trans = brep_obj.get_trans();
                        let r = cached_mesh_mgr.get_pdms_mesh_hash_key(brep_obj);
                        geo_hash = Some(r);
                    }
                }

                let parent_refno = mgr.get_owner(refno);
                // let mut parent_att = mgr.get_implicit_attr(parent_refno, Some(vec!["LEVE"])).await.unwrap_or_default();
                if let Some(geo_hash) = geo_hash {
                    let visible = attr.is_visible_by_level(None).unwrap_or(true);
                    // dbg!(&visible);
                    // dbg!(item_trans);
                    //后面要保留两个 transform，一个是最后拉伸后的几何体的，一个是原本的transform
                    let tr: TransformSRT = item_trans * transform;
                    let mut bbox = cached_mesh_mgr.get_bbox(&geo_hash).unwrap();
                    bbox.scaled(&tr.scale);
                    let geom_data = EleGeoInstData {
                        geo_hash,
                        bbox,
                        global_transform: (tr.rotation, tr.translation, tr.scale),
                        visible,
                        generic_type: "STRU".to_string(),  //todo add generic type
                        zone_refno: parent_refno,
                    };
                    inst_map.entry(refno).or_insert(Vec::new()).push(geom_data);
                }
            });
            handles.push(handle);
            if i == prim_cnt - 1 || handles.len() == 100 {
                futures::future::join_all(take(&mut handles)).await;
            }
        }
        dbg!(mgr.mesh_mgr.inst_mgr.len());
        println!("处理常规基本几何体: {} 花费时间: {} ms", prim_cnt, t.elapsed().as_millis());
        Ok(true)
    }

    pub async fn cache_pohe_geos(mgr: Arc<AiosDBManager>, project: &str) -> anyhow::Result<bool> {
        let t = Instant::now();
        let pohe_refnos = mgr.get_refnos_by_types(project, &vec!["POHE"], Option::from(vec![7200])).await?;
        let pohe_cnt = pohe_refnos.len();
        let mut handles = vec![];
        for (i, refno) in pohe_refnos.into_iter().enumerate() {
            let mgr = mgr.clone();
            let handle = tokio::spawn(async move {
                let inst_map = &mgr.mesh_mgr.inst_mgr;
                let cached_mesh_mgr = &mgr.mesh_mgr.cached_mesh_mgr;
                //在这里直接处理完所有需要处理的transform
                let transform = mgr.get_world_transform(refno).await.unwrap_or_default().unwrap_or_default();
                let mut geo_hash = None;
                let mut item_trans = TransformSRT::default();
                let mut facet = Facet::default();
                if let Ok(children_refs) = mgr.get_children_refs(refno).await {
                    for pogo_ref in children_refs {
                        let mut vertices: Vec<[f32; 3]> = vec![];
                        let mut tv = vec![];
                        if let Ok(p_refs) = mgr.get_children_refs(pogo_ref).await {
                            let v_cnt = p_refs.len();
                            if v_cnt >= 3 {
                                for x in p_refs {
                                    //todo 后面需要做错误处理
                                    let v = mgr.get_implicit_attr(x, Some(vec!["POS"])).await.unwrap_or_default().get_position().unwrap_or_default();
                                    vertices.push([v[0], v[1], v[2]]);
                                    if tv.len() < 3 {
                                        tv.push(v);
                                    }
                                }
                                let n = (tv[1] - tv[0]).cross(tv[2] - tv[1]).normalize();
                                let mut polygon = Polygon {
                                    contours: vec![Contour {
                                        vertices,
                                        normals: vec![n.into(); v_cnt],
                                    }]
                                };
                                facet.polygons.push(polygon);
                            }
                        }
                    }
                }
                if facet.check_valid() {
                    item_trans = facet.get_trans();
                    let r = cached_mesh_mgr.get_pdms_mesh_hash_key(Box::new(facet));
                    geo_hash = Some(r);
                }

                let parent_refno = mgr.get_owner(refno);
                let mut parent_att = mgr.get_implicit_attr(parent_refno, Some(vec!["LEVE"])).await.unwrap_or_default();
                if let Some(geo_hash) = geo_hash {
                    let visible = parent_att.is_visible_by_level(None).unwrap_or(true);
                    let tr: TransformSRT = item_trans * transform;
                    let mut bbox = cached_mesh_mgr.get_bbox(&geo_hash).unwrap();
                    bbox.scaled(&tr.scale);
                    let geom_data = EleGeoInstData {
                        geo_hash,
                        bbox,
                        global_transform: (tr.rotation, tr.translation, tr.scale),
                        visible,
                        generic_type: "STRU".to_string(),  //todo add generic type
                        zone_refno: refno,
                    };
                    inst_map.entry(parent_refno).or_insert(Vec::new()).push(geom_data);
                }
            });
            handles.push(handle);
            if i == pohe_cnt - 1 || handles.len() == 100 {
                futures::future::join_all(take(&mut handles)).await;
            }
        }
        println!("处理POHE几何体: {} 花费时间: {} ms", pohe_cnt, t.elapsed().as_millis());
        Ok(true)
    }

    pub async fn cache_loop_geos(mgr: Arc<AiosDBManager>, project: &str) -> anyhow::Result<bool> {
        let t = Instant::now();
        let loop_refnos = mgr.get_refnos_by_types(project, &vec!["PLOO", "LOOP"], Option::from(vec![7200])).await?;
        let loop_cnt = loop_refnos.len();
        //最好是批量取数据，而不是循环去取
        //处理loop elements
        let mut handles = vec![];
        for (i, refno) in loop_refnos.into_iter().enumerate() {
            let mgr = mgr.clone();
            let handle = tokio::spawn(async move {
                let inst_map = &mgr.mesh_mgr.inst_mgr;
                let cached_mesh_mgr = &mgr.mesh_mgr.cached_mesh_mgr;
                //在这里直接处理完所有需要处理的transform
                let transform = mgr.get_world_transform(refno).await.unwrap_or_default().unwrap_or_default();
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
                    // dbg!(parent_type);
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
                            //todo 添加方法，根据type，判断是否有哪些字段，没有的话，就默认给一个空类型
                            let attr = mgr.get_implicit_attr(refno, Some(vec!["HEIG"])).await.unwrap_or_default();
                            parent_att = mgr.get_implicit_attr(parent_refno, Some(vec!["SJUS", "HEIG"])).await.unwrap_or_default();
                            let mut height = attr.get_f32("HEIG").unwrap_or(parent_att.get_f32("HEIG").unwrap_or_default());
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
                        let tr: TransformSRT = item_trans * transform;
                        let mut bbox = cached_mesh_mgr.get_bbox(&geo_hash).unwrap();
                        bbox.scaled(&tr.scale);
                        let geom_data = EleGeoInstData {
                            geo_hash,
                            bbox,
                            global_transform: (tr.rotation, tr.translation, tr.scale),
                            visible,
                            generic_type: "STRU".to_string(),  //todo add generic type
                            zone_refno: parent_refno,
                        };
                        inst_map.entry(parent_refno).or_insert(Vec::new()).push(geom_data);
                    }
                }
            });
            handles.push(handle);
            if i == loop_cnt - 1 || handles.len() == 100 {
                futures::future::join_all(take(&mut handles)).await;
            }
        }
        dbg!(mgr.mesh_mgr.inst_mgr.len());
        println!("处理loops几何体: {} 花费时间: {} ms", loop_cnt, t.elapsed().as_millis());
        Ok(true)
    }

    /// 生成模型
    pub async fn cache_geos_data(mgr: Arc<AiosDBManager>, project: &str, mdb: &str) -> anyhow::Result<bool> {
        let mut time = Instant::now();
        // Self::cache_prim_geos(mgr.clone(), project).await?;
        // Self::cache_loop_geos(mgr.clone(), project).await?;
        // Self::cache_pohe_geos(mgr.clone(), project).await?;
        Self::cache_cata_geos(mgr.clone(), project, mdb).await?;
        println!("cache all geoms costs: {}ms", time.elapsed().as_millis());
        Ok(true)
    }
}


use config::{Config, ConfigError, Environment, File};
use dashmap::mapref::one::Ref;
use crate::api::refno_info::{get_ref0_map, sync_refno_basic_map};
use crate::ATTR_INFO_MAP;
use crate::cata::query_cata::resolve_desi_comp;
use crate::cata::sctn;
use crate::cata::sctn::geo::create_st_geos;
use crate::data_interface::defines::CachedRefBasic;
use crate::helper::qualified_table_name;

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
