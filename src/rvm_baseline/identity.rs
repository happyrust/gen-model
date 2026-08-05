//! RVM 组名 → PDMS 身份（noun / owner / 序号）的解析。
//!
//! RVM 里组名有两种形态：
//!   命名元素   `/C-IY-1R330-B`                       —— 直接就是 E3D NAME
//!   未命名元素 `BEND 3 of BRANCH /C-IY-1R330-B`      —— E3D 默认命名
//!
//! 默认命名可以嵌套，例如
//!   `SNOUT 1 of TMPLATE 1 of EQUIPMENT /03SKID3-EQUIP1`
//! 这里只解析最外层，owner 描述原样保留，由后续身份解析器逐级消歧。

/// E3D 默认命名的解析结果。
#[derive(Debug, Clone)]
pub struct DefaultName {
    /// 名词全称，如 BEND / FLANGE。
    pub noun_full: String,
    /// 在 owner 的同名词子序列中的序号，从 1 开始。
    pub ordinal: usize,
    /// owner 描述，可能是 `BRANCH /x/B1`，也可能是又一层默认命名。
    pub owner_desc: String,
}

/// 解析 `<NOUN> <n> of <OWNER_DESC>`；不是默认命名时返回 None。
pub fn parse_default_name(name: &str) -> Option<DefaultName> {
    let (left, right) = name.split_once(" of ")?;
    let (noun_full, ordinal_str) = left.rsplit_once(' ')?;
    let ordinal: usize = ordinal_str.trim().parse().ok()?;
    if ordinal == 0 {
        return None;
    }
    Some(DefaultName {
        noun_full: noun_full.trim().to_string(),
        ordinal,
        owner_desc: right.trim().to_string(),
    })
}

/// 从 owner 描述里取出命名 owner：`BRANCH /x/B1` → `/x/B1`。
/// 只有 `/` 之前恰好是单个名词（不含空格）时才成立，否则说明 owner 自身
/// 也是默认命名，需要再解析一层。
pub fn named_owner_from_desc(desc: &str) -> Option<&str> {
    let slash = desc.find('/')?;
    let prefix = desc[..slash].trim();
    if !prefix.is_empty() && !prefix.contains(' ') {
        Some(desc[slash..].trim())
    } else {
        None
    }
}

/// 名词全称 → PDMS 短名词（与站点库 `pe.noun` 形态一致）。
///
/// 兜底的「截前 4 个字符」对多数名词成立，但 PDMS 短名词并非一律 4 字：
/// SUPPORT 的站点库形态是 SUPPO（5 字，`delivery_unit_types` 默认集合里就有），
/// 截 4 会得到 join 不上的 SUPP。这类例外必须进显式映射表。
/// 有 ATT 时以其 TYPE 为权威，这里只是无 ATT 的退化路径。
pub fn full_noun_to_short(full: &str) -> String {
    match full.to_ascii_uppercase().as_str() {
        "FLANGE" => "FLAN",
        "ELBOW" => "ELBO",
        "REDUCER" => "REDU",
        "GASKET" => "GASK",
        "VALVE" => "VALV",
        "BRANCH" => "BRAN",
        "TUBING" | "TUBE" => "TUBI",
        "EQUIPMENT" => "EQUI",
        "NOZZLE" => "NOZZ",
        "COUPLING" => "COUP",
        "INSTRUMENT" => "INST",
        "ATTACHMENT" => "ATTA",
        "STRUCTURE" => "STRU",
        "FITTING" => "FITT",
        "SUPPORT" => "SUPPO",
        "HANGER" => "HANG",
        other => {
            return other.chars().take(4).collect::<String>();
        }
    }
    .to_string()
}

/// 从组名推断 PDMS 名词。命名元素推不出来（名字里没有类型信息），返回 None，
/// 留给后续按 name 查站点库 `pe` 的解析器补。
pub fn noun_from_name(name: &str) -> Option<String> {
    parse_default_name(name).map(|d| full_noun_to_short(&d.noun_full))
}

