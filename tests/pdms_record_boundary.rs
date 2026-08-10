use aios_core::tool::db_tool::db1_hash;
use parse_pdms_db::parse::{parse_ele_children, parse_ele_membs, parse_raw_ele_data};

fn terminal_padding_record(first: i32, second: i32) -> [u8; 168] {
    // Production failure shape: the declared implicit region ends at byte 160
    // and the remaining two words are terminal zero padding. The old parser
    // advanced to 168 and then sliced 168..172 while looking for a member block.
    let mut input = [0_u8; 168];
    input[0..4].copy_from_slice(&40_i32.to_be_bytes());
    input[12..16].copy_from_slice(&(db1_hash("HANG") as i32).to_be_bytes());
    input[160..164].copy_from_slice(&first.to_be_bytes());
    input[164..168].copy_from_slice(&second.to_be_bytes());
    input
}

fn assert_raw_parser_stops_at_record_end(input: &[u8; 168]) {
    let outcome = std::panic::catch_unwind(|| parse_raw_ele_data(input));
    assert!(
        outcome.is_ok(),
        "a record-bounded parser must return Err rather than read beyond byte 168"
    );
}

#[test]
fn paged_record_zero_padding_returns_instead_of_panicking() {
    assert_raw_parser_stops_at_record_end(&terminal_padding_record(0, 0));
}

#[test]
fn paged_record_continuation_padding_returns_instead_of_panicking() {
    assert_raw_parser_stops_at_record_end(&terminal_padding_record(7, 7));
}

#[test]
fn paged_record_mixed_terminal_padding_returns_instead_of_panicking() {
    assert_raw_parser_stops_at_record_end(&terminal_padding_record(0, 7));
    assert_raw_parser_stops_at_record_end(&terminal_padding_record(7, 0));
}

#[test]
fn member_and_child_parsers_stop_at_every_terminal_padding_shape() {
    for (first, second) in [(0, 0), (7, 7), (0, 7), (7, 0)] {
        let input = terminal_padding_record(first, second);
        assert!(parse_ele_membs(&input).is_empty());
        let (_, children) = parse_ele_children(&input);
        assert!(children.0.is_empty());
    }
}
