use aios_core::pdms_types::{AiosStr, AttrMap, PdmsTree, RefI32Tuple, RefU64, RefU64Vec};
use anyhow::anyhow;
use approx::abs_diff_eq;
use dashmap::DashMap;
use glam::{Quat, TransformRT, Vec3};
use id_tree::NodeId;
use smol_str::SmolStr;
use sqlx::{MySql, MySqlPool, Pool};
use crate::api::attr::{query_full_attr, query_ori_from_id, query_position_from_id};
use crate::api::element::{query_children, query_children_pdms_tree, query_dbno_from_db, query_dbno_world, query_id_name_from_dbno_type, query_name, query_owner_from_id, query_refno_infos, query_refno_infos_hash, query_type_refnos};
use crate::data_interface::data_trait::PdmsDataInterface;
use crate::database::{get_connect_url, get_tidb_pool};
use crate::consts::*;
use crate::options::DbOption;
use async_trait::async_trait;

#[derive(Debug)]
pub struct AiosDBManager {
    // db_option 中 include_project 中的所有 project 对应的 db
    pub project_map: DashMap<u32, AiosPdmsProjectTiDB>,
    // 存放 refno_info 的 db
    pub info_db: Pool<MySql>,

    pub projects: Vec<String>,

    pub needed_parse_files: Option<Vec<String>>,

    pub project_path: String,  //整个项目的路径
}

#[async_trait]
impl PdmsDataInterface for AiosDBManager {
    async fn get_ele_attr(&self, refno: RefU64) -> anyhow::Result<AttrMap> {
        let project_hash = query_refno_infos_hash(refno, self.info_db.clone()).await?;
        if let Some(project_pool) = self.project_map.get(&project_hash) {
            let attr = query_full_attr(refno, project_pool.pool.clone()).await?;
            return Ok(attr);
        }
        Ok(AttrMap::default())
    }

    async fn get_ele_children_attrs(&self, refno: RefU64) -> anyhow::Result<Vec<AttrMap>> {
        let project_hash = query_refno_infos_hash(refno, self.info_db.clone()).await?;
        let mut r = vec![];
        if let Some(project_pool) = self.project_map.get(&project_hash) {
            let children = query_children(refno, project_pool.pool.clone()).await?;
            for child in children {
                let attr = self.get_ele_attr(child.0).await?;
                r.push(attr);
            }
        }
        Ok(r)
    }

    async fn get_ele_children_refs(&self, refno: RefU64) -> anyhow::Result<RefU64Vec> {
        let project_hash = query_refno_infos_hash(refno, self.info_db.clone()).await?;
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
        let project_hash = query_refno_infos_hash(refno, self.info_db.clone()).await?;
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
    pub async fn init(db_option: &DbOption) -> anyhow::Result<Self> {
        let dir = db_option.project_path.to_string();
        let mut project_map = DashMap::new();
        use config::{Config, ConfigError, Environment, File};
        let s = Config::builder()
            .add_source(File::with_name("DbOption"))
            .build()?;
        let db_option: DbOption = s.try_deserialize().unwrap();

        for project in &db_option.included_projects {
            let url = get_connect_url(&db_option.ip, &db_option.user, &db_option.password, project, &db_option.port);
            let project_pool = MySqlPool::connect(&url).await;
            match project_pool {
                Ok(pool) => {
                    let project_db = AiosPdmsProjectTiDB { project: project.clone(), pool };
                    project_map.entry(AiosStr(SmolStr::new(project)).get_u32_hash()).or_insert(project_db);
                }
                Err(_) => { dbg!("project: {} init failed",project); }
            }
        }

        let info_url = get_connect_url(&db_option.ip, &db_option.user, &db_option.password, PDMS_INFO_DB, &db_option.port);
        let info_db = MySqlPool::connect(&info_url).await?;
        Ok(
            Self {
                project_map,
                info_db,
                projects: db_option.included_projects,
                needed_parse_files: None,
                project_path: dir,
            }
        )
    }

    ///获取世界坐标变换矩阵
    #[inline]
    pub async fn get_world_transform(&self, refno: RefU64) -> anyhow::Result<Option<glam::TransformRT>> {
        if let Ok(project) = query_refno_infos(refno, self.info_db.clone()).await {
            let project_hash = AiosStr(SmolStr::new(project)).get_u32_hash();
            if let Some(mut db) = self.project_map.get(&project_hash) {
                if let Ok(Some(dbno)) = query_dbno_from_db(refno, db.pool.clone()).await {
                    return db.get_world_transform(refno).await;
                }
            }
        }
        Ok(Some(glam::TransformRT::IDENTITY))
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
                query_ori_from_id(refno,self.pool.clone()).await?.unwrap_or_default()
            };
            translation = translation + rotation * (query_position_from_id(refno,self.pool.clone())).await?.unwrap_or_default();
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
            if let Ok(Some(owner)) = query_owner_from_id(cur_refno,self.pool.clone()).await {
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
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let mgr = AiosDBManager::init(&db_option).await?;
    let refno: RefU64 = RefI32Tuple((23584, 8)).into();
    let v = mgr.get_ele_attr(refno).await?;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_get_children_attr() -> anyhow::Result<()> {
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let mgr = AiosDBManager::init(&db_option).await?;
    let refno: RefU64 = RefI32Tuple((23584, 7)).into();
    let v = mgr.get_ele_children_attrs(refno).await?;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_get_ancestors_attrs() {
    let url = "mysql://root:root@127.0.0.1:3306";
    let pool = get_tidb_pool(&format!("{}/{}", url, "sample")).await;
    let db = AiosPdmsProjectTiDB {
        project: "sample".to_string(),
        pool
    };
    let refno : RefU64 = RefI32Tuple((23584,5)).into();
    let v = db.get_ancestors_attrs(refno).await;
    println!("v={:?}",v);
}