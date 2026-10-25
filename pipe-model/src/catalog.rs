//! 读门面（spec 决策：读侧注入，写侧只出 [`MutationBatch`]）。
//!
//! 服务做校验/推导要读数据面——属主 noun、名字查重、成员表、属性现值、写权限、
//! 编辑段 refno 分配——全部经本 trait 由调用方实现（生产侧接 SurrealDB 读面，
//! 测试侧用内存 mock）。本 trait 刻意窄：只含 Phase A 用到的最小读集，后续阶段
//! 按需扩（规格库读面在 [`crate::spec_resolver::SpecCatalog`] 另立）。

use aios_core::{NamedAttrMap, NamedAttrValue, RefU64};

/// 数据面只读访问。
pub trait PipeCatalog {
    /// 元素 noun（`TYPE` 名："PIPE"/"BRAN"/"ZONE"/"EQUI"/"TUBI"…）。
    /// `None` = refno 不在数据面。
    fn noun_of(&self, refno: RefU64) -> Option<String>;

    /// `NAME`（含 `/` 前缀的全名）。
    fn name_of(&self, refno: RefU64) -> Option<String>;

    /// 属主 refno。
    fn owner_of(&self, refno: RefU64) -> Option<RefU64>;

    /// 直属成员表（按成员序）。
    fn members_of(&self, refno: RefU64) -> Vec<RefU64>;

    /// 单属性现值。
    fn attr(&self, refno: RefU64, name: &str) -> Option<NamedAttrValue>;

    /// 全量隐式属性现值（渲染 Modify diff 的基值）。`None` = 元素不在数据面。
    /// 默认实现返回 `None`——生产侧必须覆盖；不含 diff 需求的实现可省略。
    fn attmap_of(&self, _refno: RefU64) -> Option<NamedAttrMap> {
        None
    }

    /// 名字查重（`ID <name>` 探测等价物）。
    fn name_exists(&self, name: &str) -> bool;

    /// 按名反查 refno（导入 EXISTS 行写属性用；`DBREF` 属性值解析同用）。
    /// 默认 `None`——实现方可只覆盖需要的路径。
    fn refno_by_name(&self, _name: &str) -> Option<RefU64> {
        None
    }

    /// 写权限判定（E3D：只读 MDB/WORLD 元素不可写）。
    fn writable(&self, refno: RefU64) -> bool;

    /// 分配编辑段 refno——必须与摄入行号段隔离（spec 决策：source=edit 范围）。
    /// `under` 是新元素的属主：E3D `NEW X` 恒落在**属主所在库**（refno 高部即
    /// 属主 dbnum），实现方按属主解库号并在该库内取水，不许挪到别的库。
    fn alloc_refno(&self, under: RefU64) -> RefU64;
}
