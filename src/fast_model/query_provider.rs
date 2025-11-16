//! 模型生成专用的查询提供者
//!
//! 使用 SurrealDB 作为层级查询的数据源。
//!
//! # 使用示例
//!
//! ```rust,ignore
//! use crate::fast_model::query_provider::*;
//!
//! // 获取层级过滤的子孙节点
//! let descendants = get_descendants_by_types(
//!     zone_refno,
//!     &["EQUI", "PIPE"],
//!     Some(12)
//! ).await?;
//! ```

use aios_core::RefnoEnum;
use aios_core::query_provider::*;
use aios_core::types::{NamedAttrMap as NamedAttMap, SPdmsElement as PE};
use aios_core::mdb;
use aios_core::rs_surreal;
use once_cell::sync::OnceCell;
use std::sync::Arc;

/// 全局查询提供者实例
static GLOBAL_PROVIDER: OnceCell<Arc<dyn QueryProvider>> = OnceCell::new();

/// 获取用于模型生成的查询提供者
///
pub async fn get_model_query_provider() -> anyhow::Result<Arc<dyn QueryProvider>> {
    if let Some(provider) = GLOBAL_PROVIDER.get() {
        return Ok(provider.clone());
    }

    let provider = init_provider().await?;
    let _ = GLOBAL_PROVIDER.set(provider.clone());
    Ok(provider)
}

/// 初始化查询提供者
async fn init_provider() -> anyhow::Result<Arc<dyn QueryProvider>> {
    log::info!("使用 SurrealDB 查询提供者");
    let router = QueryRouter::surreal_only()?;
    Ok(Arc::new(router))
}

// ============================================================================
// 便捷查询函数 (替换 fast_model/query.rs 中的现有函数)
// ============================================================================

/// 查询深层子孙节点并按类型过滤
///
/// # 参数
/// - `root`: 根节点 refno
/// - `nouns`: 要过滤的类型列表
/// - `max_depth`: 最大递归深度 (已忽略，保持兼容性)
///
/// # 示例
///
/// ```rust,ignore
/// // 查询 ZONE 下所有 EQUI 和 PIPE
/// let equips = get_descendants_by_types(
///     zone_refno,
///     &["EQUI", "PIPE"],
///     Some(12)
/// ).await?;
/// ```
///
/// # 注意
/// **已废弃**: 请使用 `aios_core::collect_descendant_filter_ids(&[root], nouns)` 代替
///
/// `max_depth` 参数已被忽略，因为 `collect_descendant_filter_ids` 会查询所有深度的子孙节点。
#[deprecated(
    since = "0.1.0",
    note = "使用 aios_core::collect_descendant_filter_ids(&[root], nouns, None) 代替"
)]
pub async fn get_descendants_by_types(
    root: RefnoEnum,
    nouns: &[&str],
    _max_depth: Option<usize>, // 参数保留以保持兼容性，但已忽略
) -> anyhow::Result<Vec<RefnoEnum>> {
    // 将单个 refno 包装为数组
    aios_core::collect_descendant_filter_ids(&[root], nouns, None).await
}

/// 批量获取子节点
///
/// # 参数
/// - `refnos`: 父节点 refno 列表
///
/// # 返回
/// 所有父节点的子节点 refno 列表 (去重)
///
/// # 注意
/// **已废弃**: 请使用 `aios_core::collect_descendant_filter_ids(refnos, &[])` 代替
///
/// 此函数现在直接调用 `collect_descendant_filter_ids`，传入空的 noun 过滤器表示查询所有子节点。
#[deprecated(
    since = "0.1.0",
    note = "使用 aios_core::collect_descendant_filter_ids(refnos, &[], None) 代替"
)]
pub async fn get_children_batch(refnos: &[RefnoEnum]) -> anyhow::Result<Vec<RefnoEnum>> {
    // 传入空的 noun 过滤器表示查询所有类型的子节点
    aios_core::collect_descendant_filter_ids(refnos, &[], None).await
}

/// 查询指定类型的节点
///
/// # 参数
/// - `nouns`: 类型列表
/// - `dbnum`: 数据库编号
/// - `has_children`: 是否过滤有子节点的元素
///
/// # 示例
///
/// ```rust,ignore
/// // 查询 1112 数据库中所有 ZONE
/// let zones = query_by_type(&["ZONE"], 1112, None).await?;
///
/// // 查询有子节点的 ZONE
/// let parent_zones = query_by_type(&["ZONE"], 1112, Some(true)).await?;
/// ```
pub async fn query_by_type(
    nouns: &[&str],
    dbnum: i32,
    has_children: Option<bool>,
) -> anyhow::Result<Vec<RefnoEnum>> {
    let provider = get_model_query_provider().await?;
    provider
        .query_by_type(nouns, dbnum, has_children)
        .await
        .map_err(Into::into)
}

/// 按 Noun 全库查询（Full Noun 模式专用）
///
/// 直接按 Noun 类型查询全库范围内的所有实例，不加 dbno 或 refno 层级约束。
///
/// # 参数
/// - `nouns`: Noun 类型列表（如 ["EQUI", "FITT", "BOX"]）
///
/// # 返回
/// 全库范围内所有匹配 Noun 的 refno 列表
///
/// # 示例
///
/// ```rust,ignore
/// // 查询全库所有 EQUI 和 FITT
/// let refnos = query_by_noun_all_db(&["EQUI", "FITT"]).await?;
/// ```
///
/// # 实现说明
///
/// 此函数使用 `rs_surreal::mdb::query_type_refnos_by_dbnums` 并传入空的 dbnums 列表，
/// 这会触发"查询所有数据库"的逻辑（见 `query_type_refnos_by_dbnums` 实现中的 `if dbnums.is_empty()` 分支）。
pub async fn query_by_noun_all_db(nouns: &[&str]) -> anyhow::Result<Vec<RefnoEnum>> {
    // 传入空的 dbnums 列表，触发全库查询
    let empty_dbnums: Vec<u32> = vec![];
    mdb::query_type_refnos_by_dbnums(nouns, &empty_dbnums)
        .await
        .map_err(Into::into)
}

