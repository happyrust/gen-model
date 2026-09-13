//! 增量窗口的数据操作、终态投影与 SurrealQL 渲染。
//!
//! 生产持久化消费 [`PersistElementChange`]：Add/Modified 都携带 e3d-io 目标会话解析出的
//! 完整元素状态。差分操作仍供模型影响、反向索引和诊断使用。

#![allow(clippy::all)]

use aios_core::helper::normalize_sql_string;
use aios_core::pdms_types::*;
use aios_core::{NamedAttrMap, NamedAttrValue, RefU64Vec};
use std::collections::{HashMap, HashSet};

pub use crate::data_interface::ele_data::EleData;
pub use crate::data_interface::uda_keys::UdaKeyMap;

#[derive(Clone)]
pub enum PersistElementState {
    Add(EleData),
    Modified(EleData),
    /// 元素在窗口终态被删。`base` 是它在窗口基侧（`DbSet@S`）的完整状态——墓碑 `pe` 行
    /// （noun / owner / name 保留、`deleted = true`）从它渲染，旧 noun 行按它的 noun 删；
    /// 基侧读不到时为 `None`，墓碑退化成只带 id / dbnum / sesno / deleted 的最小行并出声
    /// （`element_batch::RenderedElementBatch::tombstones_without_base`）。
    Deleted {
        base: Option<EleData>,
    },
}

#[derive(Clone)]
pub struct PersistElementChange {
    pub refno: RefU64,
    pub sesno: u32,
    pub state: PersistElementState,
}

impl std::fmt::Debug for PersistElementState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Add(_) => "Add(..)",
            Self::Modified(_) => "Modified(..)",
            Self::Deleted { base: Some(_) } => "Deleted(base)",
            Self::Deleted { base: None } => "Deleted",
        })
    }
}

impl std::fmt::Debug for PersistElementChange {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PersistElementChange")
            .field("refno", &self.refno)
            .field("sesno", &self.sesno)
            .field("state", &self.state)
            .finish()
    }
}

/// 窗口 → 每 refno 一条终态。`tombstones` 是收集器读回的被删元素基侧状态（`DbSet@S`），
/// 按 refno 取；没有就是 `Deleted { base: None }`（墓碑退化，见 [`PersistElementState::Deleted`]）。
pub fn terminal_persist_changes(
    window: &std::collections::BTreeMap<u32, Vec<EleOperationData>>,
    tombstones: &HashMap<RefU64, EleData>,
) -> Vec<PersistElementChange> {
    let mut latest = std::collections::BTreeMap::<RefU64, PersistElementChange>::new();
    for (&sesno, operations) in window {
        for operation in operations {
            let state = match &operation.detail {
                EleOperationDetail::Add(data) => PersistElementState::Add(data.clone()),
                EleOperationDetail::Modified(data) => {
                    PersistElementState::Modified(data.current_data.clone())
                }
                EleOperationDetail::Deleted => PersistElementState::Deleted {
                    base: tombstones.get(&operation.refno).cloned(),
                },
                EleOperationDetail::None => continue,
            };
            latest.insert(
                operation.refno,
                PersistElementChange {
                    refno: operation.refno,
                    sesno,
                    state,
                },
            );
        }
    }
    latest.into_values().collect()
}

impl PersistElementChange {
    /// 终态落库语句由 `data_interface::element_batch` 按**批**渲染（批量先删后插，plan 2026-09-06
    /// §3 D1/D2）；这里不再有逐元素的 `to_surql`——那正是被退出的写法。

    pub fn is_add(&self) -> bool {
        matches!(self.state, PersistElementState::Add(_))
    }

    pub fn is_deleted(&self) -> bool {
        matches!(self.state, PersistElementState::Deleted { .. })
    }

    /// 这一条终态在库里对应的 `pe` id（`ref0_ref1`）。
    pub fn id(&self) -> String {
        self.refno.to_string()
    }
}

#[cfg(test)]
mod persist_projection_tests {
    use super::{
        EleOperationData, EleOperationDetail, PersistElementState, diff_ele_data,
        terminal_persist_changes,
    };
    use crate::data_interface::ele_data::EleData;
    use aios_core::pdms_types::{RefU64, RefU64Vec};
    use std::collections::BTreeMap;

    #[test]
    fn projection_keeps_only_latest_terminal_state() {
        let refno = RefU64((8000_u64 << 32) | 7);
        let window = BTreeMap::from([
            (
                1,
                vec![EleOperationData::new(refno, 1, EleOperationDetail::Deleted)],
            ),
            (
                2,
                vec![EleOperationData::new(refno, 2, EleOperationDetail::None)],
            ),
        ]);
        let changes = terminal_persist_changes(&window, &Default::default());
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].refno, refno);
        assert_eq!(changes[0].sesno, 1);
        assert!(matches!(
            changes[0].state,
            PersistElementState::Deleted { base: None }
        ));

        // 收集器给了基侧状态就带上：墓碑行从它渲染。
        let mut base = EleData::default();
        base.refno = refno;
        let changes = terminal_persist_changes(&window, &[(refno, base)].into_iter().collect());
        assert!(matches!(
            &changes[0].state,
            PersistElementState::Deleted { base: Some(data) } if data.refno == refno
        ));
    }

    #[test]
    fn modified_projection_carries_the_complete_target_state() {
        let refno = RefU64((8000_u64 << 32) | 8);
        let base = EleData::default();
        let mut target = EleData::default();
        target.children = RefU64Vec(vec![RefU64((8000_u64 << 32) | 9)]);
        let modified = diff_ele_data(&base, &target).expect("child change");
        let window = BTreeMap::from([(
            3,
            vec![EleOperationData::new(
                refno,
                3,
                EleOperationDetail::Modified(modified),
            )],
        )]);

        let changes = terminal_persist_changes(&window, &Default::default());
        assert!(matches!(
            &changes[0].state,
            PersistElementState::Modified(data)
                if data.children.iter().eq(target.children.iter())
        ));
    }
}

