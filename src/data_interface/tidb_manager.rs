use std::collections::HashMap;
use std::env;
use aios_core::pdms_types::{AiosStr, AttrMap, CachedMeshesMgr, PdmsTree, RefI32Tuple, RefU64, RefU64Vec};
use aios_core::prim_geo::category::{CateBrepShape, convert_to_brep_shapes};
use aios_core::prim_geo::tubing::PdmsTubing;
use anyhow::anyhow;
use approx::abs_diff_eq;
use dashmap::DashMap;
use glam::{Quat, TransformRT, Vec3};
use id_tree::NodeId;
use smol_str::SmolStr;
use sqlx::{MySql, MySqlPool, Pool};
use crate::api::attr::{query_full_attr, query_ori_from_id, query_position_from_id};
use crate::api::element::{query_children, query_children_pdms_tree, query_dbno_from_db, query_dbno_world, query_id_name_from_dbno_type, query_name, query_owner_from_id, query_project_name, query_project_hash, query_type_refnos};
use crate::data_interface::interface::PdmsDataInterface;
use crate::consts::*;
use crate::options::DbOption;
use async_trait::async_trait;

pub type CateBrepShapeMap = HashMap<RefU64, Vec<CateBrepShape>>;

#[derive(Debug)]
pub struct AiosDBManager {
    // db_option 中 include_project 中的所有 project 对应的 db
    pub project_map: DashMap<u32, AiosPdmsProjectTiDB>,
    // 存放 refno_info 的 db
    pub info_db: Pool<MySql>,

    pub projects: Vec<String>,

    pub needed_parse_files: Option<Vec<String>>,

    pub project_path: String,  //整个项目的路径

    pub db_option: DbOption,
}

#[async_trait]
impl PdmsDataInterface for AiosDBManager {
    async fn get_attr(&self, refno: RefU64) -> anyhow::Result<AttrMap> {
        let project_hash = query_project_hash(refno, self.info_db.clone()).await?;
        if let Some(project_pool) = self.project_map.get(&project_hash) {
            let attr = query_full_attr(refno, project_pool.pool.clone()).await?;
            return Ok(attr);
        }
        Ok(AttrMap::default())
    }

    async fn get_ele_children_attrs(&self, refno: RefU64) -> anyhow::Result<Vec<AttrMap>> {
        let project_hash = query_project_hash(refno, self.info_db.clone()).await?;
        let mut r = vec![];
        if let Some(project_pool) = self.project_map.get(&project_hash) {
            let children = query_children(refno, project_pool.pool.clone()).await?;
            for child in children {
                let attr = self.get_attr(child.0).await?;
                r.push(attr);
            }
        }
        Ok(r)
    }

    async fn get_ele_children_refs(&self, refno: RefU64) -> anyhow::Result<RefU64Vec> {
        let project_hash = query_project_hash(refno, self.info_db.clone()).await?;
        let mut result = RefU64Vec::default();
        if let Some(project_pool) = self.project_map.get(&project_hash) {
            let children = query_children(refno, project_pool.pool.clone()).await?;
            children.into_iter().for_each(|child| {
                result.push(child.0);
            });
        }
        Ok(result)
    }

    async fn get_ele_world_transform(&self, refno: RefU64) -> anyhow::Result<Option<TransformRT>> {
        Ok(self.get_world_transform(refno).await?)
    }

    async fn get_name(&self, refno: RefU64) -> anyhow::Result<SmolStr> {
        let project_hash = query_project_hash(refno, self.info_db.clone()).await?;
        if let Some(project_pool) = self.project_map.get(&project_hash) {
            let name = query_name(refno, project_pool.pool.clone()).await?;
            return Ok(SmolStr::new(name));
        }
        Ok(SmolStr::new(""))
    }

