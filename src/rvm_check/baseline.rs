//! 基准的版本标记与比对前的版本校验（GitHub #126）。
//!
//! 基准 RVM 必须与项目数据是同一个 E3D 版本：拿 2.1 的基准去比 3.1 的项目数据（或反过来）会出少量构件级
//! 的假差异——`only_baseline` / `only_generated` / `off` / `kind_differ` 各来一点，形态与真的模型差异分不开。
//! 2026-11-08 那两批（`output/rvm-check-fused/all`、`oldpath-79`）就是这么踩的：`_deploy` 里的基准是
//! E3D 2.1 导出（见其 `PROVENANCE.txt`），拿去比本地的 3.1 库。
//!
//! 这里只做两件事：**读**基准目录的版本标记（本模块写的 `baseline-provenance.json`，或人写的
//! `PROVENANCE.txt`），以及把它与当前项目数据的 E3D 版本对一下。不拒绝运行——同一份数据有两种合法写法，
//! 版本标记也可能缺失；结论随运行记录走，查询与界面按它区分。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 本模块写的版本标记：基准目录（`--baseline-dir`）或产物目录根下。
pub const MARKER_FILE: &str = "baseline-provenance.json";
/// 人写的说明文件（`_deploy/ams-server/backend/baselines/PROVENANCE.txt` 就是这一份）。
pub const PROVENANCE_TXT: &str = "PROVENANCE.txt";

/// 基准的版本标记：E3D 版本与数据来源。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineProvenance {
    /// 归一后的写法：`e3d210` / `e3d31`。
    pub e3d_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mdb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exported_at: Option<String>,
    /// 导出批次（`output/rvm-check-batch/20261108-e3d21-full` 一类）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<String>,
    /// 从哪个文件读到的（`baseline-provenance.json` / `PROVENANCE.txt`）。读的是 JSON 时没有这一格。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_from: Option<String>,
}

/// 比对前的版本结论。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VersionVerdict {
    /// 基准与项目数据同一个 E3D 版本。
    Match,
    /// 两边版本不同（或版本标记缺、项目侧版本不明）：这一批比出来的差异里会混进版本错配的假差异。
    Mismatch {
        baseline: String,
        project: String,
    },
    /// 基准没有版本标记（旧基准）：照跑，但无法校验。
    Unmarked,
}

/// 一次版本校验的全部事实：基准侧读到了什么、项目侧是什么版本、找过哪些位置。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineVersionCheck {
    /// 基准侧的版本标记；读不到就是 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<BaselineProvenance>,
    /// 当前项目数据用的 E3D 版本（`DbOption.e3d_version`，没配就是默认的 `e3d210`）。
    pub project_version: String,
    /// 找过的位置（给人看的那句话里点名）。
    pub looked_for: Vec<String>,
}

/// 版本不一致那一笔：进 `run.json`、`summary.json` 与库里的运行行（`RunRow::baseline_version`），
/// 查询与界面据此区分「这一次的差异里混进了版本错配」。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineVersionMismatch {
    /// 基准的 E3D 版本（归一后）。
    pub baseline_version: String,
    /// 项目数据的 E3D 版本（归一后）。
    pub project_version: String,
    /// 基准标记从哪个文件读到的。
    pub marker: String,
    /// 基准的数据来源（标记里写了的话）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// 基准的导出批次（标记里写了的话）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<String>,
}

impl BaselineVersionMismatch {
    /// 给人看的那一句：两边各是什么版本、后果是什么、怎么办。
    pub fn warning(&self) -> String {
        format!(
            "警告：基准的 E3D 版本 {} 与项目数据的 {} 不一致（基准标记：{}{}{}）。\
             两版数据之间的差异会以 only_baseline / only_generated / off / kind_differ 的形式混进结果，\
             与真的模型差异分不开——换一份同版本的基准再比（导出基准与比对用同一套项目数据）。",
            self.baseline_version,
            self.project_version,
            self.marker,
            self.project
                .as_deref()
                .map(|project| format!("，数据来源 {project}"))
                .unwrap_or_default(),
            self.batch
                .as_deref()
                .map(|batch| format!("，导出批次 {batch}"))
                .unwrap_or_default(),
        )
    }
}

impl BaselineVersionCheck {
    pub fn verdict(&self) -> VersionVerdict {
        match &self.baseline {
            Some(provenance) if provenance.e3d_version == self.project_version => VersionVerdict::Match,
            Some(provenance) => VersionVerdict::Mismatch {
                baseline: provenance.e3d_version.clone(),
                project: self.project_version.clone(),
            },
            None => VersionVerdict::Unmarked,
        }
    }

    /// 版本不一致时的那一笔；一致或没有标记时为 `None`（一致时运行记录与改动前逐字段相同）。
    pub fn mismatch(&self) -> Option<BaselineVersionMismatch> {
        let VersionVerdict::Mismatch {
            baseline,
            project,
        } = self.verdict()
        else {
            return None;
        };
        let provenance = self.baseline.as_ref()?;
        Some(BaselineVersionMismatch {
            baseline_version: baseline,
            project_version: project,
            marker: provenance
                .read_from
                .clone()
                .unwrap_or_else(|| MARKER_FILE.to_string()),
            project: provenance.project.clone(),
            batch: provenance.batch.clone(),
        })
    }

