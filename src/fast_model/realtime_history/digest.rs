//! 几何摘要与源文件集指纹（design §1.4；Requirement 6.10、6.11、7.4）。
//!
//! * [`geometry_digest`]：响应里 `geometryKey` / `ETag` 的值。只看三维显示用得到的内容：
//!   每行去掉随会话号变、但不影响显示的 `snapshot_key` 与 `direct_model.sesno`，键按字典序
//!   规范化输出，按 `(kind, source_refno, id)` 排序后逐行 `kind \0 json \n` 喂 SHA-256，
//!   取前 16 字节 hex，前缀 `g:`。所以「仅属性」会话与前一版摘要相同，可以 304。
//!   空单元（实例与管段都空）走同一算法，得到一个固定值。
//! * [`source_set_fingerprint`]：结果缓存键里的「源文件指纹」。设计库文件的
//!   [`source_fingerprint`] ‖ 当前 MDB 全部 pin（含 CATA）按 `(dbnum, path)` 排序后的
//!   (路径, 长度, mtime)。只读文件元数据；同一设计库文件在 [`FINGERPRINT_TTL`] 内复用上次结果。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, UNIX_EPOCH};

use once_cell::sync::Lazy;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::data_interface::direct_store::DbPin;
use crate::fast_model::e3d_model_service::current_mdb_sources;
use crate::fast_model::historical_model::source_fingerprint;

/// 摘要前缀。
pub const GEOMETRY_KEY_PREFIX: &str = "g:";

/// 实例行与管段行的 `kind`。
pub const KIND_INSTANCE: &str = "instance";
pub const KIND_TUBE: &str = "tube";

/// 源文件集指纹的记忆时长。
pub const FINGERPRINT_TTL: Duration = Duration::from_secs(1);

/// `instances` 与 `tubes` 的几何内容摘要（`g:` + 32 位小写 hex）。
pub fn geometry_digest(instances: &[Value], tubes: &[Value]) -> String {
    let mut rows: Vec<(&'static str, String, String, String)> = instances
        .iter()
        .map(|row| (KIND_INSTANCE, row))
        .chain(tubes.iter().map(|row| (KIND_TUBE, row)))
        .map(|(kind, row)| {
            let stripped = strip_volatile(row);
            (
                kind,
                sort_field(&stripped, "source_refno"),
                sort_field(&stripped, "id"),
                canonical_json(&stripped),
            )
        })
        .collect();
    // 末项（整行规范化 JSON）兜底：同 (kind, source_refno, id) 的行也有唯一顺序，摘要与输入行序无关。
    rows.sort();

    let mut hash = Sha256::new();
    for (kind, _, _, json) in &rows {
        hash.update(kind.as_bytes());
        hash.update([0u8]);
        hash.update(json.as_bytes());
        hash.update([b'\n']);
    }
    let digest = hash.finalize();
    format!("{GEOMETRY_KEY_PREFIX}{}", hex::encode(&digest[..16]))
}

/// 去掉顶层 `snapshot_key` 与 `direct_model.sesno`。
fn strip_volatile(row: &Value) -> Value {
    let mut row = row.clone();
    if let Value::Object(map) = &mut row {
        map.remove("snapshot_key");
        if let Some(Value::Object(direct)) = map.get_mut("direct_model") {
            direct.remove("sesno");
        }
    }
    row
}

/// 排序用的字段值：字符串取原文，其余取规范化 JSON，缺字段为空串。
fn sort_field(row: &Value, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(other) => canonical_json(other),
        None => String::new(),
    }
}

