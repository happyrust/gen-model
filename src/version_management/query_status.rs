use aios_core::pdms_types::RefU64;
use crate::version_management::{RefnoStatusDifference, RefnoStatusInfo};

/// 查询该节点所有的数据状态,只返回状态信息，不返回attrmap
///
/// 按照版本赋值顺序返回
pub async fn query_refno_all_status(refno: RefU64) -> Vec<RefnoStatusInfo> {
    todo!()
}

/// 查询某个节点两个版本之间的差异数据
///
/// 如果为新增或者删除，则不进行对比，old_content 和 new_content 返回空即可
pub async fn query_difference_between_two_status(refno: RefU64, old_status: &str, new_status: &str) -> RefnoStatusDifference {
    todo!()
}