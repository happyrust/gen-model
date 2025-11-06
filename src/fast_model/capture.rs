use std::path::{Path, PathBuf};
use std::sync::Mutex;

use aios_core::options::DbOption;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::{NamedAttrValue, RefnoEnum, get_named_attmap};
use anyhow::{Context, Result, anyhow};
use glam::{Mat4, Vec2, Vec3, Vec4};
use image::{Rgba, RgbaImage};
use imageproc::drawing::{draw_text_mut, text_size};
use once_cell::sync::Lazy;
use rusttype::{Font, Scale};
use tokio::fs;

use crate::fast_model::export_model::export_obj::{
    PreparedObjExport, export_mesh_to_obj_with_unit_conversion, prepare_obj_export,
};
use crate::fast_model::model_exporter::CommonExportConfig;
use crate::fast_model::unit_converter::UnitConverter;

#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub output_dir: PathBuf,
    pub width: u32,
    pub height: u32,
    pub include_descendants: bool,
}

impl CaptureConfig {
    pub fn new(output_dir: PathBuf, width: u32, height: u32, include_descendants: bool) -> Self {
        Self {
            output_dir,
            width,
            height,
            include_descendants,
        }
    }
}

static CAPTURE_CONFIG: Lazy<Mutex<Option<CaptureConfig>>> = Lazy::new(|| Mutex::new(None));

pub fn set_capture_config(config: Option<CaptureConfig>) {
    let mut guard = CAPTURE_CONFIG.lock().unwrap();
    *guard = config;
}

pub fn capture_enabled() -> bool {
    CAPTURE_CONFIG.lock().unwrap().is_some()
}

fn current_config() -> Option<CaptureConfig> {
    CAPTURE_CONFIG.lock().unwrap().clone()
}

pub async fn capture_refnos_if_enabled(refnos: &[RefnoEnum], db_option: &DbOption) -> Result<()> {
    if !capture_enabled() || refnos.is_empty() {
        return Ok(());
    }
    capture_refnos(refnos, db_option).await
}

pub async fn capture_refnos(refnos: &[RefnoEnum], db_option: &DbOption) -> Result<()> {
    let config = match current_config() {
        Some(cfg) => cfg,
        None => return Ok(()),
    };

    let mesh_dir = db_option.get_meshes_path();
    for &refno in refnos {
        if let Err(err) = capture_single(refno, &mesh_dir, &config).await {
            eprintln!("[capture] 捕获参考号 {} 截图失败: {}", refno, err);
        }
    }

    Ok(())
}

