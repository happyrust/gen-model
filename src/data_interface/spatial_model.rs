// cal_zdis_pki

use aios_core::pdms_types::RefU64;
use aios_core::prim_geo::spine::SweepPath3D;
use aios_core::shape::pdms_shape::LEN_TOL;
use aios_core::tool::math_tool::quat_to_pdms_ori_str;
use glam::{Mat3, Quat, Vec3};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;

impl AiosDBManager {
    /// 计算ZDIS和PKDI, refno 是有这个SPLINE属性或者SCTN这种的参考号
    pub fn cal_zdis_pkdi_in_section(&self, refno: RefU64, pkdi: f32, zdis: f32) -> (Quat, Vec3) {
        let mut pos = Vec3::default();
        let mut quat = Quat::IDENTITY;
        let mut spline_paths = self.get_spline_path(refno)
            .unwrap_or_default();
        let mut sweep_paths = spline_paths.iter()
            .map(|x| x.generate_paths().0).flatten().collect::<Vec<_>>();
        let lens: Vec<f32> = sweep_paths.iter().map(|x| x.length()).collect::<Vec<_>>();
        let total_len: f32 = lens.iter().sum();
        // dbg!(&spline_paths);
        if spline_paths.is_empty() {
            return (quat, pos);
        }
        let mut tmp_dist = zdis;
        let mut tmp_porp = pkdi.clamp(0.0, 1.0);
        let start_len = total_len * tmp_porp;
        //pkdi 给了一个比例的距离
        tmp_dist += start_len;
        //后续要考虑反方向的情况
        let mut cur_len = 0.0;
        for (i, path) in sweep_paths.into_iter().enumerate() {
            tmp_dist -= cur_len;
            cur_len = lens[i];
            //在第一段范围内，或者是最后一段，就没有长度的限制
            if tmp_dist > cur_len || i == lens.len() - 1 {
                match path {
                    SweepPath3D::Line(l) => {
                        let mut dir = (l.end - l.start).normalize();
                        pos += dir * tmp_dist + l.start;
                        // let z_axis = Vec3::Y;
                        // let x_axis = -Vec3::X;
                        // let y_axis = Vec3::Z;
                        // quat = Quat::from_mat3(&Mat3::from_cols(
                        //     x_axis,
                        //     y_axis,
                        //     z_axis,
                        // ));
                        break;
                    }
                    SweepPath3D::SpineArc(arc) => {
                        //使用弧长去计算当前的点的位置
                        if arc.radius > LEN_TOL {
                            let v = (arc.start_pt - arc.center).normalize();
                            let mut start_angle = Vec3::X.angle_between(v);
                            if Vec3::X.cross(v).z < 0.0 {
                                start_angle = -start_angle;
                            }
                            let mut theta = (tmp_dist / arc.radius);
                            if arc.clock_wise {
                                theta = -theta;
                            }
                            theta = start_angle + theta;
                            pos = arc.center + arc.radius * Vec3::new(theta.cos(), theta.sin(), 0.0);
                            let y_axis = Vec3::Z;
                            let mut x_axis = (arc.center - pos).normalize();
                            if arc.clock_wise {
                                x_axis = -x_axis;
                            }
                            let z_axis = x_axis.cross(y_axis).normalize();
                            quat = Quat::from_mat3(&Mat3::from_cols(
                                x_axis,
                                y_axis,
                                z_axis,
                            ));
                            // dbg!(quat_to_pdms_ori_str(&quat));
                        }
                    }
                    _ => {}
                }
            }
        }
        (quat, pos)
    }
}