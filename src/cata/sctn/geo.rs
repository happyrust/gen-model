use std::default;
use std::f32::consts::PI;
use std::f32::EPSILON;
use std::vec::Vec;
use aios_core::parsed_data::{CateProfileParam, CateGeomsInfo};
use aios_core::parsed_data::geo_params_data::{CateGeoParam, PdmsGeoParam};
use aios_core::pdms_types::{AttrMap, RefU64};
use aios_core::prim_geo::category::CateBrepShape;
use aios_core::prim_geo::sweep_solid::SweepSolid;
use aios_core::prim_geo::spine::{Line3D, Spine3D, SpineCurveType, SweepPath3D};
use aios_core::shape::pdms_shape::BrepShapeTrait;
use aios_core::tool::dir_tool::parse_ori_str_to_quat;
use aios_core::tool::math_tool::{quat_to_pdms_ori_str, to_pdms_ori_str, to_pdms_vec_str};
use anyhow::anyhow;
use bevy_transform::prelude::Transform;
use dashmap::{DashMap, DashSet};
use glam::{Mat3, Quat, Vec3};
use regex::internal::Input;
use parry3d::bounding_volume::Aabb;
use crate::cata::direction_parse::parse_expr_to_dir;

use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::structs::CateBrepShapeMap;

pub struct ProfileGeosPoints {
    pub points: Vec<(Vec3, Vec3, Vec3)>,
}

