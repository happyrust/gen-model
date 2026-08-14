//! 会话索引差分：给定一个 dabacon 库文件与 sesno 窗口，**只靠文件本身**判定窗口内的
//! 净增删改——不逐会话解析记录，不查询任何数据库。
//!
//! 依据的文件事实（与 `PdmsIO` 的 B+ 树搜索、会话认领扫描同源）：
//!
//! * 每个会话页携带**当时的索引根**（`SessionPageData::index_root_pageno`）。索引是
//!   copy-on-write B+ 树：页一经写入不再改动，记录改过必然换页，没动的子树在新旧
//!   两棵树上是同一个页号。
//! * 非叶层条目的 `pgno` 是子索引页号（条目 refno 为该子树最大键）；叶层条目的
//!   `(pgno, offset)` 是元素记录的物理位置。首条 `0x80000001_0x80000001` 哨兵在
//!   非叶层是**最左子树指针**（必须跟进，`btree_search_optimized_recursive` 的
//!   「起始标记分支」），在叶层不是数据（跳过并计数）。
//! * 分页是追加式的：页号 ≤ base 会话 `end_pgno` 的页在 base 时刻已经存在。目标树
//!   下降时凡指向这类页的分支即为共享子树——`filter_index_data` 的认领扫描用的正是
//!   同一判据（`pgno > last_end_pgno`）。
//!
//! 差分三态是**净结果**：仅目标树有 → `Added`；仅 base 树有 → `Deleted`；两边都有
//! 但记录位置不同 → `Modified`；位置相同（重写页里原样拷贝的邻居条目）→ 未动。
//! 窗口内「加了又删」不出现，「删了又建」判 `Modified`，与
//! [`crate::data_interface::manual_update::NetOp`] 的净变化口径一致。
//!
//! **纯文件纪律**：本模块不允许出现任何数据库访问，窗口两端一律由调用方显式给定，
//! 不读水位（见 `the_diff_module_never_touches_the_database` 源码断言）。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::Instant;

use aios_core::pdms_types::RefU64;
use pdms_io::defines::IndexPageData;
use pdms_io::io::PdmsIO;

/// 元素记录的物理位置（叶层条目的 `(pgno, offset)`，比对相等即「未动」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordLoc {
    pub pgno: u32,
    pub offset: u32,
}

impl RecordLoc {
    /// 与 `RefnoDataLoc::get_att_offset` 同一公式：页 2 KB、offset 以 2 字节计。
    pub fn att_offset(&self) -> u64 {
        self.pgno as u64 * 0x800 + self.offset as u64 * 2
    }
}

/// 一条净变化条目。
#[derive(Debug, Clone, PartialEq)]
pub struct NetEntry {
    pub refno: RefU64,
    /// 判定用的记录位置：Added / Modified 取目标端新记录，Deleted 取 base 端
    /// 最后存在的旧记录（旧页不可变，仍在文件里，可解析）。
    pub loc: RecordLoc,
    /// 仅 Modified 携带：base 端旧记录的位置——净窗口收集（ADR-022）要按两端
    /// 版本做一次属性 diff，旧版本从这里直读，省一次 O(log n) 点查。
    pub base_loc: Option<RecordLoc>,
    /// 该记录版本落在哪个会话（按记录页号反查会话页范围）。Added / Modified 是
    /// 「窗口内最后写它的会话」；Deleted 是删除前最后一个版本所在会话——净差分
    /// 判不出删除动作发生在哪个会话，这里给的是旧版本的归属，不是删除时刻。
    pub last_touch_sesno: Option<i32>,
    /// `with_noun` 时按 `loc` 解析记录头补出的类型名；解析失败记入
    /// `NetChangeStats::noun_parse_failures` 并留 `None`，不让一条坏记录拖垮整个差分。
    pub noun: Option<String>,
}

/// 单棵树一次遍历的观测计数。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct WalkStats {
    pub pages_read: usize,
    pub leaf_entries: usize,
    /// 叶层哨兵条目（`0x80000001` 对）不是数据，跳过但计数——它若出现在非叶层
    /// 是最左子树指针，照常跟进，不进这个计数。
    pub sentinel_leaf_entries: usize,
    /// 真实页里存在重复条目（B+ 树搜索同样去重、首见者胜），首见之外的计数。
    pub duplicate_leaf_entries: usize,
    /// 子页层级未下降（防环守卫，与 `filter_index_data` 同款）被跳过的分支数。
    pub level_anomalies: usize,
    /// 叶层 `flag != 1` 条目数（纯观察计数，不影响存在性——生产点查对叶条目
    /// 同样不看 flag，凡是按键可达的都算命中）。
    pub nonlive_leaf_entries: usize,
    /// 非叶层同键重复子指针数（首见之外）。实测 ams8000 上同一个键会挂着多个
    /// 子页（Save Work 重写子树后陈旧指针原地留下，新旧先后与 flag 取值都不
    /// 可靠——实测两种排列都存在），生产 B+ 搜索的规则是**不看 flag、同键首见
    /// 者胜**；跟进首见之外的指针会捞出上万条已被最终发布抛弃的临时记录。
    pub duplicate_child_pointers: usize,
    /// 非叶层键序回退（比前一条已保留的键小或相等）的条目数：路由按升序前缀
    /// 生效，乱序条目点查永远选不中，跳过并计数。
    pub out_of_order_child_keys: usize,
    /// 键范围与父界相交为空的子指针数（点查按键路由到不了的分支）。
    pub out_of_range_child_pointers: usize,
    /// 叶层键落在本叶路由范围之外的残留条目数。实测 ams8000：陈旧叶被回收复用
    /// 后，键 25843 的条目躺在覆盖 [7415, 7790) 的叶子里——点查按键路由永远
    /// 到不了它，穷举遍历若不带范围就会把它当成幽灵存在。
    pub out_of_range_leaf_entries: usize,
    /// 读不动/解析不出的子页数（生产 `filter_index_data` 对子页读取失败静默跳过，
    /// 这里同样跳过但必须记账——静默失效是最高级别缺陷）。
    pub unreadable_child_pages: usize,
    /// flag 值分布。12 位字段语义未完全逆向：路由与存在性都不看 flag（与生产
    /// 点查逐字对齐），直方图把全部取值亮给人看，live 点查仲裁测试守护口径。
    pub flag_histogram: BTreeMap<u16, usize>,
}

/// 一次差分的完整统计（证据面，探针与 Python 侧原样透出）。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct NetChangeStats {
    pub base: WalkStats,
    pub target: WalkStats,
    /// 目标树下降时按「页号 ≤ base 会话末页」剪掉的共享子树根数（去重后）。
    pub shared_subtree_prunes: usize,
    pub noun_parse_failures: usize,
    pub elapsed_ms: u64,
}

/// 一个 sesno 窗口的净增删改结果。
#[derive(Debug, Clone, PartialEq)]
pub struct NetChangeSet {
    pub requested_start: i32,
    pub requested_end: i32,
    /// 差分 base 端会话 = 窗口起点前最近的会话；起点前没有任何会话时为 `None`
    /// （空树，窗口内容全部判 Added，正是首次导入的形状）。
    pub base_sesno: Option<i32>,
    /// 差分目标端会话 = ≤ 窗口终点的最近会话。
    pub target_sesno: i32,
    pub added: Vec<NetEntry>,
    pub deleted: Vec<NetEntry>,
    pub modified: Vec<NetEntry>,
    pub stats: NetChangeStats,
}