/// 已修改元素的详细信息
#[derive(Clone)]
pub struct ModifiedElement {
    /// 修改后的完整元素数据
    pub current_data: EleData,
    /// 新增的属性 {属性名 => 属性值}
    pub added_attrs: HashMap<String, NamedAttrValue>,
    /// 删除的属性 {属性名 => 旧属性值}
    pub deleted_attrs: HashMap<String, NamedAttrValue>,
    /// 修改的属性 {属性名 => (旧值, 新值)}
    pub modified_attrs: HashMap<String, (NamedAttrValue, NamedAttrValue)>,
    /// 新增的显式属性
    pub added_explicit_attrs: HashMap<String, NamedAttrValue>,
    /// 删除的显式属性
    pub deleted_explicit_attrs: HashMap<String, NamedAttrValue>,
    /// 修改的显式属性
    pub modified_explicit_attrs: HashMap<String, (NamedAttrValue, NamedAttrValue)>,
    /// 新增的UDA属性
    pub added_uda_attrs: HashMap<i32, NamedAttrValue>,
    /// 删除的UDA属性
    pub deleted_uda_attrs: HashMap<i32, NamedAttrValue>,
    /// 修改的UDA属性
    pub modified_uda_attrs: HashMap<i32, (NamedAttrValue, NamedAttrValue)>,
    /// 元素类型
    pub noun: String,
    /// 子元素变更 (old_children, new_children)
    pub children_changed: Option<(RefU64Vec, RefU64Vec)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildrenDelta {
    None,
    Reordered,
    MemberChanged,
}

/// Classify an ordered child-list change. Comparing only sets loses pure
/// reorder events; comparing only vectors cannot distinguish reorder from
/// membership changes.
pub fn classify_children_delta(old: &RefU64Vec, new: &RefU64Vec) -> ChildrenDelta {
    if old.iter().eq(new.iter()) {
        return ChildrenDelta::None;
    }
    let old_set: HashSet<_> = old.iter().copied().collect();
    let new_set: HashSet<_> = new.iter().copied().collect();
    if old.len() == new.len() && old_set == new_set {
        ChildrenDelta::Reordered
    } else {
        ChildrenDelta::MemberChanged
    }
}

/// Compare two parsed versions of one element and return the exact attribute /
/// explicit-attribute / UDA / ordered-child delta consumed by incremental writes.
///
/// This is the single implementation shared by session replay and the
/// core.dll-style two-root net-window collector. It is intentionally pure:
/// callers own record lookup and deletion classification; this function only
/// compares the two resolved element versions.
pub fn diff_ele_data(prev: &EleData, latest: &EleData) -> Option<ModifiedElement> {
    let children_changed = (!matches!(
        classify_children_delta(&prev.children, &latest.children),
        ChildrenDelta::None
    ))
    .then(|| (prev.children.clone(), latest.children.clone()));

    let mut added_attrs = HashMap::new();
    let mut deleted_attrs = HashMap::new();
    let mut modified_attrs = HashMap::new();
    for (name, value) in &latest.att_map().map {
        match prev.att_map().map.get(name) {
            Some(prev_value) if prev_value != value => {
                modified_attrs.insert(name.clone(), (prev_value.clone(), value.clone()));
            }
            Some(_) => {}
            None => {
                added_attrs.insert(name.clone(), value.clone());
            }
        }
    }
    for (name, value) in &prev.att_map().map {
        if !latest.att_map().map.contains_key(name) {
            deleted_attrs.insert(name.clone(), value.clone());
        }
    }

    let mut added_explicit_attrs = HashMap::new();
    let mut deleted_explicit_attrs = HashMap::new();
    let mut modified_explicit_attrs = HashMap::new();
    for (name, value) in &latest.explicit_attmap().map {
        match prev.explicit_attmap().map.get(name) {
            Some(prev_value) if prev_value != value => {
                modified_explicit_attrs.insert(name.clone(), (prev_value.clone(), value.clone()));
            }
            Some(_) => {}
            None => {
                added_explicit_attrs.insert(name.clone(), value.clone());
            }
        }
    }
    for (name, value) in &prev.explicit_attmap().map {
        if !latest.explicit_attmap().map.contains_key(name) {
            deleted_explicit_attrs.insert(name.clone(), value.clone());
        }
    }

    let mut added_uda_attrs = HashMap::new();
    let mut deleted_uda_attrs = HashMap::new();
    let mut modified_uda_attrs = HashMap::new();
    for uda in latest.uda_atts() {
        match prev
            .uda_atts()
            .iter()
            .find(|prev_uda| prev_uda.hash_val == uda.hash_val)
        {
            Some(prev_uda) if prev_uda.value != uda.value => {
                modified_uda_attrs
                    .insert(uda.hash_val, (prev_uda.value.clone(), uda.value.clone()));
            }
            Some(_) => {}
            None => {
                added_uda_attrs.insert(uda.hash_val, uda.value.clone());
            }
        }
    }
    for prev_uda in prev.uda_atts() {
        if !latest
            .uda_atts()
            .iter()
            .any(|uda| uda.hash_val == prev_uda.hash_val)
        {
            deleted_uda_attrs.insert(prev_uda.hash_val, prev_uda.value.clone());
        }
    }

    let changed = children_changed.is_some()
        || !added_attrs.is_empty()
        || !deleted_attrs.is_empty()
        || !modified_attrs.is_empty()
        || !added_explicit_attrs.is_empty()
        || !deleted_explicit_attrs.is_empty()
        || !modified_explicit_attrs.is_empty()
        || !added_uda_attrs.is_empty()
        || !deleted_uda_attrs.is_empty()
        || !modified_uda_attrs.is_empty();

    changed.then(|| ModifiedElement {
        noun: latest.att_map().get_type(),
        current_data: latest.clone(),
        added_attrs,
        deleted_attrs,
        modified_attrs,
        added_explicit_attrs,
        deleted_explicit_attrs,
        modified_explicit_attrs,
        added_uda_attrs,
        deleted_uda_attrs,
        modified_uda_attrs,
        children_changed,
    })
}

fn changed_indices<T: PartialEq>(old: &[T], new: &[T]) -> Vec<usize> {
    (0..old.len().max(new.len()))
        .filter(|&index| old.get(index) != new.get(index))
        .map(|index| index + 1)
        .collect()
}

/// Changed one-based E3D qualifiers for an array attribute. Scalar values
/// return an empty list and retain their ordinary attribute-level change.
pub fn changed_qualifiers(old: &NamedAttrValue, new: &NamedAttrValue) -> Vec<usize> {
    match (old, new) {
        (NamedAttrValue::F32VecType(old), NamedAttrValue::F32VecType(new)) => {
            changed_indices(old, new)
        }
        (NamedAttrValue::StringArrayType(old), NamedAttrValue::StringArrayType(new)) => {
            changed_indices(old, new)
        }
        (NamedAttrValue::BoolArrayType(old), NamedAttrValue::BoolArrayType(new)) => {
            changed_indices(old, new)
        }
        (NamedAttrValue::IntArrayType(old), NamedAttrValue::IntArrayType(new)) => {
            changed_indices(old, new)
        }
        (NamedAttrValue::RefU64Array(old), NamedAttrValue::RefU64Array(new)) => {
            changed_indices(old, new)
        }
        _ => Vec::new(),
    }
}

/// Quote/escape an attribute name for use as an object-literal key in a
/// SurrealQL `MERGE` object (e.g. `OWNER` → `"OWNER"`).
fn quote_ident(key: &str) -> String {
    serde_json::to_string(key).unwrap_or_else(|_| format!("{key:?}"))
}

/// Merge scalar/JSON fields (`main_fields`) with record-link entries
/// (`record_entries`, each already formatted as `"KEY": pe:refno`) into one
/// SurrealQL object literal `{ ... }` for `UPSERT ... MERGE`. Returns `None`
/// when there is nothing to merge.
///
/// Single source of truth that fixes three historical bugs where scalar and
/// reference fields were assembled separately:
///   1. a modify that changed ONLY reference attributes emitted no statement at
///      all (the change was silently dropped);
///   2. a modify that changed both a reference and a scalar attribute nested the
///      whole JSON object as a keyless member (`{ K: pe:1, {"A":1} }`) — invalid
///      SurrealQL that failed the whole batch and stalled the dbnum watermark;
///   3. multiple reference attributes were concatenated without a comma.
fn build_merge_fields(
    main_fields: &serde_json::Map<String, serde_json::Value>,
    record_entries: &[String],
) -> Option<String> {
    let mut entries: Vec<String> = Vec::with_capacity(main_fields.len() + record_entries.len());
    for (key, value) in main_fields {
        entries.push(format!("{}: {}", quote_ident(key), value));
    }
    entries.extend(record_entries.iter().cloned());
    (!entries.is_empty()).then(|| format!("{{ {} }}", entries.join(", ")))
}

/// Extract the NEW `OWNER` reference from a modify's attribute-change maps when
/// the element was relocated. Used to keep the denormalized `pe.owner` scalar in
/// sync on modify: the noun record's `OWNER` field and the `pe_owner` graph (via
/// the parent's `children_changed`) already move to the new owner, but `pe.owner`
/// previously stayed pointing at the OLD owner. Returns `None` when OWNER did not
/// change to a record reference.
fn new_owner_from_changes(
    added_attrs: &HashMap<String, NamedAttrValue>,
    modified_attrs: &HashMap<String, (NamedAttrValue, NamedAttrValue)>,
    added_explicit_attrs: &HashMap<String, NamedAttrValue>,
    modified_explicit_attrs: &HashMap<String, (NamedAttrValue, NamedAttrValue)>,
) -> Option<RefU64> {
    fn owner_refno(v: &NamedAttrValue) -> Option<RefU64> {
        match v {
            NamedAttrValue::RefU64Type(refno) => Some(*refno),
            _ => None,
        }
    }
    modified_attrs
        .get("OWNER")
        .and_then(|(_, new)| owner_refno(new))
        .or_else(|| added_attrs.get("OWNER").and_then(owner_refno))
        .or_else(|| {
            modified_explicit_attrs
                .get("OWNER")
                .and_then(|(_, new)| owner_refno(new))
        })
        .or_else(|| added_explicit_attrs.get("OWNER").and_then(owner_refno))
}

#[cfg(test)]
mod merge_fields_tests {
    use super::{build_merge_fields, quote_ident};
    use serde_json::{Map, Value};

    fn map(pairs: &[(&str, Value)]) -> Map<String, Value> {
        let mut m = Map::new();
        for (k, v) in pairs {
            m.insert((*k).to_string(), v.clone());
        }
        m
    }

    fn rec(key: &str, refno: u64) -> String {
        format!("{}: pe:{}", quote_ident(key), refno)
    }

    #[test]
    fn reference_only_modify_is_not_dropped() {
        // 只改引用属性（如 OWNER）时，历史实现会整条丢弃；现在必须仍生成 MERGE 对象。
        let body = build_merge_fields(&Map::new(), &[rec("OWNER", 123)]);
        assert_eq!(body.as_deref(), Some("{ \"OWNER\": pe:123 }"));
    }

    #[test]
    fn reference_plus_scalar_is_one_valid_object() {
        // 引用+标量并存：不能把 JSON 对象嵌套成无键成员（旧 bug 会生成非法 SurrealQL）。
        let body = build_merge_fields(&map(&[("PDIA", Value::from(100))]), &[rec("OWNER", 123)])
            .expect("should emit merge body");
        assert_eq!(body, "{ \"PDIA\": 100, \"OWNER\": pe:123 }");
        assert!(!body.contains(", {"), "must not nest a JSON object: {body}");
        assert!(
            !body.contains("}}"),
            "must not contain nested closing braces: {body}"
        );
    }

    #[test]
    fn multiple_reference_attrs_are_comma_separated() {
        // 多个引用属性历史实现无逗号拼接（`pe:1"TREF"`）；现在必须逗号分隔。
        let body = build_merge_fields(&Map::new(), &[rec("OWNER", 1), rec("TREF", 2)])
            .expect("should emit merge body");
        assert_eq!(body, "{ \"OWNER\": pe:1, \"TREF\": pe:2 }");
    }

    #[test]
    fn scalar_only_still_emits_object() {
        let body = build_merge_fields(&map(&[("PDIA", Value::from(100))]), &[]);
        assert_eq!(body.as_deref(), Some("{ \"PDIA\": 100 }"));
    }

    #[test]
    fn nothing_to_merge_returns_none() {
        assert_eq!(build_merge_fields(&Map::new(), &[]), None);
    }

    #[test]
    fn keys_are_quoted_and_escaped() {
        assert_eq!(quote_ident("OWNER"), "\"OWNER\"");
        assert_eq!(quote_ident("att.NAME"), "\"att.NAME\"");
    }
}

#[cfg(test)]
mod owner_sync_tests {
    use super::new_owner_from_changes;
    use aios_core::NamedAttrValue;
    use aios_core::pdms_types::RefU64;
    use std::collections::HashMap;

    #[test]
    fn owner_change_extracts_new_owner() {
        // OWNER 从 10 改到 20 → 应提取新 owner 20，供同步 pe.owner。
        let mut modified = HashMap::new();
        modified.insert(
            "OWNER".to_string(),
            (
                NamedAttrValue::RefU64Type(RefU64(10)),
                NamedAttrValue::RefU64Type(RefU64(20)),
            ),
        );
        let got =
            new_owner_from_changes(&HashMap::new(), &modified, &HashMap::new(), &HashMap::new());
        assert_eq!(got, Some(RefU64(20)));
    }

