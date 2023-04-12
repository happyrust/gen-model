use glam::Vec3;
use nom::AsBytes;

pub mod sctn;

#[derive(Debug)]
pub struct SctnAnsysData {
    pub point: Vec<Vec3>,
    pub rotation: Vec3,
    pub connect_point: Vec<u8>,
    pub w1: f32,
    pub w2: f32,
    pub w3: f32,
    pub t1: f32,
    pub t2: f32,
    pub t3: f32,
    // 是否为槽钢
    pub b_channel_steel: bool,
}

impl SctnAnsysData {
    pub fn create_single_sctn_ansys_file(&self) -> Vec<u8> {
        let mut result = Vec::new();
        result.append(&mut create_ansys_file_header());
        // 点
        for (i, point) in self.point.iter().enumerate() {
            result.append(&mut format!("k, {} , {}, {}, {} \r\n", i + 1, point.x, point.y, point.z).into_bytes());
        }
        // 方向
        result.append(&mut format!("k , {} ,{},{},{}\r\n", self.point.len() + 1, self.rotation.x, self.rotation.y, self.rotation.z).into_bytes());
        // 连接
        result.append(&mut format!("l , {} , {}\r\n\r\n", 1, 2).into_bytes());

        result.append(&mut "Mp, Ex, 1, 2e11 \r\nMp,Nuxy, 1, 0.3 \r\n \r\nSecType, 1, beam, I \r\n".as_bytes().into());
        // 截面各个属性
        result.append(&mut format!("w1 = {}\r\n", self.w1 / 1000.0).into_bytes());
        result.append(&mut format!("w2 = {}\r\n", self.w2 / 1000.0).into_bytes());
        result.append(&mut format!("w3 = {}\r\n", self.w3 / 1000.0).into_bytes());
        result.append(&mut format!("t1 = {}\r\n", self.t1 / 1000.0).into_bytes());
        result.append(&mut format!("t2 = {}\r\n", self.t2 / 1000.0).into_bytes());
        result.append(&mut format!("t3 = {}\r\n", self.t3 / 1000.0).into_bytes());
        result.append(&mut "SecData, w1, w2, w3, t1, t2, t3 \r\n\r\n".as_bytes().into());

        // 结束
        result.append(&mut "LATT,1, ,1, ,3, ,1 \r\n\r\nLMESH, 1 \r\n/VIEW,1,1,1,1 \r\n/ANG,1 \r\n/REP,FAST \r\n/SHRINK,0 \r\n/ESHAPE,1.0 \r\n/EFACET,1 \r\n/RATIO,1,1,1 \r\n/CFORMAT,32,0 \r\n/REPLOT".as_bytes().into());
        result
    }

    pub fn create_many_sctn_ansys_file(sctns: Vec<SctnAnsysData>) -> Vec<u8> {
        let mut result = Vec::new();
        for (i, sctn) in sctns.into_iter().enumerate() {
            if i == 0 { result.append(&mut create_ansys_file_header()) } else { result.append(&mut create_data_header()) }

            for (j, point) in sctn.point.iter().enumerate() {
                result.append(&mut format!("k, {} , {}, {}, {} \r\n", j + 1 + (i * 3), point.x, point.y, point.z).into_bytes());
            }

            result.append(&mut format!("k , {} ,{},{},{}\r\n", sctn.point.len() + 1 + (i * 3), sctn.rotation.x, sctn.rotation.y, sctn.rotation.z).into_bytes());

            result.append(&mut format!("l , {} , {}\r\n\r\n", 1 + (i * 3), 2 + (i * 3)).into_bytes());


            let material = if sctn.b_channel_steel { "CHAN" } else { "I" };
            result.append(&mut format!("Mp, Ex, 1, 2e11 \r\nMp,Nuxy, 1, 0.3 \r\n \r\nSecType, {}, beam, {} \r\n", i + 1, material).into_bytes());

            // 截面各个属性
            result.append(&mut format!("w1 = {}\r\n", sctn.w1 / 1000.0).into_bytes());
            result.append(&mut format!("w2 = {}\r\n", sctn.w2 / 1000.0).into_bytes());
            result.append(&mut format!("w3 = {}\r\n", sctn.w3 / 1000.0).into_bytes());
            result.append(&mut format!("t1 = {}\r\n", sctn.t1 / 1000.0).into_bytes());
            result.append(&mut format!("t2 = {}\r\n", sctn.t2 / 1000.0).into_bytes());
            result.append(&mut format!("t3 = {}\r\n", sctn.t3 / 1000.0).into_bytes());
            result.append(&mut "SecData, w1, w2, w3, t1, t2, t3 \r\n\r\n".as_bytes().into());

            result.append(&mut format!("LATT,1, ,1, ,{}, ,{} \r\n\r\nLMESH, {} \r\n/VIEW,1,1,1,1 \r\n/ANG,1 \r\n/REP,FAST \r\n/SHRINK,0 \r\n/ESHAPE,1.0 \r\n/EFACET,1 \r\n/RATIO,1,1,1 \r\n/CFORMAT,32,0 \r\n/REPLOT\r\n",
                                       sctn.point.len() + 1 + (i * 3), i + 1, i + 1).into_bytes());
        }
        result
    }
}

fn create_ansys_file_header() -> Vec<u8> {
    let mut result = Vec::new();
    result.append(&mut "finish \r\n/clear \r\n/prep7  \r\nET, 1, beam188 \r\n\r\n".as_bytes().into());
    result
}

fn create_data_header() -> Vec<u8> {
    let mut result = Vec::new();
    result.append(&mut "finish \r\n/prep7  \r\nET, 1, beam188 \r\n\r\n".as_bytes().into());
    result
}