    async fn get_refnos_by_type(&self, project_name: SmolStr, att_type: &str) -> anyhow::Result<RefU64Vec> {
        let project_hash = AiosStr(project_name).get_u32_hash();
        if let Some(project_pool) = self.project_map.get(&project_hash) {
            let r = query_type_refnos(att_type, project_pool.pool.clone()).await?;
            return Ok(r);
        }
        Ok(RefU64Vec::default())
    }

    async fn get_tree_root(&self, project: &str, db_no: u32) -> anyhow::Result<Option<(RefU64, AiosStr)>> {
        let project_hash = AiosStr(SmolStr::new(project)).get_u32_hash();
        if let Some(project_pool) = self.project_map.get(&project_hash) {
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
                    project_map.entry(AiosStr(SmolStr::new(project)).get_u32_hash()).or_insert(project_db);
                }
                Err(_) => { dbg!("project: {} init failed",project); }
            }
        }

        let info_db = AiosDBManager::get_db_pool(&default_conn, PDMS_REFNO_INFOS_TABLE).await?;
        let projects = db_option.included_projects.clone();
        Ok(
            Self {
                project_map,
                info_db,
                projects,
                needed_parse_files: None,
                project_path: dir,
                db_option,
            }
        )
    }

    ///获取世界坐标变换矩阵
    #[inline]
    pub async fn get_world_transform(&self, refno: RefU64) -> anyhow::Result<Option<glam::TransformRT>> {
        if let Ok(project) = query_project_name(refno, self.info_db.clone()).await {
            let project_hash = AiosStr(SmolStr::new(project)).get_u32_hash();
            if let Some(mut db) = self.project_map.get(&project_hash) {
                if let Ok(Some(dbno)) = query_dbno_from_db(refno, db.pool.clone()).await {
                    return db.get_world_transform(refno).await;
                }
            }
        }
        Ok(Some(glam::TransformRT::IDENTITY))
    }


    // 返回geo data
    // pub fn get_design_geoms(&mut self, refno: RefU64, cached_mesh_mgr: &mut CachedMeshesMgr) -> anyhow::Result<CateBrepShapeMap> {
    //     let mut result_map = CateBrepShapeMap::new();
    //     let desi_att = self.get_attr(refno)?;
    //     let type_name = desi_att.get_type();
    //     let is_bran = type_name == "BRAN";
    //     if !is_bran {
    //         let geoms = resolve_desi_comp(refno, self).unwrap_or_default();
    //         // dbg!(&geoms);
    //         if type_name == "SCTN" || type_name == "STWALL" || type_name == "GENSEC" {
    //             result_map.insert(refno, create_geos(&desi_att, &geoms, self));
    //         } else {
    //             let mut result_shapes = vec![];
    //             for geom in geoms.geometries {
    //                 if let Some(cate_shape) = convert_to_brep_shapes(&geom) {
    //                     result_shapes.push(cate_shape);
    //                 }
    //             }
    //             result_map.insert(refno, result_shapes);
    //         }
    //     } else {   //先暂时只让旋转用bran
    //         let bran_transform = self.get_world_transform(refno).unwrap_or_default();
    //         let bran_htube_pt = bran_transform.transform_point3(desi_att.get_vec3("HPOS").ok_or(anyhow!("HPOS not exist".to_string()))?);
    //         let bran_ttube_pt = bran_transform.transform_point3(desi_att.get_vec3("TPOS").ok_or(anyhow!("TPOS not exist".to_string()))?);
    //         let htube_ref = desi_att.get_foreign_refno("HSTU").unwrap_or_default();
    //         let mut bore = 0.0f32;
    //         if let Ok(hstube_att) = self.get_attr(htube_ref) {
    //             let hstube_cat_att = self.get_attr(hstube_att.get_foreign_refno("CATR").unwrap_or_default())?;
    //             let params = hstube_cat_att.get_f64_vec("PARA").unwrap_or_default();
    //             if params.len() >= 2 {
    //                 bore = params[1] as f32;
    //             }
    //         }
    //         let mut current_tubing = PdmsTubing {
    //             start_pt: bran_htube_pt,
    //             end_pt: Vec3::ZERO,
    //             bore,
    //             finished: false,
    //         };
    //         let children = self.get_children(refno)?.unwrap_or_default();
    //         if children.len() == 0 {
    //             if !current_tubing.finished && bran_ttube_pt.distance(current_tubing.start_pt) > TUBI_TOL {
    //                 current_tubing.end_pt = bran_ttube_pt;
    //                 current_tubing.finished = true;
    //                 result_map.insert(refno, vec![current_tubing.convert_to_shape()]);
    //             }
    //             return Ok(result_map);
    //         }
    //         //第一遍完成后，然后生成tubing
    //         let last_child = children.last().unwrap().clone();
    //         for child in children {
    //             // if child != RefU64::from_two_nums(16501, 1460) {
    //             //     continue;
    //             // }
    //             let world_trans = self.get_world_transform(child).unwrap_or_default();
    //             let mut result_shapes = vec![];
    //             let geoms = resolve_desi_comp(child, self).unwrap_or_default();
    //             let attr = self.get_attr(child)?;
    //             if let Some(arrive) = attr.get_i32("ARRI") {
    //                 //todo 加入获取arrive position 的方法
    //                 if geoms.axis_map.contains_key(&arrive) {
    //                     let p = &geoms.axis_map[&arrive].pt;
    //                     let a_pos = world_trans.transform_point3(Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32));
    //                     if !current_tubing.finished && a_pos.distance(current_tubing.start_pt) > TUBI_TOL {
    //                         current_tubing.end_pt = a_pos;
    //                         current_tubing.finished = true;
    //                         result_shapes.push(current_tubing.convert_to_shape());
    //                     }
    //                 }
    //             }
    //             if let Some(lstube) = attr.get_foreign_refno("LSTU") {
    //                 if let Ok(lstube_att) = self.get_attr(lstube) {
    //                     let lstube_cat_att = self.get_attr(lstube_att.get_foreign_refno("CATR").unwrap_or_default())?;
    //                     let params = lstube_cat_att.get_f64_vec("PARA").unwrap_or_default();
    //                     if params.len() >= 2 {
    //                         current_tubing.bore = params[1] as f32;
    //                     }
    //                 }
    //             }
    //             if let Some(leave) = attr.get_i32("LEAV") {
    //                 //todo 加入获取leave position 的方法
    //                 // current_tubing
    //                 if geoms.axis_map.contains_key(&leave) {
    //                     let p = &geoms.axis_map[&leave].pt;
    //                     let l_pos = world_trans.transform_point3(Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32));
    //                     current_tubing.start_pt = l_pos;
    //                     current_tubing.finished = false;
    //                 }
    //             }
    //             //管件的生成
    //             for geom in geoms.geometries {
    //                 if let Some(cate_shape) = convert_to_brep_shapes(&geom) {
    //                     result_shapes.push(cate_shape);
    //                     // break;
    //                 }
    //             } // end geoms.geometries
    //             if child == last_child {
    //                 if !current_tubing.finished && bran_ttube_pt.distance(current_tubing.start_pt) > TUBI_TOL {
    //                     current_tubing.end_pt = bran_ttube_pt;
    //                     current_tubing.finished = true;
    //                     result_shapes.push(current_tubing.convert_to_shape());
    //                 }
    //             }
    //             result_map.insert(child, result_shapes);
    //         }
    //     }
    //     Ok(result_map)
    // }

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

#[tokio::test]
async fn test_get_attr() -> anyhow::Result<()> {
    let mgr = AiosDBManager::init_form_config().await?;
    let refno: RefU64 = RefI32Tuple((23584, 8)).into();
    let v = mgr.get_attr(refno).await?;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_get_children_attr() -> anyhow::Result<()> {
    let mgr = AiosDBManager::init_form_config().await?;
    let refno: RefU64 = RefI32Tuple((23584, 7)).into();
    let v = mgr.get_ele_children_attrs(refno).await?;
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