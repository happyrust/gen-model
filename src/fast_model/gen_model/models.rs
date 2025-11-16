use std::sync::Arc;
use aios_core::{RefnoEnum, options::DbOption};
use aios_core::geometry::ShapeInstancesData;
use futures::stream::FuturesUnordered;
use crate::fast_model::{gen_meshes_in_db, booleans_meshes_in_db};

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
        let mut handles = FuturesUnordered::new();
        let prim_refnos = self.prim_refnos.clone();
        let loop_owner_refnos = self.loop_owner_refnos.clone();
        let use_cate_refnos = self.use_cate_refnos.clone();
        let bran_hanger_refnos = self.bran_hanger_refnos.clone();

        let db_option = db_option_arc.clone();
        handles.push(tokio::spawn(async move {
            gen_meshes_in_db(db_option, &prim_refnos)
                .await
                .expect("更新prim模型数据失败");
        }));

        let db_option = db_option_arc.clone();
        handles.push(tokio::spawn(async move {
            booleans_meshes_in_db(db_option, &loop_owner_refnos)
                .await
                .expect("更新loop模型数据失败");
        }));

        let db_option = db_option_arc.clone();
        handles.push(tokio::spawn(async move {
            gen_meshes_in_db(db_option, &use_cate_refnos)
                .await
                .expect("更新cate模型数据失败");
        }));

        let db_option = db_option_arc.clone();
        handles.push(tokio::spawn(async move {
            gen_meshes_in_db(db_option, &bran_hanger_refnos)
                .await
                .expect("更新bran_hanger模型数据失败");
        }));

        use futures::StreamExt;
        while let Some(result) = handles.next().await {
            if let Err(e) = result {
                eprintln!("Task failed: {:?}", e);
            }
        }
    }
}
