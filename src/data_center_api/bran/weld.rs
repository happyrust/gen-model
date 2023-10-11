use std::io::Write;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance};
use aios_core::data_center::AttrValue::{AttrFloat, AttrString, AttrVec3};
use aios_core::pdms_types::{PdmsElement, RefU64};
use aios_core::tool::math_tool::quat_to_pdms_ori_str;
use dashmap::DashMap;
use crate::api::element::{query_ele_node, query_name};
use crate::aql_api::foreign_refnos::query_foreign_name_aql;
use crate::aql_api::pdms_room::query_room_name_from_refno_aql;
use crate::data_center_api::auto_get_attr::get_material_map_from_code;
use crate::data_center_api::data_api::{get_bran_itema_attr, get_refno_latest_version, get_spre_material_code};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::arangodb::ArDatabase;

pub async fn get_data_center_weld_attr(refno: PdmsElement,bran_name:&str,database:&ArDatabase,aios_mgr:&AiosDBManager) -> DataCenterInstance {
    let mut result = Vec::new();
    let item_1 = DataCenterAttr {
        attribute_model_code: "ITEM1".to_string(),
        value: AttrString(refno.refno.to_refno_string()).into(),
    };
    result.push(item_1);
    let item_2 = DataCenterAttr {
        attribute_model_code: "ITEMA1".to_string(),
        value:AttrString(refno.name.clone()).into(),
    };
    result.push(item_2);
    let item_3 = DataCenterAttr {
        attribute_model_code: "ITEMA2".to_string(),
        value:  AttrString(refno.noun).into(),
    };
    result.push(item_3);
    let item_4 = DataCenterAttr {
        attribute_model_code: "ITEMAB3".to_string(),
        value:  AttrString(bran_name.to_string()).into(),
    };
    result.push(item_4);
    let item_5 = DataCenterAttr {
        attribute_model_code: "ITEMAB4".to_string(),
        value: AttrString("".to_string()).into(),
    };
    result.push(item_5);
    let world_position = aios_mgr.get_world_transform(refno.refno).unwrap_or(None).unwrap_or_default();
    let item_5 = DataCenterAttr {
        attribute_model_code: "ITEMA5".to_string(),
        value: AttrVec3(world_position.translation).into(),
    };
    result.push(item_5);
    let item_8 = DataCenterAttr {
        attribute_model_code: "ITEMA8".to_string(),
        value: AttrString(quat_to_pdms_ori_str(&world_position.rotation)).into(),
    };
    result.push(item_8);
    let room_code = query_room_name_from_refno_aql(refno.refno,database).await.unwrap_or(None).unwrap_or("".to_string());
    result.push(DataCenterAttr {
        attribute_model_code: "ITEMA20".to_string(),
        value: AttrString(room_code).into(),
    });
    DataCenterInstance {
        object_model_code: "ITEMC".to_string(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        instance_code: refno.name,
        version: get_refno_latest_version(),
        attributes: result,
    }
}

#[tokio::test]
async fn test_get_data_center_weld_attr() -> anyhow::Result<()> {
    let aios_mgr = AiosDBManager::init_form_config().await?;
    let database = aios_mgr.get_arango_db().await?;
    let tee_refno = RefU64::from_refno_str("24383/66752").unwrap();
    let pool = aios_mgr.get_project_pool_by_refno(tee_refno).await.unwrap();
    let tee_node = query_ele_node(tee_refno,&pool.1).await.unwrap();
    let owner_name = query_name(tee_node.owner,&pool.1).await.unwrap();

    let result = get_data_center_weld_attr(tee_node.into(),&owner_name,&database,&aios_mgr).await;
    let mut file = std::fs::File::create("tee.json")?;
    let json = serde_json::to_vec(&result)?;
    file.write_all(&json)?;
    Ok(())
}