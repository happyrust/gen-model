use crate::fast_model::mesh_generate::process_meshes_update_db_deep;
use aios_core::geometry::ShapeInstancesData;
use aios_core::{RefnoEnum, options::DbOption};
use futures::stream::FuturesUnordered;
use std::sync::Arc;

/// Noun 分类枚举，用于 Full Noun 模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NounCategory {
    /// 使用元件库的 Noun
    Cate,
    /// Loop owner Noun
    LoopOwner,
    /// 基本体 Noun
    Prim,
}

/// 一个db生成模型里，汇总的参考号集合
#[derive(Debug, Clone, Default)]
pub struct DbModelInstRefnos {
    pub bran_hanger_refnos: Arc<Vec<RefnoEnum>>,
    pub use_cate_refnos: Arc<Vec<RefnoEnum>>,
    pub loop_owner_refnos: Arc<Vec<RefnoEnum>>,
    pub prim_refnos: Arc<Vec<RefnoEnum>>,
}

impl DbModelInstRefnos {
    pub async fn execute_gen_inst_meshes(&self, db_option_arc: Option<Arc<DbOption>>) {
        if let Some(db_option) = db_option_arc {
            let mut roots = Vec::new();

            roots.extend(self.bran_hanger_refnos.iter().copied());
            roots.extend(self.use_cate_refnos.iter().copied());
            roots.extend(self.loop_owner_refnos.iter().copied());
            roots.extend(self.prim_refnos.iter().copied());

            if roots.is_empty() {
                return;
            }

            if let Err(e) = process_meshes_update_db_deep(&db_option, &roots).await {
                eprintln!("process_meshes_update_db_deep failed: {:?}", e);
            }
        }
    }
}
