//! Ref0 统计快照的生产者（ADR-071 D6，计划 v3 S2 后半）。
//!
//! `dbnum_info_table` 每个 Ref0 一行 `{count, max_ref1, sesno, file_name, db_type}`，此前由
//! `pe_batch::render` 的逐笔 ±1 观测块维护（每笔元素写都带 `$before_` / `$after_` 两次点读、一段
//! `FOR` 循环与一次热点键 UPSERT），`count` 的口径是「pe 行数含墓碑」。ADR-071 D6 把它改成任务收尾
//! **从内存序列化**：`count` = 该 Ref0 在目标会话 `T` 上的**活元素数**，来源是文件在 `T` 的索引
//! （`DbSet@T`：`ReadOnlyEngine::open_at(path, T)` 的 RefNo 树只列活元素，被删的元素不在树里），
//! 一趟索引遍历（24 B / 元素）就数完，零库读；随 `task_end` 那一笔按绝对值覆盖，不再逐笔加减。
//!
//! **`sesno` 取 `T`**：这份快照描述的是 `T` 上的活元素集。逐元素的「最后修改会话」不在索引里（要走
//! 全部会话 delta 才知道），而这一列的唯一消费者是兼容回退水位（`dbnum_state::LEGACY_INFO_WATERMARKS`
//! 取 `math::max(sesno)`）——它要回答的正是「这个库的数据序列化到了哪一版」，`T` 就是答案。
//! 渲染器那边仍是 `math::max([$old.sesno, T])`，单调。
//!
//! 只有[`fold_ref0_stats`]是纯函数；[`snapshot_at`]开文件、走索引，是阻塞 I/O，调用方放进
//! `spawn_blocking`。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use e3d_io::ReadOnlyEngine;
use e3d_io::refno::RefNo;

use super::frozen_source::FrozenSource;
use super::task_end::{Ref0Snapshot, Ref0Stats};

/// 一次快照要用的文件身份：冻结源在就走它（路径身份复核 + 会话链核对），没有就按路径直接开。
#[derive(Debug, Clone)]
pub(crate) struct SnapshotSource {
    pub frozen: Option<FrozenSource>,
    pub path: PathBuf,
    pub file_name: String,
    pub db_type: String,
}

/// 目标索引里的 refno 清单 → 每个 Ref0 一行（按 Ref0 升序）。
///
/// `ref0` = `word0`，`ref1` = `word1`（与 `pe:{ref0}_{ref1}` 的记录 id、`mdb_memory_store` 的
/// `ref0 → dbnum` 归属同一拆法）。空清单 → 空快照（该库在 `T` 上没有活元素，渲染器把现有行全部清零）。
pub(crate) fn fold_ref0_stats(
    refnos: impl IntoIterator<Item = RefNo>,
    target_sesno: i32,
) -> Vec<Ref0Stats> {
    let mut groups: BTreeMap<u32, (i64, u32)> = BTreeMap::new();
    for refno in refnos {
        let entry = groups.entry(refno.word0).or_insert((0, 0));
        entry.0 += 1;
        entry.1 = entry.1.max(refno.word1);
    }
    groups
        .into_iter()
        .map(|(ref0, (count, max_ref1))| Ref0Stats {
            ref0,
            count,
            max_ref1,
            sesno: target_sesno,
        })
        .collect()
}

/// 以会话 `target_sesno` 打开文件的索引，数出整库快照。
///
/// 有冻结源就经 `reopen_verified_at`：路径身份（设备 / inode 或卷序列 + 文件 id）与会话链都要对得上，
/// 文件被换过就在这里报错，而不是把另一个文件的元素数写进这个库的统计。
pub(crate) fn snapshot_at(
    source: &SnapshotSource,
    target_sesno: u32,
) -> anyhow::Result<Ref0Snapshot> {
    let mut engine = match &source.frozen {
        Some(frozen) => frozen
            .reopen_verified_at(target_sesno)
            .with_context(|| {
                format!(
                    "复核 {} 的冻结源到会话 {target_sesno}",
                    source.path.display()
                )
            })?
            .open_engine_at_target()?,
        None => open_engine_at(&source.path, target_sesno)?,
    };
    let indexed = engine.indexed_refnos().with_context(|| {
        format!(
            "遍历 {} 会话 {target_sesno} 的索引树",
            source.path.display()
        )
    })?;
    Ok(Ref0Snapshot {
        file_name: source.file_name.clone(),
        db_type: source.db_type.clone(),
        rows: fold_ref0_stats(
            indexed.into_iter().map(|(refno, _, _)| refno),
            i32::try_from(target_sesno).context("目标会话号超出 i32")?,
        ),
    })
}

