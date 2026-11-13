//! 嵌入程序图标和版本信息。清单（DPI 感知等）由 gpui-pre 默认开启的 `windows-manifest`
//! 特性嵌入，这里不能再放 RT_MANIFEST，否则链接时资源重复。
//! 从 deploy-ui 沿用，只改了名字。
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = std::env::var("OUT_DIR").unwrap();
    let out = Path::new(&out);
    let ico = out.join("app.ico");
    std::fs::write(&ico, make_icon(&[16, 32, 48, 256])).unwrap();

    let mut rc = String::from("#pragma code_page(65001)\n");
    // GPUI 的窗口类按资源 ID 1 加载图标（LoadImageW(module, 1, IMAGE_ICON)），资源管理器也取第一个图标
    rc += &format!("1 ICON \"{}\"\n", ico.display().to_string().replace('\\', "/"));
    rc += r#"1 VERSIONINFO
FILEVERSION 0,1,0,0
PRODUCTVERSION 0,1,0,0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "080404b0"
        BEGIN
            VALUE "CompanyName", "AIOS"
            VALUE "FileDescription", "AIOS 部署面板（GPUI 原型）"
            VALUE "FileVersion", "0.1.0"
            VALUE "ProductName", "AIOS 部署面板"
            VALUE "ProductVersion", "0.1.0"
            VALUE "OriginalFilename", "deploy-dash.exe"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x804, 1200
    END
END
"#;
    let rc_path = out.join("app.rc");
    std::fs::write(&rc_path, rc).unwrap();
    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}

/// 生成一个多尺寸 32 位 BMP 图标：蓝底圆角方块，中间白色圆点，右下角绿色状态点。
fn make_icon(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| icon_bmp(s)).collect();
    let mut ico = Vec::new();
    ico.extend_from_slice(&0u16.to_le_bytes());
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&s, img) in sizes.iter().zip(&images) {
        let b = if s >= 256 { 0u8 } else { s as u8 };
        ico.extend_from_slice(&[b, b, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(img.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        ico.extend_from_slice(&img);
    }
    ico
}

fn icon_bmp(s: u32) -> Vec<u8> {
    let mask_row = s.div_ceil(32) * 4;
    let mut v = Vec::new();
    for x in [40u32, s, s * 2] {
        v.extend_from_slice(&x.to_le_bytes());
    }
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&32u16.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&(s * s * 4 + mask_row * s).to_le_bytes());
    v.extend_from_slice(&[0u8; 16]);
    let f = s as f32;
    let radius = f * 0.22;
    for row in 0..s {
        let y = (s - 1 - row) as f32 + 0.5;
        for col in 0..s {
            let x = col as f32 + 0.5;
            let dx = (x - f / 2.0).abs() - (f / 2.0 - radius - f * 0.04);
            let dy = (y - f / 2.0).abs() - (f / 2.0 - radius - f * 0.04);
            let outside = dx.max(0.0).hypot(dy.max(0.0)) - radius;
            let mut px = if outside > 0.5 { [0u8, 0, 0, 0] } else { [0xEB, 0x63, 0x25, 0xFF] };
            if (x - f / 2.0).hypot(y - f * 0.46) < f * 0.2 && outside <= 0.5 {
                px = [0xFF, 0xFF, 0xFF, 0xFF];
            }
            if (x - f * 0.74).hypot(y - f * 0.74) < f * 0.13 {
                px = [0x5E, 0xC5, 0x22, 0xFF];
            }
            v.extend_from_slice(&px);
        }
    }
    v.extend(std::iter::repeat_n(0u8, (mask_row * s) as usize));
    v
}
