use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::Mutex;
use std::time::Instant;
use aios_core::consts::*;
use aios_core::pdms_types::{AiosStr, AttrMap, CachedMeshesMgr, EleGeoInstData, EleTreeNode, PdmsMeshMgr, PdmsNodeTrait, PdmsTree, RefI32Tuple, RefU64, RefU64Vec, ShapeInstancesMgr};
use aios_core::prim_geo::category::{CateBrepShape, convert_to_brep_shapes};
use aios_core::prim_geo::extrusion::{CurveType, Extrusion};
use aios_core::prim_geo::facet::{Contour, Facet, Polygon};
use aios_core::prim_geo::revolution::Revolution;
use aios_core::prim_geo::tubing::PdmsTubing;
use aios_core::shape::pdms_shape::{BrepShapeTrait, VerifiedShape};
use anyhow::anyhow;
use approx::{abs_diff_eq, abs_diff_ne};
use dashmap::DashMap;
use glam::{Quat, TransformRT, TransformSRT, Vec3};
use id_tree::NodeId;
use smol_str::SmolStr;
use once_cell::sync::Lazy;
use sqlx::{MySql, MySqlPool, Pool};
use crate::api::attr::{query_full_attr, query_ori_from_id, query_position_from_id};
use crate::api::element::*;
use crate::data_interface::interface::PdmsDataInterface;
use crate::consts::*;
use crate::options::DbOption;
use async_trait::async_trait;

pub const TUBI_TOL: f32 = 10.0f32;
pub type CateBrepShapeMap = HashMap<RefU64, Vec<CateBrepShape>>;
// static GLOBAL_COLLISION_WORLD: Lazy<Mutex<CollisionWorld<f32, (RefU64, RefU64)>>> = Lazy::new(|| {
//     let mut world = CollisionWorld::<f32, (RefU64, RefU64)>::new(0.001f32);
//     Mutex::new(world)
// });

static PRIM_HASH_NOUNS: Lazy<Vec<u32>> = Lazy::new(|| {
    vec![BOX_NOUN, CYLI_NOUN, SPHE_NOUN, CONE_NOUN, CTOR_NOUN, DISH_NOUN,
         LOOP_NOUN, PYRA_NOUN, RTOR_NOUN, REVO_NOUN, POHE_NOUN, PLOO_NOUN, SPINE_NOUN]
});

static GENRIC_NOUN_NAMES: Lazy<Vec<SmolStr>> = Lazy::new(|| {
    vec!["EQUI".into(), "PIPE".into(), "STRU".into(), "ROOM".into(), "STWALL".into(), "FLOOR".into()]
});


#[derive(Debug)]
pub struct AiosDBManager {

    pub project_map: DashMap<String, AiosPdmsProjectTiDB>,

    pub info_db: Pool<MySql>,

    pub projects: Vec<String>,

    pub needed_parse_files: Option<Vec<String>>,

    pub project_path: String,  //整个项目的路径

    pub db_option: DbOption,

    cached_mesh_mgr: CachedMeshesMgr,
}

#[async_trait]
impl PdmsDataInterface for AiosDBManager {

    async fn get_attr(&self, refno: RefU64) -> anyhow::Result<AttrMap> {
        let project_name = query_project_name(refno, self.info_db.clone()).await?;
        if let Some(project_pool) = self.project_map.get(&project_name) {
            let attr = query_full_attr(refno, project_pool.pool.clone()).await?;
            return Ok(attr);
        }
        Ok(AttrMap::default())
    }

    async fn get_world(&self, project: &str, mdb_name: &str, module: &str) -> anyhow::Result<EleTreeNode> {
        if let Some(project_pool) = self.project_map.get(project) {
            let v = query_world("SAMPLE", "DESI", &project_pool.value().pool).await?;
            return Ok(v);
        }
        return Err(anyhow!("World not found".to_string()));
    }

