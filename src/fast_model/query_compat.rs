//! 查询兼容层 - 提供与旧 API 兼容的查询函数
//!
//! 此模块提供与原 aios_core 查询函数签名完全兼容的封装，
//! 使得迁移到新的 query_provider 更加平滑。
//!
//! ## 使用方式
//!
//! ```rust,ignore
//! // 旧代码
//! use aios_core::{query_type_refnos_by_dbnum, query_multi_children_refnos};
//!
//! // 新代码 - 只需改变 import
//! use crate::fast_model::query_compat::{query_type_refnos_by_dbnum, query_multi_children_refnos};
//! ```

use crate::fast_model::query_provider;
use aios_core::RefnoEnum;
use aios_core::types::{NamedAttrMap as NamedAttMap, SPdmsElement as PE};

/// 按类型查询 refno (兼容旧 API)
///
/// # 参数
/// - `nouns`: 类型列表
/// - `dbnum`: 数据库编号
/// - `has_children`: 是否过滤有子节点的元素
/// - `_include_history`: 是否包含历史数据 (目前忽略,保持兼容性)
///
/// # 示例
///
/// ```rust,ignore
/// // 查询所有 SITE
/// let sites = query_type_refnos_by_dbnum(&["SITE"], 1112, None, false).await?;
///
/// // 查询有子节点的 ZONE
/// let zones = query_type_refnos_by_dbnum(&["ZONE"], 1112, Some(true), false).await?;
/// ```
pub async fn query_type_refnos_by_dbnum(
    nouns: &[&str],
    dbnum: u32,
    has_children: Option<bool>,
    _include_history: bool, // 暂时忽略历史数据参数
) -> anyhow::Result<Vec<RefnoEnum>> {
    query_provider::query_by_type(nouns, dbnum as i32, has_children).await
}

/// 批量查询多个节点的所有子节点 (兼容旧 API)
///
/// # 参数
/// - `refnos`: 父节点 refno 列表
///
/// # 返回
/// 所有父节点的子节点 refno 列表 (去重)
///
/// # 示例
///
/// ```rust,ignore
/// let children = query_multi_children_refnos(&[zone1, zone2, zone3]).await?;
/// ```
///
/// # 注意
/// **已废弃**: 请使用 `aios_core::collect_descendant_filter_ids(refnos, &[])` 代替
#[deprecated(
    since = "0.1.0",
    note = "使用 aios_core::collect_descendant_filter_ids(refnos, &[], None) 代替"
)]
pub async fn query_multi_children_refnos(refnos: &[RefnoEnum]) -> anyhow::Result<Vec<RefnoEnum>> {
    // 调用 collect_descendant_filter_ids，传入空的 noun 过滤器表示查询所有子节点
    aios_core::collect_descendant_filter_ids(refnos, &[], None).await
}

/// 查询使用特定 CATE 的 refno (兼容旧 API)
///
/// # 参数
/// - `cate_names`: CATE 名称列表
/// - `dbnum`: 数据库编号
/// - `_include_history`: 是否包含历史数据 (目前忽略)
///
/// # 注意
/// 这个函数需要更复杂的实现，暂时返回错误，需要后续完善
pub async fn query_use_cate_refnos_by_dbnum(
    cate_names: &[&str],
    dbnum: u32,
    _include_history: bool,
) -> anyhow::Result<Vec<RefnoEnum>> {
    // TODO: 需要实现 CATE 查询逻辑
    // 目前先调用原有的 aios_core 函数
    aios_core::query_use_cate_refnos_by_dbnum(cate_names, dbnum, _include_history).await
}

/// 获取子节点的 PE 信息 (兼容旧 API)
///
/// # 参数
/// - `refno`: 父节点 refno
///
/// # 返回
/// 子节点的完整 PE 列表
pub async fn get_children_pes(refno: RefnoEnum) -> anyhow::Result<Vec<PE>> {
    query_provider::get_children_pes(refno).await
}

/// 获取单个 PE 信息 (兼容旧 API)
pub async fn get_pe(refno: RefnoEnum) -> anyhow::Result<Option<PE>> {
    query_provider::get_pe(refno).await
}

/// 批量获取 PE 信息 (兼容旧 API)
pub async fn get_pes_batch(refnos: &[RefnoEnum]) -> anyhow::Result<Vec<PE>> {
    query_provider::get_pes_batch(refnos).await
}

/// 获取直接子节点 (兼容旧 API)
pub async fn get_children_refnos(refno: RefnoEnum) -> anyhow::Result<Vec<RefnoEnum>> {
    query_provider::get_children(refno).await
}

/// 查询深层子孙节点 (兼容旧 API)
pub async fn query_deep_children_refnos(refno: RefnoEnum) -> anyhow::Result<Vec<RefnoEnum>> {
    // 使用 collect_descendant_ids_has_inst 获取有实例关系的子孙节点
    // 限制深度为 12 层（与原 max_depth=12 对应）
    aios_core::collect_descendant_ids_has_inst(&[refno], &[], false, Some("..")).await
}

/// 查询过滤后的深层子孙 (兼容旧 API)
///
/// # 注意
/// **已废弃**: 请使用 `aios_core::collect_descendant_filter_ids(&[refno], nouns)` 代替
#[deprecated(
    since = "0.1.0",
    note = "使用 aios_core::collect_descendant_filter_ids(&[refno], nouns, None) 代替"
)]
pub async fn query_filter_deep_children(
    refno: RefnoEnum,
    nouns: &[&str],
) -> anyhow::Result<Vec<RefnoEnum>> {
    // 调用 collect_descendant_filter_ids，将单个 refno 包装为数组
    aios_core::collect_descendant_filter_ids(&[refno], nouns, None).await
}

/// 查询祖先节点 (兼容旧 API)
pub async fn query_ancestor_refnos(refno: RefnoEnum) -> anyhow::Result<Vec<RefnoEnum>> {
    query_provider::get_ancestors(refno).await
}

/// 查询特定类型的祖先 (兼容旧 API)
pub async fn query_filter_ancestors(
    refno: RefnoEnum,
    nouns: &[&str],
) -> anyhow::Result<Vec<RefnoEnum>> {
    query_provider::get_ancestors_of_type(refno, nouns).await
}

// ============================================================================
// 便捷的重导出宏
// ============================================================================

/// 使用此宏可以快速替换所有查询导入
///
/// # 示例
///
/// ```rust,ignore
/// // 在文件开头添加
/// use crate::fast_model::query_compat::*;
///
/// // 然后所有查询函数就会自动使用新的 query_provider
/// ```

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_query_type_compatibility() {
        // 测试兼容性函数是否可以正常调用
        // 需要数据库连接才能运行
    }
}
