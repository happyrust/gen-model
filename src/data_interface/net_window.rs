//! 净窗口收集（ADR-022）：由会话索引差分给出窗口净三态，再按窗口两端记录版本
//! 合成与逐会话回放**相同形状**的操作流——`BTreeMap<sesno, Vec<EleOperationData>>`，
//! 每个 refno 恰一条：
//!
//! * 净新增 → `Add(终稿 EleData)`（终稿记录一次解析）；
//! * 净删除 → `Deleted`（挂窗口终点会话：净差分判不出删除动作发生在哪个会话，
//!   墓碑语义在终点已成立）；
//! * 净修改 → `Modified(ModifiedElement)`：base / 终稿两端版本各解析一次、
//!   **一次 diff** 合成（[`diff_ele_data`]）。两端内容相同（记录被原样重写换页）
//!   时不发操作——真无事发生，计入 [`NetWindowOutcome::unchanged_rewrites`]。
//!
//! 于是模型计划、交付单元归并、ref_rev 维护、MySQL 同步、语句渲染的输入形状
//! **零改动**；回放路径的 `fold_window` 与两个终态补丁在本路径上没有输入。
//!
//! 与会话索引差分同一条**纯文件纪律**：不查库、不读水位，窗口由调用方显式给定
//! （见 `the_net_window_module_never_touches_the_database` 源码断言）。

use std::collections::{BTreeMap, HashMap};
use std::ops::RangeInclusive;

use aios_core::pdms_types::RefU64;
use parse_pdms_db::parse::EleData;
pub use pdms_io::io::diff_ele_data;
use pdms_io::io::{EleOperationData, EleOperationDetail, PdmsIO};

use crate::data_interface::session_index_diff::{self, RecordLoc};

/// 一次净窗口收集的产物。
pub struct NetWindowOutcome {
    /// 与回放收集同形状的操作流（每 refno 恰一条，挂 last-touch 会话）。
    pub window: BTreeMap<u32, Vec<EleOperationData>>,
    /// 必须进回执的收集警告（如「基版本解析失败，按新增全量处理」）——
    /// 静默失效是最高级别缺陷，调用方不得丢弃。
    pub warnings: Vec<String>,
    /// 记录位置变了但内容逐字段相同（原样重写换页）的条目数：不发操作，
    /// 但账要看得见。
    pub unchanged_rewrites: usize,
    /// 终稿记录解析失败而跳过的条目数（多为字典缺项的系统记录，如 MNUM 不在
    /// 属性表——回放路径对同一批记录同样以 `None` 操作落空，从未入过库）。
    /// 明细以聚合警告随回执透出。cea58087（08-14）曾把它升为整窗硬错误，
    /// 真实文件上每个含系统段的窗口整批打死（08-17 实测 ams8000 的 `16192_1`
    /// 必现），已回退为与回放等价的跳过 + 记账。
    pub unparseable_finals: usize,
    /// 差分统计（页读数/剪枝/耗时），随回执与日志透出。
    pub stats: session_index_diff::NetChangeStats,
}

/// 对一个已打开（或可打开）的库文件做净窗口收集。
///
/// 失败语义（与回放口径逐条对齐，不许静默）：
///
/// * 净新增 / 净修改的**终稿**记录解析失败 → 跳过该条 + 计数 + 聚合警告。
///   真实库里这是字典缺项的系统记录家族（如 `MNUM not exist in attr_info_map`，
///   ams8000 的 `16192_1`）——回放路径对同一批记录以 `None` 操作落空、从未
///   入库，硬失败会让每个含系统段的窗口整批打死，而跳过与回放行为等价。
/// * 净修改的**基版本**解析失败（终稿可读）→ 按 spec §Edge Cases 保守处理：
///   当作新增全量覆盖（模型侧整根重生成），warnings 逐条点名。
/// * 净修改条目缺 `base_loc` → **硬失败**：那是差分分类的不变量被破坏，不是
///   现场异常，不许降级。
pub fn collect_net_window(
    io: &mut PdmsIO,
    sesno_range: RangeInclusive<i32>,
) -> anyhow::Result<NetWindowOutcome> {
    let net = session_index_diff::collect_net_changes(io, sesno_range, false)?;
    synthesize_net_window(net, |loc| io.parse_raw_element(loc.att_offset()))
}

