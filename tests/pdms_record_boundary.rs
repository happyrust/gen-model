use aios_core::tool::db_tool::db1_hash;
use parse_pdms_db::parse::{parse_ele_children, parse_ele_membs, parse_raw_ele_data};

fn terminally_padded_record() -> [u8; 168] {
    let mut input = [0_u8; 168];
    input[0..4].copy_from_slice(&40_i32.to_be_bytes());
    input[12..16].copy_from_slice(&(db1_hash("HANG") as i32).to_be_bytes());
    input
}

#[test]
fn exact_record_slice_does_not_scan_into_the_next_record() {
    let input = terminally_padded_record();

    assert!(std::panic::catch_unwind(|| parse_raw_ele_data(&input)).is_ok());
    assert!(parse_ele_membs(&input).is_empty());
    let (_, children) = parse_ele_children(&input);
    assert!(children.0.is_empty());
}
