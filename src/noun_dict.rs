//! dabacon noun 字典的离线快照：分类 flag（`noun_flags.json`）与完整能力矩阵（`noun_caps.json`）。
//!
//! 出处：`old-parse-pdms-db/src/dict.rs` 的**消费面**（`NounFlags` / `NounClassifier` /
//! `NounCapabilities` / `NounCapabilityTable` 与两个全局默认表），2026-09-04 随 parse_pdms_db
//! 退役搬进本仓（parse_pdms_db 退役清单 §B4）。类型、字段、方法与判定逐字保留——
//! `model_impact` 的更新契约、`fast_model` 的负体名单与覆盖审计都拿它当权威 flag 源，
//! 搬家不许改读数。
//!
//! 数据源与再生成：两份 JSON 由 `attlib.dat`（E3D Attribute Data File）解析导出，复刻的是
//! `core.dll` 的 `ATTOPE`/`ATGTIX`/`ATRDRC`/`ATNLOG` 读取链（ADR-004-db-noun-classifier-data、
//! `teach/learning-records/0003`、`0004`）。**解析器（`AttrDataFile`）没有搬**：本仓只消费快照，
//! 快照文件在 `resource/dict/`，与 `old-parse-pdms-db@88ac733` 仓根的两份逐字节相同。
//! 字典升级时重新导出——导出器今天仍在 legacy crate 里
//! （`cargo test -p parse_pdms_db --lib dict::tests::export_noun_flags_json -- --ignored`），
//! 它的长期去处（e3d-attlib 已经是 attlib.dat 的新解析器）是一条独立 ADR，不在本次搬家范围。
//!
//! ⚠️ dict 的 `primitive` 语义 = "设计级几何叶子 noun"（含**管件** ELBO/VALV/TUBI…），与本仓
//! 现有的**生成路由**名单（`GNERAL_PRIM_NOUN_NAMES` → prim_model）**不等价**——管件必须走
//! catalogue/piping 路径。故分类器是**权威 flag 源**，替换路由名单须逐个核对，不能盲替。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

/// `resource/dict/noun_flags.json`：全部 noun 的分类 flag。
const NOUN_FLAGS_JSON: &str = include_str!("../resource/dict/noun_flags.json");
/// `resource/dict/noun_caps.json`：全部 noun 的完整能力矩阵（紧凑 JSON）。
const NOUN_CAPS_JSON: &str = include_str!("../resource/dict/noun_caps.json");

/// 一个 noun 的分类 flag（`noun_flags.json` 的一行）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NounFlags {
    pub noun_hash: i32,
    pub noun_name: String,
    pub primitive: bool,
    pub geomset: bool,
    pub extrusion: bool,
    pub is_pointset_point: bool,
    pub graphics_behaviour: Option<i32>,
}

/// 一个 noun 的**完整**能力集：`DB_Noun` 里所有可从 dabacon 取到的访问器。
///
/// 字段名与 `DB_Noun` 的访问器一一对应（蛇形化）。`primaryList` 不在其中——它走
/// `db_get_element_info`，不在字典里。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NounCapabilities {
    pub noun_hash: i32,
    pub noun_name: String,

    // ── 直接几何能力 ────────────────────────────────────────────────────────
    pub primitive: bool,
    pub geomset: bool,
    pub extrusion: bool,
    /// 顶点/点容器（LOOP/PLOO/PAVE/VERT/SPINE…）——「不能当生成根」的权威名单来源。
    pub point: bool,
    pub is_pointset_point: bool,

    // ── 生成与变化路由 ──────────────────────────────────────────────────────
    pub graphics_behaviour: Option<i32>,
    /// 变化等价类目标 noun hash；0 表示自成一类。
    pub change_type: Option<i32>,
    /// 负体 → 正体等价类型 hash；0 表示不是负体。
    pub positive_equivalent: Option<i32>,
    pub spatial_map: Option<i32>,
    pub secondary_hierarchy: Option<i32>,
    /// `== 4` 即 `DB_Noun::isPseudo`。
    pub defined: Option<i32>,
    pub validc: Option<i32>,

    // ── 显示与交互 ──────────────────────────────────────────────────────────
    pub visible: bool,
    pub toplevel: bool,
    pub pickable: bool,
    pub world: bool,
    pub default_volume_query: bool,
    pub clasher_within: bool,
    pub clasher_section: bool,

    // ── 生命周期与权限 ──────────────────────────────────────────────────────
    pub modifiable: bool,
    pub status_eligible: bool,
    pub is_cloneable: bool,
    pub requires_marine: bool,
    pub is_protected: bool,
    pub delete_member_on_copy: bool,
    pub spooler_modifiable: bool,

    // ── pointset 遍历链 ─────────────────────────────────────────────────────
    pub ps_owner: Option<i32>,
    pub ps_next: Option<i32>,
    pub ps_first_member: Option<i32>,
}