impl NetChangeSet {
    fn empty(start: i32, end: i32, base_sesno: Option<i32>, target_sesno: i32) -> Self {
        Self {
            requested_start: start,
            requested_end: end,
            base_sesno,
            target_sesno,
            added: Vec::new(),
            deleted: Vec::new(),
            modified: Vec::new(),
            stats: NetChangeStats::default(),
        }
    }

    /// 证据面 JSON（Python 绑定与探针共用同一份形状）。refno 用 `a_b` 形态，
    /// 与库内 `pe:` record id 一致，拿到即可拼 SurrealQL。
    pub fn to_json(&self) -> serde_json::Value {
        fn entries(list: &[NetEntry]) -> Vec<serde_json::Value> {
            list.iter()
                .map(|entry| {
                    let mut object = serde_json::json!({
                        "refno": entry.refno.to_string(),
                        "record_pgno": entry.loc.pgno,
                        "record_offset": entry.loc.offset,
                        "last_touch_sesno": entry.last_touch_sesno,
                        "noun": entry.noun,
                    });
                    if let Some(base) = entry.base_loc {
                        object["base_record_pgno"] = serde_json::json!(base.pgno);
                        object["base_record_offset"] = serde_json::json!(base.offset);
                    }
                    object
                })
                .collect()
        }
        fn walk(stats: &WalkStats) -> serde_json::Value {
            serde_json::json!({
                "pages_read": stats.pages_read,
                "leaf_entries": stats.leaf_entries,
                "sentinel_leaf_entries": stats.sentinel_leaf_entries,
                "duplicate_leaf_entries": stats.duplicate_leaf_entries,
                "level_anomalies": stats.level_anomalies,
                "nonlive_leaf_entries": stats.nonlive_leaf_entries,
                "duplicate_child_pointers": stats.duplicate_child_pointers,
                "out_of_order_child_keys": stats.out_of_order_child_keys,
                "out_of_range_child_pointers": stats.out_of_range_child_pointers,
                "out_of_range_leaf_entries": stats.out_of_range_leaf_entries,
                "unreadable_child_pages": stats.unreadable_child_pages,
                "flag_histogram": stats
                    .flag_histogram
                    .iter()
                    .map(|(flag, count)| (flag.to_string(), serde_json::json!(count)))
                    .collect::<serde_json::Map<_, _>>(),
            })
        }
        serde_json::json!({
            "requested_start": self.requested_start,
            "requested_end": self.requested_end,
            "base_sesno": self.base_sesno,
            "target_sesno": self.target_sesno,
            "added": entries(&self.added),
            "deleted": entries(&self.deleted),
            "modified": entries(&self.modified),
            "counts": {
                "added": self.added.len(),
                "deleted": self.deleted.len(),
                "modified": self.modified.len(),
            },
            "stats": {
                "base": walk(&self.stats.base),
                "target": walk(&self.stats.target),
                "shared_subtree_prunes": self.stats.shared_subtree_prunes,
                "noun_parse_failures": self.stats.noun_parse_failures,
                "elapsed_ms": self.stats.elapsed_ms,
            },
        })
    }
}

/// 差分核心只需要「按页号取索引页」这一个能力。抽成 trait 是为了让遍历与分类
/// 逻辑能用手搓的迷你 B 树做纯单测——不碰真实文件、不进 IO，进得了 CI。
pub(crate) trait IndexPages {
    fn index_page(&mut self, pgno: u32) -> anyhow::Result<Arc<IndexPageData>>;
}

impl IndexPages for PdmsIO {
    fn index_page(&mut self, pgno: u32) -> anyhow::Result<Arc<IndexPageData>> {
        self.read_index_data(pgno)
    }
}

#[derive(Default)]
struct WalkOutcome {
    touched: HashMap<RefU64, RecordLoc>,
    stats: WalkStats,
}

/// 键 = `(refno_0, refno_1)`，元组序与生产搜索的比较完全一致（先高位后低位；
/// 哨兵 `0x80000001` 对天然大于一切真实键）。
type Key = (u32, u32);

/// 半开路由区间 `[lower, upper)`；`None` 表示无界。
#[derive(Debug, Clone, Copy, Default)]
struct KeyBounds {
    lower: Option<Key>,
    upper: Option<Key>,
}

impl KeyBounds {
    fn contains(&self, key: Key) -> bool {
        self.lower.map_or(true, |lower| key >= lower)
            && self.upper.map_or(true, |upper| key < upper)
    }

    /// 子界 = 父界 ∩ `[child_lower, child_upper)`。
    fn narrowed(&self, child_lower: Option<Key>, child_upper: Option<Key>) -> KeyBounds {
        let lower = match (self.lower, child_lower) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        let upper = match (self.upper, child_upper) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        KeyBounds { lower, upper }
    }

    fn is_empty(&self) -> bool {
        matches!((self.lower, self.upper), (Some(lower), Some(upper)) if lower >= upper)
    }
}