async fn capture_single(refno: RefnoEnum, mesh_dir: &Path, config: &CaptureConfig) -> Result<()> {
    let refno_str = refno.to_string().replace('/', "_");
    let basename = resolve_capture_basename(refno).await;

    fs::create_dir_all(&config.output_dir)
        .await
        .with_context(|| format!("创建截图目录失败: {}", config.output_dir.display()))?;
    let obj_cache_dir = config.output_dir.join("obj-cache");
    fs::create_dir_all(&obj_cache_dir)
        .await
        .with_context(|| format!("创建 OBJ 缓存目录失败: {}", obj_cache_dir.display()))?;

    let obj_path = obj_cache_dir.join(format!("{basename}.obj"));
    let png_path = config.output_dir.join(format!("{basename}.png"));

    let mut common_config = CommonExportConfig {
        include_descendants: config.include_descendants,
        filter_nouns: None,
        verbose: true, // 启用详细输出以便调试
        unit_converter: UnitConverter::default(),
        use_basic_materials: false,
    };

    let mut prepared = prepare_obj_export(&[refno], mesh_dir, &common_config).await?;
    let mut mesh = prepared.mesh;
    let mut stats = prepared.stats;

    println!(
        "[capture] 参考号 {} 初步查询结果: vertices={}, geometry_count={}, mesh_files_found={}, mesh_files_missing={}",
        refno,
        mesh.vertices.len(),
        stats.geometry_count,
        stats.mesh_files_found,
        stats.mesh_files_missing
    );

    if (mesh.vertices.is_empty() || stats.geometry_count == 0) && !common_config.include_descendants
    {
        let mut alt_config = common_config.clone();
        alt_config.include_descendants = true;
        prepared = prepare_obj_export(&[refno], mesh_dir, &alt_config).await?;
        if !prepared.mesh.vertices.is_empty() && prepared.stats.geometry_count > 0 {
            println!("[capture] 参考号 {} 自动启用子孙节点重新收集几何体", refno);
            mesh = prepared.mesh;
            stats = prepared.stats;
            common_config = alt_config;
        }
    }

    if mesh.vertices.is_empty() || stats.geometry_count == 0 {
        println!(
            "[capture] 参考号 {} 无可导出的几何体，跳过截图 (vertices={}, geometry_count={})",
            refno,
            mesh.vertices.len(),
            stats.geometry_count
        );
        return Ok(());
    }

    let obj_path_str = obj_path
        .to_str()
        .ok_or_else(|| anyhow!("OBJ 路径无法转换为字符串"))?;

    export_mesh_to_obj_with_unit_conversion(&mesh, obj_path_str, &common_config.unit_converter)?;

    let render_mesh = mesh_with_unit_conversion(&mesh, &common_config.unit_converter);
    let display_label = get_display_label(refno).await;
    render_mesh_to_png(
        &render_mesh,
        config.width,
        config.height,
        &png_path,
        &display_label,
    )?;

    println!("📸 已生成截图: {}", png_path.display());

    Ok(())
}

async fn resolve_capture_basename(refno: RefnoEnum) -> String {
    let fallback_raw = refno.to_string().replace('/', "_");
    let fallback = sanitize_label(&fallback_raw);
    if let Ok(attmap) = get_named_attmap(refno).await {
        if let Some(name) = attmap.map.get("NAME").and_then(attr_to_string) {
            let sanitized = sanitize_label(&name);
            if !sanitized.is_empty() {
                return sanitized;
            }
        }

        let noun = attmap
            .map
            .get("TYPE")
            .or_else(|| attmap.map.get("NOUN"))
            .and_then(attr_to_string)
            .unwrap_or_else(|| "OBJECT".to_string());
        let combined = format!("{}_{}", noun, fallback_raw);
        let sanitized = sanitize_label(&combined);
        if !sanitized.is_empty() {
            return sanitized;
        }
    }

    fallback
}

fn attr_to_string(value: &NamedAttrValue) -> Option<String> {
    match value {
        NamedAttrValue::StringType(s)
        | NamedAttrValue::WordType(s)
        | NamedAttrValue::ElementType(s) => Some(s.clone()),
        NamedAttrValue::RefU64Type(r) => Some(r.to_string()),
        NamedAttrValue::RefnoEnumType(r) => Some(r.to_string()),
        _ => None,
    }
}

fn sanitize_label(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut sanitized = String::with_capacity(trimmed.len());
    for c in trimmed.chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' | '.' | ' ' => sanitized.push(c),
            '\u{4e00}'..='\u{9fff}' => sanitized.push(c),
            _ => sanitized.push('_'),
        }
    }

    if sanitized.len() > 128 {
        sanitized.chars().take(128).collect()
    } else {
        sanitized
    }
}

fn mesh_with_unit_conversion(mesh: &PlantMesh, unit_converter: &UnitConverter) -> PlantMesh {
    if !unit_converter.needs_conversion() {
        return mesh.clone();
    }

    let mut converted = mesh.clone();
    for vertex in &mut converted.vertices {
        *vertex = unit_converter.convert_vec3(vertex);
    }
    converted
}

