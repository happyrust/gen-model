//! RVM 基准快照的结构定义与读写。
//!
//! 快照是 import 与 compare 之间的唯一契约：import 把 E3D 导出的 RVM/ATT
//! 解析成它，compare 只读它，不再碰 RVM 文件。这样「RVM 解析得对不对」与
//! 「对拍判得对不对」可以分开排查，快照本身也能进版本控制、人工 diff。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

/// 快照文件格式版本。结构不兼容时递增，compare 侧据此拒绝旧快照。
pub const SNAPSHOT_VERSION: u32 = 2;

/// 基准 RVM 是按哪种口径导出的。
///
/// L3 的成员 AABB 判定只在窄口径下成立：宽口径把保温与障碍/预留体一并写进 RVM，
/// 而生成侧有意不产出它们，两边比的不是同一个东西。这件事过去只写在
/// [`super::compare`] 的模块注释里，判定逻辑照着它走却从不校验，而
/// `scripts/e3d/rvm_export_c_iy_1r330_b.mac` 恰好是宽口径导的——一个电缆槽的
/// 预留净空（catalogue 里 `OBST=1` 的那个 SBOX）于是被算成了 100mm 的几何缺陷。
/// RVM 流里的预留体就是个普通 box，`GeometryType` 分不出来，所以口径只能由导出
/// 方声明、以数据形式随快照走。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportScope {
    /// `repre insu off` + `repre obst off`：只有实体几何。
    Narrow,
    /// `repre insu on` + `repre obst on`：含保温与障碍/预留体。
    Wide,
    /// 未声明，含本字段引入之前的旧快照。按判不了处理，不得放行。
    #[default]
    Unknown,
}

impl ExportScope {
    /// 成员 AABB 能不能直接与生成侧比。
    pub fn allows_aabb_verdict(self) -> bool {
        matches!(self, ExportScope::Narrow)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ExportScope::Narrow => "narrow",
            ExportScope::Wide => "wide",
            ExportScope::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RvmSnapshot {
    pub meta: SnapshotMeta,
    pub members: Vec<RvmMember>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SnapshotMeta {
    pub version: u32,
    pub dbnum: u32,
    /// RVM 里推断出的根元素名（取最外层 group 名）。
    pub root_name: Option<String>,
    pub rvm_file: String,
    pub att_files: Vec<String>,
    pub imported_at: String,
    pub member_count: usize,
    pub geometry_count: usize,
    pub resolved: usize,
    pub unresolved: usize,
    /// 导出这份 RVM 时用的口径，L3 的 AABB 判定资格由它决定（见 [`ExportScope`]）。
    /// 旧快照没有这个字段，反序列化后落到 `Unknown`，对拍侧据此拒绝给出 PASS。
    #[serde(default)]
    pub export_scope: ExportScope,
    /// 按 RVM GeometryType 分桶计数：Primitive / Obstruction / Insulation。
    /// 对拍侧的豁免规则依赖这个分桶。
    ///
    /// 注意它只在**成员**粒度上有效：预留净空这类图元是寄生在真实构件内部的，
    /// 与实体几何同为 `Primitive`，靠这个分桶筛不掉，只能靠 `export_scope`。
    pub geo_type_counts: BTreeMap<String, usize>,
    /// 重新三角化后仍没有有效世界包围盒的几何数。
    pub degenerate_bbox_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RvmMember {
    /// 组在 RVM 层级里的完整路径，形如 `/SITE/ZONE/PIPE/BRAN`。
    pub path: String,
    /// 组名末段。命名元素是 E3D NAME；未命名元素是 `<NOUN> <n> of <OWNER>`。
    pub name: String,
    /// 推导出的 PDMS 四字名词，未知时为 None。
    pub noun: Option<String>,
    /// 默认命名解析出的 owner 描述（命名元素为 None）。
    pub owner_desc: Option<String>,
    /// 默认命名里的序号，即「owner 下第 n 个同类型子元素」。
    pub ordinal: Option<usize>,
    /// 真实 PDMS refno，形如 `24384/22404`。身份解析未接入时为 None。
    pub refno: Option<String>,
    /// `pe_name` / `default_name` / `att_direct` / `stable_hash`
    pub identity_source: String,
    pub resolved: bool,
    /// 由路径派生的稳定 id，身份未解析时充当 join key。
    pub stable_id: u64,
    pub parent_stable_id: Option<u64>,
    /// CNTB 给出的世界平移（mm）。注意 E3D 导出的是绝对世界坐标，
    /// 不是相对父级的增量，逐级累加会把坐标乘上嵌套深度。
    pub translation_mm: [f32; 3],
    /// 本组全部可渲染几何按 `geometry.transform` 变换后的包围盒并集（mm）。
    pub aabb_world_mm: Option<[f64; 6]>,
    /// ATT 属性（若提供了 .att 文件）。
    pub attrs: Option<serde_json::Value>,
    pub geometries: Vec<RvmGeometry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RvmGeometry {
    pub index: usize,
    /// rvm-rs 的原语种类：Cylinder / Snout / CircularTorus / Box / ...
    pub kind: String,
    /// Primitive / Obstruction / Insulation
    pub geo_type: String,
    /// 原语参数，L2 参数级对拍的数据来源。
    pub detail: serde_json::Value,
    /// 列主序 3x3 + 平移（mm）。
    pub transform: serde_json::Value,
    pub bbox_world_mm: Option<[f64; 6]>,
    /// 三角化后没有有效世界顶点时为 true，空间对拍需跳过。
    pub bbox_degenerate: bool,
    /// 颜色/透明度等非几何属性，保留供报告展示。
    pub extra: serde_json::Value,
}

impl RvmSnapshot {
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                fs::create_dir_all(dir)
                    .with_context(|| format!("创建快照目录失败: {}", dir.display()))?;
            }
        }
        let text = serde_json::to_string_pretty(self).context("序列化 RVM 快照失败")?;
        fs::write(path, text).with_context(|| format!("写入快照失败: {}", path.display()))?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("读取快照失败: {}", path.display()))?;
        let snapshot: RvmSnapshot = serde_json::from_str(&text)
            .with_context(|| format!("解析快照失败: {}", path.display()))?;
        if snapshot.meta.version != SNAPSHOT_VERSION {
            anyhow::bail!(
                "快照版本不匹配: 文件为 v{}，当前实现为 v{}",
                snapshot.meta.version,
                SNAPSHOT_VERSION
            );
        }
        Ok(snapshot)
    }

    pub fn print_summary(&self) {
        let m = &self.meta;
        println!("RVM 基准快照");
        println!("  dbnum          : {}", m.dbnum);
        println!(
            "  root           : {}",
            m.root_name.as_deref().unwrap_or("<未知>")
        );
        println!("  rvm            : {}", m.rvm_file);
        if m.att_files.is_empty() {
            println!("  att            : <未提供>");
        } else {
            println!("  att            : {}", m.att_files.join(", "));
        }
        println!(
            "  成员 / 几何    : {} / {}",
            m.member_count, m.geometry_count
        );
        println!("  身份 已解析/未解析: {} / {}", m.resolved, m.unresolved);
        let buckets = m
            .geo_type_counts
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" ");
        println!(
            "  geo_type 分桶  : {}",
            if buckets.is_empty() {
                "<空>".into()
            } else {
                buckets
            }
        );
        println!("  退化包围盒     : {}", m.degenerate_bbox_count);
    }
}
