//! ADR-053 探针：direct（e3d-io 文件直读）vs DB（SurrealDB）attmap 逐字段对拍。
//!
//! P0 时它跑的是 pdms-io；**已拍板改用 e3d-io**，而且不再自己拼 attmap——它现在打的是
//! `DirectStore` + `direct_attmap` 这条真实取数路径，所以这个探针同时是那两个模块的
//! 回归测试（`docs/plans/direct-mode-model-generation.md` P1：「P0 探针转正为其回归测试」）。
//!
//! 两侧：
//! - direct：`DirectStore::attrs_in`（按 `applied_sesno` pin，Q3）→ `NamedAttrMap`；
//! - DB：`aios_core::get_named_attmap`（生成期真实读路径）。
//!
//! 逐键 canonical 比对，差异分类为「值不匹配 / 仅 DB 有 / 仅 direct 有」。**分类不是
//! 为了把差异抹平**：每一类都要能回答「为什么它不是错」，回答不了的就是真值冲突，
//! 退出码 1。
//!
//! 「值不匹配」「仅 DB 有」两类都再按 `model_impact::attribute_affects_model` 切一刀：
//! 生成链不消费的键两侧不一致不影响产物，不计冲突——但**照样逐键打印**，因为这条豁免
//! 兜住的可能是库的缺陷（如 `CRFA`，见 `issues/ISSUE-027`），不是「没事」。
//!
//! 用法：
//! ```text
//! cargo run --release --bin direct_attmap_probe -- --dbnum 8000 --sample 200
//! cargo run --release --bin direct_attmap_probe -- --dbnum 0            # 列水位行
//! cargo run --release --bin direct_attmap_probe -- --dbnum 8000 --owner-climb
//! ```

use aios_core::options::DbOption;
use aios_core::types::NamedAttrValue;
use aios_core::{NamedAttrMap, RefnoEnum, SUL_DB};
use aios_database::data_interface::cata_closure::InMemoryCataLocator;
use aios_database::data_interface::direct_attmap::DirectAttrs;
use aios_database::data_interface::direct_store::{DirectSchema, DirectStore, pins_from_watermark};
use clap::Parser;
use config::{Config, File};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(name = "direct_attmap_probe")]
#[command(about = "ADR-053：e3d-io 文件直读 attmap 与 SurrealDB attmap 逐字段对拍")]
struct Args {
    /// 目标 dbnum（用其水位行定位文件与 applied_sesno）。0 = 列出全部水位行。
    #[arg(long)]
    dbnum: i32,
    /// 随机抽样元素个数（--refnos 给出时忽略）。
    #[arg(long, default_value_t = 200)]
    sample: usize,
    /// 显式参考号列表，逗号分隔（如 24384_18447,24384_18448）。
    #[arg(long)]
    refnos: Option<String>,
    /// 每类差异最多打印的明细行数。
    #[arg(long, default_value_t = 30)]
    max_detail: usize,
    /// 按 noun 过滤采样（逗号分隔，如 EQUI,BRAN,GENSEC）。
    #[arg(long)]
    nouns: Option<String>,
    /// 额外验一遍跨库 owner 上溯：抽到的元素里，OWNER 落在别的 dbnum 的，
    /// 用 ref0 反查换库后必须也能读出属性（ADR-053 R2）。
    #[arg(long, default_value_t = false)]
    owner_climb: bool,
    /// 只 dump 指定键（逗号分隔）的 direct 原始描述符值与 DB 值，用于定位精确类型。
    /// 定形规则要靠这个来定，不是靠猜——`--dump-keys BANG` 就是 BANG 那条规则的出处。
    #[arg(long)]
    dump_keys: Option<String>,
}

/// canonical 形态：抹平「同语义不同变体」的表示差异（写库 JSON 往返所致）。
#[derive(Debug, Clone, PartialEq)]
enum Canon {
    Null,
    Int(i64),
    Bool(bool),
    Floats(Vec<f32>),
    Str(String),
    Strs(Vec<String>),
    Bools(Vec<bool>),
    Ints(Vec<i64>),
    Ref(String),
    Refs(Vec<String>),
}

