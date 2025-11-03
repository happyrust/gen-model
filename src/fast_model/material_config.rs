use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

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

        Ok(Self {
            materials: file.materials,
            noun_bindings: file.noun_bindings,
            index_map,
            default_material: file.default_material,
            source_path: path_ref.to_path_buf(),
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
