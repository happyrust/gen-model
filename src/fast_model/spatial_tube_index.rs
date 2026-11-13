//! 每根 BRAN 的直管段并集盒：`nearby/branches` 靠它补上没加载过的邻管（plant3d-web 计划
//! `docs/plans/2026-10-23-pipe-nearby-clearance-plan.md` N1）。
//!
//! 直管段不进空间树（ADR-072 D4），内存投影里又只有加载过的根——邻管只靠直管段靠近源管、又没被谁加载过时，
//! 它进不了候选。这里在服务就绪后从库里读一遍全部 `tubi_relate` 的世界盒（跳过 `invalid`），按容器求并集，
//! 每根只存一个盒；之后每 [`REFRESH_EVERY`] 重读一遍，盖住启动后重新生成、又被逐出投影的根。
//! 有投影回执的根仍以投影为准（`spatial_branches::assemble`）。

use std::collections::HashMap;
use std::sync::{Arc, Once, PoisonError, RwLock};
use std::time::Duration;

use aios_core::RefU64;
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use serde::Deserialize;
use surrealdb::{Surreal, engine::any::Any};

/// 两次重读的间隔。AvevaMarineSample 全库 6,992 段直管、1,091 根，读一遍约 0.3 s。
pub const REFRESH_EVERY: Duration = Duration::from_secs(600);
/// 还没读成过时的重试间隔（库连接可能晚于服务就绪）。
const RETRY_EVERY: Duration = Duration::from_secs(30);

/// 索引此刻的状态。读成过一次之后，重读失败不回退：旧索引照用，只记日志。
#[derive(Debug, Clone)]
pub enum TubeIndexState {
    Building,
    Ready(Arc<HashMap<RefU64, Aabb>>),
    Failed(String),
}

static STATE: RwLock<TubeIndexState> = RwLock::new(TubeIndexState::Building);
static STARTED: Once = Once::new();