fn canon(v: &NamedAttrValue) -> Canon {
    match v {
        NamedAttrValue::InvalidType => Canon::Null,
        NamedAttrValue::IntegerType(i) => Canon::Int(*i as i64),
        NamedAttrValue::LongType(i) => Canon::Int(*i),
        NamedAttrValue::BoolType(b) => Canon::Bool(*b),
        NamedAttrValue::F32Type(f) => Canon::Floats(vec![*f]),
        NamedAttrValue::F32VecType(v) => Canon::Floats(v.clone()),
        NamedAttrValue::Vec3Type(v) => Canon::Floats(vec![v.x, v.y, v.z]),
        NamedAttrValue::StringType(s)
        | NamedAttrValue::WordType(s)
        | NamedAttrValue::ElementType(s) => Canon::Str(s.trim().to_string()),
        NamedAttrValue::StringArrayType(v) => {
            Canon::Strs(v.iter().map(|s| s.trim().to_string()).collect())
        }
        NamedAttrValue::BoolArrayType(v) => Canon::Bools(v.clone()),
        NamedAttrValue::IntArrayType(v) => Canon::Ints(v.iter().map(|i| *i as i64).collect()),
        NamedAttrValue::RefU64Type(r) => Canon::Ref(r.to_string()),
        NamedAttrValue::RefnoEnumType(e) => Canon::Ref(e.refno().to_string()),
        NamedAttrValue::RefU64Array(v) => {
            Canon::Refs(v.iter().map(|e| e.refno().to_string()).collect())
        }
    }
}

/// 写库侧 f32 走 3 位舍入（`f32_round_3`），浮点比较取同级容差。
fn floats_eq(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| (x - y).abs() <= f32::max(2e-3, y.abs() * 1e-4))
}

fn canon_eq(a: &Canon, b: &Canon) -> bool {
    match (a, b) {
        (Canon::Floats(x), Canon::Floats(y)) => floats_eq(x, y),
        // 单元素浮点与整数互看（JSON 数字往返可能丢类型信息）。
        (Canon::Floats(x), Canon::Int(i)) | (Canon::Int(i), Canon::Floats(x)) => {
            x.len() == 1 && floats_eq(x, &[*i as f32])
        }
        _ => a == b,
    }
}

/// DB 读损耗：写库为数组/数字，读侧 schema 认作字符串且转换失败落空串——
/// 生成侧今日看到的就是空值，direct 数据更全但视图需对齐。单列计数、不算错。
fn db_lossy_view(direct: &NamedAttrValue, db: &NamedAttrValue) -> bool {
    let db_empty = matches!(
        db,
        NamedAttrValue::StringType(s) | NamedAttrValue::WordType(s) if s.is_empty()
    ) || matches!(db, NamedAttrValue::InvalidType);
    db_empty
        && matches!(
            direct,
            NamedAttrValue::IntArrayType(_)
                | NamedAttrValue::F32VecType(_)
                | NamedAttrValue::StringArrayType(_)
                | NamedAttrValue::IntegerType(_)
                | NamedAttrValue::StringType(_)
                | NamedAttrValue::WordType(_)
        )
        && !matches!(direct, NamedAttrValue::StringType(s) | NamedAttrValue::WordType(s) if !s.is_empty())
}

#[derive(Default)]
struct RefnoDiff {
    equal_keys: usize,
    normalized_keys: usize,
    lossy_keys: Vec<String>,
    mismatches: Vec<(String, String, String)>,
    neutral_mismatches: Vec<(String, String, String)>,
    only_db: Vec<String>,
    only_db_neutral: Vec<String>,
    only_direct: Vec<String>,
}

