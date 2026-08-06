//! 全量解析产物里「被复活的已删元素」的裁剪（issue #10）。
//!
//! `parse_db_basic_data` 自顶向下按成员块展开成员树，这一步是尊重删除的：E3D 删掉
//! 一个 SITE 之后，WORLD 的成员表里就没有它了，走不到。但紧接着有一轮补救——把
//! 表内没被走到的元素也当作根展开，再由 `relink_children_by_owner` 按每条记录**自带
//! 的 owner** 把「owner 成员表里没列出它」的父子边补回去。那一轮是为另一个毛病写的
//! （同一元素有多条物理记录，选中的那条可能不带成员块，展开会在那里断掉），可它分不
//! 清两种「owner 没列我」：
//!
//! - 选中的记录压根没有成员块 —— 确实是断链，该补；
//! - owner 列了成员、里面就是没有我 —— 那是**我被删了**。
//!
//! 后者正是 `pdms_io` 增量路径判定删除的原文（`!owner_ele.children.contains(&refno)`
//! → `EleOperationDetail::Deleted`）。同一个信号，两条路径给出相反结论，于是全量解析
//! 把删掉的整棵子树按旧 refno 原样落了库：AMS 的 dbnum 7999 里 `/1WCC-PIPEBJ` 因此
//! 有两棵完全一样的子树（旧的 `24383_2` 在 sesno 21/26/29 已删，新的 `24383_66456`
//! 在 sesno 30 重建），模型树沿 `pe_owner` 边两棵都可达。用户展开靠前的那棵幽灵，
//! 之后在 E3D 里加的分支都挂在真身下面，于是「增量检测得到、树上永远看不见」。
//!
//! 这里按同一个权威口径把补链轮的越界部分摘回去：**元素自己的成员块**说了算。

use std::collections::{HashMap, HashSet, VecDeque};

use aios_core::db::EleDataEntry;
use aios_core::pdms_types::RefU64;
use dashmap::DashMap;
use parse_pdms_db::parse::parse_ele_membs;

/// 一次裁剪的产出。计数进日志，也是回归测试的断言口径。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PruneReport {
    /// 因不在 owner 成员表里而摘掉的父子边。
    pub dropped_edges: usize,
    /// 摘边后自 WORLD 不可达、整体丢弃的元素。
    pub dropped_elements: usize,
    /// 根的成员表为空——无权威可依，整轮跳过，保持解析原样。
    pub skipped_no_root_authority: bool,
}

impl PruneReport {
    /// 这一轮有没有真的动过成员树。
    pub fn is_empty(&self) -> bool {
        self.dropped_edges == 0 && self.dropped_elements == 0
    }
}

/// 按「元素自己的成员块」裁掉补链轮多挂的边，再丢弃自 `world` 不可达的元素。
///
/// `members_of` 返回该元素**自己那条记录**列出的成员；拿不到记录或记录不带成员块时
/// 返回空。空成员表一律当作「没有权威」而不是「确实没有子元素」：补链轮存在的理由
/// 就是选中的记录可能不带成员块，把空表当权威会把那个毛病原样放回来。代价是某个
/// 空记录 owner 名下的幽灵留得下来——保守方向，宁可多留不可错删。
///
/// 同理，`world` 自己的成员表为空时整轮跳过：根都没有权威，可达性无从谈起，再往下
/// 走会把整个库判成不可达。
pub fn prune_resurrected_members<F>(
    world: RefU64,
    children_map: &mut HashMap<RefU64, Vec<RefU64>>,
    members_of: F,
) -> PruneReport
where
    F: Fn(RefU64) -> Vec<RefU64>,
{
    let mut report = PruneReport::default();

    if members_of(world).is_empty() {
        report.skipped_no_root_authority = true;
        return report;
    }

    for (owner, children) in children_map.iter_mut() {
        let authoritative = members_of(*owner);
        if authoritative.is_empty() {
            continue;
        }
        let allowed: HashSet<RefU64> = authoritative.into_iter().collect();
        let before = children.len();
        children.retain(|child| allowed.contains(child));
        report.dropped_edges += before - children.len();
    }

    let mut reached: HashSet<RefU64> = HashSet::from([world]);
    let mut frontier = VecDeque::from([world]);
    while let Some(refno) = frontier.pop_front() {
        let Some(children) = children_map.get(&refno) else {
            continue;
        };
        for child in children {
            if reached.insert(*child) {
                frontier.push_back(*child);
            }
        }
    }

    let before = children_map.len();
    children_map.retain(|refno, _| reached.contains(refno));
    report.dropped_elements = before - children_map.len();

    report
}

