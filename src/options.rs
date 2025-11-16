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

    /// 目标会话号，用于历史模型生成
    #[serde(default)]
    pub target_sesno: Option<u32>,

    /// 启用全库 Noun 扫描模式（不按 dbno/refno 层级过滤）
    #[serde(default)]
    pub full_noun_mode: bool,

    /// Full Noun 模式下同时进行的 Noun 级任务数量
    /// 默认为 None 时使用合理的并发数（如 CPU 核数）
    #[serde(default)]
    pub full_noun_max_concurrent_nouns: Option<usize>,

    /// Full Noun 模式下单个 Noun 的 refno 列表按批次切分的大小
    /// 默认为 None 时复用 gen_model_batch_size
    #[serde(default)]
    pub full_noun_batch_size: Option<usize>,

    /// Full Noun 模式下启用的 noun 类别列表
    /// 可选值: "cate", "loop", "prim" 或具体 noun 名称如 "BRAN", "PANE"
    /// 空 vec 表示启用所有类别（默认行为）
    #[serde(default)]
    pub full_noun_enabled_categories: Vec<String>,

    /// Full Noun 模式下禁用的 noun 列表
    /// 即使类别启用，这里的 noun 也会被过滤掉
    #[serde(default)]
    pub full_noun_excluded_nouns: Vec<String>,
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

impl DbOptionExt {
    /// 获取 Full Noun 模式下的实际并发数
    /// 如果未配置，返回 CPU 核数（最小为 2，最大为 8）
    pub fn get_full_noun_concurrency(&self) -> usize {
        self.full_noun_max_concurrent_nouns.unwrap_or_else(|| {
            let cpu_count = num_cpus::get();
            cpu_count.clamp(2, 8)
        })
    }

    /// 获取 Full Noun 模式下的实际批次大小
    /// 如果未配置，复用 gen_model_batch_size
    pub fn get_full_noun_batch_size(&self) -> usize {
        self.full_noun_batch_size
            .unwrap_or(self.inner.gen_model_batch_size)
    }

    /// 检查 noun 类别是否启用
    /// 空列表表示启用所有类别
    pub fn is_noun_category_enabled(&self, category: &str) -> bool {
        self.full_noun_enabled_categories.is_empty()
            || self.full_noun_enabled_categories.iter()
                .any(|cat| cat == category || cat.to_lowercase() == category.to_lowercase())
    }

    /// 检查具体 noun 是否被排除
    pub fn is_noun_excluded(&self, noun: &str) -> bool {
        self.full_noun_excluded_nouns.iter()
            .any(|excluded| excluded == noun || excluded.to_lowercase() == noun.to_lowercase())
    }

    /// 检查具体 noun 是否在启用的列表中（当使用具体 noun 名称时）
    pub fn is_noun_explicitly_enabled(&self, noun: &str) -> bool {
        // 如果启用了具体 noun 名称，则检查
        !self.full_noun_enabled_categories.is_empty() 
            && (self.full_noun_enabled_categories.iter()
                .any(|cat| cat == noun || cat.to_lowercase() == noun.to_lowercase())
                // 也检查类别名称
                || self.is_noun_category_enabled(noun))
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
            target_sesno: None,
            full_noun_mode: false,
            full_noun_max_concurrent_nouns: None,
            full_noun_batch_size: None,
            full_noun_enabled_categories: Vec::new(),
            full_noun_excluded_nouns: Vec::new(),
        }
    }
}

/// 获取扩展的数据库选项
pub fn get_db_option_ext() -> DbOptionExt {
    let db_option = aios_core::get_db_option();
    DbOptionExt::from(db_option.clone())
}

/// 从指定路径加载扩展的数据库选项
pub fn get_db_option_ext_from_path(config_path: &str) -> anyhow::Result<DbOptionExt> {
    use config::{Config, File};

    // 使用 config crate 加载基础 DbOption
    let s = Config::builder()
        .add_source(File::with_name(config_path))
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build config: {}", e))?;

    let db_option = s.try_deserialize::<DbOption>()
        .map_err(|e| anyhow::anyhow!("Failed to deserialize DbOption: {}", e))?;

    // 读取 TOML 文件内容以提取扩展字段
    let config_file = format!("{}.toml", config_path);
    let content = std::fs::read_to_string(&config_file)
        .map_err(|e| anyhow::anyhow!("Failed to read config file {}: {}", config_file, e))?;

    // 解析 TOML 以提取扩展字段
    let toml_value: toml::Value = toml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Failed to parse TOML from {}: {}", config_file, e))?;

    // 提取扩展字段
    let full_noun_mode = toml_value.get("full_noun_mode")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let full_noun_max_concurrent_nouns = toml_value.get("full_noun_max_concurrent_nouns")
        .and_then(|v| v.as_integer())
        .map(|v| v as usize);

    let full_noun_batch_size = toml_value.get("full_noun_batch_size")
        .and_then(|v| v.as_integer())
        .map(|v| v as usize);

    // 解析启用的 noun 类别
    let full_noun_enabled_categories = toml_value.get("full_noun_enabled_categories")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    // 解析禁用的 noun 列表
    let full_noun_excluded_nouns = toml_value.get("full_noun_excluded_nouns")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    // 构建 DbOptionExt
    let db_option_ext = DbOptionExt {
        inner: db_option,
        mqtt_server: None,
        mqtt_port: None,
        http_server: None,
        http_port: None,
        target_sesno: None,
        full_noun_mode,
        full_noun_max_concurrent_nouns,
        full_noun_batch_size,
        full_noun_enabled_categories,
        full_noun_excluded_nouns,
    };

    // 打印加载的配置
    println!("📋 加载的配置:");
    println!(
        "   - default_lod: {:?}",
        db_option_ext.inner.mesh_precision.default_lod
    );
    println!(
        "   - LOD profiles 数量: {}",
        db_option_ext.inner.mesh_precision.lod_profiles.len()
    );
    println!(
        "   - full_noun_mode: {}",
        db_option_ext.full_noun_mode
    );
    if !db_option_ext.full_noun_enabled_categories.is_empty() {
        println!(
            "   - 启用的 noun 类别: {:?}",
            db_option_ext.full_noun_enabled_categories
        );
    }
    if !db_option_ext.full_noun_excluded_nouns.is_empty() {
        println!(
            "   - 排除的 noun: {:?}",
            db_option_ext.full_noun_excluded_nouns
        );
    }

    Ok(db_option_ext)
}
