use std::f32::EPSILON;
use std::vec::Vec;
use aios_core::parsed_data::geo_params_data::CateGeoParam;
use aios_core::parsed_data::GeomsInfo;
use aios_core::pdms_types::{AttrMap, RefU64};
use aios_core::prim_geo::category::CateBrepShape;
use aios_core::prim_geo::loft::SctnSolid;
use append_only_vec::AppendOnlyVec;
use dashmap::{DashMap, DashSet};
use glam::{TransformSRT, Vec3};
use regex::internal::Input;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::CateBrepShapeMap;

pub async fn create_st_geos<T: PdmsDataInterface>(refno: RefU64, att: &AttrMap, geom_info: &GeomsInfo,
                                                  brep_shapes_map: &CateBrepShapeMap, interface: &T) -> anyhow::Result<bool>  {
    let geoms = &geom_info.geometries;
    if geoms.len() == 0 { return Ok(true); }
    let type_name = att.get_type();
    let arc_path = if type_name == "GENSEC" {
        let parent_pos = interface.get_world_transform(refno).await?.unwrap_or_default().translation;
        //dbg!(parent_pos);
        let children_refs = interface.get_children_refs(refno).await?;
        let mut res = None;
        for x in children_refs.iter() {
            let refs = interface.get_children_refs(*x).await?;
            if refs.len() >= 3 {
                let pt1 = interface.get_world_transform(refs[0]).await?.unwrap_or_default().translation;
                let pt2 = interface.get_world_transform(refs[1]).await?.unwrap_or_default().translation;
                let pt3 = interface.get_world_transform(refs[2]).await?.unwrap_or_default().translation;
                res = Some((
                    pt1 - parent_pos,
                    pt2 - parent_pos,
                    pt3 - parent_pos,
                ));
            }
        }
        if res.is_some() {
            //dbg!(&res);
        }
        res
    } else { None };

    let mut height = 0.0;
    if let Some(poss) = att.get_poss() {
        if let Some(pose) = att.get_pose() {
            height = pose.distance(poss);
        }
    }
    let drns = att.get_vec3("DRNS").unwrap_or_default();
    let drne = att.get_vec3("DRNE").unwrap_or_default();
    //rotate the profile
    for (i, geom) in geoms.iter().enumerate() {
        if let CateGeoParam::Profile(profile) = geom{
            let loft = SctnSolid {
                profile: profile.clone(),
                drns,
                drne,
                height,
                arc_path,
            };
            brep_shapes_map.entry(refno).or_insert(Vec::new()).push(CateBrepShape{
                refno,
                brep_shape: Box::new(loft),
                transform: TransformSRT::IDENTITY,
                visible: true,
                is_tubi: false,
                pts: Default::default()
            });
        }
    }

    Ok(true)
}