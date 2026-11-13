//! `MutationBatch` → 落库行渲染（spec 决策 2 的落地端）。
//!
//! 输出是与 `data_interface::ele_ops::EleOperationDetail` **字段级同构**的中性 DTO：
//! [`EditElement`] ⇄ `EleData`、[`EditModify`] ⇄ `ModifiedElement`、
//! [`EditOp`] ⇄ `EleOperationDetail`。渲染成 `EleOperationData` 的适配在消费侧
//! （aios-database）逐字段映射——本 crate 刻意不依赖它，保持测试接缝轻量
//! （`attmap_of`/`members_of` 等基值全部经 [`PipeCatalog`] 注入）。
//!
//! 渲染契约（与 `e3d_collector::assemble_ele_data` 同一分法）：
//! - `NAME` 只进**显式**命名空间（`explicit_attmap`）——`to_modify_surql` 对
//!   `NAME` 只在那里认；`EditElement.name` 冗余一份便于直接投影；
//! - 其余属性进隐式 `attmap`；本服务不发属性删除与 UDA 写，`deleted_*`/`uda_*`
//!   面恒空，适配侧补默认即可；
//! - `noun` 字段存 noun 名的 **db1 哈希**（`aios_core::tool::db_tool::db1_hash`），
//!   与 `EleData.noun` 同义；
//! - 批内顺序即渲染顺序：同一元素的连续 `SetAttrs` 出多条 `Modify`，
//!   `current` 反映逐条应用后的累积状态（E3D 写序语义的保留面）。

use crate::batch::{Confirm, MutationBatch, MutationOp};
use crate::catalog::PipeCatalog;
use crate::outcome::PipeWarning;
use aios_core::tool::db_tool::db1_hash;
use aios_core::{NamedAttrMap, NamedAttrValue, RefU64};
use std::collections::HashMap;

/// ⇄ `EleData`：元素在某时刻的完整状态投影。
#[derive(Debug, Clone, Default)]
pub struct EditElement {
    pub refno: RefU64,
    pub owner: RefU64,
    /// noun 名的 db1 哈希（`EleData.noun` 同义）。
    pub noun: u32,
    /// noun 名字符串（`ModifiedElement.noun` 同义；EleData 侧只存哈希，
    /// 名字留给适配层填 ModifiedElement）。
    pub noun_name: String,
    /// 隐式命名空间（`EleData.whole_attmap.att_map()`）。
    pub attmap: NamedAttrMap,
    /// 显式命名空间（`EleData.whole_attmap.explicit_attmap()`）；`NAME` 只在这里。
    pub explicit_attmap: NamedAttrMap,
    /// 成员表（`EleData.children`）。
    pub children: Vec<RefU64>,
    pub name: String,
}

/// ⇄ `ModifiedElement`：一次修改的 diff + 改后全量。
#[derive(Debug, Clone, Default)]
pub struct EditModify {
    /// 本条 op 应用后的累积状态（`ModifiedElement.current_data`）。
    pub current: EditElement,
    /// 新增隐式属性。
    pub added_attrs: HashMap<String, NamedAttrValue>,
    /// 修改隐式属性：`(旧值, 新值)`。
    pub modified_attrs: HashMap<String, (NamedAttrValue, NamedAttrValue)>,
    /// 新增显式属性（改名走这里：`NAME`）。
    pub added_explicit_attrs: HashMap<String, NamedAttrValue>,
    /// 修改显式属性。
    pub modified_explicit_attrs: HashMap<String, (NamedAttrValue, NamedAttrValue)>,
    /// 成员表变更 `(旧表, 新表)`（`ModifiedElement.children_changed` 同义；
    /// `None` = 本条 op 未动成员表）。
    pub children_changed: Option<(Vec<RefU64>, Vec<RefU64>)>,
}

/// ⇄ `EleOperationDetail`：单个行级操作。
#[derive(Debug, Clone)]
pub enum EditOp {
    Add(EditElement),
    Modify(EditModify),
    Delete { refno: RefU64 },
}

/// 渲染产物：有序操作 + 透传的确认项与警告（供持久层一并落痕）。
#[derive(Debug, Default)]
pub struct RenderedBatch {
    pub name: String,
    pub ops: Vec<EditOp>,
    pub confirms: Vec<Confirm>,
    pub warnings: Vec<PipeWarning>,
}

/// 批内当前状态：渲染器对 catalog 基值 + 批内已应用 op 的滚动投影。
struct State<'a> {
    cat: &'a dyn PipeCatalog,
    elems: HashMap<RefU64, EditElement>,
    warnings: Vec<PipeWarning>,
}