/// 纯合成层：净三态 → 与回放同形状的操作流。**不碰 IO、不碰库**——记录解析由
/// `resolve` 注入（生产是 `PdmsIO::parse_raw_element`，单测是内存桩），于是三
/// 形状与三条降级路径全部进得了 CI 纯单测（ADR-022 验收 1）。
///
/// `net` 按值接收：`stats` 直接移交 [`NetWindowOutcome`]，条目也不必逐条 clone。
/// resolver 收窄成「给我这个位置的记录」，「谁的记录 / 哪一端 / 页与偏移」由
/// [`resolve_record`] 包装——错误文案只有一处权威，测试不必复刻它。
fn synthesize_net_window<F>(
    net: session_index_diff::NetChangeSet,
    mut resolve: F,
) -> anyhow::Result<NetWindowOutcome>
where
    F: FnMut(RecordLoc) -> anyhow::Result<EleData>,
{
    let target_sesno = net.target_sesno.max(0) as u32;
    let session_index_diff::NetChangeSet {
        added,
        deleted,
        modified,
        stats,
        ..
    } = net;

    let mut window: BTreeMap<u32, Vec<EleOperationData>> = BTreeMap::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut unchanged_rewrites = 0usize;
    let mut unparseable: Vec<String> = Vec::new();
    let mut push = |window: &mut BTreeMap<u32, Vec<EleOperationData>>,
                    sesno: u32,
                    refno: RefU64,
                    detail: EleOperationDetail| {
        window
            .entry(sesno)
            .or_default()
            .push(EleOperationData::new(refno, sesno, detail));
    };

    for entry in added {
        let sesno = u32::try_from(
            entry
                .last_touch_sesno
                .ok_or_else(|| anyhow::anyhow!("净新增 {} 缺 last-touch 会话", entry.refno))?,
        )?;
        match resolve_record(&mut resolve, entry.refno, entry.loc, "终稿") {
            Ok(data) => push(
                &mut window,
                sesno,
                entry.refno,
                EleOperationDetail::Add(data),
            ),
            Err(error) => unparseable.push(format!("{}: {error:#}", entry.refno)),
        }
    }

    for entry in deleted {
        push(
            &mut window,
            target_sesno,
            entry.refno,
            EleOperationDetail::Deleted,
        );
    }

    for entry in modified {
        let sesno = u32::try_from(
            entry
                .last_touch_sesno
                .ok_or_else(|| anyhow::anyhow!("净修改 {} 缺 last-touch 会话", entry.refno))?,
        )?;
        let latest = match resolve_record(&mut resolve, entry.refno, entry.loc, "终稿") {
            Ok(latest) => latest,
            Err(error) => {
                unparseable.push(format!("{}: {error:#}", entry.refno));
                continue;
            }
        };
        let base_loc = entry.base_loc.ok_or_else(|| {
            anyhow::anyhow!(
                "净修改条目 {} 缺 base 位置——classify 的不变量被破坏",
                entry.refno
            )
        })?;
        match resolve_record(&mut resolve, entry.refno, base_loc, "基版本") {
            Ok(base) => match diff_ele_data(&base, &latest) {
                Some(modified) => push(
                    &mut window,
                    sesno,
                    entry.refno,
                    EleOperationDetail::Modified(modified),
                ),
                None => unchanged_rewrites += 1,
            },
            Err(error) => {
                warnings.push(format!(
                    "净修改 {} 的基版本解析失败，按新增全量处理（保守整根重生成）: {error:#}",
                    entry.refno
                ));
                push(
                    &mut window,
                    sesno,
                    entry.refno,
                    EleOperationDetail::Add(latest),
                );
            }
        }
    }

    if !unparseable.is_empty() {
        let samples = unparseable
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("；");
        warnings.push(format!(
            "{} 条记录终稿解析失败，按回放同口径跳过（这些记录在回放路径同样以 None \
             操作落空、从未入库，多为字典缺项的系统记录）。样例：{samples}",
            unparseable.len()
        ));
    }

    Ok(NetWindowOutcome {
        window,
        warnings,
        unchanged_rewrites,
        unparseable_finals: unparseable.len(),
        stats,
    })
}

