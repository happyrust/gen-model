//! Full attribute views, checked against something outside this crate.
//!
//! Two elements, because a UDA reaches an element two different ways and one
//! element only ever exercises one of them:
//!
//! - PIPE `=24383/73958` (`/1RCV0102A`) stores **no** UDA bytes, so all twenty
//!   of its `:` lines come from Dictionary defaults. Checked against the `q att`
//!   listing E3D printed, reproduced verbatim below.
//! - BRAN `=24383/85432` (`/C-CO-5RX122-C`) **does** store one, so it covers
//!   resolving a stored `UKEY` back to its name. Checked against the raw record
//!   bytes, which is the stronger authority for "is this value really there".
//!
//! Agreeing with the other decoder in the tree would prove nothing — both read
//! the same file. That caution earned its keep: on the BRAN the two decoders
//! disagree, and the bytes settle it (see the BRAN test).
//!
//! The UDA snapshot spans the six Dictionary databases MDB `/ALL` declares,
//! across three projects — which dictionaries apply is a property of the MDB,
//! not of the database an element lives in. A snapshot of
//! `AvevaMarineSample` alone is missing five of BRAN's UDAs: `:SCHrefHole`
//! comes from `SCB`, and `:PFILoose` / `:PFILExcess` / `:PFIAExcess` /
//! `:PFConsChk` from `AvevaCatalogue`.
//!
//! Take the list from the MDB rather than from the directory listing. Both
//! mistakes are otherwise silent — the first time this fixture was built by
//! hand it both missed two dictionaries `/ALL` declares and included one it
//! does not:
//!
//! ```text
//! cargo run --release --bin mdb_dict_probe -- --project AvevaMarineSample --mdb /ALL
//! ```
//!
//! It prints a ready `--dictionary-db-list`, in `CURD` order — which matters,
//! because the first definition of a `UKEY` wins.
//!
//! One caveat that probe will show: `SCB` is not in `included_projects`, so
//! `scb6002_0001` comes back as not on disk even though `/ALL` declares it and
//! `q att` prints its `:SCHrefHole`. This fixture was built with that file
//! included; a runtime built from the configured project list alone would be
//! missing it.
//!
//! Three sources have to combine before the listing can be reproduced, and
//! each one covers a part the others cannot:
//!
//! | source | covers | of the 79 lines |
//! |---|---|---|
//! | the record itself | attributes that store bytes | 30 |
//! | `all_attr_info.json` | schema defaults for the rest | 29 |
//! | `all_uda_info.json` | Dictionary UDA defaults | 20 |
//!
//! The element stores **no UDA bytes at all** — every `:` line here is a
//! Dictionary default. That is why resolving stored UDA values (the SurrealDB
//! path) would still produce an empty UDA view for it.
//!
//! The UDA snapshot is a **test fixture**, not configuration: UDA definitions
//! belong to one project, and the runtime keeps resolving them from that
//! project's Dictionary database through the `UDA` / `ATT_UDA` tables as
//! before. Nothing here changes that path; the snapshot exists so this test
//! has an authority to compare against without a live database.
//!
//! Fixture-dependent: skips loudly when the project database is not on this
//! machine. Refresh the snapshot with
//! `e3d-descriptor emit-uda-table --dictionary-db-list ams5100_0001;ams5101_0001`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use aios_core::RefU64;
use aios_core::types::db_info::PdmsDatabaseInfo;

const DB: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7999_0001";
const PROJECT: &str = "AvevaMarineSample";
const ATTR_INFO: &str = "all_attr_info.json";
const UDA_INFO: &str = "tests/fixtures/AvevaMarineSample_uda_info.json";
/// `=24383/73958`, as `q ref` prints it.
const REFNO: RefU64 = RefU64((24383u64 << 32) | 73958);
/// A BRAN that stores a UDA value instead of inheriting the default.
const BRAN_REFNO: RefU64 = RefU64((24383u64 << 32) | 85432);
/// `:ROOM_NO`. The record carries `2652ab51 28000003 00000005 "NB122"` at byte
/// 556 — attribute hash, then type and word count, then the text.
const ROOM_NO_UKEY: i32 = 642_952_017;

