

use regex::Regex;

#[test]
fn test_regex() {
    let input = r#"
        "id": "25688_32682",
        "refno": "25688_32682",
        "owner": "25688_32681",
        "name": "/1AR07WW0002R",
        "noun": "TEST",
        "dbnum": 1112,
        "sesno": 0,
        "cata_hash": "110329119932332",
        "lock": false
    "#;

    let re = Regex::new(r#""owner":\s*"(\d+_\d+)""#).unwrap();
    let replaced_owner = re.replace_all(input, r#""owner": pe:$1"#);

    let re_refno = Regex::new(r#""refno":\s*"(\d+_\d+)""#).unwrap();
    let replaced_refno = re_refno.replace_all(&replaced_owner, r#""refno": TEST:$1"#);

    println!("{}", replaced_refno);
}