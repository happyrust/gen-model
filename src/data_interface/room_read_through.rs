//! Point room queries over the current projection, without stored room edges.
use std::{path::Path, sync::Arc};
use aios_core::RefnoEnum;
use anyhow::{Context, ensure};
use serde_json::{Value, json};
use crate::fast_model::{e3d_model_service::{ModelFileTarget, ModelTarget, model_file_identity_digest},
    model_memory_store::{ModelMemoryStore, ProjectionScope}, room_model::MemoryRoomCalculator,
    room_resident};
use super::direct_tree::DirectTreeService;

#[derive(Debug)]
pub(crate) struct RoomNotReady(pub &'static str);
impl std::fmt::Display for RoomNotReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.0) }
}
impl std::error::Error for RoomNotReady {}

fn require_stable(model: bool, source: bool, rooms: bool) -> anyhow::Result<()> {
    ensure!(model && source && rooms, RoomNotReady("room inputs changed; retry"));
    Ok(())
}

/// 回执里设计项 / 目录项的会话怎么对文件最新会话（ADR-089 D1 / D5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionRule {
    /// 回执会话 ≤ 文件最新即可：「落后」不是「过期」，中间的窗口有没有碰到这根由库级承诺回答
    /// （`model_coverage::root_credential_covers`，ADR-079 D6：水位盖到 T 就是「T 以前被碰到的根都重算过了」，
    /// 回执还在 S 的根正是没被碰到的那些）。调用方已经按那一半判过才该用它。
    NotBehind,
    /// 回执会话必须等于文件最新：没有库级承诺可听（读穿 / 只写内存形态下水位行不在或没盖到）时的退路，
    /// 即 2026-10-24 之前的判法。
    Exact,
}

/// 目标里的一项对此刻的文件（纯判据，[`target_current`] 逐项调）：身份摘要相同（冻结锚，
/// `frozen_source::model_file_identity`，ADR-089 D2），且记了会话的项按 [`SessionRule`] 对文件最新会话：
/// `NotBehind` 只拦「超前」（文件被回拨，ADR-054 实施约束 4 口径），`Exact` 连「落后」一起拦。
/// 会话位 0 的项（字典库，[`ModelFileTarget::session_tracked`]）只比身份——拿 0 去对文件最新会话永远
/// 不等，7bde2aeea 之后 `/model/ensure` 就是被这一刀判成每根每次都过期、生成完再判一次照样过期、抛 409 的。
///
/// 2026-10-24 之前这里对设计项也要等值，而窗口规划器的 `advance` 根什么都不做、回执留在 S（ADR-079 D3）：
/// 任何窗口落地后每个未触根都被判过期、按需显示整根重算；E3D 每次登录给 DESI 写一个只有 DBMDAT 的会话，
/// 让这件事天天发生（GitHub #65，夹具 8023 `db-20261024-211524-000008` 700→701 `advance=953`）。
fn target_file_current(
    file: &ModelFileTarget,
    latest_sesno: u32,
    identity_now: &str,
    rule: SessionRule,
) -> bool {
    let session_ok = !file.session_tracked()
        || match rule {
            SessionRule::NotBehind => file.session <= latest_sesno,
            SessionRule::Exact => file.session == latest_sesno,
        };
    session_ok && identity_now == file.digest
}

/// 没有生成根的元素的答复：字段与算得出的那一份同形，只是全部为空 / 未计算——前端按
/// `calculation_status` 分型，不必再多认一种形状。
fn no_generation_root_reply(member: RefnoEnum) -> Value {
    json!({"refno": member.to_string(), "memberships": [],
        "room_membership": true, "calculation_status": "not_computed", "source": "memory",
        "model_epoch": Value::Null, "source_sesno": Value::Null,
        "room_definition_version": Value::Null, "published_version": Value::Null,
        "stale": Value::Null, "publication_status": "unsupported"})
}

// Reuse the generator's identity recipe; inspect only its recorded dependencies.
pub(crate) fn target_current(target: &ModelTarget) -> anyhow::Result<bool> {
    target_current_under(target, SessionRule::NotBehind)
}