    #[test]
    fn non_owner_change_yields_none() {
        // 只改了 NAME → 不触发 pe.owner 同步。
        let mut modified = HashMap::new();
        modified.insert(
            "NAME".to_string(),
            (
                NamedAttrValue::StringType("a".to_string()),
                NamedAttrValue::StringType("b".to_string()),
            ),
        );
        let got =
            new_owner_from_changes(&HashMap::new(), &modified, &HashMap::new(), &HashMap::new());
        assert_eq!(got, None);
    }
}

#[cfg(test)]
mod children_delta_tests {
    use super::{ChildrenDelta, ModifiedElement, changed_qualifiers, classify_children_delta};
    use aios_core::NamedAttrValue;
    use aios_core::pdms_types::{RefU64, RefU64Vec};

    fn children(ids: &[u64]) -> RefU64Vec {
        RefU64Vec(ids.iter().copied().map(RefU64).collect())
    }

    #[test]
    fn distinguishes_reorder_membership_and_no_change() {
        assert_eq!(
            classify_children_delta(&children(&[1, 2]), &children(&[1, 2])),
            ChildrenDelta::None
        );
        assert_eq!(
            classify_children_delta(&children(&[1, 2]), &children(&[2, 1])),
            ChildrenDelta::Reordered
        );
        assert_eq!(
            classify_children_delta(&children(&[1, 2]), &children(&[1, 3])),
            ChildrenDelta::MemberChanged
        );
    }

    #[test]
    fn array_changes_retain_one_based_qualifiers() {
        assert_eq!(
            changed_qualifiers(
                &NamedAttrValue::F32VecType(vec![1.0, 2.0, 3.0]),
                &NamedAttrValue::F32VecType(vec![1.0, 9.0, 3.0, 4.0]),
            ),
            vec![2, 4]
        );

        let mut modified = ModifiedElement {
            current_data: Default::default(),
            added_attrs: Default::default(),
            deleted_attrs: Default::default(),
            modified_attrs: Default::default(),
            added_explicit_attrs: Default::default(),
            deleted_explicit_attrs: Default::default(),
            modified_explicit_attrs: Default::default(),
            added_uda_attrs: Default::default(),
            deleted_uda_attrs: Default::default(),
            modified_uda_attrs: Default::default(),
            noun: "TEST".into(),
            children_changed: None,
        };
        modified.modified_attrs.insert(
            "PARA".into(),
            (
                NamedAttrValue::F32VecType(vec![1.0, 2.0]),
                NamedAttrValue::F32VecType(vec![1.0, 3.0]),
            ),
        );
        assert_eq!(
            modified.qualified_attribute_changes(),
            vec![("PARA".into(), 2)]
        );
    }
}

#[cfg(test)]
mod element_diff_tests {
    use super::{EleData, diff_ele_data};
    use aios_core::NamedAttrValue;
    use aios_core::pdms_types::{RefU64, RefU64Vec};
    use aios_core::types::whole_attmap::ExplicitAttr;

    fn string(value: &str) -> NamedAttrValue {
        NamedAttrValue::StringType(value.to_owned())
    }

    fn element(children: &[u64]) -> EleData {
        let mut data = EleData::default();
        data.children = RefU64Vec(children.iter().copied().map(RefU64).collect());
        data
    }

    fn uda(hash_val: i32, value: &str) -> ExplicitAttr {
        ExplicitAttr {
            name: format!(":UDA{hash_val}"),
            value: string(value),
            is_uda: true,
            hash_val,
        }
    }

    #[test]
    fn identical_versions_have_no_delta() {
        let mut prev = element(&[1, 2]);
        prev.att_map_mut().map.insert("TYPE".into(), string("BOX"));
        let latest = prev.clone();
        assert!(diff_ele_data(&prev, &latest).is_none());
    }

    #[test]
    fn all_namespaces_and_ordered_children_share_one_diff_contract() {
        let mut prev = element(&[1, 2]);
        prev.att_map_mut().map.extend([
            ("TYPE".into(), string("BOX")),
            ("XLEN".into(), string("100")),
            ("OLD".into(), string("gone")),
        ]);
        prev.explicit_attmap_mut().map.extend([
            ("EXPLICIT".into(), string("before")),
            ("EXPLICIT_OLD".into(), string("gone")),
        ]);
        prev.uda_atts_mut()
            .extend([uda(10, "before"), uda(20, "gone")]);

        let mut latest = element(&[2, 1]);
        latest.att_map_mut().map.extend([
            ("TYPE".into(), string("BOX")),
            ("XLEN".into(), string("200")),
            ("NEW".into(), string("added")),
        ]);
        latest.explicit_attmap_mut().map.extend([
            ("EXPLICIT".into(), string("after")),
            ("EXPLICIT_NEW".into(), string("added")),
        ]);
        latest
            .uda_atts_mut()
            .extend([uda(10, "after"), uda(30, "added")]);

        let delta = diff_ele_data(&prev, &latest).expect("versions differ");
        assert_eq!(delta.noun, "BOX");
        assert!(
            delta
                .current_data
                .children
                .iter()
                .eq(latest.children.iter())
        );
        assert_eq!(delta.added_attrs.get("NEW"), Some(&string("added")));
        assert_eq!(delta.deleted_attrs.get("OLD"), Some(&string("gone")));
        assert_eq!(
            delta.modified_attrs.get("XLEN"),
            Some(&(string("100"), string("200")))
        );
        assert_eq!(
            delta.added_explicit_attrs.get("EXPLICIT_NEW"),
            Some(&string("added"))
        );
        assert_eq!(
            delta.deleted_explicit_attrs.get("EXPLICIT_OLD"),
            Some(&string("gone"))
        );
        assert_eq!(
            delta.modified_explicit_attrs.get("EXPLICIT"),
            Some(&(string("before"), string("after")))
        );
        assert_eq!(delta.added_uda_attrs.get(&30), Some(&string("added")));
        assert_eq!(delta.deleted_uda_attrs.get(&20), Some(&string("gone")));
        assert_eq!(
            delta.modified_uda_attrs.get(&10),
            Some(&(string("before"), string("after")))
        );
        let (old_children, new_children) = delta.children_changed.expect("ordered child delta");
        assert!(old_children.iter().eq(prev.children.iter()));
        assert!(new_children.iter().eq(latest.children.iter()));
    }

    #[test]
    fn raw_uda_hash_value_path_preserves_add_modify_delete() {
        let mut previous = element(&[]);
        previous
            .uda_atts_mut()
            .extend([uda(101, "before"), uda(202, "deleted")]);
        let mut latest = element(&[]);
        latest
            .uda_atts_mut()
            .extend([uda(101, "after"), uda(303, "added")]);

        let delta = diff_ele_data(&previous, &latest).expect("UDA delta");
        assert_eq!(
            delta.modified_uda_attrs.get(&101),
            Some(&(string("before"), string("after")))
        );
        assert_eq!(delta.deleted_uda_attrs.get(&202), Some(&string("deleted")));
        assert_eq!(delta.added_uda_attrs.get(&303), Some(&string("added")));
    }
}

#[cfg(test)]
mod escape_surql_tests {
    use super::ModifiedElement;

    #[test]
    fn plain_string_unchanged() {
        assert_eq!(ModifiedElement::escape_surql_str("PUMP-01"), "PUMP-01");
    }

    #[test]
    fn single_quote_is_escaped() {
        // F5：名字含单引号不得破坏单引号 SurrealQL 字面量。
        assert_eq!(ModifiedElement::escape_surql_str("O'Brien"), "O\\'Brien");
    }

    #[test]
    fn backslash_is_escaped() {
        // Windows 路径 / 特殊字符里的反斜杠需转义。
        assert_eq!(ModifiedElement::escape_surql_str("C:\\pipe"), "C:\\\\pipe");
    }

    #[test]
    fn backslash_and_quote_combined() {
        // 先转义反斜杠、再转义单引号，二者共存结果正确。
        assert_eq!(ModifiedElement::escape_surql_str("a'\\b"), "a\\'\\\\b");
    }
}

/// F4 / T402：`Add` 的 `pe_owner` 关系写必须幂等。
///
/// 落库按 `TX_CHUNK` 分块提交，整窗口不是单事务。早块已提交、后块失败时按 ADR-001
/// 会用同一 sesno 窗口重放；若 `Add` 仍是裸 `INSERT RELATION`，重放会撞上已存在的
/// 复合 id `[pe:{id}, i]` 反复报错，把该 dbnum 的水位永久卡死。语句里先 `DELETE` 本
/// 元素的入向 `pe_owner` 边，重放才收敛。
#[cfg(test)]
mod add_relate_idempotency_tests {
    use super::{EleData, EleOperationDetail};
    use aios_core::NamedAttrValue;
    use aios_core::pdms_types::{RefU64, RefU64Vec};

    /// 最小可渲染的 `Add` 载荷：`gen_sur_json_exclude` 会 `expect` 解析阶段注入的
    /// DBNUM，`get_type` 没有 TYPE 时退化成 `unset`，两者都补上才能渲染出真语句。
    fn add_with_children(children: &[u64]) -> EleOperationDetail {
        let mut ele = EleData::default();
        ele.whole_attmap.attmap.map.insert(
            "TYPE".to_string(),
            NamedAttrValue::StringType("BOX".to_string()),
        );
        ele.whole_attmap
            .attmap
            .map
            .insert("DBNUM".to_string(), NamedAttrValue::IntegerType(7997));
        ele.children = RefU64Vec(children.iter().copied().map(RefU64).collect());
        EleOperationDetail::Add(ele)
    }

