use crate::version_management::SetStatusData;
use aios_core::types::*;

/// 在增量更新中对比最新的属性，当 :CNPEversion 发生变化时,则需要存储该部分数据
pub async fn check_need_update_version(attr_map: &AttrMap) -> bool {
    todo!()
}

/// 当 check_need_update_version 为 true 或者 平台设置数据状态时，将对应的数据存到版本管理中
pub async fn set_version_from_uda(save_data: SetStatusData) -> anyhow::Result<()> {
    todo!()
}