/// [`target_current`] 的带规则版：读穿 / 只写内存形态没有库级承诺可听时传 [`SessionRule::Exact`]（ADR-089 D5）。
pub(crate) fn target_current_under(target: &ModelTarget, rule: SessionRule) -> anyhow::Result<bool> {
    for file in std::iter::once(&target.design).chain(target.catalogue.iter()) {
        let path = Path::new(&file.file);
        let latest = super::model_source::latest_source_version_of_file(file.dbnum, &file.db_type, path)?;
        // 说清是哪一项、差在会话还是身份：此前这一句被 `unwrap_or(false)` 与「正在后台生成」
        // 盖住，日志里没有一行说得出这根的模型为什么不算新。
        let stale = |reason: String| {
            println!("[模型目标核对] {} {} 已过期：{reason}（{}）", file.db_type, file.dbnum, file.file);
            Ok(false)
        };
        if file.session_tracked() && file.session > latest.sesno {
            // 回拨：文件被换回更早的世代。锚在这个会话上算不出来，先说清再返回。
            return stale(format!("回执会话 {} 超前于文件最新 {}（文件被回拨）", file.session, latest.sesno));
        }
        let identity = crate::data_interface::frozen_source::model_file_identity(
            path,
            file.session_tracked().then_some(file.session),
        )?;
        if !target_file_current(file, latest.sesno, &identity, rule) {
            return stale(if file.session_tracked() && file.session != latest.sesno {
                format!("会话 {} → 文件最新 {}（没有库级承诺可听，按等值判）", file.session, latest.sesno)
            } else {
                format!("身份 {} → 现算 {identity}（换了文件或同号会话内容不同）", file.digest)
            });
        }
    }
    let attlib = super::direct_store::DirectSchema::template_dir_from_env()?.join("attlib.dat");
    let attlib_current = model_file_identity_digest(&attlib, 0)? == target.template_attlib_digest;
    if !attlib_current {
        println!("[模型目标核对] 模板 attlib.dat 已换版：记摘要 {}（{}）",
            target.template_attlib_digest, attlib.display());
    }
    Ok(attlib_current)
}