/// Exactly what `q att` printed, in its own order, minus five lines asserted
/// on their own below: `Name` / `Type` / `Owner` are element identity rather
/// than schema attributes, and `Pspec` / `Ispec` print the name of an element
/// in another database, which needs a cross-database lookup this test does not
/// do — their references are checked instead.
///
/// `Lock` is here on purpose. It has no descriptor, so it exists only because
/// the regenerated table kept the 1023 legacy-only pairs; the 2026-08-26
/// artifact dropped them and would fail this line.
const Q_ATT: &str = "\
Lock false
Description unset
Function unset
Purpose unset
Built false
Shop false
Isohidden false
Bore 0
Temperature -100000
Pressure 0
TPressure 0
Tspec Nulref
Matref Nulref
Fluref Nulref
Casref Nulref
Ccentre 0
Cclass 0
Linetype unset
Erection 0
Rev -1
Safclass 0
Duty unset
Dscode unset
Ptspec unset
Inschedule unset
Skey unset
Lissue false
Wmaximum 0
Pmaximum 0
Smaximum 0
Jmaximum 0
Drrf Nulref
Cdrg unset
Cnumber unset
Carea unset
Splprefix unset
BendMacReference Nulref
Rlstored unset
Deldsg FALS
Farea unset
Fpline unset
Fdrawing unset
Frevision unset
Frdrawing unset
Fstatus unset
PLANU unset
JNTC 0
WLDC 0
HEATT 0
Mdsysf unset
Inprtref Nulref
Ouprtref Nulref
Sloreference Nulref
NoHuMark false";

/// `q att` prints report names; the tables are keyed by the four-to-six
/// character storage name. Only the rows where the two differ are listed.
const REPORT_TO_STORAGE: &[(&str, &str)] = &[
    ("Lock", "LOCK"),
    ("Description", "DESC"),
    ("Function", "FUNC"),
    ("Purpose", "PURP"),
    ("Built", "BUIL"),
    ("Shop", "SHOP"),
    ("Isohidden", "ISOH"),
    ("Bore", "BORE"),
    ("Temperature", "TEMP"),
    ("Pressure", "PRES"),
    ("TPressure", "TPRESS"),
    ("Tspec", "TSPE"),
    ("Matref", "MATR"),
    ("Fluref", "FLUR"),
    ("Casref", "CASR"),
    ("Ccentre", "CCEN"),
    ("Cclass", "CCLA"),
    ("Linetype", "LNTP"),
    ("Erection", "EREC"),
    ("Rev", "REV"),
    ("Safclass", "SAFC"),
    ("Duty", "DUTY"),
    ("Dscode", "DSCO"),
    ("Ptspec", "PTSP"),
    ("Inschedule", "INSC"),
    ("Skey", "SKEY"),
    ("Lissue", "LISS"),
    ("Wmaximum", "WMAX"),
    ("Pmaximum", "PMAX"),
    ("Smaximum", "SMAX"),
    ("Jmaximum", "JMAX"),
    ("Drrf", "DRRF"),
    ("Cdrg", "CDRG"),
    ("Cnumber", "CNUM"),
    ("Carea", "CARE"),
    ("Splprefix", "SPLP"),
    ("BendMacReference", "BENDMA"),
    ("Rlstored", "RLST"),
    ("Deldsg", "DELDSG"),
    ("Farea", "FAREA"),
    ("Fpline", "FPLINE"),
    ("Fdrawing", "FDRA"),
    ("Frevision", "FREV"),
    ("Frdrawing", "FRDR"),
    ("Fstatus", "FSTAT"),
    ("Mdsysf", "MDSYSF"),
    ("Inprtref", "INPRTR"),
    ("Ouprtref", "OUPRTR"),
    ("Sloreference", "SLOREF"),
    ("NoHuMark", "NOHUMA"),
];

