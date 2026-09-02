//! 增量窗口（`EleOperationData`）到 JSON 的转换。
//!
//! `EleOperationData` / `EleOperationDetail` 没有 serde 派生，这里手工转成与
//! web-service「serde 原样透传」同风格的 JSON。refno 一律输出 `a_b` 形态——
//! 与库内 `pe:` record id 一致，调试脚本拿到就能直接拼下一条 SurrealQL。

use std::collections::BTreeMap;
use std::collections::HashMap;

use aios_core::{NamedAttrMap, NamedAttrValue, RefU64};
use pdms_io::io::{EleOperationData, EleOperationDetail, ModifiedElement};
use serde_json::{Value, json};

pub fn window_to_json(window: &BTreeMap<u32, Vec<EleOperationData>>, detail: bool) -> Value {
    let mut out = serde_json::Map::new();
    for (sesno, ops) in window {
        out.insert(
            sesno.to_string(),
            Value::Array(ops.iter().map(|op| op_to_json(op, detail)).collect()),
        );
    }
    Value::Object(out)
}

fn op_to_json(op: &EleOperationData, detail: bool) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("refno".into(), json!(refno_str(op.refno)));
    obj.insert("sesno".into(), json!(op.sesno));
    match &op.detail {
        EleOperationDetail::Add(data) => {
            obj.insert("op".into(), json!("add"));
            obj.insert("noun".into(), json!(data.noun));
            obj.insert("name".into(), json!(data.name));
            obj.insert("owner".into(), json!(refno_str(data.owner)));
            obj.insert(
                "children".into(),
                serde_json::to_value(&data.children).unwrap_or(Value::Null),
            );
            if detail {
                obj.insert(
                    "attrs".into(),
                    serde_json::to_value(data.att_map()).unwrap_or(Value::Null),
                );
                obj.insert(
                    "explicit_attrs".into(),
                    serde_json::to_value(data.explicit_attmap()).unwrap_or(Value::Null),
                );
            }
        }
        EleOperationDetail::Deleted => {
            obj.insert("op".into(), json!("deleted"));
        }
        EleOperationDetail::Modified(modified) => {
            obj.insert("op".into(), json!("modified"));
            merge_modified(&mut obj, modified, detail);
        }
        EleOperationDetail::None => {
            obj.insert("op".into(), json!("none"));
        }
    }
    Value::Object(obj)
}

fn merge_modified(obj: &mut serde_json::Map<String, Value>, m: &ModifiedElement, detail: bool) {
    obj.insert("noun".into(), json!(m.noun));
    obj.insert("added".into(), single_map(&m.added_attrs, detail));
    obj.insert("deleted".into(), single_map(&m.deleted_attrs, detail));
    obj.insert("modified".into(), pair_map(&m.modified_attrs, detail));
    if !m.added_explicit_attrs.is_empty() {
        obj.insert(
            "added_explicit".into(),
            single_map(&m.added_explicit_attrs, detail),
        );
    }
    if !m.deleted_explicit_attrs.is_empty() {
        obj.insert(
            "deleted_explicit".into(),
            single_map(&m.deleted_explicit_attrs, detail),
        );
    }
    if !m.modified_explicit_attrs.is_empty() {
        obj.insert(
            "modified_explicit".into(),
            pair_map(&m.modified_explicit_attrs, detail),
        );
    }
    let uda: Vec<i32> = {
        let mut keys: Vec<i32> = m
            .added_uda_attrs
            .keys()
            .chain(m.deleted_uda_attrs.keys())
            .chain(m.modified_uda_attrs.keys())
            .copied()
            .collect();
        keys.sort_unstable();
        keys.dedup();
        keys
    };
    if !uda.is_empty() {
        obj.insert("uda_keys".into(), json!(uda));
    }
    if let Some((old, new)) = &m.children_changed {
        obj.insert(
            "children_changed".into(),
            json!({
                "old": serde_json::to_value(old).unwrap_or(Value::Null),
                "new": serde_json::to_value(new).unwrap_or(Value::Null),
            }),
        );
    }
}

/// `detail=false` 给排序后的属性名列表；`detail=true` 给 `{名: 值}`。
fn single_map(map: &HashMap<String, NamedAttrValue>, detail: bool) -> Value {
    if detail {
        let mut entries: Vec<(&String, &NamedAttrValue)> = map.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        Value::Object(
            entries
                .into_iter()
                .map(|(k, v)| (k.clone(), attr_value(v)))
                .collect(),
        )
    } else {
        sorted_keys(map.keys())
    }
}

/// `detail=false` 给排序后的属性名列表；`detail=true` 给 `{名: [旧, 新]}`。
fn pair_map(map: &HashMap<String, (NamedAttrValue, NamedAttrValue)>, detail: bool) -> Value {
    if detail {
        let mut entries: Vec<(&String, &(NamedAttrValue, NamedAttrValue))> = map.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        Value::Object(
            entries
                .into_iter()
                .map(|(k, (old, new))| (k.clone(), json!([attr_value(old), attr_value(new)])))
                .collect(),
        )
    } else {
        sorted_keys(map.keys())
    }
}

fn sorted_keys<'a>(keys: impl Iterator<Item = &'a String>) -> Value {
    let mut names: Vec<&String> = keys.collect();
    names.sort();
    Value::Array(names.into_iter().map(|k| json!(k)).collect())
}

fn attr_value(value: &NamedAttrValue) -> Value {
    serde_json::to_value(value).unwrap_or_else(|_| json!(format!("{value:?}")))
}

