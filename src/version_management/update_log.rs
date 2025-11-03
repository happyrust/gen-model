use aios_core::Datetime as SurrealDatetime;
use aios_core::RefU64;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

///属性 update flag的数据结构，包含信息：否几何体发生修改，
/// 如果检测到有相应的属性发生修改，就会将更新模型的生成
#[serde_as]
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct AttsUpdatedRecord {
    ///使用e3d的版本号作为提交id
    pub version_id: i32,
    ///记录发生修改的属性的refnos
    pub refnos: Vec<RefU64>,
    pub timestamp: SurrealDatetime, //记录发生修改的时间戳
}

//是不是要放在方法里去判断这些发生修改的，而不是在这里提前分好类
impl AttsUpdatedRecord {
    pub fn new() -> Self {
        Self {
            version_id: 0,
            refnos: vec![],
            timestamp: SurrealDatetime::default(),
        }
    }

    pub fn primitive_changed(&self) {}

    pub fn loop_changed(&self) {}

    pub fn cata_changed(&self) {}
}

///mesh update flag的数据结构，包含信息：记录发生修改的时间戳
pub struct MeshUpdatedRecord {
    //记录发生修改的时间戳, 用于对比是否完成更新，和 属性的时间戳对比
    pub timestamp: SurrealDatetime,
}
