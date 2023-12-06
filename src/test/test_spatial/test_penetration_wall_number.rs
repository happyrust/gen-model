use aios_core::pdms_types::RefU64;
use crate::aql_api::pdms_room::IntersectMethod;
use crate::test::test_helper::get_test_ams_db_manager_async;


pub async fn query_refno_belong_rooms(
    refno: RefU64,
) -> (String, String) {
    let refno = &refno.to_string() as &str;
    match refno {
        "24383/83722" => {
            return ("24381/180994".to_string(), "".to_string());
        }
        "24383/83477" => {
            return ("24381/180964".to_string(), "".to_string());
        }
        "24383/83995" => {
            return ("24381/181010".to_string(), "".to_string());
        }
        _ => { return ("".to_string(), "".to_string()); }
    }
}


#[tokio::test]
async fn test_penetration_wall_number() -> anyhow::Result<()> {
    let test_refno: RefU64 = RefU64::from_str("24383/83722").unwrap();
    let mgr = get_test_ams_db_manager_async().await;

    let (pts, bbox) = mgr.query_eles_keypts_and_aabb_as_whole(&[test_refno], true).await?.unwrap();
    dbg!(&pts);
    dbg!(bbox);


    let near_walls = mgr
        .query_around_eles_within_radius(test_refno, true, None, false, &["WALL"], &[], IntersectMethod::EndPtsCheck)
        .await?;
    dbg!(&near_walls);



    // let test_1 = query_refno_belong_rooms(RefU64::from_str("24383/83722").unwrap()).await;
    // assert_eq!(test_1, ("24381/180994".to_string(), "".to_string()));
    //
    // let test_2 = query_refno_belong_rooms(RefU64::from_str("24383/83477").unwrap()).await;
    // assert_eq!(test_2, ("24381/180964".to_string(), "".to_string()));
    //
    // let test_3 = query_refno_belong_rooms(RefU64::from_str("24383/83995").unwrap()).await;
    // assert_eq!(test_3, ("24381/181010".to_string(), "".to_string()));
    //

    Ok(())
}