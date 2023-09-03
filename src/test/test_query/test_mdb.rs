
// get_world

use aios_core::pdms_types::RefU64;
use crate::data_interface::interface::PdmsDataInterface;
use crate::test::test_helper::get_test_ams_db_manager_async;

#[tokio::test]
async fn test_get_world() -> anyhow::Result<()> {
    let mgr = get_test_ams_db_manager_async().await;

    let world = mgr.get_world("", "ALL", "DESI").await?;
    dbg!(world);

    Ok(())
}