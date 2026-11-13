//! 数据窗口结构行的**同步**落库入口（ADR-071 D1 / D11）：`IncrementPipeline::apply_one` 把本窗
//! 的 pe / ATT_ / CATA 依赖行 / `ref_rev` / 随窗语句装成内存里的 [`IncrementWindowArtifact`]，
//! 随即在这里落成 SurrealDB 里的结构行，然后才走尾事务前移水位——行先落、水位最后。
//!
//! 写的是 Plant UI 与料表读的那几张表：`pe` / `<NOUN>` / `ATT_*` / `pe_owner`、CATA 依赖行、
//! `ref_rev`、`datacenter_version` 状态。渲染器与基线 / 老直写路**同一份**
//! （`IncrementPipeline::terminal_persist_batches` / `cata_dependency::persist_resolved_rows_on`）。
//! 水位与凭证不在这里——它们由任务的尾事务写（ADR-071 D4），这里一行水位都不碰。
//!
//! 顺序：CATA 依赖 → pe / ATT_（Replace，先删后插）→ `ref_rev` → 随窗语句 → 按产物名单失效缓存。
//! 每一步都是幂等的固定目标写：中途失败时任务返回错误、水位不动、attempt 记录保留，下一次
//! 同一固定区间整窗重放即收敛（`landing_the_same_window_twice_converges`）。任务与库之间没有
//! journal、lane 或 ACK（共识 d-285 / d-288 作废了 (a) 的异步账本）。
//!
//! 回执 warnings（UDA 键缺失、墓碑退化、CATA 闭包缺失）随 [`LandingReport`] 回给任务，进任务回执。
//!
//! 住址：2026-09-10（ADR-071 S7）从 `model_publish_queue/{structured_data_sink,increment_window_artifact}.rs`
//! 搬到这里——它们是去账本之后的**生产路**，只是还住在叫「发布队列 / sink / artifact」的目录里；
//! 名字与行为不变，产物在 [`artifact`]，落库口在本文件。

pub mod artifact;

use aios_core::options::DbOption;
use anyhow::{Context, ensure};
use serde::Serialize;
use surrealdb::{Surreal, engine::any::Any};

use crate::data_interface::ele_ops::UdaKeyMap;
use crate::data_interface::increment_pipeline::{
    IncrementPipeline, PERSIST_TX_CHUNK, persist_transaction_batches,
};
pub use artifact::IncrementWindowArtifact;

/// 一条数据窗口落库的账（任务回执 / 日志）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LandingReport {
    pub sequence: u64,
    pub dbnum: u32,
    pub from_sesno: u32,
    pub target_sesno: u32,
    pub elements: usize,
    pub element_transactions: usize,
    pub cata_elements: usize,
    pub cata_transactions: usize,
    pub reverse_index_statements: usize,
    pub window_statements: usize,
    /// 落库后失效的进程内缓存键数（Task 名单 ∪ 本窗元素与属主）。
    pub cache_invalidations: usize,
    pub warnings: Vec<String>,
}

