use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use aios_core::color_scheme::ColorSchemeManager;
use aios_core::pdms_types::PdmsGenericType;

const DEFAULT_LIBRARY_PATH: &str = "assets/material/plant_pipeline_materials.json";

#[derive(Debug, Deserialize, Clone)]
pub struct MaterialLibraryFile {
    #[serde(default)]
    pub default_material: Option<String>,
    #[serde(default)]
    pub materials: Vec<MaterialDefinition>,
    #[serde(default)]
    pub noun_bindings: HashMap<String, String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MaterialDefinition {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "pbrMetallicRoughness")]
    #[serde(default)]
    pub pbr_metallic_roughness: Option<PbrMetallicRoughness>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct PbrMetallicRoughness {
    #[serde(default, rename = "baseColorFactor")]
    pub base_color_factor: Option<[f32; 4]>,
    #[serde(default, rename = "baseColorTexture")]
    pub base_color_texture: Option<TextureReference>,
    #[serde(default, rename = "metallicFactor")]
    pub metallic_factor: Option<f32>,
    #[serde(default, rename = "roughnessFactor")]
    pub roughness_factor: Option<f32>,
    #[serde(default, rename = "metallicRoughnessTexture")]
    pub metallic_roughness_texture: Option<TextureReference>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct TextureReference {
    #[serde(default)]
    pub index: Option<u32>,
    #[serde(default, rename = "texCoord")]
    pub tex_coord: Option<u32>,
    #[serde(default)]
    pub scale: Option<f32>,
}

pub struct MaterialLibrary {
    materials: Vec<MaterialDefinition>,
    noun_bindings: HashMap<String, String>,
    index_map: HashMap<String, usize>,
    default_material: Option<String>,
    source_path: PathBuf,
    color_scheme_manager: Option<ColorSchemeManager>,
}

impl MaterialLibrary {
    pub fn load_default() -> Result<Self> {
        Self::load_from_path(DEFAULT_LIBRARY_PATH)
    }

    pub fn load_from_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_ref = path.as_ref();
        let text = fs::read_to_string(path_ref)
            .with_context(|| format!("读取材质库文件失败: {}", path_ref.display()))?;
        let file: MaterialLibraryFile = serde_json::from_str(&text)
            .with_context(|| format!("解析材质库 JSON 失败: {}", path_ref.display()))?;

        let mut index_map = HashMap::new();
        for (idx, material) in file.materials.iter().enumerate() {
            index_map.insert(material.name.clone(), idx);
        }

        // 尝试加载颜色配置方案
        let color_scheme_manager = ColorSchemeManager::load_from_file("ColorSchemes.toml")
            .ok()
            .or_else(|| Some(ColorSchemeManager::default_schemes()));

        Ok(Self {
            materials: file.materials,
            noun_bindings: file.noun_bindings,
            index_map,
            default_material: file.default_material,
            source_path: path_ref.to_path_buf(),
            color_scheme_manager,
        })
    }

    pub fn material_index_for_noun(&self, noun: &str) -> Option<usize> {
        let key = noun.to_uppercase();
        if let Some(material_name) = self.noun_bindings.get(&key) {
            return self.index_map.get(material_name).copied();
        }
        if let Some(default_name) = self.default_material.as_ref() {
            return self.index_map.get(default_name).copied();
        }
        None
    }

    pub fn materials(&self) -> &[MaterialDefinition] {
        &self.materials
    }

    pub fn source_path(&self) -> &Path {
        &self.source_path
    }

    /// 根据 PDMS 类型获取颜色 (RGBA, 0-255)
    pub fn get_color_for_type(&self, pdms_type: PdmsGenericType) -> Option<[u8; 4]> {
        self.color_scheme_manager
            .as_ref()
            .and_then(|manager| manager.get_color_for_type(pdms_type))
    }

    /// 根据 noun 字符串获取颜色 (RGBA, 0-255)
    pub fn get_color_for_noun(&self, noun: &str) -> Option<[u8; 4]> {
        // 尝试将 noun 转换为 PdmsGenericType
        let noun_upper = noun.to_uppercase();
        if let Ok(pdms_type) = noun_upper.parse::<PdmsGenericType>() {
            self.get_color_for_type(pdms_type)
        } else {
            None
        }
    }

    /// 将颜色从 [u8; 4] 转换为归一化的 [f32; 4]
    pub fn color_to_normalized(color: [u8; 4]) -> [f32; 4] {
        [
            color[0] as f32 / 255.0,
            color[1] as f32 / 255.0,
            color[2] as f32 / 255.0,
            color[3] as f32 / 255.0,
        ]
    }

    /// 根据 noun 获取归一化的颜色 (0.0-1.0)
    pub fn get_normalized_color_for_noun(&self, noun: &str) -> Option<[f32; 4]> {
        self.get_color_for_noun(noun)
            .map(Self::color_to_normalized)
    }

    /// 为指定的 noun 创建一个基于颜色配置的 glTF 材质
    /// 如果颜色配置中没有该类型,则返回 None
    pub fn create_color_based_material(&self, noun: &str, use_basic: bool) -> Option<Value> {
        let color = self.get_normalized_color_for_noun(noun)?;

        let mut material = json!({
            "name": format!("{}_ColorScheme", noun.to_uppercase())
        });

        if use_basic {
            // 使用 unlit 扩展的基础材质
            material["pbrMetallicRoughness"] = json!({
                "baseColorFactor": color
            });
            material["extensions"] = json!({
                "KHR_materials_unlit": {}
            });
        } else {
            // 标准 PBR 材质
            material["pbrMetallicRoughness"] = json!({
                "baseColorFactor": color,
                "metallicFactor": 0.0,
                "roughnessFactor": 0.8
            });
        }

        Some(material)
    }

    /// 获取或创建材质索引
    /// 优先使用材质库中的绑定,如果没有则使用颜色配置创建动态材质
    pub fn get_or_create_material_for_noun(
        &self,
        noun: &str,
        use_basic: bool,
        dynamic_materials: &mut Vec<Value>,
    ) -> Option<usize> {
        // 首先尝试从材质库中获取
        if let Some(idx) = self.material_index_for_noun(noun) {
            return Some(idx);
        }

        // 如果材质库中没有,尝试使用颜色配置创建
        if let Some(material_json) = self.create_color_based_material(noun, use_basic) {
            let new_idx = self.materials.len() + dynamic_materials.len();
            dynamic_materials.push(material_json);
            return Some(new_idx);
        }

        None
    }
}

