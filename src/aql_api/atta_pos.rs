use aios_core::pdms_types::RefU64;

use crate::aql_api::tubi::query_bran_info;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use std::sync::Arc;
use glam::Vec3;



#[tokio::test]
pub async fn test_() -> anyhow::Result<()> {

    Ok(())
}

pub async fn get_atta_pos(brans: Vec<(RefU64,f32)>,mgr:Arc<AiosDBManager>) -> Vec<Vec<Vec3>> {
    let database = mgr.get_arangodb_conn().await.unwrap().clone();
    let mut atta_pos_vec = Vec::new();
    for bran in brans {
        let mut pos_vec = Vec::new();
        // 取arrive，leave
        let data = query_bran_info(bran.0, &database).await.unwrap();
        //取hpos,取tpos
        let len = data.len();
        let hpos = data[0].start_pt;
        let tpos = data[len - 1].end_pt;
        pos_vec.push(hpos);
        // 取wrt
        for i in data {
            //获取转折点坐标
            if i.att_type == "ELBO" || i.att_type == "BEND" {
                let refno: Vec<&str> = i._to.split("/").collect();
                let refno = refno[1];
                let result = mgr.get_world_transform(RefU64::from_url_refno(refno).unwrap()).await.unwrap().unwrap().clone();
                pos_vec.push(result.translation);
            }
        }
        pos_vec.push(tpos);
        let mut dis_vec = Vec::new();
        //求每段直段的距离
        for i in 0..(pos_vec.len() - 1) {
            let dx = pos_vec[i + 1].x - pos_vec[i].x;
            let dy = pos_vec[i + 1].y - pos_vec[i].y;
            let dz = pos_vec[i + 1].z - pos_vec[i].z;
            dis_vec.push((dx.powi(2) + dy.powi(2) + dz.powi(2)).sqrt().round());
        }
        //第一个ATTA点在500mm处，最后一个ATTA点离TPOS100mm以上，中间每间隔interval设置一个ATTA
        let mut atta_vec = Vec::new();
        //当前在哪段
        let mut index = 0;
        let mut dis = dis_vec[index];
        if dis >= 500.0 {
            let pos = atta_pos(pos_vec[index], pos_vec[index + 1], 500.0);
            atta_vec.push(pos);
            dis = dis - 500.0;
        }
        while index < (pos_vec.len() - 2) || dis >= bran.1 {
            if dis >= bran.1 {
                dis -= bran.1;
                let pos = atta_pos(pos_vec[index], pos_vec[index + 1], bran.1);
                atta_vec.push(pos);
            } else {
                index += 1;
                dis += dis_vec[index];
            }
        }
        atta_pos_vec.push(atta_vec);
    }
    return atta_pos_vec;
}

pub fn atta_pos(s_pos: Vec3, e_pos: Vec3, distance: f32) -> Vec3 {
    let x1 = s_pos.x;
    let y1 = s_pos.y;
    let z1 = s_pos.z;
    let x2 = e_pos.x;
    let y2 = e_pos.y;
    let z2 = e_pos.z;
    let dx = x2 - x1;
    let dy = y2 - y1;
    let dz = z2 - z1;
    let line_length = ((dx * dx + dy * dy + dz * dz).sqrt());
    let ratio = distance / line_length;
    return Vec3::new(x1 + ratio * dx, y1 + ratio * dy, z1 + ratio * dz);
}