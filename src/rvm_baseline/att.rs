//! ATT 属性文件解析。
//!
//! E3D 的 Attribute Dump（`cdxattdump` → `mattdump()`）产出的文本格式：
//!
//! ```text
//! AVEVA_Attributes_File v1.0 , start: NEW , end: END , name_end: := , sep: &end&
//! NEW Header Information
//!   Source:= AVEVA E3D Design Data &end& Date:= 04 Aug 2026 &end& Time:= 17:50
//! END
//! NEW /C-IY-1R330-B
//!                   NAME:=  /C-IY-1R330-B
//!                   TYPE:=  BRAN
//! END
//!   NEW FTUBE 1 of BRANCH /C-IY-1R330-B
//!             NAME:=  =24384/22405
//!             TYPE:=  FTUB
//!            OWNER:=  /C-IY-1R330-B
//! END
//! ```
//!
//! 对身份解析最关键的一点：**未命名元素的 `NAME` 就是 `=ref0/ref1` 形式的真实
//! refno**，不用再去站点库反查。命名元素的 `NAME` 是名字，refno 不在 ATT 里。

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

/// 属性分隔符，一行里可能塞多组 `KEY:= VALUE`。
const PAIR_SEPARATOR: &str = "&end&";
const KEY_VALUE_SEPARATOR: &str = ":=";

pub type Section = BTreeMap<String, String>;

#[derive(Debug, Default, Clone)]
pub struct AttIndex {
    sections: BTreeMap<String, Section>,
}

impl AttIndex {
    pub fn load(paths: &[impl AsRef<Path>]) -> Result<Self> {
        let mut index = AttIndex::default();
        for path in paths {
            let path = path.as_ref();
            let text = fs::read_to_string(path)
                .with_context(|| format!("读取 ATT 文件失败: {}", path.display()))?;
            index.merge(&text);
        }
        Ok(index)
    }

    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    pub fn len(&self) -> usize {
        self.sections.len()
    }

    pub fn get(&self, section: &str) -> Option<&Section> {
        self.sections.get(section)
    }

    /// 导出根元素名，取自 Header Information 的 `Element` 字段。
    pub fn root_element(&self) -> Option<&str> {
        self.get("Header Information")
            .and_then(|s| s.get("Element"))
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
    }

    fn merge(&mut self, text: &str) {
        let mut current: Option<(String, Section)> = None;

        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }

            if let Some(name) = line.strip_prefix("NEW ") {
                // 上一节没收尾就遇到新节，按已结束处理，别把属性串到下一节去。
                if let Some((key, section)) = current.take() {
                    self.sections.entry(key).or_insert(section);
                }
                current = Some((name.trim().to_string(), Section::new()));
                continue;
            }

            if line == "END" {
                if let Some((key, section)) = current.take() {
                    // 同名 section 只保留第一份：E3D 对重名元素会重复输出，
                    // 后面的覆盖前面只会让结果不稳定。
                    self.sections.entry(key).or_insert(section);
                }
                continue;
            }

            let Some((_, section)) = current.as_mut() else {
                continue;
            };
            for pair in line.split(PAIR_SEPARATOR) {
                let Some((key, value)) = pair.split_once(KEY_VALUE_SEPARATOR) else {
                    continue;
                };
                let key = key.trim();
                if key.is_empty() {
                    continue;
                }
                section.insert(key.to_string(), value.trim().to_string());
            }
        }

        if let Some((key, section)) = current.take() {
            self.sections.entry(key).or_insert(section);
        }
    }
}

/// 把 `=24384/22405` 解析成 `24384/22405`；不是这个形态则返回 None。
///
/// `=0/0` 是 PDMS 的空引用，不能当身份用。
pub fn refno_from_att_name(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let body = trimmed.strip_prefix('=')?.trim();
    let (ref0, ref1) = body.split_once('/')?;
    let ref0: u32 = ref0.trim().parse().ok()?;
    let ref1: u32 = ref1.trim().parse().ok()?;
    if ref0 == 0 || ref1 == 0 {
        return None;
    }
    Some(format!("{ref0}/{ref1}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 未命名元素的 NAME 是 `=ref0/ref1` 真实身份；命名元素的 NAME 是名字；
    /// `=0/0` 空引用与残缺形态都不能当身份。
    #[test]
    fn att_names_resolve_to_refno_only_for_the_reference_form() {
        assert_eq!(
            refno_from_att_name("=24384/22405").as_deref(),
            Some("24384/22405")
        );
        assert_eq!(
            refno_from_att_name("  =24384/22405  ").as_deref(),
            Some("24384/22405"),
            "ATT 值两侧常有对齐空白"
        );
        assert_eq!(refno_from_att_name("/C-IY-1R330-B"), None, "命名元素");
        assert_eq!(refno_from_att_name("=0/0"), None, "空引用");
        assert_eq!(refno_from_att_name("=24384/0"), None);
        assert_eq!(refno_from_att_name("=24384"), None, "缺 ref1");
        assert_eq!(refno_from_att_name("=a/b"), None);
    }

    /// 解析器的三条容错纪律：NEW 未收尾遇到下一个 NEW 按已结束处理；同名节
    /// 保留第一份（E3D 对重名元素会重复输出）；一行多组 `KEY:= VALUE` 拆开。
    #[test]
    fn att_sections_parse_with_the_documented_tolerances() {
        let mut index = AttIndex::default();
        index.merge(
            "NEW Header Information\n\
             \x20 Source:= AVEVA E3D &end& Element:= /C-IY-1R330-B\n\
             END\n\
             NEW FTUBE 1 of BRANCH /C-IY-1R330-B\n\
             \x20 NAME:=  =24384/22405 &end& TYPE:=  FTUB\n\
             NEW FTUBE 1 of BRANCH /C-IY-1R330-B\n\
             \x20 NAME:=  =99999/99999\n\
             END\n",
        );
        assert_eq!(index.root_element(), Some("/C-IY-1R330-B"));
        let first = index
            .get("FTUBE 1 of BRANCH /C-IY-1R330-B")
            .expect("section");
        assert_eq!(
            first.get("NAME").map(String::as_str),
            Some("=24384/22405"),
            "同名节必须保留第一份，后面的重复输出不能覆盖"
        );
        assert_eq!(first.get("TYPE").map(String::as_str), Some("FTUB"));
    }
}