    async fn get_children_nodes(&self, refno: RefU64) -> anyhow::Result<Vec<EleTreeNode>> {
        let project_name = query_project_name(refno, self.info_db.clone()).await?;
        let mut r = vec![];
        if let Some(project_pool) = self.project_map.get(&project_name) {
            let children = query_children(refno, &project_pool.pool).await?;
            for (refno, _) in children {
                let node = query_ele_node(refno, &project_pool.pool).await?;
                r.push(node);
            }
        }
        Ok(r)
    }

    async fn get_children_attrs(&self, refno: RefU64) -> anyhow::Result<Vec<AttrMap>> {
        let project_name = query_project_name(refno, self.info_db.clone()).await?;
        let mut r = vec![];
        if let Some(project_pool) = self.project_map.get(&project_name) {
            let children = query_children(refno, &project_pool.pool).await?;
            for child in children {
                let attr = self.get_attr(child.0).await?;
                r.push(attr);
            }
        }
        Ok(r)
    }

    async fn get_children_refs(&self, refno: RefU64) -> anyhow::Result<RefU64Vec> {
        let project_name = query_project_name(refno, self.info_db.clone()).await?;
        let mut result = RefU64Vec::default();
        if let Some(project_pool) = self.project_map.get(&project_name) {
            let children = query_children(refno, &project_pool.pool).await?;
            children.into_iter().for_each(|child| {
                result.push(child.0);
            });
        }
        Ok(result)
    }

    async fn get_world_transform(&self, refno: RefU64) -> anyhow::Result<Option<TransformRT>> {
        Ok(self.get_world_transform(refno).await?)
    }

    async fn get_name(&self, refno: RefU64) -> anyhow::Result<SmolStr> {
        let project_name = query_project_name(refno, self.info_db.clone()).await?;
        if let Some(project_pool) = self.project_map.get(&project_name) {
            let name = query_name(refno, project_pool.pool.clone()).await?;
            return Ok(SmolStr::new(name));
        }
        Ok(SmolStr::new(""))
    }

    async fn get_refnos_by_type(&self, project: &str, att_type: &str) -> anyhow::Result<RefU64Vec> {
        if let Some(project_pool) = self.project_map.get(project) {
            let r = query_type_refnos(att_type, project_pool.pool.clone()).await?;
            return Ok(r);
        }
        Ok(RefU64Vec::default())
    }

    async fn get_db_world(&self, project: &str, db_no: u32) -> anyhow::Result<Option<(RefU64, AiosStr)>> {
        if let Some(project_pool) = self.project_map.get(project) {
            let r = query_id_name_from_dbno_type(db_no as i32, "WORL", project_pool.pool.clone()).await?;
            if let Some(r) = r {
                return Ok(Some(r[0].clone()));
            }
        }
        return Ok(None);
    }
}


impl AiosDBManager {
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

    #[inline]
    pub async fn get_db_pool(connection_str: &str, project: &str) -> anyhow::Result<Pool<MySql>> {
        MySqlPool::connect(&format!("{connection_str}/{project}")).await.map_err(|x| anyhow!(x.to_string()))
    }

    #[inline]
    pub async fn get_default_pool(conn_str: &str) -> anyhow::Result<Pool<MySql>> {
        MySqlPool::connect(conn_str).await.map_err(|x| anyhow!(x.to_string()))
    }

    pub async fn init_form_config() -> anyhow::Result<Self> {
        let db_option = Self::get_db_option()?;
        Self::init(db_option).await
    }

    pub async fn init(db_option: DbOption) -> anyhow::Result<Self> {
        let dir = db_option.project_path.to_string();
        let mut project_map = DashMap::new();

        let db_option = Self::get_db_option()?;
        let default_conn = AiosDBManager::get_default_conn_str(&db_option);
        for project in &db_option.included_projects {
            let project_pool = AiosDBManager::get_db_pool(&default_conn, project).await;
            match project_pool {
                Ok(pool) => {
                    let project_db = AiosPdmsProjectTiDB { project: project.clone(), pool };
                    project_map.entry(project.clone()).or_insert(project_db);
                }
                Err(_) => { dbg!("project: {} init failed",project); }
            }
        }

        let info_db = AiosDBManager::get_db_pool(&default_conn, PDMS_INFO_DB).await?;
        let projects = db_option.included_projects.clone();
        Ok(
            Self {
                project_map,
                info_db,
                projects,
                needed_parse_files: None,
                project_path: dir,
                db_option,
                cached_mesh_mgr: Default::default()
            }
        )
    }

