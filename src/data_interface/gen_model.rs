use std::sync::Arc;
use crate::data_interface::tidb_manager::AiosDBManager;

pub async fn gen_geos_data(mut mgr: Arc<AiosDBManager>) -> anyhow::Result<bool> {
    Ok(true)
}