    #[test]
    fn add_with_children_deletes_before_inserting_relations() {
        let sql = add_with_children(&[2, 3]).to_surql("1_100", 7997, 42);

        let delete_at = sql
            .find("DELETE pe:1_100<-pe_owner")
            .expect("Add 必须先删除本元素的入向 pe_owner 边");
        let insert_at = sql
            .find("INSERT RELATION INTO pe_owner")
            .expect("Add 仍要写入 children 关系");
        assert!(
            delete_at < insert_at,
            "DELETE 必须在 INSERT RELATION 之前，否则重放会撞已存在关系:\n{sql}"
        );
    }

    #[test]
    fn rendering_twice_is_byte_identical() {
        // 语句本身不含时间戳/随机量，重放同一窗口渲染出的 SQL 必须完全一致，
        // 幂等性才只取决于「先删后插」这一条性质。
        let first = add_with_children(&[2, 3]).to_surql("1_100", 7997, 42);
        let second = add_with_children(&[2, 3]).to_surql("1_100", 7997, 42);
        assert_eq!(first, second);
    }

    #[test]
    fn add_without_children_emits_no_relation_statement() {
        // 无 children 时不该发多余的 DELETE/INSERT，避免误删他处建立的关系。
        let sql = add_with_children(&[]).to_surql("1_100", 7997, 42);
        assert!(
            !sql.contains("pe_owner"),
            "无 children 的 Add 不应触碰 pe_owner:\n{sql}"
        );
    }
}

/// B8.1（2026-09-04）：生产终态渲染器的两处护栏——`ATT_UDA` 行与 `pe_owner` 边方向。
///
/// 这两条都是增量路**既有**缺陷，与基线换底座无关：渲染器此前不写 `ATT_UDA`
/// （收集器装进 `uda_atts` 的 UDA 值到库里就没了），`pe_owner` 边写成 `in = owner`
/// （读侧 `{owner}<-pe_owner` 拿不到 children）。
#[cfg(test)]
mod terminal_render_guard_tests {
    use super::{EleData, PersistElementChange, PersistElementState, UdaKeyMap};
    use crate::data_interface::uda_keys::UdaDefinitionRow;
    use aios_core::NamedAttrValue;
    use aios_core::pdms_types::{RefU64, RefU64Vec};
    use aios_core::types::whole_attmap::ExplicitAttr;
    use aios_core::{NamedAttrMap, helper::normalize_sql_string};

    const DBNUM: i32 = 7997;

    fn refu(n: u64) -> RefU64 {
        RefU64((DBNUM as u64) << 32 | n)
    }

    /// 真实形状的 UDA 键：高于 `0x171FAD39` 才被 `is_uda` 认。
    fn uda_hash(offset: i32) -> i32 {
        0x171F_AD39 + offset
    }

    fn element(refno: RefU64, children: &[RefU64], udas: &[(i32, NamedAttrValue)]) -> EleData {
        let mut ele = EleData::default();
        ele.refno = refno;
        let map = &mut ele.whole_attmap.attmap.map;
        map.insert("REFNO".into(), NamedAttrValue::RefU64Type(refno));
        map.insert("TYPE".into(), NamedAttrValue::StringType("EQUI".into()));
        map.insert("DBNUM".into(), NamedAttrValue::IntegerType(DBNUM));
        ele.children = RefU64Vec(children.to_vec());
        for (hash, value) in udas {
            ele.whole_attmap.uda_atts.push(ExplicitAttr {
                name: ":UDA".into(),
                value: value.clone(),
                is_uda: true,
                hash_val: *hash,
            });
        }
        ele
    }

    fn keys(rows: &[(i32, u64)]) -> UdaKeyMap {
        UdaKeyMap::from_rows(rows.iter().map(|(hash, id)| UdaDefinitionRow {
            id: RefU64((24381u64 << 32) | id),
            ukey: Some(*hash as i64),
            udna: Some(":X".into()),
        }))
    }

    fn change(state: PersistElementState, refno: RefU64) -> PersistElementChange {
        PersistElementChange {
            refno,
            sesno: 42,
            state,
        }
    }

    /// 生产写法（`element_batch`，Replace）渲出的一块。
    fn batch_sql(changes: &[PersistElementChange], keys: &UdaKeyMap) -> String {
        use crate::data_interface::element_batch::{WriteMode, items_from_changes, render_items};
        let items = items_from_changes(changes, &[], DBNUM, keys);
        render_items(&items, DBNUM, WriteMode::Replace).sql
    }

    fn line_with<'a>(sql: &'a str, needle: &str) -> Option<&'a str> {
        sql.lines().find(|line| line.contains(needle))
    }

    /// 与 legacy 基线 `gen_sur_json_uda` 逐字节同形：同一份 UDA 值经两条路渲染出同一段 CONTENT。
    #[test]
    fn uda_row_content_is_byte_identical_to_legacy_gen_sur_json_uda() {
        let refno = refu(100);
        let (h_room, h_ref) = (uda_hash(0x1234), uda_hash(0x5678));
        let target = refu(300);
        let ele = element(
            refno,
            &[],
            &[
                (h_room, NamedAttrValue::StringType("A101".into())),
                (h_ref, NamedAttrValue::RefU64Type(target)),
            ],
        );
        let keys = keys(&[(h_room, 11), (h_ref, 12)]);

        let rows = super::ElementRows::render(&ele, &refno.to_string(), DBNUM, 42, &keys);
        let uda = rows
            .uda_row
            .clone()
            .expect("Add 带 UDA 值必须写 ATT_UDA 行");

        // legacy 写法：`UDA:<定义 refno>` 键进 NamedAttrMap → gen_sur_json_uda → normalize。
        let mut legacy = NamedAttrMap::default();
        legacy
            .map
            .insert("REFNO".into(), NamedAttrValue::RefU64Type(refno));
        legacy
            .map
            .insert("TYPE".into(), NamedAttrValue::StringType("EQUI".into()));
        legacy.map.insert(
            format!("UDA:{}", RefU64((24381u64 << 32) | 11)),
            NamedAttrValue::StringType("A101".into()),
        );
        legacy.map.insert(
            format!("UDA:{}", RefU64((24381u64 << 32) | 12)),
            NamedAttrValue::RefU64Type(target),
        );
        let expected = normalize_sql_string(&legacy.gen_sur_json_uda(&[]).expect("legacy json"));
        assert_eq!(
            uda, expected,
            "ATT_UDA CONTENT 必须与 legacy 基线逐字节同形"
        );
        // 消费者读的三样都在：`refno: <TYPE>:<id>`、`udas`、`u` 是 UDA:<refno> 记录链接。
        // 标量 `v` 是 NamedAttrValue 的带标签形（`{"StringType": …}`）——legacy 基线就这么写，
        // rs-core `NamedAttrValue::from((utype, Value::Object))` 取第一个字段值解回来；这里钉住
        // 的是「与现库同形」，不是「形状好看」。
        assert!(uda.contains("\"refno\": EQUI:"), "{uda}");
        assert!(uda.contains("\"udas\": ["), "{uda}");
        assert!(uda.contains("'u': UDA:24381_11"), "{uda}");
        assert!(uda.contains("'v': {\"StringType\":\"A101\"}"), "{uda}");
        assert!(
            uda.contains("'v': pe:7997_300"),
            "引用型 UDA 值走 to_pe_key: {uda}"
        );
        assert!(keys.take_misses().is_empty());
        // 批里它就是 ATT_UDA 那一条 INSERT 的内容，外加回退谓词要的 `dbnum` / `sesno`
        // 两列（计划 §9-6，`element_batch::stamp_owner_columns` 统一补）。CONTENT 本体
        // 仍与 legacy 逐字节同形——补列只在开头那个 `{` 后面插两格。
        let stamped =
            crate::data_interface::element_batch::stamp_owner_columns(&expected, DBNUM, 42);
        let sql = batch_sql(&[change(PersistElementState::Add(ele), refno)], &keys);
        assert!(
            sql.contains(&format!(
                "INSERT IGNORE INTO ATT_UDA [{stamped}] RETURN NONE;"
            )),
            "{sql}"
        );
        assert!(
            stamped.contains(expected.trim_start_matches('{')),
            "补列不得改动 CONTENT 本体: {stamped}"
        );
    }

    /// 没有 UDA 值的元素：删除块照样清它的 `ATT_UDA` 行（Modified 清空 / Add 复用 refno 的旧世代行
    /// 都不许残留），插入块不发 `ATT_UDA` 语句。
    #[test]
    fn empty_uda_is_deleted_and_never_inserted() {
        let refno = refu(101);
        let keys = UdaKeyMap::empty();

        for state in [
            PersistElementState::Modified(element(refno, &[], &[])),
            PersistElementState::Add(element(refno, &[], &[])),
        ] {
            let sql = batch_sql(&[change(state, refno)], &keys);
            assert!(
                sql.contains(&format!("LET $uda_ids = [ATT_UDA:{refno}];")),
                "无 UDA 必须删旧行:\n{sql}"
            );
            assert!(
                !sql.contains("INSERT IGNORE INTO ATT_UDA"),
                "无 UDA 不发 ATT_UDA 插入:\n{sql}"
            );
        }
    }

    /// 查不到定义的 UKEY 不静默丢：那个值不写，miss 记账给回执。
    #[test]
    fn unresolved_uda_key_is_recorded_instead_of_silently_dropped() {
        let refno = refu(102);
        let known = uda_hash(0x0100);
        let unknown = uda_hash(0x0200);
        let ele = element(
            refno,
            &[],
            &[
                (known, NamedAttrValue::IntegerType(1)),
                (unknown, NamedAttrValue::IntegerType(2)),
            ],
        );
        let keys = keys(&[(known, 21)]);
        let sql = batch_sql(&[change(PersistElementState::Modified(ele), refno)], &keys);
        assert!(
            sql.contains("INSERT IGNORE INTO ATT_UDA ["),
            "能解析的那个值仍要落库:\n{sql}"
        );
        assert!(sql.contains("'u': UDA:24381_21"), "{sql}");
        assert_eq!(
            sql.matches("'u':").count(),
            1,
            "查不到的键不许编一个链接写进去:\n{sql}"
        );

        let misses = keys.take_misses();
        assert_eq!(misses.len(), 1);
        assert!(misses[&unknown].contains(&refno));
    }

    /// `pe_owner` 边：`in = child, out = owner`，与 `cata_closure::render_pe_owner_replace`
    /// 同一份 children 渲染出同一批边——两处写口不许再各说各话。
    #[test]
    fn pe_owner_edges_point_child_to_owner_exactly_like_cata_closure() {
        let owner = refu(200);
        let children = [refu(201), refu(202), refu(203)];
        let keys = UdaKeyMap::empty();
        let sql = batch_sql(
            &[change(
                PersistElementState::Modified(element(owner, &children, &[])),
                owner,
            )],
            &keys,
        );

        let owner_key = owner.to_pe_key();
        let terminal_insert = line_with(&sql, "INSERT RELATION IGNORE INTO pe_owner")
            .expect("有 children 必须写 pe_owner");
        let terminal_edges = terminal_insert
            .split_once('[')
            .and_then(|(_, rest)| rest.rsplit_once(']'))
            .map(|(edges, _)| edges)
            .expect("边列表");
        let cata =
            crate::data_interface::cata_closure::render_pe_owner_replace(&owner_key, &children)
                .expect("cata 渲染");
        let cata_edges = line_with(&cata, "INSERT RELATION INTO pe_owner")
            .expect("cata 有 INSERT")
            .split_once('[')
            .and_then(|(_, rest)| rest.rsplit_once(']'))
            .map(|(edges, _)| edges)
            .expect("cata 边列表");
        assert_eq!(
            terminal_edges, cata_edges,
            "两处 pe_owner 写口渲出的边必须逐字节同形"
        );

        for (slot, child) in children.iter().enumerate() {
            assert!(
                terminal_insert.contains(&format!(
                    "{{ id: pe_owner:[{owner_key}, {slot}], in: {}, out: {owner_key} }}",
                    child.to_pe_key()
                )),
                "第 {slot} 条边方向错了:\n{terminal_insert}"
            );
        }
        // 删除块清的是本元素**作为 owner** 的子槽位（id 区间 `[owner, NONE]..=[owner, ..]`），
        // 与插入方向一致；Modified 不清自己的出向边（那由它的 owner 侧重插）。
        assert!(
            sql.contains(&format!(
                "DELETE pe_owner:[{owner_key}, NONE]..=[{owner_key}, ..];"
            )),
            "{sql}"
        );
        assert!(
            !sql.contains(&format!("DELETE {owner_key}->pe_owner")),
            "Modified 不许清自己的出向边:\n{sql}"
        );
        assert!(
            !terminal_insert.contains(&format!("in: {owner_key}")),
            "owner 不许出现在 in 端:\n{terminal_insert}"
        );
    }
}

