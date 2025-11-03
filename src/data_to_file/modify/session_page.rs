use std::fs;
use std::io::{Read, Write};
use std::mem::take;
use aios_core::helper::parse_to_u32;
use chrono::{Datelike, DateTime, Local, Timelike};
use itertools::Itertools;
use memchr::memmem::find_iter;
use crate::data_to_file::get_last_page_no;
use crate::data_to_file::modify::modify::GlobalPage;
use serde::{Serialize, Deserialize};

const NAME_PAGE_BYTES: [u8; 4] = [0, 0x9, 0xC1, 0x8E];
const TYPE_PAGE_BYTES: [u8; 4] = [0, 0xCC, 0x6B, 0x3F];

/// 生成 session_page 需要的数据
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionPageModify {
    /// 修改之前最后的page_number
    pub last_page_num: u32,
    /// 新生成的最后的page_num
    pub new_latest_page_num: u32,
    /// 新生成的index表的 page_num
    pub index_page_num: u32,
    /// 新生成的 global_page 的 page_num
    pub global_page_num: GlobalPage,
    /// 新生成的claim表的 page_num
    pub claim_page_num: u32,
    /// 用户名
    pub user_name: String,
    /// 提交说明
    pub commit_comment: String,
}

impl SessionPageModify {
    /// 通过旧的session_page数据 ，生成新的session_page
    ///
    /// increment_page_no : 新生成数据的page的数量
    pub fn convert_session_page(self, input: &[u8]) -> Vec<u8> {
        let mut new_session_page = vec![0u8, 0, 0, 3];
        let mut last_session_page = get_latest_session_page(input, self.last_page_num);
        // 获取修改之前最新的 session_page 的 page_number
        // let last_page_num = parse_to_u32(&last_session_page[20..24]);
        new_session_page.append(&mut self.last_page_num.to_be_bytes()[..4].to_vec());
        // 获取修改之前最新的session_no
        let session_no = parse_to_u32(&last_session_page[12..16]);
        let mut new_session_no = [vec![0u8, 0, 0, 1], (session_no + 1).to_be_bytes()[..4].to_vec()].concat();
        new_session_page.append(&mut new_session_no);
        // 当前的 session_page_num
        let mut new_session_page_num = [vec![0xFF; 4], (self.new_latest_page_num + 1).to_be_bytes()[..4].to_vec()].concat();
        new_session_page.append(&mut new_session_page_num);
        // 获取新生成的 index_page_num
        // let index_page_num = get_page_no(input, IndexPage).unwrap_or(parse_to_u32(&last_session_page[28..32]));
        let mut new_index_page = [vec![0u8, 0, 0, 1], self.index_page_num.to_be_bytes()[..4].to_vec()].concat();
        new_session_page.append(&mut new_index_page);
        // 获取新生成的 claim_page_num
        // let claim_page_num = get_page_no(input, ClaimPage).unwrap_or(parse_to_u32(&last_session_page[36..40]));
        let mut new_index_page = [vec![0u8, 0, 0, 1], self.claim_page_num.to_be_bytes()[..4].to_vec()].concat();
        new_session_page.append(&mut new_index_page);
        // 两个 0 0 0 1 固定值
        new_session_page.append(&mut vec![0, 0, 0, 1, 0, 0, 0, 1]);
        // 时间
        let mut time = convert_time_data();
        new_session_page.append(&mut time);
        // 52个 0
        new_session_page.append(&mut vec![0; 52]);
        // 用户名
        let mut data = convert_user_name_bytes(self.user_name);
        new_session_page.append(&mut data);
        // 提交描述
        let mut comment = convert_commit_comment_bytes(self.commit_comment);
        let comment_len = comment.len();
        new_session_page.append(&mut comment);
        new_session_page.append(&mut vec![0; 0x270 - comment_len + 12]); // 提交描述开头到 global_page 的 page_num 开头有0x270个0 ,+12是 global_page_num 开头有 3 * 4个 byte 0
        // global_page_num
        let global_page_num_len = parse_to_u32(&last_session_page[0x31C..0x320]) as usize * 12;
        let mut global_page_data = last_session_page[0x31C..0x31C + global_page_num_len].to_vec();
        match self.global_page_num {
            GlobalPage::NamePage(name_page_num) => {
                if let Some(position) = find_iter(&global_page_data, &NAME_PAGE_BYTES).next() {
                    global_page_data.splice(position + 4..position + 8, name_page_num.to_be_bytes());
                }
            }
            _ => {}
        }
        new_session_page.append(&mut global_page_data);
        // 其余数据都使用上一个session_page的旧数据
        last_session_page.splice(..new_session_page.len(), new_session_page);
        last_session_page.splice(last_session_page.len() - 16..last_session_page.len() - 12, session_no.to_be_bytes().to_vec());
        last_session_page
    }
}

