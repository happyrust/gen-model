use aios_core::RefnoEnum;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use std::collections::HashSet;

pub const INCREMENT_DATA: &'static str = "INCREMENT_DATA";

///需要修改的模型的增量参考号数据
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IncrGeoUpdateLog {
    //基本体模型修改了的参考号
    pub prim_refnos: HashSet<RefnoEnum>,
    //拉伸体模型修改了的参考号
    pub loop_owner_refnos: HashSet<RefnoEnum>,
    //元件库模型的属性修改了的参考号
    pub bran_hanger_refnos: HashSet<RefnoEnum>,
    //元件库模型的属性修改了的参考号
    pub basic_cata_refnos: HashSet<RefnoEnum>,
    //删除了的模型
    pub delete_refnos: HashSet<RefnoEnum>,
}

impl IncrGeoUpdateLog {
    #[inline]
    pub fn count(&self) -> usize {
        self.prim_refnos.len()
            + self.loop_owner_refnos.len()
            + self.basic_cata_refnos.len()
            + self.bran_hanger_refnos.len()
    }

    #[inline]
    pub fn get_all_visible_refnos(&self) -> HashSet<RefnoEnum> {
        let mut refnos = HashSet::new();
        refnos.extend(self.prim_refnos.iter());
        refnos.extend(self.loop_owner_refnos.iter());
        refnos.extend(self.basic_cata_refnos.iter());
        refnos.extend(self.bran_hanger_refnos.iter());
        refnos
    }

    #[inline]
    pub async fn get_all_geom_refnos_deep(&self) -> HashSet<RefnoEnum> {
        let mut refnos = HashSet::new();
        refnos.extend(self.prim_refnos.iter());
        refnos.extend(self.loop_owner_refnos.iter());
        refnos.extend(self.basic_cata_refnos.iter());
        let children = aios_core::get_all_children_refnos(self.bran_hanger_refnos.iter())
            .await
            .unwrap_or_default();
        refnos.extend(children);
        refnos
    }
}