/// 遍历一棵索引树，收集**点查可达**的叶层条目；`should_prune_child` 对每个
/// 非叶层子指针裁决是否整枝跳过（目标树按 base 末页剪共享子树，base 树按
/// 共享根集合剪）。
///
/// 「点查可达」是本函数的全部语义，与生产 B+ 搜索逐字对齐（三条都有真实文件
/// 上的反例逼出来，见各计数字段的注释）：
/// * 路由不看 flag，**同键首见者胜**（Save Work 重写子树后的陈旧同键指针）；
/// * 键序回退的条目选不中（升序前缀才参与路由）；
/// * 子树只对 `[本条目键, 下一条目键)` ∩ 父界负责，哨兵是 `(-∞, 首键)` 的
///   最左分支；叶条目键落在本叶路由范围之外 = 回收页残留，点查到不了。
///
/// 与 `filter_index_data` 同款的两道防环：层级必须严格下降，页号去重访问。
fn walk_tree<P: IndexPages>(
    pages: &mut P,
    root: u32,
    mut should_prune_child: impl FnMut(u32) -> bool,
) -> anyhow::Result<WalkOutcome> {
    let mut outcome = WalkOutcome::default();
    let mut visited: HashSet<u32> = HashSet::new();
    // (页号, 父层级, 路由界, 是否根)：根用一个必然更大的父层级放行；根读不动是
    // 硬错误，子页读不动按生产认领扫描的口径跳过（但记账，不许静默）。
    let mut stack: Vec<(u32, u32, KeyBounds, bool)> =
        vec![(root, u32::MAX, KeyBounds::default(), true)];
    while let Some((pgno, parent_level, bounds, is_root)) = stack.pop() {
        if !visited.insert(pgno) {
            continue;
        }
        let page = match pages.index_page(pgno) {
            Ok(page) => page,
            Err(error) if is_root => {
                return Err(error.context(format!("读取索引根页 {pgno} 失败")));
            }
            Err(_) => {
                outcome.stats.unreadable_child_pages += 1;
                continue;
            }
        };
        outcome.stats.pages_read += 1;
        if page.level >= parent_level {
            outcome.stats.level_anomalies += 1;
            continue;
        }

        if page.level == 0 {
            for loc in &page.refno_locs {
                *outcome.stats.flag_histogram.entry(loc.flag).or_default() += 1;
                if loc.is_start_page() {
                    outcome.stats.sentinel_leaf_entries += 1;
                    continue;
                }
                if !bounds.contains((loc.refno_0, loc.refno_1)) {
                    outcome.stats.out_of_range_leaf_entries += 1;
                    continue;
                }
                if loc.flag != 1 {
                    outcome.stats.nonlive_leaf_entries += 1;
                }
                outcome.stats.leaf_entries += 1;
                let refno = loc.get_refno();
                if outcome.touched.contains_key(&refno) {
                    outcome.stats.duplicate_leaf_entries += 1;
                    continue;
                }
                outcome.touched.insert(
                    refno,
                    RecordLoc {
                        pgno: loc.pgno,
                        offset: loc.offset,
                    },
                );
            }
            continue;
        }

        // 非叶层：先按生产搜索的规则收敛出参与路由的条目（首见去重 + 升序前缀，
        // 哨兵单列），再据相邻键切出每个子树的路由界。剪枝判定必须在去重之后：
        // 陈旧指针若混进共享根集合，base 侧会把对应子树当「幸存」整枝跳过，
        // 删除就漏报了。
        let mut sentinel_child: Option<u32> = None;
        let mut kept: Vec<(Key, u32)> = Vec::new();
        let mut seen_keys: HashSet<Key> = HashSet::new();
        for loc in &page.refno_locs {
            *outcome.stats.flag_histogram.entry(loc.flag).or_default() += 1;
            if loc.is_start_page() {
                if sentinel_child.is_none() {
                    sentinel_child = Some(loc.pgno);
                } else {
                    outcome.stats.duplicate_child_pointers += 1;
                }
                continue;
            }
            let key = (loc.refno_0, loc.refno_1);
            if !seen_keys.insert(key) {
                outcome.stats.duplicate_child_pointers += 1;
                continue;
            }
            if kept.last().is_some_and(|(last, _)| key <= *last) {
                outcome.stats.out_of_order_child_keys += 1;
                continue;
            }
            kept.push((key, loc.pgno));
        }

        let first_key = kept.first().map(|(key, _)| *key);
        if let Some(child) = sentinel_child {
            let child_bounds = bounds.narrowed(None, first_key);
            if child_bounds.is_empty() {
                outcome.stats.out_of_range_child_pointers += 1;
            } else if !should_prune_child(child) {
                stack.push((child, page.level, child_bounds, false));
            }
        }
        for (index, &(key, child)) in kept.iter().enumerate() {
            let next_key = kept.get(index + 1).map(|(next, _)| *next);
            let child_bounds = bounds.narrowed(Some(key), next_key);
            if child_bounds.is_empty() {
                outcome.stats.out_of_range_child_pointers += 1;
                continue;
            }
            if !should_prune_child(child) {
                stack.push((child, page.level, child_bounds, false));
            }
        }
    }
    Ok(outcome)
}

/// 纯分类：两侧触达集比对出净三态。位置相同的条目是重写页里原样拷贝的邻居——
/// 未动，不出现在任何一类里。Modified 连带 base 端旧位置一起给出（净窗口收集
/// 要按两端版本做属性 diff）。输出按 refno 排序，结果可复现。
fn classify(
    base: &HashMap<RefU64, RecordLoc>,
    target: &HashMap<RefU64, RecordLoc>,
) -> (
    Vec<(RefU64, RecordLoc)>,
    Vec<(RefU64, RecordLoc)>,
    Vec<(RefU64, RecordLoc, RecordLoc)>,
) {
    let mut added = Vec::new();
    let mut modified = Vec::new();
    for (refno, loc) in target {
        match base.get(refno) {
            None => added.push((*refno, *loc)),
            Some(old) if old != loc => modified.push((*refno, *loc, *old)),
            Some(_) => {}
        }
    }
    let mut deleted: Vec<(RefU64, RecordLoc)> = base
        .iter()
        .filter(|(refno, _)| !target.contains_key(refno))
        .map(|(refno, loc)| (*refno, *loc))
        .collect();
    let key2 = |entry: &(RefU64, RecordLoc)| (entry.0.get_0(), entry.0.get_1());
    let key3 = |entry: &(RefU64, RecordLoc, RecordLoc)| (entry.0.get_0(), entry.0.get_1());
    added.sort_by_key(key2);
    deleted.sort_by_key(key2);
    modified.sort_by_key(key3);
    (added, deleted, modified)
}

struct RawDiff {
    added: Vec<(RefU64, RecordLoc)>,
    deleted: Vec<(RefU64, RecordLoc)>,
    modified: Vec<(RefU64, RecordLoc, RecordLoc)>,
    stats: NetChangeStats,
}

/// 双根差分的执行体（对页源抽象，单测的入口）。
///
/// 执行序是正确性的一部分：先走目标树收集共享子树根，再用**同一个集合**剪 base
/// 树。base 树里一个完整存活的子树，必然以它的根页号出现在目标树某个新内页的
/// 子指针上（CoW 下老页不会被半引用），所以 base 侧命中共享根即可整枝跳过——
/// 两侧 IO 都正比于变更量，而不是树的大小。
fn diff_roots<P: IndexPages>(
    pages: &mut P,
    base_root: Option<u32>,
    base_end_pgno: u32,
    target_root: u32,
) -> anyhow::Result<RawDiff> {
    let mut shared_roots: HashSet<u32> = HashSet::new();
    let target_walk = walk_tree(pages, target_root, |child| {
        if child <= base_end_pgno {
            shared_roots.insert(child);
            true
        } else {
            false
        }
    })?;

    let base_walk = match base_root {
        // 树整棵被目标共享（层高增长时 base 根会成为目标某内页的子指针）：
        // base 侧一页都不用读，也不可能有删除。
        Some(root) if shared_roots.contains(&root) => WalkOutcome::default(),
        Some(root) => walk_tree(pages, root, |child| shared_roots.contains(&child))?,
        None => WalkOutcome::default(),
    };

    let (added, deleted, modified) = classify(&base_walk.touched, &target_walk.touched);
    Ok(RawDiff {
        added,
        deleted,
        modified,
        stats: NetChangeStats {
            base: base_walk.stats,
            target: target_walk.stats,
            shared_subtree_prunes: shared_roots.len(),
            noun_parse_failures: 0,
            elapsed_ms: 0,
        },
    })
}

