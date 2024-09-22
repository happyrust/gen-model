use std::default;
use std::sync::Arc;
use aios_core::pdms_types::RefU64;
use crate::api::children::travel_children_with_type;
use crate::data_interface::tidb_manager::AiosDBManager;
use aios_core::plugging_material::{PluggingMaterial, PluggingMaterialVec, PluggingVec, UpdatePluggingSettingEvent};
use aios_core::plugging_material::PluggingData;

use sqlx::{Executor, MySql, Pool, Row};
use sqlx::types::Json;
use crate::data_interface::interface::PdmsDataInterface;

pub async fn get_plugging_setting_data(pool: &Pool<MySql>) -> anyhow::Result<PluggingMaterialVec> {
    //若没有plugging_setting,则创建表
    let create_table_sql = create_plugging_setting_table_sql();
    let mut conn = pool.clone();
    let create_table_result = conn.execute(create_table_sql.as_str()).await?;

    //若为空表则在表中添加初始数据
    let query_table_sql = gen_query_table_sql();
    let mut conn = pool;
    if let Ok(query_results) = conn.fetch_all(query_table_sql.as_str()).await {
        if query_results.len() > 0 {
            //暂时这样判断空表
            if query_results[0].get::<i32, _>("COUNT(*)") == 0 as i32 {
                let init_table_sql = init_plugging_setting_table_sql();
                let mut conn = pool.clone();
                let init_table_result = conn.execute(init_table_sql.as_str()).await?;
            }
        }
    }

    //返回表中的数据
    let mut result = PluggingMaterialVec::default();
    let sql = gen_plugging_setting_table_sql();
    let mut conn = pool;
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
    let mut conn = pool.clone();
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
    INSERT IGNORE INTO plugging_setting (plugging_type,material_type,water_level,plugging_thickness, unit_usage)
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
