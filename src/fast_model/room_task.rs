//! Per-dbnum room completion from the immutable in-memory model projection.
//!
//! Calculation inputs come only from [`ModelMemoryStore`] and the resident room
//! reference built from frozen E3D files. SurrealDB is only **written** here
//! (obligation, result directory, room edges); the gate and the bookkeeping the
//! room stage needs are in memory (ADR-072 D4 / ADR-071 D5).

use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex, PoisonError};

use aios_core::SUL_DB;
use anyhow::{Context, ensure};

use crate::data_interface::batch_worker::DataBatchTaskResult;
use crate::data_interface::dbnum_state::{DbnumState, RoomInputVersion};
use crate::data_interface::frozen_source::FrozenSource;
use crate::data_interface::manual_update::{ManualUpdateStatus, UnitGenStatus};
use crate::data_interface::room_completion::PreparedRoomResult;
use crate::data_interface::task_end::TaskModelOutcome;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::model_memory_store::ModelMemoryStore;
use crate::fast_model::room_model::MemoryRoomCalculator;

const ROOM_RULE_VERSION: &str = "memory-room-calculator-v1";

fn room_input_version(
    source_token: &str,
    model_sesno: i32,
    model_version: String,
    room_definition_version: String,
) -> RoomInputVersion {
    RoomInputVersion {
        version: 1,
        source_identity: crate::data_interface::digest::digest(source_token.as_bytes()),
        model_sesno,
        model_version,
        room_definition_version,
        generation_rule_version: ROOM_RULE_VERSION.into(),
    }
}

// ---------------------------------------------------------------------------
// 房间结果登记（进程内镜像的一格，ADR-071 D5）
// ---------------------------------------------------------------------------

/// 房间段要的四格**存量**事实：上一版结果目录的版本号（`room-directory:{dbnum}:{version}`
/// 磁盘工件的键）、上一版的输入 / 义务、房间水位。
///
/// 它们此前从 `dbnum_watermark` 那一行现读（水位行的按需读），而房间排在收尾盖章
/// **之前**算——那一刻库里的行还停在上一版，拿它当门永远过不去，拿它当存量倒是对的。
/// ADR-072 D4 把门换成任务手里的 [`TaskModelOutcome`]，存量则收进这格登记：启动从
/// `dbnum_watermark` 水化一次（计划 §4.5 白名单里的那一格），之后每次房间段提交成功就地
/// 更新，房间段自己**不再回读**水位行。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RoomResultFacts {
    pub room_sesno: i32,
    pub room_input_version: Option<RoomInputVersion>,
    pub room_required_input: Option<RoomInputVersion>,
    pub room_result_version: Option<String>,
}

impl RoomResultFacts {
    pub(crate) fn from_state(state: &DbnumState) -> Self {
        Self {
            room_sesno: state.room_sesno,
            room_input_version: state.room_input_version.clone(),
            room_required_input: state.room_required_input.clone(),
            room_result_version: state.room_result_version.clone(),
        }
    }

    /// 这一版输入是否已经算完写完——与 `DbnumState::recovery_stage` 里房间那一半同判：
    /// 房间水位到了、输入与义务都是这一版、结果目录在。任务内这里恒为假（房间水位还停在
    /// 上一版），它只替启动补房间那条路省掉重算。
    pub(crate) fn completed_for(&self, input: &RoomInputVersion) -> bool {
        self.room_sesno >= input.model_sesno
            && self.room_input_version.as_ref() == Some(input)
            && self.room_required_input.as_ref() == Some(input)
            && self
                .room_result_version
                .as_ref()
                .is_some_and(|version| !version.is_empty())
    }
}

type RoomResultRegistry = Mutex<Option<BTreeMap<u32, RoomResultFacts>>>;

/// `None` = 还没水化过；`Some` 之后缺格 = 这个库从没算过房间（`Default`）。
static ROOM_RESULTS: LazyLock<RoomResultRegistry> = LazyLock::new(|| Mutex::new(None));

fn registry() -> std::sync::MutexGuard<'static, Option<BTreeMap<u32, RoomResultFacts>>> {
    ROOM_RESULTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// 房间段提交成功后就地更新这一格（`PreparedDelta::commit_on` 落的正是这四样）。
pub(crate) fn remember_room_result(dbnum: u32, facts: RoomResultFacts) {
    registry()
        .get_or_insert_with(BTreeMap::new)
        .insert(dbnum, facts);
}

