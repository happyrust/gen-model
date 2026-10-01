//! 跨库交接（GitHub #34 前置 2 / #35，2026-10-01）：一个设计库的窗口里级联命中、却落在**别的设计库**里的元素，
//! 交给那个库自己的任务重算它们的生成根。
//!
//! 谁会产生它：结构连接级联的跨库对端（[`StructuralConnectionCascade::far_end_cross_db`]，#33 在 AvevaPlantSample
//! 量到 12 条 `FIXING.CREF ↔ GENSEC.JOIE`）与电缆端点级联的跨库引用者（[`CableEndpointCascade::foreign`]，C6 普查
//! 196 条 `CABLE.STAREF` / `ENDREF` 跨库边）。它们的根在那个库的 `DbSet@T` 上，本窗的计划（`roots_T` 只有本库）装不下。
//!
//! **形态与目录级联同一条路**（`model_coverage::enqueue_catalogue_cascade`，ADR-071 S4）：每个对端库一条 `ModelOnly`
//! 任务，名单随行（[`DiscoveredBatch::regen_roots`]），目标会话 = 那个库的文件最新，`model_start_sesno` 也取它——
//! 规划器不开窗，进工作单的只有名单与账本里还欠着的根；已有排队行就并进去（名单取并集），同库任务在跑就接在它后面
//! （`BatchScheduler::enqueue`）。**不另设停放槽**：房间反查要停放（ADR-073 D13 U3 b `RoomRoots`），是因为房间的
//! 欠账按「代」计、要接准入链；模型根没有代，一条带名单的任务就说得清欠什么。
//!
//! 元素 → 根在**执行时**、在那个库的文件最新会话上折（[`fold_elements_to_roots`]：那个库 T 端 owner 索引上溯，链上
//! 每一个落在根集里的根都算——与同库对端 [`roots_on_owner_chain`] 同一口径），不在规划时折：预览不开别的库，任务的
//! 目标会话与折根的会话是同一个。
//!
//! 先后无所谓：对端根的几何读本库的元素是读**文件**（`build_set` 里本库钉当前 pin），不读本库的模型；本窗落地时本库
//! 文件已在 T，对端任务什么时候跑都读得到 T 或更新的会话。
//!
//! 只记账、不阻断：交接不成（不在监听范围 / 不在 MDB / 开不了 / 折不出根）逐库一行进本窗回执 `warnings`，本窗照常收尾
//! ——那几个根停在旧几何，回执说得出是哪几个、为什么。
//!
//! [`StructuralConnectionCascade::far_end_cross_db`]: crate::data_interface::structural_connection::StructuralConnectionCascade::far_end_cross_db
//! [`CableEndpointCascade::foreign`]: crate::data_interface::cable_endpoint::CableEndpointCascade::foreign
//! [`DiscoveredBatch::regen_roots`]: crate::data_interface::batch_scheduler::DiscoveredBatch::regen_roots

use std::collections::{BTreeMap, BTreeSet};

use e3d_io::refno::RefNo;

use crate::data_interface::batch_scheduler::{DiscoveredBatch, RegenRoot};
use crate::data_interface::generation_root::{GenerationRoot, refno_to_e3d};
use crate::data_interface::model_source::SourceVersion;
use crate::data_interface::structural_connection::roots_on_owner_chain;

/// 回执里逐库点名的根 / 元素上限，多出来的只报个数。
const NAMED_LIMIT: usize = 12;

/// 纯函数：级联命中的跨库元素按所在库分组（`a/b`，去重、按 refno 升序）。无效 refno 丢掉。
pub fn elements_by_dbnum(elements: impl IntoIterator<Item = RefNo>) -> BTreeMap<u32, Vec<String>> {
    let mut grouped: BTreeMap<u32, BTreeSet<(u32, u32)>> = BTreeMap::new();
    for element in elements.into_iter().filter(RefNo::is_valid) {
        grouped
            .entry(element.dbno())
            .or_default()
            .insert((element.word0, element.word1));
    }
    grouped
        .into_iter()
        .map(|(dbnum, keys)| {
            let refnos = keys
                .into_iter()
                .map(|(word0, word1)| RefNo::new(word0, word1).to_string())
                .collect();
            (dbnum, refnos)
        })
        .collect()
}