/// 由路径派生稳定 id。身份解析未接入时，两侧无法按真实 refno join，
/// 这个 id 至少保证同一份 RVM 重复导入结果一致、可复跑。
///
/// FNV-1a 64，避免为一个哈希再引一个依赖。
pub fn stable_id(dbnum: u32, path: &str) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in dbnum.to_le_bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    for byte in path.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    // 0 留作「无 parent」的哨兵，避免歧义。
    hash.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两种真实形态：普通默认命名，以及 owner 自身也是默认命名的嵌套形态。
    /// `split_once(" of ")` 只切最外层，嵌套的 owner 描述必须原样保留。
    #[test]
    fn default_names_parse_including_nested_owners() {
        let simple = parse_default_name("BEND 3 of BRANCH /C-IY-1R330-B").expect("simple");
        assert_eq!(simple.noun_full, "BEND");
        assert_eq!(simple.ordinal, 3);
        assert_eq!(simple.owner_desc, "BRANCH /C-IY-1R330-B");

        let nested = parse_default_name("SNOUT 1 of TMPLATE 1 of EQUIPMENT /03SKID3-EQUIP1")
            .expect("nested");
        assert_eq!(nested.noun_full, "SNOUT");
        assert_eq!(nested.ordinal, 1);
        assert_eq!(nested.owner_desc, "TMPLATE 1 of EQUIPMENT /03SKID3-EQUIP1");
    }

    /// 命名元素、序号非法、缺 " of " 的都不是默认命名。
    #[test]
    fn non_default_names_yield_none() {
        assert!(parse_default_name("/C-IY-1R330-B").is_none());
        assert!(
            parse_default_name("FLANGE 0 of BRANCH /x").is_none(),
            "序号从 1 起"
        );
        assert!(parse_default_name("FLANGE x of BRANCH /x").is_none());
        assert!(parse_default_name("JUSTONENAME").is_none());
    }

    /// 只有「单个名词 + 命名」才算命名 owner；owner 自身还是默认命名时要返回
    /// None 交给上层再剥一层，直接取 `/` 会把 `TMPLATE 1 of EQUIPMENT /x` 错认。
    #[test]
    fn named_owner_requires_a_single_noun_prefix() {
        assert_eq!(
            named_owner_from_desc("BRANCH /C-IY-1R330-B"),
            Some("/C-IY-1R330-B")
        );
        assert_eq!(named_owner_from_desc("TMPLATE 1 of EQUIPMENT /x"), None);
        assert_eq!(named_owner_from_desc("/x/B1"), None, "没有名词前缀不算");
        assert_eq!(named_owner_from_desc("BRANCH"), None, "没有命名部分不算");
    }

    /// 截断兜底对 5 字短名词失效（SUPPORT → 站点库是 SUPPO 不是 SUPP），
    /// 这类必须走显式映射；大小写不敏感；未知名词保持「截前 4」的旧口径。
    #[test]
    fn noun_shortening_covers_five_letter_pdms_nouns() {
        assert_eq!(full_noun_to_short("SUPPORT"), "SUPPO");
        assert_eq!(full_noun_to_short("support"), "SUPPO");
        assert_eq!(full_noun_to_short("HANGER"), "HANG");
        assert_eq!(full_noun_to_short("FLANGE"), "FLAN");
        assert_eq!(full_noun_to_short("TEE"), "TEE");
        assert_eq!(full_noun_to_short("Unknown"), "UNKN");
    }

    /// 同输入必须同 id（可复跑的根基）；dbnum 或路径任一不同 id 就不同；
    /// 0 是「无 parent」哨兵，永不产出。
    #[test]
    fn stable_ids_are_stable_and_never_zero() {
        let a = stable_id(8000, "/SITE/ZONE/PIPE/BRAN");
        assert_eq!(a, stable_id(8000, "/SITE/ZONE/PIPE/BRAN"));
        assert_ne!(a, stable_id(8001, "/SITE/ZONE/PIPE/BRAN"));
        assert_ne!(a, stable_id(8000, "/SITE/ZONE/PIPE/BRAN2"));
        assert_ne!(stable_id(0, ""), 0);
    }
}