/// 获得修改之前，最新的session_page的0x800的数据
pub fn get_latest_session_page(input: &[u8], last_page_no: u32) -> Vec<u8> {
    let last_page_position = (last_page_no * 0x800) as usize;
    input[last_page_position..last_page_position + 0x800].to_vec()
}

/// 生成pdms session_page中第 0x20 开始 ，长度为 0x14 的时间数据
fn convert_time_data() -> Vec<u8> {
    let mut result = vec![0, 0, 0, 4];

    let local_time: DateTime<Local> = Local::now();
    let year = local_time.year();
    let month = local_time.month();
    let m_day = local_time.day();
    let hour = local_time.hour();
    let min = local_time.minute();
    let seconds = local_time.second();

    result.append(&mut year.to_be_bytes()[..4].to_vec());
    result.append(&mut (month + 1).to_be_bytes()[..4].to_vec());
    result.append(&mut (hour + 24 * m_day).to_be_bytes()[..4].to_vec());
    result.append(&mut (seconds + 60 * min).to_be_bytes()[..4].to_vec());

    result
}

fn convert_user_name_bytes(name: String) -> Vec<u8> {
    let mut bytes = vec![0u8; 40];
    let data = name.into_bytes();
    let len = if data.len() % 4 == 0 { data.len() / 4 } else { data.len() / 4 + 1 } as u32;
    let mut data = [len.to_be_bytes()[..4].to_vec(), data].concat();
    // 40为最大长度(加上最前面的4个长度)，超过则截断
    if data.len() > 40 { data = data[..40].to_vec() }
    bytes.splice(..data.len(), data);
    bytes
}

fn convert_commit_comment_bytes(comment: String) -> Vec<u8> {
    let mut data = comment.into_bytes();
    if data.len() > 0x270 { data = data[..0x270].to_vec() } // 0x270应该是 commit_comment 的最大长度
    let len = if data.len() % 4 == 0 { data.len() / 4 } else { data.len() / 4 + 1 } as u32;
    [len.to_be_bytes()[..4].to_vec(), data].concat()
}

#[test]
fn test_convert_session_page() {
    let mut file = fs::File::open("resource/sam7200_0001").unwrap();
    let mut input = vec![];
    file.read_to_end(&mut input).unwrap();
    let session_page = SessionPageModify {
        last_page_num: 0xF23,
        new_latest_page_num: 5,
        index_page_num: 0xF16,
        global_page_num: GlobalPage::NamePage(0x1205),
        claim_page_num: 0xF24,
        user_name: "JBpeople".to_string(),
        commit_comment: "Default session comment".to_string(),
    };
    let data = SessionPageModify::convert_session_page(session_page, &input);
    let mut file = fs::File::create("resource/sam7200_0001_test_session").unwrap();
    file.write_all(&data).unwrap();
}

#[test]
fn test_convert_time_data() {
    let result = convert_time_data();
    println!("{:#4X?}", result);
}

#[test]
fn test_get_last_session_page() {
    let mut file = fs::File::open("resource/sam7200_0001").unwrap();
    let mut input = vec![];
    file.read_to_end(&mut input).unwrap();
    let page_no = get_last_page_no(&input);
    let data = get_latest_session_page(&input, page_no);
    let mut file = fs::File::create("resource/sam7200_0001_test").unwrap();
    file.write_all(&data).unwrap();
}

#[test]
fn test_convert_user_name_bytes() {
    let bytes = convert_user_name_bytes("WMY".to_string());
    println!("{:#4X?}", bytes);
}