/// `a/b` → [`RefNo`]；拼不出来或 `word1` 为负给 `None`。
pub fn parse_refno(text: &str) -> Option<RefNo> {
    let (word0, word1) = text.trim().split_once('/')?;
    let word0 = word0.trim().parse::<u32>().ok()?;
    let word1 = u32::try_from(word1.trim().parse::<i64>().ok()?).ok()?;
    RefNo::new(word0, word1)
        .is_valid()
        .then(|| RefNo::new(word0, word1))
}

/// 一个对端库的折根结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FoldedRoots {
    /// 要重算的根（去重，按元素出现序、链上自下而上）。
    pub roots: Vec<RegenRoot>,
    /// 在那个库 T 端 owner 链上找不到任何根的元素（已删 / 无根族），点名。
    pub unresolved: Vec<String>,
}

/// 纯函数：在对端库 T 端的 owner 索引与根集上把元素折成根——owner 链上（含自身）每一个落在根集里的根都算，
/// 与同库对端 [`roots_on_owner_chain`] 同一口径（嵌套显著根各管各的几何，都要重算）。
pub fn fold_elements_to_roots(
    owners: &BTreeMap<(u32, u32), RefNo>,
    roots_t: &[GenerationRoot],
    elements: &[RefNo],
) -> FoldedRoots {
    let by_key: BTreeMap<(u32, u32), &GenerationRoot> = roots_t
        .iter()
        .map(|root| {
            let refno = refno_to_e3d(root.root);
            ((refno.word0, refno.word1), root)
        })
        .collect();
    let keys: BTreeSet<(u32, u32)> = by_key.keys().copied().collect();
    let mut out = FoldedRoots::default();
    let mut seen = BTreeSet::new();
    for element in elements {
        let chain = roots_on_owner_chain(owners, &keys, *element);
        if chain.is_empty() {
            out.unresolved.push(element.to_string());
            continue;
        }
        for root in chain {
            let key = (root.word0, root.word1);
            if !seen.insert(key) {
                continue;
            }
            if let Some(generation) = by_key.get(&key) {
                out.roots.push(RegenRoot {
                    refno: generation.root.to_pdms_str(),
                    noun: generation.noun.clone(),
                });
            }
        }
    }
    out
}

/// 只带名单的 `ModelOnly`：目标会话 = 设计库文件最新，`model_start_sesno` 也取它（规划器不开窗，进工作单的只有
/// 名单与账本里还欠着的根）。与目录级联排的任务同一形状，只是不带库级目录落款。
pub(crate) fn model_only_batch(
    project: String,
    version: &SourceVersion,
    regen_roots: Vec<RegenRoot>,
) -> DiscoveredBatch {
    let latest = version.sesno as i32;
    DiscoveredBatch {
        project,
        dbnum: version.dbnum,
        db_type: version.db_type.clone(),
        phase: crate::data_interface::initialization_phase::DataPhase::Design,
        epoch_id: 0,
        intent: crate::data_interface::batch_queue::BatchIntent::ModelOnly,
        model_start_sesno: Some(latest),
        file_name: version
            .file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: version.file.clone(),
        applied_sesno: latest,
        file_latest_sesno: latest,
        previous_observed_sesno: latest,
        first_pending_sesno_time: None,
        file_latest_sesno_time: version.session_time.clone(),
        reinit_requested: false,
        regen_roots,
        catalogue_stamps: BTreeMap::new(),
    }
}

