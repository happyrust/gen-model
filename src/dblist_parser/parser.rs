//! dblist 文件解析器
//! 
//! 解析 PDMS dblist 文本格式，提取 FRMWORK、PANEL、GENSEC 等元素信息

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// PDMS 元素类型
#[derive(Debug, Clone, PartialEq)]
pub enum ElementType {
    FrmFramework,
    Panel,
    Ploop,
    Pavert,
    Gensec,
    Spine,
    Poinsp,
    Jldatum,
    Pldatum,
    Fixing,
    Rladdr,
    Handra,
    Rpath,
    Pointr,
}

impl ElementType {
    fn from_str(s: &str) -> Result<Self> {
        match s.to_uppercase().as_str() {
            "FRMWORK" => Ok(ElementType::FrmFramework),
            "PANEL" => Ok(ElementType::Panel),
            "PLOOP" => Ok(ElementType::Ploop),
            "PAVERT" => Ok(ElementType::Pavert),
            "GENSEC" => Ok(ElementType::Gensec),
            "SPINE" => Ok(ElementType::Spine),
            "POINSP" => Ok(ElementType::Poinsp),
            "JLDATUM" => Ok(ElementType::Jldatum),
            "PLDATUM" => Ok(ElementType::Pldatum),
            "FIXING" => Ok(ElementType::Fixing),
            "RLADDR" => Ok(ElementType::Rladdr),
            "HANDRA" => Ok(ElementType::Handra),
            "RPATH" => Ok(ElementType::Rpath),
            "POINTR" => Ok(ElementType::Pointr),
            _ => Err(anyhow!("Unknown element type: {}", s)),
        }
    }

    pub fn to_noun(&self) -> &'static str {
        match self {
            ElementType::FrmFramework => "FRMWORK",
            ElementType::Panel => "PANEL",
            ElementType::Ploop => "PLOOP",
            ElementType::Pavert => "PAVERT",
            ElementType::Gensec => "GENSEC",
            ElementType::Spine => "SPINE",
            ElementType::Poinsp => "POINSP",
            ElementType::Jldatum => "JLDATUM",
            ElementType::Pldatum => "PLDATUM",
            ElementType::Fixing => "FIXING",
            ElementType::Rladdr => "RLADDR",
            ElementType::Handra => "HANDRA",
            ElementType::Rpath => "RPATH",
            ElementType::Pointr => "POINTR",
        }
    }
}

/// PDMS 元素数据
#[derive(Debug, Clone)]
pub struct PdmsElement {
    pub element_type: ElementType,
    pub refno: (u32, u32), // (dbno, elno)
    pub attributes: HashMap<String, String>,
    pub children: Vec<PdmsElement>,
    pub position: Option<String>,
}

impl PdmsElement {
    pub fn new(element_type: ElementType, refno: (u32, u32)) -> Self {
        Self {
            element_type,
            refno,
            attributes: HashMap::new(),
            children: Vec::new(),
            position: None,
        }
    }

    pub fn get_id(&self) -> String {
        format!("pe:{}_{}", self.refno.0, self.refno.1)
    }
}

/// dblist 文件解析器
pub struct DblistParser {
    elements: Vec<PdmsElement>,
    current_element: Option<PdmsElement>,
    element_stack: Vec<PdmsElement>,
    current_dbno: u32,
    next_elno: u32,
}

impl DblistParser {
    pub fn new() -> Self {
        Self {
            elements: Vec::new(),
            current_element: None,
            element_stack: Vec::new(),
            current_dbno: 17496, // 默认数据库编号
            next_elno: 1,
        }
    }

    /// 从文件解析 dblist
    pub fn parse_file<P: AsRef<Path>>(file_path: P) -> Result<Vec<PdmsElement>> {
        let file = File::open(file_path)?;
        let reader = BufReader::new(file);
        let mut parser = Self::new();
        
        for line in reader.lines() {
            let line = line?;
            parser.parse_line(&line)?;
        }
        
        parser.finalize()
    }

