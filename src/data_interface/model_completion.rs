//! Durable task completion, including an authoritatively empty model inventory.
//! Receipt and model watermark commit atomically on dbnum_watermark.
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use surrealdb::{Surreal, engine::any::Any};

use super::frozen_source::FrozenSource;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CompletionReceipt {
    version: u32,
    project: String,
    source_token: String,
    policy: String,
    dependency_tokens: Vec<String>,
    pub empty: bool,
    /// A bounded inductive proof; never embeds the previous receipt recursively.
    #[serde(default)]
    coverage: Option<WindowCoverage>,
    #[serde(skip)]
    expected_base: Option<String>,
    #[serde(skip)]
    pub changed_roots: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct WindowCoverage {
    base_sesno: i32,
    window_start: i32,
    base_digest: String,
}

#[derive(Deserialize)]
struct CoverageRow {
    #[serde(default)]
    model_sesno: i32,
    #[serde(default)]
    model_completion_sesno: Option<i32>,
    #[serde(default)]
    model_completion: Option<String>,
}

pub(crate) async fn coverage_on(db: &Surreal<Any>, dbnum: u32) -> anyhow::Result<Option<(i32, String)>> {
    let row: Option<CoverageRow> = db.query(format!("SELECT model_sesno, model_completion_sesno, model_completion FROM ONLY dbnum_watermark:{dbnum};"))
        .await?.check()?.take(0)?;
    Ok(row.and_then(|row| {
        let raw = row.model_completion?;
        (row.model_completion_sesno == Some(row.model_sesno)
            && validated_receipt(Some(&raw), dbnum, row.model_sesno).is_some())
            .then_some((row.model_sesno, raw))
    }))
}

fn policy() -> String {
    let mut types: Vec<_> = super::generation_root::configured_delivery_unit_types()
        .into_iter()
        .collect();
    types.sort();
    format!("generation-inventory-v1:{types:?}")
}

/// 收据落在**水位行**上（2026-09-10，ADR-071 D1）。
///
/// 它原先寄居在「文件轮次」那一行（`incr_side_effect_pending:spatial_reconcile_*`）——
/// 账本时代收据要跟着那一轮的发布状态走，所以住在那儿。账本退役之后它得有自己的家，
/// 而它描述的本来就是「这个库追平到哪一版」，`dbnum_watermark:{dbnum}` 就是那句话的行。
///
/// 同一行上另存 `model_completion_sesno`：收据是给哪一版开的。读者拿它与 `model_sesno`
/// 对齐，对不上就当没有——**这一条替掉了原先「收据的 source_token 要等于那一轮的
/// commit_token」那道交叉校验**：收据当时寄居在别人的行上，得问两边同不同意；现在它就住在
/// 任务自己盖章的那一行，没有第二方可以不同意。
pub(crate) fn receipt_row(dbnum: u32) -> String {
    format!("dbnum_watermark:{dbnum}")
}

/// Split the denormalized dbnum lookup from legacy ownership traversal. Both
/// paths retain their meaning while the latter starts from the PE dbnum index.
pub(crate) fn projection_presence_expr(dbnum: u32) -> String {
    ["inst_relate", "tubi_relate"]
        .into_iter()
        .map(|table| {
            format!(
                "array::len((SELECT VALUE id FROM {table} WHERE dbnum={dbnum} LIMIT 1)) > 0 OR \
         array::len((SELECT VALUE id FROM pe WHERE dbnum={dbnum} AND \
         array::len((SELECT VALUE id FROM {table} WHERE in=$parent.id LIMIT 1)) > 0 LIMIT 1)) > 0"
            )
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

impl CompletionReceipt {
    /// Capture before work starts; validation after ACK rejects changed dependencies.
    pub fn capture(token: &str) -> anyhow::Result<Self> {
        let source = FrozenSource::from_commit_token_json(token)?.reopen_verified()?;
        let dependency_tokens = {
            let (pins, _) = crate::fast_model::e3d_model_service::current_mdb_sources()?;
            pins.into_iter()
                .filter(|pin| matches!(pin.db_type.as_str(), "CATA" | "PROP"))
                .map(|pin| FrozenSource::capture(pin.file)?.commit_token_json())
                .collect::<anyhow::Result<Vec<_>>>()?
        };
        source.reopen_verified()?;
        Ok(Self {
            version: 2,
            project: aios_core::get_db_option().project_name.clone(),
            source_token: token.to_owned(),
            policy: policy(),
            dependency_tokens,
            empty: false,
            coverage: None,
            expected_base: None,
            changed_roots: Vec::new(),
        })
    }

    /// Capture before applying the window. Missing/invalid legacy proof keeps the
    /// strict full-inventory validation path; a gap never becomes an empty delta.
    pub(crate) async fn bind_base_on(&mut self, db: &Surreal<Any>, dbnum: u32, start: i32) -> anyhow::Result<()> {
        if let Some((base, raw)) = coverage_on(db, dbnum).await? {
            let target = FrozenSource::from_commit_token_json(&self.source_token)?.target_sesno() as i32;
            if start <= base && base <= target {
                self.coverage = Some(WindowCoverage {
                    base_sesno: base, window_start: start,
                    base_digest: super::digest::digest(raw.as_bytes()),
                });
                self.expected_base = Some(raw);
            }
        }
        Ok(())
    }

    pub fn source_token(&self) -> &str {
        &self.source_token
    }

    pub fn validate(&self, dbnum: u32, sesno: i32) -> anyhow::Result<()> {
        ensure!(
            matches!(self.version, 1 | 2) && self.policy == policy(),
            "completion inventory policy changed"
        );
        ensure!(
            self.project == aios_core::get_db_option().project_name,
            "completion project changed"
        );
        let source = FrozenSource::from_commit_token_json(&self.source_token)?;
        if let Some(coverage) = &self.coverage {
            ensure!(self.version == 2 && coverage.window_start <= coverage.base_sesno
                && coverage.base_sesno <= sesno && !coverage.base_digest.is_empty(), "invalid completion coverage");
        }
        ensure!(
            source.dbnum() == dbnum as i32 && source.target_sesno() as i32 == sesno,
            "completion source target mismatch"
        );
        let (_, selected) = super::model_source::source_file_of(dbnum)?;
        ensure!(
            source.matches_path(&selected),
            "completion MDB source changed"
        );
        source.reopen_verified()?;
        let (pins, _) = crate::fast_model::e3d_model_service::current_mdb_sources()?;
        let current_dependencies: std::collections::BTreeSet<_> = pins
            .into_iter()
            .filter(|pin| matches!(pin.db_type.as_str(), "CATA" | "PROP"))
            .map(|pin| pin.dbnum)
            .collect();
        let expected_dependencies = self
            .dependency_tokens
            .iter()
            .map(|token| Ok(FrozenSource::from_commit_token_json(token)?.dbnum()))
            .collect::<anyhow::Result<std::collections::BTreeSet<_>>>()?;
        ensure!(
            current_dependencies == expected_dependencies,
            "completion dependency membership changed"
        );
        for token in &self.dependency_tokens {
            let dependency = FrozenSource::from_commit_token_json(token)?;
            let current = super::model_source::latest_source_version(dependency.dbnum() as u32)?;
            ensure!(
                dependency.matches_path(&current.file)
                    && dependency.target_sesno() == current.sesno,
                "completion dependency changed: {}",
                dependency.dbnum()
            );
            dependency.reopen_verified()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(empty: bool) -> CompletionReceipt {
        CompletionReceipt {
            version: 1,
            project: "test".into(),
            source_token: "frozen".into(),
            policy: policy(),
            dependency_tokens: vec![],
            empty,
            coverage: None,
            expected_base: None,
            changed_roots: Vec::new(),
        }
    }

    #[tokio::test]
    async fn completion_commits_receipt_and_watermark_atomically() {
        // (现场改动, 收据是否声称空模型, 期望被哪道闸挡下——None = 收据落地)
        //
        // 2026-09-10：原先头三格钉的是「文件轮次那一行换了令牌 / 退回 collecting 就挡下」
        // （`rejection::FILE_ROUND_CHANGED`）。收据搬到水位行之后没有那一行可问，同一件事
        // 由任务自己担保——走到这里它刚把这一轮的空间发布完，令牌就是它手里那一个。
        // 三格随那道闸一起去掉，剩下的七道闸一道没少。
        let cases: [(&str, bool, Option<&str>); 10] = [
            ("", true, None),
            (
                "DELETE dbnum_watermark:8000;",
                true,
                Some(rejection::DATA_WATERMARK_BEHIND),
            ),
            (
                "CREATE model_update_pending:x SET dbnum=8000, action='delete_cleanup';",
                true,
                Some(rejection::MODEL_WORK_PENDING),
            ),
            (
                "CREATE inst_relate:x SET dbnum=8000;",
                true,
                Some(rejection::EMPTY_BUT_PROJECTION_EXISTS),
            ),
            (
                "CREATE gen_root:x SET dbnum=8000, status='Generated', source_end_sesno=12;",
                true,
                Some(rejection::EMPTY_BUT_ROOTS_EXIST),
            ),
            ("", false, Some(rejection::NO_ROOT_EVIDENCE)),
            (
                "CREATE gen_root:x SET dbnum=8000, status='Generated', source_end_sesno=12;",
                false,
                None,
            ),
            // 凭证落后于 T 的安定根**不再拦收据**（d-427）：这一问要求整库每一根都被
            // 前移到 T，而前移已退役。「够不够新」交给库级水位，这里只问「有没有根在赶」。
            (
                "CREATE gen_root:x SET dbnum=8000, status='Generated', source_end_sesno=11;",
                false,
                None,
            ),
            (
                "CREATE gen_root:x SET dbnum=8000, status='Failed';",
                false,
                Some(rejection::UNSETTLED_ROOTS),
            ),
            (
                "CREATE gen_root:x SET dbnum=8000, status='http_422', source_end_sesno=12;",
                false,
                Some(rejection::UNSETTLED_ROOTS),
            ),
        ];
        for (change, empty, rejected_by) in cases {
            let success = rejected_by.is_none();
            let db = surrealdb::engine::any::connect("mem://").await.unwrap();
            db.use_ns("completion").use_db("atomic").await.unwrap();
            db.query(format!("CREATE dbnum_watermark:8000 SET applied_sesno=12, model_sesno=0; \
                CREATE incr_side_effect_pending:spatial_reconcile_8000_12 SET dbnum=8000, end_sesno=12, \
                status='done', room_released=true, commit_token='frozen'; {change}"))
                .await.unwrap().check().unwrap();
            let result = commit_on(&db, 8000, 12, &receipt(empty)).await;
            assert_eq!(result.is_ok(), success, "{change}: {result:?}");
            if let Err(error) = &result {
                assert_eq!(
                    rejection_of(error),
                    rejected_by,
                    "{change}（empty={empty}）要被说对是哪道闸：{error:#}"
                );
            }
            // 收据现在住在水位行上（`receipt_row`），不再寄居在文件轮次那一行。
            let mut observed = db
                .query(
                    "RETURN dbnum_watermark:8000.model_sesno; \
                RETURN dbnum_watermark:8000.model_completion; \
                RETURN dbnum_watermark:8000.model_completion_sesno;",
                )
                .await
                .unwrap()
                .check()
                .unwrap();
            let watermark: Option<i32> = observed.take(0).unwrap();
            let completion: Option<String> = observed.take(1).unwrap();
            let completion_sesno: Option<i32> = observed.take(2).unwrap();
            assert_eq!(completion.is_some(), success, "{change}");
            assert_eq!(
                completion_sesno,
                success.then_some(12),
                "{change}: 收据要说清自己是给哪一版开的"
            );
            assert_eq!(watermark == Some(12), success, "{change}: completion and watermark are atomic");
        }
    }

    #[tokio::test]
    async fn continuous_window_keeps_unchanged_root_versions_and_rejects_races() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("completion").use_db("coverage").await.unwrap();
        db.query("CREATE dbnum_watermark:8000 SET applied_sesno=12, model_sesno=10, model_completion='base';
            CREATE gen_root:8000_1 SET dbnum=8000,status='Generated',source_end_sesno=4;
            CREATE gen_root:8000_2 SET dbnum=8000,status='Generated',source_end_sesno=12;").await.unwrap().check().unwrap();
        let mut next = receipt(false);
        next.version = 2;
        next.coverage = Some(WindowCoverage { base_sesno: 10, window_start: 10, base_digest: super::super::digest::digest(b"base") });
        next.expected_base = Some("base".into());
        next.changed_roots = vec!["8000/2".into()];
        commit_on(&db, 8000, 12, &next).await.unwrap();
        let old: Option<i32> = db.query("RETURN gen_root:8000_1.source_end_sesno;").await.unwrap().take(0).unwrap();
        assert_eq!(old, Some(4));
        assert!(commit_on(&db, 8000, 12, &next).await.is_err(), "stale baseline CAS");
        db.query("UPDATE dbnum_watermark:8000 SET model_sesno=10, model_completion='base';
            UPDATE gen_root:8000_2 SET publication_status='stale';").await.unwrap().check().unwrap();
        assert!(commit_on(&db, 8000, 12, &next).await.is_err(), "failed publication");
        let watermark: Option<i32> = db.query("RETURN dbnum_watermark:8000.model_sesno;").await.unwrap().take(0).unwrap();
        assert_eq!(watermark, Some(10));
    }

    #[test]
    fn malformed_and_unversioned_receipts_never_count_as_completed() {
        for raw in ["{}", "null", "legacy", "{\"version\":99}"] {
            assert!(validated_receipt(Some(raw), 8000, 12).is_none());
        }
        assert!(validated_receipt(None, 8000, 12).is_none());
    }

    #[tokio::test]
    async fn model_checkpoint_preserves_unfinished_rooms() {
        use super::super::side_effect_pending::SideEffectCompensator;
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("completion")
            .use_db("independent_stages")
            .await
            .unwrap();
        db.query(
            "CREATE dbnum_watermark:8000 SET applied_sesno=12, model_sesno=10,
            room_sesno=8, room_input_version='previous-room-proof';
            CREATE incr_side_effect_pending:spatial_reconcile_8000_12 SET dbnum=8000, end_sesno=12,
            status='done', room_released=false, commit_token='frozen';
            CREATE model_update_pending:room SET dbnum=8000, action='room_recalc_element',
            target_refno='8000/1', attempts=9999, status='pending';",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        let proof = SideEffectCompensator::published_model_file_round_on(&db, 8000, 12)
            .await
            .unwrap();
        assert_eq!(proof.commit_token.as_deref(), Some("frozen"));
        assert!(
            SideEffectCompensator::completed_file_round_on(&db, 8000, 12)
                .await
                .is_err()
        );
        let probe = super::super::model_watermark::read_model_watermark_probe(&db, 8000)
            .await
            .unwrap();
        assert_eq!(
            probe.abandoned, 0,
            "room failures are not abandoned model roots"
        );
        commit_on(&db, 8000, 12, &receipt(true)).await.unwrap();
        // 收据落完再由收尾那一笔盖水位（ADR-071 D3）：两段分开写，房间那一枚不受影响。
        crate::data_interface::task_end::commit_task_end_on(
            &db,
            &crate::data_interface::task_end::TaskEnd {
                dbnum: 8000,
                target_sesno: 12,
                target_time: None,
                model: Some(crate::data_interface::task_end::TaskModelOutcome {
                    all_roots_ok: true,
                    dead_roots: 0,
                }),
                rooms: None,
                info: None,
                closes_task: true,
            },
        )
        .await
        .unwrap();
        let mut result = db
            .query(
                "RETURN dbnum_watermark:8000.model_sesno;
            RETURN dbnum_watermark:8000.room_sesno;
            RETURN dbnum_watermark:8000.room_input_version;
            RETURN incr_side_effect_pending:spatial_reconcile_8000_12.room_released;
            RETURN model_update_pending:room.attempts;",
            )
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(result.take::<Option<i32>>(0).unwrap(), Some(12));
        assert_eq!(result.take::<Option<i32>>(1).unwrap(), Some(8));
        assert_eq!(
            result.take::<Option<String>>(2).unwrap().as_deref(),
            Some("previous-room-proof")
        );
        assert_eq!(result.take::<Option<bool>>(3).unwrap(), Some(false));
        assert_eq!(result.take::<Option<i32>>(4).unwrap(), Some(9999));
        db.query("UPDATE incr_side_effect_pending:spatial_reconcile_8000_12 SET status='pending';")
            .await
            .unwrap()
            .check()
            .unwrap();
        assert!(
            SideEffectCompensator::published_model_file_round_on(&db, 8000, 12)
                .await
                .is_err()
        );
    }
}

pub(crate) fn validated_receipt(
    raw: Option<&str>,
    dbnum: u32,
    sesno: i32,
) -> Option<CompletionReceipt> {
    let receipt: CompletionReceipt = serde_json::from_str(raw?).ok()?;
    receipt.validate(dbnum, sesno).ok()?;
    Some(receipt)
}

/// `commit_on` 事务里各道闸的 THROW 原话。**一道闸一句话**：「凭证前移退化」（未触根的
/// `source_end_sesno` 落在 T 后面）与「有根没安定」是两个正常现场，不是「证据变了」这种
/// 该当事故报的事；调用方用 [`rejection_of`] 分型，不自己猜字串。
pub(crate) mod rejection {
    pub const FILE_ROUND_CHANGED: &str = "completion file round changed";
    pub const DATA_WATERMARK_BEHIND: &str = "completion data watermark missing or behind";
    /// 非空清单，却一行 `gen_root` 都没有。
    pub const NO_ROOT_EVIDENCE: &str = "completion has no root evidence";
    /// 有根未安定（仍在赶或已放弃）。
    pub const UNSETTLED_ROOTS: &str = "completion has unsettled roots";
    /// 根都安定了，但有根的凭证还停在 T 之前（未触根凭证前移退化，ADR-056 实施约束 5）。
    pub const ROOT_CREDENTIALS_LAG: &str = "completion root credentials lag behind target";
    /// 收据说空模型，库里却有 `gen_root` 行。
    pub const EMPTY_BUT_ROOTS_EXIST: &str = "completion claims empty but roots exist";
    /// 收据说空模型，库里却有旧管线投影。
    pub const EMPTY_BUT_PROJECTION_EXISTS: &str = "completion claims empty but projection exists";
    pub const MODEL_WORK_PENDING: &str = "completion still has model work";

    pub const ALL: [&str; 8] = [
        FILE_ROUND_CHANGED,
        DATA_WATERMARK_BEHIND,
        NO_ROOT_EVIDENCE,
        UNSETTLED_ROOTS,
        ROOT_CREDENTIALS_LAG,
        EMPTY_BUT_ROOTS_EXIST,
        EMPTY_BUT_PROJECTION_EXISTS,
        MODEL_WORK_PENDING,
    ];
}

/// 从 [`commit_on`] 的错误里认出是哪道闸挡的（回 [`rejection`] 里的那个常量）；
/// 不是闸的错误（连接、语法、编码）回 `None`，调用方原样上抛。
pub(crate) fn rejection_of(error: &anyhow::Error) -> Option<&'static str> {
    let text = format!("{error:#}");
    rejection::ALL
        .into_iter()
        .find(|reason| text.contains(reason))
}

/// Every guard is rechecked in the commit transaction. A changed file round,
/// outstanding model action or missing watermark aborts both writes.
/// 落一张任务完成收据。
///
/// 收据与模型水位同事务提交。连续窗口检查捕获的基线与变化根，
/// 没有有效基线时保留完整根证据核验；任何失败均不发布完成凭证。
pub(crate) async fn commit_on(
    db: &Surreal<Any>,
    dbnum: u32,
    sesno: i32,
    receipt: &CompletionReceipt,
) -> anyhow::Result<()> {
    let id = receipt_row(dbnum);
    let encoded = serde_json::to_string(&serde_json::to_string(receipt)?)?;
    let settled = super::model_watermark::settled_root_predicate();
    // 根证据拆成一道闸一句话（`rejection`）。非空清单：先问有没有根，再问有没有未安定的
    // （仍在赶 + 已放弃，即 `blocking_root_predicate` 的补集加 `http_422`，合起来就是
    // `NOT settled`）。第三问「安定的凭证够不够到 T」2026-09-10 随 d-427 撤掉，理由写在
    // 那一支上；`coverage` 那一支的逐根凭证检查不在此列，照旧。
    let root_guards = if let Some(coverage) = &receipt.coverage {
        let expected = receipt.expected_base.as_ref().context("completion coverage has no captured baseline")?;
        ensure!(super::digest::digest(expected.as_bytes()) == coverage.base_digest,
            "completion baseline digest mismatch");
        let expected = serde_json::to_string(expected)?;
        let mut guards = format!(
            "IF {id}.model_sesno != {} OR {id}.model_completion != {expected} \
             {{ THROW 'completion baseline changed'; }}; \
             IF array::len((SELECT VALUE id FROM gen_root WHERE dbnum={dbnum} \
             AND NOT ({settled}) LIMIT 1)) > 0 {{ THROW '{}'; }}; ",
            coverage.base_sesno, rejection::UNSETTLED_ROOTS);
        for root in &receipt.changed_roots {
            let key = super::dbnum_state::escape_surql_str(&root.replace('/', "_"));
            guards.push_str(&format!("IF array::len((SELECT id FROM [gen_root:{key}] WHERE dbnum={dbnum} AND ({settled}) AND source_end_sesno >= {sesno})) != 1 {{ THROW '{}'; }}; ", rejection::ROOT_CREDENTIALS_LAG));
        }
        guards
    } else if receipt.empty {
        format!(
            "IF array::len((SELECT VALUE id FROM gen_root WHERE dbnum={dbnum} LIMIT 1)) > 0 \
                {{ THROW '{}'; }}; \
            IF ({}) {{ THROW '{}'; }}; ",
            rejection::EMPTY_BUT_ROOTS_EXIST,
            projection_presence_expr(dbnum),
            rejection::EMPTY_BUT_PROJECTION_EXISTS,
        )
    } else {
        // 这里**不再**问「每一根安定根的凭证够不够到 T」（共识 d-427，2026-09-10）。
        //
        // 那一问要求整库每一根的 `source_end_sesno` 都被推到 T，而唯一会去推它的
        // ADR-056 P2-2 凭证前移已经退役——现场 7997 因此死锁：八行 `gen_root` 里六行
        // 停在 165，数据已到 170，收据永远提交不了，`model_sesno` 永远停在 165，每次
        // 启动还白排一个注定失败的 `intent=model_only` 恢复 Task。
        //
        // 留下的两问仍然是完整的根证据：有根、且没有根在赶。「这一版够不够新」从此
        // 由库级水位表达，不再要求每一根各自作证。上面 `coverage` 那一支的逐根凭证
        // 检查**照旧保留**——它只点名本轮真正重生成的根，那些根的凭证本来就该落在 T。
        format!(
            "IF array::len((SELECT VALUE id FROM gen_root WHERE dbnum={dbnum} LIMIT 1)) = 0 \
                {{ THROW '{}'; }}; \
            IF array::len((SELECT VALUE id FROM gen_root WHERE dbnum={dbnum} \
                AND NOT ({settled}) LIMIT 1)) > 0 {{ THROW '{}'; }}; ",
            rejection::NO_ROOT_EVIDENCE,
            rejection::UNSETTLED_ROOTS,
        )
    };
    let mut response = db
        .query(format!(
            // 原先第一道闸是「文件轮次那一行还是 done 且令牌没换」。账本退役后没有那一行
            // 可问，同一件事由任务自己担保：走到这里它刚把这一轮的空间发布完
            // （`publish_task_spatial_effects`），令牌就是它手里那一个
            // （`complete_model_task_on` 已断言收据的 source_token 与之相等）。
            "BEGIN TRANSACTION; \
        IF NOT (dbnum_watermark:{dbnum}.applied_sesno >= {sesno}) \
            {{ THROW '{}'; }}; \
        {root_guards}\
        IF array::len((SELECT VALUE id FROM model_update_pending WHERE dbnum={dbnum} \
            AND action NOT IN ['room_recalc_element','room_recalc_panel'] LIMIT 1)) > 0 \
            {{ THROW '{}'; }}; \
        IF ({id}.model_sesno ?: 0) > {sesno} {{ THROW 'completion superseded'; }}; \
        UPDATE {id} SET model_completion = {encoded}, model_completion_sesno = {sesno}, \
            model_sesno = {sesno}, model_settled_at = time::now(), \
            state_revision = (state_revision ?: 0) + 1; \
        COMMIT TRANSACTION;",
            rejection::DATA_WATERMARK_BEHIND,
            rejection::MODEL_WORK_PENDING,
        ))
        .await
        .context("commit model completion")?;
    // 不用 `.check()`：回滚掉的事务会给 THROW 之外的每一条语句都报一句笼统的「事务失败」，
    // `.check()` 只回下标最小的那一条，第二道闸起的原话就永远看不到。全部带序号留下，
    // `rejection_of` 才认得出是哪道闸。
    let mut errors = response.take_errors().into_iter().collect::<Vec<_>>();
    if !errors.is_empty() {
        errors.sort_by_key(|(index, _)| *index);
        anyhow::bail!(
            "model completion transaction of dbnum {dbnum} failed: {}",
            errors
                .into_iter()
                .map(|(index, error)| format!("statement {index}: {error}"))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    // 这一笔现在写的是**水位行**（收据搬家之后，`receipt_row`），所以它也得守水位镜像的
    // 那条规矩：写者落库后作废那一格，否则紧随其后的读者（房间那一段就在下一步）拿到的
    // 还是写之前的事实。2026-09-10 实测：漏了这一句，1112 的房间读到 `applied_sesno=0`
    // 而库里是 725，房间整段被自己的前置条件挡下。
    super::dbnum_state::DbnumState::invalidate_mirrored_on(db, dbnum);
    super::dbnum_consistency::invalidate_census();
    Ok(())
}