/// 一个元素自己那条记录列出的成员（越界或查不到记录时为空）。
///
/// 边界与 `parse_db_basic_data` 内的读法一致：记录从 `pos - 4` 起，`relink` 那轮要求
/// `pos + 20` 落在缓冲区内才肯读 owner，这里沿用同一道门。
pub fn authoritative_members(
    bytes: &[u8],
    refno_table_map: &DashMap<RefU64, EleDataEntry>,
    refno: RefU64,
) -> Vec<RefU64> {
    let Some(pos) = refno_table_map.get(&refno).map(|entry| entry.pos) else {
        return Vec::new();
    };
    if pos < 4 || pos + 20 > bytes.len() {
        return Vec::new();
    }
    parse_ele_membs(&bytes[pos - 4..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refu(n: u64) -> RefU64 {
        RefU64((4000000001u64 << 32) | n)
    }

    const WORLD: u64 = 0;
    const LIVE_SITE: u64 = 100;
    const LIVE_ZONE: u64 = 101;
    const GHOST_SITE: u64 = 1;
    const GHOST_ZONE: u64 = 2;
    const GHOST_PIPE: u64 = 3;

    /// issue #10 的现场形态：WORLD 的成员表只列了活着的 SITE，补链轮却按记录自带的
    /// owner 把已删的 SITE 挂了回来，它整棵子树的内部边本身都是权威的。
    fn issue_10_shaped() -> (HashMap<RefU64, Vec<RefU64>>, HashMap<RefU64, Vec<RefU64>>) {
        let authoritative = HashMap::from([
            (refu(WORLD), vec![refu(LIVE_SITE)]),
            (refu(LIVE_SITE), vec![refu(LIVE_ZONE)]),
            (refu(LIVE_ZONE), Vec::new()),
            // 幽灵子树自己的成员块还在文件里，内部完全自洽。
            (refu(GHOST_SITE), vec![refu(GHOST_ZONE)]),
            (refu(GHOST_ZONE), vec![refu(GHOST_PIPE)]),
            (refu(GHOST_PIPE), Vec::new()),
        ]);
        let parsed = HashMap::from([
            // 补链轮把已删 SITE 追加到了 WORLD 末尾。
            (refu(WORLD), vec![refu(LIVE_SITE), refu(GHOST_SITE)]),
            (refu(LIVE_SITE), vec![refu(LIVE_ZONE)]),
            (refu(LIVE_ZONE), Vec::new()),
            (refu(GHOST_SITE), vec![refu(GHOST_ZONE)]),
            (refu(GHOST_ZONE), vec![refu(GHOST_PIPE)]),
            (refu(GHOST_PIPE), Vec::new()),
        ]);
        (authoritative, parsed)
    }

    fn lookup(authoritative: HashMap<RefU64, Vec<RefU64>>) -> impl Fn(RefU64) -> Vec<RefU64> {
        move |refno| authoritative.get(&refno).cloned().unwrap_or_default()
    }

    #[test]
    fn a_deleted_subtree_relinked_by_owner_is_pruned_whole() {
        let (authoritative, mut parsed) = issue_10_shaped();

        let report = prune_resurrected_members(refu(WORLD), &mut parsed, lookup(authoritative));

        assert_eq!(
            report.dropped_edges, 1,
            "只有 WORLD→已删 SITE 那条边是伪造的"
        );
        assert_eq!(
            report.dropped_elements, 3,
            "摘掉那条边后整棵幽灵子树都不可达"
        );
        assert_eq!(parsed[&refu(WORLD)], vec![refu(LIVE_SITE)]);
        assert!(!parsed.contains_key(&refu(GHOST_SITE)));
        assert!(!parsed.contains_key(&refu(GHOST_ZONE)));
        assert!(!parsed.contains_key(&refu(GHOST_PIPE)));
        assert_eq!(
            parsed[&refu(LIVE_SITE)],
            vec![refu(LIVE_ZONE)],
            "活着的子树一根汗毛都不能动"
        );
    }

    /// 成员顺序是 PDMS 语义的一部分（BRAN 组件次序），裁剪只许删不许重排。
    #[test]
    fn surviving_members_keep_their_order() {
        let authoritative = HashMap::from([
            (refu(WORLD), vec![refu(30), refu(10), refu(20)]),
            (refu(10), Vec::new()),
            (refu(20), Vec::new()),
            (refu(30), Vec::new()),
        ]);
        let mut parsed = HashMap::from([
            (refu(WORLD), vec![refu(30), refu(99), refu(10), refu(20)]),
            (refu(10), Vec::new()),
            (refu(20), Vec::new()),
            (refu(30), Vec::new()),
            (refu(99), Vec::new()),
        ]);

        prune_resurrected_members(refu(WORLD), &mut parsed, lookup(authoritative));

        assert_eq!(parsed[&refu(WORLD)], vec![refu(30), refu(10), refu(20)]);
    }

    /// 补链轮真正要救的那个毛病：owner 选中的记录不带成员块。这种 owner 没有权威，
    /// 它名下的边必须原样留着，否则 TEST 项目那种「整库解析出 0 个元素」会回来。
    #[test]
    fn an_owner_without_a_member_block_keeps_its_relinked_children() {
        let authoritative = HashMap::from([
            (refu(WORLD), vec![refu(LIVE_SITE)]),
            // 选中的记录是空的——无权威。
            (refu(LIVE_SITE), Vec::new()),
            (refu(LIVE_ZONE), Vec::new()),
        ]);
        let mut parsed = HashMap::from([
            (refu(WORLD), vec![refu(LIVE_SITE)]),
            (refu(LIVE_SITE), vec![refu(LIVE_ZONE)]),
            (refu(LIVE_ZONE), Vec::new()),
        ]);

        let report = prune_resurrected_members(refu(WORLD), &mut parsed, lookup(authoritative));

        assert!(report.is_empty());
        assert_eq!(parsed[&refu(LIVE_SITE)], vec![refu(LIVE_ZONE)]);
    }

    /// 根自己都没有成员块时整轮跳过——否则可达集只剩 WORLD，整个库会被判成幽灵。
    #[test]
    fn an_empty_root_member_block_skips_the_whole_pass() {
        let authoritative = HashMap::from([(refu(LIVE_SITE), vec![refu(LIVE_ZONE)])]);
        let mut parsed = HashMap::from([
            (refu(WORLD), vec![refu(LIVE_SITE)]),
            (refu(LIVE_SITE), vec![refu(LIVE_ZONE)]),
            (refu(LIVE_ZONE), Vec::new()),
        ]);
        let before = parsed.clone();

        let report = prune_resurrected_members(refu(WORLD), &mut parsed, lookup(authoritative));

        assert!(report.skipped_no_root_authority);
        assert!(report.is_empty());
        assert_eq!(parsed, before);
    }

    /// 没有幽灵的库必须一个字节都不动——这条守着「修复不得有副作用」。
    #[test]
    fn a_clean_parse_is_left_alone() {
        let authoritative = HashMap::from([
            (refu(WORLD), vec![refu(LIVE_SITE)]),
            (refu(LIVE_SITE), vec![refu(LIVE_ZONE)]),
            (refu(LIVE_ZONE), Vec::new()),
        ]);
        let mut parsed = authoritative.clone();
        let before = parsed.clone();

        let report = prune_resurrected_members(refu(WORLD), &mut parsed, lookup(authoritative));

        assert!(report.is_empty());
        assert_eq!(parsed, before);
    }

    /// issue #10 的现场定靶，跑在真实工程文件上。
    ///
    /// AMS 的 dbnum 7999 里 `/1WCC-PIPEBJ` 有两棵结构完全相同的子树：旧的
    /// `24383_2`（sesno 3 建、21/26/29 删）与新的 `24383_66456`（sesno 30 重建）。
    /// 上面那些纯函数用例钉的是判据，这一条钉的是「判据接到真实字节上仍然成立」
    /// ——`parse_ele_membs` 的读法一旦对不上，纯函数再绿也没有意义。
    ///
    /// 默认忽略（要一份真实工程文件）。跑法：
    /// `$env:GEN_MODEL_PRUNE_FIXTURE` 指向 `ams000/ams7999_0001`，再
    /// `cargo test --lib the_deleted_site_is_pruned_from_a_real_parse -- --ignored --nocapture`
    #[test]
    #[ignore = "manual live: requires a real E3D database file"]
    fn the_deleted_site_is_pruned_from_a_real_parse() {
        use std::path::PathBuf;
        use std::str::FromStr;

        let path = std::env::var("GEN_MODEL_PRUNE_FIXTURE")
            .expect("GEN_MODEL_PRUNE_FIXTURE 指向 ams7999_0001");
        let path = PathBuf::from(path);
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("fixture file name")
            .to_string();

        let mut db_basic =
            parse_pdms_db::parse::parse_file_db_basic_data(&path, &file_name, "AvevaMarineSample")
                .expect("parse fixture");

        let ghost = RefU64::from_str("24383_2").expect("ghost refno");
        let ghost_pipe = RefU64::from_str("24383_4").expect("ghost pipe refno");
        let live = RefU64::from_str("24383_66456").expect("live refno");
        let live_pipe = RefU64::from_str("24383_66458").expect("live pipe refno");

        assert!(
            db_basic.children_map.contains_key(&ghost),
            "裁剪前幽灵必须在——否则这条定靶什么也没验证"
        );
        assert!(db_basic.children_map.contains_key(&live));

        let world = db_basic.world_refno;
        let bytes = &db_basic.bytes;
        let refno_table_map = &db_basic.refno_table_map;
        let report = prune_resurrected_members(world, &mut db_basic.children_map, |refno| {
            authoritative_members(bytes, refno_table_map, refno)
        });
        println!("{report:?}");

        assert!(!report.skipped_no_root_authority, "根成员表必须有权威");
        assert!(
            !db_basic.children_map.contains_key(&ghost),
            "已删的 /1WCC-PIPEBJ 必须被裁掉"
        );
        assert!(
            !db_basic.children_map.contains_key(&ghost_pipe),
            "幽灵子树内部的 /1WCC1135 随之不可达，必须一并裁掉"
        );
        assert!(
            db_basic.children_map.contains_key(&live),
            "重建出来的 /1WCC-PIPEBJ 必须留下"
        );
        assert!(
            db_basic.children_map.contains_key(&live_pipe),
            "真身 /1WCC1135 必须留下"
        );
        assert!(
            !db_basic.children_map[&world].contains(&ghost),
            "WORLD 名下不得再挂着已删的 SITE"
        );
        assert!(db_basic.children_map[&world].contains(&live));
    }

    /// issue #10 端到端收口：带裁剪把真实的 dbnum 7999 解析进一个空库，
    /// `/1WCC-PIPEBJ` 必须只落一棵。
    ///
    /// 上一条钉的是解析产物，这一条钉的是**落库结果**——修复前同一份文件会写出两棵
    /// 同名子树（`pe:24383_2` 与 `pe:24383_66456`），模型树两棵都可达，用户展开靠前
    /// 的那棵幽灵，此后所有新增分支都看不见。
    ///
    /// 默认忽略。跑法（空库即可，不碰 `.surreal/` 里的真实数据）：
    /// `./scripts/Start-Surreal8009.ps1 -Memory`，再
    /// `cargo test --lib a_reparse_lands_exactly_one_site_per_name -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: requires an empty Surreal on :8009 and local AMS files"]
    async fn a_reparse_lands_exactly_one_site_per_name() {
        use crate::versioned_db::database::sync_total_async_threaded;
        use dashmap::DashSet;
        use std::sync::Arc;

        let db_option = aios_core::init_test_surreal()
            .await
            .expect("connect the empty live Surreal");

        let mut options = db_option.clone();
        options.total_sync = true;
        options.replace_dbs = true;
        options.included_db_files = Some(vec!["ams7999_0001".into()]);
        options.manual_db_nums = Some(vec![7999]);
        options.gen_model = false;
        options.gen_mesh = false;

        let parsed = sync_total_async_threaded(
            &options,
            "AvevaMarineSample",
            Arc::new(DashSet::new()),
            &["DESI"],
            100,
        )
        .await
        .expect("parse dbnum 7999");
        assert!(
            parsed.get(&7999).copied().unwrap_or_default() > 0,
            "dbnum 7999 必须解析出元素，否则下面的断言是空的"
        );

        let mut response = aios_core::SUL_DB
            .query(
                "SELECT VALUE record::id(id) FROM pe \
                 WHERE dbnum = 7999 AND noun = 'SITE' AND name = '/1WCC-PIPEBJ';",
            )
            .await
            .expect("query sites")
            .check()
            .expect("valid site query");
        let sites: Vec<String> = response.take(0).expect("site rows");
        assert_eq!(
            sites,
            vec!["24383_66456".to_string()],
            "/1WCC-PIPEBJ 只该剩下 sesno 30 重建的那棵"
        );

        let mut response = aios_core::SUL_DB
            .query(
                "SELECT VALUE record::id(id) FROM pe \
                 WHERE dbnum = 7999 AND noun = 'PIPE' AND name = '/1WCC1135';",
            )
            .await
            .expect("query pipes")
            .check()
            .expect("valid pipe query");
        let pipes: Vec<String> = response.take(0).expect("pipe rows");
        assert_eq!(
            pipes,
            vec!["24383_66458".to_string()],
            "截图里那条管子也只该剩真身"
        );

        // 幽灵整棵不可达，连 pe 行都不该写出来。
        let mut response = aios_core::SUL_DB
            .query("RETURN [record::exists(pe:24383_2), record::exists(pe:24383_4)];")
            .await
            .expect("query ghosts")
            .check()
            .expect("valid ghost query");
        let ghosts: Vec<bool> = response.take(0).expect("ghost flags");
        assert_eq!(ghosts, vec![false, false], "已删子树不得留下任何 pe 行");
    }
}
