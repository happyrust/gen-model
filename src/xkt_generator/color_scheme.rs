use glam::Vec3;
use std::collections::HashMap;

/// 颜色方案管理器
pub struct ColorScheme {
    type_colors: HashMap<String, Vec3>,
    default_color: Vec3,
}

impl ColorScheme {
    /// 创建新的颜色方案
    pub fn new() -> Self {
        let mut scheme = Self {
            type_colors: HashMap::new(),
            default_color: Vec3::new(0.7, 0.7, 0.7), // 默认灰色
        };

        // 初始化常见PDMS类型的颜色
        scheme.init_pdms_colors();
        scheme
    }

    /// 初始化PDMS常见类型的颜色
    fn init_pdms_colors(&mut self) {
        // 管道系统 - 蓝色系
        self.type_colors
            .insert("PIPE".to_string(), Vec3::new(0.2, 0.4, 0.8));
        self.type_colors
            .insert("ELBO".to_string(), Vec3::new(0.3, 0.5, 0.9));
        self.type_colors
            .insert("TEE".to_string(), Vec3::new(0.1, 0.3, 0.7));
        self.type_colors
            .insert("REDU".to_string(), Vec3::new(0.4, 0.6, 1.0));
        self.type_colors
            .insert("FLANGE".to_string(), Vec3::new(0.0, 0.2, 0.6));

        // 阀门 - 红色系
        self.type_colors
            .insert("VALVE".to_string(), Vec3::new(0.8, 0.2, 0.2));
        self.type_colors
            .insert("GVAL".to_string(), Vec3::new(0.9, 0.3, 0.3));
        self.type_colors
            .insert("CHVA".to_string(), Vec3::new(0.7, 0.1, 0.1));

        // 设备 - 绿色系
        self.type_colors
            .insert("EQUIPMENT".to_string(), Vec3::new(0.2, 0.8, 0.2));
        self.type_colors
            .insert("VESSEL".to_string(), Vec3::new(0.3, 0.9, 0.3));
        self.type_colors
            .insert("TANK".to_string(), Vec3::new(0.1, 0.7, 0.1));
        self.type_colors
            .insert("PUMP".to_string(), Vec3::new(0.4, 1.0, 0.4));

        // 仪表 - 黄色系
        self.type_colors
            .insert("INSTRUMENT".to_string(), Vec3::new(0.9, 0.9, 0.2));
        self.type_colors
            .insert("GAUGE".to_string(), Vec3::new(1.0, 1.0, 0.3));
        self.type_colors
            .insert("TRANSMITTER".to_string(), Vec3::new(0.8, 0.8, 0.1));

        // 结构 - 橙色系
        self.type_colors
            .insert("STRUCTURE".to_string(), Vec3::new(0.9, 0.5, 0.1));
        self.type_colors
            .insert("BEAM".to_string(), Vec3::new(1.0, 0.6, 0.2));
        self.type_colors
            .insert("COLUMN".to_string(), Vec3::new(0.8, 0.4, 0.0));
        self.type_colors
            .insert("PLATE".to_string(), Vec3::new(0.7, 0.3, 0.1));

        // 电气 - 紫色系
        self.type_colors
            .insert("ELECTRICAL".to_string(), Vec3::new(0.6, 0.2, 0.8));
        self.type_colors
            .insert("CABLE".to_string(), Vec3::new(0.7, 0.3, 0.9));
        self.type_colors
            .insert("CONDUIT".to_string(), Vec3::new(0.5, 0.1, 0.7));

        // 暖通 - 青色系
        self.type_colors
            .insert("HVAC".to_string(), Vec3::new(0.2, 0.8, 0.8));
        self.type_colors
            .insert("DUCT".to_string(), Vec3::new(0.3, 0.9, 0.9));
        self.type_colors
            .insert("DAMPER".to_string(), Vec3::new(0.1, 0.7, 0.7));

        // 土建 - 棕色系
        self.type_colors
            .insert("CIVIL".to_string(), Vec3::new(0.6, 0.4, 0.2));
        self.type_colors
            .insert("FOUNDATION".to_string(), Vec3::new(0.5, 0.3, 0.1));
        self.type_colors
            .insert("WALL".to_string(), Vec3::new(0.7, 0.5, 0.3));

        // 通用几何体 - 灰色系
        self.type_colors
            .insert("BOX".to_string(), Vec3::new(0.6, 0.6, 0.6));
        self.type_colors
            .insert("CYLINDER".to_string(), Vec3::new(0.5, 0.5, 0.5));
        self.type_colors
            .insert("SPHERE".to_string(), Vec3::new(0.8, 0.8, 0.8));
    }

    /// 根据类型获取颜色
    pub fn get_color_for_type(&self, entity_type: &str) -> Vec3 {
        // 首先尝试精确匹配
        if let Some(color) = self.type_colors.get(entity_type) {
            return *color;
        }

        // 尝试部分匹配（不区分大小写）
        let entity_type_upper = entity_type.to_uppercase();
        for (type_name, color) in &self.type_colors {
            if entity_type_upper.contains(type_name) || type_name.contains(&entity_type_upper) {
                return *color;
            }
        }

        // 如果没有匹配，返回默认颜色
        self.default_color
    }

