//! Mesh 处理函数
//!
//! 从 gen_model_old.rs 迁移的网格处理相关函数

use aios_core::options::DbOption;
use anyhow::Result;
use std::time::Instant;

use crate::fast_model::query_provider::query_by_type;
use crate::fast_model::process_meshes_update_db_deep;

/// 根据数据库编号批量处理网格数据
///
/// 为指定数据库编号列表中的所有 SITE（场所）节点生成或更新模型网格。
///
/// # 参数
///
/// * `dbnos` - 数据库编号数组
/// * `db_option` - 数据库选项配置
///
/// # 返回值
///
/// 返回 `anyhow::Result<()>` 表示处理是否成功
///
/// # 功能说明
///
/// 1. 根据 `db_option.exclude_db_nums` 过滤数据库编号
/// 2. 对每个数据库查询所有 SITE 类型的节点
/// 3. 调用 `process_meshes_update_db_deep()` 深度处理这些节点的网格
///
/// # 示例
///
/// ```ignore
/// use aios_core::options::DbOption;
/// use gen_model::process_meshes_by_dbnos;
///
/// let dbnos = vec![1, 2, 3];
/// let db_option = DbOption::default();
/// process_meshes_by_dbnos(&dbnos, &db_option).await?;
/// ```
pub async fn process_meshes_by_dbnos(dbnos: &[u32], db_option: &DbOption) -> Result<()> {
    let mut _time = Instant::now();
    let _include_history = db_option.is_gen_history_model();

    // 过滤掉 exclude_db_nums 中的数据库编号
    let filtered_dbnos = if let Some(exclude_nums) = &db_option.exclude_db_nums {
        dbnos
            .iter()
            .filter(|&&dbno| !exclude_nums.contains(&dbno))
            .copied()
            .collect::<Vec<_>>()
    } else {
        dbnos.to_vec()
    };

    for &dbno in &filtered_dbnos {
        let sites = query_by_type(&["SITE"], dbno as i32, None).await?;
        process_meshes_update_db_deep(db_option, &sites)
            .await
            .expect("更新模型数据失败");
    }

    Ok(())
}
