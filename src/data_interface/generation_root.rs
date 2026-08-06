//! Shared generation-root policy for automatic and manual incremental updates.
//!
//! A model-affecting element resolves to the nearest minimum delivery unit
//! (MDU) when one exists. Otherwise it uses the established significant-owner
//! granularity. SITE/ZONE/WORL are hierarchy containers only and are never
//! generation roots or fallback units.
//!
//! The MDU type set is project configuration, not a compile-time constant:
//! `delivery_unit_types` in `DbOption.toml` replaces
//! [`DEFAULT_DELIVERY_UNIT_TYPES`] outright, while `append_delivery_unit_types`
//! extends it.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use aios_core::RefnoEnum;
use serde::{Deserialize, Serialize};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

use crate::data_interface::model_impact::is_loop_container_noun;

/// Guard malformed owner cycles and unexpectedly deep hierarchies.
pub const MAX_ANCESTOR_DEPTH: usize = 32;

/// Default minimum delivery-unit types, used when the project does not
/// configure its own set. Projects may replace this list entirely via
/// `delivery_unit_types` or extend it via `append_delivery_unit_types`.
pub const DEFAULT_DELIVERY_UNIT_TYPES: &[&str] = &["BRAN", "HANG", "SUPPO", "EQUI"];

/// Hierarchy containers that must never become generation roots.
pub const COARSE_HIERARCHY_NOUNS: &[&str] = &["WORL", "WORLD", "SITE", "ZONE"];
/// Known component nouns that are never valid minimum delivery units.
pub const NON_DELIVERY_UNIT_NOUNS: &[&str] = &["FTUB"];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationRootKind {
    #[default]
    DeliveryUnit,
    Normal,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GenerationNode {
    pub owner: Option<RefnoEnum>,
    pub noun: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationRoot {
    pub root: RefnoEnum,
    pub noun: String,
    pub name: String,
    pub kind: GenerationRootKind,
}

pub fn is_coarse_hierarchy_noun(noun: &str) -> bool {
    let noun = noun.trim().to_ascii_uppercase();
    COARSE_HIERARCHY_NOUNS.contains(&noun.as_str())
}

pub fn is_delivery_unit_noun(noun: &str, unit_types: &[String]) -> bool {
    let noun = noun.trim().to_ascii_uppercase();
    if NON_DELIVERY_UNIT_NOUNS.contains(&noun.as_str()) {
        return false;
    }
    unit_types.iter().any(|candidate| candidate == &noun)
}

fn is_component_only_noun(noun: &str) -> bool {
    let noun = noun.trim().to_ascii_uppercase();
    NON_DELIVERY_UNIT_NOUNS.contains(&noun.as_str())
}

/// Append trimmed, upper-cased, de-duplicated nouns to `out`.
///
/// Hierarchy containers and known component-only nouns are dropped whatever
/// the configuration says. Rejected entries are returned so the caller can
/// report a misconfiguration instead of silently narrowing the set.
fn extend_unit_types<'a>(
    out: &mut Vec<String>,
    raw_types: impl IntoIterator<Item = &'a String>,
) -> Vec<String> {
    let mut rejected = Vec::new();
    for raw in raw_types {
        let noun = raw.trim().to_ascii_uppercase();
        if noun.is_empty() || out.iter().any(|candidate| candidate == &noun) {
            continue;
        }
        if is_coarse_hierarchy_noun(&noun) || NON_DELIVERY_UNIT_NOUNS.contains(&noun.as_str()) {
            rejected.push(noun);
            continue;
        }
        out.push(noun);
    }
    rejected
}

/// Defaults ∪ configured additions.
pub fn resolve_delivery_unit_types(appended: &[String]) -> Vec<String> {
    resolve_delivery_unit_types_from_config(None, appended)
}

/// Resolve the effective delivery-unit type set from project configuration.
///
/// `configured` (`delivery_unit_types`) replaces the defaults outright when
/// present — including an explicitly empty array, which means "no delivery
/// units, every change uses normal granularity". `appended`
/// (`append_delivery_unit_types`) only applies to the default set and is
/// ignored once `configured` is present.
pub fn resolve_delivery_unit_types_from_config(
    configured: Option<&[String]>,
    appended: &[String],
) -> Vec<String> {
    let mut out = Vec::new();
    let rejected = match configured {
        Some(configured) => extend_unit_types(&mut out, configured),
        None => {
            out.extend(
                DEFAULT_DELIVERY_UNIT_TYPES
                    .iter()
                    .map(|noun| noun.to_string()),
            );
            extend_unit_types(&mut out, appended)
        }
    };
    if !rejected.is_empty() {
        log::warn!("最小交付单元配置忽略了非交付类型 {:?}", rejected);
    }
    out
}

/// Effective delivery-unit types for the running project.
///
/// Cached: the underlying `DbOption.toml` is read once per process, matching
/// `aios_core::get_db_option()`.
pub fn configured_delivery_unit_types() -> Vec<String> {
    static INSTANCE: OnceLock<Vec<String>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            let ext = crate::options::get_db_option_ext();
            let types = resolve_delivery_unit_types_from_config(
                ext.delivery_unit_types.as_deref(),
                ext.append_delivery_unit_types.as_deref().unwrap_or(&[]),
            );
            println!("最小交付单元类型: {:?}", types);
            types
        })
        .clone()
}

