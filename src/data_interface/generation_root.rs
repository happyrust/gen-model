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
use std::sync::{Arc, OnceLock};

use aios_core::{RefU64, RefnoEnum};
use anyhow::Context;
use e3d_io::db_element::DbSet;
use e3d_io::refno::RefNo;
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

/// Core3D 自己的粒度判据，P0 从 live E3D 3.1 的 `DB_Noun::getField` 导出
/// （证据 `docs/evidence/2026-08-28-core-noun-granularity-export.md`）。
const CORE_NOUN_GRANULARITY_SNAPSHOT_JSON: &str =
    include_str!("../../tests/fixtures/core-noun-granularity-e3d31.json");

#[derive(Deserialize)]
struct GranularityFieldSnapshot {
    nouns: HashMap<String, bool>,
}

#[derive(Deserialize)]
struct GranularityFieldsSnapshot {
    significant: GranularityFieldSnapshot,
    primitive_a: GranularityFieldSnapshot,
    primitive_b: GranularityFieldSnapshot,
    toplevel: GranularityFieldSnapshot,
}

#[derive(Deserialize)]
struct GranularitySnapshot {
    fields: GranularityFieldsSnapshot,
}

/// 两位分开存，不合成（R0-2）：`0xA103E` 的搭档跨版本会换——2.10 配的是
/// `0xA18B8`，那个 id 在 3.1 里根本不存在。合成成一个 `primitive` 布尔，
/// 换版本重导时就看不出是哪一位变了。
#[derive(Debug, Default)]
struct CoreNounGranularity {
    significant: HashMap<String, bool>,
    primitive_a: HashMap<String, bool>,
    primitive_b: HashMap<String, bool>,
    /// 字典的 `toplevel`（661628）。**这是另一侧的粒度位，今天没有任何判定读它。**
    ///
    /// `significant` 是 DRAFT 局部更新（`PartialUpdateDesiMgr`）的块判据；
    /// 3D 绘制侧（`DES_DrawList` 的 TOPF 家族）用的是这一个，且**一次都不读
    /// `significant`**。两张表差得不小（211 / 127 / 交集 93），而我们的产物是 3D 模型
    /// 却在用前者选根——导进来是为了让这笔账问得出来，不是为了改判定。
    /// 取证 `docs/evidence/2026-09-04-ida-core3d-partial-update-recheck.md` §4.1，
    /// 对账 `docs/evidence/2026-09-04-noun-toplevel-reconciliation.md`。
    toplevel: HashMap<String, bool>,
}

/// 快照的覆盖面，供运维确认位表确实装进了这个进程。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreGranularityCoverage {
    /// 快照登记的 noun 总数。
    pub nouns: usize,
    /// `significant` 为真的 noun 数。
    pub significant: usize,
    /// `primitive_a ∨ primitive_b` 为真的 noun 数。
    pub primitive: usize,
    /// `toplevel` 为真的 noun 数。**不参与任何判定**，见 [`core_toplevel_bit`]。
    pub toplevel: usize,
}

/// 解析失败按「整张表都不认识」处理：每个 noun 都落到保守分支，判定链的行为
/// 与位表引入之前一字不差。快照本身由
/// `core_noun_granularity_snapshot_is_complete_and_self_consistent` 钉住，
/// 所以坏掉的 fixture 在 CI 里响，不在生产里悄悄改判。
fn core_noun_granularity() -> &'static CoreNounGranularity {
    static INSTANCE: OnceLock<CoreNounGranularity> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        match serde_json::from_str::<GranularitySnapshot>(CORE_NOUN_GRANULARITY_SNAPSHOT_JSON) {
            Ok(snapshot) => CoreNounGranularity {
                significant: snapshot.fields.significant.nouns,
                primitive_a: snapshot.fields.primitive_a.nouns,
                primitive_b: snapshot.fields.primitive_b.nouns,
                toplevel: snapshot.fields.toplevel.nouns,
            },
            Err(error) => {
                log::error!("core noun 粒度位表解析失败，全部 noun 按保守处理: {error}");
                CoreNounGranularity::default()
            }
        }
    })
}

fn normalized_noun(noun: &str) -> String {
    noun.trim().to_ascii_uppercase()
}

/// 快照里登记的 `significant` 原值；`None` 表示这个 noun 不在快照里。
///
/// 与 [`noun_is_significant`] 是两件事：这里的 `Some(false)` 是「core 说不显著」，
/// `None` 是「我们没导到它」。C0-2 要求这两者分开断言。
pub fn core_significant_bit(noun: &str) -> Option<bool> {
    core_noun_granularity()
        .significant
        .get(&normalized_noun(noun))
        .copied()
}

/// 快照里登记的 `toplevel`（661628）原值；`None` 表示这个 noun 不在快照里。
///
/// **只读查询，判定链一行不读它。** 它是 Core3D **3D 绘制侧**（`DES_DrawList` 的
/// TOPF 家族）的粒度位，与 `significant`（DRAFT 局部更新侧）是两套：211 / 127 /
/// 交集 93。这里没有配套的 `noun_is_toplevel()` 是有意的——加了那个名字，
/// 下一个人就会把它当判据用，而两套口径能不能换、怎么换，是
/// `docs/evidence/2026-09-04-noun-toplevel-reconciliation.md` 还没判完的事。
pub fn core_toplevel_bit(noun: &str) -> Option<bool> {
    core_noun_granularity()
        .toplevel
        .get(&normalized_noun(noun))
        .copied()
}

/// 快照里登记的两个 primitive 位原值 `(primitive_a, primitive_b)`。
pub fn core_primitive_bits(noun: &str) -> Option<(bool, bool)> {
    let table = core_noun_granularity();
    let noun = normalized_noun(noun);
    let a = table.primitive_a.get(&noun).copied()?;
    let b = table.primitive_b.get(&noun).copied()?;
    Some((a, b))
}

/// core 眼里这个 noun 是不是一个「块」。
///
/// 快照外的 noun 保守返回 `true`，与 `primary_list_hint` 同口径：多当一次生成根
/// 是多做，判错成「不显著」是漏画。
pub fn noun_is_significant(noun: &str) -> bool {
    core_significant_bit(noun).unwrap_or(true)
}

/// core 的 `IsPrimitive`：**两位取或**（证据 §4）。
///
/// 不能改读字典的 `primitive`——它只等于 `primitive_a`，会漏掉整个结构族
/// （`GENSEC` `SCTN` `WALL` `FLOOR` `PANE` … 27 个 noun，P1 §3.2）。
pub fn noun_is_primitive(noun: &str) -> bool {
    core_primitive_bits(noun).is_none_or(|(a, b)| a || b)
}

/// 位表的覆盖面（观测用，不参与判定）。
pub fn core_noun_granularity_coverage() -> CoreGranularityCoverage {
    let table = core_noun_granularity();
    CoreGranularityCoverage {
        nouns: table.significant.len(),
        significant: table.significant.values().filter(|bit| **bit).count(),
        primitive: table
            .primitive_a
            .iter()
            .filter(|(noun, a)| **a || table.primitive_b.get(*noun).copied().unwrap_or(false))
            .count(),
        toplevel: table.toplevel.values().filter(|bit| **bit).count(),
    }
}

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

