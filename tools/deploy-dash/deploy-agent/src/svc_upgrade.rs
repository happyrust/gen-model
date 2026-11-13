//! 单服务版本探测、版本对比、分块推送暂存、备份替换升级与回滚。
//!
//! 设计目标：
//! 1. 为 `database`、`backend`、`rs_server`、`frontend` 提供统一的本地版本 + SHA-256 探测（带缓存）；
//! 2. 严禁执行 `rs-server.exe --version`（该程序无 `--version`，直接跑会尝试连库启动），改用二进制内嵌串扫描；
//! 3. 支持浏览器分块推送（`PUT /v1/services/upload`）及服务端路径对比（`POST /v1/services/compare-path`）；
//! 4. 升级流程：停服务 → 备份旧版 → 原子替换新文件 → 起服务并做健康检查（失败自动回滚）。

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::timefmt;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub const ALL_SERVICES: [&str; 4] = ["database", "backend", "rs_server", "frontend"];

pub fn is_valid_service(name: &str) -> bool {
    ALL_SERVICES.contains(&name)
}

pub fn service_title(name: &str) -> &'static str {
    match name {
        "database" => "数据库",
        "backend" => "后端",
        "rs_server" => "rs-server",
        "frontend" => "前端",
        _ => "未知服务",
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactVersionInfo {
    pub exists: bool,
    pub is_dir: bool,
    pub path: String,
    pub version: String,
    pub build_id: String,
    pub commit: String,
    pub built_at: String,
    pub modified_at: String,
    pub bytes: u64,
    pub file_count: u64,
    pub sha256: String,
    pub short_sha: String,
    pub note: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct VersionDiff {
    /// `"up_to_date"` | `"update_available"` | `"not_installed"`
    pub status: String,
    pub same_sha256: bool,
    pub version_changed: bool,
    pub build_id_changed: bool,
    pub bytes_delta: i64,
    pub summary: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BackupInfo {
    pub exists: bool,
    pub saved_at: String,
    pub artifact: Option<ArtifactVersionInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServiceVersionEntry {
    pub service: String,
    pub title: String,
    pub installed: ArtifactVersionInfo,
    pub staged: Option<ArtifactVersionInfo>,
    pub diff: Option<VersionDiff>,
    pub backup: BackupInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServicesVersionsResponse {
    pub schema: u32,
    pub agent_version: String,
    pub root: String,
    pub services: Vec<ServiceVersionEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StagedMeta {
    artifact_rel_path: String,
    original_name: String,
    staged_at: String,
    info: ArtifactVersionInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BackupMeta {
    backup_rel_path: String,
    saved_at: String,
    info: ArtifactVersionInfo,
}

#[derive(Clone)]
struct CachedProbe {
    len: u64,
    mtime_secs: u64,
    info: ArtifactVersionInfo,
}

pub struct SvcUpgradeManager {
    root: PathBuf,
    cache: Mutex<HashMap<PathBuf, CachedProbe>>,
    op_lock: Mutex<()>,
}

impl SvcUpgradeManager {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            cache: Mutex::new(HashMap::new()),
            op_lock: Mutex::new(()),
        }
    }

    pub fn staging_root(&self) -> PathBuf {
        self.root.join("run").join("gui").join("svc-staging")
    }

    pub fn backup_root(&self) -> PathBuf {
        self.root.join("run").join("gui").join("svc-backup")
    }

    fn svc_staging_dir(&self, svc: &str) -> PathBuf {
        self.staging_root().join(svc)
    }

    fn svc_backup_dir(&self, svc: &str) -> PathBuf {
        self.backup_root().join(svc)
    }

    /// 从 stack.toml 读取每个服务的目标相对/绝对路径；读不到则用安装包缺省路径。
    pub fn target_path_for(&self, svc: &str) -> PathBuf {
        let default_rel = match svc {
            "database" => "bin/surreal.exe",
            "backend" => "bin/aios-database.exe",
            "rs_server" => "bin/rs-server.exe",
            "frontend" => "www",
            _ => "",
        };
        let toml_path = self.root.join("stack.toml");
        if let Ok(text) = fs::read_to_string(&toml_path)
            && let Ok(val) = text.parse::<toml::Value>()
        {
            let key = match svc {
                "frontend" => "root",
                _ => "exe",
            };
            if let Some(raw) = val.get(svc).and_then(|s| s.get(key)).and_then(|v| v.as_str()) {
                let trimmed = raw.trim();
                if !trimmed.is_empty() {
                    let p = PathBuf::from(trimmed);
                    return if p.is_absolute() { p } else { self.root.join(p) };
                }
            }
        }
        self.root.join(default_rel)
    }

    fn release_component_hint(&self, svc: &str, sha256: &str) -> Option<(String, String, String, String)> {
        let path = self.root.join("release.json");
        let text = fs::read_to_string(path).ok()?;
        let val: serde_json::Value = serde_json::from_str(&text).ok()?;
        let want_id = match svc {
            "database" => "database",
            "backend" => "backend",
            "rs_server" => "rs-server",
            "frontend" => "plant3d-web",
            _ => return None,
        };
        let comp = val.get("components")?.as_array()?.iter().find(|c| c.get("id").and_then(|x| x.as_str()) == Some(want_id))?;
        let comp_sha = comp.get("sha256").and_then(|x| x.as_str()).unwrap_or("");
        if !sha256.is_empty() && !comp_sha.is_empty() && !comp_sha.eq_ignore_ascii_case(sha256) {
            return None;
        }
        let version = comp.get("version").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let build_id = comp.get("build_id").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let commit = comp.get("commit").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let built_at = comp.get("built_at").and_then(|x| x.as_str()).unwrap_or("").to_string();
        Some((version, build_id, commit, built_at))
    }

    pub fn probe_installed(&self, svc: &str) -> ArtifactVersionInfo {
        let target = self.target_path_for(svc);
        let mut info = self.probe_path(svc, &target, true);
        if info.exists
            && let Some((ver, bid, commit, built_at)) = self.release_component_hint(svc, &info.sha256)
        {
            if info.version.is_empty() {
                info.version = ver;
            }
            if info.build_id.is_empty() {
                info.build_id = bid;
            }
            if info.commit.is_empty() {
                info.commit = commit;
            }
            if info.built_at.is_empty() {
                info.built_at = built_at;
            }
        }
        info
    }

    pub fn probe_path(&self, svc: &str, path: &Path, use_cache: bool) -> ArtifactVersionInfo {
        if svc == "frontend" {
            return probe_frontend_dir(&self.root, path);
        }
        let Ok(meta) = fs::metadata(path) else {
            return ArtifactVersionInfo {
                exists: false,
                path: rel_or_abs(&self.root, path),
                ..Default::default()
            };
        };
        if !meta.is_file() {
            return ArtifactVersionInfo {
                exists: false,
                path: rel_or_abs(&self.root, path),
                note: "目标不是文件".into(),
                ..Default::default()
            };
        }
        let len = meta.len();
        let mtime_secs = system_time_secs(meta.modified().ok());
        if use_cache && let Ok(guard) = self.cache.lock()
            && let Some(c) = guard.get(path)
            && c.len == len
            && c.mtime_secs == mtime_secs
        {
            return c.info.clone();
        }
        let mut info = probe_binary_file(svc, &self.root, path, len, mtime_secs);
        info.short_sha = short_sha(&info.sha256);
        if use_cache && let Ok(mut guard) = self.cache.lock() {
            guard.insert(path.to_path_buf(), CachedProbe { len, mtime_secs, info: info.clone() });
        }
        info
    }

    fn invalidate_cache(&self, path: &Path) {
        if let Ok(mut guard) = self.cache.lock() {
            guard.remove(path);
        }
    }

    pub fn load_staged(&self, svc: &str) -> Option<ArtifactVersionInfo> {
        let meta_path = self.svc_staging_dir(svc).join("staged.json");
        let text = fs::read_to_string(meta_path).ok()?;
        let meta: StagedMeta = serde_json::from_str(&text).ok()?;
        let artifact = self.svc_staging_dir(svc).join(&meta.artifact_rel_path);
        if !artifact.exists() {
            return None;
        }
        Some(meta.info)
    }

    pub fn load_backup(&self, svc: &str) -> BackupInfo {
        let bdir = self.svc_backup_dir(svc);
        let meta_path = bdir.join("backup.json");
        let Ok(text) = fs::read_to_string(meta_path) else {
            return BackupInfo::default();
        };
        let Ok(meta) = serde_json::from_str::<BackupMeta>(&text) else {
            return BackupInfo::default();
        };
        if !bdir.join(&meta.backup_rel_path).exists() {
            return BackupInfo::default();
        }
        BackupInfo {
            exists: true,
            saved_at: meta.saved_at,
            artifact: Some(meta.info),
        }
    }

    pub fn entry_for(&self, svc: &str) -> ServiceVersionEntry {
        let installed = self.probe_installed(svc);
        let staged = self.load_staged(svc);
        let diff = staged.as_ref().map(|s| compute_diff(&installed, s));
        let backup = self.load_backup(svc);
        ServiceVersionEntry {
            service: svc.to_string(),
            title: service_title(svc).to_string(),
            installed,
            staged,
            diff,
            backup,
        }
    }

    pub fn list_all(&self) -> Vec<ServiceVersionEntry> {
        ALL_SERVICES.iter().map(|s| self.entry_for(s)).collect()
    }

    pub fn clear_stage(&self, svc: &str) -> anyhow::Result<()> {
        if !is_valid_service(svc) {
            anyhow::bail!("不支持的服务：{svc}");
        }
        let dir = self.svc_staging_dir(svc);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }

    /// 接收一个上传分块；当 `finalize` 为 true 时解包（若 `.zip`）并探测暂存版本信息。
    #[allow(clippy::too_many_arguments)]
    pub fn upload_chunk(
        &self,
        svc: &str,
        rel_path: &str,
        offset: u64,
        total: Option<u64>,
        reset: bool,
        finalize: bool,
        data: &[u8],
    ) -> anyhow::Result<Option<ServiceVersionEntry>> {
        if !is_valid_service(svc) {
            anyhow::bail!("不支持的服务：{svc}");
        }
        let safe_rel = sanitize_rel_path(rel_path)?;
        let stage_dir = self.svc_staging_dir(svc);
        if reset && stage_dir.exists() {
            fs::remove_dir_all(&stage_dir)?;
        }
        let raw_dir = stage_dir.join("raw");
        let file_path = raw_dir.join(&safe_rel);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut f = fs::OpenOptions::new().create(true).read(true).write(true).open(&file_path)?;
        if offset == 0 && total == Some(data.len() as u64) {
            f.set_len(0)?;
        }
        f.seek(SeekFrom::Start(offset))?;
        f.write_all(data)?;
        if let Some(t) = total
            && offset + (data.len() as u64) >= t
        {
            f.set_len(t)?;
        }
        f.flush()?;
        drop(f);

        if finalize {
            self.finalize_stage(svc, Some(&safe_rel))?;
            Ok(Some(self.entry_for(svc)))
        } else {
            Ok(None)
        }
    }

    /// 暂存完成：如有 `.zip` 则解压，定位目标构件，探测版本与 SHA-256 并写入 `staged.json`。
    pub fn finalize_stage(&self, svc: &str, hint_rel: Option<&Path>) -> anyhow::Result<()> {
        let stage_dir = self.svc_staging_dir(svc);
        let raw_dir = stage_dir.join("raw");
        if !raw_dir.exists() {
            anyhow::bail!("没有上传内容");
        }
        let files = collect_files(&raw_dir);
        if files.is_empty() {
            anyhow::bail!("暂存区为空");
        }

        let mut search_root = raw_dir.clone();
        let mut orig_name = hint_rel.and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();

        // 若上传了单个 zip 包（或 hint 是 zip），解压到 extracted/
        let zip_candidate = if files.len() == 1 && files[0].extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) {
            Some(files[0].clone())
        } else {
            None
        };
        if let Some(zip_path) = zip_candidate {
            orig_name = zip_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let extracted = stage_dir.join("extracted");
            if extracted.exists() {
                fs::remove_dir_all(&extracted)?;
            }
            fs::create_dir_all(&extracted)?;
            extract_zip(&zip_path, &extracted)?;
            search_root = extracted;
        }

        let artifact_abs = locate_staged_artifact(svc, &search_root)?;
        let artifact_rel = artifact_abs
            .strip_prefix(&stage_dir)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| artifact_abs.clone());
        let mut info = self.probe_path(svc, &artifact_abs, false);
        info.path = if orig_name.is_empty() {
            artifact_rel.display().to_string().replace('\\', "/")
        } else {
            orig_name.clone()
        };
        if !info.exists {
            anyhow::bail!("无法识别暂存文件作为 {} 的新版本", service_title(svc));
        }

        let meta = StagedMeta {
            artifact_rel_path: artifact_rel.display().to_string().replace('\\', "/"),
            original_name: orig_name,
            staged_at: timefmt::now_iso(),
            info,
        };
        fs::write(stage_dir.join("staged.json"), serde_json::to_string_pretty(&meta)?)?;
        Ok(())
    }

    /// 对比服务器本地或共享路径：支持传单个文件/目录或整包目录；若 `stage = true` 同时拷贝进暂存区。
    pub fn compare_local_path(&self, svc_opt: Option<&str>, raw_path: &str, stage: bool) -> anyhow::Result<Vec<ServiceVersionEntry>> {
        let src = PathBuf::from(raw_path.trim());
        if !src.exists() {
            anyhow::bail!("路径不存在：{}", src.display());
        }
        let targets: Vec<&str> = match svc_opt {
            Some(s) if is_valid_service(s) => vec![s],
            Some(s) => anyhow::bail!("不支持的服务：{s}"),
            None => ALL_SERVICES.to_vec(),
        };

        let mut out = Vec::with_capacity(targets.len());
        for svc in targets {
            let candidate = if let Some(s) = resolve_candidate_in_path(svc, &src) {
                s
            } else if svc_opt.is_some() {
                anyhow::bail!("在 {} 中找不到 {} 的目标文件", src.display(), service_title(svc));
            } else {
                continue;
            };
            if stage {
                self.stage_from_local(svc, &candidate)?;
                out.push(self.entry_for(svc));
            } else {
                let installed = self.probe_installed(svc);
                let mut cand_info = self.probe_path(svc, &candidate, true);
                cand_info.path = candidate.display().to_string();
                let diff = Some(compute_diff(&installed, &cand_info));
                out.push(ServiceVersionEntry {
                    service: svc.to_string(),
                    title: service_title(svc).to_string(),
                    installed,
                    staged: Some(cand_info),
                    diff,
                    backup: self.load_backup(svc),
                });
            }
        }
        if out.is_empty() {
            anyhow::bail!("在 {} 中没有找到任何服务的可对比版本文件", src.display());
        }
        Ok(out)
    }

    fn stage_from_local(&self, svc: &str, candidate: &Path) -> anyhow::Result<()> {
        let stage_dir = self.svc_staging_dir(svc);
        if stage_dir.exists() {
            fs::remove_dir_all(&stage_dir)?;
        }
        let raw_dir = stage_dir.join("raw");
        fs::create_dir_all(&raw_dir)?;
        if svc == "frontend" {
            if candidate.is_dir() {
                let dest = raw_dir.join("www");
                copy_dir_all(candidate, &dest)?;
            } else {
                let name = candidate.file_name().ok_or_else(|| anyhow::anyhow!("无效文件名"))?;
                fs::copy(candidate, raw_dir.join(name))?;
            }
        } else {
            let name = candidate.file_name().ok_or_else(|| anyhow::anyhow!("无效文件名"))?;
            fs::copy(candidate, raw_dir.join(name))?;
        }
        self.finalize_stage(svc, candidate.file_name().map(Path::new))
    }

    /// 执行单服务升级：停服务 → 备份旧版本 → 替换新文件 → 重启并校验健康（失败则自动回滚）。
    pub fn apply_upgrade<F>(&self, svc: &str, force: bool, auto_start: bool, mut on_log: F) -> anyhow::Result<ServiceVersionEntry>
    where
        F: FnMut(&str),
    {
        if !is_valid_service(svc) {
            anyhow::bail!("不支持的服务：{svc}");
        }
        let _guard = self.op_lock.lock().map_err(|_| anyhow::anyhow!("升级锁异常"))?;

        let stage_dir = self.svc_staging_dir(svc);
        let meta_path = stage_dir.join("staged.json");
        let meta_text = fs::read_to_string(&meta_path).map_err(|_| anyhow::anyhow!("{} 还没有暂存的新版本文件，请先上传/选择版本", service_title(svc)))?;
        let meta: StagedMeta = serde_json::from_str(&meta_text)?;
        let staged_artifact = stage_dir.join(&meta.artifact_rel_path);
        if !staged_artifact.exists() {
            anyhow::bail!("暂存文件不存在：{}", staged_artifact.display());
        }

        let target = self.target_path_for(svc);
        let title = service_title(svc);
        on_log(&format!("[升级 {title}] 准备从暂存版本 {} ({}) 更新 {}", meta.info.version, meta.info.short_sha, target.display()));

        let has_stack_ps1 = self.root.join("stack.ps1").is_file() && self.root.join("stack.toml").is_file();
        let was_running = if has_stack_ps1 {
            on_log(&format!("[升级 {title}] 1/4 检查并停止运行中的 {title}..."));
            self.ps_stop_service(svc, force, &mut on_log)?
        } else {
            on_log(&format!("[升级 {title}] 1/4 离线目录无 stack.ps1，跳过停服务步骤"));
            false
        };

        // 2/4 备份当前版本
        on_log(&format!("[升级 {title}] 2/4 备份当前版本..."));
        let installed_before = self.probe_installed(svc);
        let backup_dir = self.svc_backup_dir(svc);
        let has_backup = self.save_backup(svc, &target, &installed_before, &backup_dir)?;
        if has_backup {
            on_log(&format!("[升级 {title}] 已备份旧版 {} ({}) 到 {}", installed_before.version, installed_before.short_sha, backup_dir.display()));
        }

        // 3/4 替换文件
        on_log(&format!("[升级 {title}] 3/4 写入新版本文件到 {}...", target.display()));
        if let Err(e) = replace_artifact(svc, &staged_artifact, &target) {
            on_log(&format!("[升级 {title}] 写入失败：{e}，尝试恢复备份..."));
            if has_backup {
                let _ = restore_from_backup(svc, &backup_dir, &target);
            }
            return Err(e);
        }
        self.invalidate_cache(&target);
        if let Err(e) = fs::remove_dir_all(&stage_dir) {
            on_log(&format!("[升级 {title}] 清理暂存目录警告：{e}"));
        }

        // 4/4 按需启动并校验健康
        if has_stack_ps1 && (auto_start || was_running) {
            on_log(&format!("[升级 {title}] 4/4 启动 {title} 并等待健康检查..."));
            if let Err(e) = self.ps_start_service(svc, &mut on_log) {
                on_log(&format!("[升级 {title}] 新版本启动/健康检查失败：{e}，正在自动回滚到旧版本..."));
                if has_backup {
                    let _ = self.ps_stop_service(svc, true, &mut on_log);
                    if let Err(re) = restore_from_backup(svc, &backup_dir, &target) {
                        anyhow::bail!("新版本启动失败（{e}），且回滚文件也失败：{re}");
                    }
                    self.invalidate_cache(&target);
                    if was_running {
                        let _ = self.ps_start_service(svc, &mut on_log);
                    }
                    anyhow::bail!("新版本启动失败（{e}），已自动回滚到上一版本");
                }
                return Err(e);
            }
        } else {
            on_log(&format!("[升级 {title}] 4/4 文件替换完成（服务当前保持停止状态）"));
        }

        on_log(&format!("[升级 {title}] ✓ 升级完成"));
        Ok(self.entry_for(svc))
    }

    /// 回滚到上一版备份。
    pub fn rollback<F>(&self, svc: &str, auto_start: bool, mut on_log: F) -> anyhow::Result<ServiceVersionEntry>
    where
        F: FnMut(&str),
    {
        if !is_valid_service(svc) {
            anyhow::bail!("不支持的服务：{svc}");
        }
        let _guard = self.op_lock.lock().map_err(|_| anyhow::anyhow!("回滚锁异常"))?;
        let backup_dir = self.svc_backup_dir(svc);
        if !backup_dir.join("backup.json").is_file() {
            anyhow::bail!("{} 没有可回滚的上一版本备份", service_title(svc));
        }
        let target = self.target_path_for(svc);
        let title = service_title(svc);
        let has_stack_ps1 = self.root.join("stack.ps1").is_file() && self.root.join("stack.toml").is_file();

        let was_running = if has_stack_ps1 {
            on_log(&format!("[回滚 {title}] 1/3 停止运行中的 {title}..."));
            self.ps_stop_service(svc, true, &mut on_log)?
        } else {
            false
        };

        on_log(&format!("[回滚 {title}] 2/3 从备份还原文件到 {}...", target.display()));
        restore_from_backup(svc, &backup_dir, &target)?;
        self.invalidate_cache(&target);

        if has_stack_ps1 && (auto_start || was_running) {
            on_log(&format!("[回滚 {title}] 3/3 重新启动 {title}..."));
            self.ps_start_service(svc, &mut on_log)?;
        }
        on_log(&format!("[回滚 {title}] ✓ 回滚完成"));
        Ok(self.entry_for(svc))
    }

    fn save_backup(&self, svc: &str, target: &Path, info: &ArtifactVersionInfo, backup_dir: &Path) -> anyhow::Result<bool> {
        if !target.exists() {
            return Ok(false);
        }
        let tmp_backup = self.backup_root().join(format!("{svc}.tmp-{}", std::process::id()));
        if tmp_backup.exists() {
            fs::remove_dir_all(&tmp_backup)?;
        }
        fs::create_dir_all(&tmp_backup)?;

        let rel = if svc == "frontend" {
            let dest = tmp_backup.join("www");
            copy_dir_all(target, &dest)?;
            "www".to_string()
        } else {
            let fname = target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| format!("{svc}.exe"));
            fs::copy(target, tmp_backup.join(&fname))?;
            fname
        };

        let meta = BackupMeta {
            backup_rel_path: rel,
            saved_at: timefmt::now_iso(),
            info: info.clone(),
        };
        fs::write(tmp_backup.join("backup.json"), serde_json::to_string_pretty(&meta)?)?;

        if backup_dir.exists() {
            fs::remove_dir_all(backup_dir)?;
        }
        if let Some(parent) = backup_dir.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&tmp_backup, backup_dir)?;
        Ok(true)
    }

    fn ps_stop_service(&self, svc: &str, force: bool, on_log: &mut impl FnMut(&str)) -> anyhow::Result<bool> {
        let script = ps_quote(&self.root.join("stack.ps1"));
        let cfg = ps_quote(&self.root.join("stack.toml"));
        let force_ps = if force { "$true" } else { "$false" };
        let body = format!(
            r#"$ErrorActionPreference = 'Stop'
. '{script}' __lib
try {{
    $cfg = Get-StackConfig '{cfg}'
    foreach ($dir in $cfg.Root, $cfg.RunDir, $cfg.LogsDir) {{ New-Dir $dir }}
    Enter-StackLock $cfg
    $state = Read-StackState $cfg
    $name = '{svc}'
    $forceBusy = {force_ps}
    [void](Invoke-Adopt $cfg $state $true @($name))
    if (-not $state.ContainsKey($name)) {{
        Write-Output "WAS_RUNNING=0"
        exit 0
    }}
    Write-Output "WAS_RUNNING=1"
    if ($name -eq 'backend' -and (Test-RecordedProcess $cfg 'backend' $state['backend']).Ours) {{
        $busy = Get-BackendBusy $cfg
        if (-not $busy.Known -and -not $forceBusy) {{ Stop-Stack "后端接口没应答，判断不了是否在写东西（勾选强制停止再试）" 6 }}
        if ($busy.Busy -and -not $forceBusy) {{ Stop-Stack ("后端正在忙：" + ($busy.Reasons -join '；')) 6 }}
    }}
    $record = $state[$name]
    if ($cfg.Guard.ProtectPorts.Count -gt 0) {{ [void](Get-ListenTable -Fresh) }}
    $check = Test-RecordedProcess $cfg $name $record
    if ($check.Unreadable) {{ Stop-Stack ("读不到进程路径：" + $check.Why) 4 }}
    if ($check.Ours) {{
        $kids = @()
        foreach ($kid in (Get-ChildProcs $record.pid $record.startTicks)) {{ $kids += @{{ pid = [int]$kid.ProcessId; ticks = (Get-ProcTicks $kid); name = $kid.Name }} }}
        $graceful = Request-GracefulStop $cfg $name $record $check.Proc
        if (-not $graceful.Exited) {{
            if (-not (Stop-VerifiedProcess $record.pid $record.startTicks)) {{ Stop-Stack "进程未能停止" 4 }}
        }}
        foreach ($kid in $kids) {{
            if ($kid.name -ieq 'conhost.exe') {{ continue }}
            Start-Sleep -Milliseconds 200
            $p = Get-ProcInfo $kid.pid
            if ($null -ne $p -and (Test-SameStart $kid.ticks $p)) {{ [void](Stop-VerifiedProcess $kid.pid $kid.ticks) }}
        }}
        if ($record.wrapperPid -gt 0) {{
            Start-Sleep -Milliseconds 200
            $w = Get-ProcInfo $record.wrapperPid
            if ($null -ne $w -and $w.Name -ieq 'cmd.exe') {{ [void](Stop-VerifiedProcess $record.wrapperPid $record.wrapperTicks) }}
        }}
    }}
    $state.Remove($name)
    Save-StackState $cfg $state
    exit 0
}} catch {{
    $code = 1
    $ex = $_.Exception
    if ($null -ne $ex -and $null -ne $ex.Data -and $ex.Data.Contains('StackExitCode')) {{ $code = [int]$ex.Data['StackExitCode'] }}
    Write-Output "✗ $($ex.Message)"
    exit $code
}}"#
        );
        let (code, lines) = run_powershell_encoded(&self.root, &body)?;
        let mut was_running = false;
        for l in &lines {
            if l.trim() == "WAS_RUNNING=1" {
                was_running = true;
            } else if l.trim() != "WAS_RUNNING=0" {
                on_log(l);
            }
        }
        if code != 0 {
            let reason = lines.last().cloned().unwrap_or_else(|| format!("退出码 {code}"));
            anyhow::bail!("停止服务失败：{reason}");
        }
        // 等系统回收文件锁（最多 5 秒）
        if svc != "frontend" {
            let target = self.target_path_for(svc);
            wait_file_writable(&target, Duration::from_secs(5));
        }
        Ok(was_running)
    }

    fn ps_start_service(&self, svc: &str, on_log: &mut impl FnMut(&str)) -> anyhow::Result<()> {
        let script = ps_quote(&self.root.join("stack.ps1"));
        let cfg = ps_quote(&self.root.join("stack.toml"));
        let body = format!(
            r#"$ErrorActionPreference = 'Stop'
. '{script}' __lib
try {{
    $cfg = Get-StackConfig '{cfg}'
    foreach ($dir in $cfg.Root, $cfg.RunDir, $cfg.LogsDir) {{ New-Dir $dir }}
    Enter-StackLock $cfg
    $state = Read-StackState $cfg
    $name = '{svc}'
    if ($name -eq 'database') {{
        Initialize-BackendFiles $cfg
        New-Dir $cfg.Database.DataDir
    }} elseif ($name -eq 'backend') {{
        Initialize-BackendFiles $cfg
        if ((Get-ServiceState $cfg $state 'database').State -ne 'ok') {{ Stop-Stack "数据库未正常运行，无法启动后端" 4 }}
    }} elseif ($name -eq 'rs_server') {{
        if ((Get-ServiceState $cfg $state 'backend').State -ne 'ok') {{ Stop-Stack "后端未正常运行，无法启动 rs-server" 4 }}
        Initialize-RsServerFiles $cfg
    }} elseif ($name -eq 'frontend') {{
        Initialize-FrontendFiles $cfg
    }}
    Start-StackService $cfg $state $name 1 1
    if ($name -eq 'frontend' -and $cfg.Frontend.Kind -eq 'nginx' -and (Get-ServiceState $cfg $state 'backend').State -ne 'ok') {{
        $deadline = (Get-Date).AddSeconds((Get-ServiceTimeout $cfg 'frontend'))
        $ready = $false
        while ((Get-Date) -lt $deadline) {{
            $r = Invoke-StackHttp 'GET' ((Get-FrontendUrl $cfg) + '/') '' 3
            if ($r.Ok -and $r.Body -match 'id="app"') {{ $ready = $true; break }}
            Start-Sleep -Milliseconds 500
        }}
        if (-not $ready) {{ Stop-Stack "前端启动超时" 4 }}
        Say "  ✓ 前端已启动（首页可访问）" 'Green'
    }} else {{
        Wait-ServiceReady $cfg $state $name 1 1
    }}
    if ($name -eq 'rs_server' -and (Get-ServiceState $cfg $state 'backend').State -eq 'ok') {{ Update-PlantUiWebConfig $cfg }}
    exit 0
}} catch {{
    $code = 1
    $ex = $_.Exception
    if ($null -ne $ex -and $null -ne $ex.Data -and $ex.Data.Contains('StackExitCode')) {{ $code = [int]$ex.Data['StackExitCode'] }}
    Write-Output "✗ $($ex.Message)"
    exit $code
}}"#
        );
        let (code, lines) = run_powershell_encoded(&self.root, &body)?;
        for l in &lines {
            on_log(l);
        }
        if code != 0 {
            let reason = lines.last().cloned().unwrap_or_else(|| format!("退出码 {code}"));
            anyhow::bail!("启动服务失败：{reason}");
        }
        Ok(())
    }
}

pub fn compute_diff(installed: &ArtifactVersionInfo, candidate: &ArtifactVersionInfo) -> VersionDiff {
    if !installed.exists {
        return VersionDiff {
            status: "not_installed".into(),
            same_sha256: false,
            version_changed: true,
            build_id_changed: true,
            bytes_delta: candidate.bytes as i64,
            summary: format!("当前未安装，可部署新版本 {}", non_empty_ver(candidate)),
        };
    }
    let same_sha256 = !installed.sha256.is_empty() && installed.sha256.eq_ignore_ascii_case(&candidate.sha256);
    let version_changed = installed.version != candidate.version;
    let build_id_changed = installed.build_id != candidate.build_id;
    let bytes_delta = candidate.bytes as i64 - installed.bytes as i64;
    if same_sha256 {
        return VersionDiff {
            status: "up_to_date".into(),
            same_sha256: true,
            version_changed: false,
            build_id_changed: false,
            bytes_delta: 0,
            summary: format!("版本与 SHA-256 完全一致（{} · {}），已是最新", non_empty_ver(installed), installed.short_sha),
        };
    }
    let cur_v = non_empty_ver(installed);
    let new_v = non_empty_ver(candidate);
    let summary = if cur_v != new_v {
        format!("检测到版本变化：{} ({}) → {} ({})", cur_v, installed.short_sha, new_v, candidate.short_sha)
    } else {
        format!("版本号同为 {}，但构建/SHA-256 不同（{} → {}），可推送更新", cur_v, installed.short_sha, candidate.short_sha)
    };
    VersionDiff {
        status: "update_available".into(),
        same_sha256: false,
        version_changed,
        build_id_changed,
        bytes_delta,
        summary,
    }
}

fn non_empty_ver(info: &ArtifactVersionInfo) -> &str {
    if !info.build_id.is_empty() {
        &info.build_id
    } else if !info.version.is_empty() {
        &info.version
    } else {
        "未知版本"
    }
}

fn probe_frontend_dir(root: &Path, dir: &Path) -> ArtifactVersionInfo {
    let mut info = ArtifactVersionInfo {
        exists: dir.is_dir(),
        is_dir: true,
        path: rel_or_abs(root, dir),
        ..Default::default()
    };
    if !info.exists {
        return info;
    }
    if let Ok(text) = fs::read_to_string(dir.join("version.json"))
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
    {
        info.version = v.get("version").and_then(|x| x.as_str()).unwrap_or("").to_string();
        info.commit = v.get("commit").and_then(|x| x.as_str()).unwrap_or("").to_string();
        info.built_at = v.get("buildDate").and_then(|x| x.as_str()).unwrap_or("").to_string();
        if !info.version.is_empty() && !info.commit.is_empty() {
            let short_c: String = info.commit.chars().take(8).collect();
            info.build_id = format!("{} ({})", info.version, short_c);
        }
    }

    let mut files = collect_files(dir);
    files.sort();
    let mut total_bytes = 0u64;
    let mut max_mtime = 0u64;
    let mut agg_hasher = Sha256::new();
    let mut buf = [0u8; 65536];

    for fpath in &files {
        let Ok(meta) = fs::metadata(fpath) else { continue };
        let len = meta.len();
        total_bytes += len;
        let mt = system_time_secs(meta.modified().ok());
        if mt > max_mtime {
            max_mtime = mt;
        }
        let rel = fpath.strip_prefix(dir).unwrap_or(fpath).display().to_string().replace('\\', "/");
        let mut fh = Sha256::new();
        if let Ok(mut f) = fs::File::open(fpath) {
            while let Ok(n) = f.read(&mut buf) {
                if n == 0 {
                    break;
                }
                fh.update(&buf[..n]);
            }
        }
        let hex = hex_lower(&fh.finalize());
        agg_hasher.update(format!("{rel}:{len}:{hex}\n").as_bytes());
    }

    let sha = hex_lower(&agg_hasher.finalize());
    info.bytes = total_bytes;
    info.file_count = files.len() as u64;
    info.sha256 = sha.clone();
    info.short_sha = short_sha(&sha);
    info.modified_at = format_unix_iso(max_mtime);
    info
}

fn probe_binary_file(svc: &str, root: &Path, path: &Path, len: u64, mtime_secs: u64) -> ArtifactVersionInfo {
    let mut info = ArtifactVersionInfo {
        exists: true,
        is_dir: false,
        path: rel_or_abs(root, path),
        bytes: len,
        file_count: 1,
        modified_at: format_unix_iso(mtime_secs),
        ..Default::default()
    };

    let mut hasher = Sha256::new();
    let mut tail_overlap: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];

    if let Ok(mut f) = fs::File::open(path) {
        loop {
            match f.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    hasher.update(&buf[..n]);
                    let mut win = Vec::with_capacity(tail_overlap.len() + n);
                    win.extend_from_slice(&tail_overlap);
                    win.extend_from_slice(&buf[..n]);
                    scan_binary_window(svc, &win, &mut info);
                    let keep = win.len().min(256);
                    tail_overlap.clear();
                    tail_overlap.extend_from_slice(&win[win.len() - keep..]);
                }
            }
        }
    }
    info.sha256 = hex_lower(&hasher.finalize());
    info.short_sha = short_sha(&info.sha256);

    // 若从二进制扫描还没拿到完整版本信息，安全地运行 --version / version（绝不运行 rs-server.exe）
    if (info.version.is_empty() || (svc == "database" && !info.version.contains('+')) || (svc == "backend" && info.built_at.is_empty()))
        && matches!(svc, "database" | "backend")
    {
        probe_cli_version(svc, path, &mut info);
    }
    if svc == "rs_server" && info.version.is_empty() && !info.commit.is_empty() {
        info.version = format!("rs-server {}", info.commit);
    }
    info
}

