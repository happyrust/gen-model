use aios_core::cache::mgr::*;
use aios_core::cache::refno::*;
use aios_core::consts::*;
use aios_core::db_number::DbNumMgr;
use aios_core::parsed_data::geo_params_data::CateGeoParam::*;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam::*;
use aios_core::parsed_data::{CateAxisParam, GeomsInfo};
use aios_core::pdms_types::*;
use aios_core::prim_geo::category::{convert_to_brep_shapes, CateBrepShape};
use aios_core::prim_geo::extrusion::Extrusion;
use aios_core::prim_geo::facet::{Contour, Facet, Polygon};
use aios_core::prim_geo::revolution::Revolution;
use aios_core::prim_geo::tubing::{PdmsTubing, TubiEdgeAql};
use aios_core::prim_geo::wire::CurveType;
use aios_core::shape::pdms_shape::{BrepShapeTrait, PdmsMesh, VerifiedShape};
use aios_core::tool::db_tool::db1_hash;
use aios_core::tool::math_tool;
use anyhow::anyhow;
use approx::{abs_diff_eq, abs_diff_ne};
use arangors_lite::{Connection, Database};
use async_trait::async_trait;
use bevy::prelude::{dbg, Transform};
use config::{Config, ConfigError, Environment, File};
use dashmap::mapref::one::Ref;
use dashmap::{DashMap, DashSet};
use glam::{quat, EulerRot, Mat3, Quat, Vec2, Vec3};
use id_tree::{Node, NodeId};
use itertools::Itertools;
use lazy_static::lazy_static;
use nalgebra::{Quaternion, UnitQuaternion};
use once_cell::sync::Lazy;
use parry3d::bounding_volume::{aabb::Aabb, BoundingVolume};
use parry3d::math::{Isometry, Vector};
use smol_str::SmolStr;
use sqlx::pool::PoolOptions;
use sqlx::{Executor, MySql, MySqlPool, Pool, Row};
use std::boxed::Box;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::default::default;
use std::default::Default;
use std::env;
use std::f32::EPSILON;
use std::mem::take;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use aios_core::options::DbOption;
use aios_core::pdms_data::ScomInfo;
use aios_core::prim_geo;
use log::{error, info};
use nom::combinator::map;
use tokio::sync::RwLock;

use crate::api::attr::*;
use crate::api::children::*;
use crate::api::element::*;
use crate::api::project_mdb::*;
use crate::api::refno_info::{cache_plin_plax, get_ref0_projects, sync_refno_basic_map};
use crate::aql_api::children::*;
use crate::aql_api::foreign_refnos::query_foreign_refno_aql;
use crate::aql_api::para_value::{query_des_para_value, query_para_from_desi_refno};
use crate::aql_api::plin_attr::{match_jusline_attr, query_plin_attrs, query_pline_value};
use crate::cata::consts::{BANG_WIT_EXTRU_TYPES, JUSLINE_TYPES};
use crate::cata::direction_parse::parse_expr_to_dir;
use crate::cata::query_cata::resolve_desi_comp;
use crate::cata::resolve::CataExprContext;
use crate::cata::resolve_helper::{eval_str_to_f32, parse_str_axis_to_vec3};
use crate::cata::sctn;
use crate::cata::sctn::geo::create_profile_geos;
use crate::consts::*;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::structs::AIOSAxisMap;
use crate::defines::*;
use crate::graph_db::pdms_arango::{get_arangodb_conn_from_db_option, save_arangodb_with_database};
use crate::graph_db::pdms_inst_arango::sync_instance_to_graph_db;
use crate::helper::qualified_table_name;
use crate::mdb::get_project_mdb;
use crate::tables::{gen_create_project_mdb_json_sql, gen_create_project_mdb_sql};
use crate::AQL_PDMS_ELES_COLLECTION;

pub const TUBI_TOL: f32 = 10.0f32;

pub type CateBrepShapeMap = DashMap<RefU64, Vec<CateBrepShape>>;

lazy_static! {
    pub static ref CATAEXPRCONTEXT_MAP: DashMap<RefU64, CataExprContext> = {
        let mut s = DashMap::new();
        s
    };
}

pub const PRIMITIVE_NOUN_NAMES: [&'static str; 8] = [
    "BOX", "CYLI", "SPHE", "CONE", "DISH", "CTOR", "RTOR", "PYRA",
];

///基本体的名称集合
//"SPINE", "GENS",
pub const GNERAL_PRIM_NOUN_NAMES: [&'static str; 20] = [
    "BOX", "CYLI", "SPHE", "CONE", "DISH", "CTOR", "RTOR", "PYRA", "NCYL", "NBOX", "NCON", "NSNO",
    "NPYR", "NDIS", "NXTR", "NCTO", "NRTO", "NSLC", "NREV", "NSCY",
];

pub const GENRAL_NEGATIVE_NOUN_NAMES: [&'static str; 12] = [
    "NCYL", "NBOX", "NCON", "NSNO", "NPYR", "NDIS", "NXTR", "NCTO", "NRTO", "NSLC", "NREV", "NSCY",
];

pub const CATA_ATT_TYPES: [&'static str; 23] = [
    "BRAN", "HANG", // "ELCONN",
    "CMPF", "WALL", "STWALL", "GWALL", // "FIXING",
    "PJOI", "PFIT", "GENSEC", "RNODE", "PRTELE", "GPART", "SCREED", "NOZZ", "PALJ",
    // "SUBJ",
    "CABLE", "BATT", "CMFI", "SCOJ", "SEVE", "SBFI", "SCTN", "FITT",
];

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

#[derive(Debug)]
pub struct AiosDBManager {
    //不同project的连接池子
    pub project_map: DashMap<String, Pool<MySql>>,

    pub ref0_projects: DashMap<u32, Vec<String>>,

    pub info_pool: Pool<MySql>,

    pub projects: Vec<String>,

    pub needed_parse_files: Option<Vec<String>>,

    pub project_path: String, //整个项目的路径

    pub db_option: DbOption,

    pub cached_mesh_mgr: Arc<CachedMeshesMgr>,

    pub mesh_instance_mgr: Arc<DashMap<i32, CachedInstanceMgr>>,

    pub arango_database: Database,

    cached_world_transforms_map: Arc<DashMap<RefU64, bevy::prelude::Transform>>,

    // pub plin_cache_mgr: DashMap<RefU64, String>,

    pub cache_module_numbdbs: BTreeSet<i32>,

    pub mdb_dbnums: BTreeSet<i32>,
}

#[async_trait]
impl PdmsDataInterface for AiosDBManager {
    /// 获得最全的数据
    async fn get_attr(&self, refno: RefU64) -> anyhow::Result<AttrMap> {
        return if PDMS_ATT_MAP_CACHE.get(&refno).is_some() {
            let k = PDMS_ATT_MAP_CACHE.get(&refno).unwrap();
            Ok(k.value().clone())
        } else {
            let attr = query_full_attr(refno, self, None).await?;
            PDMS_ATT_MAP_CACHE
                .insert(refno, &attr)
                .expect("PDMS_ATT_MAP_CACHE save error.");
            Ok(attr)
        };
    }

    ///获取owner的参考号，从缓存读取
    #[inline]
    fn get_owner(&self, refno: RefU64) -> RefU64 {
        CACHED_REFNO_BASIC_MAP
            .get(&refno)
            .map(|x| x.value().get_owner())
            .unwrap_or_default()
    }

    /// 获得隐含数据的属性
    async fn get_implicit_attr(
        &self,
        refno: RefU64,
        columns: Option<Vec<&str>>,
    ) -> anyhow::Result<AttrMap> {
        if let Some((_, project_pool)) = self.get_project_pool_by_refno(refno).await {
            if let Some(ref_basic) = self.get_refno_basic(refno) {
                let attr =
                    query_implicit_attr(refno, ref_basic.value(), &project_pool, columns).await?;
                // PDMS_IMPLICIT_ATT_MAP_CACHE.insert(refno, attr.clone());
                return Ok(attr);
            }
        }
        Ok(AttrMap::default())
    }

    /// 获得OWNER隐含数据的属性
    async fn get_implicit_attrs_by_owner(
        &self,
        owner: RefU64,
        type_name: &str,
        columns: Option<Vec<&str>>,
    ) -> anyhow::Result<Vec<AttrMap>> {
        if let Some((_, project_pool)) = self.get_project_pool_by_refno(owner).await {
            let attr =
                query_implicit_attrs_by_owner(owner, type_name, &project_pool, columns).await?;
            return Ok(attr);
        }
        Ok(vec![])
    }

    /// 获取parent的attr数据
    async fn get_parent_attr(&self, refno: RefU64) -> anyhow::Result<AttrMap> {
        todo!()
    }

