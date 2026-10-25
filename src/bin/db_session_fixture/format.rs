//! `aios-session-fixture-v1`：一条会话链（只入库最终文件）承载 N 个录制案例。
//!
//! - `recording.json`（录制脚本产出）：dbnum、baseline_sesno、按执行顺序排列的案例。
//! - `manifest.json`（pack 产出）：在 recording 之上补齐档案与还原台账
//!   （最终文件 zip 的 SHA256、每个关键 sesno 切割快照的 SHA256）。
//!
//! 历史快照不入库：回归测试运行时用 `session_cut` 从最终文件现切，再拿台账对账，
//! 这就是「任意历史可还原」性质的载体。
//!
//! **字节补丁**（2026-10-25）：dabacon 不是纯 append-only：每写一个新会话，E3D 会就地改前一个会话页的 `end` /
//! `claim` 字（`end` 多往后指一页，所以从终稿切出来的 cut(S) 比真机上 S 刚存完的文件**长一页**）、前一会话的 claim 页，
//! 和头页的几个计数。于是 cut(S) 里 S 的会话页已经是被 S+1 改过的样子，与真机上的字节不同——而冻结源的 anchor 正是这一页
//! 的散列（`frozen_source::target_anchor`）。录制时把每个会话时刻的真文件多拷一份（`Recording::snapshots`），pack 就把
//! 「真文件 − cut(S)」记成台账里的长度（`bytes`，真文件的）+ 几个 [`BytePatch`]；台账散列、verify 的现切与
//! `replay-init / replay-step` 都是 cut → 截到 `bytes` → 打补丁。没有快照的 sesno 没有补丁、长度就是 cut 的，行为与从前一样。

use aios_core::pdms_types::RefU64;
use anyhow::{Context, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Component, Path};
use std::str::FromStr;

pub const FORMAT: &str = "aios-session-fixture-v1";
pub const MAX_ARCHIVE_BYTES: u64 = 6 * 1024 * 1024;
pub const COMPRESSION: &str = "zip-deflate-level-9";