impl MaterialDefinition {
    pub fn to_gltf_material(&self) -> Value {
        let mut material = json!({
            "name": self.name
        });

        if let Some(desc) = &self.description {
            material["extras"] = json!({ "description": desc });
        }

        if let Some(pbr) = &self.pbr_metallic_roughness {
            let mut pbr_json = json!({});
            if let Some(color) = pbr.base_color_factor {
                pbr_json["baseColorFactor"] = json!(color);
            }
            if let Some(texture) = &pbr.base_color_texture {
                pbr_json["baseColorTexture"] = texture.to_gltf_value();
            }
            if let Some(metallic) = pbr.metallic_factor {
                pbr_json["metallicFactor"] = json!(metallic);
            }
            if let Some(roughness) = pbr.roughness_factor {
                pbr_json["roughnessFactor"] = json!(roughness);
            }
            if let Some(texture) = &pbr.metallic_roughness_texture {
                pbr_json["metallicRoughnessTexture"] = texture.to_gltf_value();
            }
            material["pbrMetallicRoughness"] = pbr_json;
        }

        material
    }

    /// 以基础颜色（非 PBR）导出材质：
    /// - 使用 KHR_materials_unlit 扩展
    /// - 仅输出 baseColorFactor（若存在），忽略 metallic/roughness 等 PBR 参数
    pub fn to_basic_unlit_gltf_material(&self) -> Value {
        let mut material = json!({
            "name": self.name
        });

        if let Some(desc) = &self.description {
            material["extras"] = json!({ "description": desc });
        }

        // 基础颜色来自 pbr.baseColorFactor，如果存在
        if let Some(pbr) = &self.pbr_metallic_roughness {
            let mut pbr_basic = json!({});
            if let Some(color) = pbr.base_color_factor {
                pbr_basic["baseColorFactor"] = json!(color);
            }
            // 仅提供 baseColorFactor 作为基础颜色承载
            if !pbr_basic.as_object().unwrap().is_empty() {
                material["pbrMetallicRoughness"] = pbr_basic;
            }
        }

        // 标记为 Unlit
        material["extensions"] = json!({
            "KHR_materials_unlit": {}
        });

        material
    }
}

impl TextureReference {
    pub fn to_gltf_value(&self) -> Value {
        let mut value = json!({});
        if let Some(index) = self.index {
            value["index"] = json!(index);
        }
        if let Some(tex_coord) = self.tex_coord {
            value["texCoord"] = json!(tex_coord);
        }
        if let Some(scale) = self.scale {
            value["scale"] = json!(scale);
        }
        value
    }
}