    /// 没有版本标记的旧基准那一句：照跑，但这一批的结论没有版本校验。
    pub fn unmarked_note(&self) -> Option<String> {
        (self.verdict() == VersionVerdict::Unmarked).then(|| {
            format!(
                "提示：基准没有 E3D 版本标记（找过：{}），这次核对无法校验基准与项目数据的版本是否一致。\
                 项目数据是 {}；基准若是另一个版本的导出，差异里会混进版本错配的假差异。\
                 在基准目录写一份 {MARKER_FILE}（{{\"e3d_version\": \"{}\"}}）即可开始校验。",
                if self.looked_for.is_empty() {
                    "（未给基准目录）".to_string()
                } else {
                    self.looked_for.join("、")
                },
                self.project_version,
                self.project_version
            )
        })
    }
}

/// 版本写法归一：`e3d210` / `E3D 2.1` / `2.10` → `e3d210`；`e3d31` / `3.1` → `e3d31`。
/// 认不出给 `None`（不拿它去比，免得把 `unknown` 当成一个版本）。
pub fn normalize_e3d_version(raw: &str) -> Option<String> {
    let text = raw.trim().to_ascii_lowercase();
    let digits: String = text
        .chars()
        .filter(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect();
    if digits.is_empty() {
        return None;
    }
    let trimmed = digits.trim_matches('.').to_string();
    // 已经写成 `e3d210` 的：去掉点之后就是它。
    let compact: String = trimmed.chars().filter(|ch| *ch != '.').collect();
    match compact.as_str() {
        "210" | "21" => Some("e3d210".to_string()),
        "31" | "310" | "3" => Some("e3d31".to_string()),
        other => Some(format!("e3d{other}")),
    }
}

/// 当前项目数据用的 E3D 版本（归一后）。配置了 `e3d_version` 就用它，否则与 schema 加载的默认同一口径。
pub fn project_e3d_version() -> String {
    crate::options::configured_e3d_version()
        .ok()
        .flatten()
        .as_deref()
        .and_then(normalize_e3d_version)
        .unwrap_or_else(|| {
            normalize_e3d_version(crate::data_interface::direct_store::DEFAULT_E3D_VERSION)
                .unwrap_or_else(|| "e3d210".to_string())
        })
}

/// 从 `PROVENANCE.txt` 那种人写的说明里读 E3D 版本：认 `E3D 版本: e3d210` / `E3D 版本：e3d210` 这一行。
fn version_from_provenance_text(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        let rest = line
            .strip_prefix("E3D 版本")
            .or_else(|| line.strip_prefix("E3D版本"))
            .or_else(|| line.strip_prefix("e3d 版本"))?;
        let rest = rest.trim_start_matches([':', '：']).trim();
        if let Some(version) = normalize_e3d_version(rest) {
            return Some(version);
        }
    }
    None
}

/// 在 `dir` 里找版本标记：先看本模块写的 JSON，再看人写的 `PROVENANCE.txt`。
pub fn read_marker(dir: &Path) -> Option<BaselineProvenance> {
    let json = dir.join(MARKER_FILE);
    if let Ok(bytes) = std::fs::read(&json)
        && let Ok(mut provenance) = serde_json::from_slice::<BaselineProvenance>(&bytes)
    {
        provenance.e3d_version = normalize_e3d_version(&provenance.e3d_version)?;
        return Some(provenance);
    }
    let text = std::fs::read_to_string(dir.join(PROVENANCE_TXT)).ok()?;
    let version = version_from_provenance_text(&text)?;
    let line_with = |needle: &str| {
        text.lines()
            .map(str::trim)
            .find(|line| line.starts_with(needle))
            .map(|line| {
                line.trim_start_matches(needle)
                    .trim_start_matches([':', '：'])
                    .trim()
                    .to_string()
            })
            .filter(|value| !value.is_empty())
    };
    Some(BaselineProvenance {
        e3d_version: version,
        project: line_with("项目数据"),
        mdb: None,
        exported_at: line_with("生成时间"),
        batch: line_with("导出批次"),
        read_from: Some(PROVENANCE_TXT.to_string()),
    })
}

/// 写版本标记（导出基准之后调；同一个目录重写覆盖）。返回写到的路径。
pub fn write_marker(dir: &Path, provenance: &BaselineProvenance) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(MARKER_FILE);
    let body = serde_json::to_vec_pretty(provenance).unwrap_or_default();
    std::fs::write(&path, body)?;
    Ok(path)
}