/// 把一个数据窗口的内存产物落进 `db`。幂等：同一份产物重放收敛到同一终态。
/// `sequence` 只进回执（任务用目标会话号填），不参与任何判定。`option` 只在 CATA 依赖行触发前移
/// 刷新时用来解析目录根。
pub async fn land_increment_window(
    db: &Surreal<Any>,
    option: &DbOption,
    sequence: u64,
    window: &IncrementWindowArtifact,
) -> anyhow::Result<LandingReport> {
    let mut report = LandingReport {
        sequence,
        dbnum: window.dbnum,
        from_sesno: window.from_sesno,
        target_sesno: window.target_sesno,
        ..Default::default()
    };

    // 1. CATA 依赖块先于 DESI 块（plan 2026-09-06 §3 D6 (a)，与 `apply_one` 同序）。
    if let Some(rows) = window.cata_rows() {
        let cata = crate::data_interface::cata_dependency::persist_resolved_rows_on(
            db,
            option,
            &window.project,
            &rows,
        )
        .await
        .with_context(|| format!("land CATA dependency rows of dbnum {}", window.dbnum))?;
        report.cata_elements = cata.elements;
        report.cata_transactions = cata.transactions;
        report.warnings.extend(cata.warning(window.dbnum));
    }

    // 2. pe / ATT_ 主数据：与 `IncrementPipeline::persist_latest_main_data` 同一份渲染、同一个执行口。
    let changes = window.persist_changes();
    let retired = window.retired_rows();
    if !changes.is_empty() || !retired.is_empty() {
        let uda_keys = UdaKeyMap::load_on(db).await?;
        let (batches, elements) = IncrementPipeline::terminal_persist_batches(
            &changes,
            &retired,
            window.dbnum as i32,
            &uda_keys,
        )?;
        for sql in &batches {
            crate::data_interface::pe_batch::execute_on(db, sql)
                .await
                .with_context(|| format!("land pe batch of dbnum {}", window.dbnum))?;
        }
        report.elements = elements.elements;
        report.element_transactions = elements.transactions;
        report
            .warnings
            .extend(uda_keys.misses_warning(window.dbnum));
        report.warnings.extend(elements.warning(window.dbnum));
    }

    // 3. `ref_rev`：与 `maintain_reverse_index` 同一个分块口径。
    for chunk in window.reverse_index_statements.chunks(500) {
        let sql = chunk.join("\n");
        db.query(&sql)
            .await
            .map_err(|error| {
                anyhow::anyhow!("land reverse index of dbnum {}: {error}", window.dbnum)
            })?
            .check()
            .map_err(|error| {
                anyhow::anyhow!(
                    "land reverse index statement of dbnum {}: {error}",
                    window.dbnum
                )
            })?;
    }
    report.reverse_index_statements = window.reverse_index_statements.len();

    // 4. 随窗语句（`datacenter_version` 状态 + anc 修复）：固定目标 UPDATE，按块包成事务。
    for (index, batch) in persist_transaction_batches(&window.window_statements, PERSIST_TX_CHUNK)
        .iter()
        .enumerate()
    {
        let mut response = db.query(batch).await.map_err(|error| {
            anyhow::anyhow!(
                "land window batch {index} of dbnum {}: {error}",
                window.dbnum
            )
        })?;
        // 不用 `.check()`：回滚掉的事务给出错语句之外的每一条都报一句笼统的「事务失败」，
        // `.check()` 只回下标最小的那一条，真正的原话（THROW / 类型错）就看不到了。
        let mut errors = response.take_errors().into_iter().collect::<Vec<_>>();
        if !errors.is_empty() {
            errors.sort_by_key(|(statement, _)| *statement);
            anyhow::bail!(
                "land window batch {index} of dbnum {} failed: {}",
                window.dbnum,
                errors
                    .into_iter()
                    .map(|(statement, error)| format!("statement {statement}: {error}"))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }
    }
    report.window_statements = window.window_statements.len();

    // 5. 结构行落定之后再失效一遍进程内缓存。Task 在装产物之前已经失效过一次；落库期间读者
    //    拿到的是库里的旧行，会把缓存填回 S⁻；此刻不清，那一格就永久陈旧（直到下一窗又碰同一
    //    元素）。名单随产物来，见 `IncrementWindowArtifact::cache_refnos`。
    report.cache_invalidations = IncrementPipeline::invalidate_caches(&window.cache_refnos()).await;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_interface::e3d_collector::RetiredTypeRow;
    use crate::data_interface::ele_data::EleData;
    use crate::data_interface::ele_ops::{PersistElementChange, PersistElementState};
    use crate::data_interface::table_parity::{
        apply_all, changed_data_tables, fresh_mem_db, init_schema_on, snapshot_data_tables,
    };
    use aios_core::{NamedAttrValue, RefU64, RefU64Vec, RefnoEnum};

    const DBNUM: u32 = 7982;
    const BASE_SESNO: u32 = 41;
    const END_SESNO: u32 = 43;

    fn refu(n: u64) -> RefU64 {
        RefU64((4000000002u64 << 32) | n)
    }

    fn pe(refno: RefU64) -> String {
        RefnoEnum::from(refno).to_pe_key()
    }

    fn element(refno: RefU64, owner: RefU64, noun: &str, name: &str) -> EleData {
        let mut ele = EleData::default();
        ele.refno = refno;
        ele.owner = owner;
        ele.noun = aios_core::tool::db_tool::db1_hash(noun);
        ele.name = name.into();
        let map = &mut ele.whole_attmap.attmap.map;
        map.insert("REFNO".into(), NamedAttrValue::RefU64Type(refno));
        map.insert("OWNER".into(), NamedAttrValue::RefU64Type(owner));
        map.insert("TYPE".into(), NamedAttrValue::StringType(noun.into()));
        map.insert("NAME".into(), NamedAttrValue::StringType(name.into()));
        map.insert("DBNUM".into(), NamedAttrValue::IntegerType(DBNUM as i32));
        map.insert(
            "POS".into(),
            NamedAttrValue::Vec3Type(glam::Vec3::new(1.0, 2.5, -3.0)),
        );
        ele
    }

    /// 窗口前的世界：SITE 下一个 EQUI，EQUI 下一个将被删的 BOX 与一个将复用 refno 的 NOZZ。
    fn base_statements() -> Vec<String> {
        let (site, equi, gone, reused) = (refu(1), refu(2), refu(3), refu(4));
        vec![
            format!(
                "UPSERT {} CONTENT {{ noun: 'SITE', deleted: false, refno: SITE:⟨{site}⟩, dbnum: {DBNUM} }};",
                pe(site)
            ),
            format!("UPSERT SITE:⟨{site}⟩ CONTENT {{ TYPE: 'SITE', NAME: '/SD-SITE' }};"),
            format!(
                "UPSERT {} CONTENT {{ noun: 'EQUI', deleted: false, owner: {}, refno: EQUI:⟨{equi}⟩, dbnum: {DBNUM} }};",
                pe(equi),
                pe(site)
            ),
            format!("UPSERT EQUI:⟨{equi}⟩ CONTENT {{ TYPE: 'EQUI', NAME: '/SD-EQUI' }};"),
            format!(
                "UPSERT {} CONTENT {{ noun: 'BOX', deleted: false, owner: {}, refno: BOX:⟨{gone}⟩, dbnum: {DBNUM} }};",
                pe(gone),
                pe(equi)
            ),
            format!("UPSERT BOX:⟨{gone}⟩ CONTENT {{ TYPE: 'BOX', NAME: '/SD-GONE' }};"),
            format!(
                "UPSERT {} CONTENT {{ noun: 'NOZZ', deleted: false, owner: {}, refno: NOZZ:⟨{reused}⟩, dbnum: {DBNUM} }};",
                pe(reused),
                pe(equi)
            ),
            format!("UPSERT NOZZ:⟨{reused}⟩ CONTENT {{ TYPE: 'NOZZ', NAME: '/SD-OLD-NOZZ' }};"),
            format!(
                "UPSERT dbnum_watermark:{DBNUM} SET dbnum = {DBNUM}, applied_sesno = {BASE_SESNO};"
            ),
        ]
    }

    /// 合成窗口终态：新增 BOX、改 EQUI 的 NAME 并挂上新成员、软删 `gone`、以 BOX 重建 `reused`。
    fn window_changes() -> (Vec<PersistElementChange>, Vec<RetiredTypeRow>) {
        let (site, equi, gone, reused, new_box) = (refu(1), refu(2), refu(3), refu(4), refu(5));
        let mut equi_now = element(equi, site, "EQUI", "/SD-EQUI-2");
        equi_now.children = RefU64Vec(vec![reused, new_box]);
        let changes = vec![
            PersistElementChange {
                refno: new_box,
                sesno: 42,
                state: PersistElementState::Add(element(new_box, equi, "BOX", "/SD-NEW-BOX")),
            },
            PersistElementChange {
                refno: equi,
                sesno: END_SESNO,
                state: PersistElementState::Modified(equi_now),
            },
            PersistElementChange {
                refno: gone,
                sesno: END_SESNO,
                state: PersistElementState::Deleted {
                    base: Some(element(gone, equi, "BOX", "/SD-GONE")),
                },
            },
            PersistElementChange {
                refno: reused,
                sesno: END_SESNO,
                state: PersistElementState::Add(element(reused, equi, "BOX", "/SD-REUSED-BOX")),
            },
        ];
        let retired = vec![RetiredTypeRow {
            refno: reused,
            old_noun: "NOZZ".into(),
            new_noun: "BOX".into(),
        }];
        (changes, retired)
    }

    fn side_statements() -> (Vec<String>, Vec<String>) {
        let (equi, new_box) = (refu(2), refu(5));
        (
            vec![format!(
                "UPSERT ref_rev_probe:⟨{new_box}⟩ SET referrer = {}, referenced = {};",
                pe(new_box),
                pe(equi)
            )],
            // 随窗语句照生产形态不带分号：`wrap_in_transaction` 会补。
            vec![
                format!(
                    "UPSERT datacenter_probe:⟨{equi}⟩ SET status = 'Modify', sesno = {END_SESNO}"
                ),
                format!("UPSERT anc_probe:⟨{new_box}⟩ SET anc = [{}]", pe(equi)),
            ],
        )
    }

    fn artifact() -> IncrementWindowArtifact {
        let (changes, retired) = window_changes();
        let (reverse_index, window_statements) = side_statements();
        IncrementWindowArtifact::from_window(
            "SD",
            DBNUM,
            "DESI",
            BASE_SESNO,
            END_SESNO,
            &changes,
            &retired,
            None,
            reverse_index,
            window_statements,
            [],
        )
    }

    async fn prepared_db(name: &str) -> Surreal<Any> {
        let db = fresh_mem_db("sink_parity", name).await.expect("mem db");
        init_schema_on(&db).await.expect("schema");
        apply_all(&db, &base_statements())
            .await
            .expect("base world");
        db
    }

    /// 逐段直写（`terminal_persist_batches` + ref_rev + 随窗语句，基线与老 Task 路的写法）与
    /// 窗口产物一次落库（`land_increment_window`）落出来的数据面逐表相等——渲染器是同一份，
    /// 落库入口换了形状不能换结果。
    #[tokio::test]
    async fn landing_the_window_product_equals_the_segment_by_segment_direct_write() {
        let direct = prepared_db("direct").await;
        let (changes, retired) = window_changes();
        let (batches, _) = IncrementPipeline::terminal_persist_batches(
            &changes,
            &retired,
            DBNUM as i32,
            &UdaKeyMap::empty(),
        )
        .expect("render");
        for sql in &batches {
            crate::data_interface::pe_batch::execute_on(&direct, sql)
                .await
                .expect("direct pe batch");
        }
        let (reverse_index, window_statements) = side_statements();
        apply_all(&direct, &reverse_index)
            .await
            .expect("direct ref_rev");
        apply_all(&direct, &window_statements)
            .await
            .expect("direct window statements");

        let via_landing = prepared_db("landing").await;
        let option = DbOption::default();
        let landing =
            land_increment_window(&via_landing, &option, u64::from(END_SESNO), &artifact())
                .await
                .expect("landing");
        assert_eq!(
            (landing.sequence, landing.dbnum),
            (u64::from(END_SESNO), DBNUM)
        );
        assert_eq!(
            (landing.from_sesno, landing.target_sesno),
            (BASE_SESNO, END_SESNO)
        );
        assert_eq!(landing.elements, 4, "四条终态");
        assert_eq!(landing.reverse_index_statements, 1);
        assert_eq!(landing.window_statements, 2);

        let expected = snapshot_data_tables(&direct)
            .await
            .expect("direct snapshot");
        let actual = snapshot_data_tables(&via_landing)
            .await
            .expect("landing snapshot");
        assert_eq!(
            actual, expected,
            "窗口产物落出来的数据面必须与逐段直写逐表相等"
        );
    }

    /// 同一份工件再落一遍，终态不变：这是「中途失败 → 整条重放」能收敛的前提。
    #[tokio::test]
    async fn landing_the_same_window_twice_converges() {
        let db = prepared_db("twice").await;
        let option = DbOption::default();
        let window = artifact();
        land_increment_window(&db, &option, 1, &window)
            .await
            .expect("first");
        let once = snapshot_data_tables(&db).await.expect("snapshot");
        land_increment_window(&db, &option, 1, &window)
            .await
            .expect("second");
        let twice = snapshot_data_tables(&db).await.expect("snapshot");
        assert_eq!(twice, once);
    }

    /// 随窗语句里有一句坏的：落库返回错误（任务据此不走尾事务、水位不动），pe 那一段已经落了
    /// 也没关系——同一固定区间重放一次，Replace 收敛到与一次成功相同的终态。
    #[tokio::test]
    async fn a_failing_statement_errors_out_and_a_replay_converges() {
        let db = prepared_db("failing").await;
        let option = DbOption::default();
        // 随窗语句的目标表在生产里都是启动时 DEFINE 好的；`mem://` 上让一张**在被回滚的事务里
        // 首次隐式定义**的表再被写，写会静默落空（fork 2.1 的怪癖，与本模块无关），所以两边都
        // 先把探针表定义出来，让比对只看落库逻辑。
        let probe_tables =
            "DEFINE TABLE datacenter_probe SCHEMALESS; DEFINE TABLE anc_probe SCHEMALESS;";
        db.query(probe_tables)
            .await
            .expect("define")
            .check()
            .expect("define probe tables");
        let mut broken = artifact();
        // 注意 `THIS IS NOT SURQL` 反而是合法的 SurrealQL（裸表达式语句），要用 THROW 才真失败。
        broken
            .window_statements
            .push("THROW 'landing test: forced failure'".into());
        let error = land_increment_window(&db, &option, u64::from(END_SESNO), &broken)
            .await
            .expect_err("must fail");
        assert!(
            format!("{error:#}").contains("forced failure"),
            "错误要带出 THROW 原话: {error:#}"
        );

        land_increment_window(&db, &option, u64::from(END_SESNO), &artifact())
            .await
            .expect("replay of the good window");
        let replayed = snapshot_data_tables(&db).await.expect("snapshot");
        let clean = prepared_db("failing_clean").await;
        clean
            .query(probe_tables)
            .await
            .expect("define")
            .check()
            .expect("define probe tables");
        land_increment_window(&clean, &option, u64::from(END_SESNO), &artifact())
            .await
            .expect("clean landing");
        let once = snapshot_data_tables(&clean).await.expect("snapshot");
        let changed = changed_data_tables(&once, &replayed);
        assert!(
            changed.is_empty(),
            "失败后重放与一次成功终态相同；不同的表 {changed:?}：一次成功 {:?} / 重放 {:?}",
            changed
                .iter()
                .map(|table| once.get(table))
                .collect::<Vec<_>>(),
            changed
                .iter()
                .map(|table| replayed.get(table))
                .collect::<Vec<_>>()
        );
    }
}
