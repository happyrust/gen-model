use serde::{Serialize,Deserialize};

mod modify;
// mod increment;
mod create_att;
mod session_page;


/// 修改属性后，该文件的所有数据页
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct NewPage {
    /// 原始文件
    pub origin_file: Vec<u8>,
    /// 属性页
    pub data_page: Vec<u8>,
    /// 版本号页 1
    pub first_version_page: Vec<u8>,
    /// 版本号页 2
    pub second_version_page: Vec<u8>,
    /// 修改次数页
    pub change_times_page: Vec<u8>,
    /// 会话页
    pub conversion_page: Vec<u8>,
}

impl NewPage {
    pub fn convert_into_one_page(self) -> Vec<u8> {
        [self.origin_file, self.data_page, self.first_version_page,
            self.second_version_page, self.change_times_page,
            self.conversion_page
        ].concat()
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct DataPage {
    /// 隐式属性
    pub implicit_data: Vec<u8>,
    /// 子节点
    pub children: Vec<u8>,
    /// 显示属性
    pub explicit_data: Vec<u8>,
}

impl DataPage {

    pub fn turn_self_into_vec(self) -> Vec<u8> {
        [self.implicit_data, self.children, self.explicit_data].concat()
    }

    pub fn convert_new_data_page(self) -> Vec<u8> {
        let mut result = vec![0; 0x800];
        let value = [self.implicit_data, self.children, self.explicit_data].concat();
        result.splice(0..value.len(), value);
        result
    }
}

