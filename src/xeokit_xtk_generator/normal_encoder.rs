// 法向量编码器 - 实现 xeokit 标准的 Oct 编码

use super::*;
use anyhow::Result;
use glam::Vec3;

/// 法向量编码器
pub struct NormalEncoder {
    precision: NormalPrecision,
}

/// 法向量精度设置
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum NormalPrecision {
    Low,    // 8位 Oct 编码 (2 bytes per normal)
    Medium, // 16位 Oct 编码 (4 bytes per normal)
    High,   // 32位浮点 (12 bytes per normal)
}

impl NormalEncoder {
    pub fn new() -> Self {
        Self {
            precision: NormalPrecision::Low, // 默认使用低精度以符合 xeokit 标准
        }
    }

    pub fn with_precision(precision: NormalPrecision) -> Self {
        Self { precision }
    }

    /// 编码法向量数组
    pub fn encode_normals(&self, normals: &[Vec3]) -> Result<Vec<u8>> {
        match self.precision {
            NormalPrecision::Low => self.encode_normals_oct8(normals),
            NormalPrecision::Medium => self.encode_normals_oct16(normals),
            NormalPrecision::High => self.encode_normals_float32(normals),
        }
    }

    /// 8位 Oct 编码 (xeokit 标准)
    fn encode_normals_oct8(&self, normals: &[Vec3]) -> Result<Vec<u8>> {
        let mut encoded = Vec::with_capacity(normals.len() * 2);
        
        for &normal in normals {
            let oct_encoded = self.oct_encode_8bit(normal);
            encoded.extend_from_slice(&oct_encoded);
        }
        
        Ok(encoded)
    }

    /// 16位 Oct 编码 (高精度选项)
    fn encode_normals_oct16(&self, normals: &[Vec3]) -> Result<Vec<u8>> {
        let mut encoded = Vec::with_capacity(normals.len() * 4);
        
        for &normal in normals {
            let oct_encoded = self.oct_encode_16bit(normal);
            encoded.extend_from_slice(&oct_encoded);
        }
        
        Ok(encoded)
    }

    /// 32位浮点编码 (最高精度)
    fn encode_normals_float32(&self, normals: &[Vec3]) -> Result<Vec<u8>> {
        let mut encoded = Vec::with_capacity(normals.len() * 12);
        
        for &normal in normals {
            let bytes = normal.to_array();
            for &component in &bytes {
                encoded.extend_from_slice(&component.to_le_bytes());
            }
        }
        
        Ok(encoded)
    }