/// 库被回退到水位（ADR-072 D2）之后这一格作废：下一次房间段按「从没算过」整库重算。
pub(crate) fn forget_room_result(dbnum: u32) {
    if let Some(entries) = registry().as_mut() {
        entries.remove(&dbnum);
    }
}

/// 用一批登记行整份替换登记（启动水化 / 测试用）。
pub(crate) fn hydrate_room_results<'a>(states: impl IntoIterator<Item = &'a DbnumState>) {
    let entries = states
        .into_iter()
        .map(|state| (state.dbnum, RoomResultFacts::from_state(state)))
        .collect();
    *registry() = Some(entries);
}

/// 这个库的存量事实。登记还没水化时从 `dbnum_watermark` 整表水化**一次**（ADR-071 D7 /
/// 计划 §4.5 白名单），之后只读内存；登记里没有这一格 = 从没算过房间。
pub(crate) async fn room_result_facts(dbnum: u32) -> anyhow::Result<RoomResultFacts> {
    if let Some(entries) = registry().as_ref() {
        return Ok(entries.get(&dbnum).cloned().unwrap_or_default());
    }
    let states = DbnumState::list_registered()
        .await
        .context("hydrate the room result registry from dbnum_watermark")?;
    let mut guard = registry();
    let entries = guard.get_or_insert_with(BTreeMap::new);
    for state in &states {
        entries
            .entry(state.dbnum)
            .or_insert_with(|| RoomResultFacts::from_state(state));
    }
    Ok(entries.get(&dbnum).cloned().unwrap_or_default())
}

/// Compute and commit one complete dbnum room image. Calculation never has a
/// database handle and never consults `room_relate`, `pe`, or the global tree.
///
/// 门读的是**本任务手里的模型结果**（[`TaskModelOutcome::all_roots_ok`]，ADR-072 D4）。
/// 房间排在收尾盖章**之前**算，那一刻库里的 `model_sesno` 还停在上一版——回读水位当门
/// 就永远过不去，这正是这道门原先读本任务刚写的水位的毛病。
///
/// `facts` 是这个库的存量事实（[`RoomResultFacts`]，进程内登记），只拿它取上一版结果目录
/// 与「这一版是否已算完」，不用它判「本任务的模型段成没成」。
async fn calculate_and_commit_memory_rooms(
    mgr: &AiosDBManager,
    dbnum: u32,
    model_sesno: i32,
    source_token: &str,
    model: &TaskModelOutcome,
    facts: &RoomResultFacts,
) -> anyhow::Result<usize> {
    let _serial = super::spatial_state::lock_spatial_serial().await;
    let resident = super::room_resident::current_for_calculation()?;
    ensure!(
        resident.missing_panels.is_empty(),
        "resident room input is incomplete: {} panels have no geometry",
        resident.missing_panels.len()
    );

    let store = ModelMemoryStore::global();
    let cache = mgr.db_option.get_meshes_path().join("room-result-cache");
    let previous: crate::data_interface::room_delta::Directory = facts.room_result_version.as_ref()
        .and_then(|version| super::model_artifact::load_value(&cache, &format!("room-directory:{dbnum}:{version}")))
        .unwrap_or_default();
    let definition = resident.input_version();
    let full = !store.room_baseline_known(dbnum)
        || previous.version != facts.room_result_version || previous.version.is_none()
        || previous.input.as_ref().is_none_or(|input| input.room_definition_version != definition);
    let (epoch, model_version, changes, records) = store.room_snapshot(dbnum, full);
    let mut changed: std::collections::BTreeSet<String> = changes.keys()
        .map(|member| aios_core::RefU64(*member as u64).to_string()).collect();
    if full { changed.extend(previous.members.keys().cloned()); }
    let input = room_input_version(
        source_token,
        model_sesno,
        model_version,
        definition,
    );
    // 本任务的模型段把这一版的根全落进去了才算房间。事实来自任务手里的单元清单
    // （`TaskModelOutcome`），不回读自己刚写的水位。
    ensure!(
        model.all_roots_ok,
        "dbnum {dbnum} model stage did not land every root at {model_sesno}; rooms keep the old version"
    );
    if facts.completed_for(&input) {
        store.ack_rooms(dbnum, epoch);
        return Ok(0);
    }

    PreparedRoomResult::require_on(&SUL_DB, dbnum, &input).await?;

    let mesh_dir = mgr.db_option.get_meshes_path();
    let calculate_input = input.clone();
    let edges = tokio::task::spawn_blocking(move || {
        MemoryRoomCalculator::new(resident, mesh_dir)?.calculate(dbnum, &calculate_input, &records)
    })
    .await
    .context("join memory room calculation")??;
    ensure!(
        store.commit_epoch(dbnum) == epoch,
        "dbnum {dbnum} memory model changed during room calculation"
    );

    ensure!(super::room_resident::current_for_calculation()?.input_version() == input.room_definition_version,
        "room definition changed during calculation");
    let prepared = crate::data_interface::room_delta::PreparedDelta::new(
        dbnum, previous, input.clone(), &changed, &edges)?;
    // Save the candidate before switching visibility. A crash after the database
    // ACK can reload this exact content-addressed directory without recomputation.
    let next_version = prepared.next.version.clone().expect("prepared delta carries its version");
    super::model_artifact::store_value(&cache,
        &format!("room-directory:{dbnum}:{next_version}"), &prepared.next)?;
    prepared.stage_on(&SUL_DB).await?;
    ensure!(
        store.commit_epoch(dbnum) == epoch,
        "dbnum {dbnum} memory model changed while staging room result"
    );
    prepared.commit_on(&SUL_DB).await?;
    store.ack_rooms(dbnum, epoch);
    remember_room_result(
        dbnum,
        RoomResultFacts {
            room_sesno: model_sesno,
            room_input_version: Some(input.clone()),
            room_required_input: Some(input),
            room_result_version: Some(next_version),
        },
    );
    println!("[room-delta] dbnum={dbnum} full={full} members={} edge_writes={} directory_members_visited={}",
        changed.len(), prepared.edge_writes(), prepared.directory_members_visited());
    Ok(edges.len())
}

