use std::f32::EPSILON;
use std::vec::Vec;
use aios_core::parsed_data::geo_params_data::CateGeoParam;
use aios_core::parsed_data::{CateProfileParam, GeomsInfo};
use aios_core::pdms_types::{AttrMap, RefU64};
use aios_core::prim_geo::category::CateBrepShape;
use aios_core::prim_geo::loft::SctnSolid;
use anyhow::anyhow;
use append_only_vec::AppendOnlyVec;
use dashmap::{DashMap, DashSet};
use glam::{Quat, TransformSRT, Vec3};
use regex::internal::Input;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::CateBrepShapeMap;

pub async fn create_profile_geos<T: PdmsDataInterface>(refno: RefU64, att: &AttrMap, geom_info: &GeomsInfo,
                                                       brep_shapes_map: &CateBrepShapeMap, interface: &T) -> anyhow::Result<Vec3>  {
    let mut center = Vec3::ZERO;
    let geoms = &geom_info.geometries;
    if geoms.len() == 0 { return Ok(center); }
    let type_name = att.get_type();
    let mut plane_normal = Vec3::Z;
    let mut extrude_dir = Vec3::Z;
    let arc_path = if type_name == "GENSEC" || type_name == "WALL" {
        // let parent_pos = interface.get_world_transform(refno).await?.unwrap_or_default().translation;
        let children_refs = interface.get_children_refs(refno).await?;
        let mut res = None;
        for x in children_refs.iter() {
            let type_name = interface.get_refno_basic(*x).map(|x|x.get_type().to_string())
                .unwrap_or("unset".to_string());
            if type_name != "SPINE" {
                continue;
            }
            let refs = interface.get_children_refs(*x).await?;

            if refs.len() >= 3 {
                let att1: AttrMap = interface.get_attr(refs[0]).await?;
                let att2 = interface.get_attr(refs[1]).await?;
                let att3 = interface.get_attr(refs[2]).await?;
                let pt1 = att1.get_position().unwrap_or_default();
                let pt2 = att2.get_position().unwrap_or_default();
                let pt3 = att3.get_position().unwrap_or_default();
                res = Some((
                    pt1 ,
                    pt2,
                    pt3,
                ));
            }
        }
        res
    } else { None };

    let mut height = 0.0;
    if arc_path.is_none() {
        if let Some(poss) = att.get_poss() && let Some(pose) = att.get_pose() {
            height = pose.distance(poss);
            extrude_dir = pose - poss;
            center = extrude_dir/ 2.0;
            extrude_dir = extrude_dir.normalize();
        }
    }

    // dbg!(extrude_vec);
    let drns = att.get_vec3("DRNS").unwrap_or_default();
    let drne = att.get_vec3("DRNE").unwrap_or_default();
    //rotate the profile
    for (i, geom) in geoms.iter().enumerate() {

        if let CateGeoParam::Profile(profile) = geom{

            if let CateProfileParam::SPRO(spro) = profile{
                plane_normal = spro.normal_axis.normalize();
                let len = spro.verts.len();
                if len != 0 {
                    let rot = Quat::from_rotation_arc(Vec3::Z, plane_normal);
                    let mut tmp = Vec3::ZERO;
                    for vert in &spro.verts {
                        tmp += rot.mul_vec3(Vec3::new(vert[0], vert[1],0.0));
                    }
                    let len = len as f32;
                    center.x = tmp.x /len;
                    center.y = tmp.y /len;
                    center.z = tmp.z /len;
                }
            }
            let loft = SctnSolid {
                profile: profile.clone(),
                drns,
                drne,
                plane_normal,
                extrude_dir,
                height,
                arc_path,
            };
            // dbg!(&loft);
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

    Ok(center)
}