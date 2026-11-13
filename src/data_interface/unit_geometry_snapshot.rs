//! 交付单元的几何快照：一批开工前、收尾后各读一次根名下的模型行，进单元回执的 `geometry_before` /
//! `geometry_after` / `geometry_diff`（增量记录页「几何前 → 后」，计划 2026-10-24 W4）。
//!
//! 与 [`super::unit_room_snapshot`] 同一个拍法、同一道门：「前」在 worklist 开工之前读一次这根此刻对外答的样子，
//! 「后」在全部单元收尾之后再读一次。读法与 `/model/records` 同一条（`model_record_reader::records_of_root`：进程投影里有
//! 回执就以它为准，没有再读库上的 `inst_relate` / `tubi_relate`）——整根重算的新行先进投影、durable ACK 之后才落库并逐出
//! （ADR-082），收尾那一刻只读库拿到的还是旧行，前后会假装没动（2026-10-24 真机第一腿就是这样露馅的）。回执里不带逐条
//! 记录（一根 BRAN 几十条、一批几十根会把 `/tasks` 撑爆），只带整根的并集包围盒 + 记录数，外加一份服务端算好的差异
//! （动了 / 多了 / 少了几条，带几个样本 refno）。
//!
//! 不读的场合（回执里三格都缺席）：读透形态（行在进程 store）；一批超过 [`MAX_ROOTS`] 根。某一根读失败（投影残缺 / 库读
//! 错）只记日志、只缺那一根。

use std::collections::HashMap;
use std::str::FromStr;

use aios_core::{GeomInstQuery, RefU64, RefnoEnum};
use serde::{Deserialize, Serialize};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

/// 与房间快照同一个上限。
pub const MAX_ROOTS: usize = super::unit_room_snapshot::MAX_ROOTS;

/// 门：这一批要不要拍快照。与 [`super::unit_room_snapshot::gate_skips`] 同一套判据，只是几何
/// 没有「房间归属」这一条（几何行与房间归属无关）。纯函数：`data_face` 由调用方给。
pub(crate) fn gate_skips(roots: &[String], data_face: crate::options::DataFace) -> bool {
    roots.is_empty() || roots.len() > MAX_ROOTS || data_face.is_read_through()
}

/// 位置 / 包围盒差过这个数（mm）才算「动了」：生成器的浮点往返会在最后一位上抖。
const MOVED_TOLERANCE_MM: f32 = 0.5;

/// 回执里的样本 refno 上限。
const SAMPLE_LIMIT: usize = 8;

/// 一根此刻的几何概要。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeometrySnapshot {
    /// 模型行数（`inst_relate` + `tubi_relate`）。
    pub records: u32,
    /// 全部行的并集世界包围盒 `[min_x, min_y, min_z, max_x, max_y, max_z]`；没有行就没有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aabb: Option<[f32; 6]>,
}

/// 前后两份快照按 refno 对出来的差异。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeometryDiff {
    /// 两边都在、位置或包围盒动了超过容差的构件数。一个构件名下有几条行时（隐式管身挂在支管的 refno 下）逐条配对：
    /// 条数变了、或有一条配不上，才算这个构件动了；条与条的先后不算（[`same_rows`]）。
    pub moved: u32,
    /// 只在「后」里的构件数。
    pub added: u32,
    /// 只在「前」里的构件数。
    pub removed: u32,
    /// 最多 [`SAMPLE_LIMIT`] 个有变化的构件 `a_b`（先动的、再多的、再少的，各自按 refno 排）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample: Vec<String>,
}

impl GeometryDiff {
    pub fn is_empty(&self) -> bool {
        self.moved == 0 && self.added == 0 && self.removed == 0
    }
}

/// 一个构件名下一条行的现值：(世界平移 xyz, 包围盒 mins xyz + maxs xyz)。
pub type MemberRow = ([f32; 3], [f32; 6]);