impl NounCapabilities {
    /// 三个直接几何能力的并集——生成能力基线。
    pub fn has_direct_geometry(&self) -> bool {
        self.primitive || self.geomset || self.extrusion
    }

    /// `DB_Noun::isPseudo`：`defined == 4`。
    pub fn is_pseudo(&self) -> bool {
        self.defined == Some(4)
    }

    /// 该 noun 所属变化等价类的目标 hash：字典值为 0/缺失时按 core.dll `ReadData`
    /// 的回填规则取自身 hash。
    pub fn change_class_hash(&self) -> i32 {
        match self.change_type {
            Some(h) if h != 0 => h,
            _ => self.noun_hash,
        }
    }

    /// 负体的正体等价 hash（不是负体时为 `None`）。
    pub fn positive_equivalent_hash(&self) -> Option<i32> {
        self.positive_equivalent.filter(|h| *h != 0 && *h != -1)
    }
}

/// 全局默认能力矩阵：加载内嵌的 `noun_caps.json`。解析失败退化为空表。
pub fn default_noun_capabilities() -> &'static NounCapabilityTable {
    static DEFAULT_CAPS: OnceLock<NounCapabilityTable> = OnceLock::new();
    DEFAULT_CAPS
        .get_or_init(|| NounCapabilityTable::from_json_str(NOUN_CAPS_JSON).unwrap_or_default())
}

/// 全量能力矩阵的查询视图。
#[derive(Debug, Clone, Default)]
pub struct NounCapabilityTable {
    by_hash: HashMap<i32, NounCapabilities>,
    name_to_hash: HashMap<String, i32>,
}

impl NounCapabilityTable {
    pub fn from_caps(caps: Vec<NounCapabilities>) -> Self {
        let mut by_hash = HashMap::with_capacity(caps.len());
        let mut name_to_hash = HashMap::with_capacity(caps.len());
        for c in caps {
            let key = c.noun_name.trim().to_ascii_uppercase();
            if !key.is_empty() {
                name_to_hash.insert(key, c.noun_hash);
            }
            by_hash.insert(c.noun_hash, c);
        }
        Self {
            by_hash,
            name_to_hash,
        }
    }

    pub fn from_json_str(s: &str) -> Result<Self> {
        let caps: Vec<NounCapabilities> =
            serde_json::from_str(s).map_err(|e| anyhow!("parse noun_caps.json: {e}"))?;
        Ok(Self::from_caps(caps))
    }

    pub fn len(&self) -> usize {
        self.by_hash.len()
    }
    pub fn is_empty(&self) -> bool {
        self.by_hash.is_empty()
    }

    pub fn get(&self, noun: &str) -> Option<&NounCapabilities> {
        let h = *self.name_to_hash.get(&noun.trim().to_ascii_uppercase())?;
        self.by_hash.get(&h)
    }
    pub fn by_hash(&self, noun_hash: i32) -> Option<&NounCapabilities> {
        self.by_hash.get(&noun_hash)
    }

    pub fn iter(&self) -> impl Iterator<Item = &NounCapabilities> {
        self.by_hash.values()
    }