fn collect_chain(
    start: RefnoEnum,
    mut lookup: impl FnMut(RefnoEnum) -> Option<GenerationNode>,
) -> Option<Vec<(RefnoEnum, GenerationNode)>> {
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut current = start;

    for _ in 0..MAX_ANCESTOR_DEPTH {
        if !seen.insert(current) {
            return None;
        }
        let Some(node) = lookup(current) else {
            break;
        };
        let owner = node.owner.filter(|owner| *owner != current);
        let stop = owner.is_none() || is_coarse_hierarchy_noun(&node.noun);
        chain.push((current, node));
        if stop {
            break;
        }
        current = owner.expect("checked above");
    }
    Some(chain)
}

fn delivery_root(
    chain: &[(RefnoEnum, GenerationNode)],
    unit_types: &[String],
) -> Option<GenerationRoot> {
    chain
        .iter()
        .find(|(_, node)| is_delivery_unit_noun(&node.noun, unit_types))
        .map(|(root, node)| GenerationRoot {
            root: *root,
            noun: node.noun.trim().to_ascii_uppercase(),
            name: node.name.clone(),
            kind: GenerationRootKind::DeliveryUnit,
        })
}

fn normal_root(root: RefnoEnum, node: &GenerationNode) -> GenerationRoot {
    GenerationRoot {
        root,
        noun: node.noun.trim().to_ascii_uppercase(),
        name: node.name.clone(),
        kind: GenerationRootKind::Normal,
    }
}

/// Resolve a changed element:
/// 1. nearest MDU self/ancestor;
/// 2. otherwise its significant owner, crossing loop containers;
/// 3. if the owner is SITE/ZONE/WORL (or missing), the element itself;
/// 4. hierarchy/loop containers themselves are never fallback roots.
pub fn resolve_element_generation_root(
    refno: RefnoEnum,
    unit_types: &[String],
    lookup: impl FnMut(RefnoEnum) -> Option<GenerationNode>,
) -> Option<GenerationRoot> {
    let chain = collect_chain(refno, lookup)?;
    if let Some(root) = delivery_root(&chain, unit_types) {
        return Some(root);
    }

    let (self_refno, self_node) = chain.first()?;
    if is_coarse_hierarchy_noun(&self_node.noun) {
        return None;
    }

    for (owner_refno, owner_node) in chain.iter().skip(1) {
        if is_loop_container_noun(&owner_node.noun) || is_component_only_noun(&owner_node.noun) {
            continue;
        }
        if is_coarse_hierarchy_noun(&owner_node.noun) {
            return (!is_loop_container_noun(&self_node.noun))
                .then(|| normal_root(*self_refno, self_node));
        }
        return Some(normal_root(*owner_refno, owner_node));
    }

    (!is_loop_container_noun(&self_node.noun)).then(|| normal_root(*self_refno, self_node))
}

/// Resolve an old/new OWNER reference. The owner itself is the normal
/// granularity candidate (after crossing loop containers), while an enclosing
/// MDU still takes precedence.
pub fn resolve_owner_generation_root(
    owner: RefnoEnum,
    unit_types: &[String],
    lookup: impl FnMut(RefnoEnum) -> Option<GenerationNode>,
) -> Option<GenerationRoot> {
    let chain = collect_chain(owner, lookup)?;
    if let Some(root) = delivery_root(&chain, unit_types) {
        return Some(root);
    }

    chain
        .iter()
        .find(|(_, node)| {
            !is_loop_container_noun(&node.noun) && !is_component_only_noun(&node.noun)
        })
        .and_then(|(root, node)| {
            (!is_coarse_hierarchy_noun(&node.noun)).then(|| normal_root(*root, node))
        })
}