    ///获取世界坐标变换矩阵
    #[inline]
    pub async fn get_world_transform(&self, refno: RefU64) -> anyhow::Result<Option<glam::TransformRT>> {
        if let Ok(project) = query_project_name(refno, self.info_db.clone()).await {
            if let Some(mut db) = self.project_map.get(&project) {
                if let Ok(Some(dbno)) = query_dbno_from_db(refno, db.pool.clone()).await {
                    return db.get_world_transform(refno).await;
                }
            }
        }
        Ok(Some(glam::TransformRT::IDENTITY))
    }

    /// 返回geo data
    pub async fn get_design_geoms(&mut self, refno: RefU64) -> anyhow::Result<CateBrepShapeMap> {
        let mut result_map = CateBrepShapeMap::new();
        let desi_att = self.get_attr(refno).await?;
        let type_name = desi_att.get_type();
        let is_bran = type_name == "BRAN";
        if !is_bran {
            let geoms = resolve_desi_comp(refno, self).await.unwrap_or_default();
            // dbg!(&geoms);
            if type_name == "SCTN" || type_name == "STWALL" || type_name == "GENSEC" {
                result_map.insert(refno, create_st_geos(&desi_att, &geoms, self).await?);
            } else {
                let mut result_shapes = vec![];
                for geom in geoms.geometries {
                    if let Some(cate_shape) = convert_to_brep_shapes(&geom) {
                        result_shapes.push(cate_shape);
                    }
                }
                result_map.insert(refno, result_shapes);
            }
        } else {   //先暂时只让旋转用bran
            let bran_transform = self.get_world_transform(refno).await?.unwrap_or_default();
            let bran_htube_pt = bran_transform.transform_point3(desi_att.get_vec3("HPOS").ok_or(anyhow!("HPOS not exist".to_string()))?);
            let bran_ttube_pt = bran_transform.transform_point3(desi_att.get_vec3("TPOS").ok_or(anyhow!("TPOS not exist".to_string()))?);
            let htube_ref = desi_att.get_foreign_refno("HSTU").unwrap_or_default();
            let mut bore = 0.0f32;
            if let Ok(hstube_att) = self.get_attr(htube_ref).await {
                let hstube_cat_att = self.get_attr(hstube_att.get_foreign_refno("CATR").unwrap_or_default()).await?;
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
            let children = self.get_children_refs(refno).await.unwrap_or_default();
            if children.len() == 0 {
                if !current_tubing.finished && bran_ttube_pt.distance(current_tubing.start_pt) > TUBI_TOL {
                    current_tubing.end_pt = bran_ttube_pt;
                    current_tubing.finished = true;
                    result_map.insert(refno, vec![current_tubing.convert_to_shape()]);
                }
                return Ok(result_map);
            }
            //第一遍完成后，然后生成tubing
            let last_child = children.last().unwrap().clone();
            for child in children {
                // if child != RefU64::from_two_nums(16501, 1460) {
                //     continue;
                // }
                let world_trans = self.get_world_transform(child).await?.unwrap_or_default();
                let mut result_shapes = vec![];
                let geoms = resolve_desi_comp(child, self).await.unwrap_or_default();
                let attr = self.get_attr(child).await?;
                if let Some(arrive) = attr.get_i32("ARRI") {
                    //todo 加入获取arrive position 的方法
                    if geoms.axis_map.contains_key(&arrive) {
                        let p = &geoms.axis_map[&arrive].pt;
                        let a_pos = world_trans.transform_point3(Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32));
                        if !current_tubing.finished && a_pos.distance(current_tubing.start_pt) > TUBI_TOL {
                            current_tubing.end_pt = a_pos;
                            current_tubing.finished = true;
                            result_shapes.push(current_tubing.convert_to_shape());
                        }
                    }
                }
                if let Some(lstube) = attr.get_foreign_refno("LSTU") {
                    if let Ok(lstube_att) = self.get_attr(lstube).await {
                        let lstube_cat_att = self.get_attr(lstube_att.get_foreign_refno("CATR").unwrap_or_default()).await?;
                        let params = lstube_cat_att.get_f64_vec("PARA").unwrap_or_default();
                        if params.len() >= 2 {
                            current_tubing.bore = params[1] as f32;
                        }
                    }
                }
                if let Some(leave) = attr.get_i32("LEAV") {
                    //todo 加入获取leave position 的方法
                    // current_tubing
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
                        result_shapes.push(cate_shape);
                        // break;
                    }
                } // end geoms.geometries
                if child == last_child {
                    if !current_tubing.finished && bran_ttube_pt.distance(current_tubing.start_pt) > TUBI_TOL {
                        current_tubing.end_pt = bran_ttube_pt;
                        current_tubing.finished = true;
                        result_shapes.push(current_tubing.convert_to_shape());
                    }
                }
                result_map.insert(child, result_shapes);
            }
        }
        Ok(result_map)
    }

    /// 生成模型
    pub async fn cache_geos_data(&mut self, project: &str, mdb: &str) -> anyhow::Result<PdmsMeshMgr> {
        let mut time = Instant::now();
        let mut cached_mesh_mgr = CachedMeshesMgr::default();
        let mut inst_map = HashMap::new();
        let mut level_shape_mgr = HashMap::new();
        // let mut type_geom_refs_map = HashMap::new();
        // let mut type_refs_map = HashMap::new();

        // let root_ele = self.get_world("Sample","SAMPLE", "DESI").await?;
        let root_ele = self.get_world(project,mdb, "DESI").await?;
        dbg!(&root_ele);
        let mut children = self.get_children_nodes(root_ele.refno).await?;
        dbg!(&children);
        while !children.is_empty() {
            let cur_node = children.remove(0);
            dbg!(&cur_node);

            if PRIM_HASH_NOUNS.contains(&cur_node.get_noun_hash()) {

            }

            let nodes = self.get_children_nodes(cur_node.refno).await?;
            children.extend_from_slice(&nodes);
            if children.len() == 50 {
                break;
            }
        }


        // {
        //     let tree = tree.0;
        //     let root_node_id = tree.root_node_id().unwrap();
        //     let node_id = tree.root_node_id().unwrap();
        //
        //     if let Ok(mut nodes) = tree.traverse_level_order_ids(node_id) {
        //         while let Some(mut cur_node_id) = nodes.next() {
        //             let cur_node = tree.get(&cur_node_id).unwrap();
        //             let d = cur_node.data();
        //             let noun = d.noun;
        //             let attr = self.get_attr(d.refno).ok();
        //             if attr.is_none() { continue; }
        //             let attr = attr.unwrap();
        //             let mut geo_hash = None;
        //             let mut color_type = None;
        //             let mut item_trans = glam::TransformSRT::IDENTITY;
        //             let mut target_refno = d.refno;
        //             let mut target_att = attr.clone();
        //             let mut target_node_id = cur_node_id.clone();
        //             if PRIM_HASH_NOUNS.contains(&noun) {
        //                 //获得类型和参考号
        //                 if let Some((noun_name, r)) = self.get_general_type_refno(d.refno) {
        //                     type_geom_refs_map.entry(r).or_insert(Vec::new()).push(d.refno);
        //                     // if type_geom_refs_map.contains_key() { }
        //                     type_refs_map.entry(noun_name.clone()).or_insert(HashSet::new()).insert(r);
        //                     color_type = Some(noun_name);
        //                 }
        //                 if noun == LOOP_NOUN || noun == PLOO_NOUN {
        //                     let parent = attr.get_owner().unwrap();
        //                     target_refno = parent;
        //                     target_node_id = cur_node.parent().unwrap().clone();
        //                     let mut parent_att = self.get_attr(parent)?;
        //                     let parent_noun_name = parent_att.get_type();
        //                     let mut loop_verts: Vec<Vec3> = vec![];
        //                     let mut fradius_vec: Vec<f32> = vec![];
        //                     if let Some(children_refs) = self.get_children(d.refno)? {
        //                         for x in children_refs {
        //                             if let Ok(a) = self.get_attr(x) {
        //                                 loop_verts.push(a.get_position().unwrap_or_default());
        //                                 fradius_vec.push(a.get_f32("FRAD").unwrap_or_default());
        //                             } else {
        //                                 break;
        //                             }
        //                         }
        //                     }
        //                     if parent_noun_name == "REVO" {
        //                         let angle = parent_att.get_f32("ANGL").unwrap_or_default();
        //                         if angle >= f32::EPSILON {
        //                             let revo = Box::new(Revolution {
        //                                 loop_verts,
        //                                 angle,
        //                                 ..Default::default()
        //                             });
        //                             if revo.check_valid() {
        //                                 item_trans = revo.get_trans();
        //                                 let r = cached_mesh_mgr.get_pdms_mesh_hash_key(revo);
        //                                 geo_hash = Some(r);
        //                             }
        //                         }
        //                     } else if parent_noun_name != "NXTR" && parent_noun_name != "NREV" && parent_noun_name != "SCREED" {
        //                         let mut height = attr.get_f32("HEIG").unwrap_or(parent_att.get_f32("HEIG").unwrap_or_default());
        //                         let extrusion = Box::new(Extrusion {
        //                             verts: loop_verts,
        //                             height,
        //                             fradius_vec,
        //                             ..Default::default()
        //                         });
        //                         if extrusion.check_valid() {
        //                             item_trans = extrusion.get_trans();
        //                             if noun == PLOO_NOUN {
        //                                 if let Some(sjus) = attr.get_string("SJUS") {
        //                                     if sjus.as_str() == "UTOP" || sjus.as_str() == "DTOP" {
        //                                         item_trans.translation = item_trans.translation + Vec3::new(0.0, 0.0, -height);
        //                                     }
        //                                 }
        //                             }
        //                             let r = cached_mesh_mgr.get_pdms_mesh_hash_key(extrusion);
        //                             geo_hash = Some(r);
        //                         }
        //                     } //end of LOOP_NOUN
        //                     target_att = parent_att;
        //                 } else if noun == POHE_NOUN {  //多面体, try to save the leaf nodes in database
        //                     let children_hash = self.get_children(d.refno)?.unwrap_or_default();
        //                     let mut facet = Facet::default();
        //                     for x in children_hash {
        //                         let refs = self.get_children(x)?.unwrap_or_default();
        //                         let mut vertices: Vec<[f32; 3]> = vec![];
        //                         let mut tv = vec![];
        //                         let v_cnt = refs.len();
        //                         if v_cnt >= 3 {
        //                             for x in refs {
        //                                 let mut contour = Contour::default();
        //                                 let v = self.get_attr(x)?.get_position().unwrap_or_default();
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
        //                     if facet.check_valid() {
        //                         item_trans = facet.get_trans();
        //                         let r = cached_mesh_mgr.get_pdms_mesh_hash_key(Box::new(facet));
        //                         geo_hash = Some(r);
        //                     }
        //                 } else if noun == SPINE_NOUN {
        //                     let parent = attr.get_owner().unwrap();
        //                     target_refno = parent;
        //                     target_node_id = cur_node.parent().unwrap().clone();
        //                     let mut parent_att = self.get_attr(parent)?;
        //                     let parent_noun_name = parent_att.get_type();
        //                     //todo 假定圆心是 O
        //                     let center = Vec3::ZERO;
        //                     let params = parent_att.get_f64_vec("DESP").unwrap_or_default();
        //                     if params.len() >= 2 {
        //                         let thick = params[0] as f32;
        //                         let height = params[1] as f32;
        //                         if height >= f32::EPSILON && thick >= f32::EPSILON {
        //                             let mut verts: Vec<Vec3> = vec![];
        //                             let mut fradius_vec: Vec<f32> = vec![];
        //                             if let Some(children_refs) = self.get_children(d.refno)? {
        //                                 for x in children_refs {
        //                                     if let Ok(a) = self.get_attr(x) {
        //                                         let p = a.get_position().unwrap_or_default();
        //                                         verts.push(p);
        //                                         let c_rad = a.get_f32("RADI").unwrap_or_default();
        //                                         if abs_diff_ne!(c_rad, 0.0) {
        //                                             fradius_vec.push(c_rad);
        //                                         }
        //                                     }
        //                                 }
        //                             }
        //                             let extrusion = Box::new(Extrusion {
        //                                 verts,
        //                                 height,
        //                                 fradius_vec,
        //                                 cur_type: CurveType::Spine(thick),
        //                                 ..Default::default()
        //                             });
        //                             // dbg!(&extrusion);
        //                             if extrusion.check_valid() {
        //                                 item_trans = extrusion.get_trans();
        //                                 let r = cached_mesh_mgr.get_pdms_mesh_hash_key(extrusion);
        //                                 geo_hash = Some(r);
        //                             }
        //                         } // end height
        //                     }  //end params.len() >= 2
        //                     target_att = parent_att;
        //                 } else {
        //                     if let Some(brep_obj) = attr.create_brep_shape() {
        //                         if brep_obj.check_valid() {
        //                             item_trans = brep_obj.get_trans();
        //                             let r = cached_mesh_mgr.get_pdms_mesh_hash_key(brep_obj);
        //                             geo_hash = Some(r);
        //                         }
        //                     }
        //                 }
        //             } else {
        //                 continue;
        //                 // let ele_type = attr.get_type();
        //                 // let owner = self.get_attr(attr.get_owner().unwrap())?;
        //                 // let has_catref = attr.get_foreign_refno("CATR").is_some() || attr.get_foreign_refno("SPRE").is_some();
        //                 // //针对管道特殊处理
        //                 // if ele_type == "BRAN" || (owner.get_type() != "BRAN" && has_catref) {
        //                 //     let mut node_ids_map = HashMap::new();
        //                 //     for node_id in cur_node.children() {
        //                 //         let data = tree.get(node_id).unwrap().data();
        //                 //         node_ids_map.insert(data.refno, node_id.clone());
        //                 //     }
        //                 //     let brep_shapes = self.get_design_geoms(d.refno, &mut cached_mesh_mgr)?;
        //                 //     // dbg!(&brep_shapes);
        //                 //     for (cur_refno, shapes) in brep_shapes {
        //                 //         //记录对应的不同颜色类型
        //                 //         if let Some((noun_name, r)) = self.get_general_type_refno(d.refno) {
        //                 //             type_geom_refs_map.entry(r).or_insert(Vec::new()).push(d.refno);
        //                 //             type_refs_map.entry(noun_name.clone()).or_insert(HashSet::new()).insert(r);
        //                 //             color_type = Some(noun_name);
        //                 //         }
        //                 //         //维护每个节点有那些几何实例
        //                 //         let ancestors = tree.ancestors(&cur_node_id).unwrap();
        //                 //         for ancestor in ancestors {
        //                 //             let p_refno = ancestor.data().refno;
        //                 //             level_shape_mgr.entry(p_refno).or_insert(RefU64Vec::default()).push(cur_refno);
        //                 //         }
        //                 //         //当前自身也要加进去
        //                 //         if d.refno != cur_refno {
        //                 //             level_shape_mgr.entry(d.refno).or_insert(RefU64Vec::default()).push(cur_refno);
        //                 //         }
        //                 //         let desi_trans_origin = self.get_world_transform(cur_refno).unwrap_or_default();
        //                 //         for shape in shapes {
        //                 //             let CateBrepShape {
        //                 //                 brep_shape,
        //                 //                 mut transform,
        //                 //                 visible,
        //                 //                 is_tubing,
        //                 //             } = shape;
        //                 //             if !visible || !brep_shape.check_valid() { continue; }
        //                 //             item_trans = brep_shape.get_trans();
        //                 //             if !brep_shape.check_valid() {
        //                 //                 continue;
        //                 //             }
        //                 //             let geo_hash = cached_mesh_mgr.get_pdms_mesh_hash_key(brep_shape);
        //                 //             let mut desi_trans = desi_trans_origin.clone();
        //                 //             if !is_tubing {
        //                 //                 desi_trans.translation = desi_trans.translation + desi_trans.rotation * transform.translation;
        //                 //                 desi_trans.rotation = desi_trans.rotation * transform.rotation;
        //                 //             } else {
        //                 //                 desi_trans.translation = transform.translation;
        //                 //                 desi_trans.rotation = transform.rotation;
        //                 //             }
        //                 //             let mut bbox = cached_mesh_mgr.get_bbox(&geo_hash).unwrap();
        //                 //             bbox.scaled(&item_trans.scale);
        //                 //             let geom_data = EleGeoInstData {
        //                 //                 geo_hash,
        //                 //                 bbox,
        //                 //                 global_transform: (desi_trans.rotation, desi_trans.translation, item_trans.scale),
        //                 //                 visible: true,
        //                 //                 generic_type: color_type.clone().unwrap_or_default(),
        //                 //                 zone_refno: self.get_parent_att_by_type(cur_refno, "ZONE")?.map(|x| x.get_refno().unwrap_or_default()).unwrap_or_default(),
        //                 //                 node_id: node_ids_map.get(&cur_refno).map(|x| x.clone()).unwrap_or(cur_node_id.clone()),
        //                 //             };
        //                 //             inst_map.entry(cur_refno).or_insert(Vec::new()).push(geom_data);
        //                 //         }
        //                 //     }
        //                 // }
        //             }
        //             //处理有几何体返回的情况，需要加入到几何列表里
        //             if let Some(geo_hash) = geo_hash {
        //                 let mut ancestors = tree.ancestors(&cur_node_id).unwrap();
        //                 //维护每个节点有那些几何实例
        //                 for ancestor in ancestors {
        //                     let p_refno = ancestor.data().refno;
        //                     level_shape_mgr.entry(p_refno).or_insert(RefU64Vec::default()).push(target_refno);
        //                 }
        //                 let tr: TransformSRT = item_trans * self.get_world_transform(target_refno).unwrap_or_default();
        //                 // let xyz = tr.rotation.to_euler(glam::EulerRot::XYZ);
        //                 // //dbg!((xyz.0.to_degrees(), xyz.1.to_degrees(), xyz.2.to_degrees()));
        //                 // //dbg!(&tr);
        //                 let mut bbox = cached_mesh_mgr.get_bbox(&geo_hash).unwrap();
        //                 bbox.scaled(&tr.scale);
        //                 let geom_data = EleGeoInstData {
        //                     geo_hash,
        //                     bbox,
        //                     global_transform: (tr.rotation, tr.translation, tr.scale),
        //                     visible: target_att.is_visible_by_level(None).unwrap_or(true),
        //                     generic_type: color_type.unwrap_or_default(),
        //                     zone_refno: self.get_parent_att_by_type(target_refno, "ZONE")?.unwrap().get_refno().unwrap(),
        //                     node_id: target_node_id,
        //                 };
        //                 inst_map.entry(target_refno).or_insert(Vec::new()).push(geom_data);
        //             } // end of insert geo_map
        //         }
        //     }
        //
        //
        // }

        // let mut file = File::create(format!("{db_code}_geoms.json")).unwrap();
        // let serialized = serde_json::to_string(&inst_map).unwrap();
        // file.write_all(serialized.as_bytes()).unwrap();


        // let mut file = File::create(format!("type_refs_geoms.json")).unwrap();
        // let serialized = serde_json::to_string(&type_geom_refs_map).unwrap();
        // file.write_all(serialized.as_bytes()).unwrap();
        //
        // let mut file = File::create(format!("type_refs.json")).unwrap();
        // let serialized = serde_json::to_string(&type_refs_map).unwrap();
        // file.write_all(serialized.as_bytes()).unwrap();

        //需要把rooms单独标记出来
        // let mut file = File::create(format!("room_geoms.json")).unwrap();
        // let serialized = serde_json::to_string(&room_geom_refs_map).unwrap();
        // file.write_all(serialized.as_bytes()).unwrap();

        // cached_mesh_mgr.serialize_to_json_file();
        // cached_mesh_mgr.serialize_to_bin_file();
        let mgr = PdmsMeshMgr {
            inst_mgr: ShapeInstancesMgr {
                inst_map
            },
            cached_mesh_mgr,
            level_shape_mgr,
        };
        println!("cache all geoms costs: {}ms", time.elapsed().as_millis());
        mgr.serialize_to_bin_file(mdb);
        Ok(mgr)
    }
}