/// I/O：在 `dbnum` 的 `DbSet@target_sesno` 上把元素折成根——`build_set` + `scan_index` + 根枚举，与
/// [`crate::data_interface::window_root_plan::enumerate_target_roots`] 同一段。阻塞部分进 `spawn_blocking`。
pub(crate) async fn fold_at_target(
    dbnum: u32,
    target_sesno: u32,
    elements: Vec<RefNo>,
) -> anyhow::Result<FoldedRoots> {
    let service = crate::fast_model::e3d_model_service::E3dModelService::from_current().await?;
    let unit_types = crate::data_interface::generation_root::configured_delivery_unit_types();
    tokio::task::spawn_blocking(move || {
        let file = service.source_file(dbnum)?.to_path_buf();
        let target = service.build_set(dbnum, Some(target_sesno))?;
        let index = crate::fast_model::e3d_model_service::scan_index(&file, Some(target_sesno))?;
        let roots = crate::data_interface::generation_root::enumerate_generation_roots(
            &target,
            &index.roots,
            &unit_types,
        )?;
        anyhow::Ok(fold_elements_to_roots(&index.owners, &roots, &elements))
    })
    .await
    .map_err(|error| anyhow::anyhow!("cross-db root fold for dbnum {dbnum} panicked: {error}"))?
}

/// 执行体在一窗**落地之后**调（预览不调）：逐个对端库折根、排一条只带名单的 `ModelOnly`，每库一行回执。
/// `elements` 是计划上的 [`crate::data_interface::model_update_plan::ModelUpdatePlan::cross_db_elements`]；
/// 空 = 什么都不做、不出声。
pub(crate) async fn hand_off(
    source_dbnum: u32,
    elements: &BTreeMap<u32, Vec<String>>,
) -> Vec<String> {
    if elements.is_empty() {
        return Vec::new();
    }
    if !crate::options::model_incremental() {
        return vec![format!(
            "dbnum={source_dbnum}: cross-db handoff skipped — model_incremental=false; {} element(s) \
             in dbnum {:?} keep their old geometry",
            elements.values().map(Vec::len).sum::<usize>(),
            elements.keys().collect::<Vec<_>>()
        )];
    }
    let mut lines = Vec::with_capacity(elements.len());
    for (&dbnum, refnos) in elements {
        lines.push(hand_off_one(source_dbnum, dbnum, refnos).await);
    }
    lines
}

async fn hand_off_one(source_dbnum: u32, dbnum: u32, refnos: &[String]) -> String {
    let prefix = format!(
        "dbnum={source_dbnum}: cross-db handoff → dbnum={dbnum} ({} element(s): {})",
        refnos.len(),
        named(refnos)
    );
    if !crate::data_interface::watch_scope::admits(dbnum) {
        return format!(
            "{prefix}: not handed off — {}",
            crate::data_interface::watch_scope::excluded_reason(dbnum)
        );
    }
    let elements: Vec<RefNo> = refnos.iter().filter_map(|text| parse_refno(text)).collect();
    let version = match tokio::task::spawn_blocking(move || {
        crate::data_interface::model_source::latest_source_version(dbnum)
    })
    .await
    {
        Ok(Ok(version)) => version,
        Ok(Err(error)) => {
            return format!("{prefix}: not handed off — no readable latest session ({error:#})");
        }
        Err(error) => return format!("{prefix}: not handed off — source read panicked ({error})"),
    };
    if !crate::data_interface::model_source::is_element_db(&version.db_type) {
        return format!(
            "{prefix}: not handed off — dbnum {dbnum} is {}, not an element database",
            version.db_type
        );
    }
    let folded = match fold_at_target(dbnum, version.sesno, elements).await {
        Ok(folded) => folded,
        Err(error) => {
            return format!(
                "{prefix}: not handed off — roots at session {} could not be resolved ({error:#})",
                version.sesno
            );
        }
    };
    let unresolved = if folded.unresolved.is_empty() {
        String::new()
    } else {
        format!(
            "; {} element(s) sit under no generation root at session {}: {}",
            folded.unresolved.len(),
            version.sesno,
            named(&folded.unresolved)
        )
    };
    if folded.roots.is_empty() {
        return format!("{prefix}: nothing to hand off{unresolved}");
    }
    let root_names: Vec<String> = folded
        .roots
        .iter()
        .map(|root| format!("{} {}", root.refno, root.noun))
        .collect();
    let found = model_only_batch(
        aios_core::get_db_option().project_name.clone(),
        &version,
        folded.roots,
    );
    let outcome = crate::data_interface::batch_scheduler::BatchScheduler::global().enqueue(
        crate::data_interface::task_registry::TaskRegistry::global(),
        &found,
        false,
    );
    let line = format!(
        "{prefix}: {} root(s) [{}] queued as ModelOnly at session {} (outcome={:?}, task={}){unresolved}",
        found.regen_roots.len(),
        named(&root_names),
        version.sesno,
        outcome.outcome,
        outcome.info.task_id
    );
    println!("{line}");
    line
}