/// T2b / Core3D R9：字典判 point 容器、core 位表却**明确**判显著的 noun，开关
/// `model_significant_over_point` 开着时自己就是生成根（E3D 3.1 快照里只有 `AIDTEX`，
/// `the_significant_over_point_override_only_moves_aidtex` 钉着「恰好一个」）。
///
/// 判据是 `significant_bit(noun) == Some(true)`，**不是** [`noun_is_significant`]：后者对快照外的
/// noun 保守取真，开关开着时会把一个没导到的 point noun 抬成根——「未知 noun 走名单兜底」
/// 这条边界（C1-3）就被改了。今天快照 unknown = 0，两种写法行为相同；换版本重导那天才分开。
/// `significant_bit` 作参数只为让 C1-3 能构造「快照外」那一态，生产调用方传 [`core_significant_bit`]。
fn significant_point_container_is_its_own_root(
    noun: &str,
    significant_bit: impl Fn(&str) -> Option<bool>,
) -> bool {
    crate::options::model_significant_over_point()
        && is_loop_container_noun(noun)
        && significant_bit(noun) == Some(true)
}

/// Resolve a changed element:
/// 1. nearest MDU self/ancestor;
/// 2. otherwise its significant owner, crossing loop containers;
/// 3. if the owner is SITE/ZONE/WORL (or missing), the element itself;
/// 4. hierarchy/loop containers themselves are never fallback roots — except a
///    core-significant point container behind `model_significant_over_point` (T2b).
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
    if significant_point_container_is_its_own_root(&self_node.noun, core_significant_bit) {
        return Some(normal_root(*self_refno, self_node));
    }
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
    Ok(
        resolve_generation_roots_with_targets_on(db, refnos, unit_types)
            .await?
            .into_iter()
            .map(|(_, root)| root)
            .collect(),
    )
}