fn diff_maps(direct: &NamedAttrMap, db: &NamedAttrMap) -> RefnoDiff {
    let mut d = RefnoDiff::default();
    for (k, dv) in &direct.map {
        // SESNO 是写库簿记字段：DB 行存最后写入时的会话号，与文件里元素的会话号语义
        // 不同，属元数据。转换器本就不产出它，这里的跳过是双保险。
        if k == "SESNO" {
            continue;
        }
        match db.map.get(k) {
            Some(bv) => {
                if dv == bv {
                    d.equal_keys += 1;
                } else if canon_eq(&canon(dv), &canon(bv)) {
                    d.normalized_keys += 1;
                } else if db_lossy_view(dv, bv) {
                    d.lossy_keys.push(k.clone());
                } else if aios_database::data_interface::model_impact::attribute_affects_model(k) {
                    d.mismatches
                        .push((k.clone(), format!("{dv:?}"), format!("{bv:?}")));
                } else {
                    // 模型中立键两侧值不同：生成链不消费它，产物不受影响，所以不构成
                    // 生成期等价性冲突——与下面「仅 DB 有」用的是同一条判据。
                    //
                    // 注意这条判据里「中立」含 `Unknown`（`attribute_affects_model` 把
                    // 未登记的名字也判 false）。所以落进这个桶的键分两种：真的 data-only，
                    // 和**还没人定性过**。后者的豁免是假设，不是结论——这正是必须打印的原因。
                    //
                    // **这条豁免必须限死在 `attribute_affects_model` 上，并且逐键打印。**
                    // 已知的第一例是 `CRFA`：DB 写侧被 schema 的标量 ELEMENT 声明逼住，
                    // 把引用数组的槽位数当成 refno 的 word0 写了进去（3 槽 → `pe:3_0`，
                    // 4 槽 → `pe:4_0`，两个 id 在 pe 表里都不存在）。direct 读的是文件里
                    // 的真引用，两边对不上是**库里错了**，不是 direct 读错了。
                    // 详见 `issues/ISSUE-027`。悄悄吞掉它等于把库的缺陷洗成绿灯。
                    d.neutral_mismatches
                        .push((k.clone(), format!("{dv:?}"), format!("{bv:?}")));
                }
            }
            None => d.only_direct.push(k.clone()),
        }
    }
    for k in db.map.keys() {
        if k == "SESNO" || k == "PGNO" || direct.map.contains_key(k) {
            continue;
        }
        // 模型中立的业务元数据（如项目 UDA CACHID）不构成生成期等价性冲突。
        if aios_database::data_interface::model_impact::attribute_affects_model(k) {
            d.only_db.push(k.clone());
        } else {
            d.only_db_neutral.push(k.clone());
        }
    }
    d
}

#[derive(Default)]
struct Tally {
    identical: usize,
    normalized_only: usize,
    superset_only: usize,
    diverged: usize,
    missing_direct: Vec<String>,
    missing_db: Vec<String>,
    only_db_hist: BTreeMap<String, usize>,
    only_db_neutral_hist: BTreeMap<String, usize>,
    only_direct_hist: BTreeMap<String, usize>,
    lossy_hist: BTreeMap<String, usize>,
    mismatch_hist: BTreeMap<String, usize>,
    neutral_mismatch_hist: BTreeMap<String, usize>,
    read_fail_hist: BTreeMap<String, usize>,
    read_fail_samples: Vec<String>,
    noun_hist: BTreeMap<String, usize>,
    diverged_noun_hist: BTreeMap<String, usize>,
    outside_schema_hist: BTreeMap<String, usize>,
    unset_hist: BTreeMap<String, usize>,
    shape_conflict_hist: BTreeMap<String, usize>,
    view_divergence_hist: BTreeMap<String, usize>,
    undecoded_hist: BTreeMap<String, usize>,
}