// 单个project 的 pool
#[derive(Debug)]
pub struct AiosPdmsProjectTiDB {
    pub project: String,
    pub pool: Pool<MySql>,
}

impl AiosPdmsProjectTiDB {
    pub async fn get_attr(&self, refno: RefU64) -> anyhow::Result<AttrMap> {
        query_full_attr(refno, self.pool.clone()).await
    }

    ///获得世界坐标系
    pub async fn get_world_transform(&self, refno: RefU64) -> anyhow::Result<Option<glam::TransformRT>> {
        let mut ancestors = self.get_ancestors_attrs(refno).await;
        ancestors.reverse();
        let mut rotation = Quat::IDENTITY;
        let mut translation = Vec3::ZERO;
        for attr in ancestors {
            let t = if attr.get_type() == "SCTN" || attr.get_type() == "STWALL" {
                let tr = TransformRT {
                    rotation,
                    translation,
                };
                let mut final_rot = Quat::IDENTITY;

                let poss = attr.get_poss().ok_or(anyhow!("can not find poss"))?;
                let pose = attr.get_pose().ok_or(anyhow!("can not find poss"))?;
                let extru_dir: Vec3 = (pose - poss).normalize();
                let bangle = attr.get_f32("BANG").unwrap_or_default();
                //如果和Z轴平行，需要使用Y轴作为参考轴
                let d = extru_dir.dot(Vec3::Z).abs();

                let mut ref_axis = if abs_diff_eq!(1.0, d) {
                    Vec3::Y
                } else { Vec3::Z };

                let p_axis = ref_axis.cross(extru_dir).normalize();
                let y_axis = extru_dir.cross(p_axis).normalize();
                final_rot = Quat::from_mat3(&glam::f32::Mat3::from_cols_array_2d(
                    &[p_axis.to_array(), y_axis.to_array(), extru_dir.to_array()]
                )) * Quat::from_rotation_z(bangle.to_radians());
                final_rot
            } else {
                query_ori_from_id(refno, self.pool.clone()).await?.unwrap_or_default()
            };
            translation = translation + rotation * (query_position_from_id(refno, self.pool.clone())).await?.unwrap_or_default();
            rotation = rotation * t;
        }
        Ok(Some(glam::TransformRT {
            rotation,
            translation,
        }))
    }

