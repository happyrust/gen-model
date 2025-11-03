use aios_core::pdms_types::*;
use aios_core::shape::pdms_shape::PlantMesh;
use bevy_transform::prelude::Transform;
use parry3d::utils::hashmap::HashMap;
// use crate::aql_api::pdms_mesh::query_pdms_mesh_aql;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;

impl AiosDBManager {
    ///拉取geo_hashs对应的meshes
    pub async fn cache_plant_meshes(
        &self,
        geo_hashes: impl IntoIterator<Item = &u64>,
        overwrite: bool,
    ) -> anyhow::Result<bool> {
        // let new_hashes = {
        //     let m = self.cached_mesh_mgr.read().await;
        //     if !overwrite {
        //         let h = geo_hashes.into_iter()
        //             .filter_map(|x| (!m.contains_key(x)).then(|| Some(*x)))
        //             .map(|s| s.unwrap())
        //             .collect::<Vec<_>>();
        //         h
        //     } else {
        //         geo_hashes.into_iter().cloned().collect::<Vec<_>>()
        //     }
        // };
        // if new_hashes.is_empty() { return Ok(true);  }
        // let plant_mesh = query_pdms_mesh_aql(&self.get_arango_db().await?, &new_hashes).await?;
        // let mut cache_mesh_mgr = self.cached_mesh_mgr.write().await;
        // for (k, v) in plant_mesh.meshes {
        //     cache_mesh_mgr.insert(k, v);
        // }
        Ok(true)
    }

    ///获取geo_hash 对应的plant mesh数据
    pub async fn get_plant_mesh(&self, geo_hash: u64) -> anyhow::Result<Option<PlantMesh>> {
        // {
        //     self.cache_plant_meshes(&[geo_hash], false).await?;
        // }
        // {
        //     let m = self.cached_mesh_mgr.read().await;
        //     if m.contains_key(&geo_hash) {
        //         return Ok(m.get(&geo_hash).map(|x| x.mesh.clone()).unwrap());
        //     }
        // };
        Ok(None)
    }

    ///获得变换后的mesh
    pub async fn get_transformed_plant_mesh(
        &self,
        geo_hash: u64,
        t: &Transform,
    ) -> anyhow::Result<Option<PlantMesh>> {
        self.get_plant_mesh(geo_hash)
            .await
            .map(|x| x.map(|x| x.transform_by(&t.to_matrix().as_dmat4())))
    }

    ///获得变换后的mesh的标高
    pub async fn get_transformed_mesh_elev(
        &self,
        geo_hash: u64,
        t: &Transform,
    ) -> anyhow::Result<Option<(f32, f32)>> {
        if let Some(mesh) = self.get_transformed_plant_mesh(geo_hash, t).await? {
            let elev_max = mesh
                .vertices
                .iter()
                .map(|x| x.z)
                .max_by(|a, b| a.partial_cmp(b).unwrap())
                .unwrap_or_default();
            let elev_min = mesh
                .vertices
                .iter()
                .map(|x| x.z)
                .min_by(|a, b| a.partial_cmp(b).unwrap())
                .unwrap_or_default();
            Ok(Some((elev_min, elev_max)))
        } else {
            Ok(None)
        }
    }
}