pub async fn create_profile_geos<T: PdmsDataInterface>(refno: RefU64,
                                                       att: &AttrMap,
                                                       geom_info: &CateGeomsInfo,
                                                       brep_shapes_map: &CateBrepShapeMap,
                                                       interface: &T, ) -> anyhow::Result<bool> {
    let geoms = &geom_info.geometries;
    if geoms.len() == 0 { return Ok(false); }
    let type_name = att.get_type();
    let mut plane_normal = Vec3::Z;
    let mut extrude_dir = Vec3::Z;
    let mut drns = att.get_vec3("DRNS").unwrap_or_default().normalize();
    let mut drne = att.get_vec3("DRNE").unwrap_or_default().normalize();
    let parent_refno = att.get_owner().unwrap();
    let mut spine_paths = if type_name == "GENSEC" || type_name == "WALL" {
        let children_refs = interface.get_children_refs(refno).await?;
        let mut paths = vec![];
        for x in children_refs.iter() {
            let type_name = interface.get_refno_basic(*x).map(|x| x.get_type().to_string())
                .unwrap_or("unset".to_string());
            if type_name != "SPINE" {
                continue;
            }
            let spine_att = interface.get_attr(*x).await?;
            drns = spine_att.get_vec3("DRNS").unwrap_or_default();
            drne = spine_att.get_vec3("DRNE").unwrap_or_default();
            let ch_refs = interface.get_children_refs(*x).await?;
            if (ch_refs.len() - 1) % 2 == 0 {
                for i in 0..(ch_refs.len() - 1) / 2 {
                    let att1: AttrMap = interface.get_attr(ch_refs[2 * i]).await?;
                    let att2 = interface.get_attr(ch_refs[2 * i + 1]).await?;
                    let att3 = interface.get_attr(ch_refs[2 * i + 2]).await?;
                    let pt0 = att1.get_position().unwrap_or_default();
                    let pt1 = att3.get_position().unwrap_or_default();
                    let mid_pt = att2.get_position().unwrap_or_default();
                    let cur_type_str = att2.get_str("CURTYP").unwrap_or("unset");
                    let curve_type = match cur_type_str {
                        "CENT" => { SpineCurveType::CENT }
                        "THRU" => { SpineCurveType::THRU }
                        _ => { SpineCurveType::UNKNOWN }
                    };
                    paths.push(Spine3D {
                        pt0,
                        pt1,
                        thru_pt: mid_pt,
                        center_pt: mid_pt,
                        cond_pos: att2.get_vec3("CPOS").unwrap_or_default(),
                        curve_type,
                        preferred_dir: spine_att.get_vec3("YDIR").unwrap_or(Vec3::Z),
                        radius: att2.get_f32("RAD").unwrap_or_default(),
                    });
                }
            } else if ch_refs.len() == 2 {
                let att1: AttrMap = interface.get_attr(ch_refs[0]).await?;
                let att2 = interface.get_attr(ch_refs[1]).await?;
                let pt0 = att1.get_position().unwrap_or_default();
                let pt1 = att2.get_position().unwrap_or_default();
                if att1.get_type() == "POINSP" && att2.get_type() == "POINSP" {
                    paths.push(Spine3D {
                        pt0,
                        pt1,
                        curve_type: SpineCurveType::LINE,
                        preferred_dir: spine_att.get_vec3("YDIR").unwrap_or(Vec3::Z),
                        ..Default::default()
                    });
                }
            }
        }
        paths
    } else { vec![] };

    // let drne = Vec3::X;
    if drns.is_normalized() && drne.is_normalized() {
        let parent_rot = interface.get_world_transform(parent_refno).await.unwrap_or_default().unwrap_or_default().rotation;
        let current_rot = interface.get_world_transform(refno).await.unwrap_or_default().unwrap_or_default().rotation;
        let new_rot =  current_rot.inverse() * parent_rot;
        // dbg!(quat_to_pdms_ori_str(&new_rot));

        println!("refno: {}, 原始drns: {:?}, drne: {:?}", refno, to_pdms_vec_str(&drns), to_pdms_vec_str(&drne));
        let mut tmp_drns = (new_rot.mul_vec3(drns)).normalize();
        let mut tmp_drne = (new_rot.mul_vec3(drne)).normalize();
        ///处理随意设置方向的情况，保证一致性
        if (Vec3::Z).angle_between(tmp_drns).abs() > PI/2.0 {
            drns = -tmp_drns;
        }else{
            drns = tmp_drns;
        }
        if (Vec3::Z).angle_between(-tmp_drne).abs() > PI/2.0 {
            drne = -tmp_drne;
        }else{
            drne = tmp_drne;
        }
        println!("refno: {}, 变换后drns: {:?}, drne: {:?}", refno, to_pdms_vec_str(&drns), to_pdms_vec_str(&drne));
    }

    let mut height = 0.0;
    if spine_paths.len() == 0 {
        if let Some(poss) = att.get_poss() &&
            let Some(pose) = att.get_pose() {
            height = pose.distance(poss);
            //还原成相对坐标系下的拉升方向
            for (i, geom) in geoms.iter().enumerate() {
                if let CateGeoParam::Profile(profile) = geom {
                    if let CateProfileParam::SPRO(spro) = profile {
                        plane_normal = spro.normal_axis.normalize();
                    }
                    if let CateProfileParam::SANN(s) = profile {
                        plane_normal = s.paxis.as_ref().map(|x| x.dir).unwrap_or(Vec3::Y);
                    }
                    let bangle = att.get_f32("BANG").unwrap_or_default();
                    let solid = SweepSolid {
                        profile: profile.clone(),
                        drns: drns.normalize_or_zero(),
                        drne: drne.normalize_or_zero(),
                        bangle,
                        plane_normal,
                        extrude_dir,
                        height,
                        path: SweepPath3D::Line(Line3D {
                            start: Default::default(),
                            end: pose - poss,
                            is_spine: false,
                        }),
                        lmirror: att.get_bool("LMIRR").unwrap_or_default(),
                    };
                    // dbg!(&solid);
                    brep_shapes_map.entry(refno).or_insert(Vec::new()).push(CateBrepShape {
                        refno,
                        brep_shape: Box::new(solid),
                        transform: Transform::IDENTITY,
                        visible: true,
                        is_tubi: false,
                        shape_err: None,
                        pts: Default::default(),
                        is_ngmr: false,
                    });
                }
            }
        }
    } else {
        for spine in spine_paths {
            for (i, geom) in geoms.iter().enumerate() {
                if let CateGeoParam::Profile(profile) = geom {
                    if let CateProfileParam::SPRO(spro) = profile {
                        plane_normal = spro.normal_axis.normalize();
                    }
                    if let CateProfileParam::SANN(s) = profile {
                        plane_normal = s.paxis.as_ref().map(|x| x.dir).unwrap_or(Vec3::Y);
                    }
                    let (paths, transform) = spine.generate_paths();
                    let bangle = att.get_f32("BANG").unwrap_or_default();
                    for path in paths {
                        let loft = SweepSolid {
                            profile: profile.clone(),
                            drns: drns.normalize_or_zero(),
                            drne: drne.normalize_or_zero(),
                            bangle,
                            plane_normal,
                            extrude_dir,
                            height: 0.0,
                            path,
                            lmirror: att.get_bool("LMIRR").unwrap_or_default(),
                        };
                        let transform = loft.get_trans() * transform;
                        brep_shapes_map.entry(refno).or_insert(Vec::new()).push(CateBrepShape {
                            refno,
                            brep_shape: Box::new(loft),
                            transform,
                            visible: true,
                            is_tubi: false,
                            shape_err: None,
                            pts: Default::default(),
                            is_ngmr: false,
                        });
                    }
                }
            }
        }
    }
    Ok(true)
}