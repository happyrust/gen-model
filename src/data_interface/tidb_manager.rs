use aios_core::pdms_types::AiosStr;
use dashmap::DashMap;
use smol_str::SmolStr;
use sqlx::{MySql, MySqlPool, Pool};
use crate::consts::*;
use crate::database::get_connect_url;
use crate::options::DbOption;

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
                    let project_db = AiosPdmsProjectTiDB { project: project.clone(), pool, };
                    project_map.entry(AiosStr(SmolStr::new(project)).get_u32_hash()).or_insert(project_db);
                }
                Err(_) => { dbg!("project: {} init failed",project); }
            }
        }

        let info_url = get_connect_url(&db_option.ip, &db_option.user, &db_option.password, PDMS_REFNO_INFOS_TABLE, &db_option.port);
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
}

// 单个project 的 pool
#[derive(Debug)]
pub struct AiosPdmsProjectTiDB {
    pub project: String,
    pub pool: Pool<MySql>,
}