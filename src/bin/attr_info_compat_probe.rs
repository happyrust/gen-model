//! Read-only gate for a generated `PdmsDatabaseInfo` compatibility file.
//!
//! It first proves that every legacy schema entry is preserved, then parses the
//! latest records in one Dabacon file with both schemas and compares every
//! attribute value produced on their common domain.
//!
//! Both schemas are read from files rather than taken from `aios_core`, because
//! the embedded copy is itself the thing being replaced and cannot serve as the
//! baseline once it has been swapped. `parse_raw_ele_data_with_info` takes the
//! schema by reference, but neighbouring code can still reach for the global
//! one; the embedded arm is parsed alongside so that leak would show up as a
//! third set of values rather than silently flattening the comparison.

use std::collections::BTreeMap;

use aios_core::get_default_pdms_db_info;
use aios_core::types::db_info::PdmsDatabaseInfo;
use parse_pdms_db::parse::EleData;
use serde_json::json;

struct Args {
    legacy: Option<String>,
    generated: String,
    db: String,
    project: String,
    limit: usize,
    examples: usize,
}

fn parse_args() -> anyhow::Result<Args> {
    const USAGE: &str = "usage: attr_info_compat_probe GENERATED_JSON DB_FILE [PROJECT] \
                         [--legacy LEGACY_JSON] [--limit N] [--examples N]";
    let mut positional = Vec::new();
    let mut legacy = None;
    let mut limit = 0usize;
    let mut examples = 12usize;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--legacy" => legacy = Some(args.next().ok_or_else(|| anyhow::anyhow!(USAGE))?),
            "--limit" => limit = args.next().ok_or_else(|| anyhow::anyhow!(USAGE))?.parse()?,
            "--examples" => {
                examples = args.next().ok_or_else(|| anyhow::anyhow!(USAGE))?.parse()?
            }
            "-h" | "--help" => anyhow::bail!(USAGE),
            _ => positional.push(arg),
        }
    }
    let mut positional = positional.into_iter();
    Ok(Args {
        legacy,
        generated: positional.next().ok_or_else(|| anyhow::anyhow!(USAGE))?,
        db: positional.next().ok_or_else(|| anyhow::anyhow!(USAGE))?,
        project: positional.next().unwrap_or_else(|| "ams".into()),
        limit,
        examples,
    })
}

fn load(path: &str) -> anyhow::Result<PdmsDatabaseInfo> {
    let mut info: PdmsDatabaseInfo = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    info.fill_named_map();
    Ok(info)
}

fn pair_count(info: &PdmsDatabaseInfo) -> usize {
    info.noun_attr_info_map
        .iter()
        .map(|noun| noun.value().len())
        .sum()
}

/// Every attribute value one parse produced, keyed so that the implicit and
/// explicit halves cannot collide.
fn flatten(ele: &EleData) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (name, value) in ele.whole_attmap.attmap.iter() {
        out.insert(format!("implicit/{name}"), format!("{value:?}"));
    }
    for (name, value) in ele.whole_attmap.explicit_attmap.iter() {
        out.insert(format!("explicit/{name}"), format!("{value:?}"));
    }
    out
}

#[derive(Default)]
struct Diff {
    common: usize,
    mismatched: usize,
    only_left: usize,
    only_right: usize,
    examples: Vec<serde_json::Value>,
}

impl Diff {
    fn record(&mut self, refno: String, noun: u32, left: &EleData, right: &EleData, cap: usize) {
        let (left, right) = (flatten(left), flatten(right));
        for (key, left_value) in left.iter() {
            match right.get(key) {
                None => self.only_left += 1,
                Some(right_value) => {
                    self.common += 1;
                    if left_value != right_value {
                        self.mismatched += 1;
                        if self.examples.len() < cap {
                            self.examples.push(json!({
                                "refno": refno,
                                "noun": noun,
                                "attribute": key,
                                "left": left_value,
                                "right": right_value,
                            }));
                        }
                    }
                }
            }
        }
        self.only_right += right.keys().filter(|key| !left.contains_key(*key)).count();
    }

    fn report(&self) -> serde_json::Value {
        json!({
            "common": self.common,
            "mismatched": self.mismatched,
            "only_left": self.only_left,
            "only_right": self.only_right,
            "examples": self.examples,
        })
    }
}