/// 校验基准版本：`baseline_dir` 给了就看它，否则看产物目录根（`--stage compare` 复用旧产物时，
/// 上一轮导出留下的标记就在那里）。
pub fn check(
    out_root: &Path,
    baseline_dir: Option<&Path>,
    project_version: &str,
) -> BaselineVersionCheck {
    let dirs: Vec<PathBuf> = match baseline_dir {
        Some(dir) => vec![dir.to_path_buf()],
        None => vec![out_root.to_path_buf()],
    };
    let mut looked_for = Vec::new();
    let mut baseline = None;
    for dir in &dirs {
        looked_for.push(dir.join(MARKER_FILE).display().to_string());
        looked_for.push(dir.join(PROVENANCE_TXT).display().to_string());
        if baseline.is_none() {
            baseline = read_marker(dir);
        }
    }
    BaselineVersionCheck {
        baseline,
        project_version: project_version.to_string(),
        looked_for,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rvm-check-baseline-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// GitHub #126：基准目录里的 `PROVENANCE.txt`（`_deploy` 里那一份就是它）与项目数据版本不一致时，
    /// 校验给得出结论与那一句告警；一致时没有那一笔；没有标记的旧基准照跑、给一句「无法校验」。
    #[test]
    fn the_baseline_version_marker_is_read_and_compared_with_the_project_version() {
        assert_eq!(normalize_e3d_version("e3d210").as_deref(), Some("e3d210"));
        assert_eq!(normalize_e3d_version("E3D 2.1").as_deref(), Some("e3d210"));
        assert_eq!(normalize_e3d_version("3.1").as_deref(), Some("e3d31"));
        assert_eq!(normalize_e3d_version("e3d31").as_deref(), Some("e3d31"));
        assert_eq!(normalize_e3d_version("unknown"), None);

        let dir = temp_dir("provenance");
        std::fs::write(
            dir.join(PROVENANCE_TXT),
            "RVM 基准文件来源与版本说明\n\
             \n\
             生成时间: 2026-11-08\n\
             导出批次: output/rvm-check-batch/20261108-e3d21-full\n\
             E3D 版本: e3d210（AVEVA Everything3D 2.1 / 2.10，schema pack e3d210）\n\
             项目数据: AMS 项目 E3D 2.1 数据（AvevaMarineSample / AvevaCatalogue）\n",
        )
        .unwrap();
        let read = read_marker(&dir).expect("PROVENANCE.txt 里有版本");
        assert_eq!(read.e3d_version, "e3d210");
        assert_eq!(read.read_from.as_deref(), Some(PROVENANCE_TXT));
        assert_eq!(
            read.batch.as_deref(),
            Some("output/rvm-check-batch/20261108-e3d21-full")
        );
        assert!(read.project.unwrap().starts_with("AMS 项目 E3D 2.1 数据"));

        // 3.1 的项目数据配 2.1 的基准：结论是不一致，那一笔带两边的版本与标记来源。
        let mismatched = check(&dir, Some(&dir), "e3d31");
        assert_eq!(
            mismatched.verdict(),
            VersionVerdict::Mismatch {
                baseline: "e3d210".to_string(),
                project: "e3d31".to_string(),
            }
        );
        let stamp = mismatched.mismatch().expect("不一致要留痕");
        assert_eq!(stamp.baseline_version, "e3d210");
        assert_eq!(stamp.project_version, "e3d31");
        assert_eq!(stamp.marker, PROVENANCE_TXT);
        let warning = stamp.warning();
        assert!(
            warning.starts_with("警告：")
                && warning.contains("e3d210")
                && warning.contains("e3d31")
                && warning.contains(PROVENANCE_TXT)
                && warning.contains("换一份同版本的基准"),
            "{warning}"
        );
        assert_eq!(mismatched.unmarked_note(), None);

        // 同一个版本：没有那一笔，也没有「无法校验」的提示。
        let matched = check(&dir, Some(&dir), "e3d210");
        assert_eq!(matched.verdict(), VersionVerdict::Match);
        assert_eq!(matched.mismatch(), None);
        assert_eq!(matched.unmarked_note(), None);

        // 没有标记的旧基准：照跑，提示无法校验（点名找过的位置）。
        let empty = temp_dir("unmarked");
        let unmarked = check(&empty, Some(&empty), "e3d210");
        assert_eq!(unmarked.verdict(), VersionVerdict::Unmarked);
        assert_eq!(unmarked.mismatch(), None);
        let note = unmarked.unmarked_note().expect("没有标记要提示");
        assert!(
            note.starts_with("提示：") && note.contains(MARKER_FILE) && note.contains("e3d210"),
            "{note}"
        );

        // 导出基准时写下的标记（本模块写的 JSON）优先于 PROVENANCE.txt，写出去也读得回。
        let written = BaselineProvenance {
            e3d_version: "e3d31".to_string(),
            project: Some("AvevaMarineSample".to_string()),
            mdb: Some("/ALL".to_string()),
            exported_at: Some("2026-11-13T16:00:00+08:00".to_string()),
            batch: None,
            read_from: None,
        };
        write_marker(&dir, &written).unwrap();
        let back = read_marker(&dir).expect("JSON 标记");
        assert_eq!(back.e3d_version, "e3d31");
        assert_eq!(back.read_from, None);
        let now = check(&dir, Some(&dir), "e3d210");
        assert_eq!(now.mismatch().expect("反过来也不一致").marker, MARKER_FILE);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }
}