/// 逐条点名（封顶 [`NAMED_LIMIT`]），多出来的报个数。
fn named(items: &[String]) -> String {
    let shown: Vec<&str> = items.iter().take(NAMED_LIMIT).map(String::as_str).collect();
    let more = items.len().saturating_sub(NAMED_LIMIT);
    if more > 0 {
        format!("{} (+{more} more)", shown.join(", "))
    } else {
        shown.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_interface::generation_root::{GenerationRootKind, refno_from_e3d};

    /// 低 13 位放得下的库号在第一个 extent 上的 `word0`（`dbno = word0 & 0x1FFF | …`）。
    fn refno(dbnum: u32, id: u32) -> RefNo {
        RefNo::new(dbnum | (1 << 13), id)
    }

    fn root(refno: RefNo, noun: &str) -> GenerationRoot {
        GenerationRoot {
            root: refno_from_e3d(refno),
            noun: noun.into(),
            name: String::new(),
            kind: GenerationRootKind::Normal,
        }
    }

    /// 交接的输入按元素**所在的库**分组：去重、升序、无效 refno 丢掉——同一个对端被两条边命中只交一次。
    #[test]
    fn cross_db_elements_group_by_the_library_they_live_in() {
        let a = refno(7200, 30);
        let b = refno(7200, 12);
        let c = refno(7205, 4);
        let invalid = RefNo::new(0, 0);
        let grouped = elements_by_dbnum([a, c, b, a, invalid]);
        assert_eq!(
            grouped,
            BTreeMap::from([
                (7200, vec![b.to_string(), a.to_string()]),
                (7205, vec![c.to_string()]),
            ])
        );
        assert!(elements_by_dbnum(std::iter::empty()).is_empty());
        // 名单上的 `a/b` 拼回去还是原来那个 refno。
        assert_eq!(parse_refno(&a.to_string()), Some(a));
        assert_eq!(parse_refno(" 15392/30 "), Some(RefNo::new(15392, 30)));
        for bad in ["", "15392", "x/1", "15392/-1", "0/5"] {
            assert_eq!(parse_refno(bad), None, "{bad}");
        }
    }

    /// 折根与同库对端同一口径：owner 链上每一个落在根集里的根都算（嵌套根两层都进），同一个根只交一次，
    /// 链上一个根都没有的元素点名、不静默丢。
    ///
    /// 对端库：STRU 10 ─ FRMW 20 ─ GENSEC 30；STRU 10 ─ FRMW 21 ─ GENSEC 31；ZONE 1 ─ 孤儿 PANE 99（无根族）。
    #[test]
    fn far_end_elements_fold_to_every_root_on_their_own_libraries_owner_chain() {
        let r = |id| refno(7205, id);
        let owners = BTreeMap::from([
            ((r(30).word0, 30), r(20)),
            ((r(31).word0, 31), r(21)),
            ((r(20).word0, 20), r(10)),
            ((r(21).word0, 21), r(10)),
            ((r(10).word0, 10), r(1)),
            ((r(99).word0, 99), r(1)),
        ]);
        let roots_t = vec![
            root(r(10), "STRU"),
            root(r(20), "FRMW"),
            root(r(21), "FRMW"),
        ];
        let folded = fold_elements_to_roots(&owners, &roots_t, &[r(30), r(31), r(99), r(30)]);
        assert_eq!(
            folded
                .roots
                .iter()
                .map(|root| (root.refno.clone(), root.noun.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (refno_from_e3d(r(20)).to_pdms_str(), "FRMW"),
                (refno_from_e3d(r(10)).to_pdms_str(), "STRU"),
                (refno_from_e3d(r(21)).to_pdms_str(), "FRMW"),
            ],
            "自下而上、去重"
        );
        assert_eq!(folded.unresolved, vec![r(99).to_string()]);
        assert_eq!(
            fold_elements_to_roots(&owners, &roots_t, &[]),
            FoldedRoots::default()
        );
    }

    /// 排的任务与目录级联同一形状：`ModelOnly`、目标与下限都是文件最新（规划器不开窗）、名单随行、不带落款。
    #[test]
    fn the_handoff_task_is_a_model_only_batch_at_the_far_end_librarys_latest_session() {
        let version = SourceVersion {
            dbnum: 250110,
            db_type: "DESI".into(),
            file: std::path::PathBuf::from(r"D:\proj\aps000\aps250110_0001"),
            sesno: 30,
            session_time: Some("2026-09-30T10:00:00Z".into()),
        };
        let roots = vec![RegenRoot {
            refno: "2013286654/1800".into(),
            noun: "STRU".into(),
        }];
        let found = model_only_batch("APS".into(), &version, roots.clone());
        assert_eq!(
            found.intent,
            crate::data_interface::batch_queue::BatchIntent::ModelOnly
        );
        assert_eq!(found.dbnum, 250110);
        assert_eq!(found.model_start_sesno, Some(30));
        assert_eq!(
            (
                found.applied_sesno,
                found.file_latest_sesno,
                found.previous_observed_sesno
            ),
            (30, 30, 30)
        );
        assert_eq!(found.file_name, "aps250110_0001");
        assert_eq!(found.regen_roots, roots);
        assert!(found.catalogue_stamps.is_empty());
        assert!(!found.reinit_requested);
    }

    /// 交接只在执行体、只在落地之后：`manual_update` 的成功分支在 `collect_unit_tasks` 之后调一次，规划器
    /// （预览与执行共用）只把名单放进计划、自己不排工。
    #[test]
    fn the_handoff_runs_once_after_the_window_lands_and_never_from_the_planner() {
        let manual = include_str!("manual_update.rs").replace("\r\n", "\n");
        let production = manual.split_once("#[cfg(test)]").expect("tests follow").0;
        assert_eq!(
            production.matches("cross_db_handoff::hand_off(").count(),
            1,
            "执行体里恰好一处交接"
        );
        let units = production
            .find("units = collect_unit_tasks(")
            .expect("unit tasks");
        let hand_off = production
            .find("cross_db_handoff::hand_off(")
            .expect("handoff");
        assert!(units < hand_off, "落地成功、收完单元任务之后才交接");
        let success = production[..hand_off]
            .rfind("if let Some(success) = incr.successes.first()")
            .expect("成功分支");
        assert!(success < hand_off);

        let planner = include_str!("window_root_plan.rs").replace("\r\n", "\n");
        // 这个文件开头就有 `#[cfg(test)]` 的单行声明，生产半边按测试模块的头切。
        let planner = planner
            .split_once("#[cfg(test)]\nmod tests {")
            .expect("tests follow")
            .0;
        assert!(
            planner.contains("out.cross_db_elements = crate::data_interface::cross_db_handoff::elements_by_dbnum("),
            "规划器把跨库元素放进计划"
        );
        assert!(
            !planner.contains("cross_db_handoff::hand_off("),
            "规划器（预览共用）不排工"
        );
    }
}