impl<'a> State<'a> {
    /// 取元素当前状态；不在批内投影则从 catalog 播种。基值缺失时给最小骨架并出声
    /// （diff 退化为全量 added——不吞错也不拦行，持久层拿到的是诚实形状）。
    fn ensure(&mut self, r: RefU64) -> &mut EditElement {
        if !self.elems.contains_key(&r) {
            let mut el = EditElement {
                refno: r,
                ..Default::default()
            };
            let mut missing = false;
            match self.cat.noun_of(r) {
                Some(noun) => {
                    el.noun = db1_hash(&noun);
                    el.noun_name = noun;
                }
                None => missing = true,
            }
            if let Some(owner) = self.cat.owner_of(r) {
                el.owner = owner;
            }
            if let Some(name) = self.cat.name_of(r) {
                el.name = name;
            }
            match self.cat.attmap_of(r) {
                Some(m) => el.attmap = m,
                None => missing = true,
            }
            el.children = self.cat.members_of(r);
            if missing {
                self.warnings.push(PipeWarning {
                    message: format!("渲染基值缺失：{r} 不在 catalog（diff 退化为全量 added）"),
                    side_effect: None,
                });
            }
            self.elems.insert(r, el);
        }
        self.elems.get_mut(&r).unwrap()
    }

    /// 单属性写入 + diff 记录；`NAME` 归显式命名空间。
    fn apply_attr(&mut self, r: RefU64, key: &str, v: NamedAttrValue, m: &mut EditModify) {
        let el = self.ensure(r);
        let (target, added, modified) = if key == "NAME" {
            if let NamedAttrValue::StringType(s) = &v {
                el.name = s.clone();
            }
            (
                &mut el.explicit_attmap,
                &mut m.added_explicit_attrs,
                &mut m.modified_explicit_attrs,
            )
        } else {
            (&mut el.attmap, &mut m.added_attrs, &mut m.modified_attrs)
        };
        if let Some(old) = target.map.get(key) {
            modified.insert(key.to_string(), (old.clone(), v.clone()));
        } else {
            added.insert(key.to_string(), v.clone());
        }
        target.map.insert(key.to_string(), v);
    }
}

