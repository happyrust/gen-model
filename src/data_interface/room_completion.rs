//! Versioned output publication for the memory room calculator.
//! Reads here concern commit metadata only, never calculation inputs.
use anyhow::{Context, ensure};
use serde::Serialize;
use surrealdb::{Surreal, engine::any::Any, sql::Thing};

use super::dbnum_state::RoomInputVersion;
use crate::fast_model::room_model::ElementRoomEdge;

const BATCH_SIZE: usize = 256;

#[derive(Clone, Serialize)]
struct ResultEdge {
    panel: String,
    member: String,
    room_num: String,
    inside_count: u8,
    center_dist: f32,
}

/// A whole dbnum result at one immutable model/room input version. Incremental
/// callers assemble this from the in-memory membership index, including removals.
/// No database connection is accepted while validating or hashing the result.
pub(crate) struct PreparedRoomResult {
    dbnum: u32,
    input: RoomInputVersion,
    version: String,
    edges: Vec<ResultEdge>,
}

impl PreparedRoomResult {
    /// Validated, ordered local result batches, without database staging.
    pub(crate) fn local_batches(&self) -> impl Iterator<Item = anyhow::Result<Vec<u8>>> + '_ {
        self.edges
            .chunks(BATCH_SIZE)
            .map(|batch| Ok(serde_json::to_vec(batch)?))
    }

    pub(crate) fn local_version(&self) -> &str {
        &self.version
    }

    pub(crate) fn new(
        dbnum: u32,
        input: RoomInputVersion,
        edges: &[ElementRoomEdge],
    ) -> anyhow::Result<Self> {
        ensure!(input.is_valid(), "invalid room input proof");
        let mut edges = edges
            .iter()
            .map(|edge| {
                ensure!(
                    edge.member.inside_count <= 8
                        && edge.member.center_dist.is_finite()
                        && edge.member.center_dist >= 0.0,
                    "invalid room membership score"
                );
                Ok(ResultEdge {
                    panel: edge.panel.to_string(),
                    member: edge.member.refno.to_string(),
                    room_num: edge.room_num.clone(),
                    inside_count: edge.member.inside_count,
                    center_dist: edge.member.center_dist,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        edges.sort_by(|a, b| (&a.member, &a.panel).cmp(&(&b.member, &b.panel)));
        ensure!(
            !edges
                .windows(2)
                .any(|pair| pair[0].member == pair[1].member && pair[0].panel == pair[1].panel),
            "duplicate room membership edge"
        );
        let version = crate::data_interface::digest::digest(&serde_json::to_vec(&(
            "memory-room-result-v1",
            dbnum,
            &input,
            &edges,
        ))?);
        Ok(Self {
            dbnum,
            input,
            version,
            edges,
        })
    }

    fn parts(&self) -> usize {
        self.edges.len().div_ceil(BATCH_SIZE)
    }

    /// Each marker commits with its batch. A crash before the last batch leaves
    /// the old visible version intact; retrying identical batches is idempotent.
    pub(crate) async fn stage_on(&self, db: &Surreal<Any>) -> anyhow::Result<()> {
        for part in 0..self.parts() {
            self.stage_part_on(db, part).await?;
        }
        Ok(())
    }

    async fn stage_part_on(&self, db: &Surreal<Any>, part: usize) -> anyhow::Result<()> {
        ensure!(part < self.parts(), "room result part out of bounds");
        #[derive(Serialize)]
        struct Row {
            id: Thing,
            dbnum: u32,
            result_version: String,
            #[serde(flatten)]
            edge: ResultEdge,
        }
        let rows = self
            .edges
            .chunks(BATCH_SIZE)
            .nth(part)
            .unwrap()
            .iter()
            .enumerate()
            .map(|(offset, edge)| Row {
                id: Thing::from((
                    "room_result".to_owned(),
                    format!("{}_{}", self.version, part * BATCH_SIZE + offset),
                )),
                dbnum: self.dbnum,
                result_version: self.version.clone(),
                edge: edge.clone(),
            })
            .collect::<Vec<_>>();
        // Serialization owns the payload before awaiting. Table and row ids are
        // bound values, including room names containing quotes or SQL syntax.
        let rows = surrealdb::sql::to_value(rows)?;
        db.query(
            "BEGIN TRANSACTION;
            INSERT IGNORE INTO room_result $rows;
            UPSERT $part_id SET result_version=$version, dbnum=$dbnum;
            COMMIT TRANSACTION;",
        )
        .bind(("rows", rows))
        .bind((
            "part_id",
            Thing::from((
                "room_result_part".to_owned(),
                format!("{}_{}", self.version, part),
            )),
        ))
        .bind(("version", self.version.clone()))
        .bind(("dbnum", self.dbnum))
        .await?
        .check()
        .context("stage room result batch")?;
        Ok(())
    }

    /// Required input is a durable obligation, installed before calculation.
    ///
    /// 2026-09-10（ADR-072 D4）：这里原先是一道 CAS——比对数据水位与上一版义务，挡的是
    /// 「另一个任务换了义务，我这份陈旧结果还想接着提交」。同库改成单写者同步写之后
    /// 这道比对没有对手了（计划 §2.2 房间门那一行），退成普通写；「这一版模型成没成」
    /// 由任务手里的 `TaskModelOutcome` 在 `room_task` 那道门回答，不在这里回读水位。
    pub(crate) async fn require_on(
        db: &Surreal<Any>,
        dbnum: u32,
        next: &RoomInputVersion,
    ) -> anyhow::Result<()> {
        ensure!(next.is_valid(), "invalid required room input");
        db.query(
            "UPDATE $watermark SET room_required_input=$input,
            state_revision=($watermark.state_revision ?? 0) + 1;",
        )
        .bind((
            "watermark",
            Thing::from(("dbnum_watermark", surrealdb::sql::Id::Number(dbnum as i64))),
        ))
        .bind(("input", next.clone()))
        .await?
        .check()
        .context("register room input obligation")?;
        super::dbnum_consistency::invalidate_census();
        super::dbnum_state::DbnumState::invalidate_mirrored_on(db, dbnum);
        Ok(())
    }

    /// **只写边与结果版本，不盖房间水位**（2026-09-10，ADR-071 D3 / ADR-065 G2 (i)）：
    /// `room_sesno` 挪进任务收尾那一笔（`task_end::commit_task_end_on`），因为房间现在排在
    /// 盖章**之前**算——那一刻 `model_sesno` 还没动，拿它当闸就永远过不去。
    ///
    /// 三道对手 CAS（数据水位、房间水位单调性、义务比对）随 ADR-072 D4 退役：同库单写者，
    /// 没有「另一个任务换了义务」这回事。**留下的那一道不是 CAS 而是自检**——批次齐不齐
    /// 问的是这一笔自己刚 stage 的东西，答错了是本地 bug，不是并发。
    fn commit_sql(&self) -> &'static str {
        "BEGIN TRANSACTION;
        LET $count = array::len((SELECT VALUE id FROM $part_ids WHERE result_version=$version));
        IF $count != $parts { THROW 'room completion has missing result batches'; };
        UPDATE $watermark SET room_result_version=$version, room_input_version=$input,
            state_revision=($watermark.state_revision ?? 0) + 1;
        COMMIT TRANSACTION;"
    }

    pub(crate) async fn commit_on(&self, db: &Surreal<Any>) -> anyhow::Result<()> {
        self.execute_commit_on(db, self.commit_sql()).await
    }

    async fn execute_commit_on(&self, db: &Surreal<Any>, sql: &str) -> anyhow::Result<()> {
        db.query(sql.to_owned())
            .bind((
                "watermark",
                Thing::from((
                    "dbnum_watermark",
                    surrealdb::sql::Id::Number(self.dbnum as i64),
                )),
            ))
            .bind(("input", self.input.clone()))
            .bind(("version", self.version.clone()))
            .bind(("parts", self.parts()))
            .bind((
                "part_ids",
                (0..self.parts())
                    .map(|part| {
                        Thing::from((
                            "room_result_part".to_owned(),
                            format!("{}_{}", self.version, part),
                        ))
                    })
                    .collect::<Vec<_>>(),
            ))
            .await?
            .check()
            .context("commit room result and watermark")?;
        super::dbnum_consistency::invalidate_census();
        super::dbnum_state::DbnumState::invalidate_mirrored_on(db, self.dbnum);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::room_model::RoomMember;

    fn input(definition: &str) -> RoomInputVersion {
        RoomInputVersion {
            version: 1,
            source_identity: "source-a".into(),
            model_sesno: 12,
            model_version: "model-a".into(),
            room_definition_version: definition.into(),
            generation_rule_version: "rules-a".into(),
        }
    }
    fn result(definition: &str, count: usize) -> PreparedRoomResult {
        let edges = (0..count)
            .map(|i| ElementRoomEdge {
                panel: super::super::generation_root::refno_from_e3d(e3d_io::refno::RefNo::new(
                    24384, 1,
                )),
                room_num: "R'1; DROP TABLE pe;".into(),
                member: RoomMember {
                    refno: super::super::generation_root::refno_from_e3d(
                        e3d_io::refno::RefNo::new(24384, i as u32 + 10),
                    ),
                    inside_count: 8,
                    center_dist: 0.0,
                },
            })
            .collect::<Vec<_>>();
        PreparedRoomResult::new(8000, input(definition), &edges).unwrap()
    }
    async fn database() -> Surreal<Any> {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("rooms").use_db("atomic").await.unwrap();
        db.query(
            "CREATE dbnum_watermark:8000 SET applied_sesno=12, model_sesno=12,
            model_sesno_time='source-time', room_sesno=8, room_result_version='old';",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        db
    }
    /// 房间水位现在由任务收尾那一笔盖（`commit_on` 只写边与结果版本）。
    async fn stamp_room_watermark(db: &Surreal<Any>, dbnum: u32, sesno: i32) {
        crate::data_interface::task_end::commit_task_end_on(
            db,
            &crate::data_interface::task_end::TaskEnd {
                dbnum,
                target_sesno: sesno,
                target_time: None,
                model: None,
                rooms: Some(crate::data_interface::task_end::TaskRoomOutcome { ok: true }),
                info: None,
                // 这条只验水位那一句，不牵扯写入标记（T1 的 `write_state`）。
                closes_task: false,
            },
        )
        .await
        .unwrap();
    }

    async fn assert_state(db: &Surreal<Any>, room: i32, version: &str) {
        let mut response = db.query("RETURN dbnum_watermark:8000.room_sesno;
            RETURN dbnum_watermark:8000.room_result_version; RETURN dbnum_watermark:8000.model_sesno;
            RETURN dbnum_watermark:8000.applied_sesno;").await.unwrap().check().unwrap();
        assert_eq!(response.take::<Option<i32>>(0).unwrap(), Some(room));
        assert_eq!(
            response.take::<Option<String>>(1).unwrap().as_deref(),
            Some(version)
        );
        assert_eq!(response.take::<Option<i32>>(2).unwrap(), Some(12));
        assert_eq!(response.take::<Option<i32>>(3).unwrap(), Some(12));
    }

    #[tokio::test]
    async fn partial_batches_and_transaction_failure_preserve_old_results_and_model_progress() {
        let db = database().await;
        let result = result("rooms-a", BATCH_SIZE + 1);
        PreparedRoomResult::require_on(&db, 8000, &result.input)
            .await
            .unwrap();
        result.stage_part_on(&db, 0).await.unwrap();
        assert!(result.commit_on(&db).await.is_err());
        assert_state(&db, 8, "old").await;
        result.stage_on(&db).await.unwrap();
        let broken = result.commit_sql().replace(
            "COMMIT TRANSACTION;",
            "THROW 'simulated crash'; COMMIT TRANSACTION;",
        );
        assert!(result.execute_commit_on(&db, &broken).await.is_err());
        assert_state(&db, 8, "old").await;
        result.commit_on(&db).await.unwrap();
        assert_state(&db, 8, &result.version).await;
        stamp_room_watermark(&db, 8000, 12).await;
        assert_state(&db, 12, &result.version).await;
        let mut rows = db
            .query(
                "SELECT count() AS n FROM room_result GROUP ALL;
            RETURN dbnum_watermark:8000.room_input_version;",
            )
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(
            rows.take::<Vec<serde_json::Value>>(0).unwrap()[0]["n"],
            BATCH_SIZE + 1
        );
        assert_eq!(
            rows.take::<Option<RoomInputVersion>>(1).unwrap(),
            Some(result.input)
        );
    }

    /// 义务 CAS 退役之后（ADR-072 D4）：同库单写者，登记义务是一笔**普通写**——后写的
    /// 那一版原样落下，先前那一版也不再被库这一层挡住。挡陈旧结果的责任前移到
    /// `room_task` 那道 `TaskModelOutcome` 门，这里不再有对手判据。
    #[tokio::test]
    async fn the_obligation_is_a_plain_write_with_no_contender_check() {
        let db = database().await;
        let old = result("rooms-a", 1);
        let new = result("rooms-b", 0);
        PreparedRoomResult::require_on(&db, 8000, &old.input)
            .await
            .unwrap();
        old.stage_on(&db).await.unwrap();
        PreparedRoomResult::require_on(&db, 8000, &new.input)
            .await
            .unwrap();
        // 换过义务之后重登记先前那一版不再报错，先前那一版也提交得下去。
        PreparedRoomResult::require_on(&db, 8000, &old.input)
            .await
            .unwrap();
        old.commit_on(&db).await.unwrap();
        assert_state(&db, 8, &old.version).await;

        new.stage_on(&db).await.unwrap();
        new.commit_on(&db).await.unwrap();
        assert_state(&db, 8, &new.version).await;
        stamp_room_watermark(&db, 8000, 12).await;
        assert_state(&db, 12, &new.version).await;
    }

    /// 数据水位落后不再是这一层的闸（门前移到任务结果）；留下的那道自检——批次齐不齐
    /// ——照旧挡得住，它问的是这一笔自己刚 stage 的东西。
    #[tokio::test]
    async fn missing_batches_still_block_but_a_behind_data_watermark_does_not() {
        let db = database().await;
        let result = result("rooms-a", BATCH_SIZE + 1);
        PreparedRoomResult::require_on(&db, 8000, &result.input)
            .await
            .unwrap();
        result.stage_part_on(&db, 0).await.unwrap();
        assert!(result.commit_on(&db).await.is_err(), "缺批次要挡下");

        db.query("UPDATE dbnum_watermark:8000 SET applied_sesno=11;")
            .await
            .unwrap()
            .check()
            .unwrap();
        result.stage_on(&db).await.unwrap();
        result.commit_on(&db).await.unwrap();
        let mut rows = db
            .query("RETURN dbnum_watermark:8000.room_result_version;")
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(
            rows.take::<Option<String>>(0).unwrap().as_deref(),
            Some(result.version.as_str())
        );
    }

    /// 模型水位**自己**不再是房间提交的闸（2026-09-10，ADR-071 D3）。
    ///
    /// 房间现在排在盖章之前算，提交那一刻库里的 `model_sesno` 还停在上一版——拿它当闸
    /// 就永远过不去。挡住陈旧结果的是义务 CAS（`room_required_input`），由
    /// [`boundary_change_during_calculation_preserves_new_obligation_and_rejects_stale_commit`]
    /// 钉着；这里钉的是「闸确实松开了」，别让它哪天被顺手加回去。
    #[tokio::test]
    async fn a_model_watermark_that_moved_on_its_own_no_longer_blocks_the_room_commit() {
        let db = database().await;
        let result = result("rooms-a", 0);
        PreparedRoomResult::require_on(&db, 8000, &result.input)
            .await
            .unwrap();
        db.query("UPDATE dbnum_watermark:8000 SET model_sesno=13, applied_sesno=13;")
            .await
            .unwrap()
            .check()
            .unwrap();
        result.stage_on(&db).await.unwrap();
        result.commit_on(&db).await.unwrap();
    }

    #[tokio::test]
    async fn cross_database_boundary_refresh_changes_only_the_covered_database_proof() {
        let db = database().await;
        db.query("CREATE dbnum_watermark:8001 SET applied_sesno=7, model_sesno=7, room_sesno=7;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let mut before = input("rooms-a");
        before.model_sesno = 7;
        PreparedRoomResult::require_on(&db, 8001, &before)
            .await
            .unwrap();
        let first = PreparedRoomResult::new(8001, before.clone(), &[]).unwrap();
        first.stage_on(&db).await.unwrap();
        first.commit_on(&db).await.unwrap();
        let mut after = before.clone();
        after.room_definition_version = "rooms-b".into();
        PreparedRoomResult::require_on(&db, 8001, &after)
            .await
            .unwrap();
        let second = PreparedRoomResult::new(8001, after.clone(), &[]).unwrap();
        second.stage_on(&db).await.unwrap();
        second.commit_on(&db).await.unwrap();
        assert_state(&db, 8, "old").await;
        let mut state = db.query("RETURN dbnum_watermark:8001.room_sesno; RETURN dbnum_watermark:8001.room_input_version;")
            .await.unwrap().check().unwrap();
        assert_eq!(state.take::<Option<i32>>(0).unwrap(), Some(7));
        assert_eq!(
            state.take::<Option<RoomInputVersion>>(1).unwrap(),
            Some(after)
        );
    }
}
