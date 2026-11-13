//! `GET /v1/config`：配置摘要（设计 4.5），只给白名单字段。有 `stack.toml` 就读它（`[backend.env]`、`[rs_server.env]`、
//! `config_set` 这些一概不读不发）；没有（mock、还没配置）就从最新快照里凑。
//! 以及 `PUT /v1/config/project` 的校验（REVIEW C9：`dir` 只能是一段名字、`code` 是整数）。

use std::path::Path;

use deploy_dash_proto as proto;
use proto::{BackendConfigInfo, ConfigSummary, FsConfig, GuardConfig, InitConfig, ListenConfig, ProjectForm, ProjectInfo, ServiceConfig, SvcName};

use crate::backend::StackState;

pub fn summary(stack_toml: &Path, root: &Path, snap: Option<&StackState>, browse_roots: &[String], agent_version: &str) -> ConfigSummary {
    let mut out = match std::fs::read_to_string(stack_toml).ok().and_then(|t| t.trim_start_matches('\u{feff}').parse::<toml::Table>().ok()) {
        Some(t) => from_toml(&t, root),
        None => from_snapshot(snap),
    };
    out.schema = proto::SCHEMA;
    out.agent_version = agent_version.to_string();
    if let Some(s) = snap {
        out.backend_config.drift = s.drift.clone();
        if !out.backend_config.generated {
            out.backend_config.generated = s.backend_config_generated;
        }
    }
    out.fs = FsConfig { browse_roots: browse_roots.to_vec() };
    out
}

