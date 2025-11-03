use aios_core::pdms_types::EleOperation;
use aios_core::types::*;
use serde::{Deserialize, Serialize};

pub mod set_status;
pub mod update_log;
// pub mod query_status;

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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RefnoStatusDifference {
    // 修改时间
    pub mod_time: String,
    // 创建人
    pub ori_user: String,
    // 最后修改人
    pub mod_user: String,
    // 增删改
    pub mod_type: EleOperation,
    pub mod_ele_ref: RefU64,
    pub mod_ele_name: String,
    pub mod_ele_type: String,
    // 旧版本的数据，仅存在差异部分
    pub old_content: AttrMap,
    // 新版本的数据
    pub new_content: AttrMap,
}
