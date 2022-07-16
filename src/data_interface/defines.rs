use aios_core::pdms_types::{NounHash, RefU64};
use aios_core::tool::db_tool::{db1_dehash, db1_hash};
use serde::{Serialize, Deserialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CachedRefBasic {
    pub(crate) owner: RefU64,
    pub(crate) table: String, //提前处理好成了table name，有关键字冲突的地方，删除最后的
}

impl Into<sled::IVec> for CachedRefBasic {
    fn into(self) -> sled::IVec {
        bincode::serialize(&self).unwrap().into()
    }
}
impl Into<sled::IVec> for &CachedRefBasic {
    fn into(self) -> sled::IVec {
        bincode::serialize(self).unwrap().into()
    }
}

impl From<sled::IVec> for CachedRefBasic{
    fn from(d: sled::IVec) -> Self{
        bincode::deserialize(&d).unwrap()
    }
}

impl CachedRefBasic{

    #[inline]
    pub fn get_type(&self) -> &str{
        if self.table.ends_with("_") {
            &self.table[..self.table.len()-1]
        }else{
            self.table.as_str()
        }
    }

    #[inline]
    pub fn get_table_name(&self) -> &str{
        self.table.as_str()
    }

    #[inline]
    pub fn get_noun_hash(&self) -> NounHash{
        db1_hash(self.get_type()).into()
    }

    #[inline]
    pub fn get_owner(&self) -> RefU64{
        self.owner
    }

}

