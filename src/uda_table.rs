//! Project UDA definitions read from a snapshot, for checking a decode against
//! what E3D reports.
//!
//! **This is not where the runtime gets UDAs.** Production keeps doing what it
//! did: each project's Dictionary database is parsed during sync into the
//! `UDA` / `ATT_UDA` tables, and values resolve through those. UDA definitions
//! are per project — `AvevaMarineSample`, `AvevaCatalogue` and `ZDJ` do not
//! share a `UKEY` space — so a single file in the source tree could only ever
//! be right for one of them. A snapshot is a fixture, not a configuration.
//!
//! What it is for: `all_attr_info.json` cannot describe UDAs at all. It is
//! built from the `*vir.dat` descriptors, and a UDA is not there — E3D keeps
//! it as a `UDA` element under the Dictionary world with `UKEY` (the hash
//! Design elements store values under), `ELEL` (the nouns it applies to) and
//! `DFLT` (its default). Without a second source, no offline test can state
//! what an element's `:` attributes should read.
//!
//! The part that is easy to miss: **most UDAs on a real element store nothing
//! at all.** `q att` on PIPE `=24383/73958` prints twenty `:` lines and the
//! record holds zero UDA bytes — all twenty are Dictionary defaults. So a test
//! that only resolves stored UDA values would pass while showing nothing.
//!
//! Take a snapshot of one project with:
//!
//! ```text
//! e3d-descriptor emit-uda-table --attlib ATTLIB --dicvir DICVIR \
//!   --dictionary-db-list DICT1;DICT2 --output PROJECT_uda_info.json
//! ```
//!
//! Dictionary order matters and is preserved: the first definition for a
//! `UKEY` wins, matching the runtime's ordered MDB lookup.

use std::collections::HashMap;
use std::path::Path;

use aios_core::tool::db_tool::db1_hash;
use aios_core::types::attval::AttrVal;
use aios_core::types::db_info::PdmsDatabaseInfo;
use aios_core::types::named_attmap::NamedAttrMap;
use aios_core::types::named_attvalue::NamedAttrValue;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct UdaDefinition {
    pub ukey: u32,
    /// The hash this UDA used before its key was re-allocated. Elements last
    /// written before the move still store their value under it.
    #[serde(default)]
    pub old_key: Option<u32>,
    /// Attribute name without the leading `:`.
    pub name: String,
    /// Attribute name as `q att` prints it, `:` included.
    pub attr_name: String,
    pub att_type: String,
    #[serde(default)]
    pub is_array: bool,
    /// `None` when the Dictionary declares no usable default. That is not the
    /// same as the type's zero: `q att` prints `unset` for `:TPress`, a REAL
    /// with no default, where an ordinary REAL attribute would print `0`.
    #[serde(default)]
    pub default_val: Option<AttrVal>,
    /// `dictionary` when `DFLT` supplied a type-valid value, `unset` when it
    /// declared none, `invalid` when it declared one `UTYP` rejects. The last
    /// two both read as unset, but only one of them means the Dictionary is
    /// worth a look.
    pub default_source: String,
    /// What the attribute reads as when a typed zero is wanted instead of
    /// `unset`. Always present, never used for `q att` parity.
    #[serde(default)]
    pub type_zero: AttrVal,
    #[serde(default)]
    pub nouns: Vec<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(from = "RawUdaTable")]
pub struct UdaTable {
    by_key: HashMap<u32, UdaDefinition>,
    /// Includes `old_key` entries, so a value stored before a key
    /// re-allocation still finds its definition.
    alias_to_key: HashMap<u32, u32>,
    by_noun: HashMap<u32, Vec<u32>>,
}

#[derive(Deserialize)]
struct RawUdaTable {
    #[serde(default)]
    definitions: Vec<UdaDefinition>,
}