    /// 8位 Oct 编码实现
    fn oct_encode_8bit(&self, normal: Vec3) -> [u8; 2] {
        // 归一化法向量
        let n = normal.normalize();
        
        // Oct 编码算法
        // 1. 将法向量投影到八面体表面
        let sum = n.x.abs() + n.y.abs() + n.z.abs();
        let n = n / sum;
        
        // 2. 如果 z < 0，则折叠到正八面体
        let (x, y) = if n.z >= 0.0 {
            (n.x, n.y)
        } else {
            let sign_x = if n.x >= 0.0 { 1.0 } else { -1.0 };
            let sign_y = if n.y >= 0.0 { 1.0 } else { -1.0 };
            (
                (1.0 - n.y.abs()) * sign_x,
                (1.0 - n.x.abs()) * sign_y
            )
        };
        
        // 3. 量化到 8 位
        [
            ((x * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0) as u8,
            ((y * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0) as u8,
        ]
    }

    /// 16位 Oct 编码实现
    fn oct_encode_16bit(&self, normal: Vec3) -> [u8; 4] {
        let n = normal.normalize();
        let sum = n.x.abs() + n.y.abs() + n.z.abs();
        let n = n / sum;
        
        let (x, y) = if n.z >= 0.0 {
            (n.x, n.y)
        } else {
            let sign_x = if n.x >= 0.0 { 1.0 } else { -1.0 };
            let sign_y = if n.y >= 0.0 { 1.0 } else { -1.0 };
            (
                (1.0 - n.y.abs()) * sign_x,
                (1.0 - n.x.abs()) * sign_y
            )
        };
        
        // 量化到 16 位
        let x_quantized = ((x * 0.5 + 0.5) * 65535.0).clamp(0.0, 65535.0) as u16;
        let y_quantized = ((y * 0.5 + 0.5) * 65535.0).clamp(0.0, 65535.0) as u16;
        
        let mut result = [0u8; 4];
        result[0..2].copy_from_slice(&x_quantized.to_le_bytes());
        result[2..4].copy_from_slice(&y_quantized.to_le_bytes());
        result
    }

    /// 解码 8位 Oct 编码 (用于验证)
    pub fn oct_decode_8bit(&self, encoded: [u8; 2]) -> Vec3 {
        // 反量化
        let x = (encoded[0] as f32 / 255.0) * 2.0 - 1.0;
        let y = (encoded[1] as f32 / 255.0) * 2.0 - 1.0;
        
        // 计算 z 分量
        let z = 1.0 - x.abs() - y.abs();
        
        // 如果在折叠区域，则展开
        let (x, y) = if z < 0.0 {
            let sign_x = if x >= 0.0 { 1.0 } else { -1.0 };
            let sign_y = if y >= 0.0 { 1.0 } else { -1.0 };
            (
                (1.0 - y.abs()) * sign_x,
                (1.0 - x.abs()) * sign_y
            )
        } else {
            (x, y)
        };
        
        Vec3::new(x, y, z).normalize()
    }

    /// 解码 16位 Oct 编码 (用于验证)
    pub fn oct_decode_16bit(&self, encoded: [u8; 4]) -> Vec3 {
        let x_quantized = u16::from_le_bytes([encoded[0], encoded[1]]);
        let y_quantized = u16::from_le_bytes([encoded[2], encoded[3]]);
        
        let x = (x_quantized as f32 / 65535.0) * 2.0 - 1.0;
        let y = (y_quantized as f32 / 65535.0) * 2.0 - 1.0;
        
        let z = 1.0 - x.abs() - y.abs();
        
        let (x, y) = if z < 0.0 {
            let sign_x = if x >= 0.0 { 1.0 } else { -1.0 };
            let sign_y = if y >= 0.0 { 1.0 } else { -1.0 };
            (
                (1.0 - y.abs()) * sign_x,
                (1.0 - x.abs()) * sign_y
            )
        } else {
            (x, y)
        };
        
        Vec3::new(x, y, z).normalize()
    }

    /// 计算编码误差 (用于质量评估)
    pub fn calculate_encoding_error(&self, original: Vec3, encoded_data: &[u8]) -> f32 {
        let decoded = match self.precision {
            NormalPrecision::Low => {
                if encoded_data.len() >= 2 {
                    self.oct_decode_8bit([encoded_data[0], encoded_data[1]])
                } else {
                    return f32::INFINITY;
                }
            }
            NormalPrecision::Medium => {
                if encoded_data.len() >= 4 {
                    self.oct_decode_16bit([
                        encoded_data[0], encoded_data[1], 
                        encoded_data[2], encoded_data[3]
                    ])
                } else {
                    return f32::INFINITY;
                }
            }
            NormalPrecision::High => {
                // 32位浮点没有编码误差
                return 0.0;
            }
        };
        
        // 计算角度误差
        let dot_product = original.normalize().dot(decoded).clamp(-1.0, 1.0);
        dot_product.acos()
    }

    /// 获取每个法向量的字节数
    pub fn bytes_per_normal(&self) -> usize {
        match self.precision {
            NormalPrecision::Low => 2,
            NormalPrecision::Medium => 4,
            NormalPrecision::High => 12,
        }
    }

    /// 批量质量评估
    pub fn evaluate_encoding_quality(&self, normals: &[Vec3]) -> Result<EncodingQualityReport> {
        let encoded = self.encode_normals(normals)?;
        let bytes_per_normal = self.bytes_per_normal();
        
        let mut total_error: f32 = 0.0;
        let mut max_error: f32 = 0.0;
        let mut error_count = 0;
        
        for (i, &original) in normals.iter().enumerate() {
            let start_idx = i * bytes_per_normal;
            let end_idx = start_idx + bytes_per_normal;
            
            if end_idx <= encoded.len() {
                let encoded_slice = &encoded[start_idx..end_idx];
                let error = self.calculate_encoding_error(original, encoded_slice);
                
                if error.is_finite() {
                    total_error += error;
                    max_error = max_error.max(error);
                    error_count += 1;
                }
            }
        }
        
        let average_error = if error_count > 0 {
            total_error / error_count as f32
        } else {
            0.0
        };
        
        Ok(EncodingQualityReport {
            precision: self.precision,
            normal_count: normals.len(),
            encoded_size: encoded.len(),
            compression_ratio: (normals.len() * 12) as f32 / encoded.len() as f32,
            average_error_radians: average_error,
            max_error_radians: max_error,
            average_error_degrees: average_error.to_degrees(),
            max_error_degrees: max_error.to_degrees(),
        })
    }
}

/// 编码质量报告
#[derive(Debug, Clone)]
pub struct EncodingQualityReport {
    pub precision: NormalPrecision,
    pub normal_count: usize,
    pub encoded_size: usize,
    pub compression_ratio: f32,
    pub average_error_radians: f32,
    pub max_error_radians: f32,
    pub average_error_degrees: f32,
    pub max_error_degrees: f32,
}

impl EncodingQualityReport {
    pub fn print_report(&self) {
        println!("=== 法向量编码质量报告 ===");
        println!("精度设置: {:?}", self.precision);
        println!("法向量数量: {}", self.normal_count);
        println!("编码后大小: {} bytes", self.encoded_size);
        println!("压缩比: {:.2}:1", self.compression_ratio);
        println!("平均误差: {:.4}° ({:.6} rad)", self.average_error_degrees, self.average_error_radians);
        println!("最大误差: {:.4}° ({:.6} rad)", self.max_error_degrees, self.max_error_radians);
        
        // 质量评级
        let quality = if self.max_error_degrees < 1.0 {
            "优秀"
        } else if self.max_error_degrees < 5.0 {
            "良好"
        } else if self.max_error_degrees < 10.0 {
            "一般"
        } else {
            "较差"
        };
        println!("编码质量: {}", quality);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn test_oct_encoding_8bit() {
        let encoder = NormalEncoder::new();
        
        // 测试基本方向
        let test_normals = vec![
            Vec3::new(1.0, 0.0, 0.0),   // +X
            Vec3::new(-1.0, 0.0, 0.0),  // -X
            Vec3::new(0.0, 1.0, 0.0),   // +Y
            Vec3::new(0.0, -1.0, 0.0),  // -Y
            Vec3::new(0.0, 0.0, 1.0),   // +Z
            Vec3::new(0.0, 0.0, -1.0),  // -Z
        ];
        
        for normal in test_normals {
            let encoded = encoder.oct_encode_8bit(normal);
            let decoded = encoder.oct_decode_8bit(encoded);
            let error = encoder.calculate_encoding_error(normal, &encoded);
            
            println!("原始: {:?}, 编码: {:?}, 解码: {:?}, 误差: {:.4}°", 
                normal, encoded, decoded, error.to_degrees());
            
            // 8位编码的误差应该在合理范围内
            assert!(error < PI / 180.0 * 10.0, "编码误差过大: {:.4}°", error.to_degrees());
        }
    }

    #[test]
    fn test_oct_encoding_16bit() {
        let encoder = NormalEncoder::with_precision(NormalPrecision::Medium);
        
        let normal = Vec3::new(0.577, 0.577, 0.577).normalize(); // 对角线方向
        let encoded = encoder.encode_normals(&[normal]).unwrap();
        let error = encoder.calculate_encoding_error(normal, &encoded);
        
        println!("16位编码误差: {:.6}°", error.to_degrees());
        
        // 16位编码应该有更高的精度
        assert!(error < PI / 180.0 * 1.0, "16位编码误差过大: {:.4}°", error.to_degrees());
    }

    #[test]
    fn test_encoding_quality_report() {
        let encoder = NormalEncoder::new();
        
        // 生成随机法向量进行测试
        let test_normals: Vec<Vec3> = (0..1000)
            .map(|i| {
                let angle1 = (i as f32 / 1000.0) * 2.0 * PI;
                let angle2 = ((i * 7) as f32 / 1000.0) * PI;
                Vec3::new(
                    angle2.sin() * angle1.cos(),
                    angle2.sin() * angle1.sin(),
                    angle2.cos()
                ).normalize()
            })
            .collect();
        
        let report = encoder.evaluate_encoding_quality(&test_normals).unwrap();
        report.print_report();
        
        // 验证报告的合理性
        assert!(report.compression_ratio > 1.0);
        assert!(report.average_error_degrees < 10.0);
        assert!(report.encoded_size == test_normals.len() * 2); // 8位编码每个法向量2字节
    }

    #[test]
    fn test_precision_comparison() {
        let test_normal = Vec3::new(0.267, 0.535, 0.802).normalize();
        
        let encoders = [
            NormalEncoder::with_precision(NormalPrecision::Low),
            NormalEncoder::with_precision(NormalPrecision::Medium),
            NormalEncoder::with_precision(NormalPrecision::High),
        ];
        
        for encoder in &encoders {
            let encoded = encoder.encode_normals(&[test_normal]).unwrap();
            let error = encoder.calculate_encoding_error(test_normal, &encoded);
            
            println!("{:?}: {} bytes, 误差: {:.6}°", 
                encoder.precision, encoded.len(), error.to_degrees());
        }
    }
}
