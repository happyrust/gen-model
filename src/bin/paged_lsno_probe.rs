use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let path = std::env::var_os("PAGED_LSNO_PROBE_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(r"D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000\acp7002_0001")
        });
    let target = std::env::args()
        .nth(1)
        .as_deref()
        .unwrap_or("15194_5825")
        .into();
    let full =
        parse_pdms_db::parse::parse_file_db_basic_data(&path, "acp7002_0001", "AvevaCatalogue")?;
    let entry = full.refno_table_map.get(&target).unwrap();
    println!(
        "FULL_POS={} PAGE={} OFF={}",
        entry.pos,
        entry.pos / 2048,
        entry.pos % 2048
    );
    let mut session = parse_pdms_db::paged::PagedDbSession::open(&path)?;
    let raw = session
        .read_raw_records(&[target])?
        .remove(&target)
        .unwrap();
    println!(
        "PAGED_RAW_LEN={} tail={:02x?}",
        raw.len(),
        &raw[raw.len().saturating_sub(96)..]
    );
    let source = std::fs::read(&path)?;
    let search_start = entry.pos.saturating_sub(64);
    let search_end = (entry.pos + 64).min(source.len());
    let raw_prefix_len = raw.len().min(32);
    let absolute_start = source[search_start..search_end]
        .windows(raw_prefix_len)
        .position(|window| window == &raw[..raw_prefix_len])
        .map(|relative| search_start + relative)
        .expect("paged record prefix must exist near the full-parser position");
    let boundary_start = absolute_start + raw.len().saturating_sub(32);
    let boundary_end = (absolute_start + raw.len() + 160).min(source.len());
    println!(
        "PAGED_ABSOLUTE_START={absolute_start} FULL_ENTRY_DELTA={}",
        absolute_start as isize - entry.pos as isize
    );
    println!(
        "SOURCE_AROUND_PAGED_END={:02x?}",
        &source[boundary_start..boundary_end]
    );
    let mut parsed = pollster::block_on(
        session.parse_elements_with_info(&[target], &aios_core::get_default_pdms_db_info()),
    )?;
    let e = parsed.remove(&target).unwrap();
    println!("PAGED_IMPLICIT={:#?}", e.whole_attmap.attmap);
    println!("PAGED_EXPLICIT={:#?}", e.whole_attmap.explicit_attmap);
    let merged = e.whole_attmap.merge();
    println!("PAGED_MERGED={merged:#?}");
    println!("PAGED_SUR_JSON={}", merged.gen_sur_json().unwrap());
    Ok(())
}
