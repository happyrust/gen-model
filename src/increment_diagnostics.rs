//! Net changes for one observed session window. Never bootstrap a missing base.
use crate::{
    data_interface::{
        db_generation::FileIdentity, db_generation::GenerationChange, ele_ops::EleOperationDetail,
        frozen_source::FrozenSource, mdb_route_directory::RouteRefresh,
    },
    process_diagnostics::event,
};

pub fn report(change: &RouteRefresh, operation: u64) -> anyhow::Result<()> {
    if change.kind != GenerationChange::SessionAdvanced
        || change.from.latest_sesno == 0
        || change.to.latest_sesno <= change.from.latest_sesno
    {
        event(
            operation,
            "increment.delta.unavailable",
            "回滚/文件替换或旧会话缺失：不使用单文件前进窗口伪造增删改；继续定向模型核对",
        );
        return Ok(());
    }
    let started = std::time::Instant::now();
    event(
        operation,
        "increment.delta.begin",
        &format!(
            "dbnum={} sesno={}→{} path={}；两端净变化，不是期间每次编辑记录",
            change.dbnum,
            change.from.latest_sesno,
            change.to.latest_sesno,
            change.path.display()
        ),
    );
    anyhow::ensure!(
        FileIdentity::of(&change.path)? == change.to.identity,
        "file changed after detection"
    );
    let frozen = FrozenSource::capture_at(&change.path, change.to.latest_sesno)?;
    let chain = frozen.session_chain()?;
    anyhow::ensure!(
        chain.iter().any(|s| s.sesno == change.from.latest_sesno),
        "old session missing; full baseline scan prohibited"
    );
    let first = i32::try_from(
        change
            .from
            .latest_sesno
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("session overflow"))?,
    )?;
    let last = i32::try_from(change.to.latest_sesno)?;
    let changes = crate::data_interface::e3d_collector::collect_net_window(
        &frozen,
        first..=last,
        &change.db_type,
    )?;
    frozen.verify_path_identity()?;
    anyhow::ensure!(
        FileIdentity::of(&change.path)? == change.to.identity,
        "file changed during delta diagnostics"
    );
    let mut counts = [0usize; 3];
    for entry in changes.window.values().flatten() {
        let (kind, index) = match &entry.detail {
            EleOperationDetail::Add(_) => ("新增", 0),
            EleOperationDetail::Modified(_) => ("修改", 1),
            EleOperationDetail::Deleted => ("删除", 2),
            EleOperationDetail::None => continue,
        };
        counts[index] += 1;
        event(
            operation,
            "increment.delta.item",
            &format!(
                "dbnum={} operation={} refno={} record_sesno={} window={}→{}",
                change.dbnum,
                kind,
                entry.refno,
                entry.sesno,
                change.from.latest_sesno,
                change.to.latest_sesno
            ),
        );
    }
    event(
        operation,
        "increment.delta.done",
        &format!(
            "dbnum={} 新增={} 修改={} 删除={} unchanged_rewrites={} unparseable={} elapsed_ms={} warnings={:?}",
            change.dbnum,
            counts[0],
            counts[1],
            counts[2],
            changes.unchanged_rewrites,
            changes.unparseable_finals,
            started.elapsed().as_millis(),
            changes.warnings
        ),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires an explicit native AMS fixture path; read-only historical window"]
    fn native_ams_delta_journal() {
        let path = std::path::PathBuf::from(std::env::var("AMS_DIAG_PATH").unwrap());
        let frozen = FrozenSource::capture(&path).unwrap();
        let mut chain = frozen.session_chain().unwrap();
        chain.sort_by_key(|session| std::cmp::Reverse(session.sesno));
        assert!(chain.len() >= 2);
        let identity = FileIdentity::of(&path).unwrap();
        let generation = |sesno, generation| crate::data_interface::db_generation::DbGeneration {
            identity,
            latest_sesno: sesno,
            generation,
            base_sesno: None,
        };
        let change = RouteRefresh {
            dbnum: frozen.dbnum() as u32,
            db_type: frozen.db_type().into(),
            path,
            from: generation(chain[1].sesno, 1),
            to: generation(chain[0].sesno, 2),
            kind: GenerationChange::SessionAdvanced,
        };
        report(&change, 900001).unwrap();
        assert_eq!(FileIdentity::of(&change.path).unwrap(), identity);
    }
}
