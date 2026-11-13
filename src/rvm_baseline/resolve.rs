//! 快照组名 → 站点库身份，在 **compare 这一步**补齐。
//!
//! import 做不了这件事：它只读 RVM/ATT，而 RVM 里根本没有 refno，ATT 里也只有
//! **未命名元素**才带 `NAME:= =ref0/ref1`（命名元素的 NAME 就是名字）。`test_data/rvm/`
//! 五份基准里只有 `C-IY-1R330-B` 配了 `.att`，其余四份的成员身份全是空的——
//! 于是「参与判定 0」，对拍等于没跑。重导一次 E3D TTY 补 ATT 是一条路，但身份本来
//! 就在站点库里，连着库的 compare 自己就能解，不必每加一份基准都回 E3D 跑一趟。
//!
//! E3D 的组名只有两种形态，各对应一条解法：
//!
//! | 组名 | 解法 |
//! | --- | --- |
//! | `/C-IY-1R330-B`（命名元素） | 按 `pe.name` 反查 |
//! | `BEND 3 of BRANCH /x`（默认命名） | 先把 owner 解成 refno，再取它成员表里第 3 个 BEND |
//!
//! 默认命名可以套娃（`BRANCH 1 of PIPE 1 of ZONE /1RX07-LCT`），owner 递归解一层即可。
//!
//! 「第 n 个」的次序来自 `pe_owner` 的复合 id `[pe:{owner}, slot]`——`slot` 就是这个成员在
//! 属主成员表里的位置（`ele_ops::ElementRows::render` 按 `EleData.children` 的顺序编号）。
//! E3D 的默认命名数的是「同属主、同名词的第几个」，所以按 slot 排序后在同名词子序列里取第 n 个。
//!
//! **隐式管段是个例外**：`FTUBE n of BRANCH /x` 在 PDMS 里根本不是元素（管段是两个元件
//! 之间隐含出来的），站点库里没有它的 `pe` 行，生成侧落在 `tubi_relate` 上。这类解成
//! [`Resolved::Tube`]，由对拍侧改查 `tubi_relate`。1RX07-LCT 的 159 个成员里 109 个是 FTUB，
//! 不认它就等于认不出三分之二。

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context, Result};
use serde::Deserialize;
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

use super::identity::{full_noun_to_short, named_owner_from_desc, parse_default_name};

/// 一次查询里塞多少个 refno。SurrealQL 的长 id 列表解析起来慢，分批更稳
/// （与 [`super::compare`] 的 `QUERY_CHUNK` 同一个理由与数量级）。
const QUERY_CHUNK: usize = 200;

/// PDMS 里没有实体元素、只在两个元件之间隐含出来的管段。E3D 默认命名里叫 FTUBE，
/// 老口径的 TUBE / TUBING 一并收下。
const IMPLICIT_TUBE_NOUNS: &[&str] = &["FTUB", "TUBI", "TUBE"];

/// 一个组名解出来的身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// 站点库里的真实元素。
    Element(String),
    /// 隐式管段：这根管段属于哪条 BRANCH、是其中第几根（从 1 起）。
    Tube { branch: String, ordinal: usize },
}

/// 一次解析的来源，进快照的 `identity_source`，便于排查「这个 refno 是怎么来的」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedFrom {
    /// 命名元素，按 `pe.name` 反查。
    PeName,
    /// 默认命名，按「属主 + 名词 + 序号」在成员表里数出来。
    DefaultName,
    /// 隐式管段，落在 `tubi_relate` 上。
    ImplicitTube,
}

impl ResolvedFrom {
    pub fn as_str(self) -> &'static str {
        match self {
            ResolvedFrom::PeName => "pe_name",
            ResolvedFrom::DefaultName => "default_name",
            ResolvedFrom::ImplicitTube => "implicit_tube",
        }
    }
}

#[derive(Debug, Clone)]
struct PeElement {
    name: String,
    noun: String,
}