async fn load_live_chain(start: RefnoEnum) -> anyhow::Result<Vec<(RefnoEnum, GenerationNode)>> {
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut current = start;

    for _ in 0..MAX_ANCESTOR_DEPTH {
        if !seen.insert(current) {
            anyhow::bail!("owner chain contains a cycle at {current}");
        }
        let Some(pe) = aios_core::get_pe(current).await? else {
            break;
        };
        let owner = (pe.owner.is_valid() && pe.owner != current).then_some(pe.owner);
        let stop = owner.is_none() || is_coarse_hierarchy_noun(&pe.noun);
        chain.push((
            current,
            GenerationNode {
                owner,
                noun: pe.noun,
                name: pe.name,
            },
        ));
        if stop {
            break;
        }
        current = owner.expect("checked above");
    }
    Ok(chain)
}

pub async fn resolve_live_element_generation_root(
    refno: RefnoEnum,
    unit_types: &[String],
) -> anyhow::Result<Option<GenerationRoot>> {
    let chain = load_live_chain(refno).await?;
    Ok(resolve_element_generation_root(
        refno,
        unit_types,
        |candidate| {
            chain
                .iter()
                .find(|(node_refno, _)| *node_refno == candidate)
                .map(|(_, node)| node.clone())
        },
    ))
}

/// 按**指定库**批量解析生成根，整批共用一份 owner 链缓存。
///
/// 暂存窗口里的锁范围解析必须走这条路而不是 [`resolve_live_element_generation_root`]：
/// 解析阶段的删除与修改都渲染成 `UPDATE pe:…`，而 `UPDATE` 命不中记录就是空操作，
/// 暂存库起点又是空的——这两类目标在窗口内查无此行，归属会静默解析成 `None`。
/// 窗口前的归属只有持久层说了算。
///
/// 兄弟节点共享祖先，缓存把一棵子树的链遍历从「每个节点一整条链」压回「每个祖先一次」。
pub async fn resolve_generation_roots_on(
    db: &Surreal<Any>,
    refnos: &[RefnoEnum],
    unit_types: &[String],
) -> anyhow::Result<Vec<GenerationRoot>> {
    let mut nodes: HashMap<RefnoEnum, Option<GenerationNode>> = HashMap::new();
    let mut roots = Vec::new();
    for &refno in refnos {
        load_chain_into(db, refno, &mut nodes).await?;
        let resolved = resolve_element_generation_root(refno, unit_types, |candidate| {
            nodes.get(&candidate).cloned().flatten()
        });
        if let Some(root) = resolved {
            roots.push(root);
        }
    }
    Ok(roots)
}

/// 把 `start` 到根的整条 owner 链读进 `nodes`；已缓存的祖先不再查库。
///
/// 终止条件与 [`collect_chain`] 逐条对齐（缺行 / 无 owner / 粗层级容器 / 深度上限），
/// 否则缓存里会缺掉纯函数要读的那一节。
async fn load_chain_into(
    db: &Surreal<Any>,
    start: RefnoEnum,
    nodes: &mut HashMap<RefnoEnum, Option<GenerationNode>>,
) -> anyhow::Result<()> {
    let mut current = start;
    let mut seen = HashSet::new();

    for _ in 0..MAX_ANCESTOR_DEPTH {
        if !seen.insert(current) {
            anyhow::bail!("owner chain contains a cycle at {current}");
        }
        let node = match nodes.get(&current) {
            Some(cached) => cached.clone(),
            None => {
                let node = aios_core::get_pe_on(db, current).await?.map(|pe| GenerationNode {
                    owner: (pe.owner.is_valid() && pe.owner != current).then_some(pe.owner),
                    noun: pe.noun,
                    name: pe.name,
                });
                nodes.insert(current, node.clone());
                node
            }
        };
        let Some(node) = node else { return Ok(()) };
        if is_coarse_hierarchy_noun(&node.noun) {
            return Ok(());
        }
        let Some(owner) = node.owner else {
            return Ok(());
        };
        current = owner;
    }
    Ok(())
}