pub(crate) async fn lookup(member: RefnoEnum, tree: Arc<DirectTreeService>) -> anyhow::Result<Value> {
    ensure!(crate::options::room_membership() && crate::options::spatial_compute(),
        RoomNotReady("room calculation is disabled"));
    let resolve_tree = tree.clone();
    let Some(root) = tokio::task::spawn_blocking(move || resolve_tree.generation_root_of(member.refno(),
        &super::generation_root::configured_delivery_unit_types()))
        .await.context("room root task failed")??
    else {
        // 容器（WORL / SITE / ZONE …）与不在任何交付单元之下的元素没有生成根，也就没有房间归属可算：
        // 与摄入形态（`query_service::room` 对 `generation_root = None`）同一口径回 `not_computed`。
        // 此前抛 `RoomNotReady` → 409「updating」，前端会把一个永远不会变的容器当成「房间归属更新中」。
        return Ok(no_generation_root_reply(member));
    };
    let root = super::generation_root::refno_to_e3d(root.root);
    let native_member = super::generation_root::refno_to_e3d(member);
    let store = ModelMemoryStore::global();
    let scope = ProjectionScope::Current;
    let epoch = store.commit_epoch(root.dbno());
    let receipt = store.receipt(scope, root).ok_or(RoomNotReady("model is not ready"))?;
    let target = receipt.model_target.clone().ok_or(RoomNotReady("model has no source identity"))?;
    ensure!(target.design.dbnum == root.dbno() && target.design.session == receipt.source_sesno,
        RoomNotReady("model source identity is inconsistent"));
    let before = target.clone();
    let source_tree = tree.clone();
    ensure!(tokio::task::spawn_blocking(move || {
        let selected = source_tree.source_metadata(root.dbno())?;
        // 来源会话按「没落后」判（ADR-089 D6）：E3D 每次登录都给 DESI 前移一个只有 DBMDAT 的会话，按等值判
        // 登录之后每个根的房间点查都是「model source has changed」；身份变了 / 回拨由 `target_current` 拦。
        Ok::<_, anyhow::Error>(selected.source.sesno >= before.design.session
            && selected.source.file.canonicalize()? == Path::new(&before.design.file).canonicalize()?
            && target_current(&before)?)
    }).await??,
        RoomNotReady("model source has changed; regenerate model"));
    let _serial = crate::fast_model::spatial_state::lock_spatial_serial().await;
    let rooms = room_resident::current_for_calculation()
        .map_err(|_| RoomNotReady("room definitions are not ready"))?;
    let definition = rooms.input_version();
    let records = store.records_of_root(scope, root).into_iter()
        .filter(|record| record.source_refno == native_member).collect::<Vec<_>>();
    let model = rooms.clone();
    let mesh_dir = crate::options::meshes_dir();
    let version = definition.clone();
    let session = i32::try_from(receipt.source_sesno).context("room session overflow")?;
    let mut edges = tokio::task::spawn_blocking(move ||
        MemoryRoomCalculator::new(model, mesh_dir)?.calculate(root.dbno(), session, &version, &records))
        .await.context("room calculation task failed")??;
    let source_current = tokio::task::spawn_blocking(move ||
        Ok::<_, anyhow::Error>(tree.is_current() && target_current(&target)?)).await??;
    let after = room_resident::current_for_calculation()
        .map_err(|_| RoomNotReady("room definitions became unavailable"))?;
    require_stable(store.commit_epoch(root.dbno()) == epoch && store.receipt(scope, root).as_ref() == Some(&receipt),
        source_current, after.input_version() == definition)?;
    edges.sort_by(|a, b| b.member.inside_count.cmp(&a.member.inside_count)
        .then_with(|| a.member.center_dist.total_cmp(&b.member.center_dist))
        .then_with(|| a.room_num.cmp(&b.room_num)));
    let memberships = edges.iter().map(|edge| json!({
        "room_refno": rooms.map.rooms.iter().find(|room| room.panels.contains(&edge.panel)).map(|room| room.room.to_string()),
        "panel_refno": edge.panel.to_string(), "room_num": edge.room_num,
        "inside_count": edge.member.inside_count, "center_dist": edge.member.center_dist,
    })).collect::<Vec<_>>();
    Ok(json!({"refno": member.to_string(), "memberships": memberships,
        "room_membership": true, "calculation_status": "computed", "source": "memory",
        "model_epoch": epoch, "source_sesno": receipt.source_sesno,
        "room_definition_version": definition, "published_version": null,
        "stale": null, "publication_status": "unsupported"}))
}

#[cfg(test)]
mod tests {
    use super::*;
    /// `target_current` 逐根跑（`on_demand_model::memory_root_current`），只为取一个
    /// 模板目录就重载整套 schema 会把按需生成拖垮——`DbOption` 与 schema root 在一个
    /// 进程里不变，取缓存那一份。
    #[test]
    fn identity_checks_do_not_reload_the_schema_for_a_template_path() {
        let body = include_str!("room_read_through.rs")
            .split_once("pub(crate) fn target_current(")
            .expect("target_current exists")
            .1
            .split_once("pub(crate) async fn lookup(")
            .expect("target_current end")
            .0;
        assert!(body.contains("template_dir_from_env()"), "{body}");
        assert!(!body.contains("open_from_env()"), "{body}");
    }

    /// 会话位 0 的项（字典库）只比身份；记了会话的项身份要对上、会话按规则对。AMS 的六本字典库文件最新
    /// 会话 27/7/18/17/13/277，记的是 0——此前第一项就判假，没有一根能算 current。
    #[test]
    fn dictionary_targets_are_judged_by_identity_alone() {
        let file = |db_type: &str, session: u32, digest: &str| ModelFileTarget {
            dbnum: 5100, db_type: db_type.into(), file: "acp5100".into(), session, digest: digest.into(),
        };
        let dict = file("DICT", 0, "d@0");
        for rule in [SessionRule::NotBehind, SessionRule::Exact] {
            assert!(target_file_current(&dict, 27, "d@0", rule), "字典库文件最新会话 27 ≠ 记的 0 不算过期");
            assert!(!target_file_current(&dict, 27, "d@1", rule), "字典库换了文件 / 走了新会话才算过期");
        }
    }