fn main() -> anyhow::Result<()> {
    let args = parse_args()?;

    let generated = load(&args.generated)?;
    let embedded = get_default_pdms_db_info();
    let owned_legacy = args.legacy.as_deref().map(load).transpose()?;
    let legacy: &PdmsDatabaseInfo = owned_legacy.as_ref().unwrap_or(embedded);

    // Schema domain: is every legacy (noun, attribute) still described, and
    // described the same way?
    let mut legacy_pairs = 0usize;
    let mut schema_missing = 0usize;
    let mut schema_different = 0usize;
    let mut schema_examples = Vec::new();
    for noun in legacy.noun_attr_info_map.iter() {
        for attr in noun.value().iter() {
            legacy_pairs += 1;
            let found = generated
                .noun_attr_info_map
                .get(noun.key())
                .and_then(|attrs| attrs.get(attr.key()).map(|value| value.clone()));
            match found {
                None => {
                    schema_missing += 1;
                    if schema_examples.len() < args.examples {
                        schema_examples.push(json!({
                            "kind": "missing",
                            "noun": *noun.key(),
                            "attribute": attr.value().name,
                        }));
                    }
                }
                Some(value) if format!("{value:?}") != format!("{:?}", attr.value()) => {
                    schema_different += 1;
                    if schema_examples.len() < args.examples {
                        schema_examples.push(json!({
                            "kind": "different",
                            "noun": *noun.key(),
                            "attribute": attr.value().name,
                            "legacy": format!("{:?}", attr.value()),
                            "generated": format!("{value:?}"),
                        }));
                    }
                }
                Some(_) => {}
            }
        }
    }

    let path = std::path::PathBuf::from(&args.db);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let db = parse_pdms_db::parse::parse_file_db_basic_data(&path, file_name, &args.project)?;
    let (records, _) = parse_pdms_db::parse::gen_ref_type_pos_table(&db.bytes);

    let mut outcomes: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut legacy_vs_generated = Diff::default();
    let mut embedded_vs_legacy = Diff::default();
    let mut embedded_vs_generated = Diff::default();
    let mut compared_records = 0usize;
    for record in records.iter() {
        if args.limit > 0 && compared_records >= args.limit {
            break;
        }
        if record.value().pos < 4 {
            continue;
        }
        let bytes = &db.bytes[record.value().pos - 4..];
        let old = parse_pdms_db::parse::parse_raw_ele_data_with_info(bytes, legacy);
        let new = parse_pdms_db::parse::parse_raw_ele_data_with_info(bytes, &generated);
        *outcomes
            .entry(match (&old, &new) {
                (Ok(_), Ok(_)) => "both_ok",
                (Ok(_), Err(_)) => "legacy_only",
                (Err(_), Ok(_)) => "generated_only",
                (Err(_), Err(_)) => "neither",
            })
            .or_default() += 1;
        let (Ok(old), Ok(new)) = (old, new) else {
            continue;
        };
        compared_records += 1;
        let refno = format!("{:?}", record.key());
        legacy_vs_generated.record(refno.clone(), old.noun, &old, &new, args.examples);
        if let Ok(base) = parse_pdms_db::parse::parse_raw_ele_data_with_info(bytes, embedded) {
            embedded_vs_legacy.record(refno.clone(), old.noun, &base, &old, args.examples);
            embedded_vs_generated.record(refno, old.noun, &base, &new, args.examples);
        }
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "legacy_source": args.legacy.as_deref().unwrap_or("<embedded in aios_core>"),
            "generated_source": args.generated,
            "db_path": args.db,
            "schema": {
                "embedded_pairs": pair_count(embedded),
                "legacy_pairs": legacy_pairs,
                "generated_pairs": pair_count(&generated),
                "added_pairs": pair_count(&generated).saturating_sub(legacy_pairs),
                "missing": schema_missing,
                "different": schema_different,
                "examples": schema_examples,
            },
            "records": {
                "latest": records.len(),
                "compared": compared_records,
                "parse_outcomes": outcomes,
            },
            "values": {
                "legacy_vs_generated": legacy_vs_generated.report(),
                "embedded_vs_legacy": embedded_vs_legacy.report(),
                "embedded_vs_generated": embedded_vs_generated.report(),
            },
        }))?
    );
    Ok(())
}
