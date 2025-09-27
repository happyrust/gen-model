use glam::{EulerRot, Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};

/// XKT 材质数据结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XKTMaterial {
    pub id: String,
    pub name: String,
    pub diffuse: Vec3,
    pub ambient: Vec3,
    pub specular: Vec3,
    pub emissive: Vec3,
    pub shininess: f32,
    pub opacity: f32,
    pub metallic: f32,
    pub roughness: f32,
    pub texture_id: Option<String>,
}

impl XKTMaterial {
    /// 创建新的材质
    pub fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            diffuse: Vec3::new(0.8, 0.8, 0.8),
            ambient: Vec3::new(0.2, 0.2, 0.2),
            specular: Vec3::new(1.0, 1.0, 1.0),
            emissive: Vec3::new(0.0, 0.0, 0.0),
            shininess: 32.0,
            opacity: 1.0,
            metallic: 0.0,
            roughness: 0.5,
            texture_id: None,
        }
    }

    /// 创建基于颜色的简单材质
    pub fn create_color_material(id: String, name: String, color: Vec3) -> Self {
        Self {
            id,
            name,
            diffuse: color,
            ambient: color * 0.3,
            specular: Vec3::new(0.5, 0.5, 0.5),
            emissive: Vec3::new(0.0, 0.0, 0.0),
            shininess: 16.0,
            opacity: 1.0,
            metallic: 0.0,
            roughness: 0.7,
            texture_id: None,
        }
    }

    /// 创建金属材质
    pub fn create_metallic_material(id: String, name: String, color: Vec3) -> Self {
        Self {
            id,
            name,
            diffuse: color,
            ambient: color * 0.1,
            specular: Vec3::new(0.9, 0.9, 0.9),
            emissive: Vec3::new(0.0, 0.0, 0.0),
            shininess: 64.0,
            opacity: 1.0,
            metallic: 0.8,
            roughness: 0.2,
            texture_id: None,
        }
    }

    /// 创建塑料材质
    pub fn create_plastic_material(id: String, name: String, color: Vec3) -> Self {
        Self {
            id,
            name,
            diffuse: color,
            ambient: color * 0.4,
            specular: Vec3::new(0.3, 0.3, 0.3),
            emissive: Vec3::new(0.0, 0.0, 0.0),
            shininess: 8.0,
            opacity: 1.0,
            metallic: 0.0,
            roughness: 0.8,
            texture_id: None,
        }
    }

    /// 设置透明度
    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = opacity.clamp(0.0, 1.0);
    }

    /// 设置金属度
    pub fn set_metallic(&mut self, metallic: f32) {
        self.metallic = metallic.clamp(0.0, 1.0);
    }

    /// 设置粗糙度
    pub fn set_roughness(&mut self, roughness: f32) {
        self.roughness = roughness.clamp(0.0, 1.0);
    }

    /// 设置纹理
    pub fn set_texture(&mut self, texture_id: String) {
        self.texture_id = Some(texture_id);
    }
}

/// XKT 网格数据结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XKTMesh {
    pub id: String,
    pub mesh_index: Option<usize>,
    pub geometry_id: String,
    pub material_id: Option<String>,

    // 变换矩阵（4x4）
    pub matrix: Option<[f32; 16]>,

    // 传统变换参数（用于构建矩阵）
    pub position: Vec3,
    pub rotation: Vec3, // Euler angles in radians
    pub scale: Vec3,

    // PBR材质属性
    pub color: Vec3,
    pub opacity: f32,
    pub metallic: f32,
    pub roughness: f32,

    // 纹理集引用
    pub texture_set_id: Option<String>,

    // 状态
    pub visible: bool,
}

impl XKTMesh {
    /// 创建新的网格
    pub fn new(id: String, geometry_id: String) -> Self {
        Self {
            id,
            mesh_index: None,
            geometry_id,
            material_id: None,
            matrix: None,
            position: Vec3::ZERO,
            rotation: Vec3::ZERO,
            scale: Vec3::ONE,
            color: Vec3::ONE,
            opacity: 1.0,
            metallic: 0.0,
            roughness: 0.5,
            texture_set_id: None,
            visible: true,
        }
    }

    /// 设置材质
    pub fn set_material(&mut self, material_id: String) {
        self.material_id = Some(material_id);
    }

    /// 设置位置
    pub fn set_position(&mut self, position: Vec3) {
        self.position = position;
    }

    /// 设置旋转（欧拉角，弧度）
    pub fn set_rotation(&mut self, rotation: Vec3) {
        self.rotation = rotation;
    }

    /// 设置缩放
    pub fn set_scale(&mut self, scale: Vec3) {
        self.scale = scale;
    }

    /// 设置颜色（覆盖材质颜色）
    pub fn set_color(&mut self, color: Vec3) {
        self.color = color;
    }

    /// 设置透明度
    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = opacity.clamp(0.0, 1.0);
    }

    /// 设置可见性
    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    /// 确保存在 4x4 变换矩阵
    pub fn ensure_matrix(&mut self) {
        if self.matrix.is_some() {
            return;
        }

        let rotation = Quat::from_euler(
            EulerRot::XYZ,
            self.rotation.x,
            self.rotation.y,
            self.rotation.z,
        );

        let transform = Mat4::from_scale_rotation_translation(self.scale, rotation, self.position);
        self.matrix = Some(transform.to_cols_array());
    }

    /// 直接设置矩阵并归零 TRS 参数，便于后续维护
    pub fn set_matrix(&mut self, matrix: [f32; 16]) {
        self.matrix = Some(matrix);
        self.position = Vec3::ZERO;
        self.rotation = Vec3::ZERO;
        self.scale = Vec3::ONE;
    }
}
