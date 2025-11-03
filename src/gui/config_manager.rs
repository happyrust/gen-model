use crate::options::DbOptionExt;
use aios_core::options::DbOption;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 配置站点信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSite {
    pub id: String,
    pub name: String,
    pub description: String,
    pub config: DbOptionExt,
    pub created_at: String,
    pub updated_at: String,
}

/// 配置管理器
pub struct ConfigManager {
    config_dir: PathBuf,
}

impl ConfigManager {
    /// 创建新的配置管理器
    pub fn new() -> Result<Self> {
        let config_dir = Self::get_config_dir()?;
        std::fs::create_dir_all(&config_dir)?;
        Ok(Self { config_dir })
    }

    /// 获取配置目录
    fn get_config_dir() -> Result<PathBuf> {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .context("无法获取用户主目录")?;
        Ok(PathBuf::from(home).join(".aios-database").join("configs"))
    }

    /// 加载配置文件
    pub fn load_config(&self, path: &Path) -> Result<DbOptionExt> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("无法读取配置文件: {:?}", path))?;

        let db_option: DbOption =
            toml::from_str(&content).with_context(|| format!("无法解析配置文件: {:?}", path))?;

        Ok(DbOptionExt::from(db_option))
    }

    /// 保存配置文件
    pub fn save_config(&self, config: &DbOptionExt, path: &Path) -> Result<()> {
        let content = toml::to_string_pretty(&config.inner).context("无法序列化配置")?;

        std::fs::write(path, content).with_context(|| format!("无法写入配置文件: {:?}", path))?;

        Ok(())
    }

    /// 列出所有保存的配置站点
    pub fn list_sites(&self) -> Result<Vec<ConfigSite>> {
        let sites_file = self.config_dir.join("sites.json");
        if !sites_file.exists() {
            return Ok(Vec::new());
        }

        let content = std::fs::read_to_string(&sites_file)?;
        let sites: Vec<ConfigSite> = serde_json::from_str(&content)?;
        Ok(sites)
    }

    /// 保存配置站点
    pub fn save_site(&self, site: ConfigSite) -> Result<()> {
        let mut sites = self.list_sites()?;

        // 检查是否已存在同名站点
        if let Some(pos) = sites.iter().position(|s| s.id == site.id) {
            sites[pos] = site;
        } else {
            sites.push(site);
        }

        let sites_file = self.config_dir.join("sites.json");
        let content = serde_json::to_string_pretty(&sites)?;
        std::fs::write(sites_file, content)?;

        Ok(())
    }

    /// 删除配置站点
    pub fn delete_site(&self, site_id: &str) -> Result<()> {
        let mut sites = self.list_sites()?;
        sites.retain(|s| s.id != site_id);

        let sites_file = self.config_dir.join("sites.json");
        let content = serde_json::to_string_pretty(&sites)?;
        std::fs::write(sites_file, content)?;

        Ok(())
    }

    /// 获取指定站点
    pub fn get_site(&self, site_id: &str) -> Result<Option<ConfigSite>> {
        let sites = self.list_sites()?;
        Ok(sites.into_iter().find(|s| s.id == site_id))
    }

    /// 验证配置
    pub fn validate_config(&self, config: &DbOptionExt) -> Result<Vec<String>> {
        let mut errors = Vec::new();

        // 验证项目路径
        if config.project_path.is_empty() {
            errors.push("项目路径不能为空".to_string());
        } else if !Path::new(&config.project_path).exists() {
            errors.push(format!("项目路径不存在: {}", config.project_path));
        }

        // 验证项目名称
        if config.project_name.is_empty() {
            errors.push("项目名称不能为空".to_string());
        }

        // 验证数据库连接
        if config.v_ip.is_empty() {
            errors.push("数据库IP不能为空".to_string());
        }

        if config.v_port == 0 {
            errors.push("数据库端口无效".to_string());
        }

        // 验证用户名和密码
        if config.v_user.is_empty() {
            errors.push("数据库用户名不能为空".to_string());
        }

        // 验证模型生成参数
        if config.gen_mesh {
            if let Some(ratio) = config.mesh_tol_ratio {
                if ratio <= 0.0 {
                    errors.push("网格容差比率必须大于0".to_string());
                }
            }
        }

        Ok(errors)
    }

    /// 创建默认配置
    pub fn create_default_config() -> DbOptionExt {
        let mut db_option = DbOption::default();

        // 设置默认值
        db_option.project_path = std::env::current_dir()
            .ok()
            .and_then(|p| p.to_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "/path/to/project".to_string());

        db_option.project_name = "DefaultProject".to_string();
        db_option.mdb_name = "ALL".to_string();
        db_option.module = "DESI".to_string();

        db_option.v_ip = "127.0.0.1".to_string();
        db_option.v_port = 8009;
        db_option.v_user = "root".to_string();
        db_option.v_password = "root".to_string();

        db_option.gen_model = true;
        db_option.gen_mesh = false;
        db_option.gen_spatial_tree = true;
        db_option.apply_boolean_operation = true;
        db_option.mesh_tol_ratio = Some(3.0);

        DbOptionExt::from(db_option)
    }

    /// 从当前 DbOption.toml 加载配置
    pub fn load_from_current() -> Result<DbOptionExt> {
        let current_path = PathBuf::from("DbOption.toml");
        if !current_path.exists() {
            return Ok(Self::create_default_config());
        }

        let content = std::fs::read_to_string(&current_path)?;
        let db_option: DbOption = toml::from_str(&content)?;
        Ok(DbOptionExt::from(db_option))
    }

    /// 保存到当前 DbOption.toml
    pub fn save_to_current(&self, config: &DbOptionExt) -> Result<()> {
        let current_path = PathBuf::from("DbOption.toml");
        self.save_config(config, &current_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_default_config() {
        let config = ConfigManager::create_default_config();
        assert!(!config.project_name.is_empty());
        assert!(config.gen_model);
    }

    #[test]
    fn test_validate_config() {
        let manager = ConfigManager::new().unwrap();
        let mut config = ConfigManager::create_default_config();

        // 测试空项目路径
        config.project_path = String::new();
        let errors = manager.validate_config(&config).unwrap();
        assert!(!errors.is_empty());

        // 测试有效配置
        config.project_path = std::env::current_dir()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let errors = manager.validate_config(&config).unwrap();
        assert!(errors.is_empty());
    }
}