fn scan_binary_window(svc: &str, win: &[u8], info: &mut ArtifactVersionInfo) {
    match svc {
        "backend" => {
            // 示例：`...regen0.1.44+g2ed067be0655.1794446005.dirtye3d_model_build_id`（Rust 字符串常量之间无 NUL）
            if info.build_id.is_empty()
                && let Some(pos) = find_bytes(win, b"e3d_model_build_id")
            {
                let start = pos.saturating_sub(96);
                if let Some(bid) = extract_backend_build_id(&win[start..pos]) {
                    info.build_id = bid.clone();
                    if let Some((ver, rest)) = bid.split_once('+') {
                        info.version = ver.to_string();
                        if let Some(g) = rest.strip_prefix('g') {
                            info.commit = g.split('.').next().unwrap_or("").to_string();
                        }
                    }
                }
            }
            if info.built_at.is_empty()
                && let Some(pos) = find_bytes(win, b"aios-database ")
            {
                let end = (pos + 96).min(win.len());
                if let Ok(s) = std::str::from_utf8(&win[pos..end])
                    && let Some(line) = s.split('\0').next()
                    && let Some(idx) = line.find("(built ")
                {
                    let inner = &line[idx + 7..];
                    if let Some(close) = inner.find(')') {
                        info.built_at = inner[..close].trim().to_string();
                    }
                    if info.version.is_empty() {
                        let v = line["aios-database ".len()..idx].trim();
                        if v.starts_with("0.") {
                            info.version = v.to_string();
                        }
                    }
                }
            }
        }
        "database" => {
            // `functionsquery2.1.4surrealdb` 或 `+20250317.45013fc9`
            if info.version.is_empty()
                && let Some(pos) = find_bytes(win, b"surrealdb")
            {
                let start = pos.saturating_sub(32);
                let slice = &win[start..pos];
                for i in 0..slice.len() {
                    if slice[i..].starts_with(b"2.1.") {
                        let ver: String = slice[i..].iter().take_while(|b| b.is_ascii_digit() || **b == b'.').map(|&b| b as char).collect();
                        if ver.len() >= 5 {
                            info.version = ver;
                            break;
                        }
                    }
                }
            }
        }
        "rs_server" => {
            // x86_64 指令常把短 commit 拆进两条 64 位立即数：`48 B9 <8B> 48 89 48 05 48 B9 <8B> 48 89 08`
            if info.commit.is_empty() {
                let mut i = 0;
                while i + 27 <= win.len() {
                    if win[i] == 0x48
                        && win[i + 1] == 0xB9
                        && win[i + 10..i + 16] == [0x48, 0x89, 0x48, 0x05, 0x48, 0xB9]
                        && win[i + 24..i + 27] == [0x48, 0x89, 0x08]
                    {
                        let part2 = &win[i + 2..i + 10];
                        let part1 = &win[i + 16..i + 24];
                        if part1.iter().all(|b| b.is_ascii_graphic()) && part2.iter().all(|b| b.is_ascii_graphic()) {
                            let s = format!("{}{}", String::from_utf8_lossy(&part1[..5]), String::from_utf8_lossy(part2));
                            let hex_part = s.strip_suffix("-dirty").unwrap_or(&s);
                            if (7..=12).contains(&hex_part.len())
                                && hex_part.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                            {
                                info.commit = s.clone();
                                info.build_id = format!("rs-server {s}");
                                break;
                            }
                        }
                    }
                    i += 1;
                }
            }
        }
        _ => {}
    }
}