async fn get_display_label(refno: RefnoEnum) -> String {
    if let Ok(attmap) = get_named_attmap(refno).await {
        // 优先使用 NAME 属性
        if let Some(name) = attmap.map.get("NAME").and_then(attr_to_string) {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                // 去掉开头的斜线
                return trimmed.trim_start_matches('/').to_string();
            }
        }

        // 如果没有 NAME，使用 NOUN 或 TYPE + 参考号
        let noun = attmap
            .map
            .get("TYPE")
            .or_else(|| attmap.map.get("NOUN"))
            .and_then(attr_to_string)
            .unwrap_or_else(|| "OBJECT".to_string());
        let refno_str = refno.to_string().trim_start_matches('/').to_string();
        return format!("{}_{}", noun, refno_str);
    }

    // 如果无法获取属性，使用参考号（去掉开头的斜线）
    let refno_str = refno.to_string().trim_start_matches('/').to_string();
    format!("REFNO_{}", refno_str)
}

fn render_mesh_to_png(
    mesh: &PlantMesh,
    width: u32,
    height: u32,
    output_path: &Path,
    label: &str,
) -> Result<()> {
    // 使用超采样抗锯齿：先渲染到高分辨率，然后下采样
    const SSAA_SCALE: u32 = 2; // 2x2 超采样
    let ssaa_width = width * SSAA_SCALE;
    let ssaa_height = height * SSAA_SCALE;
    let mut ssaa_image = RgbaImage::from_pixel(ssaa_width, ssaa_height, Rgba([0, 0, 0, 0]));

    if mesh.vertices.is_empty() || mesh.indices.is_empty() {
        let mut image = RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 0]));
        image
            .save(output_path)
            .with_context(|| format!("保存截图失败: {}", output_path.display()))?;
        return Ok(());
    }

    let (bounds_min, bounds_max) =
        compute_bounds(&mesh.vertices).ok_or_else(|| anyhow!("mesh 缺失顶点数据，无法截图"))?;
    let center = (bounds_min + bounds_max) * 0.5;
    let extent = bounds_max - bounds_min;
    let max_extent = extent
        .x
        .abs()
        .max(extent.y.abs())
        .max(extent.z.abs())
        .max(1.0);
    let radius = extent.length().max(max_extent).max(1.0);
    // 增加相机距离系数，确保模型完整显示不被截断
    // 考虑到透视投影（FOV 45度）和45度视角，需要更大的距离系数
    // 使用 max_extent 而不是 radius，确保适应非立方体的边界框
    let camera_distance = max_extent * 1.8;
    // 前视图 + 俯视 45°：相机在前方（Y 轴负方向）并向上（Z 轴正方向）45 度俯视模型
    // 方向向量：(0, -cos(45°), sin(45°)) = (0, -1/√2, 1/√2)
    let cos45 = 45.0f32.to_radians().cos();
    let sin45 = 45.0f32.to_radians().sin();
    let mut camera_dir = Vec3::new(0.0, -cos45, sin45).normalize();
    if camera_dir.length_squared() < f32::EPSILON {
        camera_dir = Vec3::new(0.0, -1.0, 1.0).normalize();
    }
    // 添加 45 度侧向旋转：绕 Z 轴旋转相机方向
    let side_rotation_angle = 45.0f32.to_radians();
    let cos_side = side_rotation_angle.cos();
    let sin_side = side_rotation_angle.sin();
    // 绕 Z 轴旋转 camera_dir（保持 Z 分量不变，旋转 X-Y 平面分量）
    let camera_dir_xy = Vec3::new(camera_dir.x, camera_dir.y, 0.0);
    let camera_dir_z = camera_dir.z;
    let rotated_xy = Vec3::new(
        camera_dir_xy.x * cos_side - camera_dir_xy.y * sin_side,
        camera_dir_xy.x * sin_side + camera_dir_xy.y * cos_side,
        0.0,
    );
    camera_dir = Vec3::new(rotated_xy.x, rotated_xy.y, camera_dir_z).normalize();
    // up 向量指向 Z 轴正方向
    let up = Vec3::Z;
    let eye = center + camera_dir * camera_distance;

    let view = Mat4::look_at_rh(eye, center, up);
    let aspect = (ssaa_width as f32 / ssaa_height as f32).max(0.01);
    let near = (radius * 0.05).max(0.01);
    let far = (radius * 12.0).max(near + 10.0);
    let proj = Mat4::perspective_rh_gl(45f32.to_radians(), aspect, near, far);
    let view_proj = proj * view;

    let normals = ensure_vertex_normals(mesh);
    let mut ndc_positions = vec![Vec3::ZERO; mesh.vertices.len()];
    let mut screen_positions = vec![Vec3::ZERO; mesh.vertices.len()];
    let mut valid = vec![false; mesh.vertices.len()];

    for (idx, vertex) in mesh.vertices.iter().enumerate() {
        let pos4 = view_proj * Vec4::new(vertex.x, vertex.y, vertex.z, 1.0);
        if pos4.w.abs() < 1e-6 {
            continue;
        }
        let ndc = pos4.truncate() / pos4.w;
        ndc_positions[idx] = ndc;
        let screen_x = (ndc.x * 0.5 + 0.5) * (ssaa_width as f32 - 1.0);
        let screen_y = (1.0 - (ndc.y * 0.5 + 0.5)) * (ssaa_height as f32 - 1.0);
        let depth = (ndc.z * 0.5) + 0.5;
        screen_positions[idx] = Vec3::new(screen_x, screen_y, depth);
        valid[idx] = true;
    }

    let mut depth_buffer = vec![f32::INFINITY; (ssaa_width as usize) * (ssaa_height as usize)];
    let light_dir = Vec3::new(0.6, 0.8, 1.0).normalize();
    let base_color = Vec3::new(0.42, 0.71, 0.96);

    for tri in mesh.indices.chunks(3) {
        if tri.len() < 3 {
            continue;
        }
        let i0 = tri[0] as usize;
        let i1 = tri[1] as usize;
        let i2 = tri[2] as usize;
        if i0 >= screen_positions.len()
            || i1 >= screen_positions.len()
            || i2 >= screen_positions.len()
        {
            continue;
        }
        if !valid[i0] || !valid[i1] || !valid[i2] {
            continue;
        }

        let p0 = Vec2::new(screen_positions[i0].x, screen_positions[i0].y);
        let p1 = Vec2::new(screen_positions[i1].x, screen_positions[i1].y);
        let p2 = Vec2::new(screen_positions[i2].x, screen_positions[i2].y);

        let area = edge(p0, p1, p2);
        if area.abs() < 1e-5 {
            continue;
        }

        let inv_area = 1.0 / area;
        let x_min = p0.x.min(p1.x).min(p2.x).floor().max(0.0) as i32;
        let x_max = p0.x.max(p1.x).max(p2.x).ceil().min((ssaa_width - 1) as f32) as i32;
        let y_min = p0.y.min(p1.y).min(p2.y).floor().max(0.0) as i32;
        let y_max =
            p0.y.max(p1.y)
                .max(p2.y)
                .ceil()
                .min((ssaa_height - 1) as f32) as i32;

        for y in y_min..=y_max {
            for x in x_min..=x_max {
                // 使用像素中心采样
                let sample = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let mut w0 = edge(p1, p2, sample);
                let mut w1 = edge(p2, p0, sample);
                let mut w2 = edge(p0, p1, sample);

                if (area > 0.0 && (w0 < 0.0 || w1 < 0.0 || w2 < 0.0))
                    || (area < 0.0 && (w0 > 0.0 || w1 > 0.0 || w2 > 0.0))
                {
                    continue;
                }

                w0 *= inv_area;
                w1 *= inv_area;
                w2 *= inv_area;

                let depth = w0 * screen_positions[i0].z
                    + w1 * screen_positions[i1].z
                    + w2 * screen_positions[i2].z;
                let buffer_index = (y as usize) * (ssaa_width as usize) + (x as usize);
                // 使用更严格的深度比较，减少精度误差
                if depth >= depth_buffer[buffer_index] - 1e-6 {
                    continue;
                }
                depth_buffer[buffer_index] = depth;

                let world_pos =
                    mesh.vertices[i0] * w0 + mesh.vertices[i1] * w1 + mesh.vertices[i2] * w2;
                let mut normal = normals[i0] * w0 + normals[i1] * w1 + normals[i2] * w2;
                normal = normal.normalize_or_zero();

                let view_dir = (eye - world_pos).normalize_or_zero();
                let halfway = (light_dir + view_dir).normalize_or_zero();
                let diffuse = normal.dot(light_dir).max(0.0);
                let specular = normal.dot(halfway).max(0.0).powf(32.0);
                let ambient = 0.25;
                let intensity = (ambient + diffuse * 0.65 + specular * 0.1).clamp(0.0, 1.0);

                let shaded = (base_color * intensity).clamp(Vec3::ZERO, Vec3::splat(1.0));
                let color = Rgba([
                    (shaded.x * 255.0) as u8,
                    (shaded.y * 255.0) as u8,
                    (shaded.z * 255.0) as u8,
                    255, // 完全不透明
                ]);
                ssaa_image.put_pixel(x as u32, y as u32, color);
            }
        }
    }

    // 将超采样图像下采样到目标分辨率，实现抗锯齿
    let mut image = RgbaImage::from_pixel(width, height, Rgba([0, 0, 0, 0]));
    for y in 0..height {
        for x in 0..width {
            let mut r_sum = 0u32;
            let mut g_sum = 0u32;
            let mut b_sum = 0u32;
            let mut a_sum = 0u32;
            let mut count = 0u32;

            // 对每个输出像素，采样 SSAA_SCALE x SSAA_SCALE 个超采样像素
            for sy in 0..SSAA_SCALE {
                for sx in 0..SSAA_SCALE {
                    let sx_coord = x * SSAA_SCALE + sx;
                    let sy_coord = y * SSAA_SCALE + sy;
                    if sx_coord < ssaa_width && sy_coord < ssaa_height {
                        let pixel = ssaa_image.get_pixel(sx_coord, sy_coord);
                        r_sum += pixel[0] as u32;
                        g_sum += pixel[1] as u32;
                        b_sum += pixel[2] as u32;
                        a_sum += pixel[3] as u32;
                        count += 1;
                    }
                }
            }

            if count > 0 {
                let scale = 1.0 / (count as f32);
                let color = Rgba([
                    ((r_sum as f32) * scale) as u8,
                    ((g_sum as f32) * scale) as u8,
                    ((b_sum as f32) * scale) as u8,
                    ((a_sum as f32) * scale) as u8,
                ]);
                image.put_pixel(x, y, color);
            }
        }
    }

    // 在图片底部中间绘制标签文字
    draw_label_on_image(&mut image, label, width, height)?;

    image
        .save(output_path)
        .with_context(|| format!("保存截图失败: {}", output_path.display()))?;
    Ok(())
}

