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
    /// 两边都在、位置或包围盒动了超过容差的构件数。
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

/// 一根的逐构件现值（只在进程内比对用，不进回执）。
#[derive(Debug, Clone)]
pub struct RootGeometry {
    pub summary: GeometrySnapshot,
    /// refno `a_b` → (世界平移 xyz, 包围盒 mins xyz, maxs xyz)。同一构件多条行（管身）时取并集盒、平移取第一条。
    pub members: HashMap<String, ([f32; 3], [f32; 6])>,
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
    let mut members: HashMap<String, ([f32; 3], [f32; 6])> = HashMap::new();
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
            .and_modify(|(_, box_)| *box_ = merge(*box_, aabb))
            .or_insert((translation, aabb));
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

/// 前后两份按 refno 对：动了 / 多了 / 少了。
pub(crate) fn diff(before: &RootGeometry, after: &RootGeometry) -> GeometryDiff {
    let mut moved: Vec<&str> = Vec::new();
    let mut added: Vec<&str> = Vec::new();
    let mut removed: Vec<&str> = Vec::new();
    for (refno, (translation, aabb)) in &after.members {
        match before.members.get(refno) {
            None => added.push(refno),
            Some((t0, a0)) => {
                let shifted = translation
                    .iter()
                    .zip(t0)
                    .any(|(a, b)| (a - b).abs() > MOVED_TOLERANCE_MM)
                    || aabb
                        .iter()
                        .zip(a0)
                        .any(|(a, b)| (a - b).abs() > MOVED_TOLERANCE_MM);
                if shifted {
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