#[derive(Debug, Deserialize)]
struct OwnerEdgeRow {
    /// 成员（`pe_owner.in`）。
    refno: String,
    /// 复合 id `[pe:{owner}, slot]` 里的 slot；取不到时按 `usize::MAX` 排到最后。
    slot: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct PeRow {
    refno: String,
    name: Option<String>,
    noun: Option<String>,
}

/// 一棵子树的身份索引：名字表 + 按成员序排好的成员表。整棵树一次性读进内存，
/// 之后所有解析都不再碰库——一个组名一次查询的话，299 个成员就是 299 趟往返。
#[derive(Debug, Default)]
pub struct SubtreeIndex {
    elements: HashMap<String, PeElement>,
    /// 属主 refno → 按 slot 排好的成员 refno。
    children: HashMap<String, Vec<String>>,
    /// `pe.name` → refno。同名多份时只留第一个，并记进 [`Self::ambiguous_names`]。
    by_name: HashMap<String, String>,
    ambiguous_names: HashSet<String>,
}

impl SubtreeIndex {
    pub fn refnos(&self) -> impl Iterator<Item = &String> {
        self.elements.keys()
    }

    pub fn len(&self) -> usize {
        self.elements.len()
    }

    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    pub fn contains(&self, refno: &str) -> bool {
        self.elements.contains_key(refno)
    }

    pub fn children_of(&self, owner: &str) -> &[String] {
        self.children.get(owner).map(Vec::as_slice).unwrap_or(&[])
    }

    fn noun_of(&self, refno: &str) -> Option<&str> {
        self.elements.get(refno).map(|e| e.noun.as_str())
    }
}

/// 从根 refno 出发按 `pe_owner` 广搜整棵子树，顺带把成员序、名字与名词都读回来。
///
/// 广搜走 `pe:X<-pe_owner`（属主的入边）——**不要**写成 `WHERE out = pe:X`：
/// `unique_pe_owner` 的前缀是 `in`，谓词形式用不上索引，会整表扫
/// （`docs/2026-08-06_pe-owner-uniqueness-fix-audit.md`：778,925 行上 3.83 秒 vs 0.65 毫秒）。
pub async fn load_subtree(db: &Surreal<Any>, root_refno: &str) -> Result<SubtreeIndex> {
    let mut index = SubtreeIndex::default();
    let mut seen = HashSet::from([root_refno.to_string()]);
    let mut frontier = vec![root_refno.to_string()];

    while !frontier.is_empty() {
        let mut next = Vec::new();
        for owner in &frontier {
            let sql = format!(
                "SELECT type::string(in) AS refno, record::id(id)[1] AS slot \
                 FROM {}<-pe_owner;",
                pe_key(owner)
            );
            let mut response = db.query(sql).await.context("查询 PE 成员表失败")?;
            let rows: Vec<OwnerEdgeRow> = response.take(0).context("解析 PE 成员表失败")?;
            let mut ordered: BTreeMap<(i64, String), String> = BTreeMap::new();
            for row in rows {
                let child = refno_from_pe_key(&row.refno);
                ordered.insert((row.slot.unwrap_or(i64::MAX), child.clone()), child);
            }
            let ordered: Vec<String> = ordered.into_values().collect();
            for child in &ordered {
                if seen.insert(child.clone()) {
                    next.push(child.clone());
                }
            }
            index.children.insert(owner.clone(), ordered);
        }
        frontier = next;
    }

    let refnos: Vec<String> = seen.into_iter().collect();
    for chunk in refnos.chunks(QUERY_CHUNK) {
        let ids = chunk
            .iter()
            .map(|refno| pe_key(refno))
            .collect::<Vec<_>>()
            .join(", ");
        let sql =
            format!("SELECT type::string(id) AS refno, name, noun FROM [{ids}] WHERE id != NONE;");
        let mut response = db.query(sql).await.context("查询 PE 行失败")?;
        let rows: Vec<PeRow> = response.take(0).context("解析 PE 行失败")?;
        for row in rows {
            let refno = refno_from_pe_key(&row.refno);
            let name = row.name.unwrap_or_default().trim().to_string();
            let noun = row.noun.unwrap_or_default().trim().to_ascii_uppercase();
            if !name.is_empty() {
                // 同名元素在一个子树里不该出现；真出现了就两个都别信——
                // 挑一个等于抛硬币，报未解析至少看得见。
                if index.by_name.insert(name.clone(), refno.clone()).is_some() {
                    index.ambiguous_names.insert(name.clone());
                }
            }
            index.elements.insert(refno, PeElement { name, noun });
        }
    }

    Ok(index)
}

/// 按名字在整个 dbnum 里找一个元素。
///
/// **这是一次全表扫**（`pe` 上没有 name 索引），所以只用在「还不知道根 refno、
/// 连子树都枚举不了」这一处，一次对拍最多一次。子树装进 [`SubtreeIndex`] 之后
/// 所有解析都在内存里做。
pub async fn find_refno_by_name(
    db: &Surreal<Any>,
    dbnum: u32,
    name: &str,
) -> Result<Option<String>> {
    let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
    let sql = format!(
        "SELECT VALUE type::string(id) FROM pe \
         WHERE dbnum = {dbnum} AND name = \"{escaped}\" AND deleted != true;"
    );
    let mut response = db
        .query(sql)
        .await
        .with_context(|| format!("按名字查 pe 失败: {name}"))?;
    let ids: Vec<String> = response.take(0).context("解析按名字查 pe 的结果失败")?;
    // 同名多份时不挑：挑错一个，整棵子树都对错了对象。
    match ids.len() {
        1 => Ok(Some(refno_from_pe_key(&ids[0]))),
        _ => Ok(None),
    }
}

/// 把组名解成身份。命中的结果缓存起来——嵌套的 owner 描述会被重复问到
/// （一条 BRANCH 下每个成员都要解一遍它的 owner）。
#[derive(Debug)]
pub struct Resolver<'a> {
    index: &'a SubtreeIndex,
    cache: HashMap<String, Option<(Resolved, ResolvedFrom)>>,
}