fn draw_label_on_image(image: &mut RgbaImage, label: &str, width: u32, height: u32) -> Result<()> {
    if label.is_empty() {
        return Ok(());
    }

    // 尝试加载系统字体（优先使用常规字体，避免粗体）
    let font_paths = vec![
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf", // 常规字体
        "/System/Library/Fonts/PingFang.ttc",                   // 常规字体
        "/System/Library/Fonts/Helvetica.ttc",                  // 常规字体
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",      // 常规字体
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf", // 常规字体
        "/System/Library/Fonts/STHeiti Light.ttc",              // 细体字体
    ];

    let font_data = match font_paths.iter().find_map(|path| std::fs::read(path).ok()) {
        Some(data) => data,
        None => {
            // 如果无法加载字体，跳过文字绘制（不返回错误，避免影响截图功能）
            eprintln!("[capture] 警告: 无法加载系统字体，跳过文字标注");
            return Ok(());
        }
    };

    let font = match Font::try_from_bytes(&font_data) {
        Some(f) => f,
        None => {
            eprintln!("[capture] 警告: 无法解析字体数据，跳过文字标注");
            return Ok(());
        }
    };

    // 设置字体大小（根据图像高度自适应，进一步增大字体）
    let font_size = (height as f32 * 0.06).max(32.0).min(64.0);
    let scale = Scale::uniform(font_size);

    // 计算文字尺寸和位置（底部中间，稍微上移）
    let (text_width, text_height) = text_size(scale, &font, label);
    let x = (width.saturating_sub(text_width as u32)) / 2;
    let y = height.saturating_sub(text_height as u32 + 20); // 距离底部 20 像素（之前是 10，现在上移）

    // 绘制文字（红色，带增强的黑色描边以提高清晰度）
    let text_color = Rgba([255, 0, 0, 255]); // 红色
    let stroke_color = Rgba([0, 0, 0, 220]); // 半透明黑色描边，增强清晰度

    // 先绘制描边（增强描边效果：两层描边，外层更粗）
    // 外层描边（偏移2像素）
    for dy in -2i32..=2i32 {
        for dx in -2i32..=2i32 {
            if dx == 0 && dy == 0 {
                continue;
            }
            // 只绘制外围像素，不绘制内层
            if dx.abs() + dy.abs() == 2 {
                draw_text_mut(
                    image,
                    stroke_color,
                    (x as i32 + dx).max(0),
                    (y as i32 + dy).max(0),
                    scale,
                    &font,
                    label,
                );
            }
        }
    }
    // 内层描边（偏移1像素）
    for dy in -1i32..=1i32 {
        for dx in -1i32..=1i32 {
            if dx == 0 && dy == 0 {
                continue;
            }
            draw_text_mut(
                image,
                Rgba([0, 0, 0, 255]), // 完全不透明的黑色
                (x as i32 + dx).max(0),
                (y as i32 + dy).max(0),
                scale,
                &font,
                label,
            );
        }
    }

    // 绘制文字本体
    draw_text_mut(image, text_color, x as i32, y as i32, scale, &font, label);

    Ok(())
}

