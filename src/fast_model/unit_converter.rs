//! 单位转换器模块
//!
//! 提供灵活的单位转换功能，支持多种长度单位之间的转换。

use serde::{Deserialize, Serialize};
use std::fmt;

/// 支持的长度单位
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LengthUnit {
    /// 毫米 (mm)
    Millimeter,
    /// 厘米 (cm)
    Centimeter,
    /// 分米 (dm)
    Decimeter,
    /// 米 (m)
    Meter,
    /// 英寸 (in)
    Inch,
    /// 英尺 (ft)
    Foot,
    /// 码 (yd)
    Yard,
}

impl LengthUnit {
    /// 获取单位名称
    pub fn name(&self) -> &'static str {
        match self {
            LengthUnit::Millimeter => "mm",
            LengthUnit::Centimeter => "cm",
            LengthUnit::Decimeter => "dm",
            LengthUnit::Meter => "m",
            LengthUnit::Inch => "in",
            LengthUnit::Foot => "ft",
            LengthUnit::Yard => "yd",
        }
    }

    /// 获取单位全名
    pub fn full_name(&self) -> &'static str {
        match self {
            LengthUnit::Millimeter => "millimeter",
            LengthUnit::Centimeter => "centimeter",
            LengthUnit::Decimeter => "decimeter",
            LengthUnit::Meter => "meter",
            LengthUnit::Inch => "inch",
            LengthUnit::Foot => "foot",
            LengthUnit::Yard => "yard",
        }
    }

    /// 获取相对于米的转换因子
    pub fn to_meter_factor(&self) -> f32 {
        match self {
            LengthUnit::Millimeter => 0.001,
            LengthUnit::Centimeter => 0.01,
            LengthUnit::Decimeter => 0.1,
            LengthUnit::Meter => 1.0,
            LengthUnit::Inch => 0.0254,
            LengthUnit::Foot => 0.3048,
            LengthUnit::Yard => 0.9144,
        }
    }

    /// 从字符串解析单位
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "mm" | "millimeter" | "millimeters" => Ok(LengthUnit::Millimeter),
            "cm" | "centimeter" | "centimeters" => Ok(LengthUnit::Centimeter),
            "dm" | "decimeter" | "decimeters" => Ok(LengthUnit::Decimeter),
            "m" | "meter" | "meters" => Ok(LengthUnit::Meter),
            "in" | "inch" | "inches" => Ok(LengthUnit::Inch),
            "ft" | "foot" | "feet" => Ok(LengthUnit::Foot),
            "yd" | "yard" | "yards" => Ok(LengthUnit::Yard),
            _ => Err(format!("不支持的单位: {}", s)),
        }
    }
}

impl fmt::Display for LengthUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// 单位转换器
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnitConverter {
    /// 源单位（PDMS 数据的原始单位）
    pub source_unit: LengthUnit,
    /// 目标单位（导出时的单位）
    pub target_unit: LengthUnit,
}

impl UnitConverter {
    /// 创建新的单位转换器
    pub fn new(source_unit: LengthUnit, target_unit: LengthUnit) -> Self {
        Self {
            source_unit,
            target_unit,
        }
    }

    /// 创建默认转换器（毫米到毫米，不转换）
    pub fn default() -> Self {
        Self::new(LengthUnit::Millimeter, LengthUnit::Millimeter)
    }

    /// 获取转换因子
    pub fn conversion_factor(&self) -> f32 {
        if self.source_unit == self.target_unit {
            1.0
        } else {
            // 先转换为米，再转换为目标单位
            let source_to_meter = self.source_unit.to_meter_factor();
            let meter_to_target = 1.0 / self.target_unit.to_meter_factor();
            source_to_meter * meter_to_target
        }
    }

    /// 转换单个值
    pub fn convert_value(&self, value: f32) -> f32 {
        value * self.conversion_factor()
    }

    /// 转换 Vec3 坐标
    pub fn convert_vec3(&self, vec: &glam::Vec3) -> glam::Vec3 {
        let factor = self.conversion_factor();
        glam::Vec3::new(vec.x * factor, vec.y * factor, vec.z * factor)
    }

    /// 转换 Vec3 数组
    pub fn convert_vec3_array(&self, values: &[glam::Vec3]) -> Vec<f32> {
        let factor = self.conversion_factor();
        let mut result = Vec::with_capacity(values.len() * 3);
        for v in values {
            result.extend_from_slice(&[v.x * factor, v.y * factor, v.z * factor]);
        }
        result
    }

    /// 转换变换矩阵的平移部分
    pub fn convert_translation(&self, translation: &glam::Vec3) -> glam::Vec3 {
        self.convert_vec3(translation)
    }

    /// 检查是否需要转换
    pub fn needs_conversion(&self) -> bool {
        self.source_unit != self.target_unit
    }

    /// 获取转换描述
    pub fn description(&self) -> String {
        if self.needs_conversion() {
            format!(
                "{} -> {} (因子: {:.6})",
                self.source_unit,
                self.target_unit,
                self.conversion_factor()
            )
        } else {
            format!("无转换 ({})", self.source_unit)
        }
    }
}

impl Default for UnitConverter {
    fn default() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unit_conversion() {
        // 毫米到米
        let converter = UnitConverter::new(LengthUnit::Millimeter, LengthUnit::Meter);
        assert_eq!(converter.conversion_factor(), 0.001);
        assert_eq!(converter.convert_value(1000.0), 1.0);

        // 毫米到厘米
        let converter = UnitConverter::new(LengthUnit::Millimeter, LengthUnit::Centimeter);
        assert_eq!(converter.conversion_factor(), 0.1);
        assert_eq!(converter.convert_value(100.0), 10.0);

        // 英尺到米
        let converter = UnitConverter::new(LengthUnit::Foot, LengthUnit::Meter);
        assert_eq!(converter.conversion_factor(), 0.3048);
        assert_eq!(converter.convert_value(1.0), 0.3048);

        // 相同单位
        let converter = UnitConverter::new(LengthUnit::Meter, LengthUnit::Meter);
        assert_eq!(converter.conversion_factor(), 1.0);
        assert_eq!(converter.convert_value(100.0), 100.0);
    }

    #[test]
    fn test_vec3_conversion() {
        let converter = UnitConverter::new(LengthUnit::Millimeter, LengthUnit::Meter);
        let input = glam::Vec3::new(1000.0, 2000.0, 3000.0);
        let output = converter.convert_vec3(&input);
        assert_eq!(output, glam::Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn test_unit_parsing() {
        assert_eq!(LengthUnit::from_str("mm").unwrap(), LengthUnit::Millimeter);
        assert_eq!(LengthUnit::from_str("meter").unwrap(), LengthUnit::Meter);
        assert_eq!(LengthUnit::from_str("FOOT").unwrap(), LengthUnit::Foot);
        assert!(LengthUnit::from_str("invalid").is_err());
    }
}