fn extract_backend_build_id(slice: &[u8]) -> Option<String> {
    let plus = find_bytes(slice, b"+g")?;
    let mut start = plus;
    while start > 0 && (slice[start - 1].is_ascii_digit() || slice[start - 1] == b'.') {
        start -= 1;
    }
    let ver = std::str::from_utf8(&slice[start..plus]).ok()?.trim_start_matches('.');
    if ver.split('.').count() < 2 || ver.is_empty() {
        return None;
    }
    let mut end = plus + 2;
    while end < slice.len() && (slice[end].is_ascii_alphanumeric() || slice[end] == b'.') {
        end += 1;
    }
    let rest = std::str::from_utf8(&slice[plus..end]).ok()?;
    Some(format!("{ver}{rest}"))
}

fn probe_cli_version(svc: &str, exe: &Path, info: &mut ArtifactVersionInfo) {
    let arg = match svc {
        "database" => "version",
        "backend" => "--version",
        _ => return,
    };
    let mut cmd = Command::new(exe);
    cmd.arg(arg).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let Ok(mut child) = cmd.spawn() else { return };
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        if let Ok(Some(_)) = child.try_wait() {
            if let Ok(out) = child.wait_with_output() {
                let s = String::from_utf8_lossy(&out.stdout);
                let line = s.lines().next().unwrap_or("").trim();
                if svc == "database" && !line.is_empty() {
                    let v = line.split(" for ").next().unwrap_or(line).trim();
                    info.version = v.to_string();
                } else if svc == "backend" && let Some(rest) = line.strip_prefix("aios-database ") {
                    if let Some((v, built)) = rest.split_once("(built ") {
                        info.version = v.trim().to_string();
                        info.built_at = built.trim_end_matches(')').trim().to_string();
                    } else {
                        info.version = rest.trim().to_string();
                    }
                }
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(8).collect()
}

fn rel_or_abs(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .map(|r| r.display().to_string().replace('\\', "/"))
        .unwrap_or_else(|_| p.display().to_string())
}

fn system_time_secs(t: Option<SystemTime>) -> u64 {
    t.and_then(|x| x.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

fn format_unix_iso(secs: u64) -> String {
    if secs == 0 {
        return String::new();
    }
    let st = UNIX_EPOCH + Duration::from_secs(secs);
    timefmt::iso(st)
}

fn sanitize_rel_path(raw: &str) -> anyhow::Result<PathBuf> {
    let normalized = raw.trim().replace('\\', "/");
    if normalized.is_empty() {
        anyhow::bail!("文件名不能为空");
    }
    let p = Path::new(&normalized);
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(seg) => {
                let s = seg.to_string_lossy();
                if s.contains(':') || s.contains('\0') {
                    anyhow::bail!("文件名含非法字符");
                }
                out.push(seg);
            }
            Component::CurDir => {}
            _ => anyhow::bail!("不允许相对父目录或绝对路径：{raw}"),
        }
    }
    if out.as_os_str().is_empty() {
        anyhow::bail!("无效文件名");
    }
    Ok(out)
}

fn collect_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(cur) = stack.pop() {
        let Ok(rd) = fs::read_dir(&cur) else { continue };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.is_file() {
                out.push(p);
            }
        }
    }
    out
}

