use super::cate_single;
use super::context::NounProcessContext;
use crate::fast_model::cata_model;
use aios_core::RefnoEnum;
use aios_core::geometry::ShapeInstancesData;
use aios_core::options::DbOption;
use anyhow::Result;
use dashmap::DashMap;
use glam::Vec3;
use std::sync::Arc;

/// 处理 Cate (元件库) 类型的 refno 页面
///
/// # Arguments
/// * `ctx` - 处理上下文
/// * `loop_sjus_map_arc` - Loop SJUS 映射
/// * `sender` - 几何数据发送通道
/// * `refnos` - 要处理的 refno 列表
pub async fn process_cate_refno_page(
    ctx: &NounProcessContext,
    loop_sjus_map_arc: Arc<DashMap<RefnoEnum, (Vec3, f32)>>,
    sender: flume::Sender<ShapeInstancesData>,
    refnos: &[RefnoEnum],
) -> Result<()> {
    if refnos.is_empty() {
        return Ok(());
    }

    // 查询 refnos 对应的 cata hash 分组
    let target_cata_map = Arc::new(
        aios_core::query_group_by_cata_hash(refnos)
            .await
            .unwrap_or_default(),
    );

    if target_cata_map.is_empty() {
        return Ok(());
    }

    // 生成 cata 几何体
    cata_model::gen_cata_instances(
        ctx.db_option.clone(),
        target_cata_map,
        loop_sjus_map_arc,
        sender,
    )
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_empty_refnos() {
        let ctx = NounProcessContext::new(Arc::new(DbOption::default()), 100, 4);
        let loop_sjus_map = Arc::new(DashMap::new());
        let (sender, _receiver) = flume::unbounded();

        let result = process_cate_refno_page(&ctx, loop_sjus_map, sender, &[]).await;
        assert!(result.is_ok());
    }
}
