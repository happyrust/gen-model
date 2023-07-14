use std::fs::File;
use std::io::Write;

/// 生成csv文件
pub fn create_csv_file(header: Vec<String>, values: Vec<Vec<String>>) -> Vec<u8> {
    let mut data = Vec::new();
    // 写入表头
    let header_str = header.join(",");
    data.push(format!("{}\r\n",header_str).as_bytes().to_vec());
    // 写入数据
    for value in values {
        let value_str = value.join(",");
        data.push(format!("{}\r\n",value_str).as_bytes().to_vec());
    }
    data.into_iter().flatten().collect()
}

#[test]
fn test_create_csv_file() {
    let header = vec!["关键词".to_string(),"命中目标".to_string()];
    let values = vec![vec!["RVV".to_string(),"1RCV0001/B1".to_string()]];
    let data = create_csv_file(header,values);
    let mut file = File::create("test.csv").unwrap();
    file.write_all(&data).unwrap();
}