/// 这一版的源身份直接从**文件**冻结。发布轮次那一行随 publish 阶段一起退役，不再是取令牌的路。
/// `path` 为空时按 dbnum 解析当前 MDB 的源文件。
pub(crate) fn frozen_source_for(dbnum: u32, model_sesno: i32, path: &str) -> anyhow::Result<FrozenSource> {
    ensure!(
        model_sesno >= 0,
        "negative model watermark for dbnum {dbnum}"
    );
    let selected = if path.trim().is_empty() {
        crate::data_interface::model_source::source_file_of(dbnum)?.1
    } else {
        std::path::PathBuf::from(path)
    };
    FrozenSource::capture_at(selected, model_sesno as u32)
}

fn source_token_from_file(dbnum: u32, model_sesno: i32, path: &str) -> anyhow::Result<String> {
    frozen_source_for(dbnum, model_sesno, path)?.commit_token_json()
}

/// 一个 dbnum 任务的房间段：算出这一版的归属并写边。
///
/// 输入全在内存——常驻房间模型、[`ModelMemoryStore`] 的投影、任务手里的
/// [`TaskModelOutcome`] 与 [`FrozenSource`]、进程内的房间结果登记。这一段里库只被写，
/// 不被读来当判据（ADR-072 D4）。房间水位不在这里盖，由任务收尾那一笔 `task_end` 统一盖
/// （ADR-071 D3 / ADR-065 G2 (i)）。
///
/// `Ok(true)` = 这一版房间算完写完；`Ok(false)` = 房间没成（原因已记进 `result`
/// 的警告），调用方据此**不盖** `room_sesno`，模型那一枚照盖。
pub(crate) async fn finish_task_rooms(
    mgr: &AiosDBManager,
    dbnum: u32,
    target_sesno: i32,
    model: &TaskModelOutcome,
    source: &FrozenSource,
    result: &mut DataBatchTaskResult,
) -> anyhow::Result<bool> {
    // 清边名单从**任务内存**里拿（ADR-071 D1 / ADR-067 D7）：它就是本轮生成时算出来的
    // 「产物 → 目标」。发布轮次那一行的兜底随 publish 阶段一起退了。
    if let Some(effects) = crate::data_interface::task_effects::snapshot(dbnum, target_sesno) {
        let room_clear = effects.room_clear.into_iter().collect::<Vec<_>>();
        super::room_publication::clear_deferred_room_edges(&room_clear).await?;
    }
    if !crate::options::room_incremental() {
        return Ok(true);
    }

    let source_token = source.commit_token_json()?;
    let facts = room_result_facts(dbnum).await?;
    match calculate_and_commit_memory_rooms(mgr, dbnum, target_sesno, &source_token, model, &facts)
        .await
    {
        Ok(done) => {
            crate::data_interface::task_effects::forget(dbnum, target_sesno);
            println!(
                "房间段完成 dbnum={dbnum} sesno={target_sesno}：实时内存房间归属 {done} 条，房间水位待收尾那一笔盖"
            );
            Ok(true)
        }
        Err(error) => {
            result.status = ManualUpdateStatus::Partial;
            result.warnings.push(format!(
                "房间阶段未完成，模型那一枚照盖、房间那一枚不盖 dbnum={dbnum} sesno={target_sesno}: {error:#}"
            ));
            Ok(false)
        }
    }
}