fn record_side_channels(tally: &mut Tally, attrs: &DirectAttrs) {
    for k in &attrs.outside_schema {
        *tally.outside_schema_hist.entry(k.clone()).or_default() += 1;
    }
    for k in &attrs.unset {
        *tally.unset_hist.entry(k.clone()).or_default() += 1;
    }
    for c in &attrs.shape_conflicts {
        *tally
            .shape_conflict_hist
            .entry(format!("{}({} vs {})", c.name, c.found, c.declared))
            .or_default() += 1;
    }
    for c in &attrs.view_divergence {
        *tally
            .view_divergence_hist
            .entry(format!("{}({} vs {})", c.name, c.found, c.declared))
            .or_default() += 1;
    }
    for u in &attrs.undecoded {
        *tally
            .undecoded_hist
            .entry(format!("{}({})", u.name, u.reason))
            .or_default() += 1;
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let cfg = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = cfg.try_deserialize()?;
    aios_core::connect_surdb(
        &db_option.get_version_db_conn_str(),
        &db_option.project_code,
        &db_option.project_name,
        &db_option.v_user,
        &db_option.v_password,
    )
    .await?;

    let pins = pins_from_watermark().await?;
    if args.dbnum == 0 {
        for pin in &pins {
            println!(
                "{} type={} sesno={:?} file={}",
                pin.dbnum,
                pin.db_type,
                pin.sesno,
                pin.file.display()
            );
        }
        println!("[probe] {} 个库带文件路径", pins.len());
        return Ok(());
    }

    let target = pins
        .iter()
        .find(|pin| pin.dbnum == args.dbnum)
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "dbnum_watermark:{} 不存在或无 file_path——CATA 等按需解析库不入水位表",
                args.dbnum
            )
        })?;
    println!(
        "[probe] dbnum={} type={} pin={:?} file={}",
        target.dbnum,
        target.db_type,
        target.sesno,
        target.file.display()
    );

    // 取数底座：字典单例 + ref0 定位器 + 全部水位 pin（跨库 owner 上溯要用别的库）。
    let schema = Arc::new(DirectSchema::open_from_env()?);
    println!("[probe] 模板目录 {}", schema.template_dir().display());
    let locator = Arc::new(InMemoryCataLocator::build_for_project(&db_option.project_name).await?);
    println!(
        "[probe] 定位器 ref0={} dbnum={}",
        locator.ref0_count(),
        locator.dbnum_count()
    );
    let store = DirectStore::new(schema, locator);
    for pin in &pins {
        store.pin(pin.clone());
    }

    let refnos: Vec<RefnoEnum> = if let Some(list) = &args.refnos {
        list.split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(RefnoEnum::from)
            .filter(|r| r.is_valid())
            .collect()
    } else {
        let noun_filter = args
            .nouns
            .as_deref()
            .map(|list| {
                let quoted = list
                    .split(',')
                    .map(|s| s.trim().to_uppercase())
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("'{s}'"))
                    .collect::<Vec<_>>()
                    .join(",");
                format!(" AND noun IN [{quoted}]")
            })
            .unwrap_or_default();
        let sql = format!(
            "SELECT VALUE id FROM pe WHERE dbnum = {} AND !deleted{} ORDER BY rand() LIMIT {};",
            args.dbnum, noun_filter, args.sample
        );
        let mut response = SUL_DB.query(sql).await?;
        response.take(0)?
    };
    anyhow::ensure!(!refnos.is_empty(), "采样为空：pe 里没有该 dbnum 的存活元素");
    println!("[probe] samples={}", refnos.len());

    let dump_keys: Vec<String> = args
        .dump_keys
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty())
        .collect();
    let mut dump_seen: BTreeMap<String, usize> = BTreeMap::new();

    let mut tally = Tally::default();
    let mut detail_budget = args.max_detail;
    let mut neutral_detail_budget = args.max_detail;
    let mut direct_us = 0u128;
    let mut db_us = 0u128;
    let mut cross_db_owners = 0usize;
    let mut owner_chains_within_db = 0usize;
    let mut cross_db_climbs: Vec<String> = vec![];
    let mut owner_failures: Vec<String> = vec![];
    let mut cross_db_refs = 0usize;
    let mut cross_db_ref_samples: Vec<String> = vec![];
    let mut cross_db_ref_failures: BTreeMap<String, usize> = BTreeMap::new();
    let mut cross_db_ref_fail_samples: Vec<String> = vec![];

    for refno in &refnos {
        let t = Instant::now();
        let db_map = aios_core::get_named_attmap(*refno).await;
        db_us += t.elapsed().as_micros();

        let t = Instant::now();
        let direct = store.attrs_in(args.dbnum, refno.refno());
        direct_us += t.elapsed().as_micros();

        let (direct, db_map) = match (direct, db_map) {
            (Err(error), Err(_)) => {
                // 两侧皆无。direct 侧仍要留下是哪一类错，否则「都没有」会把
                // 「读坏了」和「本来就没有」混成一件事。
                *tally.read_fail_hist.entry(error_kind(&error)).or_default() += 1;
                continue;
            }
            (Err(error), Ok(_)) => {
                *tally.read_fail_hist.entry(error_kind(&error)).or_default() += 1;
                if tally.read_fail_samples.len() < 12 {
                    tally.read_fail_samples.push(format!("{refno}: {error}"));
                }
                tally.missing_direct.push(refno.to_string());
                continue;
            }
            (Ok(_), Err(e)) => {
                tally.missing_db.push(format!("{refno}: {e}"));
                continue;
            }
            (Ok(direct), Ok(db_map)) => (direct, db_map),
        };

        let type_name = db_map
            .map
            .get("TYPE")
            .and_then(|v| match v {
                NamedAttrValue::StringType(s) | NamedAttrValue::WordType(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default();
        *tally.noun_hist.entry(type_name.clone()).or_default() += 1;
        record_side_channels(&mut tally, &direct);

        if !dump_keys.is_empty()
            && let Ok(extraction) = store.extraction(args.dbnum, refno.refno())
        {
            for attribute in &extraction.attributes {
                if !dump_keys.iter().any(|k| k == &attribute.name) {
                    continue;
                }
                let seen = dump_seen.entry(attribute.name.clone()).or_default();
                *seen += 1;
                if *seen <= 10 {
                    println!(
                        "[dump] {refno} {} type={type_name} storage={} status={:?} raw={:?} db={:?}",
                        attribute.name,
                        attribute.storage_type_code,
                        attribute.status,
                        attribute.value,
                        db_map.map.get(&attribute.name)
                    );
                }
            }
        }

        if args.owner_climb {
            match climb_owners(&store, args.dbnum, &direct) {
                Ok(Some(hop)) => {
                    cross_db_owners += 1;
                    if cross_db_climbs.len() < 8 {
                        cross_db_climbs.push(hop);
                    }
                }
                Ok(None) => owner_chains_within_db += 1,
                Err(error) => owner_failures.push(format!("{refno}: {error}")),
            }
            for (key, target) in cross_db_references(&store, args.dbnum, &direct) {
                match target {
                    Ok(line) => {
                        cross_db_refs += 1;
                        if cross_db_ref_samples.len() < 8 {
                            cross_db_ref_samples.push(line);
                        }
                    }
                    Err(error) => {
                        *cross_db_ref_failures.entry(key).or_default() += 1;
                        if cross_db_ref_fail_samples.len() < 8 {
                            cross_db_ref_fail_samples.push(format!("{refno}: {error}"));
                        }
                    }
                }
            }
        }

        let d = diff_maps(&direct.map, &db_map);
        for k in &d.only_db {
            *tally.only_db_hist.entry(k.clone()).or_default() += 1;
        }
        for k in &d.only_db_neutral {
            *tally.only_db_neutral_hist.entry(k.clone()).or_default() += 1;
        }
        for (k, dv, bv) in &d.neutral_mismatches {
            *tally.neutral_mismatch_hist.entry(k.clone()).or_default() += 1;
            if neutral_detail_budget > 0 {
                neutral_detail_budget -= 1;
                println!("  [neutral-diff] {refno} {k}: direct={dv} db={bv}");
            }
        }
        for k in &d.only_direct {
            *tally.only_direct_hist.entry(k.clone()).or_default() += 1;
        }
        for k in &d.lossy_keys {
            *tally.lossy_hist.entry(k.clone()).or_default() += 1;
        }

        if d.mismatches.is_empty() && d.only_db.is_empty() {
            if !d.only_direct.is_empty() {
                tally.superset_only += 1;
            } else if d.normalized_keys > 0
                || !d.lossy_keys.is_empty()
                || !d.only_db_neutral.is_empty()
                || !d.neutral_mismatches.is_empty()
            {
                tally.normalized_only += 1;
            } else {
                tally.identical += 1;
            }
        } else {
            tally.diverged += 1;
            *tally
                .diverged_noun_hist
                .entry(type_name.clone())
                .or_default() += 1;
            for (k, dv, bv) in &d.mismatches {
                *tally.mismatch_hist.entry(k.clone()).or_default() += 1;
                if detail_budget > 0 {
                    detail_budget -= 1;
                    println!("  [diff] {refno} {k}: direct={dv} db={bv}");
                }
            }
            for k in &d.only_db {
                if detail_budget > 0 {
                    detail_budget -= 1;
                    println!("  [only-db] {refno} {k}: db={:?}", db_map.map.get(k));
                }
            }
        }
    }

    let n = refnos.len().max(1) as u128;
    println!("\n===== direct_attmap_probe 汇总（e3d-io） =====");
    println!(
        "样本 {}：完全一致 {}｜归一后一致 {}｜direct 超集（DB 行缺键）{}｜真值冲突 {}｜direct 缺失 {}｜DB 缺失 {}",
        refnos.len(),
        tally.identical,
        tally.normalized_only,
        tally.superset_only,
        tally.diverged,
        tally.missing_direct.len(),
        tally.missing_db.len()
    );
    println!("样本 noun 分布：{:?}", tally.noun_hist);
    if !tally.read_fail_hist.is_empty() {
        println!("direct 读失败（按类）：{:?}", tally.read_fail_hist);
        for s in &tally.read_fail_samples {
            println!("  [read-fail] {s}");
        }
    }
    if !tally.diverged_noun_hist.is_empty() {
        println!("有真值冲突的 noun：{:?}", tally.diverged_noun_hist);
    }
    if !tally.lossy_hist.is_empty() {
        println!(
            "DB 读损耗键（DB 视图为空、direct 更全；生成侧今日读到的即空）：{:?}",
            tally.lossy_hist
        );
    }
    if !tally.only_db_hist.is_empty() {
        println!("仅 DB 有的模型相关键（次数）：{:?}", tally.only_db_hist);
    }
    if !tally.only_db_neutral_hist.is_empty() {
        println!("仅 DB 有的模型中立键：{:?}", tally.only_db_neutral_hist);
    }
    if !tally.only_direct_hist.is_empty() {
        println!(
            "仅 direct 有的键（DB 行历史未写入；生成读侧得 None）：{:?}",
            tally.only_direct_hist
        );
    }
    if !tally.mismatch_hist.is_empty() {
        println!("值不匹配键（次数）：{:?}", tally.mismatch_hist);
    }
    if !tally.neutral_mismatch_hist.is_empty() {
        println!(
            "值不匹配但模型中立的键（不计冲突，但**每个都得有账**——CRFA 见 ISSUE-027）：{:?}",
            tally.neutral_mismatch_hist
        );
    }
    println!("\n--- 转换器回执 ---");
    if !tally.outside_schema_hist.is_empty() {
        println!(
            "schema 外键（DB 读侧同样跳过）：{:?}",
            tally.outside_schema_hist
        );
    }
    if !tally.unset_hist.is_empty() {
        println!(
            "文件说 unset（E3D 逻辑未设，未编默认值塞入）：{:?}",
            tally.unset_hist
        );
    }
    if !tally.view_divergence_hist.is_empty() {
        println!(
            "视图分歧（schema 声明文本、文件是字/整数；DB 读侧同样读不出来，交原样）：{:?}",
            tally.view_divergence_hist
        );
    }
    if !tally.shape_conflict_hist.is_empty() {
        println!(
            "**形状冲突（声明成数值/引用却对不上形状，未猜——这一类算错）：{:?}",
            tally.shape_conflict_hist
        );
    }
    if !tally.undecoded_hist.is_empty() {
        println!("描述符在场但没解出值：{:?}", tally.undecoded_hist);
    }
    if args.owner_climb {
        println!(
            "\n跨库 owner 上溯：{} 条链走到了别的 dbnum 并读出属性，{} 条到根都没出本库，{} 条失败",
            cross_db_owners,
            owner_chains_within_db,
            owner_failures.len()
        );
        for c in &cross_db_climbs {
            println!("  [owner] {c}");
        }
        for f in owner_failures.iter().take(12) {
            println!("  [owner-fail] {f}");
        }
        println!(
            "跨库引用换库：{} 条引用指向别的 dbnum 并读出了属性，{} 条失败",
            cross_db_refs,
            cross_db_ref_failures.values().sum::<usize>()
        );
        for s in &cross_db_ref_samples {
            println!("  [xref] {s}");
        }
        if !cross_db_ref_failures.is_empty() {
            println!("  [xref-fail] 按属性名：{cross_db_ref_failures:?}");
            for s in &cross_db_ref_fail_samples {
                println!("  [xref-fail] {s}");
            }
        }
    }
    println!(
        "\n耗时：direct 平均 {}us/元素，DB 平均 {}us/元素｜会话池 {:?}",
        direct_us / n,
        db_us / n,
        store
    );

    let fatal = tally.diverged
        + tally.missing_direct.len()
        + tally.missing_db.len()
        + tally.shape_conflict_hist.values().sum::<usize>();
    if fatal > 0 {
        println!("\n[probe] 存在 {fatal} 处真值冲突 / 缺失 / 形状冲突，direct 供给等价性不通过。");
        std::process::exit(1);
    }
    println!("\n[probe] 无真值冲突：direct 读出的属性是 DB 读出的超集。direct 供给等价性通过。");
    Ok(())
}

/// 顺 OWNER 链一路上溯，直到走进**另一个 dbnum**（ADR-053 R2：DESI 元素的 owner 在
/// SITE 库）。
///
/// 单看一个元素的 OWNER 是验不出跨库的：随机抽到的都是叶子，它们的 owner 就在本库。
/// 跨库那一跳发生在树的顶上（ZONE → SITE），所以必须一路爬。每一跳都走
/// `DirectStore::dbnum_of`（ref0 反查）换库，换完还要真把属性读出来——只换不读证明不了
/// 那个库能开。
///
/// `Ok(Some(路径))` = 走到了别的库并读出属性；`Ok(None)` = 到根都在本库。
fn climb_owners(
    store: &DirectStore,
    start_db: i32,
    start: &DirectAttrs,
) -> anyhow::Result<Option<String>> {
    /// SITE 之上就是 WORL，从最深的构件数上来也用不了这么多跳。
    const MAX_HOPS: usize = 16;

    let mut current_db = start_db;
    let mut path = vec![format!("{}@{start_db}", refno_of(start))];
    let mut owner = match owner_of(start) {
        Some(owner) => owner,
        None => return Ok(None),
    };

    for _ in 0..MAX_HOPS {
        let owner_db = store.dbnum_of(owner)?;
        let attrs = store.attrs_in(owner_db, owner)?;
        path.push(format!("{owner}@{owner_db}"));

        if owner_db != current_db {
            // 换库后确实读出了属性，才算这一跳成立。
            let noun = match attrs.map.map.get("TYPE") {
                Some(NamedAttrValue::StringType(s) | NamedAttrValue::WordType(s)) => s.clone(),
                _ => String::new(),
            };
            return Ok(Some(format!("{} → {noun}", path.join(" → "))));
        }
        current_db = owner_db;

        owner = match owner_of(&attrs) {
            Some(next) => next,
            None => return Ok(None),
        };
    }
    anyhow::bail!("owner 链走满 {MAX_HOPS} 跳还没到根：{}", path.join(" → "))
}

/// 元素身上指向**别的库**的引用属性，逐个换库读一遍。
///
/// 这才是这套工程里真正的跨库路径。owner 链是不跨库的（opus-5-20 在 ams8000 上量到
/// 6605/6605 全在本库），跨库的是 `SPRE` / `LSTU` / `PSPE` / `CATR` 这些命名引用——
/// 同一份实测里，8000 库的命名引用属性 82% 指向别的库，主要是目录库 5052。目录库不入
/// 水位表，所以这条同时验的是 `DirectStore::pin_from_locator`。
fn cross_db_references(
    store: &DirectStore,
    home_db: i32,
    attrs: &DirectAttrs,
) -> Vec<(String, anyhow::Result<String>)> {
    let mut out = Vec::new();
    for (key, value) in &attrs.map.map {
        if key == "OWNER" || key == "REFNO" {
            continue;
        }
        let NamedAttrValue::RefU64Type(target) = value else {
            continue;
        };
        if !target.is_valid() {
            continue;
        }
        // 反查不到归属的引用不算失败：本工程之外的库本来就不在定位器里。
        let Ok(Some(target_db)) = store.locator_dbnum(*target) else {
            continue;
        };
        if target_db == home_db {
            continue;
        }
        out.push((
            key.clone(),
            store
                .attrs(*target)
                .map(|read| {
                    let noun = match read.map.map.get("TYPE") {
                        Some(NamedAttrValue::StringType(s) | NamedAttrValue::WordType(s)) => {
                            s.clone()
                        }
                        _ => String::new(),
                    };
                    format!(
                        "{key} → {target}@{target_db} ({noun}, pinned_sesno={:?})",
                        store.pinned_sesno(target_db)
                    )
                })
                .map_err(|error| anyhow::anyhow!("{key} → {target}@{target_db}: {error}")),
        ));
    }
    out
}

fn owner_of(attrs: &DirectAttrs) -> Option<aios_core::RefU64> {
    match attrs.map.map.get("OWNER") {
        Some(NamedAttrValue::RefU64Type(owner)) if owner.is_valid() => Some(*owner),
        _ => None,
    }
}

fn refno_of(attrs: &DirectAttrs) -> String {
    match attrs.map.map.get("REFNO") {
        Some(NamedAttrValue::RefU64Type(refno)) => refno.to_string(),
        _ => "?".to_string(),
    }
}

/// 把错误收敛成一个可计数的类名——明细另留样本，直方图要能看出「哪一类多」。
fn error_kind(error: &aios_database::data_interface::direct_store::DirectStoreError) -> String {
    use aios_database::data_interface::direct_store::DirectStoreError as E;
    match error {
        E::NotPinned { .. } => "NotPinned".into(),
        E::UnresolvedRef0 { .. } => "UnresolvedRef0".into(),
        E::Session { .. } => "Session".into(),
        E::NoSuchElement { .. } => "NoSuchElement".into(),
        E::FileReplaced { .. } => "FileReplaced".into(),
        E::NoFileForDbnum { .. } => "NoFileForDbnum".into(),
        E::Extract { detail, .. } => format!("Extract: {detail}"),
        E::Convert { source, .. } => format!("Convert: {source}"),
        E::Poisoned { .. } => "Poisoned".into(),
    }
}