/// 键按字典序的紧凑 JSON。不依赖 `serde_json` 是否开了 `preserve_order`。
fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(&map[key.as_str()], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// 设计库文件 `design_file` 与当前 MDB 全部 pin 的源文件集指纹（64 位小写 hex）。
///
/// 同一 `design_file` 在 [`FINGERPRINT_TTL`] 内返回上次的结果，不再读 MDB 成员、不再 stat。
pub fn source_set_fingerprint(design_file: &Path) -> anyhow::Result<String> {
    static MEMO: Lazy<TtlMemo> = Lazy::new(|| TtlMemo::new(FINGERPRINT_TTL));
    MEMO.get_or_compute(design_file, Instant::now(), || {
        let (pins, _) = current_mdb_sources()?;
        fingerprint_of(design_file, &pins)
    })
}

/// [`source_set_fingerprint`] 的不记忆核：`source_fingerprint(design_file)` ‖ 按 `(dbnum, path)`
/// 排序的每个 pin 的 (路径, 长度, mtime)。pin 的文件读不到元数据时记一个「缺失」标记，不让整个
/// 请求失败（CATA 声明的文件不一定是生成真读的那份）；设计库文件本身读不到则报错。
pub(crate) fn fingerprint_of(design_file: &Path, pins: &[DbPin]) -> anyhow::Result<String> {
    let mut hash = Sha256::new();
    hash.update(source_fingerprint(design_file)?.as_bytes());

    let mut ordered: Vec<&DbPin> = pins.iter().collect();
    ordered.sort_by(|a, b| (a.dbnum, &a.file).cmp(&(b.dbnum, &b.file)));
    for pin in ordered {
        let path = pin.file.as_os_str().to_string_lossy();
        hash.update((path.len() as u64).to_be_bytes());
        hash.update(path.as_bytes());
        match std::fs::metadata(&pin.file) {
            Ok(meta) => {
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                    .map(|value| value.as_nanos())
                    .unwrap_or_default();
                hash.update([1u8]);
                hash.update(meta.len().to_be_bytes());
                hash.update(modified.to_be_bytes());
            }
            Err(_) => hash.update([0u8]),
        }
    }
    Ok(hex::encode(hash.finalize()))
}

/// 按设计库文件路径记住指纹，过期即重算。只缓存成功结果。
struct TtlMemo {
    ttl: Duration,
    entries: Mutex<HashMap<PathBuf, (Instant, String)>>,
}

impl TtlMemo {
    fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::new(HashMap::new()),
        }
    }

    fn get_or_compute(
        &self,
        key: &Path,
        now: Instant,
        compute: impl FnOnce() -> anyhow::Result<String>,
    ) -> anyhow::Result<String> {
        {
            let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((at, value)) = entries.get(key) {
                if now.saturating_duration_since(*at) < self.ttl {
                    return Ok(value.clone());
                }
            }
        }
        // 计算不持锁：stat 与读 MDB 成员可能慢，不挡别的设计库文件的请求。
        let value = compute()?;
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|_, (at, _)| now.saturating_duration_since(*at) < self.ttl);
        entries.insert(key.to_path_buf(), (now, value.clone()));
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn instance(id: &str, refno: &str, sesno: u32, snapshot_key: &str) -> Value {
        json!({
            "id": id,
            "snapshot_key": snapshot_key,
            "dbnum": 7997,
            "source_refno": refno,
            "mesh_id": format!("m-{id}"),
            "world_bounds": { "mins": [0.0, 0.0, 0.0], "maxs": [1.0, 1.0, 1.0] },
            "direct_model": { "source": "e3d-model", "sesno": sesno, "mesh_id": format!("m-{id}") },
        })
    }

    fn tube(id: &str, refno: &str, sesno: u32) -> Value {
        json!({
            "id": id,
            "source_refno": refno,
            "route_ordinal": 0,
            "direct_model": { "source": "e3d-model", "sesno": sesno },
        })
    }

    fn is_key(value: &str) -> bool {
        value.len() == 2 + 32
            && value.starts_with("g:")
            && value[2..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    #[test]
    fn has_prefix_and_sixteen_byte_hex() {
        let key = geometry_digest(&[instance("a", "7997_1", 3, "")], &[]);
        assert!(is_key(&key), "{key}");
    }

    #[test]
    fn ignores_snapshot_key_and_direct_model_sesno() {
        let before = geometry_digest(
            &[instance("a", "7997_1", 3, "7997_1@3")],
            &[tube("t", "7997_1", 3)],
        );
        let after = geometry_digest(
            &[instance("a", "7997_1", 9, "7997_1@9")],
            &[tube("t", "7997_1", 9)],
        );
        let no_fields = geometry_digest(
            &[{
                let mut row = instance("a", "7997_1", 0, "");
                row.as_object_mut().unwrap().remove("snapshot_key");
                row["direct_model"].as_object_mut().unwrap().remove("sesno");
                row
            }],
            &[{
                let mut row = tube("t", "7997_1", 0);
                row["direct_model"].as_object_mut().unwrap().remove("sesno");
                row
            }],
        );
        assert_eq!(before, after);
        assert_eq!(before, no_fields);
    }

    #[test]
    fn other_fields_change_the_key() {
        let base = geometry_digest(&[instance("a", "7997_1", 3, "")], &[]);
        let mut moved = instance("a", "7997_1", 3, "");
        moved["world_bounds"]["maxs"] = json!([2.0, 1.0, 1.0]);
        assert_ne!(base, geometry_digest(&[moved], &[]));
        // 同一行放到另一个 kind 下也是不同内容。
        let row = instance("a", "7997_1", 3, "");
        assert_ne!(
            geometry_digest(std::slice::from_ref(&row), &[]),
            geometry_digest(&[], std::slice::from_ref(&row))
        );
    }

    #[test]
    fn row_order_does_not_matter() {
        let a = instance("a", "7997_2", 3, "");
        let b = instance("b", "7997_1", 3, "");
        let c = instance("c", "7997_1", 3, "");
        let t1 = tube("t1", "7997_1", 3);
        let t2 = tube("t2", "7997_3", 3);
        assert_eq!(
            geometry_digest(&[a.clone(), b.clone(), c.clone()], &[t1.clone(), t2.clone()]),
            geometry_digest(&[c, a, b], &[t2, t1]),
        );
    }

    #[test]
    fn key_order_does_not_matter() {
        let forward = json!({ "id": "a", "source_refno": "7997_1", "mesh_id": "m", "nested": { "x": 1, "y": 2 } });
        let mut reversed = serde_json::Map::new();
        reversed.insert("nested".into(), json!({ "y": 2, "x": 1 }));
        reversed.insert("mesh_id".into(), json!("m"));
        reversed.insert("source_refno".into(), json!("7997_1"));
        reversed.insert("id".into(), json!("a"));
        assert_eq!(
            geometry_digest(&[forward], &[]),
            geometry_digest(&[Value::Object(reversed)], &[])
        );
    }

    #[test]
    fn empty_unit_has_a_fixed_key() {
        let empty = geometry_digest(&[], &[]);
        assert!(is_key(&empty), "{empty}");
        assert_eq!(empty, geometry_digest(&[], &[]));
        // 空单元 = 不喂任何字节的 SHA-256 前 16 字节。
        assert_eq!(empty, "g:e3b0c44298fc1c149afbf4c8996fb924");
        assert_ne!(empty, geometry_digest(&[instance("a", "7997_1", 3, "")], &[]));
    }

    fn pin(dbnum: i32, db_type: &str, file: PathBuf) -> DbPin {
        DbPin {
            dbnum,
            db_type: db_type.to_string(),
            file,
            sesno: None,
        }
    }

    #[test]
    fn fingerprint_follows_pin_files_but_not_pin_order() {
        let dir = tempfile::tempdir().unwrap();
        let design = dir.path().join("des7997");
        let cata = dir.path().join("cat7000");
        std::fs::write(&design, b"design").unwrap();
        std::fs::write(&cata, b"cata").unwrap();
        let pins = vec![
            pin(7997, "DESI", design.clone()),
            pin(7000, "CATA", cata.clone()),
        ];
        let reversed: Vec<DbPin> = pins.iter().rev().cloned().collect();

        let first = fingerprint_of(&design, &pins).unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(first, fingerprint_of(&design, &reversed).unwrap());

        // CATA 文件长度变了 → 指纹变。
        std::fs::write(&cata, b"cata-longer").unwrap();
        assert_ne!(first, fingerprint_of(&design, &pins).unwrap());

        // 设计库文件读不到 → 报错；pin 文件读不到只改变指纹。
        assert!(fingerprint_of(&dir.path().join("missing"), &pins).is_err());
        let with_missing = vec![pin(7001, "CATA", dir.path().join("gone"))];
        assert!(fingerprint_of(&design, &with_missing).is_ok());
    }

    #[test]
    fn memo_reuses_within_ttl_and_recomputes_after() {
        let memo = TtlMemo::new(Duration::from_secs(1));
        let key = Path::new("des7997");
        let start = Instant::now();
        let compute = |value: &str| {
            let value = value.to_string();
            move || Ok(value)
        };
        assert_eq!(memo.get_or_compute(key, start, compute("v1")).unwrap(), "v1");
        assert_eq!(
            memo.get_or_compute(key, start + Duration::from_millis(999), compute("v2"))
                .unwrap(),
            "v1"
        );
        assert_eq!(
            memo.get_or_compute(key, start + Duration::from_secs(1), compute("v3"))
                .unwrap(),
            "v3"
        );
        // 失败不进缓存。
        let other = Path::new("des8000");
        assert!(
            memo.get_or_compute(other, start, || anyhow::bail!("boom"))
                .is_err()
        );
        assert_eq!(memo.get_or_compute(other, start, compute("ok")).unwrap(), "ok");
    }
}