fn text(v: Option<&toml::Value>) -> String {
    match v {
        Some(toml::Value::String(s)) => s.clone(),
        Some(toml::Value::Integer(i)) => i.to_string(),
        Some(toml::Value::Float(f)) => f.to_string(),
        Some(toml::Value::Boolean(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn num(v: Option<&toml::Value>) -> f64 {
    match v {
        Some(toml::Value::Integer(i)) => *i as f64,
        Some(toml::Value::Float(f)) => *f,
        _ => 0.0,
    }
}

fn from_toml(t: &toml::Table, root: &Path) -> ConfigSummary {
    let section = |name: &str| t.get(name).and_then(|v| v.as_table());
    let project = section("project").map(|p| ProjectInfo {
        e3d_root: text(p.get("e3d_root")),
        name: text(p.get("name")),
        dir: text(p.get("dir")),
        code: text(p.get("code")),
        mdb: Some(text(p.get("mdb"))).filter(|m| !m.is_empty()).unwrap_or_else(|| "ALL".into()),
        e3d_version: text(p.get("e3d_version")),
    });
    // 缺省端口和开关同 dash2 的 parse_stack_toml：数据库、后端缺省开；rs-server、前端要有那一节且没写 enabled = false
    let defaults: [u16; 4] = [8009, 8022, 9098, 3100];
    let services = SvcName::ALL
        .iter()
        .zip(defaults)
        .map(|(svc, port)| {
            let s = section(svc.as_str());
            let explicit_off = s.and_then(|s| s.get("enabled")).and_then(|v| v.as_bool()) == Some(false);
            let enabled = match svc {
                SvcName::Database | SvcName::Backend => !explicit_off,
                _ => s.is_some() && !explicit_off,
            };
            let port = s.and_then(|s| s.get("port")).and_then(|v| v.as_integer()).and_then(|p| u16::try_from(p).ok()).unwrap_or(port);
            ServiceConfig { name: *svc, enabled, port }
        })
        .collect();
    let guard = section("guard");
    let g = |k: &str| num(guard.and_then(|g| g.get(k)));
    let ints = |v: Option<&toml::Value>| -> Vec<i64> { v.and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_integer()).collect()).unwrap_or_default() };
    let init = section("init");
    let min_disk = g("min_disk_free_gb");
    ConfigSummary {
        schema: proto::SCHEMA,
        agent_version: String::new(),
        project,
        project_defaults: None,
        services,
        listen: ListenConfig { loopback_only: section("listen").and_then(|l| l.get("loopback_only")).and_then(|v| v.as_bool()).unwrap_or(false) },
        guard: GuardConfig {
            min_commit_free_gb: g("min_commit_free_gb"),
            abort_commit_free_gb: g("abort_commit_free_gb"),
            max_backend_gb: g("max_backend_gb"),
            min_disk_free_gb: if min_disk > 0.0 { min_disk } else { 2.0 },
            protect_ports: ints(guard.and_then(|g| g.get("protect_ports"))).into_iter().filter_map(|p| u16::try_from(p).ok()).collect(),
        },
        init: InitConfig {
            dbnums: ints(init.and_then(|i| i.get("dbnums"))).into_iter().filter_map(|d| u32::try_from(d).ok()).collect(),
            auto_new: init.and_then(|i| i.get("auto_new")).and_then(|v| v.as_bool()).unwrap_or(false),
        },
        backend_config: BackendConfigInfo { generated: root.join("work").join("backend").join("DbOption.toml").is_file(), drift: Vec::new() },
        fs: FsConfig::default(),
    }
}

fn from_snapshot(snap: Option<&StackState>) -> ConfigSummary {
    let Some(s) = snap else { return ConfigSummary::default() };
    let mut services: Vec<ServiceConfig> = s.services.iter().map(|x| ServiceConfig { name: x.svc, enabled: x.enabled, port: x.port }).collect();
    services.sort_by_key(|x| x.name.index().unwrap_or(usize::MAX));
    ConfigSummary {
        project: s.project.clone(),
        services,
        guard: GuardConfig {
            min_commit_free_gb: s.guard.min_commit_free_gb,
            abort_commit_free_gb: s.guard.abort_commit_free_gb,
            max_backend_gb: s.guard.max_backend_gb,
            min_disk_free_gb: s.guard.min_disk_free_gb,
            protect_ports: Vec::new(),
        },
        backend_config: BackendConfigInfo { generated: s.backend_config_generated, drift: Vec::new() },
        ..Default::default()
    }
}

/// `[project]` 的值合不合规矩：会被写进 stack.toml 的双引号字符串里，不许有引号、反斜杠以外的控制字符；
/// `dir` 只能是一段名字；`code` 是整数（可空）。
pub fn check_project(p: &ProjectForm) -> Result<(), String> {
    let fields = [("e3d_root", &p.e3d_root), ("name", &p.name), ("dir", &p.dir), ("code", &p.code), ("mdb", &p.mdb), ("e3d_version", &p.e3d_version)];
    for (k, v) in fields {
        if v.len() > 260 {
            return Err(format!("{k} 太长"));
        }
        if v.chars().any(|c| c.is_control() || c == '"') {
            return Err(format!("{k} 里不能有引号和控制字符"));
        }
    }
    if p.dir.contains(['\\', '/', ':']) || p.dir == ".." || p.dir == "." {
        return Err("dir 只能是一段文件夹名字（不带斜杠）".into());
    }
    if !p.code.trim().is_empty() && !p.code.trim().chars().all(|c| c.is_ascii_digit()) {
        return Err("code 要是整数".into());
    }
    for (k, v) in [("name", &p.name), ("mdb", &p.mdb), ("e3d_version", &p.e3d_version)] {
        if v.contains(['\\', '/']) {
            return Err(format!("{k} 里不能有斜杠"));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ServicesPortUpdate {
    pub database: u16,
    pub backend: u16,
    pub rs_server: u16,
    pub frontend: u16,
    #[serde(default)]
    pub loopback_only: bool,
    #[serde(default)]
    pub stack_name: Option<String>,
    #[serde(default)]
    pub reset_work: bool,
}

fn set_toml_top_level_kv(src: &str, key: &str, value_literal: &str) -> String {
    let eol = if src.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = src.lines().map(|l| l.to_string()).collect();
    let mut first_sec = lines.len();
    let mut key_idx: Option<usize> = None;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && !trimmed.starts_with("[[") {
            first_sec = idx;
            break;
        }
        if !trimmed.starts_with('#') && !trimmed.is_empty() {
            if let Some((k, _)) = trimmed.split_once('=') {
                if k.trim() == key {
                    key_idx = Some(idx);
                    break;
                }
            }
        }
    }
    let new_line = format!("{key} = {value_literal}");
    if let Some(i) = key_idx {
        lines[i] = new_line;
    } else {
        lines.insert(first_sec, new_line);
    }
    let mut out = lines.join(eol);
    if src.ends_with('\n') && !out.ends_with('\n') {
        out.push_str(eol);
    }
    out
}

fn set_toml_section_kv(src: &str, section: &str, key: &str, value_literal: &str) -> String {
    let eol = if src.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = src.lines().map(|l| l.to_string()).collect();
    let mut in_sec = false;
    let mut sec_start: Option<usize> = None;
    let mut sec_end: usize = lines.len();
    let mut key_idx: Option<usize> = None;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && !trimmed.starts_with("[[") {
            if let Some(end) = trimmed.find(']') {
                let name = trimmed[1..end].trim();
                if in_sec {
                    sec_end = idx;
                    break;
                }
                if name == section {
                    in_sec = true;
                    sec_start = Some(idx);
                }
            }
        } else if in_sec && !trimmed.starts_with('#') && !trimmed.is_empty() {
            if let Some((k, _)) = trimmed.split_once('=') {
                if k.trim() == key {
                    key_idx = Some(idx);
                    break;
                }
            }
        }
    }
    let new_line = format!("{key} = {value_literal}");
    if let Some(i) = key_idx {
        lines[i] = new_line;
    } else if sec_start.is_some() {
        lines.insert(sec_end, new_line);
    } else {
        if !lines.is_empty() && !lines.last().unwrap().trim().is_empty() {
            lines.push(String::new());
        }
        lines.push(format!("[{section}]"));
        lines.push(new_line);
    }
    let mut out = lines.join(eol);
    if src.ends_with('\n') && !out.ends_with('\n') {
        out.push_str(eol);
    }
    out
}

pub fn update_services_toml(stack_toml: &Path, u: &ServicesPortUpdate) -> Result<(), String> {
    let ports = [("database", u.database), ("backend", u.backend), ("rs_server", u.rs_server), ("frontend", u.frontend)];
    for (name, p) in ports {
        if p == 0 {
            return Err(format!("{name} 端口不能为 0"));
        }
    }
    for i in 0..ports.len() {
        for j in (i + 1)..ports.len() {
            if ports[i].1 == ports[j].1 {
                return Err(format!("{} 和 {} 的端口不能重复（{}）", ports[i].0, ports[j].0, ports[i].1));
            }
        }
    }
    let src = if stack_toml.is_file() {
        std::fs::read_to_string(stack_toml).map_err(|e| format!("读取 stack.toml 失败: {e}"))?
    } else {
        let def = stack_toml.with_file_name("stack.default.toml");
        if def.is_file() {
            std::fs::read_to_string(&def).map_err(|e| format!("读取 stack.default.toml 失败: {e}"))?
        } else {
            return Err("找不到 stack.toml 或 stack.default.toml".into());
        }
    };
    let protect = crate::config::stack_protect_ports(stack_toml);
    for (name, p) in ports {
        if protect.contains(&p) {
            return Err(format!("{name} 端口 {p} 在保护名单 protect_ports 里，不能占用"));
        }
    }
    let mut next = src;
    for (sec, p) in ports {
        next = set_toml_section_kv(&next, sec, "port", &p.to_string());
    }
    next = set_toml_section_kv(&next, "listen", "loopback_only", if u.loopback_only { "true" } else { "false" });
    if let Some(name) = u.stack_name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if !name.chars().any(|c| c.is_control() || c == '"') {
            next = set_toml_section_kv(&next, "stack", "name", &format!("\"{name}\""));
        }
    }
    std::fs::write(stack_toml, next.as_bytes()).map_err(|e| format!("写入 stack.toml 失败: {e}"))?;
    if let Some(root) = stack_toml.parent() {
        if u.reset_work {
            for rel in ["work/backend/DbOption.toml", "work/rs-server/DbOption.toml"] {
                let p = root.join(rel);
                if p.is_file() {
                    let _ = std::fs::remove_file(&p);
                }
            }
        } else {
            sync_work_dboptions(root, stack_toml)?;
        }
    }
    Ok(())
}

pub fn sync_work_dboptions(root: &Path, stack_toml: &Path) -> Result<(), String> {
    let Ok(raw) = std::fs::read_to_string(stack_toml) else { return Ok(()) };
    let Ok(t) = raw.trim_start_matches('\u{feff}').parse::<toml::Table>() else { return Ok(()) };
    let sec = |name: &str| t.get(name).and_then(|v| v.as_table());
    let loopback = sec("listen").and_then(|l| l.get("loopback_only")).and_then(|v| v.as_bool()).unwrap_or(false);
    let bind_host = if loopback { "127.0.0.1" } else { "0.0.0.0" };
    let db_port = sec("database").and_then(|s| s.get("port")).and_then(|v| v.as_integer()).unwrap_or(8009);
    let be_port = sec("backend").and_then(|s| s.get("port")).and_then(|v| v.as_integer()).unwrap_or(8022);
    let rs_port = sec("rs_server").and_then(|s| s.get("port")).and_then(|v| v.as_integer()).unwrap_or(9098);

    let proj = sec("project");
    let e3d_root = proj.map(|p| text(p.get("e3d_root")).replace('\\', "/")).unwrap_or_default();
    let proj_name = proj.map(|p| text(p.get("name"))).unwrap_or_default();
    let proj_code = proj.map(|p| text(p.get("code"))).unwrap_or_default();
    let proj_mdb = proj.map(|p| text(p.get("mdb"))).filter(|s| !s.is_empty()).unwrap_or_else(|| "ALL".into());
    let e3d_ver = proj.map(|p| text(p.get("e3d_version"))).unwrap_or_default();

    let be_path = root.join("work/backend/DbOption.toml");
    if be_path.is_file() {
        let mut s = std::fs::read_to_string(&be_path).map_err(|e| format!("读取 backend DbOption.toml 失败: {e}"))?;
        if !e3d_root.is_empty() { s = set_toml_top_level_kv(&s, "project_path", &format!("\"{e3d_root}\"")); }
        if !proj_name.is_empty() { s = set_toml_top_level_kv(&s, "project_name", &format!("\"{proj_name}\"")); }
        if !proj_code.is_empty() {
            s = set_toml_top_level_kv(&s, "project_code", &proj_code);
            s = set_toml_top_level_kv(&s, "surreal_ns", &format!("\"{proj_code}\""));
        }
        s = set_toml_top_level_kv(&s, "mdb_name", &format!("\"{proj_mdb}\""));
        if !e3d_ver.is_empty() { s = set_toml_top_level_kv(&s, "e3d_version", &format!("\"{e3d_ver}\"")); }
        s = set_toml_top_level_kv(&s, "v_port", &db_port.to_string());
        s = set_toml_top_level_kv(&s, "http_api_addr", &format!("\"{bind_host}:{be_port}\""));
        s = set_toml_top_level_kv(&s, "server_release_ip", &format!("\"{bind_host}:{be_port}\""));
        std::fs::write(&be_path, s.as_bytes()).map_err(|e| format!("写入 backend DbOption.toml 失败: {e}"))?;
    }

    let rs_path = root.join("work/rs-server/DbOption.toml");
    if rs_path.is_file() {
        let mut s = std::fs::read_to_string(&rs_path).map_err(|e| format!("读取 rs-server DbOption.toml 失败: {e}"))?;
        if !e3d_root.is_empty() { s = set_toml_top_level_kv(&s, "project_path", &format!("\"{e3d_root}\"")); }
        if !proj_name.is_empty() { s = set_toml_top_level_kv(&s, "project_name", &format!("\"{proj_name}\"")); }
        if !proj_code.is_empty() {
            s = set_toml_top_level_kv(&s, "project_code", &proj_code);
            s = set_toml_top_level_kv(&s, "surreal_ns", &format!("\"{proj_code}\""));
        }
        s = set_toml_top_level_kv(&s, "mdb_name", &format!("\"{proj_mdb}\""));
        if !e3d_ver.is_empty() { s = set_toml_top_level_kv(&s, "e3d_version", &format!("\"{e3d_ver}\"")); }
        s = set_toml_top_level_kv(&s, "v_port", &db_port.to_string());
        s = set_toml_top_level_kv(&s, "http_api_addr", &format!("\"{bind_host}:{be_port}\""));
        s = set_toml_top_level_kv(&s, "server_release_ip", &format!("\"{bind_host}:{rs_port}\""));
        s = set_toml_top_level_kv(&s, "aios_database_url", &format!("\"http://127.0.0.1:{be_port}\""));
        std::fs::write(&rs_path, s.as_bytes()).map_err(|e| format!("写入 rs-server DbOption.toml 失败: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_whitelisted_fields_only() {
        let dir = std::env::temp_dir().join(format!("deploy-agent-stackcfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("stack.toml");
        std::fs::write(
            &p,
            "[project]\ne3d_root = \"D:/AVEVA/Projects/E3D3.1\"\nname = \"AvevaPlantSample\"\ncode = 9001\n\n[database]\nport = 18709\n[backend]\nport = 18722\n[backend.env]\nSECRET_THING = \"不该外发\"\n[rs_server]\nport = 18798\n[frontend]\nport = 18700\nenabled = false\n[listen]\nloopback_only = true\n[guard]\nmin_commit_free_gb = 6\nprotect_ports = [8023, 8029]\n[init]\ndbnums = [7997, 8000]\n",
        )
        .unwrap();
        let s = summary(&p, &dir, None, &["D:/AVEVA/Projects".into()], "0.1.0");
        let proj = s.project.clone().unwrap();
        assert_eq!((proj.name.as_str(), proj.code.as_str(), proj.mdb.as_str()), ("AvevaPlantSample", "9001", "ALL"));
        assert_eq!(s.services.iter().map(|x| (x.port, x.enabled)).collect::<Vec<_>>(), vec![(18709, true), (18722, true), (18798, true), (18700, false)]);
        assert!(s.listen.loopback_only);
        assert_eq!(s.guard.protect_ports, vec![8023, 8029]);
        assert_eq!(s.guard.min_disk_free_gb, 2.0);
        assert_eq!(s.init.dbnums, vec![7997, 8000]);
        assert_eq!(s.fs.browse_roots, vec!["D:/AVEVA/Projects".to_string()]);
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("SECRET_THING") && !json.contains("不该外发"), "env 一节不外发");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_checks() {
        let ok = ProjectForm { e3d_root: "D:\\AVEVA\\Projects".into(), name: "AvevaMarineSample".into(), code: "1516".into(), ..Default::default() };
        assert!(check_project(&ok).is_ok());
        assert!(check_project(&ProjectForm { dir: "a\\b".into(), ..ok.clone() }).is_err());
        assert!(check_project(&ProjectForm { dir: "..".into(), ..ok.clone() }).is_err());
        assert!(check_project(&ProjectForm { code: "15a".into(), ..ok.clone() }).is_err());
        assert!(check_project(&ProjectForm { name: "x\"y".into(), ..ok.clone() }).is_err());
        assert!(check_project(&ProjectForm { e3d_root: "D:\\a\nb".into(), ..ok }).is_err());
    }
}
