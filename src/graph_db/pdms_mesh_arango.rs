use std::mem::take;

use aios_core::pdms_types::*;
use arangors_lite::AqlQuery;
use itertools::Itertools;

use crate::consts::AQL_PDMS_MESH_COLLECTION;
use crate::data_interface::tidb_manager::AiosDBManager;

///保存mesh数据到本地缓存
pub fn save_mesh_to_local_db(
    mgr: &AiosDBManager,
    mesh_mgr: &PlantMeshesData,
    replace_mesh: bool,
) -> anyhow::Result<bool> {
    // let mesh_tree = mgr.local_mesh_db.clone();
    // let aabb_tree = mgr.local_mesh_aabb_db.clone();
    // for (h, p) in &mesh_mgr.meshes {
    //     if let Some(m) = &p.mesh && let Some(a) = &p.aabb {
    //         if !replace_mesh {
    //             //跳过不必要的保存
    //             if mesh_tree.get(h.to_be_bytes().as_slice())?.is_some() {
    //                 continue;
    //             }
    //         }
    //         mesh_tree.insert(h.to_be_bytes().as_slice(), m.into_compress_bytes())?;
    //         aabb_tree.insert(h.to_be_bytes().as_slice(), a.to_bytes()?)?;
    //     }
    // }
    Ok(true)
}

// todo 需要改成使用保存文件的方式，不要存储在数据库
///保存mesh数据到图数据库
pub async fn save_mesh_data(
    mgr: &AiosDBManager,
    mesh_mgr: &mut PlantMeshesData,
    replace: bool,
) -> anyhow::Result<()> {
    println!("开始保存mesh数据");
    //不是replace，需要考虑缓存
    let mut meshes_map = &mut mesh_mgr.meshes;
    println!("当前mesh数量：{}", meshes_map.len());

    std::fs::create_dir_all("assets/meshes").unwrap();
    for (hash, p) in meshes_map {
        if let Some(mesh) = &mut p.mesh {
            let file_path = format!("assets/meshes/{}.mesh", hash);
            #[cfg(feature = "debug_obj_export")]
            #[cfg(feature = "debug_obj_export")]
            {
                let _ = std::fs::create_dir_all("models");
                mesh.export_obj(false, &format!("models/{}.obj", hash))
                    .expect("TODO: panic message");
            }
            //跳过重复的保存
            if replace || !std::path::Path::new(&file_path).exists() {
                p.serialize_to_specify_file(&file_path);
            }
        }
    }

    Ok(())
}