impl ModifiedElement {
    /// Attribute + one-based qualifier pairs for modified array values.
    pub fn qualified_attribute_changes(&self) -> Vec<(String, usize)> {
        let mut changes = self
            .modified_attrs
            .iter()
            .chain(self.modified_explicit_attrs.iter())
            .flat_map(|(name, (old, new))| {
                changed_qualifiers(old, new)
                    .into_iter()
                    .map(|qualifier| (name.clone(), qualifier))
            })
            .collect::<Vec<_>>();
        changes.extend(
            self.modified_uda_attrs
                .iter()
                .flat_map(|(hash, (old, new))| {
                    changed_qualifiers(old, new)
                        .into_iter()
                        .map(move |qualifier| (format!("UDA:{hash}"), qualifier))
                }),
        );
        changes.sort();
        changes.dedup();
        changes
    }

    ///生成json patch的语句
    pub fn to_patch_json(&self) -> String {
        let mut operations = Vec::new();

        // 处理新增的普通属性
        for (key, attr) in &self.added_attrs {
            let value: serde_json::Value = attr.clone().into();
            operations.push(serde_json::json!({
                "op": "add",
                "path": key,
                "value": value
            }));
        }

        // 处理删除的普通属性
        for (key, _) in &self.deleted_attrs {
            operations.push(serde_json::json!({
                "op": "remove",
                "path": key
            }));
        }

        // 处理修改的普通属性
        for (key, (old_value, new_value)) in &self.modified_attrs {
            let value: serde_json::Value = new_value.clone().into();
            let old_value: serde_json::Value = old_value.clone().into();
            operations.push(serde_json::json!({
                "op": "replace",
                "path": key,
                "value": value,
                "old": old_value
            }));
        }

        // 处理新增的显式属性
        for (key, attr) in &self.added_explicit_attrs {
            let value: serde_json::Value = attr.clone().into();
            operations.push(serde_json::json!({
                "op": "add",
                "path": format!("{}", key),
                "value": value
            }));
        }

        // 处理删除的显式属性
        for (key, _) in &self.deleted_explicit_attrs {
            operations.push(serde_json::json!({
                "op": "remove",
                "path": format!("{}", key)
            }));
        }

        // 处理修改的显式属性
        for (key, (old_value, new_value)) in &self.modified_explicit_attrs {
            let value: serde_json::Value = new_value.clone().into();
            let old_value: serde_json::Value = old_value.clone().into();
            operations.push(serde_json::json!({
                "op": "replace",
                "path": format!("{}", key),
                "value": value,
                "old": old_value
            }));
        }

        // 处理子元素变更
        if let Some((old_children, new_children)) = &self.children_changed {
            // 将children序列化为RefU64的数组格式
            let old_children_json: Vec<String> =
                old_children.iter().map(|refno| refno.to_string()).collect();

            let new_children_json: Vec<String> =
                new_children.iter().map(|refno| refno.to_string()).collect();

            operations.push(serde_json::json!({
                "op": "replace",
                "path": "children",
                "value": new_children_json,
                "old": old_children_json
            }));
        }

        // // 处理新增的UDA属性
        // for (key, attr) in &self.added_uda_attrs {
        //     operations.push(serde_json::json!({
        //         "op": "add",
        //         "path": format!("uda/{}", key),
        //         "value": serde_json::to_value(attr.clone()).unwrap_or(serde_json::Value::Null)
        //     }));
        // }

        // // 处理删除的UDA属性
        // for (key, _) in &self.deleted_uda_attrs {
        //     operations.push(serde_json::json!({
        //         "op": "remove",
        //         "path": format!("uda/{}", key)
        //     }));
        // }

        // // 处理修改的UDA属性
        // for (key, (_, new_value)) in &self.modified_uda_attrs {
        //     operations.push(serde_json::json!({
        //         "op": "replace",
        //         "path": format!("uda/{}", key),
        //         "value": serde_json::to_value(new_value.clone()).unwrap_or(serde_json::Value::Null)
        //     }));
        // }

        // 序列化为字符串
        serde_json::to_string(&operations).unwrap_or_default()
    }

    /// 转义嵌入单引号 SurrealQL 字符串字面量的外部文本（如 NAME）：反斜杠与单引号。
    /// 与 gen-model 的 `dbnum_state::escape_surql_str` 同规则，避免名字含 `'` 或 `\`
    /// （中文录入 / 特殊字符）破坏 SQL 字面量甚至注入。
    fn escape_surql_str(raw: &str) -> String {
        raw.replace('\\', "\\\\").replace('\'', "\\'")
    }