    /// ADR-089 D1 / D5 的真值表：同锚、落后（回执 11 < 文件 12）在 `NotBehind` 下不是过期——中间的窗口有没有碰到
    /// 这根由库级承诺回答；`Exact`（没有库级承诺可听）仍按 2026-10-24 之前的等值判。超前（回拨）、锚不同
    /// 在两条规则下都是过期。E3D 登录写的 DBMDAT 会话就是「同锚、落后」这一格（GitHub #65）。
    #[test]
    fn a_receipt_behind_the_file_is_stale_only_without_a_library_promise() {
        let design = ModelFileTarget {
            dbnum: 8000, db_type: "DESI".into(), file: "ams8000_0001".into(), session: 11, digest: "anchor@11".into(),
        };
        assert!(target_file_current(&design, 11, "anchor@11", SessionRule::NotBehind));
        assert!(target_file_current(&design, 11, "anchor@11", SessionRule::Exact));
        assert!(target_file_current(&design, 12, "anchor@11", SessionRule::NotBehind), "落后不是过期（登录写的会话）");
        assert!(!target_file_current(&design, 12, "anchor@11", SessionRule::Exact), "没有库级承诺时落后就是过期");
        for rule in [SessionRule::NotBehind, SessionRule::Exact] {
            assert!(!target_file_current(&design, 10, "anchor@11", rule), "回执超前于文件（回拨）是过期");
            assert!(!target_file_current(&design, 11, "anchor@11-replaced", rule), "同号会话换了文件是过期");
            assert!(!target_file_current(&design, 12, "anchor@11-replaced", rule), "落后且换了文件是过期");
        }
    }

    /// 源码钉（ADR-089 实施约束 4）：库文件的身份不再按 mtime 算——`target_current` 里只有模板项还调
    /// `model_file_identity_digest`；判据里没有「== latest」的绝对写法，等值只在 `SessionRule::Exact` 那一臂。
    #[test]
    fn library_file_identity_is_the_frozen_anchor_not_mtime() {
        let source = include_str!("room_read_through.rs");
        let current = source
            .split_once("pub(crate) fn target_current_under(")
            .expect("target_current_under exists")
            .1
            .split_once("pub(crate) async fn lookup(")
            .expect("target_current_under end")
            .0;
        assert_eq!(current.matches("model_file_identity_digest(").count(), 1, "只有 attlib 那一处: {current}");
        assert!(current.contains("model_file_identity_digest(&attlib, 0)"), "{current}");
        assert!(current.contains("frozen_source::model_file_identity("), "{current}");
        let predicate = source
            .split_once("fn target_file_current(")
            .expect("target_file_current exists")
            .1
            .split_once("pub(crate) fn target_current(")
            .expect("target_file_current end")
            .0;
        assert!(!predicate.contains("latest_sesno == file.session"), "{predicate}");
        assert!(predicate.contains("SessionRule::NotBehind => file.session <= latest_sesno"), "{predicate}");
    }

    /// 容器 / 无生成根的元素回 `not_computed`（与摄入形态同口径），不再是 `RoomNotReady` → 409。
    #[test]
    fn an_element_without_a_generation_root_is_not_computed_rather_than_updating() {
        let reply = no_generation_root_reply(RefnoEnum::from("24381/147826"));
        assert_eq!(reply["refno"], "24381_147826");
        assert_eq!(reply["calculation_status"], "not_computed");
        assert_eq!(reply["memberships"], serde_json::json!([]));
        assert_eq!(reply["room_membership"], true);
        assert!(reply["model_epoch"].is_null() && reply["source_sesno"].is_null());
        let body = include_str!("room_read_through.rs")
            .split_once("pub(crate) async fn lookup(")
            .expect("lookup exists")
            .1
            .split_once("#[cfg(test)]")
            .expect("lookup end")
            .0;
        assert!(!body.contains("element has no owning generation root"), "{body}");
        assert!(body.contains("no_generation_root_reply(member)"), "{body}");
    }

    #[test]
    fn changes_to_any_room_input_reject_the_result() {
        for model in [false, true] { for source in [false, true] { for rooms in [false, true] {
            assert_eq!(require_stable(model, source, rooms).is_ok(), model && source && rooms);
        }}}
    }
}
