//! 数据面的元素记录：`EleData`。
//!
//! 这是 `old-parse-pdms-db::parse::EleData` 的本仓定义（parse_pdms_db 退役清单 §B1，2026-09-04）：
//! 字段、派生与访问器逐字相同，只是所有权从解析器搬到了消费方。它从来就不是「解析器的产物」
//! 而是数据面的 DTO——今天唯一的生产写者是 e3d-io 底座的 [`super::e3d_collector`]
//! （`DbSet@T` 上的 `DbElement` → `direct_attmap::to_named_attmap` → `EleData`），唯一的消费者是
//! [`super::ele_ops`] 的终态投影与 SurrealQL 渲染（`PersistElementState::Add / Modified`）。
//! 六个字段的类型全部来自 `aios_core`（`RefU64` / `WholeAttMap` / `RefU64Vec`），本模块不带任何
//! 字节级解析。
//!
//! `whole_attmap` 三个命名空间的分法（隐式 `attmap` / 显式 `explicit_attmap` / `uda_atts`）是渲染
//! 契约的一部分：`to_modify_surql` 对 `NAME` 只在显式命名空间里认，`ATT_*` 行只带文件里真存了的
//! 属性。收集器按 e3d-io 描述符的状态分拣（见 `e3d_collector::assemble_ele_data`），与 legacy
//! 解析器的 `WholeAttMap` 同一分法。

use aios_core::types::whole_attmap::ExplicitAttr;
use aios_core::{NamedAttrMap, RefU64, RefU64Vec, RefnoEnum, WholeAttMap};

/// 一个元素在某一会话时刻的完整状态。
#[derive(Debug, Clone, Default)]
pub struct EleData {
    pub refno: RefU64,
    pub owner: RefU64,
    /// noun 的 db1 哈希（`TYPE` 字）。
    pub noun: u32,
    pub whole_attmap: WholeAttMap,
    /// 成员表（`owner.children` 包含性的来源）。
    pub children: RefU64Vec,
    pub name: String,
}

impl EleData {
    #[inline]
    pub fn att_map(&self) -> &NamedAttrMap {
        self.whole_attmap.att_map()
    }

    #[inline]
    pub fn explicit_attmap(&self) -> &NamedAttrMap {
        self.whole_attmap.explicit_attmap()
    }

    #[inline]
    pub fn explicit_attmap_mut(&mut self) -> &mut NamedAttrMap {
        self.whole_attmap.explicit_attmap_mut()
    }

    #[inline]
    pub fn uda_atts(&self) -> &Vec<ExplicitAttr> {
        self.whole_attmap.uda_atts()
    }

    #[inline]
    pub fn uda_atts_mut(&mut self) -> &mut Vec<ExplicitAttr> {
        self.whole_attmap.uda_atts_mut()
    }

    #[inline]
    pub fn att_map_mut(&mut self) -> &mut NamedAttrMap {
        self.whole_attmap.att_map_mut()
    }

    /// 会话内 refno（`REFNO` 属性）的枚举形式。
    #[inline]
    pub fn refno_enum(&self) -> RefnoEnum {
        self.att_map().ses_refno().into()
    }

    /// `REFNO` 属性里的最新 refno（与字段 `refno` 同名是 legacy 接口原样）。
    #[inline]
    pub fn refno(&self) -> RefU64 {
        self.att_map().latest_refno()
    }

    #[inline]
    pub fn latest_refno_enum(&self) -> RefnoEnum {
        self.att_map().latest_refno().into()
    }
}