fn refno_str(refno: RefU64) -> String {
    format!("{}_{}", refno.get_0(), refno.get_1())
}

/// 单元素文件直读 dump（`parse.element`）。`found_sesno` 是命中的版本所在会话号
/// （元素最后一次被写入的会话，不一定等于查询给的上限）。
pub fn ele_data_to_json(data: &parse_pdms_db::parse::EleData, found_sesno: u32) -> Value {
    json!({
        "refno": refno_str(data.refno),
        "found_sesno": found_sesno,
        "noun_hash": data.noun,
        "noun": aios_core::tool::db_tool::db1_dehash(data.noun),
        "name": data.name,
        "owner": refno_str(data.owner),
        "children": data.children.0.iter().map(|r| refno_str(*r)).collect::<Vec<_>>(),
        "attrs": serde_json::to_value(data.att_map()).unwrap_or(Value::Null),
        "explicit_attrs": serde_json::to_value(data.explicit_attmap()).unwrap_or(Value::Null),
    })
}

/// `NamedAttrValue` → 平面 JSON 值（`parse.attmap` 的生成期语义视图用）。
///
/// 只做结构转换、不做语义归一：Int/Long → int、F32 → float、Vec3 → `[x,y,z]`、
/// 数组 → list、refno 一律 `a_b`、Invalid → None。词属性在 direct 原始视图里是
/// 词哈希整数——按 schema 反哈希对齐 DB 视图是 D2 同源转换器（Q4）的职责
/// （`docs/plans/direct-dbelement-read-api.md`），这里不做第二实现。
pub fn plain_attr_value(value: &NamedAttrValue) -> Value {
    match value {
        NamedAttrValue::InvalidType => Value::Null,
        NamedAttrValue::IntegerType(v) => json!(v),
        NamedAttrValue::LongType(v) => json!(v),
        NamedAttrValue::BoolType(v) => json!(v),
        NamedAttrValue::F32Type(v) => json!(v),
        NamedAttrValue::F32VecType(v) => json!(v),
        NamedAttrValue::Vec3Type(v) => json!([v.x, v.y, v.z]),
        NamedAttrValue::StringType(v)
        | NamedAttrValue::WordType(v)
        | NamedAttrValue::ElementType(v) => json!(v),
        NamedAttrValue::StringArrayType(v) => json!(v),
        NamedAttrValue::BoolArrayType(v) => json!(v),
        NamedAttrValue::IntArrayType(v) => json!(v),
        NamedAttrValue::RefU64Type(v) => json!(refno_str(*v)),
        NamedAttrValue::RefnoEnumType(v) => json!(refno_str(v.refno())),
        NamedAttrValue::RefU64Array(v) => {
            Value::Array(v.iter().map(|e| json!(refno_str(e.refno()))).collect())
        }
    }
}

/// 合并 attmap → `{属性名: 平面值}`（BTreeMap，键序稳定）。
pub fn named_attr_map_to_plain(map: &NamedAttrMap) -> Value {
    Value::Object(
        map.map
            .iter()
            .map(|(k, v)| (k.clone(), plain_attr_value(v)))
            .collect(),
    )
}

/// 生成期语义视图 dump（`parse.attmap`）：`WholeAttMap::merge()` 后的平面形态。
///
/// 与 [`ele_data_to_json`]（原始两段 tagged dump）的差别：attrs 是「常规 attmap
/// 打底、显式属性补缺」的合并结果 + 平面值；另给 `explicit_keys`（哪些键来自
/// 显式段）与 `uda_count`（UDA 显式属性条数，merge 不含它们）便于测试对账。
pub fn ele_data_to_merged_json(data: &parse_pdms_db::parse::EleData, found_sesno: u32) -> Value {
    let merged = data.whole_attmap.merge();
    let explicit_keys: Vec<&String> = data.whole_attmap.explicit_attmap().map.keys().collect();
    // 直接从文件属性构造可复用的几何参数；这里不落库，也不调用 aios_core 查询接口。
    // 容器节点通常为 Unknown，基本体节点则带有 PdmsGeoParam，供后续 direct 生成器消费。
    let (geo_param, mesh, rvm_primitive) = merged
        .create_brep_shape(None)
        .map(|shape| {
            let mesh = shape
                .gen_csg_mesh()
                .and_then(|mesh| serde_json::to_value(mesh).ok());
            let rvm_primitive = shape
                .convert_to_geo_param()
                .and_then(|param| param.convert_rvm_pri_data())
                .and_then(|bytes| String::from_utf8(bytes).ok());
            let param = shape
                .convert_to_geo_param()
                .and_then(|param| serde_json::to_value(param).ok());
            (param, mesh, rvm_primitive)
        })
        .unwrap_or((None, None, None));
    let geo_valid = geo_param.is_some();
    json!({
        "refno": refno_str(data.refno),
        "found_sesno": found_sesno,
        "noun_hash": data.noun,
        "noun": aios_core::tool::db_tool::db1_dehash(data.noun),
        "name": data.name,
        "owner": refno_str(data.owner),
        "children": data.children.0.iter().map(|r| refno_str(*r)).collect::<Vec<_>>(),
        "attrs": named_attr_map_to_plain(&merged),
        "explicit_keys": explicit_keys,
        "uda_count": data.whole_attmap.uda_atts().len(),
        "geo_param": geo_param,
        "geo_valid": geo_valid,
        "mesh": mesh,
        "rvm_primitive": rvm_primitive,
    })
}