    /// 命中谓词的 noun 名（大写、升序、去重、跳过无名项）。
    pub fn nouns_where(&self, pred: impl Fn(&NounCapabilities) -> bool) -> Vec<String> {
        let mut v: Vec<String> = self
            .by_hash
            .values()
            .filter(|c| pred(c) && !c.noun_name.trim().is_empty())
            .map(|c| c.noun_name.trim().to_ascii_uppercase())
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// 顶点/点容器名单（`point == true`）。
    pub fn point_nouns(&self) -> Vec<String> {
        self.nouns_where(|c| c.point)
    }

    /// 直接几何能力名单（`primitive ∪ geomset ∪ extrusion`）。
    pub fn direct_geometry_nouns(&self) -> Vec<String> {
        self.nouns_where(|c| c.has_direct_geometry())
    }

    /// 伪类型名单（`defined == 4`）。
    pub fn pseudo_nouns(&self) -> Vec<String> {
        self.nouns_where(|c| c.is_pseudo())
    }

    /// 负体 → 正体名字映射（只含字典登记了 `positiveEquivalent` 的 noun）。
    pub fn positive_equivalents(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self
            .by_hash
            .values()
            .filter_map(|c| {
                let target = c.positive_equivalent_hash()?;
                let to = self.by_hash.get(&target)?;
                (!c.noun_name.trim().is_empty() && !to.noun_name.trim().is_empty()).then(|| {
                    (
                        c.noun_name.trim().to_ascii_uppercase(),
                        to.noun_name.trim().to_ascii_uppercase(),
                    )
                })
            })
            .collect();
        v.sort_unstable();
        v
    }

    /// 一个 noun 的变化等价类名（目标 noun 未登记在字典时用 `db1_dehash` 还原，
    /// 例如抽象类 `LINEAR`/`MULTC`/`INLINE`/`PCONN`）。
    pub fn change_class_name(&self, noun: &str) -> Option<String> {
        let c = self.get(noun)?;
        let h = c.change_class_hash();
        Some(match self.by_hash.get(&h) {
            Some(t) if !t.noun_name.trim().is_empty() => t.noun_name.trim().to_ascii_uppercase(),
            _ => dehash_noun(h).trim().to_ascii_uppercase(),
        })
    }

    /// 全部非平凡变化等价类：`类名 → 成员名单`（只含 `change_type` 非 0 的 noun）。
    pub fn change_classes(&self) -> BTreeMap<String, Vec<String>> {
        let mut out: BTreeMap<String, Vec<String>> = Default::default();
        for c in self.by_hash.values() {
            let Some(h) = c.change_type.filter(|h| *h != 0) else {
                continue;
            };
            if c.noun_name.trim().is_empty() {
                continue;
            }
            let class = match self.by_hash.get(&h) {
                Some(t) if !t.noun_name.trim().is_empty() => {
                    t.noun_name.trim().to_ascii_uppercase()
                }
                _ => dehash_noun(h).trim().to_ascii_uppercase(),
            };
            out.entry(class)
                .or_default()
                .push(c.noun_name.trim().to_ascii_uppercase());
        }
        for v in out.values_mut() {
            v.sort_unstable();
            v.dedup();
        }
        out
    }
}

// ── NounClassifier（ADR-004 阶段 3）──────────────────────────────────────────
//
// 以 core.dll dabacon 字典的 per-noun flag 为准的类型分类器，复刻 `DB_Noun` 的
// `primitive/geomset/extrusion/isPointsetPoint/graphicsBehaviour`(+`hashValue`/`findNoun`)。
// 数据来自 `noun_flags.json`。

/// 以 dict flag 为准的 noun 分类器。
#[derive(Debug, Clone, Default)]
pub struct NounClassifier {
    by_hash: HashMap<i32, NounFlags>,
    /// 归一化(大写去空)名 → noun_hash。
    name_to_hash: HashMap<String, i32>,
}

impl NounClassifier {
    /// 从一组 `NounFlags` 构建。
    pub fn from_flags(flags: Vec<NounFlags>) -> Self {
        let mut by_hash = HashMap::with_capacity(flags.len());
        let mut name_to_hash = HashMap::with_capacity(flags.len());
        for f in flags {
            let key = f.noun_name.trim().to_ascii_uppercase();
            if !key.is_empty() {
                name_to_hash.insert(key, f.noun_hash);
            }
            by_hash.insert(f.noun_hash, f);
        }
        Self {
            by_hash,
            name_to_hash,
        }
    }

