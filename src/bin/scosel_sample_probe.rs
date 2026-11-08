//! 一次性取证：把 AvevaPlantSample 里 GPSET 的 `SCOSEL` 原始 int 数组打出来，用来定
//! `100009,<N>` 与 `100015,<len>` 的长度口径（取证文档 §2.1 / §2.2 说 N 不含自身、len 含自身，
//! 而 `scosel.rs` 的实现按相反口径写的，两者必须有一个是错的）。
//!
//! 只读，不改库。
//!
//! ```text
//! cargo run --release --bin scosel_sample_probe -- --limit 6
//! ```

use aios_core::RefU64;
use aios_database::data_interface::direct_tree::DirectTreeService;
use aios_database::data_interface::mdb_read_store::MdbReadProvider;
use e3d_io::record::explicit::ExplicitValue;

fn main() -> anyhow::Result<()> {
    let limit: usize = std::env::args()
        .skip_while(|arg| arg != "--limit")
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(6);

    let service = DirectTreeService::open("AvevaPlantSample", "ALL")?;
    let provider = MdbReadProvider::shared("AvevaPlantSample", "ALL")?;
    let store = provider.view()?;

    // 不依赖 `roots_of_nouns`（那是工作区未提交的改动），也不碰 service 的私有字段：
    // 直接问 store 要每个库的 WORLD（refno 的 word1 为 1 是库根约定），再读它的成员。
    // GPWL 是 WORLD 的直接成员，不在 SITE 下面。
    let mut worlds = Vec::new();
    let metadata = service.metadata()?;
    for db in provider.membership.element_databases() {
        if db.path.is_none() {
            continue;
        }
        let Some(world) = metadata.world_of(db.dbnum as u32) else {
            continue;
        };
        for member in store.members(world).unwrap_or_default() {
            worlds.push(member);
        }
    }
    println!("候选成员：{}", worlds.len());

    let scosel_hash = e3d_attlib::db1_hash("SCOSEL");
    let mut printed = 0;
    let mut sets = 0;
    let mut samples = Vec::new();
    for member in &worlds {
        let member_refno = *member;
        if !store
            .named_attmap(member_refno)
            .ok()
            .and_then(|attrs| attrs.get_as_string("TYPE"))
            .is_some_and(|noun| noun.trim().eq_ignore_ascii_case("GPWL"))
        {
            continue;
        }
        for set in service.resolved_children(member_refno)?.nodes.iter() {
            if !set.node.noun.eq_ignore_ascii_case("GPSET") {
                continue;
            }
            sets += 1;
            let set_refno: RefU64 = set.node.refno.refno();
            let snapshot = store.element_snapshot(set_refno)?;
            let Some(attr) = snapshot
                .explicit_attributes
                .iter()
                .find(|attr| attr.hash == scosel_hash)
            else {
                println!("{set_refno} {}: 无 SCOSEL", set.node.name);
                continue;
            };
            let words = match &attr.value {
                ExplicitValue::IntArray(words) => words.clone(),
                ExplicitValue::RawWords(words) => words.iter().map(|word| *word as i32).collect(),
                other => {
                    println!("{set_refno} {}: SCOSEL 不是数组 {other:?}", set.node.name);
                    continue;
                }
            };
            println!(
                "\n=== {set_refno} {} (type_code {}) 共 {} 字 ===",
                set.node.name,
                attr.type_code,
                words.len()
            );
            println!("{words:?}");
            // 逐字带下标，方便对着取证文档数偏移。
            for (at, word) in words.iter().enumerate() {
                print!("{at}:{word}  ");
            }
            println!();
            // 现有实现能不能解开真样本？这是本次取证要回答的问题。
            match aios_database::data_interface::scosel::decode(&words) {
                Ok(decoded) => println!("  decode -> {decoded:?}"),
                Err(error) => println!("  decode -> 失败：{error}"),
            }
            samples.push(serde_json::json!({
                "name": set.node.name,
                "refno": set_refno.to_string(),
                "words": words,
            }));
            printed += 1;
            if printed >= limit {
                break;
            }
        }
        if printed >= limit {
            break;
        }
    }
    println!("\n共 {sets} 个 GPSET，已打印 {printed}");

    if let Some(path) = std::env::args().skip_while(|arg| arg != "--out").nth(1) {
        std::fs::write(&path, serde_json::to_string_pretty(&samples)?)?;
        println!("样本写入 {path}");
    }
    Ok(())
}
