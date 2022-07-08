
use serde::{Serialize, Deserialize};
use derive_more::{Deref, DerefMut};

#[derive(Serialize, Deserialize, Deref, DerefMut, Clone, Default,Eq, Hash, PartialEq)]
pub struct AiosString(pub String);

impl Into<sled::IVec> for AiosString {
    fn into(self) -> sled::IVec {
        bincode::serialize(&self).unwrap().into()
    }
}
impl Into<sled::IVec> for &AiosString {
    fn into(self) -> sled::IVec {
        bincode::serialize(self).unwrap().into()
    }
}

impl From<sled::IVec> for AiosString{
    fn from(d: sled::IVec) -> Self{
        bincode::deserialize(&d).unwrap()
    }
}