    /// 获得缓存的refno基本信息, todo 改成使用sql的intersect
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
        if let Some((_, project_pool)) = self.get_project_pool_by_refno(refno).await {
            if let Ok(node) = query_ele_node(refno, &project_pool).await {
                return Ok(Some(node));
            }
        }
        Ok(None)
    }

    async fn get_owner_ele_node(&self, refno: RefU64) -> anyhow::Result<Option<EleTreeNode>> {
        let mut node = None;
        if let Some((_, project_pool)) = self.get_project_pool_by_refno(refno).await {
            let parent = self.get_owner(refno);
            if parent.is_valid() {
                node = Some(query_ele_node(parent, &project_pool).await?);
            }
        }
        Ok(node)
    }

    async fn get_world(
        &self,
        project: &str,
        mdb_name: &str,
        module: &str,
    ) -> anyhow::Result<EleTreeNode> {
        if let Some(project_pool) = self.project_map.get(project) {
            let v = query_world("SAMPLE", "DESI", project_pool.value()).await?;
            return Ok(v);
        }
        return Err(anyhow!("World not found".to_string()));
    }

    async fn get_children_nodes(&self, refno: RefU64) -> anyhow::Result<Vec<EleTreeNode>> {
        let mut r = vec![];
        if let Some((_, project_pool)) = self.get_project_pool_by_refno(refno).await {
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
        if let Some((_, project_pool)) = self.get_project_pool_by_refno(refno).await {
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
        if let Some((_, project_pool)) = self.get_project_pool_by_refno(refno).await {
            let children = query_children(refno, &project_pool).await?;
            children.into_iter().for_each(|child| {
                result.push(child.0);
            });
        }
        Ok(result)
    }

    async fn get_name(&self, refno: RefU64) -> anyhow::Result<SmolStr> {
        if let Some((_, project_pool)) = self.get_project_pool_by_refno(refno).await {
            let name = query_name(refno, &project_pool).await?;
            return Ok(SmolStr::new(name));
        }
        Ok(SmolStr::new(""))
    }

    /// dbnos为空代表所有db都会去获取
    async fn get_refnos_by_types(
        &self,
        project: &str,
        att_types: &[&str],
        dbnos: Option<&[i32]>,
    ) -> anyhow::Result<RefU64Vec> {
        if let Some(project_pool) = self.project_map.get(project) {
            let r = query_types_refnos(att_types, project_pool.value(), dbnos).await?;
            return Ok(r);
        }
        Ok(RefU64Vec::default())
    }

    async fn get_db_world(
        &self,
        project: &str,
        db_no: u32,
    ) -> anyhow::Result<Option<(RefU64, String)>> {
        if let Some(project_pool) = self.project_map.get(project) {
            let r =
                query_id_name_from_dbno_type(db_no as i32, "WORL", project_pool.value()).await?;
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

    ///获得不包含world的父节点路径
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
        if let Some((_, pool)) = self.get_project_pool_by_refno(refno).await {
            while let Ok(attr) = self.get_implicit_attr(cur_refno, None).await {
                //后面是不是要缓存这个层级结构
                if let Ok(Some(owner)) = query_owner_from_id(cur_refno, &pool).await {
                    r.push(attr);
                    cur_refno = owner;
                } else {
                    break;
                }
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

    // async fn get_transform(&self, refno: RefU64) -> anyhow::Result<Option<Transform>> {
    //     // self.get_transform(refno)
    // }

    ///获得世界坐标系, 需要缓存数据，如果已经存在数据了，直接获取
    async fn get_world_transform(&self, refno: RefU64) -> anyhow::Result<Option<Transform>> {
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
        //需要判断owner 下是不是有spine，如wall，顺时针逆时针会影响plin的方向
        for (refno, ref_basic) in ancestors {
            let mut jusl_vec = Vec3::new(0.0, 0.0, 0.0);
            let att = self.get_attr(refno).await?;
            let mut pos = att.get_position().unwrap_or_default();
            let mut quat = Quat::IDENTITY;
            let type_name = att.get_type();
            if let Some(jusl) = att.get_str("JUSL") {
                if let Some(exps) = query_pline_value(refno, jusl, &self.arango_database).await? {
                    let x = self.resolve_expression_to_f32(&exps[0], refno).await?;
                    let y = self.resolve_expression_to_f32(&exps[1], refno).await?;
                    jusl_vec = Vec3::new(x, y, 0.0);
                }
            }
            //先获得下面的PLOO
            let owner = ref_basic.owner;
            if let Some(owner_basic) = self.get_refno_basic(owner) {
                //特殊情况的一些处理
                match owner_basic.get_type() {
                    "FLOOR" => {
                        let sjus = att.get_str("JUSL").unwrap_or("unset");
                        let height = self
                            .get_attr(refno)
                            .await?
                            .get_f32("HEIG")
                            .unwrap_or_default();
                        let mut off_z = if sjus == "UTOP" || sjus == "DTOP" {
                            -height
                        } else if sjus == "UCEN" || sjus == "DCEN" {
                            -height / 2.0
                        } else {
                            0.0
                        };
                        pos.z += off_z;
                    }
                    "PLDATU" => {
                        let grand = owner_basic.owner;
                        let grand_att = self.get_attr(grand).await?;
                        let zdis = (att.get_f32("ZDIS").unwrap_or_default() * Vec3::Z);
                        let pkdi = att.get_f32("PKDI").unwrap_or_default();
                        //获取比例的位置
                        pos = pos + zdis;
                    }
                    _ => {}
                }
            }

            let mut quat_v = att.get_rotation();
            let mut need_bangle = false;
            if quat_v.is_some() {
                quat = quat_v.unwrap();
            } else {
                let extru_dir: Vec3 = if let Some(poss) = att.get_poss() &&
                    let Some(pose) = att.get_pose()
                {
                    need_bangle = true;
                    (pose - poss).normalize()
                } else {
                    Vec3::Z
                };
                let d = extru_dir.dot(Vec3::Z).abs();
                let mut ref_axis = if abs_diff_eq!(1.0, d) {
                    Vec3::Y
                } else {
                    Vec3::Z
                };

                let p_axis = ref_axis.cross(extru_dir).normalize();
                let y_axis = extru_dir.cross(p_axis).normalize();
                quat = Quat::from_mat3(&glam::f32::Mat3::from_cols_array_2d(&[
                    p_axis.to_array(),
                    y_axis.to_array(),
                    extru_dir.to_array(),
                ]));
            }

            if let Some(bangle) = att.get_f32("BANG") {
                need_bangle |= type_name == "PFIT";
                if need_bangle {
                    quat = quat * Quat::from_rotation_z(bangle.to_radians());
                }
            }
            //弧墙下方没有fitt
            if let Some(pos_line) = att.get_str("POSL") {
                // dbg!(pos_line);
                //plin里的位置偏移
                let mut plin_pos = Vec3::new(0.0, 0.0, 0.0);
                let mut pline_plax = -Vec3::X;

                let delta_vec = att.get_vec3("DELP").unwrap_or_default() /*+ plin_pos*/;
                let zdis = (att.get_f32("ZDIS").unwrap_or_default() * Vec3::Z);
                let bangle = att.get_f32("BANG").unwrap_or_default();
                let pos_line =
                    query_pline_value(att.get_owner().unwrap(), pos_line, &self.arango_database)
                        .await?;
                if let Some(pos_line) = pos_line {
                    // dbg!(&pos_line);
                    let owner = ref_basic.owner;
                    //对于fitting这种，需要取parent的值
                    let x = self.resolve_expression_to_f32(&pos_line[0], owner).await?;
                    let y = self.resolve_expression_to_f32(&pos_line[1], owner).await?;
                    plin_pos = Vec3::new(x, y, 0.0);
                }
                if let Some(v) = CACHED_PLIN_MAP.get(&refno) {
                    pline_plax = parse_expr_to_dir(&v.value());
                }
                let bangle_rot = Quat::from_rotation_z(bangle.to_radians());
                let y_axis = Vec3::Z;
                let z_axis = pline_plax;
                let x_axis = y_axis.cross(z_axis).normalize();
                let quat = Quat::from_mat3(&glam::f32::Mat3::from_cols_array_2d(&[
                    x_axis.to_array(),
                    y_axis.to_array(),
                    z_axis.to_array(),
                ]));
                translation = translation
                    + rotation * (pos + zdis + plin_pos - jusl_vec)
                    + rotation * quat * bangle_rot * delta_vec;
                rotation = rotation * quat * bangle_rot;
            } else {
                translation = translation + rotation * pos;
                rotation = rotation * quat;
            }

            self.cached_world_transforms_map
                .entry(refno)
                .or_insert(Transform {
                    rotation,
                    translation,
                    scale: Vec3::ONE,
                });
        }
        //将rotation 还原为角度
        // let angles = rotation.to_euler(EulerRot::XYZ);
        if self.db_option.debug_print_world_transform {
            let rot_mat = Mat3::from_quat(rotation);
            let ori_str = math_tool::to_pdms_ori_str(&rot_mat);
            println!("{} : {:?}", refno.to_refno_str(), (translation, ori_str));
        }
        Ok(Some(Transform {
            rotation,
            translation,
            scale: Vec3::ONE,
        }))
    }

    async fn get_travel_children_attrs(&self, refno: RefU64) -> anyhow::Result<Vec<AttrMap>> {
        let mut r = vec![];
        if let Ok(database) = self.get_arangodb_conn().await {
            let children = query_travel_children_aql(&database, refno).await?;
            for child in children {
                let child_refno = RefU64::from_refno_str(&child.refno)?;
                let attr = self.get_attr(child_refno).await?;
                r.push(attr);
            }
        }
        Ok(r)
    }
}

impl AiosDBManager {
    /// 从默认配置文件初始化
    pub async fn init_form_config() -> anyhow::Result<Self> {
        let db_option = Self::get_db_option()?;
        let mut mgr = Self::init(&db_option).await?;
        mgr.init_mdb(
            &db_option.project_name,
            &db_option.mdb_name,
            &db_option.module,
        ).await?;
        Ok(mgr)
    }

    pub fn get_table_name(&self, refno: RefU64) -> String {
        CACHED_REFNO_BASIC_MAP
            .get(&refno)
            .map(|x| x.get_table_name().to_string())
            .unwrap_or("UNSET".to_string())
    }
    #[inline]
    pub fn get_db_option() -> anyhow::Result<DbOption> {
        use config::{Config, ConfigError, Environment, File};
        let s = Config::builder()
            .add_source(File::with_name("DbOption"))
            .build()?;
        s.try_deserialize::<DbOption>()
            .map_err(|x| anyhow!(x.to_string()))
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
        let url = &format!("{connection_str}/{}", project);
        PoolOptions::new()
            .max_connections(500)
            .acquire_timeout(Duration::from_secs(10 * 60))
            .connect(url)
            .await
            .map_err({ |x| anyhow!(x.to_string()) })
    }

    #[inline]
    pub async fn get_arangodb_conn(&self) -> anyhow::Result<Database> {
        let conn = Connection::establish_jwt(
            &self.db_option.arangodb_url,
            &self.db_option.arangodb_user,
            &self.db_option.arangodb_password,
        )
            .await?;

        Ok(conn.db(&self.db_option.arangodb_database).await?)
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
                    sync_refno_basic_map(kv.value() /* &self.mdb_dbnums*/).await.unwrap();
                }
            }
        }
        // 将对应mdb module 下所有的 numbdb 存下来
        println!("缓存RefBasic数据花费：{}ms", time.elapsed().as_millis());
        //创建table, 如果已经存在，可以忽略
        if self.db_option.reset_mdb_project.is_some() && self.db_option.reset_mdb_project.unwrap() {
            let create_sql = gen_create_project_mdb_sql();
            let _ = conn.execute(create_sql.as_str()).await;
            println!("正在插入mdb数据");
            let _ = self.insert_project_mdb(&project_pool, &self.info_pool).await;
        }
        cache_mdb_site_map(mdb, module, &project_pool).await;
        self.mdb_dbnums = query_mdb_all_dbnums(mdb, &project_pool).await?;
        if need_sync_refno_basic {
            for project in &self.db_option.included_projects {
                if let Some(kv) = self.project_map.get(project) {
                    let dbnums = self.mdb_dbnums.iter().cloned().collect::<Vec<_>>();
                    if let Ok(m) = cache_plin_plax(
                        kv.value(),
                        Some(&dbnums),
                        &self.arango_database,
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
        let database = get_arangodb_conn_from_db_option(&db_option).await.unwrap();
        Ok(Self {
            project_map,
            ref0_projects,
            info_pool: info_conn,
            projects,
            needed_parse_files: None,
            project_path: dir,
            db_option,
            cached_mesh_mgr: Arc::new(Default::default()),
            mesh_instance_mgr: Arc::new(Default::default()),
            arango_database: database,
            cached_world_transforms_map: Arc::new(Default::default()),
            cache_module_numbdbs: Default::default(),
            mdb_dbnums: Default::default(),
        })
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
                //check if exist in pdms_elements
                // for project in projects.value() {
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
        let mdbs = query_types_refnos(&vec!["MDB"], project_pool, None).await?;
        dbg!(&mdbs.len());
        for mdb_refno in mdbs {
            dbg!(&mdb_refno);
            let Ok(mdb_attr) = query_full_attr(mdb_refno, self, None).await else {
                continue;
            };
            let Ok(mdb_name) = query_name(mdb_refno, &project_pool).await else {
                continue;
            };
            dbg!(&mdb_name);
            if let Some(dbs) = mdb_attr.get_refu64_vec("CURD") {
                let mut map = HashMap::new();
                for (i, db_refno) in dbs.iter().enumerate() {
                    if let Ok(att) = self.get_implicit_attr(*db_refno, Some(vec!["NUMBDB"])).await {
                        let db_num = att.get_i32("NUMBDB").unwrap_or_default();
                        dbg!(&db_num);
                        if let Ok(Some(mut quick_info)) = self.query_quick_info_by_dbno(*db_refno, db_num, info_pool).await {
                            dbg!(&quick_info.db_type);
                            quick_info.order_number = i as _;
                            map.entry(quick_info.db_type.clone())
                                .or_insert_with(Vec::new).push(quick_info);
                        }
                    }
                }
                mdb_map.entry(mdb_name).or_insert(map);
                dbg!("ok");
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

    ///获取单个元件的模型数据
    pub async fn get_cata_single_geoms(
        mgr: Arc<AiosDBManager>,
        design_refno: RefU64,
        brep_shape_map: &CateBrepShapeMap,
        refno_ptset_map: &DashMap<RefU64, AIOSAxisMap>,
        scom_info_map: &RwLock<HashMap<RefU64, ScomInfo>>,
    ) -> anyhow::Result<bool> {
        let cur_ele = mgr.get_refno_basic(design_refno).unwrap();
        let type_name = cur_ele.get_type();
        let owner = mgr.get_owner_ref_basic(design_refno);
        if owner.is_none() {
            return Ok(false);
        }
        let desi_att = mgr.get_attr(design_refno).await?;
        let geoms = resolve_desi_comp(design_refno, None, Some(mgr.as_ref()), scom_info_map)
            .await
            .unwrap_or_default();
        // dbg!(&geoms);
        if type_name == "SCTN"
            || type_name == "STWALL"
            || type_name == "GENSEC"
            || type_name == "WALL"
        {
            create_profile_geos(
                design_refno,
                &desi_att,
                &geoms,
                &brep_shape_map,
                mgr.as_ref(),
            ).await?;
        } else {
            let GeomsInfo {
                geometries,
                axis_map,
            } = geoms;
            for (i, geom) in geometries.into_iter().enumerate() {
                if let Some(cate_shape) = convert_to_brep_shapes(&geom) {
                    let geo_hash = mgr
                        .cached_mesh_mgr
                        .gen_pdms_mesh(cate_shape.brep_shape.clone(), false);
                    let mut bbox = mgr.cached_mesh_mgr.get_bbox(&geo_hash);
                    if let Some(mut aabb) = bbox {
                        let rot = cate_shape.transform.rotation;
                        let translation = cate_shape.transform.translation;
                    }
                    brep_shape_map
                        .entry(design_refno)
                        .or_insert(Vec::new())
                        .push(cate_shape);
                }
            }
            refno_ptset_map.insert(design_refno, axis_map);
        }
        Ok(true)
    }

    async fn get_cata_auto_tubi_geoms(
        mgr: Arc<AiosDBManager>,
        branch_refno: RefU64,
        group_att: &AttrMap,
        brep_shape_map: &CateBrepShapeMap,
        refno_ptset_map: &DashMap<RefU64, AIOSAxisMap>,
        tubi_aqls: &mut Arc<DashMap<u64, TubiEdgeAql>>,
        scom_info_map: &RwLock<HashMap<RefU64, ScomInfo>>,
    ) -> anyhow::Result<bool> {
        let group_transform = mgr
            .get_world_transform(branch_refno)
            .await?
            .unwrap_or_default();
        let htube_pt = group_transform.transform_point(
            group_att
                .get_vec3("HPOS")
                .ok_or(anyhow!("HPOS not exist".to_string()))?,
        );
        let hdir = group_transform
            .transform_point(
                group_att
                    .get_vec3("HDIR")
                    .ok_or(anyhow!("HDIR not exist".to_string()))?,
            )
            .normalize_or_zero();
        let bran_ttube_pt = group_transform.transform_point(
            group_att
                .get_vec3("TPOS")
                .ok_or(anyhow!("TPOS not exist".to_string()))?,
        );

        let is_hang = group_att.get_type() == "HANG";

        let h_ref = group_att
            .get_foreign_refno(if is_hang { "HREF" } else { "HSTU" })
            .unwrap_or_default();
        let hconnect = group_att.get_as_string("HCON").unwrap_or_default();
        let bran_name = group_att.get_name().0.to_string();
        let mut has_tubi = true;
        let mut bore = 0.0f32;
        let mut href_type = "".to_string();
        //头节点的处理
        if let Ok(h_att) = mgr.get_attr(h_ref).await {
            href_type = h_att.get_type().to_string();
            let h_cat_ref = h_att.get_foreign_refno("CATR").unwrap_or_default();
            let tubi_geoms_info =
                resolve_desi_comp(branch_refno, Some(h_cat_ref), Some(mgr.as_ref()), scom_info_map)
                    .await
                    .unwrap_or_default();
            let mut has_tube_geom = false;
            for tubi_geom in &tubi_geoms_info.geometries {
                if let TubeImplied(d) = tubi_geom {
                    bore = d.diameter;
                    has_tube_geom = true;
                    break;
                }
            }

            if !has_tube_geom {
                let h_cat_att = mgr.get_attr(h_cat_ref).await?;
                let params = h_cat_att.get_f64_vec("PARA").unwrap_or_default();
                if params.len() >= 2 {
                    bore = params[if is_hang { 0 } else { 1 }] as f32;
                }
            }
        }
        let mut current_tubing = PdmsTubing {
            refno: branch_refno,
            start_pt: htube_pt,
            end_pt: Vec3::ZERO,
            desire_leave_dir: hdir,
            desire_arrive_dir: Default::default(),
            _from: format!("{AQL_PDMS_ELES_COLLECTION}/{}", h_ref.to_url_refno()),
            _to: Default::default(),
            bore,
            finished: false,
        };
        let children = mgr
            .get_children_refs(branch_refno)
            .await
            .unwrap_or_default();
        // 整个 bran 就一个 tubi
        if children.len() == 0 {
            if !current_tubing.finished
                && bran_ttube_pt.distance(current_tubing.start_pt) > TUBI_TOL
            {
                current_tubing.end_pt = bran_ttube_pt;
                current_tubing.finished = true;
                //需要检查href的方位
                current_tubing.desire_arrive_dir = -current_tubing.get_dir();
                //检查一下方向是否一致，不一致的，不显示，或者加标记位
                if current_tubing.is_dir_ok() {
                    let shape = current_tubing.convert_to_shape();
                    brep_shape_map
                        .entry(branch_refno)
                        .or_insert(Vec::new())
                        .push(shape);
                } else {
                    error!("{} 的直段方向有问题", branch_refno.to_refno_string());
                }
            }
            // 将 tubi 数据保存到图数据库
            let tref = group_att
                .get_foreign_refno(if is_hang { "TREF" } else { "LSTU" })
                .unwrap_or_default();
            let key = h_ref.hash_with_another_refno(tref);
            tubi_aqls.entry(key).or_insert(TubiEdgeAql {
                _key: key.to_string(),
                _from: format!("{AQL_PDMS_ELES_COLLECTION}/{}", h_ref.to_url_refno()),
                _to: format!("{AQL_PDMS_ELES_COLLECTION}/{}", tref.to_url_refno()),
                start_pt: current_tubing.start_pt,
                end_pt: current_tubing.end_pt,
                att_type: group_att.get_type().to_string(),
                extra_type: "".to_string(),
                bore,
                bran_name: bran_name.to_string(),
            });
            return Ok(true);
        }
        // 将 bran 保存到 tubi_edges 中
        {
            // 保存bran 和 第一个子节点的连接关系
            if let Some(first_refno) = children.0.first() {
                let first_attr = mgr.get_attr(*first_refno).await?;
                let geoms = resolve_desi_comp(*first_refno, None,
                                              Some(mgr.as_ref()), &scom_info_map).await;
                if let Ok(geoms) = geoms {
                    if let Some(arrive) = first_attr.get_i32("ARRI") {
                        if geoms.axis_map.contains_key(&arrive) {
                            let first_world_trans = mgr
                                .get_world_transform(*first_refno)
                                .await?
                                .unwrap_or_default();
                            let p = &geoms.axis_map[&arrive].pt;
                            let a_pos = first_world_trans.transform_point(Vec3::new(
                                p[0] as f32,
                                p[1] as f32,
                                p[2] as f32,
                            ));
                            let key = branch_refno.hash_with_another_refno(*first_refno);
                            let att_type = first_attr.get_type();
                            let mut extra_type = "".to_string();
                            if att_type == "ATTA" {
                                let attype = first_attr.get_str("ATTY").unwrap_or("");
                                extra_type = attype.to_string();
                            }
                            tubi_aqls.entry(key).or_insert(TubiEdgeAql {
                                _key: key.to_string(),
                                _from: format!("{AQL_PDMS_ELES_COLLECTION}/{}", branch_refno.to_url_refno()),
                                _to: format!("{AQL_PDMS_ELES_COLLECTION}/{}", first_refno.to_url_refno()),
                                start_pt: htube_pt,
                                end_pt: a_pos,
                                att_type: att_type.to_string(),
                                extra_type,
                                bore,
                                bran_name: bran_name.to_string(),
                            });
                        }
                    }
                }
            }
            // 保存元件之间的距离
            for (idx, refno) in children.clone().into_iter().enumerate() {
                let mut edge = TubiEdgeAql::default();
                edge._from = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
                if idx >= children.len() - 1 {
                    break;
                }
                let to_refno = children[idx + 1];
                let key = refno.hash_with_another_refno(to_refno);
                edge._key = key.to_string();
                edge._to = format!("{AQL_PDMS_ELES_COLLECTION}/{}", to_refno.to_url_refno());
                edge.bran_name = bran_name.to_string();

                let Ok(attr) = mgr.get_attr(refno).await else {
                    continue;
                };
                let Ok(to_attr) = mgr.get_attr(to_refno).await else {
                    continue;
                };
                let att_type = to_attr.get_type();
                edge.att_type = att_type.to_string();
                // 单独存 atta 的 attype
                if att_type == "ATTA" {
                    let attype = to_attr.get_str("ATTY").unwrap_or("");
                    edge.extra_type = attype.to_string();
                }

                let world_trans = mgr.get_world_transform(refno).await?.unwrap_or_default();

                let Ok(mut geoms) = resolve_desi_comp(refno, None, Some(mgr.as_ref()), scom_info_map).await else {
                    continue;
                };
                let Ok(mut to_geoms) = resolve_desi_comp(to_refno, None, Some(mgr.as_ref()), scom_info_map).await else {
                    continue;
                };
                let to_world_trans = mgr.get_world_transform(to_refno).await?.unwrap_or_default();
                if let Some(arrive) = to_attr.get_i32("ARRI") {
                    if to_geoms.axis_map.contains_key(&arrive) {
                        let p = &to_geoms.axis_map[&arrive].pt;
                        let a_pos = to_world_trans.transform_point(Vec3::new(
                            p[0] as f32,
                            p[1] as f32,
                            p[2] as f32,
                        ));
                        edge.end_pt = a_pos;
                    } else {
                        dbg!(&to_refno);
                        dbg!(&arrive);
                    }
                }
                if let Some(lstube) = attr.get_foreign_refno(if is_hang { "LSRO" } else { "LSTU" }) {
                    if let Ok(lstube_att) = mgr.get_attr(lstube).await {
                        let lstube_cat_refno =
                            lstube_att.get_foreign_refno("CATR").unwrap_or_default();
                        let tubi_geoms_info = resolve_desi_comp(
                            refno,
                            Some(lstube_cat_refno),
                            Some(mgr.as_ref()),
                            scom_info_map,
                        )
                            .await
                            .unwrap_or_default();
                        let mut has_tube_geom = false;
                        for tubi_geom in &tubi_geoms_info.geometries {
                            if let TubeImplied(d) = tubi_geom {
                                edge.bore = d.diameter;
                                has_tube_geom = true;
                                break;
                            }
                        }
                        if !has_tube_geom {
                            let lstube_cat_att = mgr.get_attr(lstube_cat_refno).await?;
                            let params = lstube_cat_att.get_f64_vec("PARA").unwrap_or_default();
                            if params.len() >= 2 {
                                edge.bore = params[if is_hang { 0 } else { 1 }] as f32;
                            }
                        }
                    }
                }
                if let Some(leave) = attr.get_i32("LEAV") {
                    if geoms.axis_map.contains_key(&leave) {
                        let p = &geoms.axis_map[&leave].pt;
                        let l_pos = world_trans.transform_point(Vec3::new(
                            p[0] as f32,
                            p[1] as f32,
                            p[2] as f32,
                        ));
                        edge.start_pt = l_pos;
                    }
                }
                if !edge._key.is_empty() {
                    // bran children 之间的关系
                    tubi_aqls.entry(key).or_insert(edge);
                }
            }
            // 保存最后一个元件
            if let Some(last_refno) = children.0.last() {
                let last_attr = mgr.get_attr(*last_refno).await?;
                let last_geoms = resolve_desi_comp(*last_refno, None, Some(mgr.as_ref()), scom_info_map).await;
                if let Ok(last_geoms) = last_geoms {
                    let last_world_trans = mgr
                        .get_world_transform(*last_refno)
                        .await?
                        .unwrap_or_default();
                    if let Some(leave) = last_attr.get_i32("LEAV") {
                        if last_geoms.axis_map.contains_key(&leave) {
                            let tref = group_att.get_foreign_refno("TREF").unwrap_or(RefU64(0));
                            let p = &last_geoms.axis_map[&leave].pt;
                            let l_pos = last_world_trans.transform_point(Vec3::new(
                                p[0] as f32,
                                p[1] as f32,
                                p[2] as f32,
                            ));
                            let key = last_refno.hash_with_another_refno(tref);
                            let att_type = last_attr.get_type();
                            let mut extra_type = "".to_string();
                            if att_type == "ATTA" {
                                let attype = last_attr.get_str("ATTY").unwrap_or("");
                                extra_type = attype.to_string();
                            }
                            tubi_aqls.entry(key).or_insert(TubiEdgeAql {
                                _key: key.to_string(),
                                _from: format!("{AQL_PDMS_ELES_COLLECTION}/{}", last_refno.to_url_refno()),
                                _to: format!("{AQL_PDMS_ELES_COLLECTION}/{}", tref.to_url_refno()),
                                start_pt: l_pos,
                                end_pt: bran_ttube_pt,
                                att_type: att_type.to_string(),
                                extra_type,
                                bore,
                                bran_name: bran_name.to_string(),
                            });
                        }
                    }
                }
            }
        }

        let last_child = children.last().unwrap().clone();
        //第一遍完成后，然后生成tubing
        for refno in children {
            let Ok(attr) = mgr.get_attr(refno).await else {
                continue;
            };
            println!(
                "正在处理元件{}: {}",
                attr.get_type(),
                refno.to_refno_string()
            );
            let world_trans = mgr.get_world_transform(refno).await?.unwrap_or_default();
            let mut geoms = resolve_desi_comp(refno, None, Some(mgr.as_ref()), scom_info_map).await;
            if geoms.is_err() {
                error!("{:?}", geoms.err().unwrap());
                continue;
            }
            let mut geoms = geoms.unwrap();
            //有隐含管段
            if has_tubi && attr.get_type() != "ATTA" {
                if let Some(arrive) = attr.get_i32("ARRI") {
                    if geoms.axis_map.contains_key(&arrive) {
                        let p = &geoms.axis_map[&arrive].pt;
                        let a_pos = world_trans.transform_point(Vec3::new(
                            p[0] as f32,
                            p[1] as f32,
                            p[2] as f32,
                        ));
                        let dir = geoms.axis_map[&arrive].dir;
                        let a_dir = world_trans.transform_vec3(dir).normalize_or_zero();
                        let arrive_refno = geoms.axis_map[&arrive].refno;
                        if !current_tubing.finished
                            && a_pos.distance(current_tubing.start_pt) > TUBI_TOL
                        {
                            current_tubing.refno = refno;
                            current_tubing.end_pt = a_pos;
                            current_tubing.desire_arrive_dir = a_dir;
                            current_tubing.finished = true;
                            if current_tubing.is_dir_ok() {
                                let brep_shape = current_tubing.convert_to_shape();
                                brep_shape_map
                                    .entry(refno)
                                    .or_insert(Vec::new())
                                    .push(brep_shape);
                            } else {
                                error!("{} 的直段方向有问题", refno.to_refno_string());
                            }
                        }
                    }
                }
                if let Some(lstube) = attr.get_foreign_refno(if is_hang { "LSRO" } else { "LSTU" })
                {
                    if let Ok(lstube_att) = mgr.get_attr(lstube).await {
                        let lstube_cat_refno =
                            lstube_att.get_foreign_refno("CATR").unwrap_or_default();
                        //todo check how to get the bore value
                        let tubi_geoms_info = resolve_desi_comp(
                            refno,
                            Some(lstube_cat_refno),
                            Some(mgr.as_ref()),
                            scom_info_map,
                        )
                            .await
                            .unwrap_or_default();
                        let mut has_tube_geom = false;
                        for tubi_geom in &tubi_geoms_info.geometries {
                            if let TubeImplied(d) = tubi_geom {
                                current_tubing.bore = d.diameter;
                                has_tube_geom = true;
                                break;
                            }
                        }
                        if !has_tube_geom {
                            let lstube_cat_att = mgr.get_attr(lstube_cat_refno).await?;
                            let params = lstube_cat_att.get_f64_vec("PARA").unwrap_or_default();
                            if params.len() >= 2 {
                                current_tubing.bore = params[if is_hang { 0 } else { 1 }] as f32;
                            }
                        }
                    }
                }
                if let Some(leave) = attr.get_i32("LEAV") {
                    if geoms.axis_map.contains_key(&leave) {
                        let p = &geoms.axis_map[&leave].pt;
                        let dir = geoms.axis_map[&leave].dir;
                        let l_dir = world_trans.transform_vec3(dir).normalize_or_zero();
                        let l_pos = world_trans.transform_point(Vec3::new(
                            p[0] as f32,
                            p[1] as f32,
                            p[2] as f32,
                        ));
                        current_tubing.start_pt = l_pos;
                        current_tubing.desire_leave_dir = l_dir;
                        current_tubing.finished = false;
                    }
                }
            }
            //管件的生成
            let GeomsInfo {
                geometries,
                axis_map,
            } = geoms;
            for (i, geom) in geometries.into_iter().enumerate() {
                if let Some(cate_shape) = convert_to_brep_shapes(&geom) {
                    brep_shape_map
                        .entry(refno)
                        .or_insert(Vec::new())
                        .push(cate_shape);
                }
            }
            refno_ptset_map.insert(refno, axis_map);
            //有隐含管段
            if has_tubi {
                //最后一段的管道
                if refno == last_child {
                    if !current_tubing.finished
                        && bran_ttube_pt.distance(current_tubing.start_pt) > TUBI_TOL
                    {
                        //检查是否有一端是世界坐标原点
                        current_tubing.refno = refno;
                        current_tubing.end_pt = bran_ttube_pt;
                        current_tubing.finished = true;
                        //todo 需要取得连接到的，tref的点对应的arrive方向
                        current_tubing.desire_arrive_dir = -current_tubing.desire_leave_dir;
                        if current_tubing.is_dir_ok() {
                            let shape = current_tubing.convert_to_shape();
                            brep_shape_map
                                .entry(refno)
                                .or_insert(Vec::new())
                                .push(shape);
                        } else {
                            error!("{} 的直段方向有问题", refno.to_refno_string());
                        }
                    }
                }
            }
        }
        Ok(true)
    }

    /// 通用的解析表达式的方法, 解析desi参考号下的 表达式值
    /// 如果 desi_refno 为空，代表design的数据不需要参与计算
    pub async fn resolve_expression_to_f32(
        &self,
        expr: &str,
        desi_refno: RefU64,
    ) -> anyhow::Result<f32> {
        let cata_context = if let Some(cata) = CATAEXPRCONTEXT_MAP.get(&desi_refno) {
            cata.value().clone()
        } else {
            let cata = CataExprContext::create(desi_refno, &self.arango_database)
                .await
                .unwrap_or_default()
                .unwrap_or_default();
            CATAEXPRCONTEXT_MAP.insert(desi_refno, cata.clone());
            cata
        };
        let context = cata_context.build(self, desi_refno).await;
        eval_str_to_f32(expr, &context, Some(self))
    }


    /// 生成元件库的branch型几何体
    pub async fn cache_cata_geos(
        mgr: Arc<AiosDBManager>,
        instance_mgr: Arc<CachedInstanceMgr>,
        project: &str,
        db_nos: Option<&[i32]>,
        db_option: &DbOption,
    ) -> anyhow::Result<bool> {
        let batch_size = mgr.db_option.gen_model_batch_size;
        let mdb = &db_option.mdb_name;
        let t = Instant::now();
        let mut has_cata_refnos = RefU64Vec::default();
        let mut is_debug = false;
        if db_option.debug_refno_types.iter().any(|x| x == "CATA") {
            if let Some(branch_refno) = &db_option.debug_branch_refno {
                has_cata_refnos = RefU64Vec(vec![
                    RefU64::from_refno_str(branch_refno).unwrap_or_default()
                ]);
                is_debug = true;
            } else if let Some(design_refno) = &db_option.debug_desi_refno {
                has_cata_refnos = RefU64Vec(vec![
                    RefU64::from_refno_str(design_refno).unwrap_or_default()
                ]);
                is_debug = true;
            } else if !db_option.debug_root_refnos.is_empty() {
                for root_refno_str in &db_option.debug_root_refnos {
                    if let Ok(root_refno) = RefU64::from_refno_str(root_refno_str) {
                        query_travel_children_with_types_aql(
                            &mgr.arango_database,
                            root_refno,
                            &CATA_ATT_TYPES,
                        )
                            .await?
                            .into_iter()
                            .for_each(|child| {
                                has_cata_refnos.push(child.refno);
                            });
                    }
                }
                is_debug = true;
            }
        }
        if !is_debug {
            has_cata_refnos = mgr
                .get_refnos_by_types(project, &CATA_ATT_TYPES, db_nos)
                .await?;
        }
        let has_cata_cnt = has_cata_refnos.len();
        if has_cata_cnt == 0 { return Ok(true); }
        println!("使用元件库的模型总数：{has_cata_cnt}");
        let batch_chunks_cnt = has_cata_cnt / batch_size + 1;
        let mut handles = vec![];
        let all_refnos = Arc::new(has_cata_refnos);
        // if is_debug {
        //     dbg!(&all_refnos);
        // }
        let processed_cnt = Arc::new(Mutex::new(has_cata_cnt));
        let mut tubi_aqls = Arc::new(DashMap::new());
        let replace_mesh = db_option.replace_mesh;
        let scom_info_map: Arc<RwLock<HashMap<RefU64, ScomInfo>>> = Arc::new(RwLock::new(HashMap::new()));
        for i in 0..batch_chunks_cnt as usize {
            let mgr = mgr.clone();
            let instance_mgr = instance_mgr.clone();
            let all_refnos = all_refnos.clone();
            let processed_cnt = processed_cnt.clone();
            let mut tubi_aqls_clone = tubi_aqls.clone();
            let scom_info_map = scom_info_map.clone();
            let handle = tokio::spawn(async move {
                let start_idx = i * batch_size;
                let mut end_idx = start_idx + batch_size;
                if end_idx > has_cata_cnt as usize {
                    end_idx = has_cata_cnt as usize;
                }
                println!("当前范围: {start_idx} ~ {end_idx}");
                for j in start_idx..end_idx {
                    let refno = all_refnos[j];
                    println!(
                        "正在处理元件库的模型，索引：{}, 当前参考号：{}, 剩余: {}",
                        j,
                        refno.to_refno_string(),
                        processed_cnt.lock().unwrap().to_owned()
                    );
                    let inst_map = &instance_mgr.inst_mgr;
                    let cached_mesh_mgr = &mgr.cached_mesh_mgr;
                    let level_shape_mgr = &instance_mgr.level_shape_mgr;
                    //在这里直接处理完所有需要处理的transform
                    let brep_shapes_map = CateBrepShapeMap::new();
                    let current_att = mgr.get_attr(refno).await.unwrap_or_default();
                    let mut refno_ptset_map = DashMap::new();
                    let cur_type = current_att.get_type();
                    if cur_type == "BRAN" || cur_type == "HANG" {
                        Self::get_cata_auto_tubi_geoms(
                            mgr.clone(),
                            refno,
                            &current_att,
                            &brep_shapes_map,
                            &refno_ptset_map,
                            &mut tubi_aqls_clone,
                            &scom_info_map,
                        )
                            .await
                            .unwrap_or_default();
                    } else {
                        Self::get_cata_single_geoms(
                            mgr.clone(),
                            refno,
                            &brep_shapes_map,
                            &refno_ptset_map,
                            &scom_info_map,
                        )
                            .await
                            .unwrap_or_default();
                    }
                    for (child_refno, shapes) in brep_shapes_map {
                        let trans_origin = mgr
                            .get_world_transform(child_refno)
                            .await
                            .unwrap_or_default()
                            .unwrap_or_default();
                        let ancestors = mgr.get_ancestors_refnos_without_world(child_refno);
                        for p_refno in ancestors {
                            level_shape_mgr
                                .entry(p_refno)
                                .or_insert(RefU64Vec::default())
                                .push(child_refno);
                        }
                        let child_att = mgr.get_attr(child_refno).await.unwrap_or_default();
                        let mut geos_info = EleGeosInfo {
                            _key: child_refno.to_refno_normal_string(),
                            data: vec![],
                            visible: true,
                            generic_type: mgr.get_generic_type(child_refno),
                            aabb: None,
                            world_transform: (
                                trans_origin.rotation,
                                trans_origin.translation,
                                Vec3::ONE,
                            ),
                            ptset_map: refno_ptset_map
                                .remove(&child_refno)
                                .map(|x| x.1)
                                .unwrap_or_default(),
                            flow_pt_indexs: vec![
                                child_att.get_i32("ARRI"),
                                child_att.get_i32("LEAV"),
                            ],
                        };
                        let mut geo_insts = &mut geos_info.data;
                        let mut ele_aabb = Aabb::new_invalid();
                        let mut tubi_aabb = Aabb::new_invalid();
                        let mut has_tubi = false;
                        for shape in shapes {
                            let CateBrepShape {
                                refno,
                                brep_shape,
                                transform,
                                visible,
                                is_tubi,
                                pts,
                                ..
                            } = shape;
                            if !visible || !brep_shape.check_valid() {
                                continue;
                            }
                            let trans = brep_shape.get_trans();
                            let geo_hash =
                                cached_mesh_mgr.gen_pdms_mesh(brep_shape.clone(), replace_mesh);
                            let mut bbox = cached_mesh_mgr.get_bbox(&geo_hash);
                            if bbox.is_none() {
                                // dbg!(refno.to_refno_string());
                                // dbg!(&brep_shape);
                                continue;
                            }
                            let mut aabb = bbox.unwrap();

                            let rot = transform.rotation;
                            let translation =
                                transform.translation + transform.rotation * trans.translation;
                            let scale = trans.scale;
                            aabb = aabb.scaled(&Vector::new(
                                trans.scale.x,
                                trans.scale.y,
                                trans.scale.z,
                            ));
                            let transformed_aabb = aabb.transform_by(&Isometry {
                                rotation: UnitQuaternion::from_quaternion(Quaternion::new(
                                    rot.w, rot.x, rot.y, rot.z,
                                )),
                                translation: Vector::new(
                                    translation.x,
                                    translation.y,
                                    translation.z,
                                )
                                    .into(),
                            });
                            if is_tubi {
                                has_tubi = true;
                                tubi_aabb.merge(&transformed_aabb);
                            } else {
                                ele_aabb.merge(&transformed_aabb);
                            }

                            //tubi 需要特殊处理
                            let geom_inst = EleGeoInstance {
                                geo_hash,
                                refno,
                                pts,
                                aabb,
                                transform: (rot, translation, scale),
                                geo_param: brep_shape
                                    .convert_to_geo_param()
                                    .unwrap_or(PdmsGeoParam::Unknown),
                                visible,
                                is_tubi,
                            };
                            geo_insts.push(geom_inst);
                        }
                        geos_info.aabb = Some(
                            ele_aabb.transform_by(&Isometry {
                                rotation: UnitQuaternion::from_quaternion(Quaternion::new(
                                    trans_origin.rotation.w,
                                    trans_origin.rotation.x,
                                    trans_origin.rotation.y,
                                    trans_origin.rotation.z,
                                )),
                                translation: Vector::new(
                                    trans_origin.translation.x,
                                    trans_origin.translation.y,
                                    trans_origin.translation.z,
                                ).into(),
                            }),
                        );

                        if has_tubi {
                            geos_info.aabb.as_mut().unwrap().merge(&tubi_aabb);
                        }

                        inst_map.insert(child_refno, geos_info);
                    }

                    *processed_cnt.lock().unwrap() -= 1;
                }
            });
            handles.push(handle);
            if !db_option.multi_threads {
                if !handles.is_empty() {
                    futures::future::join_all(take(&mut handles)).await;
                }
            }
        }
        futures::future::join_all(take(&mut handles)).await;
        println!("模型生成完毕,正在保存到数据库");
        let tubi_result = Arc::try_unwrap(tubi_aqls)
            .unwrap()
            .into_iter()
            .map(|x| x.1)
            .collect::<Vec<_>>();
        // dbg!(&tubi_result.len());
        if !tubi_result.is_empty() {
            let conn = mgr.get_arangodb_conn().await.unwrap();
            let json = serde_json::to_value(tubi_result).unwrap_or_default();
            save_arangodb_with_database(json, "tubi_edges", &conn)
                .await
                .unwrap();
        }
        println!(
            "处理元件库几何体: {} 花费时间: {} ms",
            has_cata_cnt,
            t.elapsed().as_millis()
        );
        Ok(true)
    }


    /// 生成基本体的几何数据
    pub async fn cache_prim_geos(
        mgr: Arc<AiosDBManager>,
        instance_mgr: Arc<CachedInstanceMgr>,
        db_option: &DbOption,
        db_nos: Option<&[i32]>,
    ) -> anyhow::Result<bool> {
        let t = Instant::now();
        let batch_size = mgr.db_option.gen_model_batch_size;
        let mut prim_refnos = RefU64Vec::default();
        let mut is_debug = false;
        if db_option.debug_refno_types.iter().any(|x| x == "PRIM") {
            let target_debug_refno = db_option
                .debug_desi_refno
                .as_ref()
                .map(|x| RefU64::from_refno_str(x).unwrap_or_default());
            if target_debug_refno.is_some() {
                prim_refnos = RefU64Vec(vec![target_debug_refno.unwrap()]);
                is_debug = true;
            } else {
                if !db_option.debug_root_refnos.is_empty() {
                    for root_refno_str in &db_option.debug_root_refnos {
                        if let Ok(root_refno) = RefU64::from_refno_str(root_refno_str) {
                            query_travel_children_with_types_aql(
                                &mgr.arango_database,
                                root_refno,
                                &GNERAL_PRIM_NOUN_NAMES,
                            )
                                .await?
                                .iter()
                                .for_each(|x| prim_refnos.push(x.refno));
                        }
                    }
                    is_debug = true;
                }
            }
        }
        if !is_debug {
            prim_refnos = mgr
                .get_refnos_by_types(
                    db_option.project_name.as_str(),
                    &GNERAL_PRIM_NOUN_NAMES,
                    db_nos,
                )
                .await?;
        }
        let prim_cnt = prim_refnos.len();
        dbg!(prim_cnt);
        if prim_cnt == 0 { return Ok(true); }
        let batch_chunks_cnt = prim_cnt / batch_size + 1;
        let mut handles = vec![];
        let all_refnos = Arc::new(prim_refnos);
        let processed_cnt = Arc::new(Mutex::new(prim_cnt));
        let replace_mesh = db_option.replace_mesh;
        for i in 0..batch_chunks_cnt as usize {
            let mgr = mgr.clone();
            let instance_mgr = instance_mgr.clone();

            let all_refnos = all_refnos.clone();
            let processed_cnt = processed_cnt.clone();
            let handle = tokio::spawn(async move {
                let start_idx = i * batch_size;
                let mut end_idx = start_idx + batch_size;
                if end_idx > prim_cnt as usize {
                    end_idx = prim_cnt as usize;
                }

                let inst_map = &instance_mgr.inst_mgr;
                let cached_mesh_mgr = &mgr.cached_mesh_mgr;
                let level_shape_mgr = &instance_mgr.level_shape_mgr;
                for j in start_idx..end_idx {
                    let refno = all_refnos[j];
                    let trans_origin = mgr
                        .get_world_transform(refno)
                        .await
                        .unwrap_or_default()
                        .unwrap_or_default();
                    let ancestors = mgr.get_ancestors_refnos_without_world(refno);
                    //todo 可以使用图数据库的edge 来获取，不用提前存储，暂时可以用来做调试用
                    for p_refno in ancestors {
                        level_shape_mgr
                            .entry(p_refno)
                            .or_insert(RefU64Vec::default())
                            .push(refno);
                    }
                    let mut geos_info = EleGeosInfo {
                        _key: refno.to_refno_normal_string(),
                        data: vec![],
                        visible: true,
                        generic_type: mgr.get_generic_type(refno),
                        aabb: None,
                        world_transform: (
                            trans_origin.rotation,
                            trans_origin.translation,
                            Vec3::ONE,
                        ),
                        ptset_map: default(),
                        flow_pt_indexs: vec![],
                    };
                    let mut geo_insts = &mut geos_info.data;
                    let mut geo_hash = None;
                    let mut item_trans = Transform::default();

                    let attr = mgr.get_attr(refno).await.unwrap_or_default();
                    let mut geo_param = PdmsGeoParam::Unknown;
                    if let Some(brep_obj) = attr.create_brep_shape() {
                        if brep_obj.check_valid() {
                            item_trans = brep_obj.get_trans();
                            geo_param = brep_obj
                                .convert_to_geo_param()
                                .unwrap_or(PdmsGeoParam::Unknown);
                            let r = cached_mesh_mgr.gen_pdms_mesh(brep_obj, replace_mesh);
                            geo_hash = Some(r);
                        }
                    }
                    if let Some(geo_hash) = geo_hash {
                        let visible = attr.is_visible_by_level(None).unwrap_or(true);
                        let tr = &item_trans;
                        let mut bbox = cached_mesh_mgr.get_bbox(&geo_hash);
                        let mut aabb = bbox.unwrap();
                        aabb = aabb.scaled(&Vector::new(tr.scale.x, tr.scale.y, tr.scale.z));
                        let ele_aabb = aabb.transform_by(&Isometry {
                            rotation: UnitQuaternion::from_quaternion(Quaternion::new(
                                tr.rotation.w,
                                tr.rotation.x,
                                tr.rotation.y,
                                tr.rotation.z,
                            )),
                            translation: Vector::new(
                                tr.translation.x,
                                tr.translation.y,
                                tr.translation.z,
                            )
                                .into(),
                        });
                        let geom_inst = EleGeoInstance {
                            geo_hash,
                            refno,
                            pts: Default::default(),
                            aabb,
                            transform: (tr.rotation, tr.translation, tr.scale),
                            geo_param,
                            visible,
                            is_tubi: false,
                        };
                        geo_insts.push(geom_inst);
                        geos_info.aabb = Some(
                            ele_aabb.transform_by(&Isometry {
                                rotation: UnitQuaternion::from_quaternion(Quaternion::new(
                                    trans_origin.rotation.w,
                                    trans_origin.rotation.x,
                                    trans_origin.rotation.y,
                                    trans_origin.rotation.z,
                                )),
                                translation: Vector::new(
                                    trans_origin.translation.x,
                                    trans_origin.translation.y,
                                    trans_origin.translation.z,
                                )
                                    .into(),
                            }),
                        );
                        inst_map.insert(refno, geos_info);
                    }
                }
            });
            handles.push(handle);
            if !db_option.multi_threads {
                if !handles.is_empty() {
                    futures::future::join_all(take(&mut handles)).await;
                }
            }
        }
        futures::future::join_all(take(&mut handles)).await;
        println!(
            "处理常规基本几何体: {} 花费时间: {} ms",
            prim_cnt,
            t.elapsed().as_millis()
        );
        Ok(true)
    }

    pub async fn cache_pohe_geos(mgr: Arc<AiosDBManager>, project: &str) -> anyhow::Result<bool> {
        let pohe_refnos = mgr
            .get_refnos_by_types(project, &vec!["POHE"], Some(&[1]))
            .await?;
        let pohe_cnt = pohe_refnos.len();
        dbg!(pohe_cnt);
        // let mut handles = vec![];
        // for (i, refno) in pohe_refnos.into_iter().enumerate() {
        //     let mgr = mgr.clone();
        //     let handle = tokio::spawn(async move {
        //         let inst_map = &mgr.mesh_mgr.inst_mgr;
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

    pub async fn cache_loop_geos(
        mgr: Arc<AiosDBManager>,
        instance_mgr: Arc<CachedInstanceMgr>,
        db_option: &DbOption,
        db_nos: Option<&[i32]>,
    ) -> anyhow::Result<bool> {
        let t = Instant::now();
        let batch_size = mgr.db_option.gen_model_batch_size;
        let mut loop_refnos = RefU64Vec::default();
        let mut is_debug = false;
        if db_option.debug_refno_types.iter().any(|x| x == "LOOP") {
            let target_debug_refno = db_option
                .debug_desi_refno
                .as_ref()
                .map(|x| RefU64::from_refno_str(x).unwrap_or_default());
            if target_debug_refno.is_some() {
                loop_refnos = RefU64Vec(vec![target_debug_refno.unwrap()]);
                is_debug = true;
            } else if !db_option.debug_root_refnos.is_empty() {
                is_debug = true;
                for root_refno_str in &db_option.debug_root_refnos {
                    if let Ok(root_refno) = RefU64::from_refno_str(root_refno_str) {
                        query_travel_children_with_types_aql(
                            &mgr.arango_database,
                            root_refno,
                            &["PLOO", "LOOP"],
                        )
                            .await?
                            .iter()
                            .for_each(|x| loop_refnos.push(x.refno));
                    }
                }
            }
        }
        if !is_debug {
            loop_refnos = mgr
                .get_refnos_by_types(&db_option.project_name, &["PLOO", "LOOP"], db_nos)
                .await?;
        }
        let loop_cnt = loop_refnos.len();
        dbg!(loop_cnt);
        if loop_cnt == 0 { return Ok(true); }
        //处理loop elements
        let batch_chunks_cnt = loop_cnt / batch_size + 1;
        let mut handles = vec![];
        let all_refnos = Arc::new(loop_refnos);
        let processed_cnt = Arc::new(Mutex::new(loop_cnt));
        let replace_mesh = db_option.replace_mesh;
        for i in 0..batch_chunks_cnt as usize {
            let mgr = mgr.clone();
            let instance_mgr = instance_mgr.clone();

            let all_refnos = all_refnos.clone();
            let processed_cnt = processed_cnt.clone();
            let handle = tokio::spawn(async move {
                let start_idx = i * batch_size;
                let mut end_idx = start_idx + batch_size;
                if end_idx > loop_cnt as usize {
                    end_idx = loop_cnt as usize;
                }

                let inst_map = &instance_mgr.inst_mgr;
                let cached_mesh_mgr = &mgr.cached_mesh_mgr;
                let level_shape_mgr = &instance_mgr.level_shape_mgr;
                for j in start_idx..end_idx {
                    let refno = all_refnos[j];
                    println!(
                        "正在处理loops的模型，索引：{}, 当前参考号：{}, 剩余: {}",
                        j,
                        refno.to_refno_string(),
                        processed_cnt.lock().unwrap().to_owned()
                    );
                    let trans_origin = mgr
                        .get_world_transform(refno)
                        .await
                        .unwrap_or_default()
                        .unwrap_or_default();
                    *processed_cnt.lock().unwrap() -= 1;
                    let Some(refno_basic) = mgr.get_refno_basic(refno) else {
                        continue;
                    };
                    let parent_basic = mgr.get_owner_ref_basic(refno).unwrap();
                    let target_type = parent_basic.get_type();
                    let parent_refno = refno_basic.get_owner();
                    let mut geos_info = EleGeosInfo {
                        _key: refno.to_refno_normal_string(),
                        data: vec![],
                        visible: false,
                        world_transform: (
                            trans_origin.rotation,
                            trans_origin.translation,
                            Vec3::ONE,
                        ),
                        generic_type: mgr.get_generic_type(parent_refno),
                        ptset_map: default(),
                        flow_pt_indexs: vec![],
                        aabb: None,
                    };
                    let mut geo_insts = &mut geos_info.data;
                    let mut target_refno = parent_refno;
                    let mut loop_verts: Vec<Vec3> = vec![];
                    let mut fradius_vec: Vec<f32> = vec![];

                    if let Ok(children_refs) = mgr.get_children_refs(refno).await {
                        for x in children_refs {
                            if let Ok(a) = mgr.get_implicit_attr(x, Some(vec!["POS", "FRAD"])).await {
                                let pt = a.get_position().unwrap_or_default() /*- origin_pt*/;
                                if loop_verts.len() > 0 {
                                    if pt.distance(*loop_verts.last().unwrap()) > EPSILON {
                                        loop_verts.push(pt);
                                        fradius_vec.push(a.get_f32("FRAD").unwrap_or_default());
                                    }
                                } else {
                                    loop_verts.push(pt);
                                    fradius_vec.push(a.get_f32("FRAD").unwrap_or_default());
                                }
                            }
                        }
                    }
                    if loop_verts.is_empty() { continue; }
                    let mut parent_att = AttrMap::default();
                    let mut geo_hash = None;
                    let mut item_trans = Transform::default();
                    let mut geo_param = PdmsGeoParam::Unknown;
                    // dbg!(&target_type);
                    match target_type {
                        "REVO" => {
                            parent_att = mgr.get_attr(parent_refno).await.unwrap_or_default();
                            let angle = parent_att.get_f32("ANGL").unwrap_or_default();
                            if angle >= f32::EPSILON {
                                let revo = Box::new(Revolution {
                                    verts: loop_verts,
                                    fradius_vec,
                                    angle,
                                    ..Default::default()
                                });
                                if revo.check_valid() {
                                    item_trans = revo.get_trans();
                                    geo_param = revo
                                        .convert_to_geo_param()
                                        .unwrap_or(PdmsGeoParam::Unknown);
                                    geo_hash =
                                        Some(cached_mesh_mgr.gen_pdms_mesh(revo, replace_mesh));
                                }
                            }
                        }
                        //todo 关于justline，可能需要jusline的信息才能判断中心点
                        "AEXTR" | "EXTR" | "PANE" | "FLOOR" | "SCREED" | "GWALL" => {
                            let attr = mgr.get_attr(refno).await.unwrap_or_default();
                            parent_att = mgr.get_attr(parent_refno).await.unwrap_or_default();
                            target_refno = parent_refno;
                            let mut height = attr
                                .get_f32("HEIG")
                                .unwrap_or(parent_att.get_f32("HEIG").unwrap_or_default());
                            let i: usize = 0;
                            //fix 1516 的情况  =24381/36952，当为DBOT的时候，会变成DISH
                            let sjus = attr.get_str("SJUS").unwrap_or_default();
                            {
                                //check if all the fradius are the same
                                let r = fradius_vec[0];
                                let all_same = fradius_vec.iter().all(|x| *x == r);
                                let is_dbot = sjus;
                                if all_same && sjus == "DBOT" {
                                    let dish = Box::new(prim_geo::dish::Dish {
                                        pdis: 0.0,
                                        pheig: r,
                                        pdia: r * 2.0,
                                        ..Default::default()
                                    });
                                    // dbg!(&dish);
                                    geo_param = dish
                                        .convert_to_geo_param()
                                        .unwrap_or(PdmsGeoParam::Unknown);
                                    item_trans = dish.get_trans();
                                    let r = cached_mesh_mgr.gen_pdms_mesh(dish, replace_mesh);
                                    geo_hash = Some(r);
                                } else {
                                    let extrusion = Box::new(Extrusion {
                                        verts: loop_verts,
                                        height,
                                        fradius_vec,
                                        ..Default::default()
                                    });
                                    geo_param = extrusion
                                        .convert_to_geo_param()
                                        .unwrap_or(PdmsGeoParam::Unknown);
                                    item_trans = extrusion.get_trans();
                                    let r = cached_mesh_mgr.gen_pdms_mesh(extrusion, replace_mesh);
                                    geo_hash = Some(r);
                                }
                            };

                            // if extrusion.check_valid() {
                            // item_trans = extrusion.get_trans();
                            let off_z = if sjus == "UTOP" || sjus == "DTOP" {
                                -height
                            } else if sjus == "UCEN" || sjus == "DCEN" {
                                -height / 2.0
                            } else {
                                0.0
                            };
                            item_trans.translation =
                                item_trans.translation + Vec3::new(0.0, 0.0, off_z);

                            // }
                        }
                        _ => {}
                    }

                    if let Some(geo_hash) = geo_hash {
                        let visible = parent_att.is_visible_by_level(None).unwrap_or(true);
                        let tr: Transform = item_trans;
                        if let Some(mut aabb) = cached_mesh_mgr.get_bbox(&geo_hash) {
                            aabb =
                                aabb.scaled(&Vector::new(tr.scale.x, tr.scale.y, tr.scale.z));
                            let ele_aabb = aabb.transform_by(&Isometry {
                                rotation: UnitQuaternion::from_quaternion(Quaternion::new(
                                    tr.rotation.w,
                                    tr.rotation.x,
                                    tr.rotation.y,
                                    tr.rotation.z,
                                )),
                                translation: Vector::new(
                                    tr.translation.x,
                                    tr.translation.y,
                                    tr.translation.z,
                                ).into(),
                            });
                            let geom_inst = EleGeoInstance {
                                geo_hash,
                                refno,
                                pts: Default::default(),
                                aabb,
                                transform: (tr.rotation, tr.translation, tr.scale),
                                visible,
                                is_tubi: false,
                                geo_param,
                            };
                            geo_insts.push(geom_inst);
                            geos_info.aabb = Some(
                                ele_aabb.transform_by(&Isometry {
                                    rotation: UnitQuaternion::from_quaternion(Quaternion::new(
                                        trans_origin.rotation.w,
                                        trans_origin.rotation.x,
                                        trans_origin.rotation.y,
                                        trans_origin.rotation.z,
                                    )),
                                    translation: Vector::new(
                                        trans_origin.translation.x,
                                        trans_origin.translation.y,
                                        trans_origin.translation.z,
                                    ).into(),
                                }),
                            );
                        } else {
                            error!("楼板有问题：{} ", refno.to_refno_string());
                        }
                    }
                    let ancestors = mgr.get_ancestors_refnos_without_world(refno);
                    for p_refno in ancestors {
                        level_shape_mgr
                            .entry(p_refno)
                            .or_insert(RefU64Vec::default())
                            .push(target_refno);
                    }
                    inst_map.insert(target_refno, geos_info);
                }
            });
            handles.push(handle);
            if !db_option.multi_threads {
                if !handles.is_empty() {
                    futures::future::join_all(take(&mut handles)).await;
                }
            }
        }
        futures::future::join_all(take(&mut handles)).await;
        println!(
            "处理loops几何体: {} 花费时间: {} ms",
            loop_cnt,
            t.elapsed().as_millis()
        );
        Ok(true)
    }


    // 需要区分project，不同project的mesh，是不同的
    pub async fn cache_geos_data(
        mgr: Arc<AiosDBManager>,
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
            // dbg!(&db_nos);
            info!("当前mdb的所有dbnos: {:?}", db_nos);
        }
        std::fs::create_dir_all("./assets/mesh").unwrap();
        std::fs::create_dir_all("./assets/instance").unwrap();

        for db_no in db_nos {
            if db_option.debug_db_num.is_some() && db_no != db_option.debug_db_num.unwrap() as i32 {
                continue;
            }
            let instance_mgr = CachedInstanceMgr::deserialize_from_bin_file(&format!(
                "./assets/instance/{db_no}.inst"
            )).unwrap_or_default();
            // dbg!(instance_mgr.inst_mgr.len());
            let instance_mgr = Arc::new(instance_mgr);
            let instance_mgr_clone = instance_mgr.clone();
            let db_option_clone = db_option.clone();
            let mgr_clone = mgr.clone();

            println!("开始处理db: {db_no}");

            let d_types = &db_option.debug_refno_types;
            let not_debug = db_option.debug_refno_types.is_empty();
            let mut run_cache_cata = not_debug || d_types.iter().any(|x| x == "CATA");
            let mut run_cache_loop = not_debug || d_types.iter().any(|x| x == "LOOP");
            let mut run_cache_prim = not_debug || d_types.iter().any(|x| x == "PRIM");

            dbg!(run_cache_cata);
            dbg!(run_cache_loop);
            dbg!(run_cache_prim);
            if run_cache_cata {
                let project = project.clone();
                let handle = tokio::spawn(async move {
                    Self::cache_cata_geos(
                        mgr_clone.clone(),
                        instance_mgr_clone.clone(),
                        &project,
                        Some(&[db_no]),
                        &db_option_clone,
                    )
                        .await
                        .unwrap();
                });
                futures::future::join_all(vec![handle]).await;
            }

            if run_cache_prim {
                let instance_mgr_clone = instance_mgr.clone();
                let db_option_clone = db_option.clone();
                let mgr_clone = mgr.clone();
                let handle = tokio::spawn(async move {
                    Self::cache_prim_geos(
                        mgr_clone.clone(),
                        instance_mgr_clone.clone(),
                        &db_option_clone,
                        Some(&[db_no]),
                    )
                        .await
                        .unwrap();
                });
                futures::future::join_all(vec![handle]).await;
            }

            if run_cache_loop {
                let instance_mgr_clone = instance_mgr.clone();
                let db_option_clone = db_option.clone();
                let mgr_clone = mgr.clone();
                let handle = tokio::spawn(async move {
                    Self::cache_loop_geos(
                        mgr_clone.clone(),
                        instance_mgr_clone.clone(),
                        &db_option_clone,
                        Some(&[db_no]),
                    )
                        .await
                        .unwrap();
                });
                futures::future::join_all(vec![handle]).await;
            }

            mgr.cached_mesh_mgr
                .serialize_to_specify_file("./assets/mesh/mesh.bin");

            instance_mgr.serialize_to_specify_file(&format!("./assets/instance/{db_no}.inst"));
            println!("{db_no} 生成完毕。");
        }
        println!("cache all geoms costs: {}ms", time.elapsed().as_millis());
        Ok(true)
    }

    /// 获取缓存好的site
    pub fn get_cached_site_nodes(
        &self,
        world_refno: RefU64,
    ) -> anyhow::Result<Option<Vec<PdmsElement>>> {
        if let Some(k) = CACHED_MDB_SITE_MAP.get(&world_refno) {
            return Ok(Some(k.value().0.clone()));
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