    /// 添加自定义类型颜色
    pub fn add_type_color(&mut self, entity_type: String, color: Vec3) {
        self.type_colors.insert(entity_type, color);
    }

    /// 设置默认颜色
    pub fn set_default_color(&mut self, color: Vec3) {
        self.default_color = color;
    }

    /// 获取所有已定义的类型
    pub fn get_defined_types(&self) -> Vec<&String> {
        self.type_colors.keys().collect()
    }

    /// 生成基于哈希的颜色（用于未知类型）
    pub fn generate_hash_color(&self, entity_type: &str) -> Vec3 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        entity_type.hash(&mut hasher);
        let hash = hasher.finish();

        // 使用哈希值生成HSV颜色，然后转换为RGB
        let hue = (hash % 360) as f32;
        let saturation = 0.7; // 固定饱和度
        let value = 0.8; // 固定明度

        hsv_to_rgb(hue, saturation, value)
    }

    /// 获取材质类型的颜色（金属、塑料等）
    pub fn get_material_color(&self, material_type: &str) -> Vec3 {
        match material_type.to_uppercase().as_str() {
            "STEEL" | "METAL" => Vec3::new(0.7, 0.7, 0.8),
            "ALUMINUM" => Vec3::new(0.9, 0.9, 0.95),
            "COPPER" => Vec3::new(0.8, 0.5, 0.2),
            "PLASTIC" => Vec3::new(0.2, 0.6, 0.9),
            "RUBBER" => Vec3::new(0.1, 0.1, 0.1),
            "GLASS" => Vec3::new(0.8, 0.9, 1.0),
            "CONCRETE" => Vec3::new(0.6, 0.6, 0.5),
            "WOOD" => Vec3::new(0.6, 0.4, 0.2),
            _ => self.default_color,
        }
    }
}

impl Default for ColorScheme {
    fn default() -> Self {
        Self::new()
    }
}

/// HSV转RGB颜色空间转换
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Vec3 {
    let h = h % 360.0;
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;

    let (r, g, b) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };

    Vec3::new(r + m, g + m, b + m)
}

/// 预定义的颜色常量
pub mod colors {
    use glam::Vec3;

    pub const RED: Vec3 = Vec3::new(1.0, 0.0, 0.0);
    pub const GREEN: Vec3 = Vec3::new(0.0, 1.0, 0.0);
    pub const BLUE: Vec3 = Vec3::new(0.0, 0.0, 1.0);
    pub const YELLOW: Vec3 = Vec3::new(1.0, 1.0, 0.0);
    pub const CYAN: Vec3 = Vec3::new(0.0, 1.0, 1.0);
    pub const MAGENTA: Vec3 = Vec3::new(1.0, 0.0, 1.0);
    pub const WHITE: Vec3 = Vec3::new(1.0, 1.0, 1.0);
    pub const BLACK: Vec3 = Vec3::new(0.0, 0.0, 0.0);
    pub const GRAY: Vec3 = Vec3::new(0.5, 0.5, 0.5);
    pub const ORANGE: Vec3 = Vec3::new(1.0, 0.5, 0.0);
    pub const PURPLE: Vec3 = Vec3::new(0.5, 0.0, 1.0);
    pub const BROWN: Vec3 = Vec3::new(0.6, 0.3, 0.1);
    pub const PINK: Vec3 = Vec3::new(1.0, 0.7, 0.8);
    pub const LIME: Vec3 = Vec3::new(0.5, 1.0, 0.0);
    pub const NAVY: Vec3 = Vec3::new(0.0, 0.0, 0.5);
    pub const MAROON: Vec3 = Vec3::new(0.5, 0.0, 0.0);
    pub const OLIVE: Vec3 = Vec3::new(0.5, 0.5, 0.0);
    pub const TEAL: Vec3 = Vec3::new(0.0, 0.5, 0.5);
    pub const SILVER: Vec3 = Vec3::new(0.75, 0.75, 0.75);
    pub const GOLD: Vec3 = Vec3::new(1.0, 0.84, 0.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_scheme() {
        let scheme = ColorScheme::new();

        // 测试已定义类型
        let pipe_color = scheme.get_color_for_type("PIPE");
        assert_eq!(pipe_color, Vec3::new(0.2, 0.4, 0.8));

        // 测试部分匹配
        let pipe_component_color = scheme.get_color_for_type("PIPE_COMPONENT");
        assert_eq!(pipe_component_color, Vec3::new(0.2, 0.4, 0.8));

        // 测试未知类型
        let unknown_color = scheme.get_color_for_type("UNKNOWN_TYPE");
        assert_eq!(unknown_color, scheme.default_color);
    }

    #[test]
    fn test_hsv_to_rgb() {
        let red = hsv_to_rgb(0.0, 1.0, 1.0);
        assert!((red.x - 1.0).abs() < 0.001);
        assert!(red.y.abs() < 0.001);
        assert!(red.z.abs() < 0.001);

        let green = hsv_to_rgb(120.0, 1.0, 1.0);
        assert!(green.x.abs() < 0.001);
        assert!((green.y - 1.0).abs() < 0.001);
        assert!(green.z.abs() < 0.001);
    }
}
