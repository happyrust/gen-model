use std::default;
use std::sync::Arc;
use aios_core::pdms_types::RefU64;
use crate::api::children::travel_children_with_type;
use crate::data_interface::tidb_manager::AiosDBManager;
use aios_core::plugging_material::{PluggingMaterial, PluggingMaterialVec, PluggingVec, UpdatePluggingSettingEvent};
use aios_core::plugging_material::PluggingData;
use sqlx::{Executor, MySql, Pool, Row};
use crate::data_interface::interface::PdmsDataInterface;

pub async fn get_plugging_data_detail(aios_mgr: &AiosDBManager, refnos: Vec<(RefU64, String)>) -> anyhow::Result<PluggingVec> {
    let mut hole_data_vec = PluggingVec::default();
    let mut refno_vec = Vec::new();
    let mut wall_vec = Vec::new();
//得到对应类型的参考号
    for i in refnos {
        // let refno_url = i.0.to_url_refno();
        let mut attr_type = "".to_string();
        match i.1.as_str() {
            "STWALL" => {
                attr_type = "FITT".to_string();
            }
            "GWALL" => {
                attr_type = "PFIT".to_string();
            }
            "WALL" => {
                attr_type = "JLDATU".to_string();
            }
            (_) => {}
        }
        if attr_type != "".to_string() {
            if let Some((_, project_db)) = aios_mgr.get_project_pool_by_refno(i.0.clone()).await {
                if let Ok(val) = travel_children_with_type(i.0.clone(), attr_type.clone(), &project_db).await {
                    let mut result = val.into_iter().map(|x| x.refno).collect::<Vec<RefU64>>();
                    if attr_type == "JLDATU".to_string() {
                        for j in result {
                            wall_vec.push((j, i.0.clone()));
                        }
                    } else {
                        for j in result {
                            refno_vec.push((j, i.0.clone()));
                        }
                    }
                }
            }
        }
    }
    //通过得到的参考号取对应的name和para1(除Wall)
    for i in refno_vec {
        if let Ok(attr) = aios_mgr.get_attr(i.0.clone()).await {
            let mut desp = "".to_string();
            let mut materials = "".to_string();
            if let Some(data) = attr.get_f64_vec("DESP") {
                if data.len() != 0 {
                    desp = data[0].to_string();
                }
            }
            // if let Some(data) = attr.get_as_string("JGOBJNOTE") {
            //     materials = data.;
            // }
            if attr.get_name_string().contains("LL") || attr.get_name_string().contains("EE") || attr.get_name_string().contains("KK") {
                hole_data_vec.data.push(PluggingData {
                    name: attr.get_name_string(),
                    size: desp,
                    refno: i.1.clone(),
                    materials,
                    ..Default::default()
                });
            }
        }
    }
    //取Wall
    for i in wall_vec {
        let mut name = "".to_string();
        let mut desp = "".to_string();
        let mut materials = "".to_string();
        //取name
        if let Ok(attr) = aios_mgr.get_attr(i.0.clone()).await {
            name = attr.get_name_string();
            // if let Some(data) = attr.get_as_string("JGOBJNOTE") {
            //     materials = data;
            // }
        }
        //取尺寸
        if let Some((_, project_db)) = aios_mgr.get_project_pool_by_refno(i.0.clone()).await {
            if let Ok(val) = travel_children_with_type(i.0.clone(), "FIXING".to_string(), &project_db).await {
                let mut result = val.into_iter().map(|x| x.refno).collect::<Vec<RefU64>>();
                if result.len() > 0 {
                    if let Ok(attr) = aios_mgr.get_attr(result[0]).await {
                        if let Some(data) = attr.get_f64_vec("DESP") {
                            if attr.len() > 0 {
                                desp = data[0].to_string();
                            }
                        }
                    }
                }
            }
        }
        if name.contains("LL") || name.contains("EE") || name.contains("KK") {
            hole_data_vec.data.push(PluggingData {
                name,
                size: desp,
                refno: i.1.clone(),
                materials,
                ..Default::default()
            });
        }
    }
    return Ok(hole_data_vec);
}