/// 窗口两端锚点（纯函数）：base = 起点前最近会话，target = ≤ 终点的最近会话。
///
/// `end` 超出文件最新会话是**错误**而不是夹逼——窗口来自调用方，超界说明窗口
/// 与文件对不上（拿错文件 / 记错水位），静默夹逼会把「差半截」演成「没差」。
fn resolve_window_anchors(
    sesno_pgno_map: &BTreeMap<i32, u32>,
    start: i32,
    end: i32,
) -> anyhow::Result<(Option<i32>, i32)> {
    anyhow::ensure!(start >= 1, "窗口起点必须 ≥ 1，得到 {start}");
    anyhow::ensure!(start <= end, "非法窗口 {start}..={end}");
    let (&max_sesno, _) = sesno_pgno_map
        .last_key_value()
        .ok_or_else(|| anyhow::anyhow!("文件里没有任何会话页"))?;
    anyhow::ensure!(
        end <= max_sesno,
        "窗口终点 {end} 超出文件最新会话 {max_sesno}，不猜——先确认窗口与文件是同一来源"
    );
    let target_sesno = sesno_pgno_map
        .range(..=end)
        .next_back()
        .map(|(sesno, _)| *sesno)
        .ok_or_else(|| anyhow::anyhow!("窗口终点 {end} 之前没有任何会话"))?;
    let base_sesno = sesno_pgno_map
        .range(..start)
        .next_back()
        .map(|(sesno, _)| *sesno);
    Ok((base_sesno, target_sesno))
}

/// 对一个已打开（或可打开）的库文件做窗口净差分。
///
/// `with_noun` 为真时对三类条目按记录位置解析记录头补类型名（Deleted 解析的是
/// base 端旧记录——旧页不可变，仍然可读）；每 refno 一次解析，是显式的付费项。
pub fn collect_net_changes(
    io: &mut PdmsIO,
    range: RangeInclusive<i32>,
    with_noun: bool,
) -> anyhow::Result<NetChangeSet> {
    let started = Instant::now();
    let (start, end) = (*range.start(), *range.end());
    if io.sesno_pgno_map.is_empty() {
        io.open()
            .map_err(|error| anyhow::anyhow!("打开 PDMS IO 失败: {error}"))?;
    }
    let (base_sesno, target_sesno) = resolve_window_anchors(&io.sesno_pgno_map, start, end)?;

    // 窗口内一个会话都没有（target 落在起点之前）：净变化为空。
    if target_sesno < start {
        let mut set = NetChangeSet::empty(start, end, base_sesno, target_sesno);
        set.stats.elapsed_ms = started.elapsed().as_millis() as u64;
        return Ok(set);
    }

    let (target_root, _) = session_anchor(io, target_sesno)?;
    let (base_root, base_end_pgno) = match base_sesno {
        Some(sesno) => {
            let (root, end_pgno) = session_anchor(io, sesno)?;
            (Some(root), end_pgno)
        }
        None => (None, 0),
    };

    if base_root == Some(target_root) {
        // 两端同一棵树（窗口内的会话没动索引）：零差分。
        let mut set = NetChangeSet::empty(start, end, base_sesno, target_sesno);
        set.stats.elapsed_ms = started.elapsed().as_millis() as u64;
        return Ok(set);
    }
    if let Some(base_root) = base_root {
        // 追加模型下目标根必然是新页。不满足说明文件被压缩/回卷过，差分的
        // 共享判据（页号边界）不再成立，宁可拒绝也不给一个悄悄错的答案。
        anyhow::ensure!(
            target_root > base_end_pgno,
            "目标索引根页 {target_root} 不高于 base 会话末页 {base_end_pgno}\
             （base 根 {base_root}），文件形态超出追加模型，拒绝差分"
        );
    }

    let raw = diff_roots(io, base_root, base_end_pgno, target_root)?;
    let mut stats = raw.stats;

    let mut enrich = |io: &mut PdmsIO,
                      list: Vec<(RefU64, RecordLoc, Option<RecordLoc>)>,
                      stats: &mut NetChangeStats|
     -> Vec<NetEntry> {
        list.into_iter()
            .map(|(refno, loc, base_loc)| {
                let last_touch_sesno = io.get_sesno(loc.pgno).map(|sesno| sesno as i32);
                let noun = if with_noun {
                    match io.parse_raw_element(loc.att_offset()) {
                        Ok(mut data) => Some(data.att_map().get_type()),
                        Err(_) => {
                            stats.noun_parse_failures += 1;
                            None
                        }
                    }
                } else {
                    None
                };
                NetEntry {
                    refno,
                    loc,
                    base_loc,
                    last_touch_sesno,
                    noun,
                }
            })
            .collect()
    };

    let with_none =
        |list: Vec<(RefU64, RecordLoc)>| -> Vec<(RefU64, RecordLoc, Option<RecordLoc>)> {
            list.into_iter()
                .map(|(refno, loc)| (refno, loc, None))
                .collect()
        };
    let added = enrich(io, with_none(raw.added), &mut stats);
    let deleted = enrich(io, with_none(raw.deleted), &mut stats);
    let modified_input: Vec<(RefU64, RecordLoc, Option<RecordLoc>)> = raw
        .modified
        .into_iter()
        .map(|(refno, loc, base_loc)| (refno, loc, Some(base_loc)))
        .collect();
    let modified = enrich(io, modified_input, &mut stats);
    stats.elapsed_ms = started.elapsed().as_millis() as u64;

    Ok(NetChangeSet {
        requested_start: start,
        requested_end: end,
        base_sesno,
        target_sesno,
        added,
        deleted,
        modified,
        stats,
    })
}