/// 窗口净变化的合法取值，对应 `manual_update::NetOp` 的 snake_case 序列化。
pub const NET_OPS: [&str; 4] = ["added", "modified", "deleted", "cancelled"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFixtureManifest {
    pub format: String,
    pub dbnum: u32,
    pub baseline_sesno: u32,
    pub archive: ArchiveSpec,
    /// zip 内唯一条目：最终会话时刻的完整 DB 文件。
    #[serde(rename = "final")]
    pub final_snapshot: FinalSnapshotSpec,
    /// 还原台账：pack 时切割并散列过的每个关键 sesno（含 baseline 与 final）。
    pub session_snapshots: Vec<SessionSnapshotSpec>,
    pub cases: Vec<CaseSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveSpec {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub compression: String,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalSnapshotSpec {
    /// zip 内路径（`final/<原文件名>`）。
    pub path: String,
    pub sesno: u32,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshotSpec {
    pub sesno: u32,
    /// 忠实快照的大小与散列：有真快照时是真文件的（cut 截到这个长度再打补丁），没有时就是 cut 本身。
    pub bytes: u64,
    pub sha256: String,
    /// 真文件在这个会话时刻与 cut(sesno) 不同的字节（见模块注）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patches: Vec<BytePatch>,
}

/// 一段连续的字节差异：终稿切出来是 `final_hex`，真机上当时是 `real_hex`（两者等长）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BytePatch {
    pub page: u32,
    pub offset: u32,
    pub final_hex: String,
    pub real_hex: String,
}

/// 补丁总字节数上限：真正的就地改写只有几个字；超过这个数说明快照和终稿不是同一条链（或被 MERGE / PURGE 过）。
pub const MAX_PATCH_BYTES: usize = 4096;
/// 补丁涉及的页数上限（头页 + 前一会话页 + claim 页那一类）。
pub const MAX_PATCH_PAGES: usize = 16;

/// 真文件最多比 cut 短这么多页（E3D 把前一会话的 `end` 往后指：登录会话之后量到 +1 页，SAVEWORK 之后的 QUIT 尾会话
/// 量到 +3 页；再多就不是同一会话时刻）。
pub const MAX_TRAILING_PAGES: usize = 4;

/// 把真文件的长度套到 cut 上：cut 只能比真文件长整页、最多 [`MAX_TRAILING_PAGES`] 页。回截短后的 cut。
pub fn trim_cut_to(cut: &[u8], real_len: usize, page_size: usize) -> anyhow::Result<&[u8]> {
    ensure!(
        real_len <= cut.len(),
        "快照 {real_len} 字节比 cut {} 字节还长，不是同一会话时刻",
        cut.len()
    );
    let extra = cut.len() - real_len;
    ensure!(
        extra.is_multiple_of(page_size) && extra / page_size <= MAX_TRAILING_PAGES,
        "快照 {real_len} 字节与 cut {} 字节差 {extra} 字节，不是「多指一页」那种差，不是同一会话时刻",
        cut.len()
    );
    Ok(&cut[..real_len])
}

/// 把「真文件 − cut」的差异折成补丁。两者必须等长（调用方先用 [`trim_cut_to`] 对齐）；差异超过上限就拒绝。
pub fn diff_patches(cut: &[u8], real: &[u8], page_size: usize) -> anyhow::Result<Vec<BytePatch>> {
    ensure!(
        cut.len() == real.len(),
        "快照 {} 字节与 cut {} 字节不等长，不是同一会话时刻",
        real.len(),
        cut.len()
    );
    let mut patches = Vec::new();
    let mut total = 0usize;
    let mut pages = BTreeSet::new();
    let mut i = 0usize;
    while i < cut.len() {
        if cut[i] == real[i] {
            i += 1;
            continue;
        }
        let page = i / page_size;
        let page_end = (page + 1) * page_size;
        let start = i;
        while i < page_end && i < cut.len() && cut[i] != real[i] {
            i += 1;
        }
        total += i - start;
        pages.insert(page);
        ensure!(
            total <= MAX_PATCH_BYTES && pages.len() <= MAX_PATCH_PAGES,
            "快照与 cut 的差异超过补丁上限（{total}+ 字节 / {} 页）：不像同一条链的就地改写",
            pages.len()
        );
        patches.push(BytePatch {
            page: page as u32,
            offset: (start - page * page_size) as u32,
            final_hex: hex::encode(&cut[start..i]),
            real_hex: hex::encode(&real[start..i]),
        });
    }
    Ok(patches)
}

/// 把补丁打到 cut 上（cut → 真文件）。打之前核对原字节确实是 `final_hex`：终稿换了、补丁还是老的，在这里就红。
/// 回打进去的字节数。
pub fn apply_patches(
    bytes: &mut [u8],
    patches: &[BytePatch],
    page_size: usize,
) -> anyhow::Result<usize> {
    let mut applied = 0usize;
    for patch in patches {
        let expected = hex::decode(&patch.final_hex).with_context(|| {
            format!(
                "补丁 page {} +{} final_hex 不是 hex",
                patch.page, patch.offset
            )
        })?;
        let real = hex::decode(&patch.real_hex).with_context(|| {
            format!(
                "补丁 page {} +{} real_hex 不是 hex",
                patch.page, patch.offset
            )
        })?;
        ensure!(
            expected.len() == real.len() && !real.is_empty(),
            "补丁 page {} +{} 两边长度不等或为空",
            patch.page,
            patch.offset
        );
        let start = patch.page as usize * page_size + patch.offset as usize;
        let end = start + real.len();
        ensure!(
            patch.offset as usize + real.len() <= page_size && end <= bytes.len(),
            "补丁 page {} +{} 长 {} 越页或越文件尾",
            patch.page,
            patch.offset,
            real.len()
        );
        ensure!(
            bytes[start..end] == expected[..],
            "补丁 page {} +{} 原字节 {} 与台账 final_hex {} 不符：终稿或补丁不是同一份",
            patch.page,
            patch.offset,
            hex::encode(&bytes[start..end]),
            patch.final_hex
        );
        bytes[start..end].copy_from_slice(&real);
        applied += real.len();
    }
    Ok(applied)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseSpec {
    pub id: String,
    pub apply_sesno: u32,
    /// 破坏性案例（如 delete 收尾）可以没有 restore 腿。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_sesno: Option<u32>,
    /// 逻辑名 -> refno（"24384/24775" 或 "24384_24775" 均可）。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub refs: BTreeMap<String, String>,
    /// 存在性探针：pack/verify 都会在对应快照上核对。至少一条。
    pub elements: Vec<CaseElementState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<CaseExpected>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseElementState {
    pub refno: String,
    pub noun: String,
    pub before_apply: bool,
    pub after_apply: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_restore: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseExpected {
    /// `merge_net_changes(apply..=窗口末)` 的期望净结果。
    #[serde(default)]
    pub net_window: Vec<NetExpectation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetExpectation {
    pub refno: String,
    pub net: String,
}

/// 录制脚本的产物，pack 的输入。案例必须按执行顺序排列。
#[derive(Debug, Clone, Deserialize)]
pub struct Recording {
    pub dbnum: u32,
    pub baseline_sesno: u32,
    #[serde(default)]
    pub source: Option<String>,
    /// 录制期间每个会话时刻拷下的真文件（路径；属于哪个 sesno 读它自己的头）。台账里有快照的 sesno 才有补丁；
    /// 不在台账里的快照拒绝（多半是拷错了腿）。
    #[serde(default)]
    pub snapshots: Vec<String>,
    pub cases: Vec<CaseSpec>,
}

pub fn parse_refno(text: &str) -> anyhow::Result<RefU64> {
    let normalized = text.trim().replace('_', "/");
    match RefU64::from_str(&normalized) {
        Ok(refno) => Ok(refno),
        Err(_) => bail!("非法 refno：{text:?}"),
    }
}

/// 一个快照上要核对的单条存在性探针。
#[derive(Debug, Clone)]
pub struct SnapshotProbe {
    pub case_id: String,
    pub refno_text: String,
    pub refno: RefU64,
    pub noun: String,
    pub present: bool,
}

/// pack/verify 共用的执行计划：最终会话、要切割的台账 sesno、逐快照探针。
#[derive(Debug)]
pub struct PackPlan {
    pub final_sesno: u32,
    pub ledger: BTreeSet<u32>,
    pub probes: BTreeMap<u32, Vec<SnapshotProbe>>,
}

/// 校验案例序列并推导执行计划。
///
/// 约束：id 非空且唯一；案例窗口按执行顺序严格递增、互不重叠且都在 baseline 之后；
/// 每案例至少一条元素探针；refno / 净变化取值合法。
pub fn plan_cases(baseline_sesno: u32, cases: &[CaseSpec]) -> anyhow::Result<PackPlan> {
    ensure!(!cases.is_empty(), "recording 里没有任何案例");
    let mut ids = HashSet::new();
    let mut ledger = BTreeSet::from([baseline_sesno]);
    let mut probes: BTreeMap<u32, Vec<SnapshotProbe>> = BTreeMap::new();
    let mut previous_end = baseline_sesno;

    for case in cases {
        ensure!(!case.id.trim().is_empty(), "案例 id 不能为空");
        ensure!(ids.insert(case.id.as_str()), "案例 id 重复：{}", case.id);
        ensure!(
            case.apply_sesno > previous_end,
            "案例 {} 的 apply_sesno={} 未按执行顺序排在前一窗口末 {} 之后",
            case.id,
            case.apply_sesno,
            previous_end
        );
        let window_end = match case.restore_sesno {
            Some(restore) => {
                ensure!(
                    restore > case.apply_sesno,
                    "案例 {} 的 restore_sesno={restore} 必须大于 apply_sesno={}",
                    case.id,
                    case.apply_sesno
                );
                restore
            }
            None => case.apply_sesno,
        };
        previous_end = window_end;

        for (name, refno) in &case.refs {
            parse_refno(refno).with_context(|| format!("案例 {} refs[{name}]", case.id))?;
        }
        ensure!(
            !case.elements.is_empty(),
            "案例 {} 至少要有一条元素探针",
            case.id
        );
        for element in &case.elements {
            let refno = parse_refno(&element.refno)
                .with_context(|| format!("案例 {} 元素探针", case.id))?;
            ensure!(
                !element.noun.trim().is_empty(),
                "案例 {} 元素 {} 缺少 noun",
                case.id,
                element.refno
            );
            let mut push = |sesno: u32, present: bool| {
                probes.entry(sesno).or_default().push(SnapshotProbe {
                    case_id: case.id.clone(),
                    refno_text: element.refno.clone(),
                    refno,
                    noun: element.noun.clone(),
                    present,
                });
            };
            push(case.apply_sesno - 1, element.before_apply);
            push(case.apply_sesno, element.after_apply);
            if let (Some(restore), Some(after_restore)) =
                (case.restore_sesno, element.after_restore)
            {
                push(restore, after_restore);
            }
        }
        if let Some(expected) = &case.expected {
            for net in &expected.net_window {
                parse_refno(&net.refno)
                    .with_context(|| format!("案例 {} expected.net_window", case.id))?;
                ensure!(
                    NET_OPS.contains(&net.net.as_str()),
                    "案例 {} 净变化取值非法：{}（合法：{:?}）",
                    case.id,
                    net.net,
                    NET_OPS
                );
            }
        }

        ledger.insert(case.apply_sesno - 1);
        ledger.insert(case.apply_sesno);
        if let Some(restore) = case.restore_sesno {
            ledger.insert(restore);
        }
    }

    Ok(PackPlan {
        final_sesno: previous_end,
        ledger,
        probes,
    })
}

pub fn validate_relative_path(path: &str) -> anyhow::Result<()> {
    ensure!(!path.is_empty(), "档案路径为空");
    let path = Path::new(path);
    ensure!(
        !path.is_absolute(),
        "档案路径不能是绝对路径：{}",
        path.display()
    );
    for component in path.components() {
        ensure!(
            matches!(component, Component::Normal(_)),
            "不安全的档案路径：{}",
            path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(refno: &str, before: bool, after: bool, restored: Option<bool>) -> CaseElementState {
        CaseElementState {
            refno: refno.to_owned(),
            noun: "BOX".to_owned(),
            before_apply: before,
            after_apply: after,
            after_restore: restored,
        }
    }

    fn case(id: &str, apply: u32, restore: Option<u32>) -> CaseSpec {
        CaseSpec {
            id: id.to_owned(),
            apply_sesno: apply,
            restore_sesno: restore,
            refs: BTreeMap::new(),
            elements: vec![element("24384/24779", false, true, Some(false))],
            expected: None,
        }
    }

    #[test]
    fn refno_accepts_both_slash_and_underscore() {
        assert_eq!(
            parse_refno("24384/24775").unwrap(),
            parse_refno("24384_24775").unwrap()
        );
        assert!(parse_refno("not-a-refno").is_err());
    }

    #[test]
    fn plan_collects_ledger_and_probes() {
        let cases = vec![case("add", 27, Some(28)), case("move", 29, Some(30))];
        let plan = plan_cases(26, &cases).unwrap();
        assert_eq!(plan.final_sesno, 30);
        assert_eq!(
            plan.ledger.iter().copied().collect::<Vec<_>>(),
            vec![26, 27, 28, 29, 30]
        );
        // add: before@26 / after@27 / restore@28；move: before@28 / after@29 / restore@30
        assert_eq!(plan.probes[&26].len(), 1);
        assert_eq!(plan.probes[&28].len(), 2);
        assert_eq!(plan.probes[&30].len(), 1);
    }

    #[test]
    fn plan_rejects_out_of_order_or_overlapping_windows() {
        assert!(plan_cases(26, &[case("late", 26, None)]).is_err());
        assert!(plan_cases(26, &[case("a", 27, Some(29)), case("b", 29, Some(30))]).is_err());
        assert!(plan_cases(26, &[case("bad", 28, Some(27))]).is_err());
        let mut duplicated = vec![case("dup", 27, Some(28)), case("dup", 29, Some(30))];
        duplicated[1].id = "dup".to_owned();
        assert!(plan_cases(26, &duplicated).is_err());
    }

    #[test]
    fn plan_rejects_probeless_cases_and_bad_nets() {
        let mut empty = case("empty", 27, Some(28));
        empty.elements.clear();
        assert!(plan_cases(26, &[empty]).is_err());

        let mut bad_net = case("net", 27, Some(28));
        bad_net.expected = Some(CaseExpected {
            net_window: vec![NetExpectation {
                refno: "24384/24779".to_owned(),
                net: "vanished".to_owned(),
            }],
        });
        assert!(plan_cases(26, &[bad_net]).is_err());
    }

    #[test]
    fn archive_paths_must_stay_relative_and_normal() {
        assert!(validate_relative_path("final/ams8000_0001").is_ok());
        for unsafe_path in ["", "../escape", "safe/../../escape", "/absolute"] {
            assert!(
                validate_relative_path(unsafe_path).is_err(),
                "accepted unsafe path {unsafe_path}"
            );
        }
        #[cfg(windows)]
        assert!(validate_relative_path(r"C:\escape").is_err());
    }

    /// 真文件 = cut 截短一页 + 几个字节不同：diff 折成按页的连续段，apply 还原逐字节相等；越界的差、对不上的 final_hex、
    /// 多出不止整页的长度都拒。
    #[test]
    fn byte_patches_round_trip_between_the_cut_and_the_real_snapshot() {
        let page = 64usize;
        let cut: Vec<u8> = (0..page * 8).map(|i| (i % 251) as u8).collect();
        let mut real = cut[..page * 5].to_vec();
        real[0x17] ^= 0x20;
        real[page * 2 + 5] = 0xEE;
        real[page * 2 + 6] = 0xEF;

        let trimmed = trim_cut_to(&cut, real.len(), page).unwrap();
        let patches = diff_patches(trimmed, &real, page).unwrap();
        assert_eq!(
            patches
                .iter()
                .map(|p| (p.page, p.offset, p.real_hex.len() / 2))
                .collect::<Vec<_>>(),
            vec![(0, 0x17, 1), (2, 5, 2)]
        );
        let mut rebuilt = trimmed.to_vec();
        assert_eq!(apply_patches(&mut rebuilt, &patches, page).unwrap(), 3);
        assert_eq!(rebuilt, real);

        // 长度差不是整页、或超过上限，都不是同一会话时刻。
        assert!(trim_cut_to(&cut, page * 5 + 1, page).is_err());
        assert!(trim_cut_to(&cut, page * (8 - MAX_TRAILING_PAGES - 1), page).is_err());
        // 打在不是那份终稿上就拒。
        let mut other = trimmed.to_vec();
        other[0x17] = 0x00;
        assert!(apply_patches(&mut other, &patches, page).is_err());
    }
}