/// 给一次记录解析套上「谁的记录、哪一端、页与偏移」——出错时光有底层报错认不出
/// 是哪条元素的哪一端。
fn resolve_record<F>(
    resolve: &mut F,
    refno: RefU64,
    loc: RecordLoc,
    side: &str,
) -> anyhow::Result<EleData>
where
    F: FnMut(RecordLoc) -> anyhow::Result<EleData>,
{
    resolve(loc).map_err(|error| {
        anyhow::anyhow!(
            "解析 {refno} 的{side}记录（页 {} 偏移 {}）失败: {error}",
            loc.pgno,
            loc.offset
        )
    })
}

/// The two-version element comparison is owned by `pdms_io::io::diff_ele_data` and
/// re-exported above. Net-window synthesis and legacy session replay therefore share
/// one attribute / explicit-attribute / UDA / ordered-child diff implementation.
/// The cross-collector property tests below pin the rendered payload equivalence.

#[cfg(test)]
mod tests {
    use super::*;
    use aios_core::NamedAttrValue;

    /// T14：净窗口不得再长出一份属性/成员 diff；编译期 import 保证共享函数存在，
    /// 源码断言保证这里没有悄悄复制回来一份同名实现。
    #[test]
    fn net_window_uses_the_vendor_element_diff_single_source() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/data_interface/net_window.rs"
        ));
        let shared_import = concat!("pub use pdms_io::io::", "diff_ele_data;");
        let local_definition = concat!("pub fn diff_", "ele_data(");
        assert!(
            source.contains(shared_import),
            "净窗口必须直接复用 pdms-io 的共享元素 diff"
        );
        assert_eq!(
            source.matches(local_definition).count(),
            0,
            "net_window.rs 不得重新定义第二份 diff_ele_data"
        );
    }

    fn element(pairs: &[(&str, &str)], children: &[u64]) -> EleData {
        let mut data = EleData::default();
        for (name, value) in pairs {
            data.att_map_mut().map.insert(
                (*name).to_owned(),
                NamedAttrValue::StringType((*value).into()),
            );
        }
        for &child in children {
            data.children.0.push(RefU64(child));
        }
        data
    }

    /// 两端逐字段相同 = 原样重写换页，真无事发生：不合成操作。
    #[test]
    fn identical_versions_diff_to_none() {
        let prev = element(&[("TYPE", "BOX"), ("XLEN", "100")], &[7]);
        let latest = element(&[("TYPE", "BOX"), ("XLEN", "100")], &[7]);
        assert!(diff_ele_data(&prev, &latest).is_none());
    }

    /// 三个桶各归各位：改值进 modified（旧值在前）、新键进 added、消失的键进
    /// deleted；noun 与 current_data 取终稿端。
    #[test]
    fn attribute_buckets_carry_old_and_new_values() {
        let prev = element(&[("TYPE", "BOX"), ("XLEN", "100"), ("GONE", "1")], &[]);
        let latest = element(&[("TYPE", "BOX"), ("XLEN", "200"), ("NEW", "9")], &[]);

        let modified = diff_ele_data(&prev, &latest).expect("有净变化");

        assert_eq!(
            modified.modified_attrs.get("XLEN"),
            Some(&(
                NamedAttrValue::StringType("100".into()),
                NamedAttrValue::StringType("200".into())
            )),
            "修改桶必须携带（旧值, 新值）"
        );
        assert_eq!(
            modified.added_attrs.get("NEW"),
            Some(&NamedAttrValue::StringType("9".into()))
        );
        assert_eq!(
            modified.deleted_attrs.get("GONE"),
            Some(&NamedAttrValue::StringType("1".into()))
        );
        assert!(modified.children_changed.is_none());
    }

    /// 纯 children 变化（含重排）也必须发 Modified——渲染端靠 children_changed
    /// 做 pe_owner 全量替换，成员增删的信号只在这里。
    #[test]
    fn children_only_change_still_emits_modified() {
        let prev = element(&[("TYPE", "ZONE")], &[1, 2]);
        let latest = element(&[("TYPE", "ZONE")], &[2, 1]);

        let modified = diff_ele_data(&prev, &latest).expect("children 重排是净变化");
        let (old, new) = modified.children_changed.expect("children 两端都要带");
        assert_eq!(old.0, vec![RefU64(1), RefU64(2)]);
        assert_eq!(new.0, vec![RefU64(2), RefU64(1)]);
        assert!(modified.added_attrs.is_empty());
    }

    // ── 纯合成层：三形状 + 两条降级 + 一条硬失败 + 原样重写（ADR-022 验收 1）──
    //
    // 注入 resolver 之后这些分支全部不碰文件、不碰库，是 CI 里常驻的那一份；
    // 真实负载的等价性另有 `db8000_session_pairs` 性质 i 与 live 对拍兜底。

    use crate::data_interface::session_index_diff::{NetChangeSet, NetChangeStats, NetEntry};

    fn at(pgno: u32, offset: u32) -> RecordLoc {
        RecordLoc { pgno, offset }
    }

    fn net_entry(
        refno: u64,
        loc: RecordLoc,
        base_loc: Option<RecordLoc>,
        last_touch_sesno: Option<i32>,
    ) -> NetEntry {
        NetEntry {
            refno: RefU64(refno),
            loc,
            base_loc,
            last_touch_sesno,
            noun: None,
        }
    }

    fn change_set(target_sesno: i32) -> NetChangeSet {
        NetChangeSet {
            requested_start: 1,
            requested_end: target_sesno,
            base_sesno: Some(1),
            target_sesno,
            added: Vec::new(),
            deleted: Vec::new(),
            modified: Vec::new(),
            stats: NetChangeStats::default(),
        }
    }

    /// 只认位置的记录桩：桩里没有的位置就是解析失败——真实库里那是字典缺项的
    /// 系统记录（`MNUM not exist in attr_info_map`）。
    fn records(
        known: Vec<(RecordLoc, EleData)>,
    ) -> impl FnMut(RecordLoc) -> anyhow::Result<EleData> {
        move |wanted| {
            known
                .iter()
                .find(|(loc, _)| *loc == wanted)
                .map(|(_, data)| data.clone())
                .ok_or_else(|| anyhow::anyhow!("MNUM not exist in attr_info_map"))
        }
    }

    /// 净新增挂在它的 last-touch 会话上，不是窗口终点；`stats` 原样移交回执。
    #[test]
    fn a_net_added_entry_becomes_an_add_on_its_last_touch_session() {
        let mut net = change_set(30);
        net.added.push(net_entry(7, at(10, 0), None, Some(12)));
        net.stats.elapsed_ms = 42;

        let outcome = synthesize_net_window(
            net,
            records(vec![(at(10, 0), element(&[("TYPE", "BOX")], &[]))]),
        )
        .expect("合成");

        let ops = outcome.window.get(&12).expect("挂 last-touch 会话 12");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].refno, RefU64(7));
        assert_eq!(ops[0].sesno, 12);
        match &ops[0].detail {
            EleOperationDetail::Add(data) => assert_eq!(data.att_map().get_type(), "BOX"),
            other => panic!("净新增必须合成 Add，得到 {other:?}"),
        }
        assert!(
            !outcome.window.contains_key(&30),
            "不得挂到窗口终点会话上：{:?}",
            outcome.window.keys().collect::<Vec<_>>()
        );
        assert!(outcome.warnings.is_empty());
        assert_eq!(outcome.stats.elapsed_ms, 42, "差分统计要原样带进回执");
    }

    /// 净删除挂窗口终点会话——净差分判不出删除动作发生在哪个会话，`last_touch`
    /// 说的是**旧版本**所在会话，拿它当删除时刻就是编一个看着像真的数。
    /// 顺带钉住：删除不解析任何记录。
    #[test]
    fn a_net_deleted_entry_hangs_on_the_window_end_session() {
        let mut net = change_set(30);
        net.deleted.push(net_entry(8, at(11, 4), None, Some(12)));

        let outcome = synthesize_net_window(net, records(Vec::new())).expect("合成");

        assert!(
            !outcome.window.contains_key(&12),
            "不得挂在旧版本所在会话上"
        );
        let ops = outcome.window.get(&30).expect("挂窗口终点会话 30");
        assert!(matches!(ops[0].detail, EleOperationDetail::Deleted));
        assert_eq!(ops[0].sesno, 30);
        assert_eq!(
            outcome.unparseable_finals, 0,
            "删除条目不该去解析记录（桩是空的，解析了就会计数）"
        );
    }

    /// 净修改：两端各解析**恰一次**（终稿在前、基版本在后），一次 diff 合成。
    #[test]
    fn a_net_modified_entry_diffs_both_versions_exactly_once() {
        let mut net = change_set(30);
        net.modified
            .push(net_entry(9, at(20, 0), Some(at(19, 0)), Some(25)));
        let base = element(&[("TYPE", "BOX"), ("XLEN", "100")], &[]);
        let latest = element(&[("TYPE", "BOX"), ("XLEN", "200")], &[]);

        let mut seen: Vec<RecordLoc> = Vec::new();
        let outcome = synthesize_net_window(net, |wanted| {
            seen.push(wanted);
            if wanted == at(20, 0) {
                Ok(latest.clone())
            } else if wanted == at(19, 0) {
                Ok(base.clone())
            } else {
                anyhow::bail!("桩里没有 {wanted:?}")
            }
        })
        .expect("合成");

        assert_eq!(
            seen,
            vec![at(20, 0), at(19, 0)],
            "两端各解析一次、终稿在前；多解析一次就是白付一趟记录解析"
        );
        let ops = outcome.window.get(&25).expect("挂 last-touch 会话 25");
        match &ops[0].detail {
            EleOperationDetail::Modified(modified) => assert_eq!(
                modified.modified_attrs.get("XLEN"),
                Some(&(
                    NamedAttrValue::StringType("100".into()),
                    NamedAttrValue::StringType("200".into())
                ))
            ),
            other => panic!("净修改必须合成 Modified，得到 {other:?}"),
        }
        assert_eq!(outcome.unchanged_rewrites, 0);
        assert!(outcome.warnings.is_empty());
    }

    /// 基版本读不出来 = 拿不到差集，按新增全量覆盖（宁多勿漏，模型侧整根重生成），
    /// 并逐条点名——降级可以，静默不行。
    #[test]
    fn a_base_parse_failure_degrades_to_add_and_names_the_refno() {
        let mut net = change_set(30);
        net.modified
            .push(net_entry(9, at(20, 0), Some(at(19, 0)), Some(25)));

        let outcome = synthesize_net_window(
            net,
            records(vec![(at(20, 0), element(&[("TYPE", "BOX")], &[]))]),
        )
        .expect("合成");

        let ops = outcome.window.get(&25).expect("降级后照样要有操作");
        assert!(
            matches!(ops[0].detail, EleOperationDetail::Add(_)),
            "基版本读不了就整条按新增覆盖，不许退化成不发操作"
        );
        assert_eq!(outcome.warnings.len(), 1);
        let warning = &outcome.warnings[0];
        assert!(
            warning.contains(&RefU64(9).to_string()) && warning.contains("基版本"),
            "降级警告必须点名 refno 与降级原因: {warning}"
        );
        assert_eq!(outcome.unparseable_finals, 0, "失败的是基版本不是终稿");
    }

    /// 终稿解析不出来：跳过 + 计数 + **聚合**警告（回放路径对同一批记录同样以
    /// `None` 落空、从未入库）。逐条刷屏会把回执淹掉，所以明细走样例。
    ///
    /// 回归背景：cea58087（08-14）曾把它升为整窗硬错误，而字典缺项的系统记录在
    /// 真实文件上必现（08-17 实测 ams8000 的 `16192_1` 报 `MNUM not exist in
    /// attr_info_map`），升硬错误等于每个含系统段的窗口整批打死。本测试若因
    /// 「整窗失败」变红，说明容忍又被改掉了。
    #[test]
    fn an_unparseable_final_is_skipped_counted_and_aggregated() {
        let mut net = change_set(30);
        net.added.push(net_entry(7, at(10, 0), None, Some(12)));
        net.modified
            .push(net_entry(9, at(20, 0), Some(at(19, 0)), Some(25)));

        let outcome = synthesize_net_window(net, records(Vec::new())).expect("合成");

        assert!(
            outcome.window.is_empty(),
            "解析不出终稿的条目一条都不许入窗口: {:?}",
            outcome.window.keys().collect::<Vec<_>>()
        );
        assert_eq!(outcome.unparseable_finals, 2);
        assert_eq!(outcome.warnings.len(), 1, "明细走聚合警告，不逐条刷屏");
        let warning = &outcome.warnings[0];
        assert!(warning.contains("2 条"), "聚合警告要报条数: {warning}");
        assert!(
            warning.contains(&RefU64(7).to_string()) && warning.contains(&RefU64(9).to_string()),
            "聚合警告要带样例 refno: {warning}"
        );
    }

    #[test]
    fn a_missing_last_touch_session_fails_instead_of_using_the_window_end() {
        let mut net = change_set(30);
        net.added.push(net_entry(7, at(10, 0), None, None));
        let error = synthesize_net_window(
            net,
            records(vec![(at(10, 0), element(&[("TYPE", "BOX")], &[]))]),
        )
        .err()
        .expect("last-touch 缺失必须整窗失败");
        assert!(format!("{error:#}").contains("last-touch"));
    }

    /// `base_loc` 缺失是**差分分类的不变量被破坏**，不是现场异常：硬失败整批，
    /// 不许按新增或跳过降级——那会把一个逻辑缺陷变成一批悄悄错的数据。
    #[test]
    fn a_missing_base_loc_fails_hard_and_names_the_refno() {
        let mut net = change_set(30);
        net.modified.push(net_entry(9, at(20, 0), None, Some(25)));

        let outcome = synthesize_net_window(
            net,
            records(vec![(at(20, 0), element(&[("TYPE", "BOX")], &[]))]),
        );

        let Err(error) = outcome else {
            panic!("缺 base 位置必须硬失败，不许降级成一条看着像真的操作");
        };
        let text = format!("{error:#}");
        assert!(
            text.contains(&RefU64(9).to_string()) && text.contains("不变量"),
            "硬失败必须点名 refno 与原因: {text}"
        );
    }

    /// 记录换了页但两端逐字段相同（Save Work 原样重写）：不发操作——这不是降级，
    /// 是正常判定的正常结果；但账要看得见。
    #[test]
    fn an_identical_rewrite_emits_nothing_but_is_counted() {
        let mut net = change_set(30);
        net.modified
            .push(net_entry(9, at(20, 0), Some(at(19, 0)), Some(25)));
        let same = element(&[("TYPE", "BOX"), ("XLEN", "100")], &[7]);

        let outcome = synthesize_net_window(
            net,
            records(vec![(at(20, 0), same.clone()), (at(19, 0), same)]),
        )
        .expect("合成");

        assert!(outcome.window.is_empty(), "两端相同 = 真无事发生");
        assert_eq!(outcome.unchanged_rewrites, 1);
        assert!(
            outcome.warnings.is_empty(),
            "原样重写不是降级路径，不该有警告: {:?}",
            outcome.warnings
        );
    }

    /// live：真实 ams8000 上净窗口收集器与逐会话回放对拍。三态在差分层已由点查
    /// 仲裁（session_index_diff 的 live 用例），这里验的是**负载合成**：单触达
    /// refno 的 Add 渲染逐字符相等（Add 无引用型条目乱序问题，`to_surql` 全程
    /// 确定性），Modified 三桶键集 + children 两端 + noun 相等。
    /// 前置：本机存在 ams8000_0001（`AIOS_AMS8000_FILE` 可覆盖）；仓库根运行。
    #[cfg(feature = "legacy_session_replay")]
    #[test]
    #[ignore = "manual live: needs the real ams8000_0001 file on this machine"]
    fn live_ams8000_net_window_payloads_match_replay_on_single_touch_refnos() {
        use std::collections::BTreeSet;
        use std::path::PathBuf;
        use std::time::Instant;

        let path = std::env::var_os("AIOS_AMS8000_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001")
            });
        assert!(path.is_file(), "找不到 ams8000 文件: {}", path.display());

        let mut io = PdmsIO::new("", path.clone(), true);
        io.open().expect("open ams8000");
        let latest = io.get_latest_sesno().expect("latest sesno") as i32;

        let mut add_compared = 0usize;
        let mut modified_compared = 0usize;
        for start in [1, latest / 2, (latest - 5).max(1)] {
            let window = start..=latest;
            let net_started = Instant::now();
            let outcome = collect_net_window(&mut io, window.clone()).expect("net window");
            let net_ms = net_started.elapsed().as_millis();

            let replay_started = Instant::now();
            let replay =
                crate::data_interface::increment_pipeline::IncrementPipeline::collect_changes(
                    &path,
                    window.clone(),
                )
                .expect("replay collect");
            let replay_ms = replay_started.elapsed().as_millis();

            let mut net_ops: HashMap<RefU64, &EleOperationData> = HashMap::new();
            for operation in outcome.window.values().flatten() {
                net_ops.insert(operation.refno, operation);
            }
            let mut touch_counts: HashMap<RefU64, usize> = HashMap::new();
            for operation in replay.values().flatten() {
                if !matches!(operation.detail, EleOperationDetail::None) {
                    *touch_counts.entry(operation.refno).or_default() += 1;
                }
            }

            let mut window_adds = 0usize;
            let mut window_modified = 0usize;
            for (sesno, operations) in &replay {
                for replay_op in operations {
                    if touch_counts.get(&replay_op.refno) != Some(&1) {
                        continue;
                    }
                    let Some(net_op) = net_ops.get(&replay_op.refno) else {
                        continue; // 类别分歧属回放盲区，由差分层 live 仲裁用例管
                    };
                    if net_op.sesno != *sesno {
                        continue;
                    }
                    match (&replay_op.detail, &net_op.detail) {
                        (EleOperationDetail::Add(_), EleOperationDetail::Add(_)) => {
                            let id = replay_op.refno.to_string();
                            assert_eq!(
                                net_op.to_surql(&id, 8000, *sesno),
                                replay_op.to_surql(&id, 8000, *sesno),
                                "Add {} 的渲染不一致（终稿合成漂移）",
                                replay_op.refno
                            );
                            window_adds += 1;
                        }
                        (
                            EleOperationDetail::Modified(replay_modified),
                            EleOperationDetail::Modified(net_modified),
                        ) => {
                            let keys = |map: &HashMap<String, aios_core::NamedAttrValue>| {
                                map.keys().cloned().collect::<BTreeSet<_>>()
                            };
                            let pair_keys = |map: &HashMap<
                                String,
                                (aios_core::NamedAttrValue, aios_core::NamedAttrValue),
                            >| {
                                map.keys().cloned().collect::<BTreeSet<_>>()
                            };
                            assert_eq!(
                                keys(&net_modified.added_attrs),
                                keys(&replay_modified.added_attrs),
                                "Modified {} 的 added 键集不一致",
                                replay_op.refno
                            );
                            assert_eq!(
                                pair_keys(&net_modified.modified_attrs),
                                pair_keys(&replay_modified.modified_attrs),
                                "Modified {} 的 modified 键集不一致",
                                replay_op.refno
                            );
                            assert_eq!(
                                keys(&net_modified.deleted_attrs),
                                keys(&replay_modified.deleted_attrs),
                                "Modified {} 的 deleted 键集不一致",
                                replay_op.refno
                            );
                            assert_eq!(
                                net_modified.noun, replay_modified.noun,
                                "Modified {} 的 noun 不一致",
                                replay_op.refno
                            );
                            assert_eq!(
                                net_modified
                                    .children_changed
                                    .as_ref()
                                    .map(|(old, new)| (old.0.clone(), new.0.clone())),
                                replay_modified
                                    .children_changed
                                    .as_ref()
                                    .map(|(old, new)| (old.0.clone(), new.0.clone())),
                                "Modified {} 的 children 两端不一致",
                                replay_op.refno
                            );
                            window_modified += 1;
                        }
                        _ => {}
                    }
                }
            }
            add_compared += window_adds;
            modified_compared += window_modified;
            println!(
                "[live] 窗口 {start}..={latest}: 净收集 {net_ms}ms vs 回放 {replay_ms}ms；\
                 负载对拍 Add {window_adds} / Modified {window_modified}，原样重写跳过 {}，\
                 收集警告 {} 条",
                outcome.unchanged_rewrites,
                outcome.warnings.len()
            );
            for warning in &outcome.warnings {
                println!("[live]   收集警告: {warning}");
            }
        }
        assert!(
            add_compared > 1000,
            "Add 负载对拍样本太少（{add_compared}），窗口选择失效"
        );
        // Modified 允许为 0：真实文件当前形状下单触达修改稀少（多触达的回放
        // 逐会话 diff 与净端两端 diff 合法不同，不可逐桶比对）；Modified 负载
        // 等价由 db8000_session_pairs 性质 i 在 CI 常驻钉住。
        println!("[live] 合计负载对拍：Add {add_compared} / Modified {modified_compared}");
    }

    /// T18 记录项（ADR-031，非门）：release 下按协议计时完整收集。
    /// 1 warmup + 5 次，报 median / min / p95；冷启动另报；两类窗口。
    #[cfg(feature = "legacy_session_replay")]
    #[test]
    #[ignore = "manual live: release timing for the single-caliber collector"]
    fn live_ams8000_single_caliber_release_timing() {
        use std::path::PathBuf;
        use std::time::Instant;

        let path = std::env::var_os("AIOS_AMS8000_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let testbed = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("python/testbed/projects/AvevaMarineSample/ams000/ams8000_0001");
                if testbed.is_file() {
                    testbed
                } else {
                    PathBuf::from(r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001")
                }
            });
        assert!(path.is_file(), "找不到计时文件: {}", path.display());

        let latest = {
            let mut io = PdmsIO::new("", path.clone(), true);
            io.open().expect("open");
            io.get_latest_sesno().expect("latest") as i32
        };
        let windows = [
            ("high-retouch", (latest / 2).max(1)..=latest),
            ("add-floor", 1..=latest),
        ];

        fn stats(samples: &[u128]) -> (u128, u128, u128) {
            let mut sorted = samples.to_vec();
            sorted.sort_unstable();
            let n = sorted.len();
            let median = sorted[n / 2];
            let min = sorted[0];
            let p95_idx = ((n as f64) * 0.95).ceil() as usize - 1;
            (median, min, sorted[p95_idx.min(n - 1)])
        }

        println!(
            "[T18] file={} sha-skipped latest={latest} cfg={}",
            path.display(),
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        );
        for (name, window) in windows {
            let mut net_samples = Vec::new();
            let mut replay_samples = Vec::new();
            let cold_net = Instant::now();
            let net0 =
                crate::data_interface::increment_pipeline::IncrementPipeline::collect_window(
                    &path,
                    window.clone(),
                )
                .expect("net cold");
            let cold_net_ms = cold_net.elapsed().as_millis();
            let cold_replay = Instant::now();
            let replay0 =
                crate::data_interface::increment_pipeline::IncrementPipeline::collect_changes(
                    &path,
                    window.clone(),
                )
                .expect("replay cold");
            let cold_replay_ms = cold_replay.elapsed().as_millis();

            for _ in 0..5 {
                let started = Instant::now();
                let _ =
                    crate::data_interface::increment_pipeline::IncrementPipeline::collect_window(
                        &path,
                        window.clone(),
                    )
                    .expect("net warm");
                net_samples.push(started.elapsed().as_millis());
                let started = Instant::now();
                let _ =
                    crate::data_interface::increment_pipeline::IncrementPipeline::collect_changes(
                        &path,
                        window.clone(),
                    )
                    .expect("replay warm");
                replay_samples.push(started.elapsed().as_millis());
            }

            let net_ops = net0.range_eles.values().flatten().count();
            let replay_ops = replay0
                .values()
                .flatten()
                .filter(|op| !matches!(op.detail, EleOperationDetail::None))
                .count();
            let retouch = if net_ops == 0 {
                0.0
            } else {
                replay_ops as f64 / net_ops as f64
            };
            let (n_med, n_min, n_p95) = stats(&net_samples);
            let (r_med, r_min, r_p95) = stats(&replay_samples);
            let ratio = if n_med == 0 {
                f64::INFINITY
            } else {
                r_med as f64 / n_med as f64
            };
            println!(
                "[T18] {name} {window:?} sessions~{} net_ops={net_ops} replay_ops={replay_ops} \
                 retouch={retouch:.2} cold_net={cold_net_ms}ms cold_replay={cold_replay_ms}ms \
                 net median/min/p95={n_med}/{n_min}/{n_p95}ms \
                 replay median/min/p95={r_med}/{r_min}/{r_p95}ms ratio≈{ratio:.1}×",
                *window.end() - *window.start() + 1
            );
        }
    }

    /// 纯文件纪律钉死（与 session_index_diff 同款）：净窗口收集不许出现任何
    /// 数据库访问；窗口由调用方给定，不读水位。
    #[test]
    fn the_net_window_module_never_touches_the_database() {
        let source = include_str!("net_window.rs");
        let forbidden = [
            concat!("SUL", "_DB"),
            concat!(".que", "ry("),
            concat!("surreal", "db"),
        ];
        for needle in forbidden {
            assert_eq!(
                source.matches(needle).count(),
                0,
                "净窗口收集必须纯文件：源码里不得出现 {needle}"
            );
        }
    }
}