    /// 生成SurrealQL的UPSERT MERGE语句，用于将ModifiedElement的修改应用到数据库
    ///
    /// # 参数
    /// * `id` - 要修改的记录ID
    ///
    /// # 返回值
    /// 返回完整的SurrealQL UPSERT MERGE语句
    pub fn to_modify_surql(&self, id: &str, sesno: u32) -> String {
        let mut main_fields = serde_json::Map::new();
        let mut uda_attrs = serde_json::Map::new();

        // 引用型(record link)字段条目，如 `"OWNER": pe:123`。必须与标量字段合并进
        // 同一个 MERGE 对象，且条目间以逗号分隔（历史实现用无分隔的字符串拼接，见 build_merge_fields）。
        let mut record_entries: Vec<String> = Vec::new();
        // 默认更新 sesno
        let mut pe_update_sql = format!("UPDATE pe:{} SET sesno = {}", id, sesno);
        let mut relate_sql = String::new();
        let mut final_sql = String::new();

        // 如果children发生变化,需要先删除现有的pe_owner关系,再重新插入新的children关系
        if let Some((_, new_children)) = &self.children_changed {
            // 删除现有的pe_owner关系
            final_sql.push_str(&format!("DELETE pe:{}<-pe_owner;\n", id));

            let mut json = Vec::new();
            // 插入新的children关系
            for (i, child) in new_children.iter().enumerate() {
                json.push(format!(
                    "{{ id:[pe:{}, {}], in: pe:{}, out: pe:{} }}",
                    id, i, child, id
                ));
            }
            if !json.is_empty() {
                final_sql.push_str(&format!(
                    "INSERT RELATION INTO pe_owner [ {} ];\n",
                    json.join(", ")
                ));
            }
        }
        // dbg!(&final_sql);

        // 处理新增的普通属性
        for (key, attr) in &self.added_attrs {
            if let NamedAttrValue::RefU64Type(refno) = attr {
                record_entries.push(format!("{}: pe:{refno}", quote_ident(key)));
            } else {
                main_fields.insert(key.clone(), attr.clone().into());
            }
        }

        // 处理修改的普通属性
        for (key, (_, new_attr)) in &self.modified_attrs {
            if let NamedAttrValue::RefU64Type(refno) = new_attr {
                record_entries.push(format!("{}: pe:{refno}", quote_ident(key)));
            } else {
                main_fields.insert(key.clone(), new_attr.clone().into());
            }
        }

        // 处理删除的普通属性
        for key in self.deleted_attrs.keys() {
            main_fields.insert(key.clone(), serde_json::Value::Null);
        }

        // 处理新增的显式属性
        for (key, attr) in &self.added_explicit_attrs {
            if key == "NAME" {
                if let NamedAttrValue::StringType(name) = attr {
                    // 如果是NAME属性，生成pe的更新语句，同时更新name和sesno
                    pe_update_sql = format!(
                        "UPDATE pe:{} SET name = '{}', sesno = {}",
                        id,
                        Self::escape_surql_str(name),
                        sesno
                    );
                }
            }
            if let NamedAttrValue::RefU64Type(refno) = attr {
                record_entries.push(format!("{}: pe:{refno}", quote_ident(key)));
            } else {
                main_fields.insert(key.clone(), attr.clone().into());
            }
        }

        // 处理修改的显式属性
        for (key, (_, new_attr)) in &self.modified_explicit_attrs {
            if key == "NAME" {
                if let NamedAttrValue::StringType(name) = new_attr {
                    // 如果是NAME属性，生成pe的更新语句，同时更新name和sesno
                    pe_update_sql = format!(
                        "UPDATE pe:{} SET name = '{}', sesno = {}",
                        id,
                        Self::escape_surql_str(name),
                        sesno
                    );
                }
            }
            if let NamedAttrValue::RefU64Type(refno) = new_attr {
                record_entries.push(format!("{}: pe:{refno}", quote_ident(key)));
            } else {
                main_fields.insert(key.clone(), new_attr.clone().into());
            }
        }

        // 处理删除的显式属性
        for key in self.deleted_explicit_attrs.keys() {
            main_fields.insert(key.clone(), serde_json::Value::Null);
        }

        // 处理新增的UDA属性
        for (key, attr) in &self.added_uda_attrs {
            if let NamedAttrValue::RefU64Type(refno) = attr {
                uda_attrs.insert(
                    key.to_string(),
                    serde_json::json!({ "type": "refno", "value": format!("pe:{refno}") }),
                );
            } else {
                uda_attrs.insert(key.to_string(), attr.clone().into());
            }
        }

        // 处理修改的UDA属性
        for (key, (_, new_attr)) in &self.modified_uda_attrs {
            if let NamedAttrValue::RefU64Type(refno) = new_attr {
                uda_attrs.insert(
                    key.to_string(),
                    serde_json::json!({ "type": "refno", "value": format!("pe:{refno}") }),
                );
            } else {
                uda_attrs.insert(key.to_string(), new_attr.clone().into());
            }
        }

        // 处理删除的UDA属性
        for key in self.deleted_uda_attrs.keys() {
            uda_attrs.insert(key.to_string(), serde_json::Value::Null);
        }

        // 如果有UDA属性，则添加到main_fields中
        if !uda_attrs.is_empty() {
            main_fields.insert("uda".to_string(), serde_json::Value::Object(uda_attrs));
        }

        // P2-1：OWNER 变更（元素搬迁）时同步更新 pe.owner 标量。否则搬迁后 noun 记录
        // 的 OWNER 与父 children_changed 重建的 pe_owner 图都指向新父，唯独 pe.owner
        // 仍停留在旧父，造成数据面不一致（Add 路径已写 owner，这里对齐 modify）。
        if let Some(new_owner) = new_owner_from_changes(
            &self.added_attrs,
            &self.modified_attrs,
            &self.added_explicit_attrs,
            &self.modified_explicit_attrs,
        ) {
            pe_update_sql.push_str(&format!(", owner = pe:{}", new_owner));
        }

        // 构建完整的SurrealQL语句：把标量/UDA/置空字段与引用型(record link)字段
        // 合并进同一个 MERGE 对象（历史实现分开拼接，导致引用属性丢失 / 生成非法 SQL /
        // 多引用缺逗号三处正确性 bug，详见 build_merge_fields 文档）。
        let id = format!("{}:{}", &self.noun, id);
        if let Some(merge_body) = build_merge_fields(&main_fields, &record_entries) {
            final_sql.push_str(&format!("UPSERT {} MERGE {}", id, merge_body));
        }

        // 总是包含pe更新语句（至少更新sesno）
        if !final_sql.is_empty() {
            final_sql = format!("{};\n{}", final_sql, pe_update_sql);
        } else {
            final_sql = pe_update_sql;
        }

        final_sql
    }

    /// 获取所有属性名称
    pub fn att_names(&self) -> HashSet<String> {
        let mut names = HashSet::new();
        names.extend(self.added_attrs.keys().cloned());
        names.extend(self.deleted_attrs.keys().cloned());
        names.extend(self.modified_attrs.keys().cloned());
        names.extend(self.added_explicit_attrs.keys().cloned());
        names.extend(self.deleted_explicit_attrs.keys().cloned());
        names.extend(self.modified_explicit_attrs.keys().cloned());
        names
    }

    /// 获取所有UDA属性键
    pub fn uda_keys(&self) -> HashSet<i32> {
        let mut keys = HashSet::new();
        keys.extend(self.added_uda_attrs.keys().cloned());
        keys.extend(self.deleted_uda_attrs.keys().cloned());
        keys.extend(self.modified_uda_attrs.keys().cloned());
        keys
    }
}

/// 参考号操作状态的详细信息
#[derive(Clone)]
pub enum EleOperationDetail {
    /// 新增元素，包含完整属性映射
    Add(EleData),
    /// 已删除的元素, 里面包含的是类型
    Deleted,
    /// 已修改的元素，包含新增、删除和修改的属性
    Modified(ModifiedElement),
    /// 无操作
    None,
}

/// 元素操作数据，包含操作明细、参考号和会话号
#[derive(Clone, Debug)]
pub struct EleOperationData {
    /// 参考号
    pub refno: RefU64,
    /// 会话号
    pub sesno: u32,
    /// 操作明细
    pub detail: EleOperationDetail,
}

impl EleOperationData {
    /// 创建新的元素操作数据
    pub fn new(refno: RefU64, sesno: u32, detail: EleOperationDetail) -> Self {
        Self {
            refno,
            sesno,
            detail,
        }
    }

    /// 获取操作类型
    pub fn get_op_type(&self) -> &'static str {
        self.detail.get_op_type()
    }

    /// 获取元素类型
    pub fn get_noun_type(&self) -> String {
        self.detail.get_noun_type()
    }

    /// 检查是否为几何体变化
    pub fn is_geometry_update(&self) -> bool {
        self.detail.is_geometry_update()
    }

    /// 检查是否为变换（位置旋转等）变化
    pub fn is_transform_change(&self) -> bool {
        self.detail.is_transform_change()
    }

    /// 将操作状态转换为SurrealQL语句
    pub fn to_surql(&self, id: &str, dbnum: i32, sesno: u32) -> String {
        self.detail.to_surql(id, dbnum, sesno)
    }
}

/// 将RefU64到EleOperationDetail的映射转换为EleOperationData向量
pub fn convert_to_operation_data(
    map: HashMap<RefU64, EleOperationDetail>,
    sesno: u32,
) -> Vec<EleOperationData> {
    map.into_iter()
        .map(|(refno, detail)| EleOperationData::new(refno, sesno, detail))
        .collect()
}

/// 元素没有 UDA 值时 `ATT_UDA:{id}` 行该怎么处理。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UdaRowPolicy {
    /// `Add`：refno 复用留下的旧行由 `render_terminal_persist_statements` 的前置清理
    /// （`DELETE ATT_UDA:{pe}`）负责，这里不再发一条。
    AbsentIsClean,
    /// `Modified`：UDA 值全被清掉时旧行不许残留（与 `RetiredTypeRow` 同理）。
    DeleteWhenEmpty,
}

/// `pe_owner` 边的唯一写法：`in = child, out = owner`，id 是 `[owner, 槽位]`。
///
/// 读侧（rs-core `{owner}<-pe_owner` 取 `in` 当 children、`->pe_owner.out` 取 owner）、
/// `unique_pe_owner (in, out)` 索引、`cata_closure::render_pe_owner_statements`、
/// legacy `EleOperationDetail::to_surql` 全按这个方向；基线路自 2026-09-04 换成
/// e3d-io 退化窗口后直接经 `ElementRows` 调本函数，不再有第二个 `pe_owner` 写口；
/// 2026-09-04 之前本文件的终态渲染器是唯一写反的地方（`in = owner, out = child`），
/// 增量路写出的子树在树面板上读不到。
pub(crate) fn pe_owner_edge(owner_key: &str, slot: usize, child_key: &str) -> String {
    format!("{{ id: pe_owner:[{owner_key}, {slot}], in: {child_key}, out: {owner_key} }}")
}

