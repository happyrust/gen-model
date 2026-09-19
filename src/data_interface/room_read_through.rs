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

/// 目标里的一项对此刻的文件（纯判据，[`target_current`] 逐项调）：记了会话的项要求文件最新会话
/// 等于记录会话、且身份摘要相同；会话位 0 的项（字典库，[`ModelFileTarget::session_tracked`]）
/// 只比摘要——拿 0 去对文件最新会话永远不等，7bde2aeea 之后 `/model/ensure` 就是被这一刀判成
/// 每根每次都过期、生成完再判一次照样过期、抛 409 的。
fn target_file_current(file: &ModelFileTarget, latest_sesno: u32, digest_now: &str) -> bool {
    (!file.session_tracked() || latest_sesno == file.session) && digest_now == file.digest
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
    for file in std::iter::once(&target.design).chain(target.catalogue.iter()) {
        let path = Path::new(&file.file);
        let latest = super::model_source::latest_source_version_of_file(file.dbnum, &file.db_type, path)?;
        let digest = model_file_identity_digest(path, file.session)?;
        if !target_file_current(file, latest.sesno, &digest) {
            // 说清是哪一项、差在会话还是摘要：此前这一句被 `unwrap_or(false)` 与「正在后台生成」
            // 盖住，日志里没有一行说得出这根的模型为什么不算新。
            let reason = if file.session_tracked() && latest.sesno != file.session {
                format!("会话 {} → 文件最新 {}", file.session, latest.sesno)
            } else {
                format!("摘要 {} → 现算 {digest}", file.digest)
            };
            println!("[模型目标核对] {} {} 已过期：{reason}（{}）", file.db_type, file.dbnum, file.file);
            return Ok(false);
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
        Ok::<_, anyhow::Error>(selected.source.sesno == before.design.session
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

    /// 会话位 0 的项（字典库）只比摘要；记了会话的项会话、摘要都要对上。AMS 的六本字典库文件最新
    /// 会话 27/7/18/17/13/277，记的是 0——此前第一项就判假，没有一根能算 current。
    #[test]
    fn dictionary_targets_are_judged_by_digest_alone() {
        let file = |db_type: &str, session: u32, digest: &str| ModelFileTarget {
            dbnum: 5100, db_type: db_type.into(), file: "acp5100".into(), session, digest: digest.into(),
        };
        let dict = file("DICT", 0, "d@0");
        assert!(target_file_current(&dict, 27, "d@0"), "字典库文件最新会话 27 ≠ 记的 0 不算过期");
        assert!(!target_file_current(&dict, 27, "d@1"), "字典库换了文件（长度 / 修改时间）才算过期");
        let cata = file("CATA", 11, "c@11");
        assert!(target_file_current(&cata, 11, "c@11"));
        assert!(!target_file_current(&cata, 12, "c@11"), "目录库前移一版就过期");
        assert!(!target_file_current(&cata, 11, "c@11-replaced"), "同一会话换了文件也过期");
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
