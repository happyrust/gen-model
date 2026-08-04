use aios_core::types::*;
use serde::{Deserialize, Serialize};

pub mod set_status;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SetStatusData {
    pub refno: RefU64,
    pub status: String,
    pub user: String,
    // 设置状态的时间
    pub time: String,
    // 备注,在平台设置数据状态可以写备注
    pub node: String,
    pub attr_map: AttrMap,
}