impl From<RawUdaTable> for UdaTable {
    fn from(raw: RawUdaTable) -> Self {
        let mut table = UdaTable::default();
        for definition in raw.definitions {
            table.alias_to_key.insert(definition.ukey, definition.ukey);
            if let Some(old) = definition.old_key {
                table.alias_to_key.entry(old).or_insert(definition.ukey);
            }
            for noun in &definition.nouns {
                table.by_noun.entry(*noun).or_default().push(definition.ukey);
            }
            table.by_key.insert(definition.ukey, definition);
        }
        for keys in table.by_noun.values_mut() {
            keys.sort_unstable();
            keys.dedup();
        }
        table
    }
}

impl UdaTable {
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("读取 {} 失败：{e}", path.display()))?;
        serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("解析 {} 失败：{e}", path.display()))
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    pub fn noun_count(&self) -> usize {
        self.by_noun.len()
    }

    /// UKEYs of every UDA declared for this noun, in ascending key order.
    pub fn applicable(&self, noun_hash: u32) -> &[u32] {
        self.by_noun
            .get(&noun_hash)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The definition a stored attribute hash belongs to, following `OLDKEY`.
    pub fn by_key(&self, key: u32) -> Option<&UdaDefinition> {
        self.alias_to_key
            .get(&key)
            .and_then(|current| self.by_key.get(current))
    }

    pub fn by_name(&self, name: &str) -> Option<&UdaDefinition> {
        let name = name.trim_start_matches(':');
        self.by_key
            .values()
            .find(|definition| definition.name.eq_ignore_ascii_case(name))
    }

    /// Write the Dictionary default of every UDA this noun declares, skipping
    /// names the element already carries. Returns how many were added.
    pub fn fill_defaults(&self, noun_hash: u32, map: &mut NamedAttrMap) -> usize {
        let mut added = 0;
        for key in self.applicable(noun_hash) {
            let Some(definition) = self.by_key.get(key) else {
                continue;
            };
            if map.map.contains_key(&definition.attr_name) {
                continue;
            }
            // `InvalidType` is this codebase's `unset`: `get_as_string` renders
            // it as `UNSET_STR` and the SurrealDB writers treat it as a key to
            // clear. It is what a UDA with no Dictionary default should read as.
            let value = definition
                .default_val
                .as_ref()
                .map(Into::into)
                .unwrap_or(NamedAttrValue::InvalidType);
            map.map.insert(definition.attr_name.clone(), value);
            added += 1;
        }
        added
    }

}

/// Every attribute an element has, the way `q att` lists them.
///
/// Three sources, in increasing priority:
///
/// 1. the noun's schema defaults for attributes that store nothing
///    (`schema`, normally `all_attr_info.json`);
/// 2. the Dictionary default of every UDA the noun declares (this module);
/// 3. what the record actually stores — implicit slots, the explicit stream,
///    and UDA values keyed by `UKEY`.
///
/// Both tables are parameters rather than globals. `schema` cannot be
/// `get_default_pdms_db_info()` because that is whichever table `aios_core`
/// was compiled against and the caller is normally checking a different one —
/// filling PIPE from the embedded 339-noun table instead of the 1878-noun file
/// leaves seven of its `q att` attributes (`TPRESS`, `BENDMA`, `PLANU`,
/// `JNTC`, `WLDC`, `HEATT`, `NOHUMA`) with no row at all. `uda` is a parameter
/// because UDA definitions belong to one project, and a caller that reaches
/// for an ambient default would silently get another project's.
///
/// Stored UDA hashes resolve through `uda` rather than through SurrealDB, so
/// this works offline. A hash with no definition keeps a `UDA:<hash>` key
/// instead of being dropped — the async runtime path silently discarded those.
pub fn full_attribute_view(
    element: &parse_pdms_db::parse::EleData,
    schema: &PdmsDatabaseInfo,
    uda: &UdaTable,
) -> NamedAttrMap {
    let mut view = NamedAttrMap::default();
    for (name, value) in element.whole_attmap.attmap.map.iter() {
        view.map.insert(name.clone(), value.clone());
    }
    for (name, value) in element.whole_attmap.explicit_attmap.map.iter() {
        view.map.insert(name.clone(), value.clone());
    }

    for attr in element.whole_attmap.uda_atts.iter() {
        let key = match uda.by_key(attr.hash_val as u32) {
            Some(definition) => definition.attr_name.clone(),
            None => format!("UDA:{}", attr.hash_val),
        };
        view.map.insert(key, attr.value.clone());
    }

    let noun_hash = if element.noun != 0 {
        element.noun
    } else {
        db1_hash(view.get_type_str())
    };
    uda.fill_defaults(noun_hash, &mut view);
    fill_schema_defaults(noun_hash, schema, &mut view);
    view
}