/// 读某会话页的 `(索引根, 会话末页)`，立即拷出（`read_ses_data` 借用 `&mut self`）。
fn session_anchor(io: &mut PdmsIO, sesno: i32) -> anyhow::Result<(u32, u32)> {
    let ses_pgno = *io
        .sesno_pgno_map
        .get(&sesno)
        .ok_or_else(|| anyhow::anyhow!("会话 {sesno} 不在会话页映射里"))?;
    let data = io
        .read_ses_data(ses_pgno)
        .map_err(|error| anyhow::anyhow!("读取会话页 {ses_pgno}（sesno={sesno}）失败: {error}"))?;
    Ok((data.index_root_pageno, data.end_pgno))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdms_io::defines::RefnoDataLoc;

    const SENTINEL: u32 = 0x8000_0001;

    fn loc(r0: u32, r1: u32, pgno: u32, offset: u32, flag: u16) -> RefnoDataLoc {
        RefnoDataLoc {
            refno_0: r0,
            refno_1: r1,
            pgno,
            offset,
            flag,
        }
    }

    fn page(level: u32, locs: Vec<RefnoDataLoc>) -> Arc<IndexPageData> {
        Arc::new(IndexPageData {
            page_type: 6,
            noun: 0x00CC_47DF,
            level,
            unknowns: [0; 3],
            pfno: 0,
            refno_locs: locs,
            remain_zero_bytes: Vec::new(),
        })
    }

    /// 迷你页源：记录每次取页，剪枝断言靠它（被剪掉的子树一页都不许读）。
    struct MemPages {
        pages: HashMap<u32, Arc<IndexPageData>>,
        reads: Vec<u32>,
    }

    impl MemPages {
        fn new(pages: Vec<(u32, Arc<IndexPageData>)>) -> Self {
            Self {
                pages: pages.into_iter().collect(),
                reads: Vec::new(),
            }
        }
    }

    impl IndexPages for MemPages {
        fn index_page(&mut self, pgno: u32) -> anyhow::Result<Arc<IndexPageData>> {
            self.reads.push(pgno);
            self.pages
                .get(&pgno)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("测试页源里没有页 {pgno}"))
        }
    }

    fn refnos(list: &[(RefU64, RecordLoc)]) -> Vec<RefU64> {
        list.iter().map(|(refno, _)| *refno).collect()
    }

    fn refnos3(list: &[(RefU64, RecordLoc, RecordLoc)]) -> Vec<RefU64> {
        list.iter().map(|(refno, _, _)| *refno).collect()
    }

    fn r(r1: u32) -> RefU64 {
        RefU64::from_two_nums(100, r1)
    }

    /// 叶层四态一次说死：新增 / 删除 / 位置变了是修改 / 位置没变（重写页里原样
    /// 拷贝的邻居）不算任何一类。
    #[test]
    fn leaf_diff_classifies_added_deleted_modified_and_untouched() {
        // base（root=5，末页 10）：a@2:0、b@2:2、c@2:4；
        // target（root=11）：a 原位拷贝、b 换了记录页（改）、d 新条目；c 没了（删）。
        let mut pages = MemPages::new(vec![
            (
                5,
                page(
                    0,
                    vec![
                        loc(100, 1, 2, 0, 1),
                        loc(100, 2, 2, 2, 1),
                        loc(100, 3, 2, 4, 1),
                    ],
                ),
            ),
            (
                11,
                page(
                    0,
                    vec![
                        loc(100, 1, 2, 0, 1),
                        loc(100, 2, 12, 0, 1),
                        loc(100, 4, 12, 2, 1),
                    ],
                ),
            ),
        ]);

        let diff = diff_roots(&mut pages, Some(5), 10, 11).expect("diff");

        assert_eq!(refnos(&diff.added), vec![r(4)]);
        assert_eq!(refnos(&diff.deleted), vec![r(3)]);
        assert_eq!(refnos3(&diff.modified), vec![r(2)]);
        assert_eq!(
            diff.modified[0].1,
            RecordLoc {
                pgno: 12,
                offset: 0
            },
            "修改条目携带目标端新位置"
        );
        assert_eq!(
            diff.modified[0].2,
            RecordLoc { pgno: 2, offset: 2 },
            "修改条目连带 base 端旧位置（净窗口 diff 直读，省一次点查）"
        );
        assert_eq!(
            diff.deleted[0].1,
            RecordLoc { pgno: 2, offset: 4 },
            "删除条目携带 base 端旧位置（旧页不可变，仍可解析）"
        );
    }

    /// 共享子树两侧都必须整枝跳过：目标树按页号边界剪，base 树按同一个共享根
    /// 集合剪——被剪掉的子树连页都不许读（IO 正比于变更量的证据）。
    #[test]
    fn shared_subtrees_are_pruned_on_both_sides_without_reading_them() {
        // base（root=8，末页 10）：内页 8 → 哨兵→6（左叶，共享），键(100,8)→7（右叶）。
        // target（root=20）：内页 20 → 哨兵→6（同一左叶），键(100,8)→21（重写的右叶）。
        let left_leaf = page(0, vec![loc(100, 1, 2, 0, 1), loc(100, 2, 2, 2, 1)]);
        let base_right = page(0, vec![loc(100, 8, 3, 0, 1), loc(100, 9, 3, 2, 1)]);
        let target_right = page(0, vec![loc(100, 8, 13, 0, 1), loc(100, 9, 3, 2, 1)]);
        let mut pages = MemPages::new(vec![
            (6, left_leaf),
            (7, base_right),
            (
                8,
                page(
                    1,
                    vec![loc(SENTINEL, SENTINEL, 6, 0, 1), loc(100, 8, 7, 0, 1)],
                ),
            ),
            (21, target_right),
            (
                20,
                page(
                    1,
                    vec![loc(SENTINEL, SENTINEL, 6, 0, 1), loc(100, 8, 21, 0, 1)],
                ),
            ),
        ]);

        let diff = diff_roots(&mut pages, Some(8), 10, 20).expect("diff");

        assert!(
            !pages.reads.contains(&6),
            "共享左叶被两侧整枝跳过，不许读: {:?}",
            pages.reads
        );
        assert_eq!(diff.stats.shared_subtree_prunes, 1);
        assert_eq!(
            refnos3(&diff.modified),
            vec![r(8)],
            "只有右叶里换了位置的条目"
        );
        assert!(diff.added.is_empty() && diff.deleted.is_empty());
        // 右叶里原样拷贝的邻居（100_9）两边位置相同 → 未动，不出现在任何一类。
    }

    /// 非叶层哨兵是最左子树指针必须跟进；叶层哨兵不是数据，跳过并计数。
    #[test]
    fn internal_sentinel_is_followed_and_leaf_sentinel_is_skipped() {
        let mut pages = MemPages::new(vec![
            (30, page(1, vec![loc(SENTINEL, SENTINEL, 31, 0, 1)])),
            (
                31,
                page(
                    0,
                    vec![loc(SENTINEL, SENTINEL, 2, 0, 1), loc(100, 1, 32, 0, 1)],
                ),
            ),
        ]);

        let diff = diff_roots(&mut pages, None, 0, 30).expect("diff");

        assert_eq!(refnos(&diff.added), vec![r(1)], "空 base = 全部判新增");
        assert_eq!(diff.stats.target.sentinel_leaf_entries, 1);
        assert!(
            pages.reads.contains(&31),
            "内层哨兵携带的子树必须被走到: {:?}",
            pages.reads
        );
    }

    /// 真实页里存在重复条目（B+ 树搜索同样去重、首见者胜）：首见之外只计数。
    #[test]
    fn duplicate_leaf_entries_keep_the_first_and_are_counted() {
        let mut pages = MemPages::new(vec![(
            40,
            page(0, vec![loc(100, 1, 41, 0, 1), loc(100, 1, 42, 0, 1)]),
        )]);

        let diff = diff_roots(&mut pages, None, 0, 40).expect("diff");

        assert_eq!(diff.added.len(), 1);
        assert_eq!(
            diff.added[0].1,
            RecordLoc {
                pgno: 41,
                offset: 0
            },
            "首见者胜"
        );
        assert_eq!(diff.stats.target.duplicate_leaf_entries, 1);
    }

    /// 层级不下降的子页跳过并计数（与 `filter_index_data` 同款防环）；读不动的
    /// 子页容忍但计数；路由与存在性都不看 flag（对齐生产点查）——可达的
    /// flag != 1 叶条目照样算存在，只进观察计数。三处异常都不许静默。
    #[test]
    fn level_regressions_and_routing_anomalies_are_counted_and_flags_stay_blind() {
        let mut pages = MemPages::new(vec![
            (
                50,
                page(
                    1,
                    vec![
                        loc(100, 1, 51, 0, 1),
                        loc(100, 2, 52, 0, 1),
                        // flag=2 的内层指针照样跟进（生产搜索不看 flag）；页 99
                        // 不存在 → 按认领扫描口径跳过整枝但记账。
                        loc(100, 3, 99, 0, 2),
                    ],
                ),
            ),
            // 51 层级与父相同 → 防环守卫跳过。
            (51, page(1, vec![loc(100, 1, 60, 0, 1)])),
            // 52 覆盖 [(100,2),(100,3))：一条 flag=3 的可达条目（算存在 + 观察
            // 计数）+ 一条键越界的回收页残留（不可达，剔除并计数）。
            (
                52,
                page(0, vec![loc(100, 2, 53, 0, 3), loc(100, 7, 53, 2, 1)]),
            ),
        ]);

        let diff = diff_roots(&mut pages, None, 0, 50).expect("diff");

        assert_eq!(diff.stats.target.level_anomalies, 1);
        assert_eq!(diff.stats.target.unreadable_child_pages, 1);
        assert_eq!(
            diff.stats.target.nonlive_leaf_entries, 1,
            "flag != 1 只是观察计数"
        );
        assert_eq!(diff.stats.target.out_of_range_leaf_entries, 1);
        assert_eq!(
            refnos(&diff.added),
            vec![r(2)],
            "可达的 flag=3 条目算存在；越界残留不算"
        );
        assert_eq!(diff.stats.target.flag_histogram.get(&3), Some(&1));
        assert_eq!(diff.stats.target.flag_histogram.get(&2), Some(&1));
        assert!(
            !pages.reads.contains(&60),
            "层级异常的子树不得继续下降: {:?}",
            pages.reads
        );
        assert!(
            pages.reads.contains(&99),
            "内层路由不看 flag，页 99 应该被尝试读取: {:?}",
            pages.reads
        );
    }

    /// 实测 ams8000 的第三种形状钉成回归：陈旧叶被回收复用后，键远超本叶路由
    /// 范围的残留条目仍躺在页里（键 25843 出现在覆盖 [7415, 7790) 的叶子里）。
    /// 点查按键路由永远到不了它——差分同样不许把它当成存在。
    #[test]
    fn out_of_range_leftover_leaf_entries_are_invisible_like_the_point_search() {
        let mut pages = MemPages::new(vec![
            // 叶 61 覆盖 [(100,10),(100,20))，却残留一条键 (100,99) 的条目。
            (
                61,
                page(0, vec![loc(100, 10, 7, 0, 1), loc(100, 99, 7, 2, 1)]),
            ),
            (62, page(0, vec![loc(100, 20, 7, 4, 1)])),
            (
                60,
                page(1, vec![loc(100, 10, 61, 0, 1), loc(100, 20, 62, 0, 1)]),
            ),
        ]);

        let diff = diff_roots(&mut pages, None, 0, 60).expect("diff");

        assert_eq!(
            refnos(&diff.added),
            vec![r(10), r(20)],
            "残留键 (100,99) 点查不可达，不算存在"
        );
        assert_eq!(diff.stats.target.out_of_range_leaf_entries, 1);
    }

    /// 实测 ams8000 的形状钉成回归：Save Work 重写子树后，内页里同一个键会留下
    /// 「新指针在前、陈旧指针在后」的重复条目。陈旧指针必须被首见去重丢掉——
    /// 不丢会捞出已被最终发布抛弃的临时记录；而且它若混进共享根集合，base 侧
    /// 会把对应子树当「幸存」跳过，删除就静默漏报（本测试的第二重断言）。
    #[test]
    fn stale_duplicate_key_child_pointers_are_ignored_and_never_pollute_shared_roots() {
        let mut pages = MemPages::new(vec![
            // base 树：单叶 91（页号 ≤ base_end=100，正是会被误当共享根的形状）。
            (
                91,
                page(0, vec![loc(100, 1, 5, 0, 1), loc(100, 2, 5, 2, 1)]),
            ),
            // 目标树（新页号都在 base_end 之上）：新叶 190 里 A 换了记录位置、
            // B 消失；新根 195 上同键两条，首见指向新叶，陈旧条目仍指向旧叶 91。
            (190, page(0, vec![loc(100, 1, 105, 0, 1)])),
            (
                195,
                page(1, vec![loc(100, 1, 190, 0, 1), loc(100, 1, 91, 0, 1)]),
            ),
        ]);

        let diff = diff_roots(&mut pages, Some(91), 100, 195).expect("diff");

        assert_eq!(diff.stats.target.duplicate_child_pointers, 1);
        assert_eq!(
            diff.stats.shared_subtree_prunes, 0,
            "陈旧指针不得进入共享根集合"
        );
        assert_eq!(refnos3(&diff.modified), vec![r(1)]);
        assert_eq!(
            refnos(&diff.deleted),
            vec![r(2)],
            "base 叶必须被真实走到，B 的删除不许漏报"
        );
        assert_eq!(
            pages.reads.iter().filter(|&&p| p == 91).count(),
            1,
            "旧叶只该被 base 侧读一次（目标侧的陈旧指针不跟进）: {:?}",
            pages.reads
        );
    }

    /// 子页读不动按认领扫描口径跳过整枝，但必须记账——静默失效是最高级别缺陷。
    /// 根页读不动仍是硬错误。
    #[test]
    fn unreadable_child_pages_are_skipped_with_a_count_and_a_bad_root_is_fatal() {
        let mut pages = MemPages::new(vec![
            (
                70,
                page(1, vec![loc(100, 1, 71, 0, 1), loc(100, 2, 72, 0, 1)]),
            ),
            // 71 缺失（读取报错）；72 正常。
            (72, page(0, vec![loc(100, 2, 73, 0, 1)])),
        ]);

        let diff = diff_roots(&mut pages, None, 0, 70).expect("diff");
        assert_eq!(diff.stats.target.unreadable_child_pages, 1);
        assert_eq!(refnos(&diff.added), vec![r(2)]);

        let mut missing_root = MemPages::new(vec![]);
        assert!(
            diff_roots(&mut missing_root, None, 0, 80).is_err(),
            "根页读不动必须硬错误"
        );
    }

    /// base 根整棵被目标共享（层高增长的形状）：base 一页不读、没有删除。
    #[test]
    fn a_fully_shared_base_tree_reads_nothing_and_deletes_nothing() {
        // base：单叶 root=6（末页 10）。target：新内页 20 → 哨兵→6（共享）+ 新叶 21。
        let mut pages = MemPages::new(vec![
            (6, page(0, vec![loc(100, 1, 2, 0, 1)])),
            (21, page(0, vec![loc(100, 2, 22, 0, 1)])),
            (
                20,
                page(
                    1,
                    vec![loc(SENTINEL, SENTINEL, 6, 0, 1), loc(100, 2, 21, 0, 1)],
                ),
            ),
        ]);

        let diff = diff_roots(&mut pages, Some(6), 10, 20).expect("diff");

        assert_eq!(refnos(&diff.added), vec![r(2)]);
        assert!(diff.deleted.is_empty(), "共享子树里的条目全部存活");
        assert_eq!(
            pages.reads.iter().filter(|&&p| p == 6).count(),
            0,
            "base 根被共享时一页都不读: {:?}",
            pages.reads
        );
        assert_eq!(diff.stats.base.pages_read, 0);
    }

    /// 窗口锚点解析是纯函数：起点前无会话 → base None；终点超界 → 响亮报错。
    #[test]
    fn window_anchors_resolve_base_and_target_and_refuse_overruns() {
        let map: BTreeMap<i32, u32> = [(3, 30), (5, 50), (9, 90)].into_iter().collect();

        assert_eq!(
            resolve_window_anchors(&map, 4, 9).expect("anchors"),
            (Some(3), 9)
        );
        assert_eq!(
            resolve_window_anchors(&map, 1, 9).expect("anchors"),
            (None, 9),
            "起点前没有会话 = 空树 base"
        );
        assert_eq!(
            resolve_window_anchors(&map, 6, 8).expect("anchors"),
            (Some(5), 5),
            "窗口内没有会话时 target 落在起点之前（调用方按空差分处理）"
        );
        assert!(
            resolve_window_anchors(&map, 4, 10).is_err(),
            "终点超界必须报错"
        );
        assert!(resolve_window_anchors(&map, 0, 5).is_err());
        assert!(resolve_window_anchors(&map, 7, 6).is_err());
    }

    /// live 对拍的独立仲裁：对同一 refno 用**生产 B+ 树点查**在窗口两端各查一次，
    /// 由「在场与否 + 记录位置」推出期望三态。刻意不用 `fold_net_op` 当 oracle——
    /// 回放折叠与净差分在「删了又建 → Added vs Modified」「临时删除」上有已知的
    /// 措辞差异，拿它仲裁会把两套口径的措辞差异误判成缺陷。
    fn expected_class(start_loc: Option<u64>, end_loc: Option<u64>) -> Option<&'static str> {
        match (start_loc, end_loc) {
            (None, Some(_)) => Some("added"),
            (Some(_), None) => Some("deleted"),
            (Some(start), Some(end)) if start != end => Some("modified"),
            _ => None, // 位置未变 = 未动；两端都不在场 = 窗口内自我抵消
        }
    }

    /// live：真实 ams8000 上，净差分对「回放触达集 ∪ 差分结果集」逐 refno 与
    /// 生产点查仲裁一致，且回放触达的每个 refno 都被差分正确处置。
    /// 前置：本机存在 AvevaMarineSample 的 ams8000_0001（`AIOS_AMS8000_FILE`
    /// 可覆盖路径）；从仓库根运行（属性字典按 CWD 相对路径装载）。
    #[test]
    #[ignore = "manual live: needs the real ams8000_0001 file on this machine"]
    fn live_ams8000_net_diff_agrees_with_point_lookups_and_covers_replay() {
        use std::path::PathBuf;

        let path = std::env::var_os("AIOS_AMS8000_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001")
            });
        assert!(path.is_file(), "找不到 ams8000 文件: {}", path.display());

        let mut io = PdmsIO::new("", path.clone(), true);
        io.open().expect("open ams8000");
        let latest = io.get_latest_sesno().expect("latest sesno") as i32;
        let starts: Vec<i32> = {
            let mut list = vec![1, latest / 2, (latest - 5).max(1), latest];
            list.dedup();
            list
        };

        for start in starts {
            let window = start..=latest;
            let diff_started = Instant::now();
            let diff = collect_net_changes(&mut io, window.clone(), false).expect("net diff");
            let diff_ms = diff_started.elapsed().as_millis();

            let replay_started = Instant::now();
            let replay =
                crate::data_interface::increment_pipeline::IncrementPipeline::collect_changes(
                    &path,
                    window.clone(),
                )
                .expect("replay collect");
            let replay_ms = replay_started.elapsed().as_millis();

            let mut diff_class: HashMap<RefU64, &'static str> = HashMap::new();
            for entry in &diff.added {
                diff_class.insert(entry.refno, "added");
            }
            for entry in &diff.deleted {
                diff_class.insert(entry.refno, "deleted");
            }
            for entry in &diff.modified {
                diff_class.insert(entry.refno, "modified");
            }

            let mut union: HashSet<RefU64> = diff_class.keys().copied().collect();
            let mut replay_touched = 0usize;
            for ops in replay.values() {
                for op in ops {
                    if !matches!(op.detail, pdms_io::io::EleOperationDetail::None) {
                        union.insert(op.refno);
                        replay_touched += 1;
                    }
                }
            }

            for refno in &union {
                let start_loc = diff
                    .base_sesno
                    .and_then(|base| io.search_latest_refno(*refno, Some(base as u32)))
                    .map(|(_, offset)| offset);
                let end_loc = io
                    .search_latest_refno(*refno, Some(diff.target_sesno as u32))
                    .map(|(_, offset)| offset);
                let expected = expected_class(start_loc, end_loc);
                let got = diff_class.get(refno).copied();
                assert_eq!(
                    got, expected,
                    "refno {refno} 在窗口 {start}..={latest} 的净判定与生产点查仲裁不一致\
                     （start_loc={start_loc:?} end_loc={end_loc:?}）"
                );
            }

            println!(
                "[live] 窗口 {start}..={latest}: diff {}ms（added={} deleted={} modified={} \
                 目标侧读页 {} / 剪枝 {}） vs 回放 {}ms（触达 {} 条 op）；仲裁 {} 个 refno 全一致",
                diff_ms,
                diff.added.len(),
                diff.deleted.len(),
                diff.modified.len(),
                diff.stats.target.pages_read,
                diff.stats.shared_subtree_prunes,
                replay_ms,
                replay_touched,
                union.len()
            );
            println!(
                "[live] flag 分布 target={:?} base={:?} 叶哨兵 t/b={}/{} 重复 t/b={}/{} 层级异常 t/b={}/{}",
                diff.stats.target.flag_histogram,
                diff.stats.base.flag_histogram,
                diff.stats.target.sentinel_leaf_entries,
                diff.stats.base.sentinel_leaf_entries,
                diff.stats.target.duplicate_leaf_entries,
                diff.stats.base.duplicate_leaf_entries,
                diff.stats.target.level_anomalies,
                diff.stats.base.level_anomalies,
            );
        }
    }

    /// live 诊断：某个「树上可达但点查不认」的 refno，把它在目标树里的全部可达
    /// 路径挖出来（每级内页的键/子页/flag、同键重复情况），用来给存在性口径定案。
    /// 用法：`AIOS_DIAG_REFNO=24384_22234 AIOS_DIAG_SESNO=214 cargo test ... --ignored`
    #[test]
    #[ignore = "manual live: path diagnosis on the real ams8000_0001 file"]
    fn live_ams8000_diagnose_reachable_paths_for_one_refno() {
        use std::path::PathBuf;

        let path = std::env::var_os("AIOS_AMS8000_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001")
            });
        let raw = std::env::var("AIOS_DIAG_REFNO").unwrap_or_else(|_| "24384_22234".into());
        let (r0, r1) = raw
            .split_once(['_', '/'])
            .map(|(a, b)| (a.parse::<u32>().unwrap(), b.parse::<u32>().unwrap()))
            .expect("AIOS_DIAG_REFNO 形态 a_b");
        let target = RefU64::from_two_nums(r0, r1);

        let mut io = PdmsIO::new("", path, true);
        io.open().expect("open");
        let sesno = std::env::var("AIOS_DIAG_SESNO")
            .ok()
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or_else(|| io.get_latest_sesno().expect("latest") as i32);
        let ses_pgno = *io.sesno_pgno_map.get(&sesno).expect("session page");
        let root = io
            .read_ses_data(ses_pgno)
            .expect("ses data")
            .index_root_pageno;
        println!("[diag] refno={target} sesno={sesno} root={root}");

        // DFS 全树（与 walk_tree 同口径：flag 盲 + 同键首见去重），记录 parent 链；
        // 到叶层找目标 refno 的条目。
        let mut parent: HashMap<u32, (u32, usize)> = HashMap::new(); // page -> (父页, 父内条目序)
        let mut stack = vec![(root, u32::MAX)];
        let mut visited: HashSet<u32> = HashSet::new();
        let mut hits: Vec<(u32, usize)> = Vec::new(); // (叶页, 条目序)
        while let Some((pgno, parent_level)) = stack.pop() {
            if !visited.insert(pgno) {
                continue;
            }
            let Ok(page) = io.read_index_data(pgno) else {
                continue;
            };
            if page.level >= parent_level {
                continue;
            }
            let mut seen_keys: HashSet<(u32, u32)> = HashSet::new();
            for (idx, entry) in page.refno_locs.iter().enumerate() {
                if page.level == 0 {
                    if !entry.is_start_page() && entry.get_refno() == target {
                        hits.push((pgno, idx));
                        println!(
                            "[diag] 叶命中 页{pgno} 条目#{idx} loc=({},{}) flag={}",
                            entry.pgno, entry.offset, entry.flag
                        );
                    }
                } else if seen_keys.insert((entry.refno_0, entry.refno_1)) {
                    parent.entry(entry.pgno).or_insert((pgno, idx));
                    stack.push((entry.pgno, page.level));
                }
            }
        }
        println!("[diag] 命中 {} 处", hits.len());

        for (leaf, _) in &hits {
            println!("[diag] ---- 自叶 {leaf} 上溯 ----");
            let mut cursor = *leaf;
            while let Some(&(parent_page, entry_idx)) = parent.get(&cursor) {
                let page = io.read_index_data(parent_page).expect("parent page");
                let entry = &page.refno_locs[entry_idx];
                let same_key: Vec<String> = page
                    .refno_locs
                    .iter()
                    .enumerate()
                    .filter(|(_, other)| {
                        other.refno_0 == entry.refno_0 && other.refno_1 == entry.refno_1
                    })
                    .map(|(i, other)| format!("#{i}: pgno={} flag={}", other.pgno, other.flag))
                    .collect();
                println!(
                    "[diag] 内页{parent_page}(level={}) 经条目#{entry_idx} key={}_{} → 子页{cursor}；\
                     同键条目: [{}]（本页共 {} 条）",
                    page.level,
                    entry.refno_0,
                    entry.refno_1,
                    same_key.join(", "),
                    page.refno_locs.len()
                );
                cursor = parent_page;
            }
        }

        // 顺带：生产点查的裁决与最终索引（parse_pdms_db）的裁决。
        let search = io.search_latest_refno(target, Some(sesno as u32));
        println!("[diag] search_latest_refno@{sesno} = {search:?}");

        // 逐步复演生产搜索的选枝（同款「首个大于目标 → 取前一条」规则），
        // 打印每一步的候选区，看它在哪一层拐离命中叶。
        let dump_around = |io: &mut PdmsIO, pgno: u32, center: Option<usize>, span: usize| {
            let page = io.read_index_data(pgno).expect("dump page");
            let total = page.refno_locs.len();
            let (from, to) = match center {
                Some(center) => (center.saturating_sub(span), (center + span).min(total - 1)),
                None => (0, total - 1),
            };
            println!(
                "[diag] 页{pgno} level={} 共{}条，条目[{from}..={to}]:",
                page.level, total
            );
            for idx in from..=to {
                let e = &page.refno_locs[idx];
                println!(
                    "[diag]   #{idx} key={}_{} pgno={} offset={} flag={}",
                    e.refno_0, e.refno_1, e.pgno, e.offset, e.flag
                );
            }
        };
        dump_around(&mut io, root, None, 0);
        dump_around(&mut io, 4667, Some(37), 4);
        dump_around(&mut io, 3096, Some(14), 3);
        dump_around(&mut io, 3095, Some(77), 3);

        let mut cursor = root;
        loop {
            let page = io.read_index_data(cursor).expect("navigate page");
            if page.level == 0 {
                let hit = page
                    .refno_locs
                    .iter()
                    .position(|e| e.refno_0 == r0 && e.refno_1 == r1);
                println!("[diag] 复演到叶 {cursor}，精确命中 = {hit:?}");
                break;
            }
            // 与 btree_search_optimized_recursive 同款：跳哨兵、按键去重（首见胜）、
            // 首个大于目标的条目取前一条，全大于取哨兵、全小于取最后一条。
            let mut uniques: Vec<&pdms_io::defines::RefnoDataLoc> = Vec::new();
            let mut seen: HashSet<(u32, u32)> = HashSet::new();
            let mut sentinel: Option<&pdms_io::defines::RefnoDataLoc> = None;
            for e in &page.refno_locs {
                if e.is_start_page() {
                    sentinel = sentinel.or(Some(e));
                    continue;
                }
                if seen.insert((e.refno_0, e.refno_1)) {
                    uniques.push(e);
                }
            }
            let less = |a: (u32, u32), b: (u32, u32)| a.0 < b.0 || (a.0 == b.0 && a.1 < b.1);
            let mut chosen: Option<&pdms_io::defines::RefnoDataLoc> = None;
            let mut prev: Option<&pdms_io::defines::RefnoDataLoc> = None;
            for e in &uniques {
                if less((r0, r1), (e.refno_0, e.refno_1)) {
                    chosen = prev.or(sentinel);
                    break;
                }
                if (r0, r1) == (e.refno_0, e.refno_1) {
                    chosen = Some(e);
                    break;
                }
                prev = Some(e);
            }
            let chosen = chosen.or(prev).or(sentinel).expect("有分支可选");
            println!(
                "[diag] 复演 页{cursor} level={} → 选 key={}_{} 子页{}",
                page.level, chosen.refno_0, chosen.refno_1, chosen.pgno
            );
            cursor = chosen.pgno;
        }
    }

    /// 纯文件纪律钉死：本模块（含测试）不得出现任何数据库访问。窗口判定的输入
    /// 只有文件页——谁往这里引 Surreal 连接或查询，这条立刻红。禁词用 `concat!`
    /// 拆开写，免得断言自己把自己数进去。
    #[test]
    fn the_diff_module_never_touches_the_database() {
        let source = include_str!("session_index_diff.rs");
        let forbidden = [
            concat!("SUL", "_DB"),
            concat!(".que", "ry("),
            concat!("surreal", "db"),
        ];
        for needle in forbidden {
            assert_eq!(
                source.matches(needle).count(),
                0,
                "净差分必须纯文件：源码里不得出现 {needle}"
            );
        }
    }
}
