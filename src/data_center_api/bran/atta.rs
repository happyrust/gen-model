use std::io::Write;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance};
use aios_core::data_center::AttrValue::{AttrFloat, AttrString, AttrVec3};
use aios_core::pdms_types::*;
use aios_core::tool::math_tool::quat_to_pdms_ori_str;
use dashmap::DashMap;
use crate::api::element::{query_ele_node, query_name};
use crate::aql_api::foreign_refnos::query_foreign_name_aql;
use crate::aql_api::pdms_room::query_room_name_from_refno_aql;
use crate::data_center_api::auto_get_attr::get_material_map_from_code;
use crate::data_center_api::data_api::{get_bran_itema_attr, get_ispec_from_attr, get_refno_latest_version, get_rtext_from_attr, get_spre_material_code};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::arangodb::ArDatabase;

pub async fn get_data_center_atta_attr(refno: PdmsElement, bran_name: &str, database: &ArDatabase, aios_mgr: &AiosDBManager) -> DataCenterInstance {
    let need_query_material_code = vec![("ITEMB14".to_string(), "Code".to_string()),
                                        ("ITEMB15".to_string(), "RCCM".to_string()),
                                        ("ITEMB16".to_string(), "QAGrade".to_string()),
                                        ("ITEMB17".to_string(), "Diameter".to_string()), ];
    let mut result = Vec::new();
    let item_1 = DataCenterAttr {
        attribute_model_code: "ITEM1".to_string(),
        value: AttrString(refno.refno.to_refno_string()).into(),
    };
    result.push(item_1);
    let item_2 = DataCenterAttr {
        attribute_model_code: "ITEMB1".to_string(),
        value: AttrString(refno.name.clone()).into(),
    };
    result.push(item_2);
    let item_3 = DataCenterAttr {
        attribute_model_code: "ITEMB2".to_string(),
        value: AttrString(refno.noun).into(),
    };
    result.push(item_3);
    let item_4 = DataCenterAttr {
        attribute_model_code: "ITEMB3".to_string(),
        value: AttrString(bran_name.to_string()).into(),
    };
    result.push(item_4);
    let item_5 = DataCenterAttr {
        attribute_model_code: "ITEMB4".to_string(),
        value: AttrString("".to_string()).into(),
    };
    result.push(item_5);
    let world_position = aios_mgr.get_world_transform(refno.refno).unwrap_or(None).unwrap_or_default();
    let item_5 = DataCenterAttr {
        attribute_model_code: "ITEMB5".to_string(),
        value: AttrVec3(world_position.translation).into(),
    };
    result.push(item_5);
    let item_8 = DataCenterAttr {
        attribute_model_code: "ITEMB8".to_string(),
        value: AttrString(quat_to_pdms_ori_str(&world_position.rotation)).into(),
    };
    result.push(item_8);
    let room_code = query_room_name_from_refno_aql(refno.refno, database).await.unwrap_or(None).unwrap_or("".to_string());
    result.push(DataCenterAttr {
        attribute_model_code: "ITEMB11".to_string(),
        value: AttrString(room_code).into(),
    });
    let attr = aios_mgr.get_attr(refno.refno).await.unwrap_or_default();
    let ispec = get_ispec_from_attr(&attr, &aios_mgr).await.unwrap_or("".to_string());
    result.push(DataCenterAttr {
        attribute_model_code: "ITEMB12".to_string(),
        value: AttrString(ispec).into(),
    });
    let tspe = query_foreign_name_aql(refno.refno, vec!["TSPE", "TSPE"], database).await.unwrap_or(None).unwrap_or("".to_string());
    result.push(DataCenterAttr {
        attribute_model_code: "ITEMB13".to_string(),
        value: AttrString(tspe).into(),
    });

    let spre_name = query_foreign_name_aql(refno.refno, vec!["SPRE", "SPRE"], database).await.unwrap_or(None).unwrap_or("".to_string());
    let material_code = get_spre_material_code(&spre_name).unwrap_or("".to_string());
    let material_map = if let Ok(puhua_pool) = aios_mgr.get_puhua_pool().await {
        let query_code = need_query_material_code.iter().map(|x| x.1.clone()).collect::<Vec<_>>();
        let material_map = get_material_map_from_code(&material_code, query_code, &puhua_pool).await;
        material_map
    } else {
        DashMap::default()
    };
    for (item_code, material_code) in &need_query_material_code {
        let material = if material_map.contains_key(material_code) {
            material_map.get(material_code).unwrap().value().clone()
        } else {
            "".to_string()
        };
        result.push(DataCenterAttr {
            attribute_model_code: item_code.to_string(),
            value: material,
        });
    }
    DataCenterInstance {
        object_model_code: "ITEMAB".to_string(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        instance_code: refno.name,
        version: get_refno_latest_version(),
        attributes: result,
    }
}

#[tokio::test]

async fn test_get_data_center_atta_attr() -> anyhow::Result<()> {
    let aios_mgr = AiosDBManager::init_form_config().await?;
    let database = aios_mgr.get_arango_db().await?;
    let tee_refno = RefU64::from_refno_str("24383/66752").unwrap();
    let pool = aios_mgr.get_project_pool_by_refno(tee_refno).await.unwrap();
    let tee_node = query_ele_node(tee_refno, &pool.1).await.unwrap();
    let owner_name = query_name(tee_node.owner, &pool.1).await.unwrap();

    let result = get_data_center_atta_attr(tee_node.into(), &owner_name, &database, &aios_mgr).await;
    let mut file = std::fs::File::create("tee.json")?;
    let json = serde_json::to_vec(&result)?;
    file.write_all(&json)?;
    Ok(())
}

#[test]
fn test_lock() {
    use std::sync::{Mutex, Arc};
    use std::thread;

    // 创建两个互斥锁
    let mutex1 = Arc::new(Mutex::new(1));
    let mutex2 = Arc::new(Mutex::new(2));

    let mutex1_clone = Arc::clone(&mutex1);
    let mutex2_clone = Arc::clone(&mutex2);

    let handle1 = thread::spawn(move || {
        // 尝试获取 mutex1
        let _lock1 = mutex1_clone.lock().unwrap();
        println!("Thread 1 acquired mutex1");
        // 等待一段时间，模拟其他工作
        thread::sleep(std::time::Duration::from_millis(10));
        println!("Thread 1 waiting for mutex2");
        // 尝试获取 mutex2
        let _lock2 = mutex2_clone.lock().unwrap();
        println!("Thread 1 acquired mutex2");
    });

    let handle2 = thread::spawn(move || {
        // 尝试获取 mutex2
        let _lock2 = mutex2.lock().unwrap();
        println!("Thread 2 acquired mutex2");
        // 等待一段时间，模拟其他工作
        thread::sleep(std::time::Duration::from_millis(10));
        println!("Thread 2 waiting for mutex1");
        // 尝试获取 mutex1，但由于它已被Thread 1占用，因此会阻塞
        let _lock1 = mutex1.lock().unwrap();
        println!("Thread 2 acquired mutex1");
    });

    // 等待两个线程完成
    handle1.join().unwrap();
    handle2.join().unwrap();
}