/// `offset == 0` means the attribute has no slot in the implicit region, so
/// the record stores it only when it has been set. Absent here therefore means
/// "reads as the schema default", which is what E3D prints.
fn fill_schema_defaults(noun_hash: u32, schema: &PdmsDatabaseInfo, view: &mut NamedAttrMap) {
    let Some(attrs) = schema.noun_attr_info_map.get(&(noun_hash as i32)) else {
        return;
    };
    for info in attrs.value() {
        if info.offset == 0 && !view.map.contains_key(&info.name) {
            view.map
                .insert(info.name.clone(), (&info.default_val).into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table_from(json: &str) -> UdaTable {
        serde_json::from_str(json).expect("parse table")
    }

    const TWO: &str = r#"{
        "definitions": [
            {"ukey": 100, "old_key": 90, "name": "G_STATUS", "attr_name": ":G_STATUS",
             "att_type": "STRING", "default_val": {"StringType": "DESIGN"},
             "default_source": "dictionary", "nouns": [641779, 808220]},
            {"ukey": 200, "name": "TPress", "attr_name": ":TPress",
             "att_type": "DOUBLE", "default_val": null,
             "default_source": "unset", "type_zero": {"DoubleType": 0.0},
             "nouns": [641779]}
        ]
    }"#;

    #[test]
    fn nouns_index_both_definitions_and_keys_stay_sorted() {
        let table = table_from(TWO);
        assert_eq!(table.len(), 2);
        assert_eq!(table.noun_count(), 2);
        assert_eq!(table.applicable(641779), &[100, 200]);
        assert_eq!(table.applicable(808220), &[100]);
        assert!(table.applicable(1).is_empty());
    }

    /// A value written before the key moved is still that UDA's value, so the
    /// old hash has to resolve — otherwise it reads back as unset.
    #[test]
    fn an_old_key_resolves_to_the_definition_that_replaced_it() {
        let table = table_from(TWO);
        assert_eq!(table.by_key(100).unwrap().name, "G_STATUS");
        assert_eq!(table.by_key(90).unwrap().name, "G_STATUS");
        assert!(table.by_key(91).is_none());
    }

    #[test]
    fn defaults_fill_only_what_the_element_does_not_already_carry() {
        let table = table_from(TWO);
        let mut map = NamedAttrMap::default();
        map.map.insert(
            ":G_STATUS".into(),
            NamedAttrValue::StringType("ISSUED".into()),
        );

        assert_eq!(table.fill_defaults(641779, &mut map), 1);
        assert_eq!(
            map.get_as_string(":G_STATUS").as_deref(),
            Some("ISSUED"),
            "a stored value must win over the Dictionary default"
        );
    }

    /// `q att` prints `unset` for a REAL UDA the Dictionary gives no default,
    /// where an ordinary REAL attribute would print `0`. Collapsing the two
    /// invents a measurement that was never taken.
    #[test]
    fn a_uda_without_a_dictionary_default_reads_as_unset_not_as_zero() {
        let table = table_from(TWO);
        let mut map = NamedAttrMap::default();
        table.fill_defaults(641779, &mut map);

        assert_eq!(map.get_as_string(":TPress").as_deref(), Some("unset"));
        assert!(matches!(
            map.map.get(":TPress"),
            Some(NamedAttrValue::InvalidType)
        ));
        assert_eq!(map.get_as_string(":G_STATUS").as_deref(), Some("DESIGN"));
    }

    #[test]
    fn a_missing_table_is_empty_rather_than_fatal() {
        let table = UdaTable::default();
        assert!(table.is_empty());
        let mut map = NamedAttrMap::default();
        assert_eq!(table.fill_defaults(641779, &mut map), 0);
        assert!(map.map.is_empty());
    }

}