/// 一根的逐构件现值（只在进程内比对用，不进回执）。
#[derive(Debug, Clone)]
pub struct RootGeometry {
    pub summary: GeometrySnapshot,
    /// refno `a_b` → 这个构件名下的每一条行。一个构件通常一条；隐式管身挂在支管的 refno 下，一根支管几段管身就几条。
    ///
    /// 条与条的先后**不带含义**，比对时不许按位置对（GitHub #75）：两条读路给管身的顺序不同——库上冷读按
    /// `tubi_relate:[pe, 槽位]` 的槽位排，投影覆盖按投影记录 id（字符串）排。「前」读库、「后」读投影的那一批
    /// （冷重启后的第一批、根被逐出投影之后的一批）里，同一组管身前后顺序就不一样。
    pub members: HashMap<String, Vec<MemberRow>>,
}

/// 在指定库上拍（投影覆盖优先）。**不带门**——调用方先用 [`gate_skips`] 判一次（同一批任务的
/// 前后两次快照必须用同一份形态，所以形态由调用方给）。解不出的根跳过；读失败的根记一句日志、不进表。
pub(crate) async fn snapshot_on(
    db: &Surreal<Any>,
    roots: &[String],
) -> HashMap<String, RootGeometry> {
    let mut out = HashMap::with_capacity(roots.len());
    for root in roots {
        let Ok(refno) = RefU64::from_str(root) else {
            continue;
        };
        match crate::fast_model::model_record_reader::records_of_root(db, RefnoEnum::from(refno))
            .await
        {
            Ok(rows) => {
                out.insert(root.clone(), fold(&rows));
            }
            Err(error) => {
                log::warn!("单元几何快照读根 {root} 失败，这根的回执里不带几何前后: {error:#}");
            }
        }
    }
    out
}

/// 把一根的行折成概要 + 逐构件现值。
pub(crate) fn fold(rows: &[GeomInstQuery]) -> RootGeometry {
    let mut union: Option<[f32; 6]> = None;
    let mut members: HashMap<String, Vec<MemberRow>> = HashMap::new();
    for row in rows {
        let aabb = [
            row.world_aabb.mins.x,
            row.world_aabb.mins.y,
            row.world_aabb.mins.z,
            row.world_aabb.maxs.x,
            row.world_aabb.maxs.y,
            row.world_aabb.maxs.z,
        ];
        if aabb.iter().any(|v| !v.is_finite()) {
            continue;
        }
        union = Some(match union {
            None => aabb,
            Some(u) => merge(u, aabb),
        });
        let translation = row.world_trans.translation.to_array();
        members
            .entry(row.refno.to_string())
            .or_default()
            .push((translation, aabb));
    }
    RootGeometry {
        summary: GeometrySnapshot {
            records: rows.len() as u32,
            aabb: union,
        },
        members,
    }
}

fn merge(a: [f32; 6], b: [f32; 6]) -> [f32; 6] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].min(b[2]),
        a[3].max(b[3]),
        a[4].max(b[4]),
        a[5].max(b[5]),
    ]
}

/// 两条行是不是同一条：平移与包围盒的九个分量没有哪一个差过容差。
fn same_row(a: &MemberRow, b: &MemberRow) -> bool {
    // 差是 NaN 的分量不算差过：平移里出了 NaN 的行照旧不算动（与改之前 `差 > 容差` 同一个判法）。
    let within = |x: &f32, y: &f32| {
        let gap = (x - y).abs();
        gap.is_nan() || gap <= MOVED_TOLERANCE_MM
    };
    a.0.iter().zip(&b.0).all(|(x, y)| within(x, y))
        && a.1.iter().zip(&b.1).all(|(x, y)| within(x, y))
}

/// 两条行差多少（九个分量差的绝对值之和）。只在 [`same_rows`] 里几条候选都在容差内时用来挑最近的那一条。
fn row_distance(a: &MemberRow, b: &MemberRow) -> f32 {
    let gap = |x: &f32, y: &f32| (x - y).abs();
    a.0.iter().zip(&b.0).map(|(x, y)| gap(x, y)).sum::<f32>()
        + a.1.iter().zip(&b.1).map(|(x, y)| gap(x, y)).sum::<f32>()
}

