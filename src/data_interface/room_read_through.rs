//! Point room queries over the current projection, without stored room edges.
use std::{path::Path, sync::Arc};
use aios_core::RefnoEnum;
use anyhow::{Context, ensure};
use serde_json::{Value, json};
use crate::fast_model::{e3d_model_service::{ModelTarget, model_file_identity_digest},
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

// Reuse the generator's identity recipe; inspect only its recorded dependencies.
pub(crate) fn target_current(target: &ModelTarget) -> anyhow::Result<bool> {
    for file in std::iter::once(&target.design).chain(target.catalogue.iter()) {
        let path = Path::new(&file.file);
        let latest = super::model_source::latest_source_version_of_file(file.dbnum, &file.db_type, path)?;
        if latest.sesno != file.session
            || model_file_identity_digest(path, file.session)? != file.digest {
            return Ok(false);
        }
    }
    let schema = super::direct_store::DirectSchema::open_from_env()?;
    Ok(model_file_identity_digest(&schema.template_dir().join("attlib.dat"), 0)?
        == target.template_attlib_digest)
}

pub(crate) async fn lookup(member: RefnoEnum, tree: Arc<DirectTreeService>) -> anyhow::Result<Value> {
    ensure!(crate::options::room_membership() && crate::options::spatial_compute(),
        RoomNotReady("room calculation is disabled"));
    let resolve_tree = tree.clone();
    let root = tokio::task::spawn_blocking(move || resolve_tree.generation_root_of(member.refno(),
        &super::generation_root::configured_delivery_unit_types()))
        .await.context("room root task failed")??
        .ok_or(RoomNotReady("element has no owning generation root"))?.root;
    let root = super::generation_root::refno_to_e3d(root);
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
    let mesh_dir = aios_core::get_db_option().get_meshes_path();
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
    #[test]
    fn changes_to_any_room_input_reject_the_result() {
        for model in [false, true] { for source in [false, true] { for rooms in [false, true] {
            assert_eq!(require_stable(model, source, rooms).is_ok(), model && source && rooms);
        }}}
    }
}
