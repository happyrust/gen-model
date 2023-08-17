
use crate::{
    data_interface::interface::PdmsDataInterface, test::test_helper::get_test_ams_db_manager_async,
};
use aios_core::pdms_types::RefU64;

///获得branch下的所有托臂
#[tokio::test]
async fn test_query_support_arms() -> anyhow::Result<()> {
    let gensec_refno: RefU64 = "24384/25797".into();
    let mgr = get_test_ams_db_manager_async().await;
    let tmp = mgr.query_foreign_refnos(&[gensec_refno], &[&["SPRE", "CATR"]],
                                        &["PSTR", "PTSS"], &[], 4).await?;
    dbg!(tmp);
    assert_eq!(tmp.pop().unwrap().to_refno_str(), "21438/2368");

    Ok(())
}