/// 同一个构件前后两组行是不是同一组：条数相同，且「后」的每一条都能在「前」里配到一条还没被配走的同一条
/// （[`same_row`]）。**不按位置配**——两条读路给管身的顺序不同（见 [`RootGeometry::members`]）。此前是「包围盒取并集、
/// 平移取读到的第一条」，同一组管身换个顺序读回来就被数成「动了 1」（GitHub #75：ams7997 的 `24381/145764` 只改
/// HPOS、四段管身一段没动，「前」读库、「后」读投影，回执写 `moved=1`）；反过来，不是第一条的那段管身动了、
/// 并集盒又没变时，它数不出来。
fn same_rows(before: &[MemberRow], after: &[MemberRow]) -> bool {
    if before.len() != after.len() {
        return false;
    }
    // 只有一条的构件、两边顺序相同的管身：按位置一遍就对完，不必配对。
    if before.iter().zip(after).all(|(a, b)| same_row(a, b)) {
        return true;
    }
    let mut taken = vec![false; before.len()];
    after.iter().all(|row| {
        let nearest = before
            .iter()
            .enumerate()
            .filter(|(index, candidate)| !taken[*index] && same_row(candidate, row))
            .min_by(|(_, a), (_, b)| row_distance(a, row).total_cmp(&row_distance(b, row)))
            .map(|(index, _)| index);
        match nearest {
            Some(index) => {
                taken[index] = true;
                true
            }
            None => false,
        }
    })
}

