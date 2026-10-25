//! `pipe_model::render::RenderedBatch` → `EleOperationData` 适配层。
//!
//! 语义与测试在 `pipe-model` crate（docs/specs/pipe-modeling.md）；这里只做
//! 逐字段映射——DTO 本就按 `EleOperationDetail`/`EleData`/`ModifiedElement`
//! 字段级同构设计（NAME 只进显式命名空间、noun 存 db1 哈希、children_changed
//! 带旧新成员表），本文件不含任何编辑语义。
//!
//! 产出行投进既有 `terminal_persist_changes` 收口——终态合并、墓碑 tombstone、
//! refno 复用退役行全部走收集器同一条道，不另立旁路。`sesno` 由调用方填
//! （编辑事务的会话号归属规则与收集器窗口一致）。

use crate::data_interface::ele_data::EleData;
use crate::data_interface::ele_ops::{
    EleOperationData, EleOperationDetail, ModifiedElement,
};
use aios_core::{RefU64Vec, WholeAttMap};
use pipe_model::render::{EditElement, EditModify, EditOp, RenderedBatch};

/// `EditElement` → `EleData`：字段级同构的机械映射（UDA 面恒空——服务不发 UDA 写）。
pub fn edit_element_to_ele_data(el: &EditElement) -> EleData {
    EleData {
        refno: el.refno,
        owner: el.owner,
        noun: el.noun,
        whole_attmap: WholeAttMap {
            attmap: el.attmap.clone(),
            explicit_attmap: el.explicit_attmap.clone(),
            uda_atts: Vec::new(),
        },
        children: RefU64Vec(el.children.clone()),
        name: el.name.clone(),
    }
}

/// `EditModify` → `ModifiedElement`：diff 四面直搬，children delta 装箱；
/// 本服务不发属性删除与 UDA 写，`deleted_*`/`uda_*` 面恒空。
fn edit_modify_to_modified_element(m: &EditModify) -> ModifiedElement {
    ModifiedElement {
        current_data: edit_element_to_ele_data(&m.current),
        added_attrs: m.added_attrs.clone(),
        deleted_attrs: Default::default(),
        modified_attrs: m.modified_attrs.clone(),
        added_explicit_attrs: m.added_explicit_attrs.clone(),
        deleted_explicit_attrs: Default::default(),
        modified_explicit_attrs: m.modified_explicit_attrs.clone(),
        added_uda_attrs: Default::default(),
        deleted_uda_attrs: Default::default(),
        modified_uda_attrs: Default::default(),
        noun: m.current.noun_name.clone(),
        children_changed: m
            .children_changed
            .as_ref()
            .map(|(old, new)| (RefU64Vec(old.clone()), RefU64Vec(new.clone()))),
    }
}

/// 单个 `EditOp` → `EleOperationData`（refno 取操作自身；sesno 由调用方统一赋）。
pub fn edit_op_to_ele_operation(op: &EditOp, sesno: u32) -> EleOperationData {
    match op {
        EditOp::Add(el) => EleOperationData::new(
            el.refno,
            sesno,
            EleOperationDetail::Add(edit_element_to_ele_data(el)),
        ),
        EditOp::Modify(m) => EleOperationData::new(
            m.current.refno,
            sesno,
            EleOperationDetail::Modified(edit_modify_to_modified_element(m)),
        ),
        EditOp::Delete { refno } => {
            EleOperationData::new(*refno, sesno, EleOperationDetail::Deleted)
        }
    }
}

/// 整批渲染结果 → 有序 `EleOperationData` 序列。
///
/// 返回值可直接按 `BTreeMap<sesno, Vec<EleOperationData>>` 进
/// [`crate::data_interface::ele_ops::terminal_persist_changes`]；
/// 确认项与警告不参与行渲染（`requires_confirm` 非空时调用方不应走到这里）。
pub fn rendered_batch_to_ele_operations(
    rendered: &RenderedBatch,
    sesno: u32,
) -> Vec<EleOperationData> {
    rendered
        .ops
        .iter()
        .map(|op| edit_op_to_ele_operation(op, sesno))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_core::{NamedAttrMap, NamedAttrValue, RefU64};
    use pipe_model::batch::{MutationBatch, MutationOp};
    use pipe_model::catalog::PipeCatalog;
    use pipe_model::render::render_batch;
    use std::collections::HashMap;

    /// 最小 PipeCatalog：只为适配测试喂基值（noun/owner/name/members/attrs 全空表）。
    struct FlatCatalog;
    impl PipeCatalog for FlatCatalog {
        fn noun_of(&self, _: RefU64) -> Option<String> {
            None
        }
        fn name_of(&self, _: RefU64) -> Option<String> {
            None
        }
        fn owner_of(&self, _: RefU64) -> Option<RefU64> {
            None
        }
        fn members_of(&self, _: RefU64) -> Vec<RefU64> {
            Vec::new()
        }
        fn attr(&self, _: RefU64, _: &str) -> Option<NamedAttrValue> {
            None
        }
        fn name_exists(&self, _: &str) -> bool {
            false
        }
        fn writable(&self, _: RefU64) -> bool {
            true
        }
        fn alloc_refno(&self, _: RefU64) -> RefU64 {
            RefU64(1)
        }
    }

    #[test]
    fn create_op_maps_to_add_detail() {
        let mut batch = MutationBatch::named("t");
        batch.ops.push(MutationOp::Create {
            refno: RefU64(1001),
            noun: "PIPE".into(),
            under: RefU64(50),
            name: Some("/P-1".into()),
            attrs: NamedAttrMap {
                map: HashMap::from([(
                    "BORE".to_string(),
                    NamedAttrValue::F32Type(100.0),
                )])
                .into_iter()
                .collect(),
            },
        });
        let rendered = render_batch(&batch, &FlatCatalog);
        let ops = rendered_batch_to_ele_operations(&rendered, 42);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].refno, RefU64(1001));
        assert_eq!(ops[0].sesno, 42);
        let EleOperationDetail::Add(data) = &ops[0].detail else {
            panic!("Create 应映到 Add")
        };
        assert_eq!(data.owner, RefU64(50));
        assert_eq!(data.name, "/P-1");
        // NAME 只在显式命名空间；隐式面只有 BORE。
        assert!(data.att_map().map.get("NAME").is_none());
        assert_eq!(
            data.explicit_attmap().map.get("NAME"),
            Some(&NamedAttrValue::StringType("/P-1".into()))
        );
        assert!(data.att_map().map.contains_key("BORE"));
    }

    #[test]
    fn delete_op_maps_to_deleted_detail() {
        let mut batch = MutationBatch::named("t");
        batch.ops.push(MutationOp::Delete {
            element: RefU64(77),
        });
        let rendered = render_batch(&batch, &FlatCatalog);
        let ops = rendered_batch_to_ele_operations(&rendered, 1);
        assert!(matches!(ops[0].detail, EleOperationDetail::Deleted));
    }

    #[test]
    fn modify_carries_noun_name_and_children_delta() {
        let mut batch = MutationBatch::named("t");
        batch.ops.push(MutationOp::Include {
            element: RefU64(9),
            into: RefU64(20),
            order: Some(0),
        });
        let rendered = render_batch(&batch, &FlatCatalog);
        let ops = rendered_batch_to_ele_operations(&rendered, 7);
        let EleOperationDetail::Modified(m) = &ops[0].detail else {
            panic!("Include 应映到 Modified")
        };
        let (old, new) = m.children_changed.as_ref().expect("children delta 应存在");
        assert!(old.is_empty());
        assert_eq!(new.0, vec![RefU64(9)]);
    }
}