pub fn current() -> TubeIndexState {
    STATE.read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// 起后台重读循环，重复调用只起一次；要在 tokio 运行时里调。
pub fn ensure_started() {
    STARTED.call_once(|| {
        tokio::spawn(async {
            loop {
                let ready = refresh(&aios_core::SUL_DB).await;
                tokio::time::sleep(if ready { REFRESH_EVERY } else { RETRY_EVERY }).await;
            }
        });
    });
}

/// 读一遍并换上；返回此刻是否有可用的索引。
async fn refresh(db: &Surreal<Any>) -> bool {
    match read_from(db).await {
        Ok(index) => {
            log::info!("BRAN 直管段索引：{} 根", index.len());
            *STATE.write().unwrap_or_else(PoisonError::into_inner) =
                TubeIndexState::Ready(Arc::new(index));
            true
        }
        Err(error) => {
            log::warn!("BRAN 直管段索引读取失败：{error:#}");
            let mut state = STATE.write().unwrap_or_else(PoisonError::into_inner);
            if matches!(*state, TubeIndexState::Ready(_)) {
                return true;
            }
            *state = TubeIndexState::Failed(format!("{error:#}"));
            false
        }
    }
}

#[derive(Debug, Deserialize)]
struct TubeBoxRow {
    container: String,
    world_aabb: Aabb,
}

/// 整表扫一遍：周边查询要的是「哪根管的直管段在附近」，不分库。
const TUBE_BOXES_SQL: &str = "SELECT record::id(in) AS container, aabb.d AS world_aabb FROM tubi_relate \
     WHERE aabb.d != NONE AND (invalid ?? false) = false;";

pub async fn read_from(db: &Surreal<Any>) -> anyhow::Result<HashMap<RefU64, Aabb>> {
    let mut response = db.query(TUBE_BOXES_SQL).await?.check()?;
    let rows: Vec<TubeBoxRow> = response.take(0)?;
    Ok(fold(rows))
}

/// 按容器求并集；refno 解析不出、盒不可用（NaN / Inf / 反向）的行丢弃。
fn fold(rows: Vec<TubeBoxRow>) -> HashMap<RefU64, Aabb> {
    let mut index: HashMap<RefU64, Aabb> = HashMap::new();
    for row in rows {
        let Some(bran) = row
            .container
            .parse::<RefU64>()
            .ok()
            .filter(RefU64::is_valid)
        else {
            continue;
        };
        let aabb = row.world_aabb;
        let finite = aabb
            .mins
            .coords
            .iter()
            .chain(aabb.maxs.coords.iter())
            .all(|value| value.is_finite());
        if !finite || (0..3).any(|axis| aabb.mins[axis] > aabb.maxs[axis]) {
            continue;
        }
        index
            .entry(bran)
            .and_modify(|current| *current = current.merged(&aabb))
            .or_insert(aabb);
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use parry3d::math::Point;

    /// 本文件的用例独占 `4000000018/*` 这一组 refno。
    fn id(seq: u32) -> RefU64 {
        RefU64::from_two_nums(4_000_000_018, seq)
    }

    #[test]
    fn rows_fold_into_one_box_per_branch_and_bad_rows_are_dropped() {
        let row = |container: String, min: [f32; 3], max: [f32; 3]| TubeBoxRow {
            container,
            world_aabb: Aabb::new(min.into(), max.into()),
        };
        let index = fold(vec![
            row(id(1).to_string(), [0.0; 3], [100.0, 10.0, 10.0]),
            row(id(1).to_string(), [200.0, -5.0, 0.0], [300.0, 10.0, 10.0]),
            row(id(2).to_string(), [f32::NAN, 0.0, 0.0], [1.0; 3]),
            row(id(2).to_string(), [5.0, 0.0, 0.0], [1.0; 3]),
            row("nonsense".into(), [0.0; 3], [1.0; 3]),
        ]);
        assert_eq!(index.len(), 1, "反向 / NaN 盒与解析不出的 refno 都不进索引");
        let aabb = index[&id(1)];
        assert_eq!(
            (aabb.mins, aabb.maxs),
            (Point::new(0.0, -5.0, 0.0), Point::new(300.0, 10.0, 10.0))
        );
    }

    #[tokio::test]
    async fn the_database_read_skips_invalid_tubes_and_rows_without_a_box() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test").use_db("tube_index").await.unwrap();
        db.query(format!(
            r#"
          CREATE aabb:a SET d={{mins:[0,0,0],maxs:[100,10,10]}};
          CREATE aabb:b SET d={{mins:[200,0,0],maxs:[300,10,10]}};
          CREATE aabb:c SET d={{mins:[5000,0,0],maxs:[6000,10,10]}};
          INSERT RELATION INTO tubi_relate [
            {{id:tubi_relate:[pe:⟨{a}⟩,0],in:pe:⟨{a}⟩,out:inst_geo:m,aabb:aabb:a}},
            {{id:tubi_relate:[pe:⟨{a}⟩,1],in:pe:⟨{a}⟩,out:inst_geo:m,aabb:aabb:b,invalid:false}},
            {{id:tubi_relate:[pe:⟨{a}⟩,2],in:pe:⟨{a}⟩,out:inst_geo:m,aabb:aabb:c,invalid:true}},
            {{id:tubi_relate:[pe:⟨{b}⟩,0],in:pe:⟨{b}⟩,out:inst_geo:m}}
          ];
        "#,
            a = id(1),
            b = id(2),
        ))
        .await
        .unwrap()
        .check()
        .unwrap();

        let index = read_from(&db).await.unwrap();
        assert_eq!(
            index.keys().copied().collect::<Vec<_>>(),
            [id(1)],
            "没有盒的行不进索引"
        );
        let aabb = index[&id(1)];
        assert_eq!(
            (aabb.mins.x, aabb.maxs.x),
            (0.0, 300.0),
            "invalid 段不并进来"
        );
    }
}