/// 渲染入口：批 → 有序落库行。`catalog` 只读（基值播种），不产生副作用。
pub fn render_batch(batch: &MutationBatch, cat: &dyn PipeCatalog) -> RenderedBatch {
    let mut out = RenderedBatch {
        name: batch.name.clone(),
        confirms: batch.requires_confirm.clone(),
        warnings: batch.warnings.clone(),
        ..Default::default()
    };
    let mut st = State {
        cat,
        elems: HashMap::new(),
        warnings: Vec::new(),
    };

    for op in &batch.ops {
        match op {
            MutationOp::Create {
                refno,
                noun,
                under,
                name,
                attrs,
            } => {
                let mut el = EditElement {
                    refno: *refno,
                    owner: *under,
                    noun: db1_hash(noun),
                    noun_name: noun.clone(),
                    name: name.clone().unwrap_or_default(),
                    ..Default::default()
                };
                for (k, v) in &attrs.map {
                    if k == "NAME" {
                        if let NamedAttrValue::StringType(s) = v {
                            el.name = s.clone();
                        }
                        el.explicit_attmap.map.insert(k.clone(), v.clone());
                    } else {
                        el.attmap.map.insert(k.clone(), v.clone());
                    }
                }
                if let Some(n) = name {
                    el.explicit_attmap
                        .map
                        .insert("NAME".into(), NamedAttrValue::StringType(n.clone()));
                }
                out.ops.push(EditOp::Add(el.clone()));
                st.elems.insert(*refno, el);
            }
            MutationOp::SetAttrs { element, attrs } => {
                st.ensure(*element);
                let mut m = EditModify::default();
                for (k, v) in &attrs.map {
                    st.apply_attr(*element, k, v.clone(), &mut m);
                }
                m.current = st.elems[element].clone();
                out.ops.push(EditOp::Modify(m));
            }
            MutationOp::Rename {
                element,
                name,
                cascade: _,
            } => {
                st.ensure(*element);
                let mut m = EditModify::default();
                st.apply_attr(
                    *element,
                    "NAME",
                    NamedAttrValue::StringType(name.clone()),
                    &mut m,
                );
                m.current = st.elems[element].clone();
                out.ops.push(EditOp::Modify(m));
            }
            MutationOp::Delete { element } => {
                st.elems.remove(element);
                out.ops.push(EditOp::Delete { refno: *element });
            }
            MutationOp::Include {
                element,
                into,
                order,
            } => {
                st.ensure(*into);
                let mut m = EditModify::default();
                let el = st.elems.get_mut(into).unwrap();
                let old_children = el.children.clone();
                let pos = order.unwrap_or(el.children.len()).min(el.children.len());
                el.children.insert(pos, *element);
                m.children_changed = Some((old_children, el.children.clone()));
                m.current = el.clone();
                out.ops.push(EditOp::Modify(m));
            }
            MutationOp::Remove { element } => {
                // 属主经 catalog 反查；批内新建的属主已在状态投影里。
                let owner = st
                    .elems
                    .get(element)
                    .and_then(|e| {
                        let o = e.owner;
                        if o == RefU64(0) { None } else { Some(o) }
                    })
                    .or_else(|| cat.owner_of(*element));
                match owner {
                    Some(owner) => {
                        st.ensure(owner);
                        let el = st.elems.get_mut(&owner).unwrap();
                        let old_children = el.children.clone();
                        el.children.retain(|c| c != element);
                        let mut m = EditModify::default();
                        m.children_changed = Some((old_children, el.children.clone()));
                        m.current = el.clone();
                        out.ops.push(EditOp::Modify(m));
                    }
                    None => {
                        st.warnings.push(PipeWarning {
                            message: format!("Remove {element}：属主反查失败，仅发删除成员提示行"),
                            side_effect: None,
                        });
                    }
                }
            }
        }
    }
    out.warnings.extend(st.warnings);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::MutationOp;
    use crate::context::BranchEnd;
    use crate::pipeline::{EndEdit, PipeSpec};
    use crate::testkit::{MockCatalog, MockSpec, ctx};
    use crate::{PipeModel, PipeModelService};

    #[test]
    fn create_pipe_renders_add_rows() {
        let mut cat = MockCatalog::new();
        let zone = RefU64(10);
        cat.zone(zone, "/ZONE1");
        let s = PipeModelService::new(cat).with_spec(MockSpec::default());
        let spec = PipeSpec {
            owner: zone,
            name: Some("/P-100".into()),
            spec: RefU64(5),
            bore_mm: 100.0,
            insulation: None,
            tracing: None,
            temperature: None,
            pressure: None,
            slope_ref: None,
        };
        let batch = s.create_pipe(&ctx(), &spec).unwrap();
        let out = render_batch(&batch, &s.catalog);
        assert_eq!(out.ops.len(), 2);
        let EditOp::Add(pipe) = &out.ops[0] else {
            panic!("首行应 Add")
        };
        assert_eq!(pipe.noun, db1_hash("PIPE"));
        assert_eq!(pipe.owner, zone);
        assert_eq!(pipe.name, "/P-100");
        // NAME 契约：只在显式命名空间。
        assert!(pipe.attmap.map.get("NAME").is_none());
        assert_eq!(
            pipe.explicit_attmap.map.get("NAME"),
            Some(&NamedAttrValue::StringType("/P-100".into()))
        );
        assert!(matches!(
            pipe.attmap.map.get("BORE"),
            Some(NamedAttrValue::F32Type(b)) if (*b - 100.0).abs() < 1e-6
        ));
        // 同批第二行：BRAN 挂到新 PIPE（批内引用）。
        let EditOp::Add(bran) = &out.ops[1] else {
            panic!("次行应 Add")
        };
        assert_eq!(bran.noun, db1_hash("BRAN"));
        assert_eq!(bran.owner, pipe.refno);
    }

    #[test]
    fn set_attrs_diff_added_vs_modified() {
        let mut cat = MockCatalog::new();
        let b = RefU64(20);
        cat.branch(b, "/P1/B1", RefU64(10), RefU64(5));
        let mut batch = MutationBatch::named("t");
        // HBOR 未存 → added；PSPE 已存 → modified。
        batch.ops.push(MutationOp::SetAttrs {
            element: b,
            attrs: NamedAttrMap {
                map: [
                    ("HBOR".into(), NamedAttrValue::F32Type(100.0)),
                    ("PSPE".into(), NamedAttrValue::RefU64Type(RefU64(9))),
                ]
                .into_iter()
                .collect(),
            },
        });
        let out = render_batch(&batch, &cat);
        let EditOp::Modify(m) = &out.ops[0] else {
            panic!("应 Modify")
        };
        assert!(m.added_attrs.contains_key("HBOR"));
        let (old, _) = &m.modified_attrs["PSPE"];
        assert_eq!(*old, NamedAttrValue::RefU64Type(RefU64(5)));
        assert_eq!(m.current.noun, db1_hash("BRAN"));
    }

    #[test]
    fn edit_branch_end_preserves_write_order() {
        let mut cat = MockCatalog::new();
        let b = RefU64(20);
        cat.branch(b, "/P1/B1", RefU64(10), RefU64(5));
        let s = PipeModelService::new(cat).with_spec(MockSpec::default());
        let edit = EndEdit {
            bore_mm: Some(150.0),
            position: Some([0.0, 0.0, 0.0]),
            direction: Some([1.0, 0.0, 0.0]),
            connection: None,
            free: true,
        };
        let batch = s
            .edit_branch_end(&ctx(), b, BranchEnd::Head, &edit)
            .unwrap();
        // 多个 SetAttrs → 多个 Modify，序与批序一致。
        let out = render_batch(&batch, &s.catalog);
        assert_eq!(out.ops.len(), batch.ops.len());
        assert!(out.ops.iter().all(|o| matches!(o, EditOp::Modify(_))));
    }

    #[test]
    fn include_modify_owner_children() {
        let mut cat = MockCatalog::new();
        let pipe = RefU64(10);
        let b1 = RefU64(20);
        cat.zone(RefU64(1), "/Z");
        cat.pipe(pipe, "/P1", RefU64(5), 100.0);
        cat.branch(b1, "/P1/B1", pipe, RefU64(5));
        let new_member = RefU64(99);
        let mut batch = MutationBatch::named("t");
        batch.ops.push(MutationOp::Include {
            element: new_member,
            into: b1,
            order: Some(0),
        });
        let out = render_batch(&batch, &cat);
        let EditOp::Modify(m) = &out.ops[0] else {
            panic!("应 Modify")
        };
        assert_eq!(m.current.refno, b1);
        assert_eq!(m.current.children, vec![new_member]);
    }

    #[test]
    fn rename_goes_to_explicit_name() {
        let mut cat = MockCatalog::new();
        let pipe = RefU64(10);
        cat.pipe(pipe, "/P1", RefU64(5), 100.0);
        let s = PipeModelService::new(cat).with_spec(MockSpec::default());
        let batch = s.rename_cascade(&ctx(), pipe, "/P2").unwrap();
        let out = render_batch(&batch, &s.catalog);
        let EditOp::Modify(m) = &out.ops[0] else {
            panic!("应 Modify")
        };
        assert!(
            m.added_explicit_attrs.contains_key("NAME")
                || m.modified_explicit_attrs.contains_key("NAME")
        );
        assert_eq!(m.current.name, "/P2");
        assert!(m.added_attrs.is_empty() && m.modified_attrs.is_empty());
    }

    #[test]
    fn delete_and_remove_render() {
        let mut cat = MockCatalog::new();
        let pipe = RefU64(10);
        let b1 = RefU64(20);
        cat.pipe(pipe, "/P1", RefU64(5), 100.0);
        cat.branch(b1, "/P1/B1", pipe, RefU64(5));
        let mut batch = MutationBatch::named("t");
        batch.ops.push(MutationOp::Remove { element: b1 });
        batch.ops.push(MutationOp::Delete { element: b1 });
        let out = render_batch(&batch, &cat);
        // Remove → Modify(pipe.children 移除 b1)；Delete → Delete 行。
        let EditOp::Modify(m) = &out.ops[0] else {
            panic!("首行应 Modify")
        };
        assert_eq!(m.current.refno, pipe);
        assert!(m.current.children.is_empty());
        assert!(matches!(out.ops[1], EditOp::Delete { refno } if refno == b1));
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn unknown_element_warns_but_renders() {
        let cat = MockCatalog::new();
        let mut batch = MutationBatch::named("t");
        batch.ops.push(MutationOp::SetAttrs {
            element: RefU64(77),
            attrs: NamedAttrMap {
                map: [("X".into(), NamedAttrValue::IntegerType(1))]
                    .into_iter()
                    .collect(),
            },
        });
        let out = render_batch(&batch, &cat);
        assert_eq!(out.ops.len(), 1);
        assert_eq!(out.warnings.len(), 1);
        let EditOp::Modify(m) = &out.ops[0] else {
            panic!()
        };
        assert!(m.added_attrs.contains_key("X"));
    }
}