fn ensure_vertex_normals(mesh: &PlantMesh) -> Vec<Vec3> {
    if mesh.normals.len() == mesh.vertices.len() && !mesh.normals.is_empty() {
        return mesh.normals.clone();
    }

    let mut normals = vec![Vec3::ZERO; mesh.vertices.len()];
    for tri in mesh.indices.chunks(3) {
        if tri.len() < 3 {
            continue;
        }
        let i0 = tri[0] as usize;
        let i1 = tri[1] as usize;
        let i2 = tri[2] as usize;
        if i0 >= mesh.vertices.len() || i1 >= mesh.vertices.len() || i2 >= mesh.vertices.len() {
            continue;
        }

        let v0 = mesh.vertices[i0];
        let v1 = mesh.vertices[i1];
        let v2 = mesh.vertices[i2];
        let normal = (v1 - v0).cross(v2 - v0);
        if normal.length_squared() > f32::EPSILON {
            let n = normal.normalize();
            normals[i0] += n;
            normals[i1] += n;
            normals[i2] += n;
        }
    }

    for normal in &mut normals {
        if normal.length_squared() > f32::EPSILON {
            *normal = normal.normalize();
        } else {
            *normal = Vec3::Y;
        }
    }

    normals
}

fn compute_bounds(vertices: &[Vec3]) -> Option<(Vec3, Vec3)> {
    if vertices.is_empty() {
        return None;
    }

    let mut min = vertices[0];
    let mut max = vertices[0];
    for vertex in vertices.iter().skip(1) {
        min = min.min(*vertex);
        max = max.max(*vertex);
    }
    Some((min, max))
}

#[inline]
fn edge(a: Vec2, b: Vec2, c: Vec2) -> f32 {
    (c.x - a.x) * (b.y - a.y) - (c.y - a.y) * (b.x - a.x)
}