pub async fn resolve_live_owner_generation_root(
    owner: RefnoEnum,
    unit_types: &[String],
) -> anyhow::Result<Option<GenerationRoot>> {
    let chain = load_live_chain(owner).await?;
    Ok(resolve_owner_generation_root(
        owner,
        unit_types,
        |candidate| {
            chain
                .iter()
                .find(|(node_refno, _)| *node_refno == candidate)
                .map(|(_, node)| node.clone())
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_core::pdms_types::RefU64;
    use std::collections::HashMap;

    fn r(id: u32) -> RefnoEnum {
        RefU64::from_two_nums(24381, id).into()
    }

    fn owned(types: &[&str]) -> Vec<String> {
        types.iter().map(|noun| noun.to_string()).collect()
    }

    #[test]
    fn absent_config_uses_defaults_plus_appended() {
        assert_eq!(
            resolve_delivery_unit_types_from_config(None, &[]),
            owned(&["BRAN", "HANG", "SUPPO", "EQUI"])
        );
        assert_eq!(
            resolve_delivery_unit_types_from_config(None, &owned(&[" pipe ", "BRAN", ""])),
            owned(&["BRAN", "HANG", "SUPPO", "EQUI", "PIPE"])
        );
    }

    #[test]
    fn configured_types_replace_defaults_and_ignore_appended() {
        assert_eq!(
            resolve_delivery_unit_types_from_config(
                Some(&owned(&["equi", " pipe ", "EQUI"])),
                &owned(&["HANG"])
            ),
            owned(&["EQUI", "PIPE"])
        );
    }

    #[test]
    fn explicitly_empty_config_disables_delivery_units() {
        assert!(resolve_delivery_unit_types_from_config(Some(&[]), &owned(&["PIPE"])).is_empty());
    }

    #[test]
    fn invalid_delivery_types_are_rejected_from_any_config() {
        assert!(!is_delivery_unit_noun("FTUB", &owned(&["FTUB"])));
        assert_eq!(
            resolve_delivery_unit_types_from_config(
                Some(&owned(&["ZONE", "SITE", "FTUB", "EQUI"])),
                &[]
            ),
            owned(&["EQUI"])
        );
        assert_eq!(
            resolve_delivery_unit_types_from_config(None, &owned(&["worl", "WORLD"])),
            owned(&["BRAN", "HANG", "SUPPO", "EQUI"])
        );
    }

    #[test]
    fn components_resolve_to_their_nearest_delivery_unit() {
        let nodes = HashMap::from([
            (
                r(3),
                GenerationNode {
                    owner: None,
                    noun: "ZONE".into(),
                    name: String::new(),
                },
            ),
            (
                r(5),
                GenerationNode {
                    owner: Some(r(3)),
                    noun: "BRAN".into(),
                    name: String::new(),
                },
            ),
            (
                r(6),
                GenerationNode {
                    owner: Some(r(5)),
                    noun: "FTUB".into(),
                    name: String::new(),
                },
            ),
            (
                r(7),
                GenerationNode {
                    owner: Some(r(6)),
                    noun: "TUBE".into(),
                    name: String::new(),
                },
            ),
            (
                r(8),
                GenerationNode {
                    owner: Some(r(3)),
                    noun: "EQUI".into(),
                    name: String::new(),
                },
            ),
            (
                r(9),
                GenerationNode {
                    owner: Some(r(8)),
                    noun: "NOZZ".into(),
                    name: String::new(),
                },
            ),
        ]);
        let units = resolve_delivery_unit_types(&[]);
        let resolve = |refno| {
            resolve_element_generation_root(refno, &units, |id| nodes.get(&id).cloned())
                .map(|root| (root.root, root.noun))
        };

        assert_eq!(resolve(r(6)), Some((r(5), "BRAN".into())));
        assert_eq!(resolve(r(7)), Some((r(5), "BRAN".into())));
        assert_eq!(resolve(r(8)), Some((r(8), "EQUI".into())));
        assert_eq!(resolve(r(9)), Some((r(8), "EQUI".into())));
        assert_eq!(resolve(r(3)), None);
    }

    #[test]
    fn ftub_is_never_a_normal_generation_root() {
        let nodes = HashMap::from([
            (
                r(5),
                GenerationNode {
                    owner: Some(r(3)),
                    noun: "BRAN".into(),
                    name: String::new(),
                },
            ),
            (
                r(6),
                GenerationNode {
                    owner: Some(r(5)),
                    noun: "FTUB".into(),
                    name: String::new(),
                },
            ),
            (
                r(7),
                GenerationNode {
                    owner: Some(r(6)),
                    noun: "TUBE".into(),
                    name: String::new(),
                },
            ),
        ]);

        let resolve = |refno| {
            resolve_element_generation_root(refno, &[], |id| nodes.get(&id).cloned())
                .map(|root| (root.root, root.noun, root.kind))
        };

        assert_eq!(
            resolve(r(7)),
            Some((r(5), "BRAN".into(), GenerationRootKind::Normal))
        );
        assert_eq!(
            resolve_owner_generation_root(r(6), &[], |id| nodes.get(&id).cloned())
                .map(|root| (root.root, root.noun, root.kind)),
            Some((r(5), "BRAN".into(), GenerationRootKind::Normal))
        );
    }

    #[test]
    fn structural_children_resolve_to_renderable_parent() {
        let nodes = HashMap::from([
            (
                r(0),
                GenerationNode {
                    owner: Some(r(2)),
                    noun: "PAVE".into(),
                    name: String::new(),
                },
            ),
            (
                r(1),
                GenerationNode {
                    owner: Some(r(2)),
                    noun: "VERT".into(),
                    name: String::new(),
                },
            ),
            (
                r(2),
                GenerationNode {
                    owner: Some(r(3)),
                    noun: "PLOO".into(),
                    name: String::new(),
                },
            ),
            (
                r(3),
                GenerationNode {
                    owner: Some(r(4)),
                    noun: "FLOOR".into(),
                    name: String::new(),
                },
            ),
            (
                r(4),
                GenerationNode {
                    owner: None,
                    noun: "CFLOOR".into(),
                    name: "/TEST-FLOOR".into(),
                },
            ),
            (
                r(5),
                GenerationNode {
                    owner: Some(r(6)),
                    noun: "WALL".into(),
                    name: String::new(),
                },
            ),
            (
                r(6),
                GenerationNode {
                    owner: None,
                    noun: "CWALL".into(),
                    name: "/TEST-WALL".into(),
                },
            ),
            (
                r(7),
                GenerationNode {
                    owner: Some(r(8)),
                    noun: "SPINE".into(),
                    name: String::new(),
                },
            ),
            (
                r(8),
                GenerationNode {
                    owner: Some(r(9)),
                    noun: "GENSEC".into(),
                    name: "/TEST-GENSEC".into(),
                },
            ),
            (
                r(9),
                GenerationNode {
                    owner: Some(r(10)),
                    noun: "FRMW".into(),
                    name: String::new(),
                },
            ),
            (
                r(10),
                GenerationNode {
                    owner: None,
                    noun: "SUPPO".into(),
                    name: "/TEST-SUPPO".into(),
                },
            ),
            (
                r(11),
                GenerationNode {
                    owner: Some(r(12)),
                    noun: "PLDATU".into(),
                    name: String::new(),
                },
            ),
            (
                r(12),
                GenerationNode {
                    owner: Some(r(5)),
                    noun: "JLDATU".into(),
                    name: String::new(),
                },
            ),
        ]);
        let units = resolve_delivery_unit_types(&[]);

        let pave_root =
            resolve_element_generation_root(r(0), &units, |id| nodes.get(&id).cloned()).unwrap();
        assert_eq!((pave_root.root, pave_root.noun.as_str()), (r(3), "FLOOR"));

        let vertex_root =
            resolve_element_generation_root(r(1), &units, |id| nodes.get(&id).cloned()).unwrap();
        assert_eq!(
            (vertex_root.root, vertex_root.noun.as_str()),
            (r(3), "FLOOR")
        );

        let floor_root =
            resolve_element_generation_root(r(3), &units, |id| nodes.get(&id).cloned()).unwrap();
        assert_eq!(
            (floor_root.root, floor_root.noun.as_str()),
            (r(4), "CFLOOR")
        );

        let wall_root =
            resolve_element_generation_root(r(5), &units, |id| nodes.get(&id).cloned()).unwrap();
        assert_eq!((wall_root.root, wall_root.noun.as_str()), (r(6), "CWALL"));

        let gensec_root =
            resolve_element_generation_root(r(8), &units, |id| nodes.get(&id).cloned()).unwrap();
        assert_eq!(
            (gensec_root.root, gensec_root.noun.as_str()),
            (r(10), "SUPPO")
        );

        let spine_root =
            resolve_element_generation_root(r(7), &units, |id| nodes.get(&id).cloned()).unwrap();
        assert_eq!(
            (spine_root.root, spine_root.noun.as_str()),
            (r(10), "SUPPO")
        );

        let datum_root =
            resolve_element_generation_root(r(11), &units, |id| nodes.get(&id).cloned()).unwrap();
        assert_eq!((datum_root.root, datum_root.noun.as_str()), (r(5), "WALL"));
    }
}