/// 前后两份按 refno 对：动了 / 多了 / 少了。
pub(crate) fn diff(before: &RootGeometry, after: &RootGeometry) -> GeometryDiff {
    let mut moved: Vec<&str> = Vec::new();
    let mut added: Vec<&str> = Vec::new();
    let mut removed: Vec<&str> = Vec::new();
    for (refno, rows) in &after.members {
        match before.members.get(refno) {
            None => added.push(refno),
            Some(rows_before) => {
                if !same_rows(rows_before, rows) {
                    moved.push(refno);
                }
            }
        }
    }
    for refno in before.members.keys() {
        if !after.members.contains_key(refno) {
            removed.push(refno);
        }
    }
    moved.sort_unstable();
    added.sort_unstable();
    removed.sort_unstable();
    let sample = moved
        .iter()
        .chain(added.iter())
        .chain(removed.iter())
        .take(SAMPLE_LIMIT)
        .map(|refno| (*refno).to_owned())
        .collect();
    GeometryDiff {
        moved: moved.len() as u32,
        added: added.len() as u32,
        removed: removed.len() as u32,
        sample,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 根名下两路行都收；并集盒、记录数、逐构件现值；改了一条的 z 后 diff 正好数出那一条。
    #[tokio::test]
    async fn a_root_snapshot_folds_rows_and_the_diff_names_the_moved_member() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test")
            .use_db("unit_geometry_snapshot")
            .await
            .unwrap();
        let root: RefnoEnum = "24384_23257".into();
        let packed = root.refno().0;
        db.query(format!(
            r#"
            CREATE aabb:a SET d={{mins:[0,0,2900],maxs:[10,10,2930]}};
            CREATE aabb:b SET d={{mins:[20,0,2900],maxs:[30,10,2950]}};
            CREATE trans:a SET d={{translation:[5,5,2900],rotation:[0,0,0,1],scale:[1,1,1]}};
            CREATE trans:b SET d={{translation:[25,5,2900],rotation:[0,0,0,1],scale:[1,1,1]}};
            CREATE inst_relate:a SET in=pe:⟨24384_23262⟩, anc=[{packed}], aabb=aabb:a, world_trans=trans:a, generic='PIPE', booled_id='x';
            CREATE inst_relate:b SET in=pe:⟨24384_23260⟩, anc=[{packed}], aabb=aabb:b, world_trans=trans:b, generic='PIPE', booled_id='y';
            CREATE inst_relate:other SET in=pe:⟨24384_999⟩, anc=[1], aabb=aabb:b, world_trans=trans:b, generic='PIPE', booled_id='z';
            "#
        ))
        .await
        .unwrap()
        .check()
        .unwrap();
        let before = snapshot_on(&db, &["24384/23257".to_owned(), "junk".to_owned()]).await;
        assert_eq!(before.len(), 1, "解不出的根跳过");
        let before = before.get("24384/23257").expect("root snapshotted");
        assert_eq!(before.summary.records, 2);
        assert_eq!(
            before.summary.aabb,
            Some([0.0, 0.0, 2900.0, 30.0, 10.0, 2950.0])
        );
        assert_eq!(before.members.len(), 2);

        db.query("UPDATE aabb:a SET d={mins:[0,0,3400],maxs:[10,10,3430]}; UPDATE trans:a SET d={translation:[5,5,3400],rotation:[0,0,0,1],scale:[1,1,1]};")
            .await
            .unwrap()
            .check()
            .unwrap();
        let after = snapshot_on(&db, &["24384/23257".to_owned()]).await;
        let after = after.get("24384/23257").unwrap();
        assert_eq!(
            after.summary.aabb,
            Some([0.0, 0.0, 2900.0, 30.0, 10.0, 3430.0])
        );
        let d = diff(before, after);
        assert_eq!(
            d,
            GeometryDiff {
                moved: 1,
                added: 0,
                removed: 0,
                sample: vec!["24384_23262".to_owned()],
            }
        );
        assert!(diff(before, before).is_empty(), "同一份比自己没有差异");
    }

    /// 一条查询行：只填快照会看的格（构件号、世界平移、世界包围盒）。
    fn query_row(refno: &str, translation: [f32; 3], aabb: [f32; 6]) -> GeomInstQuery {
        let refno: RefnoEnum = refno.into();
        GeomInstQuery {
            refno,
            old_refno: None,
            owner: refno,
            world_aabb: parry3d::bounding_volume::Aabb::new(
                [aabb[0], aabb[1], aabb[2]].into(),
                [aabb[3], aabb[4], aabb[5]].into(),
            ),
            world_trans: bevy_transform::components::Transform::from_translation(
                translation.into(),
            ),
            insts: Vec::new(),
            has_neg: false,
            generic: "TUBI".into(),
            pts: None,
            date: None,
        }
    }

    const BRANCH: &str = "24381_145764";

    /// ams7997 的 BRAN `24381/145764` 名下四段隐式管身的（世界平移，世界包围盒），按库上的槽位序。385 与 387 两个
    /// 会话逐字相同（387 只改了这根支管的 HPOS）；数取自 2026-11-11 回放的 `/model/records`。
    const TUBES: [MemberRow; 4] = [
        (
            [8384.06, 10034.69, 450.0],
            [8353.5, 10002.4, 405.5, 10094.4, 11660.9, 494.5],
        ),
        (
            [8301.15, 9956.01, 4535.7],
            [8256.7, 9911.6, 564.3, 8345.6, 10000.5, 4535.7],
        ),
        (
            [6762.22, 11577.71, 4650.0],
            [6730.0, 10008.3, 4605.5, 8254.7, 11608.3, 4694.5],
        ),
        (
            [6683.54, 11660.62, 4150.0],
            [6639.1, 11616.2, 4150.0, 6728.0, 11705.1, 4535.7],
        ),
    ];

    /// 这根支管的一份快照：头上那道焊缝（盒是随手给的）+ 给定的几段管身，按给定的先后。管身都挂在支管的 refno 下。
    fn branch_snapshot(tubes: &[MemberRow]) -> RootGeometry {
        let mut rows = vec![query_row(
            "24381_145765",
            [10310.42, 11862.73, 450.0],
            [10262.0, 11814.0, 402.0, 10359.0, 11911.0, 498.0],
        )];
        rows.extend(
            tubes
                .iter()
                .map(|(translation, aabb)| query_row(BRANCH, *translation, *aabb)),
        );
        fold(&rows)
    }

    /// GitHub #75：同一组管身，库上冷读（按槽位：0、1、2、3）与投影覆盖（按记录 id：回放里读到的是 3、1、0、2）
    /// 读回来的先后不同，回执不许因此说「动了」。此前平移取读到的第一条，这一例数成 `moved=1`、样本是支管自己。
    /// 浮点往返在最后一位上的抖动（容差以内）也不算。
    #[test]
    fn the_diff_does_not_depend_on_the_order_a_branch_s_tubes_are_read_in() {
        let [t0, t1, t2, t3] = TUBES;
        let nudged = |(translation, aabb): MemberRow, by: f32| -> MemberRow {
            ([translation[0] + by, translation[1], translation[2]], aabb)
        };
        let durable = branch_snapshot(&[t0, t1, t2, t3]);
        let overlay = branch_snapshot(&[nudged(t3, 0.25), nudged(t1, -0.25), nudged(t0, 0.25), t2]);
        assert_eq!(
            durable.summary, overlay.summary,
            "概要（行数 + 并集盒）本来就不看顺序"
        );
        assert_eq!(durable.members[BRANCH].len(), 4, "四段管身都在支管名下");
        assert_eq!(
            diff(&durable, &overlay),
            GeometryDiff::default(),
            "「前」读库、「后」读投影"
        );
        assert_eq!(
            diff(&overlay, &durable),
            GeometryDiff::default(),
            "反过来也一样"
        );
    }

    /// 反面：真动了的管身要数得出来，不管它是第几条、并集盒变没变。此前只比「读到的第一条」的平移和整组的并集盒，
    /// 不是第一条的那一段在并集盒里面变了时数成没动。条数变了（少了一段、多了一段）算这根支管动了，不算多了、
    /// 少了一个构件。
    #[test]
    fn a_tube_that_really_moved_is_counted_whichever_row_it_is() {
        let [t0, t1, t2, t3] = TUBES;
        let before = branch_snapshot(&[t0, t1, t2, t3]);
        let branch_moved = GeometryDiff {
            moved: 1,
            added: 0,
            removed: 0,
            sample: vec![BRANCH.to_owned()],
        };

        // 第三段沿 Y 缩短 300：它自己的盒和平移变了；整组的并集盒没变（别的行把它罩着），读到的第一条也没变。
        let shortened: MemberRow = (
            [6762.22, 11277.71, 4650.0],
            [6730.0, 10008.3, 4605.5, 8254.7, 11308.3, 4694.5],
        );
        let after = branch_snapshot(&[t0, t1, shortened, t3]);
        assert_eq!(before.summary, after.summary, "并集盒没变，概要看不出来");
        assert_eq!(
            diff(&before, &after),
            branch_moved,
            "动的是支管名下的一段：数在支管头上"
        );
        assert_eq!(
            diff(&before, &branch_snapshot(&[t3, shortened, t0, t1])),
            branch_moved,
            "换个顺序读回来，结论不变"
        );

        let fewer = branch_snapshot(&[t0, t1, t2]);
        assert_eq!(diff(&before, &fewer), branch_moved, "少了一段");
        assert_eq!(diff(&fewer, &before), branch_moved, "多了一段");
    }
    /// 门：空表、超过根数上限、读透形态都不读库。
    #[test]
    fn the_gate_skips_empty_oversized_and_read_through_batches() {
        let face = crate::options::DataFace::Ingest;
        assert!(gate_skips(&[], face), "空表不读库");
        let many: Vec<String> = (0..=MAX_ROOTS).map(|i| format!("1/{i}")).collect();
        assert!(gate_skips(&many, face), "超过根数上限不读库");
        let one = vec!["1/1".to_owned()];
        assert!(!gate_skips(&one, face), "单根要读库");
        assert!(!gate_skips(&many[..MAX_ROOTS], face), "正好上限要读库");
        assert!(
            gate_skips(&one, crate::options::DataFace::ReadThrough),
            "读透形态不读库（行在进程 store）"
        );
    }
}