impl<'a> Resolver<'a> {
    pub fn new(index: &'a SubtreeIndex) -> Self {
        Self {
            index,
            cache: HashMap::new(),
        }
    }

    pub fn resolve(&mut self, group_name: &str) -> Option<(Resolved, ResolvedFrom)> {
        if let Some(hit) = self.cache.get(group_name) {
            return hit.clone();
        }
        // 环形的 owner 描述解不动，先占位防止无限递归。
        self.cache.insert(group_name.to_string(), None);
        let resolved = self.resolve_uncached(group_name);
        self.cache.insert(group_name.to_string(), resolved.clone());
        resolved
    }

    fn resolve_uncached(&mut self, group_name: &str) -> Option<(Resolved, ResolvedFrom)> {
        let name = group_name.trim();
        if name.starts_with('/') && !name.contains(" of ") {
            if self.index.ambiguous_names.contains(name) {
                return None;
            }
            let refno = self.index.by_name.get(name)?.clone();
            return Some((Resolved::Element(refno), ResolvedFrom::PeName));
        }

        let default = parse_default_name(name)?;
        let owner = self.resolve_owner(&default.owner_desc)?;
        let noun = full_noun_to_short(&default.noun_full);

        if let Some(refno) = self.nth_child_of_noun(&owner, &default.noun_full, default.ordinal) {
            return Some((Resolved::Element(refno), ResolvedFrom::DefaultName));
        }

        // 成员表里数不出这一个，而它又是管段类名词 → 这是**隐式**管段：PDMS 里两个元件
        // 之间隐含出来的那一段，没有 `pe` 行，生成侧落在 `tubi_relate` 上。
        //
        // 顺序不能反过来（先按名词认管段、再查成员表）：`FTUB` 在不少项目里是**真元素**
        // ——AvevaMarineSample 的 1RX07-LCT 就有 109 个 FTUB 的 `pe` 行。先认名词会把它们
        // 全判成「生成侧缺了 109 根管段」，同时把那 109 个真元素报成「生成侧多出来的」，
        // 一个身份错配印成两笔账。
        if IMPLICIT_TUBE_NOUNS.contains(&noun.as_str()) {
            return Some((
                Resolved::Tube {
                    branch: owner,
                    ordinal: default.ordinal,
                },
                ResolvedFrom::ImplicitTube,
            ));
        }
        None
    }