fn copy_dir_all(src: &Path, dst: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let sp = entry.path();
        let dp = dst.join(entry.file_name());
        if sp.is_dir() {
            copy_dir_all(&sp, &dp)?;
        } else {
            fs::copy(&sp, &dp)?;
        }
    }
    Ok(())
}

fn replace_artifact(svc: &str, staged: &Path, target: &Path) -> anyhow::Result<()> {
    if svc == "frontend" {
        let parent = target.parent().ok_or_else(|| anyhow::anyhow!("前端目录路径无效"))?;
        fs::create_dir_all(parent)?;
        let temp_new = parent.join(format!(".www-new-{}", std::process::id()));
        if temp_new.exists() {
            fs::remove_dir_all(&temp_new)?;
        }
        copy_dir_all(staged, &temp_new)?;
        let temp_old = parent.join(format!(".www-old-{}", std::process::id()));
        if temp_old.exists() {
            let _ = fs::remove_dir_all(&temp_old);
        }
        if target.exists() {
            fs::rename(target, &temp_old)?;
        }
        if let Err(e) = fs::rename(&temp_new, target) {
            if temp_old.exists() && !target.exists() {
                let _ = fs::rename(&temp_old, target);
            }
            anyhow::bail!("替换前端目录失败：{e}");
        }
        if temp_old.exists() {
            let _ = fs::remove_dir_all(&temp_old);
        }
        return Ok(());
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_target = target.with_extension("exe.new");
    if tmp_target.exists() {
        let _ = fs::remove_file(&tmp_target);
    }
    fs::copy(staged, &tmp_target)?;
    // 重试 rename（Windows 上进程刚退出可能短暂占用句柄）
    let mut last_err = None;
    for _ in 0..15 {
        match fs::rename(&tmp_target, target) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
    let _ = fs::remove_file(&tmp_target);
    Err(anyhow::anyhow!("覆盖目标程序 {} 失败：{}", target.display(), last_err.map(|e| e.to_string()).unwrap_or_default()))
}

fn restore_from_backup(svc: &str, backup_dir: &Path, target: &Path) -> anyhow::Result<()> {
    let meta_text = fs::read_to_string(backup_dir.join("backup.json"))?;
    let meta: BackupMeta = serde_json::from_str(&meta_text)?;
    let src = backup_dir.join(&meta.backup_rel_path);
    if !src.exists() {
        anyhow::bail!("备份文件缺失：{}", src.display());
    }
    replace_artifact(svc, &src, target)
}

fn locate_staged_artifact(svc: &str, search_root: &Path) -> anyhow::Result<PathBuf> {
    if svc == "frontend" {
        if search_root.join("index.html").is_file() {
            return Ok(search_root.to_path_buf());
        }
        if search_root.join("www").join("index.html").is_file() {
            return Ok(search_root.join("www"));
        }
        if search_root.join("dist").join("index.html").is_file() {
            return Ok(search_root.join("dist"));
        }
        for f in collect_files(search_root) {
            if f.file_name().is_some_and(|n| n.eq_ignore_ascii_case("index.html"))
                && let Some(p) = f.parent()
            {
                return Ok(p.to_path_buf());
            }
        }
        anyhow::bail!("上传的前端包中没有找到 index.html");
    }

    let expected = match svc {
        "database" => "surreal.exe",
        "backend" => "aios-database.exe",
        "rs_server" => "rs-server.exe",
        _ => "",
    };
    let exes: Vec<PathBuf> = collect_files(search_root)
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")))
        .collect();
    if let Some(exact) = exes.iter().find(|p| p.file_name().is_some_and(|n| n.eq_ignore_ascii_case(expected))) {
        return Ok(exact.clone());
    }
    if exes.len() == 1 {
        return Ok(exes[0].clone());
    }
    anyhow::bail!("上传内容中没有找到 {}（共发现 {} 个 .exe）", expected, exes.len())
}

fn resolve_candidate_in_path(svc: &str, src: &Path) -> Option<PathBuf> {
    if src.is_file() {
        return Some(src.to_path_buf());
    }
    if !src.is_dir() {
        return None;
    }
    if svc == "frontend" {
        if src.join("index.html").is_file() {
            return Some(src.to_path_buf());
        }
        if src.join("www").join("index.html").is_file() {
            return Some(src.join("www"));
        }
        if src.join("dist").join("index.html").is_file() {
            return Some(src.join("dist"));
        }
        return None;
    }
    let expected = match svc {
        "database" => "surreal.exe",
        "backend" => "aios-database.exe",
        "rs_server" => "rs-server.exe",
        _ => return None,
    };
    for rel in [expected, &format!("bin/{expected}")] {
        let p = src.join(rel);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn extract_zip(zip_path: &Path, dst: &Path) -> anyhow::Result<()> {
    let z = ps_quote(zip_path);
    let d = ps_quote(dst);
    let script = format!(
        "Add-Type -AssemblyName System.IO.Compression.FileSystem\n[System.IO.Compression.ZipFile]::ExtractToDirectory('{z}', '{d}')\n"
    );
    let (code, lines) = run_powershell_encoded(dst, &script)?;
    if code != 0 {
        anyhow::bail!("解压 zip 失败（退出码 {code}）：{}", lines.join(" | "));
    }
    Ok(())
}

fn wait_file_writable(path: &Path, timeout: Duration) {
    if !path.is_file() {
        return;
    }
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if fs::OpenOptions::new().write(true).open(path).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn ps_quote(p: &Path) -> String {
    p.display().to_string().replace('\'', "''")
}

fn encode_base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

fn run_powershell_encoded(cwd: &Path, script: &str) -> anyhow::Result<(i32, Vec<String>)> {
    let utf16: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let encoded = encode_base64(&utf16);
    let ps = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Windows"))
        .join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let mut c = Command::new(&ps);
    c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encoded])
        .current_dir(cwd)
        .env_remove("PSModulePath")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    let out = c.output()?;
    let code = out.status.code().unwrap_or(1);
    let mut lines = Vec::new();
    for raw in [out.stdout, out.stderr] {
        let s = crate::token::decode_console(&raw);
        for l in s.lines() {
            let t = l.trim();
            if !t.is_empty() {
                lines.push(t.to_string());
            }
        }
    }
    Ok((code, lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_dir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("svc-upgrade-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn upload_compare_upgrade_and_rollback_single_binary_and_frontend() {
        let root = fresh_dir("cycle");
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(root.join("www")).unwrap();

        // 构造初始版本（旧版，模拟无 NUL 分隔的紧邻常量串）
        let old_be = b"MZ-dummy-aios-database 0.1.44 (built 2026-11-12 09:13:25 +08:00)model_scope_regen0.1.44+g1111111.100e3d_model_build_id";
        fs::write(root.join("bin/aios-database.exe"), old_be).unwrap();
        fs::write(root.join("www/index.html"), "<div id=\"app\">v1</div>").unwrap();
        fs::write(root.join("www/version.json"), r#"{"version":"0.2.10","commit":"aaaa1111"}"#).unwrap();

        let mgr = SvcUpgradeManager::new(root.clone());
        let list = mgr.list_all();
        assert_eq!(list.len(), 4);
        let be = list.iter().find(|s| s.service == "backend").unwrap();
        assert!(be.installed.exists);
        assert_eq!(be.installed.version, "0.1.44");
        assert_eq!(be.installed.build_id, "0.1.44+g1111111.100");

        // 上传同一个 backend 二进制，应该对比出 up_to_date
        let entry_same = mgr.upload_chunk("backend", "aios-database.exe", 0, Some(old_be.len() as u64), true, true, old_be).unwrap().unwrap();
        assert_eq!(entry_same.diff.as_ref().unwrap().status, "up_to_date");

        // 分两块上传新版本 backend
        let new_be = b"MZ-dummy-aios-database 0.1.45 (built 2026-11-13 12:00:00 +08:00)\00.1.45+g2222222.200\0e3d_model_build_id";
        let mid = new_be.len() / 2;
        assert!(mgr.upload_chunk("backend", "aios-database.exe", 0, Some(new_be.len() as u64), true, false, &new_be[..mid]).unwrap().is_none());
        let entry_new = mgr.upload_chunk("backend", "aios-database.exe", mid as u64, Some(new_be.len() as u64), false, true, &new_be[mid..]).unwrap().unwrap();
        assert_eq!(entry_new.diff.as_ref().unwrap().status, "update_available");
        assert_eq!(entry_new.staged.as_ref().unwrap().version, "0.1.45");

        // 执行升级（无 stack.ps1 时只做备份+原子替换）
        let mut logs = Vec::new();
        let upgraded = mgr.apply_upgrade("backend", false, false, |l| logs.push(l.to_string())).unwrap();
        assert_eq!(upgraded.installed.version, "0.1.45");
        assert!(upgraded.backup.exists);
        assert_eq!(upgraded.backup.artifact.as_ref().unwrap().version, "0.1.44");

        // 回滚回 0.1.44
        let rolled = mgr.rollback("backend", false, |l| logs.push(l.to_string())).unwrap();
        assert_eq!(rolled.installed.version, "0.1.44");

        // 上传前端多文件并升级
        mgr.upload_chunk("frontend", "index.html", 0, None, true, false, b"<div id=\"app\">v2</div>").unwrap();
        let fe_staged = mgr.upload_chunk("frontend", "version.json", 0, None, false, true, br#"{"version":"0.2.11","commit":"bbbb2222"}"#).unwrap().unwrap();
        assert_eq!(fe_staged.diff.as_ref().unwrap().status, "update_available");
        let fe_up = mgr.apply_upgrade("frontend", false, false, |l| logs.push(l.to_string())).unwrap();
        assert_eq!(fe_up.installed.version, "0.2.11");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scans_rs_server_inline_commit_pattern() {
        let mut fake = vec![0u8; 32];
        // False-positive earlier constant "COMP_LOCATION" in the same instruction shape must be ignored.
        fake.extend_from_slice(&[
            0x48, 0xB9, b'L', b'O', b'C', b'A', b'T', b'I', b'O', b'N',
            0x48, 0x89, 0x48, 0x05,
            0x48, 0xB9, b'C', b'O', b'M', b'P', b'_', b'L', b'O', b'C',
            0x48, 0x89, 0x08,
        ]);
        fake.extend_from_slice(&[0u8; 32]);
        // 48 B9 "f7-dirty" 48 89 48 05 48 B9 "c0b2af7-" 48 89 08
        let pattern: &[u8] = &[
            0x48, 0xB9, b'f', b'7', b'-', b'd', b'i', b'r', b't', b'y',
            0x48, 0x89, 0x48, 0x05,
            0x48, 0xB9, b'c', b'0', b'b', b'2', b'a', b'f', b'7', b'-',
            0x48, 0x89, 0x08,
        ];
        fake.extend_from_slice(pattern);
        let mut info = ArtifactVersionInfo::default();
        scan_binary_window("rs_server", &fake, &mut info);
        assert_eq!(info.commit, "c0b2af7-dirty");
        assert_eq!(info.build_id, "rs-server c0b2af7-dirty");
    }
}