fn open_engine_at(path: &Path, target_sesno: u32) -> anyhow::Result<ReadOnlyEngine> {
    ReadOnlyEngine::open_at(path, target_sesno).map_err(|error| {
        anyhow::anyhow!(
            "以会话 {target_sesno} 打开 {} 失败: {error}",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refno(word0: u32, word1: u32) -> RefNo {
        RefNo::new(word0, word1)
    }

    /// 纯函数口径：按 word0 分组数活元素、取最大 word1、`sesno` 一律是目标会话；输出按 Ref0 升序；
    /// 空清单是空快照（渲染器据此把现有行全部清零），不是「没有快照」。
    #[test]
    fn fold_counts_live_elements_per_ref0_and_stamps_the_target_session() {
        let rows = fold_ref0_stats(
            [
                refno(24384, 5),
                refno(24384, 26229),
                refno(24384, 23264),
                refno(17496, 950),
                refno(17496, 12),
            ],
            26,
        );
        assert_eq!(
            rows,
            vec![
                Ref0Stats {
                    ref0: 17496,
                    count: 2,
                    max_ref1: 950,
                    sesno: 26,
                },
                Ref0Stats {
                    ref0: 24384,
                    count: 3,
                    max_ref1: 26229,
                    sesno: 26,
                },
            ]
        );
        assert!(fold_ref0_stats(std::iter::empty(), 26).is_empty());
    }

    /// 真文件（装了 AVEVA 样例才跑）：快照的 `count` 之和 == 该会话索引树的元素数，`max_ref1`
    /// 不小于树里任一同 Ref0 的 word1，`sesno` 全等于目标会话，`file_name` / `db_type` 随源。
    #[test]
    fn snapshot_of_a_real_file_matches_its_index_tree() {
        const AMS1112: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams1112_0001";
        let path = Path::new(AMS1112);
        if !path.is_file() {
            println!("skip: E3D fixtures are not installed");
            return;
        }
        let frozen = FrozenSource::capture(path).expect("capture latest");
        let target = frozen.target_sesno();
        let source = SnapshotSource {
            frozen: Some(frozen.clone()),
            path: path.to_path_buf(),
            file_name: "ams1112_0001".to_owned(),
            db_type: frozen.db_type().to_owned(),
        };
        let snapshot = snapshot_at(&source, target).expect("snapshot");
        let indexed = ReadOnlyEngine::open_at(path, target)
            .expect("open at target")
            .indexed_refnos()
            .expect("index walk");
        assert!(!snapshot.rows.is_empty());
        assert_eq!(
            snapshot.rows.iter().map(|row| row.count).sum::<i64>(),
            indexed.len() as i64,
            "活元素数之和 == 索引树元素数"
        );
        for (refno, _, _) in &indexed {
            let row = snapshot
                .rows
                .iter()
                .find(|row| row.ref0 == refno.word0)
                .expect("每个 word0 都有一行");
            assert!(row.max_ref1 >= refno.word1);
            assert_eq!(row.sesno, target as i32);
        }
        assert!(
            snapshot
                .rows
                .windows(2)
                .all(|pair| pair[0].ref0 < pair[1].ref0)
        );
        assert_eq!(snapshot.file_name, "ams1112_0001");
        assert_eq!(snapshot.db_type, frozen.db_type());
        // 没有冻结源、按路径直接开，得到同一份。
        let bare = snapshot_at(
            &SnapshotSource {
                frozen: None,
                ..source
            },
            target,
        )
        .expect("snapshot by path");
        assert_eq!(bare.rows, snapshot.rows);
    }
}