    /// 属主成员表里第 `ordinal` 个该名词的成员。
    ///
    /// **同一个名词在站点库里有三种拼法，一次只认一种。** 2026-09-15 在 1112 上逐个量过：
    ///
    /// | RVM 全称 | `pe.noun` | 规律 |
    /// | --- | --- | --- |
    /// | `GWALL` / `CWALL` / `FIXING` / `STWALL` | 原样 | 本来就不超 6 字 |
    /// | `PLDATUM` / `JLDATUM` | `PLDATU` / `JLDATU` | **截到 6 字** |
    /// | `FTUBE` / `ELBOW` / `BRANCH` | `FTUB` / `ELBO` / `BRAN` | 字典短名词（4 字为主） |
    ///
    /// 只按 `full_noun_to_short`（4 字 + 例外表）数，前两行一个都对不上：1RS-WF03 的
    /// 118 个 `GWALL n of CWALL /…` 与 38 个 `PLDATUM 1 of JLDATUM /…` 就是这么全数
    /// 解不出来的。三种拼法**按序试**，哪一种在这个属主下有成员就用哪一种数完为止——
    /// 混在一起数会把序号数歪，所以是「三选一」不是「都要」。
    fn nth_child_of_noun(&self, owner: &str, noun_full: &str, ordinal: usize) -> Option<String> {
        let full = noun_full.to_ascii_uppercase();
        let six: String = full.chars().take(6).collect();
        let short = full_noun_to_short(noun_full);
        let children = self.index.children_of(owner);
        let mut tried: Vec<&str> = Vec::with_capacity(3);
        for spelling in [full.as_str(), six.as_str(), short.as_str()] {
            if tried.contains(&spelling) {
                continue;
            }
            tried.push(spelling);
            let mut matching = children
                .iter()
                .filter(|child| self.index.noun_of(child) == Some(spelling));
            if let Some(hit) = matching.nth(ordinal - 1) {
                return Some(hit.clone());
            }
            // 这个拼法一个都没有 → 换下一个拼法；有但数不到第 n 个 → 就是没有，别换拼法再数。
            if children
                .iter()
                .any(|child| self.index.noun_of(child) == Some(spelling))
            {
                return None;
            }
        }
        None
    }

    /// owner 描述要么是「单个名词 + 命名元素」（`BRANCH /x`），要么本身又是一层
    /// 默认命名（`PIPE 1 of ZONE /1RX07-LCT`）。前者剥掉名词前缀，后者整串再解一遍。
    fn resolve_owner(&mut self, desc: &str) -> Option<String> {
        let target = named_owner_from_desc(desc)
            .map(str::to_string)
            .unwrap_or_else(|| desc.to_string());
        match self.resolve(&target)? {
            (Resolved::Element(refno), _) => Some(refno),
            // 管段不能当属主：它连元素都不是。
            (Resolved::Tube { .. }, _) => None,
        }
    }
}

pub fn pe_key(refno: &str) -> String {
    format!("pe:{}", refno.replace('/', "_"))
}