pub async fn get_plugging_setting_data(pool: &Pool<MySql>) -> anyhow::Result<PluggingMaterialVec> {
    //若没有plugging_setting,则创建表
    let create_table_sql = create_plugging_setting_table_sql();
    let mut conn = pool.clone().acquire().await?;
    let create_table_result = conn.execute(create_table_sql.as_str()).await?;

    //若为空表则在表中添加初始数据
    let query_table_sql = gen_query_table_sql();
    let mut conn = pool.acquire().await?;
    if let Ok(query_results) = conn.fetch_all(query_table_sql.as_str()).await {
        if query_results.len() > 0 {
            //暂时这样判断空表
            if query_results[0].get::<i32, _>("COUNT(*)") == 0 as i32 {
                let init_table_sql = init_plugging_setting_table_sql();
                let mut conn = pool.clone().acquire().await?;
                let init_table_result = conn.execute(init_table_sql.as_str()).await?;
            }
        }
    }

    //返回表中的数据
    let mut result = PluggingMaterialVec::default();
    let sql = gen_plugging_setting_table_sql();
    let mut conn = pool.acquire().await?;
    if let Ok(query_results) = conn.fetch_all(sql.as_str()).await {
        for query_result in query_results {
            let plugging_type = query_result.get::<String, _>("plugging_type");
            let water_level = query_result.get::<String, _>("water_level");
            let plugging_thickness = query_result.get::<String, _>("plugging_thickness");
            let material_type = query_result.get::<String, _>("material_type");
            let unit_usage = query_result.get::<String, _>("unit_usage");
            let setting = PluggingMaterial {
                plugging_type,
                material_type,
                hight: water_level,
                thickness: plugging_thickness,
                usage: unit_usage,
            };
            result.data.push(setting);
        }
    }

    Ok(result)
}


pub async fn update_plugging_setting_data(plugging: UpdatePluggingSettingEvent, pool: &Pool<MySql>) -> anyhow::Result<()> {
    let add_setting = plugging.add_plugging_setting;
    let delete_setting = plugging.delete_plugging_setting;
    let mut conn = pool.clone().acquire().await?;
    //新增记录
    let insert_value_sql = gen_insert_plugging_setting_sql(add_setting);
    let _ = conn.execute(insert_value_sql.as_str()).await;
    //删除记录
    let delete_value_sql = delete_plugging_setting_sql(delete_setting);
    let _ = conn.execute(delete_value_sql.as_str()).await;

    Ok(())
}


fn create_plugging_setting_table_sql() -> String {
    format!("CREATE TABLE IF NOT EXISTS plugging_setting(
        plugging_type VARCHAR(255) NOT NULL,
        water_level VARCHAR(255) NOT NULL,
        plugging_thickness VARCHAR(255) NOT NULL,
        material_type VARCHAR(255) NOT NULL,
        unit_usage VARCHAR(255) NOT NULL
    );")
}


fn init_plugging_setting_table_sql() -> String {
    format!("
    INSERT INTO plugging_setting (plugging_type,material_type,water_level,plugging_thickness, unit_usage)
    VALUES ('AFW', '低密硅酮', '<2m','200mm','1'),
           ('AFW', '高密硅酮', '>2m','墙厚','1'),
           ('AFWB', '高密硅酮', '不限','墙厚','1'),
           ('MCT+AFW', '低密硅酮', '不限','200mm','1')")
}


fn gen_plugging_setting_table_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT * FROM plugging_setting"));
    sql
}


fn gen_query_table_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT COUNT(*) FROM plugging_setting"));
    sql
}

fn gen_insert_plugging_setting_sql(plugging_vec: Vec<PluggingMaterial>) -> String {
    let mut insert_sql = String::from("INSERT IGNORE INTO plugging_setting (plugging_type,material_type,water_level,plugging_thickness,unit_usage) VALUES ");
    for plugging in plugging_vec {
        insert_sql.push_str(&format!("( '{}', '{}', '{}', '{}','{}') ,", plugging.plugging_type, plugging.material_type, plugging.hight, plugging.thickness, plugging.usage));
    }
    insert_sql.remove(insert_sql.len() - 1);
    insert_sql
}

fn delete_plugging_setting_sql(plugging_vec: Vec<PluggingMaterial>) -> String {
    let mut delete_sql = String::new();
    for plugging in plugging_vec {
        delete_sql.push_str(&format!("DELETE FROM plugging_setting WHERE plugging_type ='{}' And material_type = '{}' And water_level = '{}' And plugging_thickness = '{}' And unit_usage = '{}' ;", plugging.plugging_type, plugging.material_type, plugging.hight, plugging.thickness, plugging.usage));
    }
    delete_sql
}
