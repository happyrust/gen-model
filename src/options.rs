use aios_core::options::DbOption;
use serde::{Deserialize, Serialize};
use std::ops::{Deref, DerefMut};

/// 扩展DbOption，添加异地部署相关的配置
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DbOptionExt {
    #[serde(flatten)]
    pub inner: DbOption,
    
    /// MQTT服务器地址，用于异地部署
    #[serde(default)]
    pub mqtt_server: Option<String>,
    
    /// MQTT服务器端口，用于异地部署
    #[serde(default)]
    pub mqtt_port: Option<u16>,
    
    /// HTTP数据服务器地址，用于异地部署
    #[serde(default)]
    pub http_server: Option<String>,
    
    /// HTTP数据服务器端口，用于异地部署
    #[serde(default)]
    pub http_port: Option<u16>,
}

impl Deref for DbOptionExt {
    type Target = DbOption;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for DbOptionExt {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl From<DbOption> for DbOptionExt {
    fn from(option: DbOption) -> Self {
        Self {
            inner: option,
            mqtt_server: None,
            mqtt_port: None,
            http_server: None,
            http_port: None,
        }
    }
}

/// 获取扩展的数据库选项
pub fn get_db_option_ext() -> DbOptionExt {
    let db_option = aios_core::get_db_option();
    DbOptionExt::from(db_option.clone())
} 