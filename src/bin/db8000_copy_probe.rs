//! Ad-hoc probe: does the post-Save-Work paged index see the elements that the
//! session log reports as Add for the copied EQUI window?

use aios_core::pdms_types::RefU64;
use pdms_io::io::{EleOperationDetail, PdmsIO};
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let path = Path::new(r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001");

    let mut io = PdmsIO::new("", path.to_path_buf(), true);
    io.open().map_err(|e| anyhow::anyhow!("open failed: {e}"))?;
    let range_eles = io.collect_increment_eles(Some(20..=22))?;

    let mut adds = Vec::new();
    for (sesno, ops) in &range_eles {
        println!("SESSION {sesno} ops={}", ops.len());
        for op in ops {
            let kind = match &op.detail {
                EleOperationDetail::Add(_) => "Add",
                EleOperationDetail::Modified(_) => "Modified",
                EleOperationDetail::Deleted => "Deleted",
                EleOperationDetail::None => "None",
            };
            println!("  {kind:8} {}", op.refno);
            if matches!(&op.detail, EleOperationDetail::Add(_)) {
                adds.push(op.refno);
            }
        }
    }

    let mut paged = parse_pdms_db::paged::PagedDbSession::open(path)?;
    println!("SNAPSHOT {:?}", paged.snapshot());
    let expected_equi = RefU64::from_two_nums(24384, 24776);
    let expected_record = paged.read_raw_records(&[expected_equi])?;
    anyhow::ensure!(
        expected_record.contains_key(&expected_equi),
        "expected copied EQUI is absent from the final paged snapshot: {expected_equi}"
    );
    println!(
        "EXPECTED EQUI {} final_paged_record_bytes={}",
        expected_equi,
        expected_record[&expected_equi].len()
    );
    let found = paged.read_raw_records(&adds)?;
    let (live, dead): (Vec<RefU64>, Vec<RefU64>) =
        adds.iter().partition(|r| found.contains_key(*r));
    println!(
        "ADDS total={} live={} dead={}",
        adds.len(),
        live.len(),
        dead.len()
    );
    println!("LIVE {live:?}");
    println!("DEAD {dead:?}");

    // Sweep the tail of the ref0=24384 space to see what the final index really holds.
    let sweep = (24_700u32..=26_400)
        .map(|lo| RefU64::from_two_nums(24384, lo))
        .collect::<Vec<_>>();
    let mut present = paged
        .read_raw_records(&sweep)?
        .into_keys()
        .map(|r| r.get_1())
        .collect::<Vec<_>>();
    present.sort_unstable();
    println!("SWEEP 24384/24700..26400 present={} ", present.len());
    println!(
        "SWEEP tail={:?}",
        &present[present.len().saturating_sub(80)..]
    );

    // Cross-check with the legacy whole-file index: if the legacy scan sees the
    // adds that the paged index root misses, the paged snapshot is the liar.
    let legacy = parse_pdms_db::parse::parse_file_db_index_data(&path.to_path_buf())?;
    if let Some(entry) = legacy.refno_table_map.get(&expected_equi) {
        let owner = RefU64::from(&legacy.bytes[entry.pos + 12..entry.pos + 20]);
        println!("EXPECTED EQUI {} owner={}", expected_equi, owner);
    }
    let legacy_hit = adds
        .iter()
        .filter(|r| legacy.refno_table_map.contains_key(*r))
        .count();
    println!(
        "LEGACY ses_pgno={} total_refnos={} adds_present={}/{}",
        legacy.ses_pgno,
        legacy.refno_table_map.len(),
        legacy_hit,
        adds.len()
    );
    for refno in adds
        .iter()
        .filter(|refno| legacy.refno_table_map.contains_key(*refno))
    {
        let entry = legacy.refno_table_map.get(refno).unwrap();
        let owner = RefU64::from(&legacy.bytes[entry.pos + 12..entry.pos + 20]);
        println!(
            "PHYSICAL LIVE ADD refno={} is_equi={} noun_hash={} owner={} pos={}",
            refno,
            entry.noun_hash == aios_core::tool::db_tool::db1_hash("EQUI") as i32,
            entry.noun_hash,
            owner,
            entry.pos
        );
    }
    let mut legacy_tail = legacy
        .refno_table_map
        .iter()
        .map(|e| *e.key())
        .filter(|r| r.get_0() == 24384 && r.get_1() >= 25600)
        .map(|r| r.get_1())
        .collect::<Vec<_>>();
    legacy_tail.sort_unstable();
    println!("LEGACY tail(>=25600)={legacy_tail:?}");

    Ok(())
}
