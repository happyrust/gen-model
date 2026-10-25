//! 命名规则（功能文档 §18.9）。
//!
//! 纯函数、无 DB 依赖——存在性探测/取 CE 名由调用方注入闭包，便于单测。

/// 支管自动命名（pipecreation :1406 `branchName` 语义）。
///
/// `root` = PIPE 全名；`sep` = idSeparator（通常 `/`）；`rule` 为编号格式
/// （`"B{}"` 对应 E3D 的 `idRule='B'&index`）。按 1..=member_count+1 逐号
/// 探测 `exists`，取第一个空闲名；全占返回 `None`。
pub fn branch_name(
    root: &str,
    member_count: usize,
    sep: &str,
    rule: &str,
    exists: impl Fn(&str) -> bool,
) -> Option<String> {
    for index in 1..=member_count + 1 {
        let name = format!("{root}{sep}{}", rule.replace("{i}", &index.to_string()));
        if !exists(&name) {
            return Some(name);
        }
    }
    None
}

/// 分段派生名 `<base>-Split(<n>)`（`splitpipe.pmlfnc`：`pipeName.before('-Split(')`
/// 先剥旧后缀再 `handle(41,12)` 冲突自增）。
///
/// 冲突时 `n` 自增重试；`exists` 探测由调用方注入。
pub fn split_name(base: &str, start_index: u32, exists: impl Fn(&str) -> bool) -> Option<String> {
    let stem = match base.find("-Split(") {
        Some(i) => &base[..i],
        None => base,
    };
    (start_index..start_index + 1000)
        .map(|n| format!("{stem}-Split({n})"))
        .find(|name| !exists(name))
}

/// 新元素名归一化（`!!checkName(..., 'NEW')` 语义，§18.9）。
///
/// - `*` → STAR 名（空时回落 CE 名，但 PML 侧会先弹 confirm——本函数只替换）
/// - `^` → 属主名；`~` → CE 名（须以 `/` 开头，否则原样保留非法输入）
/// - 纯数字/未命名 → 原样返回（调用方走自动命名）
/// - 含 `$@`/`$$` → `Err`（非法字符段）
/// - 其余：删空格、补 `/` 前缀
///
/// `ce_name`/`parent_name`/`star_name` 由调用方提供；返回 `None` 表示未命名
/// （由调用方走规则引擎）。
pub fn normalise_new_name(
    input: &str,
    ce_name: &str,
    parent_name: &str,
    star_name: &str,
) -> Result<Option<String>, String> {
    let mut n = input.trim().to_string();
    if n.is_empty() || n.chars().all(|c| c.is_ascii_digit()) {
        return Ok(None);
    }
    if n == "*" {
        return Ok(Some(if star_name.is_empty() { ce_name.to_string() } else { star_name.to_string() }));
    }
    if n.starts_with('~') && !n.starts_with('/') {
        n = ce_name.to_string();
    }
    if n.starts_with('^') && !n.starts_with('/') {
        n = parent_name.to_string();
    }
    if n.contains("$@") || n.contains("$$") {
        return Err("NAME 不能含 $@ 或 $$ 字符段".into());
    }
    n.retain(|c| c != ' ');
    if !n.starts_with('/') {
        n = format!("/{n}");
    }
    Ok(Some(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_name_first_free() {
        // B1、B2 已占 → 取 B3（E3D 从 1 起按成员数探测）。
        let name = branch_name("/PIPE1", 5, "/", "B{i}", |n| n == "/PIPE1/B1" || n == "/PIPE1/B2");
        assert_eq!(name.as_deref(), Some("/PIPE1/B3"));
    }

    #[test]
    fn split_name_increments_on_conflict() {
        let name = split_name("/PIPE1/B1", 1, |n| n == "/PIPE1/B1-Split(1)");
        assert_eq!(name.as_deref(), Some("/PIPE1/B1-Split(2)"));
    }

    #[test]
    fn normalise_star_caret_tilde() {
        assert_eq!(normalise_new_name("*", "/CE", "/PAR", "/STAR").unwrap(), Some("/STAR".into()));
        assert_eq!(normalise_new_name("*", "/CE", "/PAR", "").unwrap(), Some("/CE".into()));
        assert_eq!(normalise_new_name("~", "/CE", "/PAR", "").unwrap(), Some("/CE".into()));
        assert_eq!(normalise_new_name("^", "/CE", "/PAR", "").unwrap(), Some("/PAR".into()));
    }

    #[test]
    fn normalise_invalid_and_unnamed() {
        assert!(normalise_new_name("/BAD$@X", "", "", "").is_err());
        assert_eq!(normalise_new_name("", "", "", "").unwrap(), None);
        assert_eq!(normalise_new_name("123", "", "", "").unwrap(), None);
        assert_eq!(normalise_new_name("pipe 1", "", "", "").unwrap(), Some("/pipe1".into()));
    }
}