    /// 解析单行内容
    fn parse_line(&mut self, line: &str) -> Result<()> {
        let trimmed = line.trim();
        
        // 跳过空行和注释
        if trimmed.is_empty() || trimmed.starts_with("--") || trimmed.starts_with('$') {
            return Ok(());
        }

        // 解析数据库编号（从文件名或特定行提取）
        if trimmed.contains("FRMW_") {
            if let Some(dbno_str) = trimmed.split("FRMW_").nth(1) {
                if let Some(dbno_part) = dbno_str.split('_').next() {
                    if let Some(dbno_part) = dbno_part.split_whitespace().next() {
                        if let Ok(dbno) = dbno_part.parse::<u32>() {
                            self.current_dbno = dbno;
                        }
                    }
                }
            }
        }

        // 解析 NEW 元素
        if trimmed.starts_with("NEW ") {
            self.start_new_element(trimmed)?;
        }
        // 解析属性
        else if let Some((key, value)) = self.parse_attribute(trimmed) {
            self.add_attribute(key, value)?;
        }
        // 解析 END
        else if trimmed == "END" {
            self.end_element()?;
        }

        Ok(())
    }

    /// 开始解析新元素
    fn start_new_element(&mut self, line: &str) -> Result<()> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            return Ok(());
        }

        let element_type = ElementType::from_str(parts[1])?;
        let refno = (self.current_dbno, self.next_elno);
        self.next_elno += 1;

        let element = PdmsElement::new(element_type, refno);
        
        if let Some(current) = self.current_element.take() {
            self.element_stack.push(current);
        }
        
        self.current_element = Some(element);
        Ok(())
    }

    /// 解析属性行
    fn parse_attribute(&self, line: &str) -> Option<(String, String)> {
        if let Some(space_pos) = line.find(' ') {
            let key = line[..space_pos].trim().to_string();
            let value = line[space_pos..].trim().to_string();
            
            if !key.is_empty() && !value.is_empty() {
                return Some((key, value));
            }
        }
        None
    }

    /// 添加属性到当前元素
    fn add_attribute(&mut self, key: String, value: String) -> Result<()> {
        if let Some(ref mut element) = self.current_element {
            // 特殊处理位置信息
            if key == "POS" {
                element.position = Some(value.clone());
            }
            element.attributes.insert(key, value);
        }
        Ok(())
    }

    /// 结束当前元素
    fn end_element(&mut self) -> Result<()> {
        if let Some(element) = self.current_element.take() {
            if let Some(mut parent) = self.element_stack.pop() {
                parent.children.push(element);
                self.current_element = Some(parent);
            } else {
                self.elements.push(element);
            }
        }
        Ok(())
    }

    /// 完成解析并返回结果
    fn finalize(mut self) -> Result<Vec<PdmsElement>> {
        // 处理未结束的元素
        while let Some(element) = self.current_element.take() {
            if let Some(mut parent) = self.element_stack.pop() {
                parent.children.push(element);
                self.current_element = Some(parent);
            } else {
                self.elements.push(element);
                break;
            }
        }
        
        Ok(self.elements)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_element_type_from_str() {
        assert_eq!(ElementType::from_str("FRMWORK").unwrap(), ElementType::FrmFramework);
        assert_eq!(ElementType::from_str("GENSEC").unwrap(), ElementType::Gensec);
        assert!(ElementType::from_str("UNKNOWN").is_err());
    }

    #[test]
    fn test_parse_simple_dblist() {
        let dblist_content = r#"
NEW FRMWORK
DESC 'Test Framework'
BUIL false
GRADE 0

NEW PANEL
POS W 100mm N 200mm D 300mm
BUIL false

END
END
"#;
        
        // 这里可以添加更详细的测试
        // 由于 parse_line 是私有方法，可以通过 parse_file 来测试
    }
}