/// `ATT_UDA:{id}` 行的 CONTENT：与 legacy 基线 `gen_sur_json_uda` **同一个函数**产出
/// （`{ id, TYPE, refno: <TYPE>:<id>, udas: [{ u: UDA:<def>, v }] }`），只是 UKEY → `UDA:<refno>`
/// 的换算走内存表。没有一个 UDA 值能落库时返回 `None`。
fn render_uda_row_content(
    ele_data: &EleData,
    att_map: &NamedAttrMap,
    uda_keys: &UdaKeyMap,
) -> Option<String> {
    if ele_data.whole_attmap.uda_atts.is_empty() {
        return None;
    }
    let element = att_map
        .get_refno_by_att("REFNO")
        .map(|refno| refno.refno())
        .unwrap_or(ele_data.refno);
    let mut uda_map = NamedAttrMap::default();
    for key in ["REFNO", "TYPE"] {
        if let Some(value) = att_map.map.get(key) {
            uda_map.map.insert(key.to_string(), value.clone());
        }
    }
    let mut resolved = 0usize;
    for attr in &ele_data.whole_attmap.uda_atts {
        let Some(definition) = uda_keys.resolve_for(element, attr.hash_val) else {
            continue;
        };
        uda_map
            .map
            .insert(format!("UDA:{definition}"), attr.value.clone());
        resolved += 1;
    }
    if resolved == 0 {
        return None;
    }
    uda_map
        .gen_sur_json_uda(&[])
        .map(|json| normalize_sql_string(&json))
}

/// 一个元素终态的**行形状**：`pe` / `<NOUN>` / `ATT_UDA` / `pe_owner` 边。
///
/// rocksdb 形态下基线与增量都经 `data_interface::element_batch` 把它们攒成
/// 「先删后插」的批（每表一条 `INSERT IGNORE INTO … [..]`，plan 2026-09-06 §3 D2）——语句形状
/// 与行形状都只有这一份（宪法 II）：`pe` 行 = `SPdmsElement::gen_sur_json`、noun 行 =
/// `NamedAttrMap::gen_sur_json`、UDA 行 = `gen_sur_json_uda`、边 = [`pe_owner_edge`]。
/// 读透形态（内存库）刷新缓存行走 [`Self::read_through_statements`]（逐元素 UPSERT），
/// **那不是 rocksdb 口径**。
pub struct ElementRows {
    /// `dbnum_refno`。
    pub id: String,
    pub noun: String,
    pe: aios_core::pe::SPdmsElement,
    /// `ATT_<noun>:{id}` 的 CONTENT（自带 `"id"` 字段）。
    pub noun_row: String,
    /// `ATT_UDA:{id}` 的 CONTENT；没有一个 UDA 值能落库时为 `None`。
    pub uda_row: Option<String>,
    /// `{ id: pe_owner:[owner, slot], in: child, out: owner }`，按成员序。
    pub owner_edges: Vec<String>,
}

impl ElementRows {
    pub fn render(
        ele_data: &EleData,
        id: &str,
        dbnum: i32,
        sesno: u32,
        uda_keys: &UdaKeyMap,
    ) -> Self {
        let att_map = ele_data.whole_attmap.merge();
        let mut pe = att_map.pe(dbnum);
        pe.sesno = sesno as _;
        let owner_key = format!("pe:{id}");
        let owner_edges = ele_data
            .children
            .iter()
            .enumerate()
            .map(|(slot, child)| pe_owner_edge(&owner_key, slot, &format!("pe:{child}")))
            .collect();
        Self {
            id: id.to_string(),
            noun: att_map.get_type(),
            uda_row: render_uda_row_content(ele_data, &att_map, uda_keys),
            noun_row: att_map.gen_sur_json().unwrap_or_default(),
            owner_edges,
            pe,
        }
    }

    pub fn pe_key(&self) -> String {
        format!("pe:{}", self.id)
    }

    /// `pe` 行 CONTENT，不带 `id`（给 `UPSERT pe:{id} CONTENT …`）。
    pub fn pe_row(&self) -> String {
        self.pe.gen_sur_json(None)
    }

    /// `pe` 行 JSON，带 `id: pe:{id}`（给 `INSERT IGNORE INTO pe [..]`）。
    pub fn pe_row_with_id(&self) -> String {
        self.pe.gen_sur_json(Some(self.pe_key()))
    }

    /// 墓碑 `pe` 行 JSON（带 id）：与 [`Self::pe_row_with_id`] 同一形状，只是 `deleted = true`、
    /// `sesno` 换成删除落地的那个会话。noun / owner / name 原样保留——`datacenter` 的 Deleted
    /// 分支、ADR-036 成员补删、`cleanup_deleted_by_pe_state` 读的都是它们。
    pub fn tombstone_row_with_id(&self, sesno: u32) -> String {
        let mut pe = self.pe.clone();
        pe.deleted = true;
        pe.sesno = sesno as _;
        pe.gen_sur_json(Some(self.pe_key()))
    }

    /// 基侧读不到时的最小墓碑：只带 id / dbnum / sesno / deleted。没有 noun 与 owner，
    /// 读它的人会把这一条当「链断」跳过——比不留痕迹好，比完整墓碑差，所以要出声。
    pub fn minimal_tombstone_row(id: &str, dbnum: i32, sesno: u32) -> String {
        format!("{{ \"id\": pe:{id}, \"dbnum\": {dbnum}, \"sesno\": {sesno}, \"deleted\": true }}")
    }

    pub(crate) fn sesno(&self) -> u32 {
        self.pe.sesno.max(0) as u32
    }

    /// 读透形态（内存库）刷新缓存行的逐元素语句（`DeleteWhenEmpty`）。**不是 rocksdb 口径**：
    /// 摄入形态的基线与增量一律走 `element_batch` 的批量先删后插；这里保留 UPSERT 只因为
    /// 读透缓存要「新会话盖旧会话」的守卫（[`Self::read_through_guarded_statements`]）。
    pub(crate) fn read_through_statements(&self, uda_policy: UdaRowPolicy) -> String {
        let id = &self.id;
        let owner_key = self.pe_key();
        let mut statements = vec![
            format!("UPSERT {owner_key} CONTENT {};", self.pe_row()),
            format!("UPSERT {}:{id} CONTENT {};", self.noun, self.noun_row),
        ];
        match (&self.uda_row, uda_policy) {
            (Some(content), _) => {
                statements.push(format!("UPSERT ATT_UDA:{id} CONTENT {content};"))
            }
            (None, UdaRowPolicy::DeleteWhenEmpty) => {
                statements.push(format!("DELETE ATT_UDA:{id};"))
            }
            (None, UdaRowPolicy::AbsentIsClean) => {}
        }
        statements.push(format!("DELETE {owner_key}<-pe_owner;"));
        if !self.owner_edges.is_empty() {
            statements.push(format!(
                "INSERT RELATION INTO pe_owner [{}];",
                self.owner_edges.join(",")
            ));
        }
        statements.join("\n")
    }

    /// Read-through cache replacement guarded by the element's last-touch
    /// session in the same transaction as every dependent row.  An older
    /// parse that finishes late therefore cannot overwrite a newer `pe`, noun,
    /// UDA or owner-edge view.
    pub(crate) fn read_through_guarded_statements(&self, uda_policy: UdaRowPolicy) -> String {
        let variable = format!("current_{}", self.id.replace(['-', '/'], "_"));
        let noun_variable = format!("noun_{}", self.id.replace(['-', '/'], "_"));
        let body = self.read_through_statements(uda_policy);
        let noun = serde_json::to_string(&self.noun).unwrap_or_else(|_| "\"\"".into());
        let record_id = serde_json::to_string(&self.id).unwrap_or_else(|_| "\"\"".into());
        format!(
            "LET ${variable} = (SELECT VALUE sesno FROM ONLY {});\n\
             LET ${noun_variable} = (SELECT VALUE noun FROM ONLY {});\n\
             IF ${variable} = NONE OR ${variable} <= {} {{\n\
             IF ${noun_variable} != NONE AND ${noun_variable} != {noun} {{ DELETE type::thing(${noun_variable}, {record_id}); }};\n{}\n}};",
            self.pe_key(),
            self.pe_key(),
            self.sesno(),
            body
        )
    }
}

impl EleOperationDetail {
    /// 根据操作类型生成SurrealQL语句
    ///
    /// # 参数
    /// * `id` - 要操作的记录ID
    ///
    /// # 返回值
    /// 返回完整的SurrealQL语句（CREATE/UPSERT/DELETE）
    pub fn to_surql(&self, id: &str, dbnum: i32, sesno: u32) -> String {
        match self {
            // 新增元素：使用幂等的 UPSERT（原 CREATE/INSERT 非幂等）
            Self::Add(ele_data) => {
                let att_map = ele_data.whole_attmap.merge();

                // pe 记录：用 UPSERT ... CONTENT（幂等 create-or-replace）。
                // 原来的 `INSERT INTO pe [...]` 在记录已存在时会报错——ADR-001 下
                // 失败批次会按同一 sesno 窗口重试，若上次已部分写入，重试就会撞已
                // 存在记录而反复失败、导致该 dbnum 水位永久卡死。gen_sur_json(None)
                // 不含 id 字段，由 UPSERT 目标 pe:{id} 指定。
                let mut pe_data = att_map.pe(dbnum);
                pe_data.sesno = sesno as _;
                let pe_json = pe_data.gen_sur_json(None);
                let pe_sql = format!("UPSERT pe:{} CONTENT {};", id, pe_json);

                // noun 主记录同样改用 UPSERT（原 CREATE 撞已存在记录会失败）；
                // 「只保留最新状态」下 create-or-replace 正是期望语义。
                let create_sql = format!(
                    "UPSERT {}:{} CONTENT {};",
                    att_map.get_type(),
                    id,
                    att_map.gen_sur_json().unwrap()
                );

                let mut relate_sql = String::new();
                if !ele_data.children.is_empty() {
                    // 先删后插，保证幂等（与 Modified 路径一致）：pe 主记录已用 UPSERT，
                    // 但 pe_owner 关系用固定复合 id `[pe:{id}, i]`，裸 INSERT RELATION 在
                    // 记录已存在时会报错。ADR-001 下失败批次按同一 sesno 窗口重试；若上次
                    // 已部分写入 pe_owner，重试就会撞已存在关系反复失败、卡死该 dbnum 水位。
                    // 先 DELETE 本元素的入向 pe_owner 边再重插，使重放收敛。
                    relate_sql.push_str(&format!("DELETE pe:{}<-pe_owner;\n", id));

                    // 插入新的children关系
                    let mut json = Vec::new();
                    for (i, child) in ele_data.children.iter().enumerate() {
                        json.push(format!(
                            "{{ id:[pe:{}, {}], in: pe:{}, out: pe:{} }}",
                            id, i, child, id
                        ));
                    }
                    if !json.is_empty() {
                        relate_sql.push_str(&format!(
                            "INSERT RELATION INTO pe_owner [ {} ];\n",
                            json.join(", ")
                        ));
                    }
                }

                // 合并两个SQL语句
                format!("{}\n{}\n{}", pe_sql, create_sql, relate_sql)
            }

            // 修改元素：使用UPSERT MERGE语句
            Self::Modified(modified_element) => modified_element.to_modify_surql(id, sesno),

            // 删除元素：使用DELETE语句
            Self::Deleted => {
                format!("UPDATE pe:{} SET deleted = true, sesno = {}", id, sesno)
            }

            // 无操作：返回空字符串
            Self::None => String::new(),
        }
    }