/// Repair only missing room stages on restart. Scope is the initialized DESI
/// dbnums selected by the watcher; cold dbnums remain available for manual init.
///
/// 这里读的 `dbnum_watermark` 整表是启动水化那一次（计划 §4.5 白名单）：读回来的行同时
/// 灌进房间结果登记，之后任务的房间段只看登记。
pub(crate) async fn recover_missing_room_watermarks(mgr: &AiosDBManager) -> anyhow::Result<usize> {
    let current_project = mgr.db_option.project_name.as_str();
    let states = DbnumState::list_registered().await?;
    hydrate_room_results(states.iter());
    let mut completed = 0usize;
    for state in states {
        if !state.db_type.eq_ignore_ascii_case("DESI")
            || !state.initialized
            || state.model_sesno <= 0
            || state.model_sesno != state.applied_sesno
            || (!state.owner_project.is_empty() && state.owner_project != current_project)
            || !crate::data_interface::watch_scope::admits(state.dbnum)
        {
            continue;
        }
        let token = source_token_from_file(state.dbnum, state.model_sesno, &state.file_path)
            .with_context(|| format!("restore room source proof for dbnum {}", state.dbnum))?;
        // 这条路的入选条件里已经有 `model_sesno == applied_sesno`（上面那道筛），
        // 即模型段早已追平——这一版的根按定义全落进去了。
        let model = TaskModelOutcome {
            all_roots_ok: true,
            dead_roots: 0,
        };
        let facts = RoomResultFacts::from_state(&state);
        let done = calculate_and_commit_memory_rooms(
            mgr,
            state.dbnum,
            state.model_sesno,
            &token,
            &model,
            &facts,
        )
        .await
        .with_context(|| format!("restore room watermark for dbnum {}", state.dbnum))?;
        // 房间水位不再跟着结果一起提交（见 `PreparedRoomResult::commit_sql`）：这条恢复路
        // 自己盖，目标就是这个库现在的模型水位。只在**确实落后**时盖——否则每次启动都会给
        // 每个已完成的库白涨一次 `state_revision`。
        if state.room_sesno < state.model_sesno || state.room_result_version.is_none() {
            crate::data_interface::task_end::commit_task_end_on(
                &SUL_DB,
                &crate::data_interface::task_end::TaskEnd {
                    dbnum: state.dbnum,
                    target_sesno: state.model_sesno,
                    target_time: state.model_sesno_time.clone(),
                    model: None,
                    rooms: Some(crate::data_interface::task_end::TaskRoomOutcome { ok: true }),
                    info: None,
                    // 这是启动补房间水位，不是一个任务的收尾：它没有开过
                    // `dbnum_write_state` 的 `writing`，也就不该由它写 `finished`。
                    closes_task: false,
                },
            )
            .await
            .with_context(|| format!("stamp restored room watermark for dbnum {}", state.dbnum))?;
        }
        if done > 0 || state.room_sesno != state.model_sesno || state.room_result_version.is_none()
        {
            completed += 1;
            println!(
                "房间阶段恢复完成 dbnum={} room_sesno={}，实时内存归属 {} 条",
                state.dbnum, state.model_sesno, done
            );
        }
    }
    Ok(completed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 房间段的门读**任务手里的模型结果**（ADR-072 D4），不回读本任务刚写的水位；
    /// 计算段照旧不碰库，publish 阶段的簿记不再是房间段的入口条件。
    #[test]
    fn task_room_calculation_gates_on_the_task_outcome_and_never_queries_the_database() {
        let source = include_str!("room_task.rs");
        let production = source.split_once("#[cfg(test)]").expect("test boundary").0;
        let calculation = source
            .split_once("async fn calculate_and_commit_memory_rooms(")
            .expect("memory calculation")
            .1
            .split_once("pub(crate) fn frozen_source_for(")
            .expect("calculation boundary")
            .0;
        assert!(calculation.contains("ModelMemoryStore::global()"));
        assert!(calculation.contains("MemoryRoomCalculator::new"));
        assert!(!calculation.contains("SUL_DB.query"));
        assert!(!production.contains("startup_rooms_pending"));
        assert!(!production.contains("drain_rooms_scoped"));
        assert!(calculation.contains("store.room_snapshot(dbnum, full)"));
        assert!(!calculation.contains("snapshot_dbnum"));
        assert!(calculation.contains("room_delta::PreparedDelta::new"));

        // 门 = 任务结果，不是水位；存量 = 进程内登记，不是水位行。
        assert!(calculation.contains("model.all_roots_ok"));
        assert!(calculation.contains("facts.completed_for(&input)"));
        assert!(!production.contains(concat!("DbnumState::", "read")));
        assert!(!calculation.contains("applied_sesno"));
        assert!(!calculation.contains("state.model_sesno"));

        // 房间段不再问发布轮次那一行，也不再替第二条异步账（房间待办行）销账。
        assert!(!production.contains("SideEffectCompensator::file_round("));
        assert!(!production.contains("published_model_file_round_on"));
        assert!(!production.contains(concat!("model_update_", "pending")));

        // 水位整表只在启动补房间那一次读（白名单的 `dbnum_watermark`），房间段本体不读。
        assert_eq!(
            production.matches("DbnumState::list_registered()").count(),
            2,
            "一次在登记的惰性水化、一次在启动补房间；再多就是房间段又回去读库了"
        );
    }

    fn input(model_sesno: i32, definition: &str) -> RoomInputVersion {
        room_input_version(
            "token",
            model_sesno,
            "model-v".to_string(),
            definition.to_string(),
        )
    }

    /// 登记里「这一版已算完」的判据与 `DbnumState::recovery_stage` 的房间那一半同判：
    /// 房间水位到了、输入与义务都是这一版、结果目录在——缺一样都算没算完。任务内房间水位
    /// 还停在上一版，所以任务内它恒为假。
    #[test]
    fn room_result_facts_are_complete_only_when_every_room_half_matches() {
        let current = input(12, "rooms-a");
        let complete = RoomResultFacts {
            room_sesno: 12,
            room_input_version: Some(current.clone()),
            room_required_input: Some(current.clone()),
            room_result_version: Some("delta2_x".into()),
        };
        assert!(complete.completed_for(&current));

        let behind = RoomResultFacts { room_sesno: 11, ..complete.clone() };
        assert!(!behind.completed_for(&current), "房间水位还在上一版 = 任务内的常态");
        let other_input = RoomResultFacts {
            room_input_version: Some(input(12, "rooms-b")),
            ..complete.clone()
        };
        assert!(!other_input.completed_for(&current));
        let obligation_moved = RoomResultFacts {
            room_required_input: Some(input(13, "rooms-a")),
            ..complete.clone()
        };
        assert!(!obligation_moved.completed_for(&current));
        let no_directory = RoomResultFacts {
            room_result_version: Some(String::new()),
            ..complete.clone()
        };
        assert!(!no_directory.completed_for(&current));
        assert!(!RoomResultFacts::default().completed_for(&current));
    }

    /// 登记是进程内镜像的一格：水化整份替换、提交就地更新、回退作废；没水化过的库读
    /// 缺省（从没算过房间）。
    #[test]
    fn the_room_result_registry_is_hydrated_updated_and_invalidated_in_memory() {
        let mut a = DbnumState::default();
        a.dbnum = 9_000_001;
        a.room_sesno = 3;
        a.room_result_version = Some("delta2_a".into());
        let mut b = DbnumState::default();
        b.dbnum = 9_000_002;
        hydrate_room_results([&a, &b]);
        {
            let guard = registry();
            let entries = guard.as_ref().expect("hydrated");
            assert_eq!(entries[&9_000_001], RoomResultFacts::from_state(&a));
            assert_eq!(entries[&9_000_002], RoomResultFacts::default());
        }

        let committed = RoomResultFacts {
            room_sesno: 4,
            room_input_version: Some(input(4, "rooms-a")),
            room_required_input: Some(input(4, "rooms-a")),
            room_result_version: Some("delta2_b".into()),
        };
        remember_room_result(9_000_001, committed.clone());
        assert_eq!(
            registry().as_ref().unwrap()[&9_000_001],
            committed,
            "提交后登记就是刚落进库的那四样"
        );

        forget_room_result(9_000_001);
        assert!(
            !registry().as_ref().unwrap().contains_key(&9_000_001),
            "回退到水位之后这一格作废"
        );
        // 其它库的格不受影响；整份重水化后旧格消失。
        assert!(registry().as_ref().unwrap().contains_key(&9_000_002));
        hydrate_room_results([&a]);
        assert!(!registry().as_ref().unwrap().contains_key(&9_000_002));
    }
}