/// 与 [`resolve_generation_roots_on`] 相同，但保留“输入元素 → 生成根”配对。
///
/// 房间面板补偿需要把多个缺失 PANE 合并到同一个生成根，同时在生成完成后逐块验证；
/// 只返回根会丢掉这层对应关系。
pub async fn resolve_generation_roots_with_targets_on(
    db: &Surreal<Any>,
    refnos: &[RefnoEnum],
    unit_types: &[String],
) -> anyhow::Result<Vec<(RefnoEnum, GenerationRoot)>> {
    let mut nodes: HashMap<RefnoEnum, Option<GenerationNode>> = HashMap::new();
    let mut roots = Vec::new();
    for &refno in refnos {
        load_chain_into(db, refno, &mut nodes).await?;
        let resolved = resolve_element_generation_root(refno, unit_types, |candidate| {
            nodes.get(&candidate).cloned().flatten()
        });
        if let Some(root) = resolved {
            roots.push((refno, root));
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
                let node = aios_core::get_pe_on(db, current)
                    .await?
                    .map(|pe| GenerationNode {
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

// ---------------------------------------------------------------------------
// 子树枚举：一个根（通常是 WORL）之下的全部生成根，读文件不读 `pe`（ADR-056 N7）。
// ---------------------------------------------------------------------------

/// 枚举器眼里的一个元素：noun、文件里存的 NAME、按**存储原序**的直接成员。
///
/// 取数适配各自负责把 direct attmap（`TYPE` / `NAME` / `members`）或 e3d-io
/// `DbElement`（`element_type` / `stored_name` / `member_refnos`）投影成这个形状；
/// 遍历本身只认这三格。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubtreeElement {
    pub noun: String,
    pub name: String,
    /// 成员原序是语义的一部分（BRAN 的成员序就是管路走向），不排序不去重。
    pub members: Vec<RefnoEnum>,
}

/// `root` 子树（含它自己）里的全部生成根，按存储成员序前序排列。
///
/// 口径与 direct 模式 `/model/ensure` 一直用的那段一致：交付单元优先，进了交付单元
/// 之下就不再出根；交付单元之外 Core3D 判显著的 noun 兜底成根，并继续向下走（显著
/// 节点名下的显著节点也是根）。层级容器（WORL / SITE / ZONE）永远不是根——位表里
/// 它们本来就不显著，这里再守一道是为了 `WORLD` 这类不在位表里的别名，以及位表
/// 加载失败时「未知即显著」的保守分支不会把容器变成根。
///
/// 这是 direct 按需选根（`DirectStore` 适配，`direct_tree.rs`）与增量规划器文件侧
/// 根枚举（[`enumerate_generation_roots`]，e3d-io `DbSet` 适配）**共用的唯一一段遍历**。
/// 每个元素恰好 lookup 一次；成员表重复项与成环只算一次。读不到元素整体报错，
/// 不静默漏根。
pub fn enumerate_generation_roots_in_subtree(
    root: RefnoEnum,
    unit_types: &[String],
    mut lookup: impl FnMut(RefnoEnum) -> anyhow::Result<SubtreeElement>,
) -> anyhow::Result<Vec<GenerationRoot>> {
    let mut out = Vec::new();
    let mut stack = vec![(root, false)];
    let mut seen = HashSet::new();
    while let Some((current, inside_delivery)) = stack.pop() {
        if !seen.insert(current) {
            continue;
        }
        let element = lookup(current)?;
        let noun = normalized_noun(&element.noun);
        // SAVEWORK creates a separate DBMETA → DBMDAT bookkeeping tree. These
        // are database records, not unknown design nouns: the conservative
        // significant-bit fallback must not turn every save into model work.
        // The recorded P/H/E windows exercise this through the real collector.
        if matches!(noun.as_str(), "DBMETA" | "DBMDAT") {
            continue;
        }
        let delivery = is_delivery_unit_noun(&noun, unit_types);
        if delivery
            || (!inside_delivery && !is_coarse_hierarchy_noun(&noun) && noun_is_significant(&noun))
        {
            out.push(GenerationRoot {
                root: current,
                noun: noun.clone(),
                name: element.name.trim().to_string(),
                kind: if delivery {
                    GenerationRootKind::DeliveryUnit
                } else {
                    GenerationRootKind::Normal
                },
            });
        }
        let next_inside = inside_delivery || delivery;
        stack.extend(
            element
                .members
                .into_iter()
                .rev()
                .map(|member| (member, next_inside)),
        );
    }
    Ok(out)
}

/// e3d-io `RefNo` → `RefnoEnum`（两个 u32 字拼成一个 u64）。
pub(crate) fn refno_from_e3d(refno: RefNo) -> RefnoEnum {
    RefnoEnum::from(RefU64::from_two_nums(refno.word0, refno.word1))
}

/// `RefnoEnum` → e3d-io `RefNo`；会话引用（`SesRef`）只取它的 refno 部分。
pub(crate) fn refno_to_e3d(refno: RefnoEnum) -> RefNo {
    let raw = refno.refno().0;
    RefNo::new((raw >> 32) as u32, raw as u32)
}

/// e3d-io 取数适配：noun 由记录 noun hash 反查，NAME 取文件原文（没存就空），成员按
/// 记录里的原序。库未注册、元素不存在都按错误上抛。
pub fn subtree_element_from_set(
    set: &Arc<DbSet>,
    refno: RefnoEnum,
) -> anyhow::Result<SubtreeElement> {
    let element = set.element(refno_to_e3d(refno));
    let noun = element
        .element_type()
        .with_context(|| format!("read TYPE of {refno} from e3d-io"))?;
    let name = element
        .stored_name()
        .with_context(|| format!("read NAME of {refno} from e3d-io"))?
        .unwrap_or_default();
    let members = element
        .member_refnos()
        .with_context(|| format!("read members of {refno} from e3d-io"))?
        .into_iter()
        .map(refno_from_e3d)
        .collect();
    Ok(SubtreeElement {
        noun,
        name,
        members,
    })
}

/// 给定若干顶层元素（通常是一个库的 WORL 根，即 `scan_index(...).roots`），在 `DbSet`
/// 钉住的会话上枚举它们之下的全部生成根。
///
/// 这就是 `fn::sync_gen_roots` 的文件侧替身（ADR-056 F10 / N7）：一行 SurrealDB 都不读，
/// 从未跑过数据增量、`pe` 零行的库也能得出根集。返回顺序 = `roots` 顺序 × 各自子树的
/// 存储成员序前序。
pub fn enumerate_generation_roots(
    set: &Arc<DbSet>,
    roots: &[RefNo],
    unit_types: &[String],
) -> anyhow::Result<Vec<GenerationRoot>> {
    let mut out = Vec::new();
    for &root in roots {
        out.extend(enumerate_generation_roots_in_subtree(
            refno_from_e3d(root),
            unit_types,
            |refno| subtree_element_from_set(set, refno),
        )?);
    }
    Ok(out)
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

    fn granularity_field(snapshot: &serde_json::Value, name: &str) -> serde_json::Value {
        snapshot["fields"][name].clone()
    }

    fn nouns_where_true(field: &serde_json::Value) -> HashSet<String> {
        field["nouns"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(_, value)| value.as_bool() == Some(true))
            .map(|(noun, _)| noun.clone())
            .collect()
    }

    /// Same regimen as `core_primary_list_snapshot_is_complete_and_self_consistent`
    /// in `model_impact.rs`: the counts have to add up, `unknown` may not leak into
    /// the resolved map, and the core.dll the numbers came from is pinned.
    #[test]
    fn core_noun_granularity_snapshot_is_complete_and_self_consistent() {
        let snapshot: serde_json::Value =
            serde_json::from_str(CORE_NOUN_GRANULARITY_SNAPSHOT_JSON).unwrap();
        assert_eq!(snapshot["schema"], 3);
        assert_eq!(
            snapshot["core_sha256"],
            "668783707a924c343759e99e8676fa8482d14987f4d83f10a83246001a7f5c18"
        );
        assert_eq!(snapshot["count"], 1931);

        // (field, id, fieldType, true_count). fieldType is core's own verdict via
        // `DB_Noun::fieldType`; 0 is the bool overload. It is recorded because
        // reading a field through the wrong overload is silent, not an error.
        // `toplevel` carries `None`: it came from the offline dictionary, not from
        // that live channel, and claiming a fieldType we never measured is the same
        // silent-wrong-overload hazard the column exists to catch.
        let expected = [
            ("significant", 90536458u64, Some(0), 127usize),
            ("primitive_a", 659518, Some(0), 347),
            ("primitive_b", 196958940, Some(0), 112),
            ("toplevel", 661628, None, 211),
        ];
        assert_eq!(
            snapshot["fields"].as_object().unwrap().len(),
            expected.len()
        );

        for (name, field_id, field_type, true_count) in expected {
            let field = granularity_field(&snapshot, name);
            assert_eq!(field["field_id"], field_id, "{name} field id");
            match field_type {
                Some(value) => assert_eq!(field["field_type"], value, "{name} field type"),
                None => assert!(field["field_type"].is_null(), "{name} field type"),
            }
            assert_eq!(field["resolved_count"], 1931, "{name} resolved");
            assert_eq!(field["unknown_count"], 0, "{name} unknown");
            assert_eq!(field["not_found_count"], 0, "{name} nouns not found");
            assert_eq!(field["true_count"], true_count, "{name} true count");
            assert_eq!(
                field["false_count"],
                1931 - true_count,
                "{name} false count"
            );
            assert!(
                field["non_binary_values"].as_array().unwrap().is_empty(),
                "{name} is a bool field and must not carry non-binary values"
            );

            let nouns = field["nouns"].as_object().unwrap();
            assert_eq!(nouns.len(), 1931, "{name} resolved map");
            assert_eq!(
                nouns
                    .values()
                    .filter(|value| value.as_bool() == Some(true))
                    .count(),
                true_count,
                "{name} true entries"
            );
            for row in field["unknown"].as_array().unwrap() {
                let noun = row["noun"].as_str().unwrap();
                assert!(!nouns.contains_key(noun), "{name}: unknown leaked: {noun}");
            }
        }

        // 渠道要分得开：三个位是 live `getField` 导的，`toplevel` 是离线字典解析的。
        // 混成一句「都是 core 说的」，下一次有人拿它当同等强度的证据用。
        assert_eq!(
            granularity_field(&snapshot, "toplevel")["provenance"],
            "offline-dict"
        );
        assert_eq!(
            snapshot["toplevel_source_sha256"],
            "c57f941f6562d82672788d93f9a080796ef3f48189a5ef2a7d00e07a83fa47fa"
        );
        for name in ["significant", "primitive_a", "primitive_b"] {
            assert!(
                granularity_field(&snapshot, name)["provenance"].is_null(),
                "{name} 是 live 通道导的，不该带 provenance 标记"
            );
        }
    }

    /// `IsPrimitive` reads two fields and ors them (evidence section 4). Neither
    /// bit contains the other, so collapsing the pair into one lookup would lose
    /// nouns - 27 of them on this snapshot.
    #[test]
    fn the_two_primitive_bits_are_not_redundant() {
        let snapshot: serde_json::Value =
            serde_json::from_str(CORE_NOUN_GRANULARITY_SNAPSHOT_JSON).unwrap();
        let primitive_a = nouns_where_true(&granularity_field(&snapshot, "primitive_a"));
        let primitive_b = nouns_where_true(&granularity_field(&snapshot, "primitive_b"));
        assert!(!primitive_b.is_subset(&primitive_a));
        assert_eq!(primitive_a.union(&primitive_b).count(), 374);
    }

    /// The reconciliation P1 exists to do, reduced to the one comparison we can
    /// already make: core calls three of our four default MDU types significant
    /// and does not call `SUPPO` significant. Pinned so that a snapshot refresh
    /// that changes the answer shows up as a failure rather than as a silent
    /// premise change under the parity plan.
    #[test]
    fn core_disagrees_with_our_default_delivery_units_on_suppo_alone() {
        let snapshot: serde_json::Value =
            serde_json::from_str(CORE_NOUN_GRANULARITY_SNAPSHOT_JSON).unwrap();
        let significant = nouns_where_true(&granularity_field(&snapshot, "significant"));
        let disagreements: Vec<&str> = DEFAULT_DELIVERY_UNIT_TYPES
            .iter()
            .copied()
            .filter(|noun| !significant.contains(*noun))
            .collect();
        assert_eq!(disagreements, vec!["SUPPO"]);

        // The other direction is the real scope question for P2: core has two
        // orders of magnitude more significant nouns than we have MDU types.
        assert_eq!(significant.len(), 127);
        for noun in COARSE_HIERARCHY_NOUNS {
            assert!(
                !significant.contains(*noun),
                "core would make the hierarchy container {noun} a generation root"
            );
        }
    }

    /// T2a：位表进生产。判定链还不读它，所以这里断言的全部是查询本身——
    /// 尤其是「登记为假」与「不在快照里」必须给出相反的答案（C0-2）。
    #[test]
    fn core_bits_are_queryable_and_unknown_nouns_stay_conservative() {
        assert_eq!(core_significant_bit("EQUI"), Some(true));
        assert!(noun_is_significant("EQUI"));

        // core 说不显著。这一条是 `SUPPO`——我们唯一与 core 不同判的 MDU。
        assert_eq!(core_significant_bit("SUPPO"), Some(false));
        assert!(!noun_is_significant("SUPPO"));

        // 快照外的 noun：没有原值，但保守当作显著。
        assert_eq!(core_significant_bit("FOOB"), None);
        assert!(noun_is_significant("FOOB"));
        assert_eq!(core_primitive_bits("FOOB"), None);
        assert!(noun_is_primitive("FOOB"));

        // 输入按 noun 字段原样来，可能带空白和小写。
        assert_eq!(core_significant_bit(" equi "), Some(true));
        assert_eq!(core_primitive_bits("nozz"), Some((true, false)));
    }

    /// `IsPrimitive` 取两位的或。改读字典的 `primitive`（= `primitive_a`）
    /// 会把整个结构族判成非图元——R12 上卷实现的那一刻就是真漏。
    #[test]
    fn is_primitive_ors_both_bits_and_keeps_them_separate() {
        assert_eq!(core_primitive_bits("GENSEC"), Some((false, true)));
        assert!(noun_is_primitive("GENSEC"));
        assert_eq!(core_primitive_bits("NOZZ"), Some((true, false)));
        assert!(noun_is_primitive("NOZZ"));
        assert_eq!(core_primitive_bits("ZONE"), Some((false, false)));
        assert!(!noun_is_primitive("ZONE"));

        let snapshot: serde_json::Value =
            serde_json::from_str(CORE_NOUN_GRANULARITY_SNAPSHOT_JSON).unwrap();
        let primitive_a = nouns_where_true(&granularity_field(&snapshot, "primitive_a"));
        let b_only: Vec<String> = nouns_where_true(&granularity_field(&snapshot, "primitive_b"))
            .into_iter()
            .filter(|noun| !primitive_a.contains(noun))
            .collect();
        assert_eq!(b_only.len(), 27);
        assert!(b_only.iter().all(|noun| noun_is_primitive(noun)));
    }

    /// 位表确实装进了这个进程——三个计数与快照自洽测试里的数字对得上。
    /// 解析失败的兜底会让这三个数变成 0，所以这条同时是那条兜底的守卫。
    #[test]
    fn the_granularity_snapshot_is_loaded_into_the_process() {
        assert_eq!(
            core_noun_granularity_coverage(),
            CoreGranularityCoverage {
                nouns: 1931,
                significant: 127,
                primitive: 374,
                toplevel: 211,
            }
        );
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

    /// U-H3：通风构件归到最近的 BRAN 交付单元；HVAC 容器自身像 PIPE 一样落自身（Normal）、
    /// 不是交付单元根，ZONE 永不成根。形状按 7997 `HVAC → BRAN 24381/100817 →
    /// {STRT, DAMP, TRNS, BATT, BEND}`——与 `tests/hvac_bran_increment.rs` 同源，这里钉 change-time 路。
    #[test]
    fn hvac_components_resolve_to_their_branch_and_hvac_is_never_a_delivery_unit() {
        let node = |owner: Option<u32>, noun: &str| GenerationNode {
            owner: owner.map(r),
            noun: noun.into(),
            name: String::new(),
        };
        let nodes = HashMap::from([
            (r(1), node(None, "ZONE")),
            (r(2), node(Some(1), "HVAC")),
            (r(3), node(Some(2), "BRAN")),
            (r(4), node(Some(3), "STRT")),
            (r(5), node(Some(3), "DAMP")),
            (r(6), node(Some(3), "TRNS")),
            (r(7), node(Some(3), "BATT")),
            (r(8), node(Some(3), "BEND")),
        ]);
        let units = resolve_delivery_unit_types(&[]);
        let resolve = |refno| {
            resolve_element_generation_root(refno, &units, |id| nodes.get(&id).cloned())
                .map(|root| (root.root, root.noun, root.kind))
        };
        for member in [r(4), r(5), r(6), r(7), r(8)] {
            assert_eq!(
                resolve(member),
                Some((r(3), "BRAN".into(), GenerationRootKind::DeliveryUnit)),
                "{member} must roll up to the BRAN delivery unit"
            );
        }
        // HVAC 自身：落自身、Normal——不是交付单元，也不进窗口根集（与 PIPE 同形）。
        assert!(!is_delivery_unit_noun("HVAC", &units));
        assert_eq!(
            resolve(r(2)),
            Some((r(2), "HVAC".into(), GenerationRootKind::Normal))
        );
        assert_eq!(resolve(r(1)), None, "ZONE 是层级容器，永不成根");
    }

    /// U-E2：设备子件归到最近的 EQUI 交付单元；EQUI 本身即 MDU（自成根）。SUBE 虽是 core
    /// 显著 noun，但它在 EQUI 之下——交付单元压过显著位，子件仍归 EQUI。形状按 8000
    /// `ZONE → EQUI → {BOX, CYLI, NOZZ, SUBE → BOX}`，与 `tests/equi_increment.rs` 同源。
    #[test]
    fn equipment_sub_elements_resolve_to_their_equipment_delivery_unit() {
        let node = |owner: Option<u32>, noun: &str| GenerationNode {
            owner: owner.map(r),
            noun: noun.into(),
            name: String::new(),
        };
        let nodes = HashMap::from([
            (r(1), node(None, "ZONE")),
            (r(2), node(Some(1), "EQUI")),
            (r(3), node(Some(2), "BOX")),
            (r(4), node(Some(2), "CYLI")),
            (r(5), node(Some(2), "NOZZ")),
            (r(6), node(Some(2), "SUBE")),
            (r(7), node(Some(6), "BOX")),
        ]);
        let units = resolve_delivery_unit_types(&[]);
        assert!(is_delivery_unit_noun("EQUI", &units), "EQUI 是默认 MDU");
        let resolve = |refno| {
            resolve_element_generation_root(refno, &units, |id| nodes.get(&id).cloned())
                .map(|root| (root.root, root.noun, root.kind))
        };
        for member in [r(3), r(4), r(5), r(6), r(7)] {
            assert_eq!(
                resolve(member),
                Some((r(2), "EQUI".into(), GenerationRootKind::DeliveryUnit)),
                "{member} must roll up to the EQUI delivery unit"
            );
        }
        assert_eq!(
            resolve(r(2)),
            Some((r(2), "EQUI".into(), GenerationRootKind::DeliveryUnit))
        );
        assert_eq!(resolve(r(1)), None, "ZONE 是层级容器，永不成根");
    }

    /// 电缆桥架 `ZONE → CWAY → CWBRAN → {CTMTRL → CTSTRA → CTRAY, RPATH → POINTR}`（U-T1）。
    ///
    /// core 位表里只有 `CWBRAN` 显著，`CTMTRL / CTSTRA / CTRAY / RPATH / POINTR` 都不显著，
    /// 而默认交付单元集又不含 `CWBRAN`：于是 change-time 路（[`resolve_element_generation_root`]，
    /// 显著属主 = 直接属主）把托盘的变化归到 `CTSTRA`，文件侧枚举
    /// （[`enumerate_generation_roots_in_subtree`]）却只出 `CWBRAN`——同一只托盘在两条路上
    /// 得到的根互不相交。路径侧不一样：`RPATH` 是点容器（noun 能力表 `point = true`），
    /// 归根时被跨过，`POINTR` / `RPATH` 直接落到 `CWBRAN`。把 `CWBRAN` 配进
    /// `append_delivery_unit_types` 之后两条路全部合上。
    /// 前半段钉的是现状，后半段是契约；改归根规则时两段要一起改。
    #[test]
    fn cable_tray_members_only_share_a_root_when_cwbran_is_a_delivery_unit() {
        let members: HashMap<RefnoEnum, (&str, Option<RefnoEnum>, Vec<RefnoEnum>)> =
            HashMap::from([
                (r(1), ("ZONE", None, vec![r(2)])),
                (r(2), ("CWAY", Some(r(1)), vec![r(3)])),
                (r(3), ("CWBRAN", Some(r(2)), vec![r(4), r(7)])),
                (r(4), ("CTMTRL", Some(r(3)), vec![r(5)])),
                (r(5), ("CTSTRA", Some(r(4)), vec![r(6)])),
                (r(6), ("CTRAY", Some(r(5)), vec![])),
                (r(7), ("RPATH", Some(r(3)), vec![r(8)])),
                (r(8), ("POINTR", Some(r(7)), vec![])),
            ]);
        assert!(
            is_loop_container_noun("RPATH"),
            "test premise: RPATH is a point container in the noun capability table"
        );
        for noun in ["CWAY", "CTMTRL", "CTSTRA", "CTRAY", "RPATH", "POINTR"] {
            assert_eq!(
                core_significant_bit(noun),
                Some(false),
                "test premise: {noun}"
            );
        }
        assert_eq!(core_significant_bit("CWBRAN"), Some(true));
        let node = |refno| {
            members.get(&refno).map(|(noun, owner, _)| GenerationNode {
                owner: *owner,
                noun: (*noun).into(),
                name: String::new(),
            })
        };
        let subtree = |refno| {
            members
                .get(&refno)
                .map(|(noun, _, members)| SubtreeElement {
                    noun: (*noun).into(),
                    name: String::new(),
                    members: members.clone(),
                })
                .ok_or_else(|| anyhow::anyhow!("no element {refno}"))
        };
        let resolve = |refno, units: &[String]| {
            resolve_element_generation_root(refno, units, node)
                .map(|root| (root.root, root.noun, root.kind))
        };
        let enumerate = |units: &[String]| {
            enumerate_generation_roots_in_subtree(r(1), units, subtree)
                .unwrap()
                .into_iter()
                .map(|root| (root.root, root.noun, root.kind))
                .collect::<Vec<_>>()
        };

        // 现状：默认交付单元集下，托盘归到直接属主，枚举只认 CWBRAN。
        let defaults = resolve_delivery_unit_types(&[]);
        assert!(!defaults.iter().any(|noun| noun == "CWBRAN"));
        assert_eq!(
            resolve(r(6), &defaults),
            Some((r(5), "CTSTRA".into(), GenerationRootKind::Normal))
        );
        assert_eq!(
            resolve(r(5), &defaults),
            Some((r(4), "CTMTRL".into(), GenerationRootKind::Normal))
        );
        // 路径侧跨过点容器 RPATH：POINTR 与 RPATH 自身都落到 CWBRAN（恰好在枚举根集里）。
        for member in [r(7), r(8)] {
            assert_eq!(
                resolve(member, &defaults),
                Some((r(3), "CWBRAN".into(), GenerationRootKind::Normal)),
                "{member} must cross the RPATH point container"
            );
        }
        assert_eq!(
            enumerate(&defaults),
            vec![(r(3), "CWBRAN".into(), GenerationRootKind::Normal)]
        );

        // 契约：CWBRAN 配成交付单元后，托盘 / 直段 / 材料容器 / 路径 / 路径点全部归到 CWBRAN，
        // 枚举也只出它。
        let with_cwbran = resolve_delivery_unit_types(&owned(&["CWBRAN"]));
        for member in [r(4), r(5), r(6), r(7), r(8)] {
            assert_eq!(
                resolve(member, &with_cwbran),
                Some((r(3), "CWBRAN".into(), GenerationRootKind::DeliveryUnit)),
                "{member} must roll up to the CWBRAN delivery unit"
            );
        }
        assert_eq!(
            enumerate(&with_cwbran),
            vec![(r(3), "CWBRAN".into(), GenerationRootKind::DeliveryUnit)]
        );
        // CWAY 与 ZONE 从头到尾都不是根集成员。
        assert_eq!(resolve(r(1), &with_cwbran), None);
        assert!(
            !enumerate(&with_cwbran)
                .iter()
                .any(|(root, _, _)| *root == r(2))
        );
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

    #[test]
    fn significant_over_point_only_promotes_aidtex_when_enabled() {
        let nodes = HashMap::from([
            (
                r(1),
                GenerationNode {
                    owner: Some(r(2)),
                    noun: "AIDTEX".into(),
                    name: "/AID-TEXT".into(),
                },
            ),
            (
                r(2),
                GenerationNode {
                    owner: Some(r(3)),
                    noun: "ZONE".into(),
                    name: "/ZONE".into(),
                },
            ),
            (
                r(3),
                GenerationNode {
                    owner: None,
                    noun: "WORL".into(),
                    name: "/*".into(),
                },
            ),
        ]);

        {
            let _override = crate::options::SignificantOverPointOverride::set(false);
            assert!(
                resolve_element_generation_root(r(1), &[], |id| nodes.get(&id).cloned()).is_none(),
                "the compatibility-off path keeps point containers out of the root set"
            );
        }
        {
            let _override = crate::options::SignificantOverPointOverride::set(true);
            let root = resolve_element_generation_root(r(1), &[], |id| nodes.get(&id).cloned())
                .expect("the significant bit promotes AIDTEX");
            assert_eq!((root.root, root.noun.as_str()), (r(1), "AIDTEX"));
        }
    }

    // ---- T2b 契约（用例集 C1-1 / C1-2 / C1-3 + 「恰好只差 AIDTEX」） ----

    /// `(refno, owner, noun)` 摆成一条 owner 链，省掉每条用例里的 `GenerationNode` 样板。
    fn owner_chain(nodes: &[(u32, Option<u32>, &str)]) -> HashMap<RefnoEnum, GenerationNode> {
        nodes
            .iter()
            .map(|(id, owner, noun)| {
                (
                    r(*id),
                    GenerationNode {
                        owner: owner.map(r),
                        noun: (*noun).into(),
                        name: format!("/{noun}-{id}"),
                    },
                )
            })
            .collect()
    }

    fn resolve_in(
        nodes: &HashMap<RefnoEnum, GenerationNode>,
        refno: RefnoEnum,
        unit_types: &[String],
    ) -> Option<(RefnoEnum, String, GenerationRootKind)> {
        resolve_element_generation_root(refno, unit_types, |id| nodes.get(&id).cloned())
            .map(|root| (root.root, root.noun, root.kind))
    }

    /// 同一形状在开关两态下各解一次。
    fn resolve_both_states(
        nodes: &HashMap<RefnoEnum, GenerationNode>,
        refno: RefnoEnum,
    ) -> (
        Option<(RefnoEnum, String, GenerationRootKind)>,
        Option<(RefnoEnum, String, GenerationRootKind)>,
    ) {
        let off = {
            let _off = crate::options::SignificantOverPointOverride::set(false);
            resolve_in(nodes, refno, &[])
        };
        let on = {
            let _on = crate::options::SignificantOverPointOverride::set(true);
            resolve_in(nodes, refno, &[])
        };
        (off, on)
    }

    /// C1-1（R9 / R14）：位表说 `AIDTEX` 显著，开关开着它就是自己的根——**含自身**；
    /// 一个 primitive 的根是最近的显著 owner，且那个 owner 在 core 位表里确实显著（位表当 oracle）。
    #[test]
    fn significant_bit_resolves_to_itself_and_to_nearest_significant_owner() {
        let _on = crate::options::SignificantOverPointOverride::set(true);

        let aid = owner_chain(&[
            (1, Some(2), "AIDTEX"),
            (2, Some(3), "ZONE"),
            (3, None, "WORL"),
        ]);
        assert_eq!(core_significant_bit("AIDTEX"), Some(true));
        assert_eq!(
            resolve_in(&aid, r(1), &[]),
            Some((r(1), "AIDTEX".into(), GenerationRootKind::Normal)),
            "a core-significant point container is its own root"
        );

        let equi = owner_chain(&[(9, Some(8), "BOX"), (8, Some(3), "EQUI"), (3, None, "ZONE")]);
        assert_eq!(core_significant_bit("BOX"), Some(false));
        assert_eq!(core_significant_bit("EQUI"), Some(true));
        assert_eq!(
            resolve_in(&equi, r(9), &[]),
            Some((r(8), "EQUI".into(), GenerationRootKind::Normal)),
            "a primitive rolls up to its nearest significant owner"
        );
        assert_eq!(
            resolve_in(&equi, r(8), &[]),
            Some((r(8), "EQUI".into(), GenerationRootKind::Normal)),
            "SignificantOwner includes the element itself (R14)"
        );
    }

    /// C1-2（R9）：MDU 是项目交付语义，压过位表——`AIDTEX` 位表显著、开关开着，挂在 EQUI（MDU）下
    /// 仍归到 EQUI 且 kind 是 DeliveryUnit；拿掉 MDU 配置，位表那一层才说话。
    #[test]
    fn delivery_unit_outranks_the_significant_bit() {
        let _on = crate::options::SignificantOverPointOverride::set(true);
        let nodes = owner_chain(&[
            (1, Some(8), "AIDTEX"),
            (9, Some(8), "NOZZ"),
            (8, Some(3), "EQUI"),
            (3, None, "ZONE"),
        ]);
        let units = owned(&["EQUI"]);
        assert_eq!(
            resolve_in(&nodes, r(1), &units),
            Some((r(8), "EQUI".into(), GenerationRootKind::DeliveryUnit))
        );
        assert_eq!(
            resolve_in(&nodes, r(9), &units),
            Some((r(8), "EQUI".into(), GenerationRootKind::DeliveryUnit))
        );
        assert_eq!(
            resolve_in(&nodes, r(1), &[]),
            Some((r(1), "AIDTEX".into(), GenerationRootKind::Normal)),
            "without an MDU the significant bit decides"
        );
    }

    /// C1-3（R9）：位表未知的 noun 走名单兜底，**开关开着也一样**。
    ///
    /// 「字典判 point、快照却没导到」这一态今天用真 noun 构造不出来（1931 个 noun unknown = 0；
    /// 名单兜底的 LOOP/PLOO/VERT/PAVE 与三个 *DATU 全在快照里），所以第二段直接钉 T2b 谓词：
    /// 位表答 `None` 时不抬根——这正是不能用保守取真的 `noun_is_significant` 的原因。
    #[test]
    fn unknown_noun_falls_back_to_the_hand_written_list() {
        assert_eq!(core_significant_bit("FOOB"), None);
        assert!(
            noun_is_significant("FOOB"),
            "the conservative query says true for unknown nouns — which is why T2b must not use it"
        );
        for nodes in [
            owner_chain(&[
                (1, Some(2), "FOOB"),
                (2, Some(3), "ZONE"),
                (3, None, "WORL"),
            ]),
            owner_chain(&[
                (1, Some(8), "FOOB"),
                (8, Some(3), "EQUI"),
                (3, None, "ZONE"),
            ]),
        ] {
            let (off, on) = resolve_both_states(&nodes, r(1));
            assert_eq!(
                off, on,
                "an unknown noun must resolve identically in both states"
            );
            assert!(off.is_some(), "the hand-written list still gives it a root");
        }

        let _on = crate::options::SignificantOverPointOverride::set(true);
        assert!(is_loop_container_noun("AIDTEX"));
        assert!(significant_point_container_is_its_own_root(
            "AIDTEX",
            core_significant_bit
        ));
        assert!(
            !significant_point_container_is_its_own_root("AIDTEX", |_| None),
            "outside the snapshot: never promoted, even with the switch on"
        );
        assert!(!significant_point_container_is_its_own_root(
            "AIDTEX",
            |_| Some(false)
        ));
        assert!(
            !significant_point_container_is_its_own_root("BOX", |_| Some(true)),
            "the override only touches point containers"
        );
    }

    /// R9 / T2b 的「恰好一个」：44 个字典 point noun 加三个 `*DATU` 逐个在两种形状下跑，
    /// 开关开与关的结果对称差**恰好 = {AIDTEX}**。换版本重导后位表若多翻转一个 noun，这条先红。
    #[test]
    fn the_significant_over_point_override_only_moves_aidtex() {
        let mut point_nouns = crate::noun_dict::default_noun_capabilities().point_nouns();
        assert_eq!(point_nouns.len(), 44, "dabacon point capability drifted");
        point_nouns.extend(["JLDATU", "PLDATU", "ENDATU"].map(String::from));

        let mut moved = std::collections::BTreeSet::new();
        for noun in &point_nouns {
            assert!(
                is_loop_container_noun(noun),
                "{noun} must classify as a point container"
            );
            for nodes in [
                owner_chain(&[(1, Some(2), noun), (2, Some(3), "ZONE"), (3, None, "WORL")]),
                owner_chain(&[(1, Some(8), noun), (8, Some(3), "EQUI"), (3, None, "ZONE")]),
            ] {
                let (off, on) = resolve_both_states(&nodes, r(1));
                if off != on {
                    moved.insert(noun.clone());
                }
            }
        }
        assert_eq!(
            moved,
            std::collections::BTreeSet::from(["AIDTEX".to_string()]),
            "the switch may move exactly one noun"
        );

        let significant_points: Vec<&String> = point_nouns
            .iter()
            .filter(|noun| core_significant_bit(noun) == Some(true))
            .collect();
        assert_eq!(
            significant_points,
            vec![&"AIDTEX".to_string()],
            "the bit table agrees: AIDTEX is the only core-significant point container"
        );
    }

    /// direct 文件窗口那一侧（`enumerate_generation_roots_in_subtree`）**不看**这个开关：
    /// 它按 `noun_is_significant` 枚举，`AIDTEX` 直接挂 ZONE 下本来就是恰好一个 Normal 根。
    /// P1 判出的「AIDTEX 零工作项」只发生在旧 PE 路径（`resolve_element_generation_root`）。
    #[test]
    fn direct_enumeration_already_lists_aidtex_under_zone_as_exactly_one_root() {
        let nodes = tree(&[
            (1, "WORL", "/*", &[2]),
            (2, "ZONE", "/Z", &[3, 4]),
            (3, "AIDTEX", "/AID-TEXT", &[]),
            (4, "PTSE", "/PTSE", &[]),
        ]);
        for on in [false, true] {
            let _guard = crate::options::SignificantOverPointOverride::set(on);
            let roots = enumerate_generation_roots_in_subtree(r(1), &[], lookup_in(&nodes))
                .expect("enumerate");
            assert_eq!(
                keys(&roots),
                vec![(r(3), "AIDTEX".to_string(), GenerationRootKind::Normal)],
                "switch={on}: AIDTEX is a root, the non-significant point container PTSE is not"
            );
        }
    }

    // ---- 子树枚举（P2-1 / N7：文件枚举根集，不经 `fn::sync_gen_roots`） ----

    /// `(refno, noun, name, members)` 摆成一棵内存树。`FOOB` 不在 core 位表里，按
    /// `core_bits_are_queryable_and_unknown_nouns_stay_conservative` 的口径保守判显著，
    /// 正好用来扮演「非交付单元的显著 noun」。
    fn tree(nodes: &[(u32, &str, &str, &[u32])]) -> HashMap<RefnoEnum, SubtreeElement> {
        nodes
            .iter()
            .map(|(id, noun, name, members)| {
                (
                    r(*id),
                    SubtreeElement {
                        noun: noun.to_string(),
                        name: name.to_string(),
                        members: members.iter().map(|m| r(*m)).collect(),
                    },
                )
            })
            .collect()
    }

    fn lookup_in(
        graph: &HashMap<RefnoEnum, SubtreeElement>,
    ) -> impl FnMut(RefnoEnum) -> anyhow::Result<SubtreeElement> + '_ {
        move |refno| {
            graph
                .get(&refno)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing {refno}"))
        }
    }

    fn keys(roots: &[GenerationRoot]) -> Vec<(RefnoEnum, String, GenerationRootKind)> {
        roots
            .iter()
            .map(|root| (root.root, root.noun.clone(), root.kind))
            .collect()
    }

    /// 与 `DirectTreeService::generation_roots_in_subtree` 同一口径：交付单元优先且其
    /// 子树不再出根；交付单元之外显著 noun 兜底，并继续向下走（显著节点的显著子节点
    /// 也是根）；容器不出根。结果按存储成员序前序排列。
    #[test]
    fn enumeration_prefers_delivery_units_and_falls_back_to_significant_nouns_outside_them() {
        let graph = tree(&[
            (1, "WORL", "", &[2]),
            (2, "SITE", "/SITE", &[3]),
            (3, "ZONE", "/ZONE", &[5, 8, 30]),
            (5, "BRAN", "/B1", &[6, 20]),
            (6, "FTUB", "", &[7]),
            (7, "TUBE", "", &[]),
            (20, "FOOB", "/INSIDE-BRAN", &[]),
            (8, "equi ", " /E1 ", &[9]),
            (9, "FOOB", "/INSIDE-EQUI", &[]),
            (30, "FOOB", "/OUTER", &[31]),
            (31, "FOOB", "/INNER", &[]),
        ]);
        assert_eq!(
            core_significant_bit("FOOB"),
            None,
            "test premise: FOOB is off the bit table"
        );

        let units = resolve_delivery_unit_types(&[]);
        let roots = enumerate_generation_roots_in_subtree(r(1), &units, lookup_in(&graph)).unwrap();
        assert_eq!(
            keys(&roots),
            vec![
                (r(5), "BRAN".into(), GenerationRootKind::DeliveryUnit),
                (r(8), "EQUI".into(), GenerationRootKind::DeliveryUnit),
                (r(30), "FOOB".into(), GenerationRootKind::Normal),
                (r(31), "FOOB".into(), GenerationRootKind::Normal),
            ]
        );
        // noun 归一化成大写、name 去掉两端空白——与 direct tree 的读法一字不差。
        assert_eq!(roots[1].name, "/E1");

        // 没有交付单元时同一棵树退到显著 noun：EQUI 仍是根（core 判显著），
        // 但它名下的 FOOB 现在也出根，因为不再有「交付单元之内」这层压制。
        let roots = enumerate_generation_roots_in_subtree(r(3), &[], lookup_in(&graph)).unwrap();
        assert_eq!(
            keys(&roots),
            vec![
                (r(5), "BRAN".into(), GenerationRootKind::Normal),
                (r(20), "FOOB".into(), GenerationRootKind::Normal),
                (r(8), "EQUI".into(), GenerationRootKind::Normal),
                (r(9), "FOOB".into(), GenerationRootKind::Normal),
                (r(30), "FOOB".into(), GenerationRootKind::Normal),
                (r(31), "FOOB".into(), GenerationRootKind::Normal),
            ]
        );
    }

    /// 起点本身也参与判定：从一个 BRAN 起枚举，答案就是它自己。
    #[test]
    fn enumeration_includes_the_requested_root_itself() {
        let graph = tree(&[(5, "BRAN", "/B1", &[6]), (6, "FOOB", "", &[])]);
        let roots = enumerate_generation_roots_in_subtree(
            r(5),
            &resolve_delivery_unit_types(&[]),
            lookup_in(&graph),
        )
        .unwrap();
        assert_eq!(
            keys(&roots),
            vec![(r(5), "BRAN".into(), GenerationRootKind::DeliveryUnit)]
        );
    }

    /// `WORLD` 是 `COARSE_HIERARCHY_NOUNS` 里的别名，不在 core 位表里——按「未知即显著」
    /// 它会被判成根。容器守卫必须压住它：位表缺项或整表加载失败时，SITE/ZONE/WORL
    /// 也不能变成生成根。
    #[test]
    fn enumeration_never_returns_hierarchy_containers() {
        assert_eq!(
            core_significant_bit("WORLD"),
            None,
            "test premise: WORLD is off the bit table"
        );
        assert!(
            noun_is_significant("WORLD"),
            "test premise: unknown nouns read as significant"
        );
        let graph = tree(&[
            (1, "WORLD", "", &[2]),
            (2, "SITE", "", &[3]),
            (3, "ZONE", "", &[8]),
            (8, "EQUI", "/E1", &[]),
        ]);
        let roots = enumerate_generation_roots_in_subtree(
            r(1),
            &resolve_delivery_unit_types(&[]),
            lookup_in(&graph),
        )
        .unwrap();
        assert_eq!(
            keys(&roots),
            vec![(r(8), "EQUI".into(), GenerationRootKind::DeliveryUnit)]
        );
    }

    /// 成员表里的重复项与 owner 成环都只算一次，且**不再查第二遍**——每个元素恰好一次
    /// lookup，这是文件枚举在几十万元素的库上能跑的前提。
    #[test]
    fn enumeration_visits_each_element_once_despite_duplicates_and_cycles() {
        let graph = tree(&[
            (3, "ZONE", "", &[8, 8, 30]),
            (8, "EQUI", "/E1", &[]),
            (30, "FOOB", "/LOOPY", &[3, 30]),
        ]);
        let mut calls = Vec::new();
        let mut lookup = lookup_in(&graph);
        let roots = enumerate_generation_roots_in_subtree(
            r(3),
            &resolve_delivery_unit_types(&[]),
            |refno| {
                calls.push(refno);
                lookup(refno)
            },
        )
        .unwrap();
        assert_eq!(
            keys(&roots),
            vec![
                (r(8), "EQUI".into(), GenerationRootKind::DeliveryUnit),
                (r(30), "FOOB".into(), GenerationRootKind::Normal),
            ]
        );
        assert_eq!(calls, vec![r(3), r(8), r(30)]);
    }

    /// 读不到一个元素不是「它不是根」，是这一次枚举整体失败——静默漏根就是漏画。
    #[test]
    fn enumeration_propagates_lookup_errors() {
        let graph = tree(&[(3, "ZONE", "", &[8]), (8, "EQUI", "/E1", &[])]);
        let error = enumerate_generation_roots_in_subtree(
            r(3),
            &resolve_delivery_unit_types(&[]),
            |refno| {
                if refno == r(8) {
                    anyhow::bail!("page read failed for {refno}")
                }
                lookup_in(&graph)(refno)
            },
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("page read failed"),
            "{error:#}"
        );
    }

    /// direct 模式 `/model/ensure` 的选根与增量的文件枚举必须是**同一段遍历**：
    /// `direct_tree.rs` 只准做 `DirectStore` 的取数适配，不准再养一份自己的 DFS。
    #[test]
    fn direct_tree_root_enumeration_is_the_shared_traversal() {
        let source = include_str!("direct_tree.rs");
        let body = source
            .split_once("pub fn generation_roots_in_subtree(")
            .expect("direct tree keeps its generation-root entry")
            .1
            .split_once("pub fn ancestors(")
            .expect("ancestors follows generation roots")
            .0;
        assert!(
            body.contains("generation_roots_in_subtree_on("),
            "the service method must delegate to the store-level adapter: {body}"
        );
        assert!(
            body.contains("resolve_element_generation_root("),
            "a direct leaf must resolve its generation root through the file-side owner chain: {body}"
        );
        let adapter = source
            .split_once("pub fn generation_roots_in_subtree_on(")
            .expect("store-level adapter exists")
            .1;
        assert!(
            adapter.contains("enumerate_generation_roots_in_subtree("),
            "the adapter must call the shared traversal: {adapter}"
        );
        assert!(
            !body.contains("inside_delivery") && !adapter.contains("inside_delivery"),
            "no second DFS allowed in direct_tree.rs"
        );
    }

    /// P2-1 对拍：同一 DESI 文件、同一会话，e3d-io `DbSet` 枚举出的生成根集必须与
    /// direct 模式 `/model/ensure` 走的 `DirectStore` 枚举逐条相同（refno / noun / name /
    /// kind 与前序）。两边共用同一段遍历，所以这里对的是两个取数适配——`element_type` /
    /// `stored_name` / `member_refnos` 对上 direct attmap 的 `TYPE` / `NAME` / `members`。
    ///
    /// 只跑单库：`AIOS_PROJAMS_GEOMETRY_FILE`（默认 ams8000_0001）+ `AIOS_E3D_TEMPLATE_DIR`。
    #[test]
    #[ignore = "manual live: needs a real E3D DESI file and the E3D template directory"]
    fn live_dbset_enumeration_matches_direct_store_enumeration() {
        use std::path::PathBuf;
        use std::sync::Arc;

        use e3d_io::db_element::{DbFilePin, DbSet, template_file_for};
        use e3d_io::engine::ReadOnlyEngine;

        use crate::data_interface::cata_closure::InMemoryCataLocator;
        use crate::data_interface::direct_store::{DbPin, DirectSchema, DirectStore};
        use crate::data_interface::direct_tree::generation_roots_in_subtree_on;
        use crate::fast_model::e3d_model_service::scan_index;

        let file = PathBuf::from(std::env::var("AIOS_PROJAMS_GEOMETRY_FILE").unwrap_or_else(
            |_| r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001".into(),
        ));
        let schema = Arc::new(DirectSchema::open_from_env().expect("E3D template directory"));
        let mut engine = ReadOnlyEngine::open(&file).expect("open DESI file");
        let sesno = engine.session().sesno;
        let dbnum = engine.descriptor().db_mark;
        let unit_types = resolve_delivery_unit_types(&[]);

        // direct 侧：单库 store，ref0 归属从同一文件的索引建，钉同一会话。
        let affiliations: HashMap<u32, u32> = engine
            .indexed_refnos()
            .expect("indexed refnos")
            .into_iter()
            .map(|(refno, _, _)| (refno.word0, dbnum))
            .collect();
        let locator = Arc::new(InMemoryCataLocator::from_parts(
            affiliations,
            HashMap::from([(
                dbnum,
                ("DESI".to_string(), "live".to_string(), file.clone()),
            )]),
        ));
        let store = DirectStore::new(schema.clone(), locator);
        store.pin(DbPin {
            dbnum: dbnum as i32,
            db_type: "DESI".into(),
            file: file.clone(),
            sesno: Some(sesno),
        });
        let mut worlds = store
            .indexes(dbnum as i32)
            .expect("direct indexes")
            .refnos_of_noun("WORL", Some(true));
        worlds.sort();
        let mut direct = Vec::new();
        for world in worlds {
            direct.extend(
                generation_roots_in_subtree_on(&store, world, &unit_types)
                    .expect("direct store enumeration"),
            );
        }

        // e3d 侧：`DbSet` 钉同一会话，WORL 根取自 `scan_index`（生产 `generate_dbnum` 同源）。
        let set = Arc::new(
            DbSet::with_attlib_file(schema.template_dir().join("attlib.dat")).expect("attlib"),
        );
        set.add_db(DbFilePin {
            file: file.clone(),
            template: template_file_for(schema.template_dir(), "DESI").expect("DESI template"),
            db_type: Some("DESI".into()),
            sesno: Some(sesno),
        })
        .expect("pin DESI in DbSet");
        let index = scan_index(&file, Some(sesno)).expect("scan index");
        let mut top = index.roots.clone();
        top.sort_by_key(|refno| (refno.word0, refno.word1));
        let e3d = enumerate_generation_roots(&set, &top, &unit_types).expect("DbSet enumeration");

        assert!(
            !direct.is_empty(),
            "direct side found no generation roots in {}",
            file.display()
        );
        let describe = |roots: &[GenerationRoot]| {
            roots
                .iter()
                .map(|root| {
                    format!(
                        "{} {} {:?} {}",
                        root.root.to_pdms_str(),
                        root.noun,
                        root.kind,
                        root.name
                    )
                })
                .collect::<Vec<_>>()
        };
        let (direct_rows, e3d_rows) = (describe(&direct), describe(&e3d));
        let only_direct: Vec<_> = direct_rows
            .iter()
            .filter(|row| !e3d_rows.contains(row))
            .collect();
        let only_e3d: Vec<_> = e3d_rows
            .iter()
            .filter(|row| !direct_rows.contains(row))
            .collect();
        assert!(
            only_direct.is_empty() && only_e3d.is_empty(),
            "root sets differ at sesno {sesno}: only_direct={only_direct:#?} only_e3d={only_e3d:#?}"
        );
        assert_eq!(
            direct_rows, e3d_rows,
            "pre-order differs although the sets agree"
        );
        println!(
            "parity ok: {} generation roots from {} WORL root(s) at sesno {sesno}",
            e3d.len(),
            top.len()
        );
    }
}