/// `pe:24384_22404` / `pe:⟨24384_22404⟩` / `24384_22404` → `24384/22404`。
pub fn refno_from_pe_key(raw: &str) -> String {
    raw.trim()
        .trim_start_matches("pe:")
        .trim_matches(|c| c == '⟨' || c == '⟩')
        .replace('_', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(refno: &str, name: &str, noun: &str) -> (String, PeElement) {
        (
            refno.to_string(),
            PeElement {
                name: name.to_string(),
                noun: noun.to_string(),
            },
        )
    }

    /// 一条 BRANCH 下的成员表，成员序就是 `pe_owner` 的 slot 序。
    fn fixture() -> SubtreeIndex {
        let mut index = SubtreeIndex::default();
        for (refno, pe) in [
            element("8000/1", "/1RX07-LCT", "ZONE"),
            element("8000/2", "", "PIPE"),
            element("8000/3", "", "BRAN"),
            element("8000/4", "", "BEND"),
            element("8000/5", "", "BEND"),
            element("8000/6", "", "FLAN"),
            element("8000/7", "/NAMED-VALVE", "VALV"),
        ] {
            if !pe.name.is_empty() {
                index.by_name.insert(pe.name.clone(), refno.clone());
            }
            index.elements.insert(refno, pe);
        }
        index
            .children
            .insert("8000/1".into(), vec!["8000/2".into()]);
        index
            .children
            .insert("8000/2".into(), vec!["8000/3".into()]);
        index.children.insert(
            "8000/3".into(),
            vec![
                "8000/4".into(),
                "8000/6".into(),
                "8000/5".into(),
                "8000/7".into(),
            ],
        );
        index
    }

    /// 命名元素直接按 `pe.name` 反查。
    #[test]
    fn a_named_group_resolves_through_the_name_table() {
        let index = fixture();
        let mut resolver = Resolver::new(&index);
        assert_eq!(
            resolver.resolve("/1RX07-LCT"),
            Some((Resolved::Element("8000/1".into()), ResolvedFrom::PeName))
        );
        assert_eq!(resolver.resolve("/NOT-THERE"), None);
    }

    /// 默认命名数的是「同属主、同名词的第几个」，不是成员表里的第几个：
    /// 成员序是 BEND / FLAN / BEND / VALV，所以 `BEND 2` 是第三个成员。
    #[test]
    fn a_default_name_counts_within_its_noun_in_member_order() {
        let index = fixture();
        let mut resolver = Resolver::new(&index);
        assert_eq!(
            resolver.resolve("BEND 1 of BRANCH 1 of PIPE 1 of ZONE /1RX07-LCT"),
            Some((
                Resolved::Element("8000/4".into()),
                ResolvedFrom::DefaultName
            ))
        );
        assert_eq!(
            resolver.resolve("BEND 2 of BRANCH 1 of PIPE 1 of ZONE /1RX07-LCT"),
            Some((
                Resolved::Element("8000/5".into()),
                ResolvedFrom::DefaultName
            )),
            "同名词子序列里数第 2 个，中间隔着的 FLAN 不算"
        );
        assert_eq!(
            resolver.resolve("BEND 3 of BRANCH 1 of PIPE 1 of ZONE /1RX07-LCT"),
            None,
            "数不出第 3 个就是没有，不许退回随便挑一个"
        );
    }

    /// owner 描述本身是默认命名时要再剥一层（E3D 的 owner 链可以套好几层）。
    #[test]
    fn nested_owner_descriptions_peel_one_layer_at_a_time() {
        let index = fixture();
        let mut resolver = Resolver::new(&index);
        assert_eq!(
            resolver.resolve("PIPE 1 of ZONE /1RX07-LCT"),
            Some((
                Resolved::Element("8000/2".into()),
                ResolvedFrom::DefaultName
            ))
        );
        assert_eq!(
            resolver.resolve("BRANCH 1 of PIPE 1 of ZONE /1RX07-LCT"),
            Some((
                Resolved::Element("8000/3".into()),
                ResolvedFrom::DefaultName
            ))
        );
    }

    /// 管段类名词先当**真元素**查成员表，查不到才当隐式管段。
    ///
    /// 反过来会两头错：AvevaMarineSample 的 1RX07-LCT 有 109 个 FTUB 的真 `pe` 行，
    /// 先按名词认管段会把它们全判成「生成侧缺了 109 根管段」，同时把那 109 个真元素
    /// 报成「生成侧多出来的」——一个身份错配印成两笔账（2026-09-15 实测）。
    #[test]
    fn a_tube_noun_is_an_element_when_the_member_table_has_one() {
        let mut index = fixture();
        index.elements.insert(
            "8000/8".into(),
            PeElement {
                name: String::new(),
                noun: "FTUB".into(),
            },
        );
        index
            .children
            .get_mut("8000/3")
            .expect("BRAN 的成员表")
            .push("8000/8".into());

        let mut resolver = Resolver::new(&index);
        assert_eq!(
            resolver.resolve("FTUBE 1 of BRANCH 1 of PIPE 1 of ZONE /1RX07-LCT"),
            Some((
                Resolved::Element("8000/8".into()),
                ResolvedFrom::DefaultName
            )),
            "成员表里有这一个，就是真元素"
        );
        assert_eq!(
            resolver.resolve("FTUBE 2 of BRANCH 1 of PIPE 1 of ZONE /1RX07-LCT"),
            Some((
                Resolved::Tube {
                    branch: "8000/3".into(),
                    ordinal: 2,
                },
                ResolvedFrom::ImplicitTube
            )),
            "数不出第 2 个 FTUB，那它就是隐含出来的那一段"
        );
    }

    /// 站点库里一个管段元素都没有时，整批都按隐式管段解（老口径的 TUBE 一并收下）。
    #[test]
    fn implicit_tubes_resolve_to_a_branch_and_an_ordinal_not_an_element() {
        let index = fixture();
        let mut resolver = Resolver::new(&index);
        assert_eq!(
            resolver.resolve("FTUBE 3 of BRANCH 1 of PIPE 1 of ZONE /1RX07-LCT"),
            Some((
                Resolved::Tube {
                    branch: "8000/3".into(),
                    ordinal: 3,
                },
                ResolvedFrom::ImplicitTube
            ))
        );
        assert_eq!(
            resolver.resolve("TUBE 1 of BRANCH 1 of PIPE 1 of ZONE /1RX07-LCT"),
            Some((
                Resolved::Tube {
                    branch: "8000/3".into(),
                    ordinal: 1,
                },
                ResolvedFrom::ImplicitTube
            )),
            "老口径的 TUBE 一并收下"
        );
    }

    /// 三种拼法都认，一次只认一种：全称（`GWALL`）、截 6 字（`PLDATUM` → `PLDATU`）、
    /// 字典短名词（`FTUBE` → `FTUB`）。2026-09-15 在 1112 上逐个量过，三种都真实存在。
    #[test]
    fn a_noun_capped_at_six_characters_still_resolves() {
        let mut index = fixture();
        index.by_name.insert("/JL".into(), "1112/20".into());
        index.elements.insert(
            "1112/20".into(),
            PeElement {
                name: "/JL".into(),
                noun: "JLDATU".into(),
            },
        );
        index.elements.insert(
            "1112/21".into(),
            PeElement {
                name: String::new(),
                noun: "PLDATU".into(),
            },
        );
        index
            .children
            .insert("1112/20".into(), vec!["1112/21".into()]);

        let mut resolver = Resolver::new(&index);
        assert_eq!(
            resolver.resolve("PLDATUM 1 of JLDATUM /JL"),
            Some((
                Resolved::Element("1112/21".into()),
                ResolvedFrom::DefaultName
            )),
            "站点库把 PLDATUM 截成 6 字的 PLDATU，全称与 4 字短名词都对不上"
        );
    }

    /// 站点库里存的名词可能是**全称**（1112 的 CWALL 下实测是 GWALL / STWALL / WALL），
    /// 截成 4 字的 GWAL / STWA 一个都对不上——1RS-WF03 的 118 个 `GWALL n of CWALL /…`
    /// 就是这么全数解不出来的。三种拼法都认，但一次只认一种。
    #[test]
    fn a_noun_stored_in_full_resolves_just_like_a_shortened_one() {
        let mut index = fixture();
        for (n, refno) in [(1u64, "1112/10"), (2, "1112/11")] {
            index.elements.insert(
                refno.to_string(),
                PeElement {
                    name: String::new(),
                    noun: "GWALL".into(),
                },
            );
            let _ = n;
        }
        index.by_name.insert("/CW".into(), "1112/9".into());
        index.elements.insert(
            "1112/9".into(),
            PeElement {
                name: "/CW".into(),
                noun: "CWALL".into(),
            },
        );
        index
            .children
            .insert("1112/9".into(), vec!["1112/10".into(), "1112/11".into()]);

        let mut resolver = Resolver::new(&index);
        assert_eq!(
            resolver.resolve("GWALL 2 of CWALL /CW"),
            Some((
                Resolved::Element("1112/11".into()),
                ResolvedFrom::DefaultName
            )),
            "站点库存的是全称 GWALL，短名词 GWAL 对不上也得认出来"
        );
        assert_eq!(
            resolver.resolve("GWALL 3 of CWALL /CW"),
            None,
            "全称这一拼法只有两个，数不出第 3 个就是没有——不许换个拼法再数一遍"
        );
        // 短名词那一侧照旧（BEND 在夹具里存的就是 4 字）。
        assert_eq!(
            resolver.resolve("BEND 1 of BRANCH 1 of PIPE 1 of ZONE /1RX07-LCT"),
            Some((
                Resolved::Element("8000/4".into()),
                ResolvedFrom::DefaultName
            ))
        );
    }

    /// 同名元素两份时两个都不信：挑一个等于抛硬币，报未解析至少看得见。
    #[test]
    fn an_ambiguous_name_resolves_to_nothing() {
        let mut index = fixture();
        index.ambiguous_names.insert("/1RX07-LCT".to_string());
        let mut resolver = Resolver::new(&index);
        assert_eq!(resolver.resolve("/1RX07-LCT"), None);
        assert_eq!(
            resolver.resolve("PIPE 1 of ZONE /1RX07-LCT"),
            None,
            "属主解不出来，成员也就解不出来"
        );
    }

    /// 环形 owner 描述不能把解析器转死。
    #[test]
    fn a_self_referencing_owner_terminates() {
        let index = SubtreeIndex::default();
        let mut resolver = Resolver::new(&index);
        assert_eq!(resolver.resolve("BEND 1 of BEND 1 of BEND 1 of /x"), None);
    }

    /// 广搜必须走图形式 `pe:X<-pe_owner`，不许退回谓词。
    ///
    /// `unique_pe_owner` 的前缀是 `in`，`WHERE out = Y` 用不上它：
    /// `docs/2026-08-06_pe-owner-uniqueness-fix-audit.md` 实测 778,925 行的表上
    /// `Iterate Table` / 3.83 秒，走索引的形式 0.65 毫秒。广搜每层每个属主各发一次，
    /// 写错这一句就是一次对拍把整张边表扫上几十遍。
    #[test]
    fn the_subtree_walk_goes_through_the_indexed_edge_form() {
        let body = include_str!("resolve.rs")
            .split_once("pub async fn load_subtree(")
            .expect("广搜入口还在")
            .1
            .split_once("\npub async fn find_refno_by_name(")
            .expect("广搜的边界")
            .0;
        assert!(body.contains("<-pe_owner"), "必须走图形式");
        assert!(!body.contains("WHERE out"), "不许退回 out 谓词: {body}");
        assert_eq!(pe_key("24384/22404"), "pe:24384_22404");
    }

    #[test]
    fn pe_keys_round_trip_through_both_id_spellings() {
        assert_eq!(pe_key("24384/22404"), "pe:24384_22404");
        assert_eq!(refno_from_pe_key("pe:24384_22404"), "24384/22404");
        assert_eq!(refno_from_pe_key("pe:⟨24384_22404⟩"), "24384/22404");
        assert_eq!(refno_from_pe_key("24384_22404"), "24384/22404");
    }
}
