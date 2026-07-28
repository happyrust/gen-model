use aios_core::SUL_DB;
use aios_core::error::init_save_database_error;
use dashmap::DashMap;
use parry3d::bounding_volume::Aabb;
use std::collections::HashMap;
use tokio::task::JoinSet;

use crate::surreal_retry::execute_surreal_checked;

/// 把一批 `aabb:⟨hash⟩` 记录写进库。
///
/// **必须先于指向它们的 `inst_relate.aabb` 指针落库**——与 `trans` 记录同一条 D9
/// 教训（见 `increment_manager::update_world_transforms`）：指针先落而记录缺位时，
/// 所有 `aabb.d` 读者取到 none，元素从几何查询与包围盒刷新里整条消失。
/// `INSERT IGNORE` 幂等，重放收敛；失败上抛由调用方按任务结算，不再静默吞掉。
pub async fn save_aabb_to_surreal(aabb_map: &DashMap<String, Aabb>) -> anyhow::Result<()> {
    if aabb_map.is_empty() {
        return Ok(());
    }
    let keys = aabb_map
        .iter()
        .map(|kv| kv.key().clone())
        .collect::<Vec<_>>();
    for chunk in keys.chunks(300) {
        let mut sql = "".to_string();
        for k in chunk {
            let v = aabb_map.get(k).expect("key was collected from this map");
            let json = format!(
                "{{'id':aabb:⟨{}⟩, 'd':{}}}",
                k,
                serde_json::to_string(v.value())?
            );
            sql.push_str(&format!("INSERT IGNORE INTO aabb {};", json));
        }
        execute_surreal_checked(&sql, "insert aabb records").await?;
    }
    Ok(())
}

pub async fn save_pts_to_surreal(vec3_map: &DashMap<u64, String>) {
    if !vec3_map.is_empty() {
        let keys = vec3_map.iter().map(|kv| *kv.key()).collect::<Vec<_>>();
        for chunk in keys.chunks(100) {
            let mut sql = "".to_string();
            for &k in chunk {
                let v = vec3_map.get(&k).unwrap();
                let json = format!("{{'id':vec3:⟨{}⟩, 'd':{}}}", k, v.value());
                sql.push_str(&format!("INSERT IGNORE INTO vec3 {};", json));
            }
            match SUL_DB.query(&sql).await {
                Ok(_) => {}
                Err(_e) => {
                    init_save_database_error(&sql, &std::panic::Location::caller().to_string());
                }
            };
        }
    }
}

pub async fn save_transforms_to_surreal(trans_map: &HashMap<u64, String>) -> anyhow::Result<()> {
    if !trans_map.is_empty() {
        let keys = trans_map.keys().collect::<Vec<_>>();
        for chunk in keys.chunks(100) {
            let mut sql = "".to_string();
            for &k in chunk {
                let v = trans_map.get(&k).unwrap();
                let json = format!("{{'id':trans:⟨{}⟩, 'd':{}}}", k, v);
                sql.push_str(&format!("INSERT IGNORE INTO trans {};", json));
            }
            // 此前是 `.unwrap()`：传输抖动直接 panic 掉整个 Transform 任务，
            // 队列行既不结算也不累计失败次数（同款 panic 在生产日志有实证）。
            execute_surreal_checked(&sql, "insert trans records").await?;
        }
    }
    Ok(())
}