/// UDAs the snapshot says apply to these nouns but `q att` never prints.
///
/// They are not a dictionary that is absent from the MDB — `/ALL` declares
/// every dictionary they come from, and `:PFILoose` out of the very same
/// `acp7006_0001` *is* printed. Nothing this table exports separates them
/// either: `UPSEUD` is false on all five, names are static for all five,
/// types run REF / TEXT / LOG, and `:MDSComment` applies to 46 nouns where the
/// printed `:PFILoose` applies to 30.
///
/// What they do share is an owning application — `PSI` stress and `MDS`
/// multi-discipline supports — and E3D was in Design when this `q att` ran.
/// A module scope would explain it, but nothing in the Dictionary element as
/// currently decoded says so, and a guess in this list would be worse than an
/// open question.
///
/// Asserted by name and by count, so a sixth appearing, or one of these
/// starting to print, fails rather than drifts.
const UDAS_E3D_DOES_NOT_PRINT: &[&str] = &[
    ":MDSComment",
    ":MDSDType",
    ":MDSTrun",
    ":PsiDate",
    ":PsiSystem",
];

/// `q att` prints one line per definition, so a name with two definitions
/// appears twice. The view is keyed by name and therefore holds one. Checked
/// rather than ignored: two definitions sharing a name but not a default would
/// make the view depend on iteration order.
fn assert_uda_view(
    view: &aios_core::types::named_attmap::NamedAttrMap,
    uda: &aios_database::uda_table::UdaTable,
    noun: u32,
    expected: &[(&str, &str)],
) {
    let mut wrong = Vec::new();
    for (name, want) in expected {
        let got = view.get_as_string(name).unwrap_or_default();
        let got = if got.is_empty() {
            "unset".to_owned()
        } else {
            got
        };
        if got != *want {
            wrong.push(format!("{name}: q att={want} view={got}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "UDA values disagree with q att: {wrong:#?}"
    );

    let shown: std::collections::BTreeSet<&str> = view
        .map
        .keys()
        .filter(|k| k.starts_with(':'))
        .map(String::as_str)
        .collect();
    let printed: std::collections::BTreeSet<&str> =
        expected.iter().map(|(name, _)| *name).collect();
    let extra: Vec<&str> = shown.difference(&printed).copied().collect();
    let mut expected_extra: Vec<&str> = UDAS_E3D_DOES_NOT_PRINT
        .iter()
        .copied()
        .filter(|name| shown.contains(name))
        .collect();
    expected_extra.sort_unstable();
    assert_eq!(extra, expected_extra, "unexpected UDA in the view");
    assert!(
        printed.difference(&shown).next().is_none(),
        "the view is missing UDAs q att printed: {:?}",
        printed.difference(&shown).collect::<Vec<_>>()
    );
    assert!(
        !view.map.keys().any(|key| key.starts_with("UDA:")),
        "an unresolved UKEY means the snapshot and the record disagree"
    );

    // Every definition applying to this noun is either printed by q att or
    // named above; nothing is quietly folded away by the name-keyed view.
    let mut by_name: std::collections::BTreeMap<&str, Vec<u32>> = Default::default();
    for key in uda.applicable(noun) {
        let definition = uda.by_key(*key).expect("applicable keys resolve");
        by_name
            .entry(definition.attr_name.as_str())
            .or_default()
            .push(*key);
    }
    for (name, keys) in by_name.iter().filter(|(_, keys)| keys.len() > 1) {
        let defaults: std::collections::BTreeSet<String> = keys
            .iter()
            .map(|key| format!("{:?}", uda.by_key(*key).unwrap().default_val))
            .collect();
        assert_eq!(
            defaults.len(),
            1,
            "{name} has {} definitions with differing defaults, so the view would \
             depend on iteration order: {keys:?}",
            keys.len()
        );
    }
    assert_eq!(by_name.len(), shown.len());
}

const Q_ATT_UDA: &[(&str, &str)] = &[
    (":3D_SJJD", "C"),
    (":3D_FAZT", "unset"),
    (":3D_SJZT", "Designing"),
    (":3D_WCZT", "0"),
    (":3D_THZT", "unset"),
    (":3D_SJRY", "unset"),
    (":3D_JDRY", "unset"),
    (":3D_SHRY", "unset"),
    (":3D_SDRY", "unset"),
    (":3D_PZRY", "unset"),
    (":3D_SJBG", "unset"),
    (":3D_GCBG", "unset"),
    (":3D_PZJC", "unset"),
    (":3D_GXZH", "unset"),
    (":3D_MXGH", "unset"),
    (":3D_FAMX", "unset"),
    (":3D_KKZT", "unset"),
    (":TPress", "unset"),
    (":G_STATUS", "DESIGN"),
    (":MechanicalReportNo", "unset"),
];

fn fixtures_present() -> bool {
    for path in [DB, ATTR_INFO, UDA_INFO] {
        if !std::path::Path::new(path).exists() {
            eprintln!(
                "SKIP pipe_full_attributes_real: {path} is absent (cwd {:?})",
                std::env::current_dir().unwrap_or_default()
            );
            return false;
        }
    }
    true
}

/// The schema is read from the file rather than from `aios_core`, because the
/// pinned revision embeds the 339-noun table and PIPE needs the 1878-noun one.
fn load_schema() -> PdmsDatabaseInfo {
    let mut info: PdmsDatabaseInfo =
        serde_json::from_str(&std::fs::read_to_string(ATTR_INFO).expect("read attr info"))
            .expect("deserialise attr info");
    info.fill_named_map();
    info
}

/// Storage name to declared `att_type`, for the PIPE noun only. The renderer
/// needs the type: `0` means zero for an INTEGER and unset for a WORD.
fn pipe_att_types() -> BTreeMap<String, String> {
    let schema = load_schema();
    let pipe = aios_core::tool::db_tool::db1_hash("PIPE") as i32;
    schema
        .noun_attr_info_map
        .get(&pipe)
        .map(|attrs| {
            attrs
                .iter()
                .map(|info| (info.name.clone(), format!("{:?}", info.att_type)))
                .collect()
        })
        .unwrap_or_default()
}

fn load_uda_snapshot() -> aios_database::uda_table::UdaTable {
    aios_database::uda_table::UdaTable::load(UDA_INFO).expect("read the project UDA snapshot")
}

fn parse_element_at(refno: RefU64) -> parse_pdms_db::parse::EleData {
    let schema = load_schema();
    let path = PathBuf::from(DB);
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap();
    let db = parse_pdms_db::parse::parse_file_db_basic_data(&path, file_name, PROJECT)
        .expect("open the design database");
    let entry = parse_pdms_db::refno_index::find_refno_entry(&db.bytes, refno)
        .unwrap_or_else(|| panic!("{refno:?} must be in the latest-session index"));
    parse_pdms_db::parse::parse_raw_ele_data_with_info(&db.bytes[entry.pos - 4..], &schema)
        .expect("decode the record")
}

fn parse_element() -> parse_pdms_db::parse::EleData {
    parse_element_at(REFNO)
}

/// `q att` renders a value by its type, and "no value" looks different per
/// type: an empty STRING and a WORD holding hash 0 both print as `unset`,
/// while a null reference prints as `Nulref`. Units (`0mm`, `-100000degC`) are
/// display only. Reduce both sides to a comparable core.
fn comparable(att_type: &str, value: &str) -> String {
    let value = value
        .trim()
        .trim_end_matches("mm")
        .trim_end_matches("degC")
        .trim_end_matches("pascal")
        .trim();
    match att_type {
        "ELEMENT" | "RefU64Vec" => {
            if value.is_empty()
                || value.eq_ignore_ascii_case("nulref")
                || value == "0_0"
                || value == "0/0"
            {
                return "Nulref".into();
            }
        }
        "WORD" => {
            if value.is_empty() || value == "0" {
                return "unset".into();
            }
        }
        _ => {
            if value.is_empty() {
                return "unset".into();
            }
        }
    }
    if value.eq_ignore_ascii_case("unset") {
        return "unset".into();
    }
    match value.parse::<f64>() {
        Ok(number) if number == number.trunc() => format!("{}", number as i64),
        Ok(number) => format!("{number}"),
        Err(_) => value.to_owned(),
    }
}

/// Two attributes whose declared `att_type` contradicts their descriptor, so
/// the empty case renders as the wrong kind of nothing:
///
/// | attribute | table says | descriptor says | q att | view |
/// |---|---|---|---|---|
/// | `SLOREF` | STRING | `stc 16`, 2 words — a reference | `Nulref` | `unset` |
/// | `MDSYSF` | ELEMENT | `stc 17`, 21 words — not a plain ref | `unset` | `Nulref` |
///
/// Both types are older than the regenerated table and were copied into it
/// verbatim by `--legacy-attr-info`. That is what "no regression" costs: the
/// rule that guarantees no value changes also guarantees no value is fixed.
/// Named here so the deviation stays visible instead of hiding inside a
/// loosened comparison — and the count is asserted, so fixing either type
/// fails this test until the entry is removed.
const KNOWN_TYPE_DEVIATIONS: &[&str] = &["Sloreference", "Mdsysf"];

#[test]
fn every_q_att_line_is_reproduced_offline() {
    if !fixtures_present() {
        return;
    }
    let element = parse_element();
    let view = aios_database::uda_table::full_attribute_view(
        &element,
        &load_schema(),
        &load_uda_snapshot(),
    );

    // `EleData::name` stays empty on this path; NAME arrives through the
    // explicit stream, which is where the view picks it up.
    assert_eq!(view.get_as_string("NAME").as_deref(), Some("/1RCV0102A"));
    assert_eq!(view.get_type_str(), "PIPE");
    assert_eq!(
        view.get_as_string("OWNER").as_deref(),
        Some("24383/73928"),
        "owner is /1RCV-PIPE-RX"
    );
    // `q att` prints `Pspec /VMC1` and `Ispec /I80-HL`; both live in database
    // 13245, so the names need a lookup outside this element. The references
    // themselves are what the record stores, and they are what is checked.
    assert_eq!(
        view.get_as_string("PSPE").as_deref(),
        Some("13245/854703"),
        "Pspec /VMC1"
    );
    assert_eq!(
        view.get_as_string("ISPE").as_deref(),
        Some("13245/917998"),
        "Ispec /I80-HL"
    );

    let att_types = pipe_att_types();
    let storage: BTreeMap<&str, &str> = REPORT_TO_STORAGE.iter().copied().collect();
    let mut absent = Vec::new();
    let mut wrong = Vec::new();
    let mut deviations = Vec::new();
    for line in Q_ATT.lines() {
        let (report, want) = line.split_once(' ').expect("every q att line has a value");
        let key = storage.get(report).copied().unwrap_or(report);
        let att_type = att_types.get(key).map(String::as_str).unwrap_or("");
        match view.get_as_string(key) {
            None => absent.push(format!("{report} ({key})")),
            Some(got) if comparable(att_type, &got) != comparable(att_type, want) => {
                let note = format!("{report} ({key}, {att_type}): q att={want} view={got}");
                if KNOWN_TYPE_DEVIATIONS.contains(&report) {
                    deviations.push(note);
                } else {
                    wrong.push(note);
                }
            }
            Some(_) => {}
        }
    }
    assert_eq!(
        deviations.len(),
        KNOWN_TYPE_DEVIATIONS.len(),
        "a known deviation stopped deviating; delete it from the list: {deviations:#?}"
    );

    assert!(
        absent.is_empty(),
        "{} of {} q att attributes are missing from the view: {absent:#?}",
        absent.len(),
        Q_ATT.lines().count()
    );
    assert!(wrong.is_empty(), "values disagree with q att: {wrong:#?}");
}

/// The element stores no UDA bytes, so all twenty come from the Dictionary
/// table. A regression here reads as "this project has no UDAs" rather than as
/// an error, which is exactly why it is asserted rather than logged.
#[test]
fn all_twenty_udas_come_back_with_their_dictionary_values() {
    if !fixtures_present() {
        return;
    }
    let element = parse_element();
    assert!(
        element.whole_attmap.uda_atts.is_empty(),
        "this fixture is only meaningful while the record itself stores no UDA"
    );

    let uda = load_uda_snapshot();
    let view = aios_database::uda_table::full_attribute_view(&element, &load_schema(), &uda);
    assert_uda_view(&view, &uda, element.noun, Q_ATT_UDA);
}

/// `q att` on the BRAN, UDA lines only, de-duplicated. E3D printed
/// `:SCHrefHole`, `:HXYsize` and `:TXYsize` twice each because each has two
/// Dictionary definitions under two `UKEY`s.
const Q_ATT_BRAN_UDA: &[(&str, &str)] = &[
    (":SCHrefHole", "unset"),
    (":PLANREF1", "unset"),
    (":PLANREF2", "unset"),
    (":PLANREF3", "unset"),
    (":TPress", "unset"),
    (":ROOM_NO", "NB122"),
    (":HXYsize", "unset"),
    (":TXYsize", "unset"),
    (":ISOREF", "unset"),
    (":CLEAN", "unset"),
    (":AREA", "unset"),
    (":HYDROLOOP", "unset"),
    (":G_ICS", "unset"),
    (":BranWidth", "unset"),
    (":BranHigh", "unset"),
    (":ISODrawingNo", "unset"),
    (":H-AIRQUANTITY", "0"),
    (":H-FLOWDIRECTION", "true"),
    (":H-PRESSURESPEC", "TRUE"),
    (":H-TEMPERATURE", "unset"),
    (":ICSR-DoseRate-NrOp", "unset"),
    (":ICSR-DoseRate-NrSh", "unset"),
    (":ICSR-DoseRate-Acci", "unset"),
    (":PFILoose", "unset"),
    (":PFILExcess", "unset"),
    (":PFIAExcess", "unset"),
    (":PFConsChk", "false"),
];

/// The other path: a UDA whose value the element actually stores.
///
/// The parse hands stored UDAs back with their name thrown away — `_UDAS` for
/// all of them, only `hash_val` intact — so the whole question is whether the
/// hash can be turned back into a name without asking SurrealDB.
///
/// Every expected value here is read off the record, not off another decoder.
/// That matters: `e3d-descriptor extract --uda on` reports this element's
/// `:ROOM_NO` as `unset` and its `NAME` as unstored, because it finds no
/// explicit attributes on this record at all. The bytes at offset 556 say
/// otherwise — `2652ab51` is `:ROOM_NO`'s `UKEY`, followed by five characters
/// of `NB122` — and the neighbouring branches carry `NB133` and `NB132`, so
/// this is a room-number series rather than a stray decode.
#[test]
fn a_stored_uda_resolves_to_its_name_and_beats_the_dictionary_default() {
    if !fixtures_present() {
        return;
    }
    let element = parse_element_at(BRAN_REFNO);
    let stored = &element.whole_attmap.uda_atts;
    assert_eq!(
        stored.len(),
        1,
        "this fixture is the stored-value path; without a stored UDA it proves nothing"
    );
    assert_eq!(stored[0].hash_val, ROOM_NO_UKEY);
    assert_eq!(
        stored[0].name, "_UDAS",
        "the parse discards UDA names, which is why the table has to supply them"
    );

    let uda = load_uda_snapshot();
    let definition = uda
        .by_key(ROOM_NO_UKEY as u32)
        .expect("the snapshot must define the UKEY this record stores");
    assert_eq!(definition.attr_name, ":ROOM_NO");
    assert!(
        definition.default_val.is_none(),
        "the Dictionary gives ROOM_NO no default, so the stored value is the only source"
    );

    let view = aios_database::uda_table::full_attribute_view(&element, &load_schema(), &uda);
    assert_eq!(view.get_type_str(), "BRAN");
    assert_eq!(
        view.get_as_string("NAME").as_deref(),
        Some("/C-CO-5RX122-C")
    );
    assert_eq!(view.get_as_string(":ROOM_NO").as_deref(), Some("NB122"));

    // `:ROOM_NO` has no Dictionary default, so it must arrive through the
    // stored value and must not be appended beside an `unset` twin.
    assert_uda_view(&view, &uda, element.noun, Q_ATT_BRAN_UDA);
}
