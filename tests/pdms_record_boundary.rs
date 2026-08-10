use aios_core::tool::db_tool::db1_hash;
use parse_pdms_db::parse::{parse_ele_children, parse_ele_membs, parse_raw_ele_data};

fn terminal_padding_record() -> [u8; 168] {
    // Production failure shape: the declared implicit region ends at byte 160
    // and the remaining two words are terminal zero padding. The old parser
    // advanced to 168 and then sliced 168..172 while looking for a member block.
    let mut input = [0_u8; 168];
    input[0..4].copy_from_slice(&40_i32.to_be_bytes());
    input[12..16].copy_from_slice(&(db1_hash("HANG") as i32).to_be_bytes());
    input
}

#[test]
fn paged_record_terminal_padding_returns_instead_of_panicking() {
    let input = terminal_padding_record();
    let outcome = std::panic::catch_unwind(|| parse_raw_ele_data(&input));
    assert!(
        outcome.is_ok(),
        "a record-bounded parser must return Err rather than read beyond byte 168"
    );
}

#[test]
fn member_only_parser_stops_at_record_end() {
    assert!(parse_ele_membs(&terminal_padding_record()).is_empty());
}

#[test]
fn child_only_parser_stops_at_record_end() {
    let (_, children) = parse_ele_children(&terminal_padding_record());
    assert!(children.0.is_empty());
}
