use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::fs;

/// 元数据文件
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SiteMetadataFile {
    pub env_id: Option<String>,
    pub env_name: Option<String>,
    pub site_id: Option<String>,
    pub site_name: Option<String>,
    pub site_http_host: Option<String>,
    pub generated_at: String,
    pub entries: Vec<SiteMetadataEntry>,
}

/// 单条文件元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteMetadataEntry {
    pub file_name: String,
    pub file_path: String,
    pub file_size: u64,
    pub file_hash: Option<String>,
    pub record_count: Option<u64>,
    pub direction: Option<String>,
    pub source_env: Option<String>,
    pub download_url: Option<String>,
    pub relative_path: Option<String>,
    pub updated_at: String,
}

/// 元数据来源
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataSource {
    LocalPath,
    RemoteHttp,
    Cache,
    Unknown,
}

impl Default for MetadataSource {
    fn default() -> Self {
        MetadataSource::Unknown
    }
}

impl MetadataSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            MetadataSource::LocalPath => "local_path",
            MetadataSource::RemoteHttp => "remote_http",
            MetadataSource::Cache => "cache",
            MetadataSource::Unknown => "unknown",
        }
    }
}

/// 缓存文件结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedMetadata {
    pub cached_at: String,
    pub metadata: SiteMetadataFile,
}

/// 生成统一的当前时间
pub fn timestamp_now() -> String {
    Utc::now().to_rfc3339()
}

/// 清洗路径段，避免非法字符
pub fn sanitize_path_segment(segment: &str) -> String {
    let replaced = segment
        .chars()
        .map(|c| if c == '/' || c == '\\' { '_' } else { c })
        .collect::<String>();
    let trimmed = replaced.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed
    }
}

/// 判断是否为 HTTP URL
pub fn is_http_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

/// 判断是否为本地路径提示
pub fn is_local_path_hint(value: &str) -> bool {
    value.starts_with("file://") || value.starts_with('/') || value.starts_with(".")
}

/// 归一化本地路径
pub fn normalize_local_base(value: &str) -> PathBuf {
    if let Some(stripped) = value.strip_prefix("file://") {
        PathBuf::from(stripped)
    } else {
        PathBuf::from(value)
    }
}

/// 元数据文件路径
pub fn metadata_file_path(base: &Path) -> PathBuf {
    base.join("metadata.json")
}

/// 根据环境和站点计算缓存目录
pub fn metadata_cache_dir(env_id: Option<&str>, site_id: Option<&str>) -> PathBuf {
    let mut path = PathBuf::from("output");
    path.push("remote_sync");
    path.push("metadata_cache");
    if let Some(env) = env_id {
        path.push(sanitize_path_segment(env));
    }
    if let Some(site) = site_id {
        path.push(sanitize_path_segment(site));
    }
    path
}

/// 缓存文件路径
pub fn metadata_cache_path(env_id: Option<&str>, site_id: Option<&str>) -> PathBuf {
    metadata_cache_dir(env_id, site_id).join("metadata.json")
}

/// 写入缓存
pub async fn write_cache(
    env_id: Option<&str>,
    site_id: Option<&str>,
    metadata: &SiteMetadataFile,
) -> Result<PathBuf> {
    let dir = metadata_cache_dir(env_id, site_id);
    fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("创建元数据缓存目录 {:?} 失败", dir))?;
    let path = dir.join("metadata.json");
    let wrapper = CachedMetadata {
        cached_at: timestamp_now(),
        metadata: metadata.clone(),
    };
    let data = serde_json::to_vec_pretty(&wrapper)?;
    fs::write(&path, data)
        .await
        .with_context(|| format!("写入元数据缓存 {:?} 失败", path))?;
    Ok(path)
}

/// 读取缓存
pub async fn read_cache(env_id: Option<&str>, site_id: Option<&str>) -> Result<CachedMetadata> {
    let path = metadata_cache_path(env_id, site_id);
    let contents = fs::read_to_string(&path)
        .await
        .with_context(|| format!("读取元数据缓存 {:?} 失败", path))?;
    let cached: CachedMetadata =
        serde_json::from_str(&contents).with_context(|| format!("解析缓存文件 {:?} 失败", path))?;
    Ok(cached)
}

/// 读取本地元数据
pub async fn read_local_metadata(base: &Path) -> Result<SiteMetadataFile> {
    let path = metadata_file_path(base);
    let contents = fs::read_to_string(&path)
        .await
        .with_context(|| format!("读取元数据文件 {:?} 失败", path))?;
    let metadata: SiteMetadataFile = serde_json::from_str(&contents)
        .with_context(|| format!("解析元数据文件 {:?} 失败", path))?;
    Ok(metadata)
}

/// 写入本地元数据文件
pub async fn write_local_metadata(base: &Path, metadata: &SiteMetadataFile) -> Result<()> {
    fs::create_dir_all(base)
        .await
        .with_context(|| format!("创建元数据目录 {:?} 失败", base))?;
    let path = metadata_file_path(base);
    let data = serde_json::to_vec_pretty(metadata)?;
    fs::write(&path, data)
        .await
        .with_context(|| format!("写入元数据文件 {:?} 失败", path))?;
    Ok(())
}

/// 根据站点 HTTP 根地址获取元数据 URL
pub fn metadata_url(base: &str) -> String {
    format!("{}/metadata.json", base.trim_end_matches('/'))
}

/// 通过 HTTP 拉取远程元数据
pub async fn fetch_remote_metadata(http_base: &str) -> Result<SiteMetadataFile> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .context("创建元数据 HTTP 客户端失败")?;

    let url = metadata_url(http_base);
    let response = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("请求远程元数据 {} 失败", url))?;
    if !response.status().is_success() {
        return Err(anyhow!(
            "远程元数据请求失败({}): {}",
            response.status(),
            url
        ));
    }
    let text = response
        .text()
        .await
        .with_context(|| format!("读取远程元数据 {} 失败", url))?;
    let metadata: SiteMetadataFile =
        serde_json::from_str(&text).with_context(|| format!("解析远程元数据 {} 失败", url))?;
    Ok(metadata)
}