    /// 从 `noun_flags.json` 文本构建。
    pub fn from_json_str(s: &str) -> Result<Self> {
        let flags: Vec<NounFlags> =
            serde_json::from_str(s).map_err(|e| anyhow!("parse noun_flags.json: {e}"))?;
        Ok(Self::from_flags(flags))
    }

    /// 从 `noun_flags.json` 路径构建。
    pub fn from_json_path(path: &Path) -> Result<Self> {
        let s = std::fs::read_to_string(path)
            .map_err(|e| anyhow!("read noun_flags.json {:?}: {e}", path))?;
        Self::from_json_str(&s)
    }

    pub fn len(&self) -> usize {
        self.by_hash.len()
    }
    pub fn is_empty(&self) -> bool {
        self.by_hash.is_empty()
    }

    /// db1 hash（复刻 `DB_Noun::hashValue`：noun 名 → hash）。
    pub fn hash_value(noun: &str) -> i32 {
        aios_core::tool::db_tool::db1_hash(noun.trim().to_ascii_uppercase().as_str()) as i32
    }

    /// 按 noun 名取 flag（大小写/空白不敏感）。
    pub fn flags(&self, noun: &str) -> Option<&NounFlags> {
        let h = *self.name_to_hash.get(&noun.trim().to_ascii_uppercase())?;
        self.by_hash.get(&h)
    }

    /// 复刻 `DB_Noun::findNoun`：按 noun hash 定位分类记录。
    pub fn find_noun(&self, noun_hash: i32) -> Option<&NounFlags> {
        self.by_hash.get(&noun_hash)
    }

    /// noun 是否存在于字典。
    pub fn contains(&self, noun: &str) -> bool {
        self.name_to_hash
            .contains_key(&noun.trim().to_ascii_uppercase())
    }

    // ── 逐 noun 分类谓词（未知 noun → false，保守）──────────────────────────
    pub fn primitive(&self, noun: &str) -> bool {
        self.flags(noun).map_or(false, |f| f.primitive)
    }
    pub fn geomset(&self, noun: &str) -> bool {
        self.flags(noun).map_or(false, |f| f.geomset)
    }
    pub fn extrusion(&self, noun: &str) -> bool {
        self.flags(noun).map_or(false, |f| f.extrusion)
    }
    pub fn is_pointset_point(&self, noun: &str) -> bool {
        self.flags(noun).map_or(false, |f| f.is_pointset_point)
    }
    pub fn graphics_behaviour(&self, noun: &str) -> Option<i32> {
        self.flags(noun).and_then(|f| f.graphics_behaviour)
    }

    // ── 集合访问器（返回命中某 flag 的 noun 名，升序）——供需要"名单"的消费点 ──
    fn nouns_where(&self, pred: impl Fn(&NounFlags) -> bool) -> Vec<String> {
        let mut v: Vec<String> = self
            .by_hash
            .values()
            .filter(|f| pred(f) && !f.noun_name.trim().is_empty())
            .map(|f| f.noun_name.trim().to_ascii_uppercase())
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }
    pub fn primitive_nouns(&self) -> Vec<String> {
        self.nouns_where(|f| f.primitive)
    }
    pub fn geomset_nouns(&self) -> Vec<String> {
        self.nouns_where(|f| f.geomset)
    }
    pub fn extrusion_nouns(&self) -> Vec<String> {
        self.nouns_where(|f| f.extrusion)
    }

