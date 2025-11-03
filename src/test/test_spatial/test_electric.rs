//测试电气的查询

use std::str::FromStr;

use crate::{
    data_interface::interface::PdmsDataInterface, test::test_helper::get_test_ams_db_manager_async,
};
use aios_core::pdms_types::RefU64;
use crate::aql_api::pdms_room::IntersectMethod;

///获得branch下的所有托臂
#[tokio::test]
async fn test_query_support_arms() -> anyhow::Result<()> {
    let bran_refno: RefU64 = RefU64::from_str("24383/95023").unwrap();
    let mgr = get_test_ams_db_manager_async().await;
    let support_arms = mgr
        .query_children_around_eles_within_radius(bran_refno, true, None, true, &["SCTN"], &[], IntersectMethod::None)
        .await?;
    dbg!(&support_arms);
    let mut positions = vec![];
    for s in &support_arms {
        let trans = mgr
            .get_world_transform(s.refno).await
            .unwrap_or_default()
            .unwrap_or_default();
        positions.push(trans.translation);
    }
    dbg!(&positions);
    Ok(())
}