    /// 获取操作类型
    pub fn get_op_type(&self) -> &'static str {
        match self {
            Self::Add(_) => "新增",
            Self::Modified(_) => "修改",
            Self::Deleted => "删除",
            Self::None => "无操作",
        }
    }

    /// 获取元素的类型名称
    pub fn get_noun_type(&self) -> String {
        match self {
            Self::Add(ele) => ele.att_map().get_type(),
            Self::Modified(ele) => ele.noun.clone(),
            Self::Deleted => String::new(),
            Self::None => String::new(),
        }
    }

    //增加方法检查是否是几何体的变化
    pub fn is_geometry_update(&self) -> bool {
        //if is none return false
        let noun_type = self.get_noun_type();
        let noun_type_str = noun_type.as_str();
        if noun_type.is_empty() {
            return false;
        }

        //判断是否是结合体类型
        let is_cata_geo_type = CATA_WITHOUT_REUSE_GEO_NAMES.contains(&noun_type_str)
            || CATA_HAS_TUBI_GEO_NAMES.contains(&noun_type_str)
            || CATA_SINGLE_REUSE_GEO_NAMES.contains(&noun_type_str);

        let is_piping_type = PIPING_NOUN_NAMES.contains(&noun_type_str);

        let is_prim_geo_type = PRIMITIVE_NOUN_NAMES.contains(&noun_type_str)
            || GENRAL_NEG_NOUN_NAMES.contains(&noun_type_str);
        if is_prim_geo_type {
            match self {
                Self::Add(_) => true,
                Self::Modified(ele) => ele
                    .att_names()
                    .iter()
                    .any(|name| PRIMITIVE_GEO_ATTR_NAMES.contains(&name.as_str())),
                Self::Deleted => true,
                Self::None => false,
            }
        } else if is_cata_geo_type || is_piping_type {
            match self {
                Self::Add(_) => true,
                Self::Modified(ele) => ele
                    .att_names()
                    .iter()
                    .any(|name| CATA_GEO_ATTR_NAMES.contains(&name.as_str())),
                Self::Deleted => true,
                Self::None => false,
            }
        } else {
            false
        }
    }

    //还要加入是否是transform 变化，让子节点都去更新 world transform
    pub fn is_transform_change(&self) -> bool {
        match self {
            Self::Add(_) => false,
            Self::Modified(ele) => ele
                .att_names()
                .iter()
                .any(|name| TRANSFORM_ATTR_NAMES.contains(&name.as_str())),
            Self::Deleted => false,
            Self::None => false,
        }
    }
}

impl std::fmt::Debug for EleOperationDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_transform_change() {
            writeln!(f, "  存在坐标变换修改")?;
        }

        let is_geometry_update = self.is_geometry_update();
        match self {
            Self::Add(ele) => {
                writeln!(f, "EleOperationDetail::Add({:?})", ele.refno)?;
                writeln!(f, "  类型: {}", ele.att_map().get_type())?;
                if is_geometry_update {
                    writeln!(f, "  新增几何体")?;
                }
                Ok(())
            }
            Self::Deleted => {
                writeln!(f, "EleOperationDetail::Deleted")?;
                if is_geometry_update {
                    writeln!(f, "  删除几何体")?;
                }
                Ok(())
            }
            Self::Modified(ele) => {
                writeln!(f, "EleOperationDetail::Modified {{")?;

                let mut has_changes = false;
                // 打印类型
                writeln!(f, "  类型: {}", ele.noun)?;
                if is_geometry_update {
                    writeln!(f, "  几何体变化")?;
                }

                // 检查子元素变化
                if let Some((old_children, new_children)) = &ele.children_changed {
                    has_changes = true;
                    writeln!(f, "  子元素变化:")?;
                    writeln!(f, "    - 旧子元素数量: {}", old_children.len())?;
                    writeln!(f, "    - 新子元素数量: {}", new_children.len())?;

                    // 找出新增的子元素
                    let added: Vec<_> = new_children
                        .iter()
                        .filter(|refno| !old_children.contains(refno))
                        .collect();
                    if !added.is_empty() {
                        writeln!(f, "    - 新增子元素 ({}):", added.len())?;
                        for refno in added.iter().take(5) {
                            writeln!(f, "      * {:?}", refno)?;
                        }
                        if added.len() > 5 {
                            writeln!(f, "      * ... 及其他 {} 个", added.len() - 5)?;
                        }
                    }

                    // 找出删除的子元素
                    let removed: Vec<_> = old_children
                        .iter()
                        .filter(|refno| !new_children.contains(refno))
                        .collect();
                    if !removed.is_empty() {
                        writeln!(f, "    - 删除子元素 ({}):", removed.len())?;
                        for refno in removed.iter().take(5) {
                            writeln!(f, "      * {:?}", refno)?;
                        }
                        if removed.len() > 5 {
                            writeln!(f, "      * ... 及其他 {} 个", removed.len() - 5)?;
                        }
                    }
                }

                // 普通属性变化
                if !ele.added_attrs.is_empty() {
                    has_changes = true;
                    writeln!(f, "  新增属性 ({}):", ele.added_attrs.len())?;
                    for (name, value) in &ele.added_attrs {
                        writeln!(f, "    - {}: {}", name, value.get_val_as_string())?;
                    }
                }

                if !ele.deleted_attrs.is_empty() {
                    has_changes = true;
                    writeln!(f, "  删除属性 ({}):", ele.deleted_attrs.len())?;
                    for (name, value) in &ele.deleted_attrs {
                        writeln!(f, "    - {}: {}", name, value.get_val_as_string())?;
                    }
                }

                if !ele.modified_attrs.is_empty() {
                    has_changes = true;
                    writeln!(f, "  修改属性 ({}):", ele.modified_attrs.len())?;
                    for (name, (old_val, new_val)) in &ele.modified_attrs {
                        // dbg!(&new_val);
                        writeln!(
                            f,
                            "    - {}: {} -> {}",
                            name,
                            old_val.get_val_as_string(),
                            new_val.get_val_as_string()
                        )?;
                    }
                }

                // 显式属性变化
                if !ele.added_explicit_attrs.is_empty() {
                    has_changes = true;
                    writeln!(f, "  新增显式属性 ({}):", ele.added_explicit_attrs.len())?;
                    for (name, value) in &ele.added_explicit_attrs {
                        writeln!(f, "    - {} = {}", name, value.get_val_as_string())?;
                    }
                }

                if !ele.deleted_explicit_attrs.is_empty() {
                    has_changes = true;
                    writeln!(f, "  删除显式属性 ({}):", ele.deleted_explicit_attrs.len())?;
                    for (name, value) in &ele.deleted_explicit_attrs {
                        writeln!(f, "    - {} = {}", name, value.get_val_as_string())?;
                    }
                }

                if !ele.modified_explicit_attrs.is_empty() {
                    has_changes = true;
                    writeln!(f, "  修改显式属性 ({}):", ele.modified_explicit_attrs.len())?;
                    for (name, (old_val, new_val)) in &ele.modified_explicit_attrs {
                        writeln!(
                            f,
                            "    - {} = {} -> {}",
                            name,
                            old_val.get_val_as_string(),
                            new_val.get_val_as_string()
                        )?;
                    }
                }

                // UDA属性变化
                if !ele.added_uda_attrs.is_empty() {
                    has_changes = true;
                    writeln!(f, "  新增UDA属性 ({}):", ele.added_uda_attrs.len())?;
                    for (name, value) in &ele.added_uda_attrs {
                        writeln!(f, "    - {} = {}", name, value.get_val_as_string())?;
                    }
                }

                if !ele.deleted_uda_attrs.is_empty() {
                    has_changes = true;
                    writeln!(f, "  删除UDA属性 ({}):", ele.deleted_uda_attrs.len())?;
                    for (name, value) in &ele.deleted_uda_attrs {
                        writeln!(f, "    - {} = {}", name, value.get_val_as_string())?;
                    }
                }

                if !ele.modified_uda_attrs.is_empty() {
                    has_changes = true;
                    writeln!(f, "  修改UDA属性 ({}):", ele.modified_uda_attrs.len())?;
                    for (name, (old_val, new_val)) in &ele.modified_uda_attrs {
                        writeln!(
                            f,
                            "    - {} = {} -> {}",
                            name,
                            old_val.get_val_as_string(),
                            new_val.get_val_as_string()
                        )?;
                    }
                }

                // 如果所有属性都为空，显示无变化信息
                if !has_changes {
                    writeln!(f, "  无任何属性变化")?;
                }

                write!(f, "}}")
            }
            Self::None => write!(f, "EleOperationDetail::None"),
        }
    }
}