    /// 负体候选名单（**启发式**：`N` 前缀 ∩ 几何(primitive/geomset/extrusion)）。
    ///
    /// ⚠️ dict 的 5 个已 RE flag 里**没有**显式 negative 字段（ADR-006 未决），故负体只能
    /// 启发式推断。实测：curated `TOTAL_NEG`(23) 是本集合的**干净子集**；本启发式相对
    /// `TOTAL_NEG` **多出** `NBXI/NPOLYH/NSLC/NTUB`(候选真负体) + **误纳** `NOZZ`(实为喷嘴正体)。
    /// ⇒ 生产用途**不要**直接用本集合当负体名单；本方法仅供交叉核对/发现用。
    pub fn negative_candidate_nouns(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .by_hash
            .values()
            .filter(|f| f.primitive || f.geomset || f.extrusion)
            .map(|f| f.noun_name.trim().to_ascii_uppercase())
            .filter(|n| !n.is_empty() && n.starts_with('N'))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// 全局默认 `NounClassifier`：加载内嵌的 `noun_flags.json`。首次调用懒加载；解析失败退化为
/// 空分类器（谓词全 false，保守不误判）。
pub fn default_noun_classifier() -> &'static NounClassifier {
    static DEFAULT_CLASSIFIER: OnceLock<NounClassifier> = OnceLock::new();
    DEFAULT_CLASSIFIER
        .get_or_init(|| NounClassifier::from_json_str(NOUN_FLAGS_JSON).unwrap_or_default())
}

/// gen-model 全部几何路由/覆盖名单的并集（近似「顶层任一路径能处理」的 noun 集）。
///
/// 缺口清单与运行期覆盖观测（`fast_model::coverage_audit`）共用本函数，避免两处口径漂移。
pub fn routing_coverage_nouns() -> &'static BTreeSet<String> {
    static COVERAGE: OnceLock<BTreeSet<String>> = OnceLock::new();
    COVERAGE.get_or_init(|| {
        use aios_core::pdms_types::{
            CATA_GEO_NAMES, CATA_WITHOUT_REUSE_GEO_NAMES, GENRAL_NEG_NOUN_NAMES,
            GENRAL_POS_NOUN_NAMES, GNERAL_LOOP_OWNER_NOUN_NAMES, GNERAL_PRIM_NOUN_NAMES,
            PIPING_NOUN_NAMES, POHE_GEO_NAMES, PRIMITIVE_NOUN_NAMES, TOTAL_CATA_GEO_NOUN_NAMES,
            TOTAL_CONTAIN_NGMR_GEO_NAEMS, TOTAL_GEO_NOUN_NAMES, TOTAL_LOOP_NOUN_NAMES,
            TOTAL_NEG_NOUN_NAMES, TOTAL_VERT_NOUN_NAMES, USE_CATE_NOUN_NAMES, VISBILE_GEO_NOUNS,
        };
        let mut coverage: BTreeSet<String> = BTreeSet::new();
        for list in [
            PRIMITIVE_NOUN_NAMES.as_slice(),
            GNERAL_PRIM_NOUN_NAMES.as_slice(),
            GNERAL_LOOP_OWNER_NOUN_NAMES.as_slice(),
            USE_CATE_NOUN_NAMES.as_slice(),
            PIPING_NOUN_NAMES.as_slice(),
            GENRAL_NEG_NOUN_NAMES.as_slice(),
            TOTAL_NEG_NOUN_NAMES.as_slice(),
            TOTAL_VERT_NOUN_NAMES.as_slice(),
            TOTAL_LOOP_NOUN_NAMES.as_slice(),
            GENRAL_POS_NOUN_NAMES.as_slice(),
            TOTAL_GEO_NOUN_NAMES.as_slice(),
            TOTAL_CATA_GEO_NOUN_NAMES.as_slice(),
            CATA_GEO_NAMES.as_slice(),
            CATA_WITHOUT_REUSE_GEO_NAMES.as_slice(),
            VISBILE_GEO_NOUNS.as_slice(),
            TOTAL_CONTAIN_NGMR_GEO_NAEMS.as_slice(),
            POHE_GEO_NAMES.as_slice(),
        ] {
            coverage.extend(list.iter().map(|s| s.trim().to_ascii_uppercase()));
        }
        coverage
    })
}

/// dict 认定为几何（`primitive ∪ geomset ∪ extrusion`）却不在任何路由/覆盖名单里的 noun。
///
/// ⚠️ 这是**上界**，不是 bug 列表：gen-model 是层级式生成，管件 / 吊架件 / 暖通 / 桥架等
/// 子孙 noun 在生成根子树深度展开时经 catalogue 渲染，本就不需要进任何顶层名单。真实缺口
/// 只能由运行期观测（`fast_model::coverage_audit`）收敛。
pub fn uncovered_geometry_nouns(clf: &NounClassifier) -> BTreeSet<String> {
    let coverage = routing_coverage_nouns();
    clf.primitive_nouns()
        .into_iter()
        .chain(clf.geomset_nouns())
        .chain(clf.extrusion_nouns())
        .filter(|noun| !coverage.contains(noun))
        .collect()
}

fn dehash_noun(noun_hash: i32) -> String {
    aios_core::tool::db_tool::db1_dehash(noun_hash as u32).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两份内嵌快照都得解出来——退化成空表的话谓词全 false，模型影响契约会静默放行。
    #[test]
    fn embedded_snapshots_parse_and_are_not_empty() {
        let clf = default_noun_classifier();
        let caps = default_noun_capabilities();
        assert!(clf.len() > 1000, "noun_flags.json 只解出 {} 条", clf.len());
        assert!(caps.len() > 1000, "noun_caps.json 只解出 {} 条", caps.len());
        // 两份快照来自同一份 attlib.dat，noun 集合应当一致。
        assert_eq!(clf.len(), caps.len());
    }

    /// 名字 → hash 的复刻与快照里记的 hash 一致（否则按 hash 查与按名查会对不上）。
    #[test]
    fn hash_value_matches_the_snapshot_hashes() {
        let clf = default_noun_classifier();
        for noun in ["BOX", "CYLI", "EQUI", "PIPE", "WORL", "SITE"] {
            let flags = clf
                .flags(noun)
                .unwrap_or_else(|| panic!("{noun} 不在字典里"));
            assert_eq!(NounClassifier::hash_value(noun), flags.noun_hash, "{noun}");
            assert!(clf.find_noun(flags.noun_hash).is_some());
        }
    }

    /// 设计图元是 primitive、层级容器不是——快照搬家后读数不变的最小钉。
    #[test]
    fn design_primitives_and_containers_classify_like_core_dll() {
        let clf = default_noun_classifier();
        for prim in ["BOX", "CYLI", "CONE", "SNOU", "DISH", "PYRA"] {
            assert!(clf.primitive(prim), "{prim} 应 primitive=true");
        }
        for container in ["WORL", "SITE", "ZONE", "EQUI"] {
            assert!(!clf.primitive(container), "{container} 应 primitive=false");
        }
        assert!(!clf.primitive("NOT_A_NOUN"));
    }

    /// 能力矩阵与 flag 表对同一 noun 的三个几何 flag 说法一致。
    #[test]
    fn capability_table_agrees_with_the_flag_table() {
        let clf = default_noun_classifier();
        let caps = default_noun_capabilities();
        for c in caps.iter() {
            let Some(f) = clf.find_noun(c.noun_hash) else {
                panic!("{} 只在 noun_caps.json 里", c.noun_name);
            };
            assert_eq!(
                (f.primitive, f.geomset, f.extrusion),
                (c.primitive, c.geomset, c.extrusion),
                "{}",
                c.noun_name
            );
        }
    }

    /// 未覆盖名单是几何名单的子集，且不含任何已在路由/覆盖名单里的 noun。
    #[test]
    fn uncovered_nouns_are_geometry_outside_the_routing_coverage() {
        let clf = default_noun_classifier();
        let coverage = routing_coverage_nouns();
        let uncovered = uncovered_geometry_nouns(clf);
        for noun in &uncovered {
            assert!(
                clf.primitive(noun) || clf.geomset(noun) || clf.extrusion(noun),
                "{noun} 不是几何 noun"
            );
            assert!(!coverage.contains(noun), "{noun} 已在覆盖名单里");
        }
        assert!(coverage.contains("BOX"));
        assert!(!uncovered.contains("BOX"));
    }
}
