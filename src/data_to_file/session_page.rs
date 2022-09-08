use std::mem::take;
use chrono::{Datelike, DateTime, Local, Timelike};
use itertools::Itertools;

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

#[test]
fn test_convert_time_data() {
    let result = convert_time_data();
    println!("{:#4X?}", result);
}