/// 统计指定 noun 在全库范围内的实例数量（GROUP ALL + LIMIT 1）
pub async fn count_noun_all_db(noun: &str) -> anyhow::Result<u64> {
    mdb::count_refnos_by_noun(noun)
        .await
        .map_err(Into::into)
}

/// 根据分页参数获取指定 noun 的 refno 列表
pub async fn query_noun_page_all_db(
    noun: &str,
    start: usize,
    limit: usize,
) -> anyhow::Result<Vec<RefnoEnum>> {
    mdb::query_refnos_by_noun_page(noun, start, limit)
        .await
        .map_err(Into::into)
}

/// 批量获取 PE 信息
///
/// # 参数
/// - `refnos`: refno 列表
///
/// # 返回
/// PE 列表 (保持顺序，如果某个 refno 不存在则跳过)
pub async fn get_pes_batch(refnos: &[RefnoEnum]) -> anyhow::Result<Vec<PE>> {
    let provider = get_model_query_provider().await?;
    provider.get_pes_batch(refnos).await.map_err(Into::into)
}

/// 获取单个 PE 信息
///
/// # 参数
/// - `refno`: PE 的 refno
///
/// # 返回
/// PE 信息，如果不存在返回 None
pub async fn get_pe(refno: RefnoEnum) -> anyhow::Result<Option<PE>> {
    let provider = get_model_query_provider().await?;
    provider.get_pe(refno).await.map_err(Into::into)
}

/// 获取直接子节点
///
/// # 参数
/// - `refno`: 父节点 refno
///
/// # 返回
/// 子节点的 refno 列表
pub async fn get_children(refno: RefnoEnum) -> anyhow::Result<Vec<RefnoEnum>> {
    let provider = get_model_query_provider().await?;
    provider.get_children(refno).await.map_err(Into::into)
}

/// 查询所有祖先节点
///
/// # 参数
/// - `refno`: 子节点 refno
///
/// # 返回
/// 祖先节点 refno 列表 (从直接父节点到根节点)
pub async fn get_ancestors(refno: RefnoEnum) -> anyhow::Result<Vec<RefnoEnum>> {
    let provider = get_model_query_provider().await?;
    provider.get_ancestors(refno).await.map_err(Into::into)
}

/// 查询特定类型的祖先
///
/// # 参数
/// - `refno`: 子节点 refno
/// - `nouns`: 要过滤的类型列表
///
/// # 返回
/// 匹配类型的祖先节点 refno 列表
pub async fn get_ancestors_of_type(
    refno: RefnoEnum,
    nouns: &[&str],
) -> anyhow::Result<Vec<RefnoEnum>> {
    let provider = get_model_query_provider().await?;
    provider
        .get_ancestors_of_type(refno, nouns)
        .await
        .map_err(Into::into)
}

/// 获取子节点的完整 PE 信息
///
/// # 参数
/// - `refno`: 父节点 refno
///
/// # 返回
/// 子节点的完整 PE 列表
pub async fn get_children_pes(refno: RefnoEnum) -> anyhow::Result<Vec<PE>> {
    let provider = get_model_query_provider().await?;
    provider.get_children_pes(refno).await.map_err(Into::into)
}

/// 批量获取属性映射
///
/// # 参数
/// - `refnos`: refno 列表
///
/// # 返回
/// NamedAttMap 列表
pub async fn get_attmaps_batch(refnos: &[RefnoEnum]) -> anyhow::Result<Vec<NamedAttMap>> {
    let provider = get_model_query_provider().await?;
    provider.get_attmaps_batch(refnos).await.map_err(Into::into)
}

/// 多起点、多类型的深层子孙查询
///
/// # 参数
/// - `refnos`: 起点节点列表
/// - `nouns`: 要过滤的类型列表
///
/// # 返回
/// 匹配条件的 refno 列表
///
/// # 注意
/// **已废弃**: 请直接使用 `aios_core::collect_descendant_filter_ids(refnos, nouns)` 代替
///
/// 此函数现在直接调用 `collect_descendant_filter_ids`，未来版本将移除。
#[deprecated(
    since = "0.1.0",
    note = "使用 aios_core::collect_descendant_filter_ids(refnos, nouns, None) 代替"
)]
pub async fn query_multi_descendants(
    refnos: &[RefnoEnum],
    nouns: &[&str],
) -> anyhow::Result<Vec<RefnoEnum>> {
    // 直接调用 aios_core 的统一接口
    aios_core::collect_descendant_filter_ids(refnos, nouns, None).await
}

// ============================================================================
// 诊断和调试函数
// ============================================================================

/// 获取当前使用的查询提供者名称
///
/// 用于调试和日志输出
pub async fn get_provider_name() -> String {
    match get_model_query_provider().await {
        Ok(provider) => provider.provider_name().to_string(),
        Err(_) => "未初始化".to_string(),
    }
}

/// 健康检查
///
/// 检查数据库连接是否正常
pub async fn health_check() -> anyhow::Result<bool> {
    let provider = get_model_query_provider().await?;
    provider.health_check().await.map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_provider_initialization() {
        let provider = get_model_query_provider().await;
        assert!(provider.is_ok());
    }

    #[tokio::test]
    async fn test_provider_name() {
        let name = get_provider_name().await;
        assert!(!name.is_empty());
        println!("当前查询提供者: {}", name);
    }
}