    //包含自己
    pub async fn get_ancestors_attrs(&self, refno: RefU64) -> Vec<AttrMap> {
        let mut cur_refno = refno;
        let mut r = vec![];
        while let Ok(attr) = self.get_attr(cur_refno).await {
            if let Ok(Some(owner)) = query_owner_from_id(cur_refno, self.pool.clone()).await {
                r.push(attr);
                cur_refno = owner;
            } else {
                break;
            }
        }
        r
    }
}

use config::{Config, ConfigError, Environment, File};
use crate::cata::query_cata::resolve_desi_comp;
use crate::cata::sctn;
use crate::cata::sctn::geo::create_st_geos;

#[tokio::test]
async fn test_get_attr() -> anyhow::Result<()> {
    let mut mgr = AiosDBManager::init_form_config().await?;
    let refno: RefU64 = RefI32Tuple((23584, 8)).into();
    let v = mgr.get_attr(refno).await?;
    println!("v={:?}", v);

    mgr.cache_geos_data("Sample", "SAMPLE").await?;

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

#[tokio::test]
async fn test_get_ancestors_attrs() -> anyhow::Result<()> {
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let db = AiosPdmsProjectTiDB {
        project: "sample".to_string(),
        pool,
    };
    let refno: RefU64 = RefI32Tuple((23584, 5)).into();
    let v = db.get_ancestors_attrs(refno).await;
    println!("v={:?}", v);
    Ok(())
}