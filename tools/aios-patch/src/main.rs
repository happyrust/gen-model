//! Self-contained binary patches for `aios-database.exe`.
//!
//! `create` appends a JSON manifest, a bsdiff payload, and a fixed footer to
//! this executable. Windows' PE loader ignores that overlay, so the resulting
//! file remains directly executable. Running it without a subcommand patches
//! the adjacent `aios-database.exe`.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write, stdin, stdout};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use clap::{Args, Parser, Subcommand};
use memmap2::Mmap;
use qbsdiff::{Bsdiff, Bspatch};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const BUNDLE_MAGIC: [u8; 16] = *b"AIOSPATCHBUNDLE1";
const FOOTER_BYTES: u64 = 32;
const FORMAT: &str = "aios-binary-patch-v1";
const PRODUCT: &str = "aios-database";

#[derive(Debug, Parser)]
#[command(
    name = "aios-database-patch",
    version,
    about = "Create or apply a self-contained binary patch for aios-database.exe"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Infer both executable identities and generate a verified patch.
    Make(MakeArgs),
    /// Compare two executables and emit one directly executable patch file.
    Create(CreateArgs),
    /// Apply an embedded patch. This is also the no-subcommand default.
    Apply(ApplyArgs),
    /// Print the manifest embedded in a generated patch executable.
    Inspect(InspectArgs),
}

#[derive(Debug, Args)]
struct MakeArgs {
    /// Installed old aios-database.exe (recommended), or an old release ZIP.
    #[arg(value_name = "OLD_INPUT")]
    source: PathBuf,
    /// New release ZIP (recommended), or a newly built aios-database.exe.
    #[arg(value_name = "NEW_INPUT")]
    target: PathBuf,
    /// Output directory. Defaults beside the old EXE/ZIP.
    #[arg(short, long)]
    output_dir: Option<PathBuf>,
    /// qbsdiff's bzip2 level. Level 1 is normally both faster and no larger.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(0..=9))]
    compression_level: u32,
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Args)]
struct CreateArgs {
    #[arg(long)]
    source: PathBuf,
    #[arg(long)]
    target: PathBuf,
    #[arg(long)]
    output: PathBuf,
    #[arg(long, default_value = "unknown")]
    source_version: String,
    #[arg(long, default_value = "unknown")]
    source_build_id: String,
    #[arg(long, default_value = "unknown")]
    target_version: String,
    #[arg(long, default_value = "unknown")]
    target_build_id: String,
    #[arg(long, default_value = "aios-database.exe")]
    target_name: String,
    /// qbsdiff's bzip2 level. Level 1 is normally both faster and no larger.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(0..=9))]
    compression_level: u32,
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Args, Default)]
struct ApplyArgs {
    /// Installed executable to replace. Defaults to aios-database.exe beside this patch.
    #[arg(long)]
    target: Option<PathBuf>,
    /// Read the bundle from another file. Intended for automation and tests.
    #[arg(long, hide = true)]
    patch: Option<PathBuf>,
    /// Verify source identity and embedded payload without writing anything.
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args, Default)]
struct InspectArgs {
    /// Patch executable to inspect. Defaults to the current executable.
    #[arg(long)]
    patch: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ArtifactIdentity {
    version: String,
    build_id: String,
    sha256: String,
    bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PatchManifest {
    format: String,
    product: String,
    generator_version: String,
    algorithm: String,
    created_unix: u64,
    target_filename: String,
    source: ArtifactIdentity,
    target: ArtifactIdentity,
    patch_sha256: String,
    patch_bytes: u64,
}

#[derive(Debug)]
struct BundleLayout {
    base_bytes: u64,
    manifest_bytes: u64,
    patch_start: u64,
    patch_bytes: u64,
    manifest: PatchManifest,
}

#[derive(Debug, Serialize)]
struct ApplyReceipt {
    status: &'static str,
    patch: String,
    target: String,
    from_version: String,
    from_build_id: String,
    to_version: String,
    to_build_id: String,
    target_sha256: String,
    backup: Option<String>,
}

fn main() -> Result<()> {
    run_cli()
}

pub fn run_cli() -> Result<()> {
    match Cli::parse().command {
        Some(Command::Make(args)) => make_patch(&args),
        Some(Command::Create(args)) => create_patch(&args),
        Some(Command::Apply(args)) => apply_patch(&args),
        Some(Command::Inspect(args)) => inspect_patch(&args),
        None => {
            let current = std::env::current_exe().context("locate current executable")?;
            if read_bundle_layout(&current).is_ok() {
                apply_patch(&ApplyArgs::default())
            } else {
                run_interactive_maker()
            }
        }
    }
}

pub fn current_executable_has_bundle() -> bool {
    std::env::current_exe()
        .ok()
        .is_some_and(|path| read_bundle_layout(&path).is_ok())
}

#[derive(Debug, Serialize, Deserialize)]
struct ExecutableBuildInfo {
    product: String,
    version: String,
    build_id: String,
}

#[derive(Debug, Deserialize)]
struct ExecutedApplyReceipt {
    status: String,
    backup: Option<String>,
}

struct ResolvedExecutable {
    original: PathBuf,
    executable: PathBuf,
    kind: &'static str,
    _temp: Option<tempfile::TempDir>,
}

fn make_patch(args: &MakeArgs) -> Result<()> {
    make_patch_from_inputs(
        &args.source,
        &args.target,
        args.output_dir.as_deref(),
        args.compression_level,
        args.force,
    )
    .map(|_| ())
}

fn make_patch_from_inputs(
    source: &Path,
    target: &Path,
    output_dir: Option<&Path>,
    compression_level: u32,
    force: bool,
) -> Result<PathBuf> {
    let source = resolve_executable_input(source, "old")?;
    let target = resolve_executable_input(target, "new")?;
    let source_info = executable_build_info(&source.executable).with_context(|| {
        format!(
            "read old executable identity from {}",
            source.original.display()
        )
    })?;
    let target_info = executable_build_info(&target.executable).with_context(|| {
        format!(
            "read new executable identity from {}",
            target.original.display()
        )
    })?;
    ensure!(
        source_info.product == PRODUCT && target_info.product == PRODUCT,
        "both inputs must identify as {PRODUCT}; got {:?} and {:?}",
        source_info.product,
        target_info.product
    );

    let target_hash = sha256_file(&target.executable)?;
    let output_dir = output_dir
        .map(Path::to_path_buf)
        .or_else(|| source.original.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("create output directory {}", output_dir.display()))?;
    let output = output_dir.join(format!(
        "aios-database-{}-to-{}-{}.patch.exe",
        file_token(&source_info.build_id),
        file_token(&target_info.build_id),
        &target_hash[..12]
    ));
    let create = CreateArgs {
        source: source.executable.clone(),
        target: target.executable.clone(),
        output: output.clone(),
        source_version: source_info.version.clone(),
        source_build_id: source_info.build_id.clone(),
        target_version: target_info.version.clone(),
        target_build_id: target_info.build_id.clone(),
        target_name: "aios-database.exe".into(),
        compression_level,
        force,
    };
    create_patch(&create)?;
    verify_generated_patch(&output, &source.executable, &target.executable)?;

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "status": "created-and-verified",
            "patch": output,
            "source_input": source.original,
            "source_input_kind": source.kind,
            "target_input": target.original,
            "target_input_kind": target.kind,
            "source": source_info,
            "target": target_info,
            "target_sha256": target_hash,
            "usage": "stop the service, place the patch beside aios-database.exe, then run the patch"
        }))?
    );
    Ok(output)
}

pub fn make_patch_for_ui(source: &Path, target: &Path, output_dir: &Path) -> Result<PathBuf> {
    make_patch_from_inputs(source, target, Some(output_dir), 1, false)
}

fn resolve_executable_input(path: &Path, role: &str) -> Result<ResolvedExecutable> {
    ensure!(
        path.is_file(),
        "{role} input does not exist: {}",
        path.display()
    );
    let is_zip = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"));
    if !is_zip {
        return Ok(ResolvedExecutable {
            original: path.to_path_buf(),
            executable: path.to_path_buf(),
            kind: "executable",
            _temp: None,
        });
    }

    let file = File::open(path).with_context(|| format!("open release ZIP {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("read release ZIP {}", path.display()))?;
    let mut exact = None;
    let mut candidates = Vec::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().replace('\\', "/");
        if name.eq_ignore_ascii_case("aios-database.exe") {
            exact = Some(index);
            break;
        }
        if name
            .rsplit('/')
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("aios-database.exe"))
        {
            candidates.push(index);
        }
    }
    let index = exact
        .or_else(|| (candidates.len() == 1).then_some(candidates[0]))
        .with_context(|| {
            format!(
                "{} must contain one unambiguous aios-database.exe (found {} nested candidates)",
                path.display(),
                candidates.len()
            )
        })?;

    let temp = tempfile::Builder::new()
        .prefix("aios-patch-input-")
        .tempdir()
        .context("create ZIP extraction directory")?;
    let executable = temp.path().join("aios-database.exe");
    let mut entry = archive.by_index(index)?;
    ensure!(
        entry.size() > 0,
        "{} contains an empty aios-database.exe",
        path.display()
    );
    let mut output = File::create(&executable)
        .with_context(|| format!("create extracted executable {}", executable.display()))?;
    std::io::copy(&mut entry, &mut output)
        .with_context(|| format!("extract aios-database.exe from {}", path.display()))?;
    output.sync_all().context("sync extracted executable")?;

    Ok(ResolvedExecutable {
        original: path.to_path_buf(),
        executable,
        kind: "release-zip",
        _temp: Some(temp),
    })
}

fn executable_build_info(path: &Path) -> Result<ExecutableBuildInfo> {
    let output = ProcessCommand::new(path)
        .args(["version", "--json"])
        .current_dir(path.parent().unwrap_or_else(|| Path::new(".")))
        .output()
        .with_context(|| format!("run {} version --json", path.display()))?;
    if output.status.success() {
        let info: ExecutableBuildInfo = serde_json::from_slice(&output.stdout)
            .with_context(|| format!("parse {} version --json", path.display()))?;
        ensure!(!info.version.trim().is_empty(), "version is empty");
        ensure!(!info.build_id.trim().is_empty(), "build_id is empty");
        return Ok(info);
    }

    let fallback = ProcessCommand::new(path)
        .arg("--version")
        .current_dir(path.parent().unwrap_or_else(|| Path::new(".")))
        .output()
        .with_context(|| format!("run {} --version", path.display()))?;
    ensure!(
        fallback.status.success(),
        "version probe failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let text = String::from_utf8_lossy(&fallback.stdout);
    let mut words = text.split_whitespace();
    let product = words.next().unwrap_or_default().to_string();
    let version = words.next().unwrap_or_default().to_string();
    ensure!(!version.is_empty(), "unrecognized --version output: {text}");
    Ok(ExecutableBuildInfo {
        product,
        build_id: version.clone(),
        version,
    })
}

fn file_token(value: &str) -> String {
    let mut token = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    token.truncate(64);
    if token.is_empty() {
        "unknown".into()
    } else {
        token
    }
}

fn execute_generated_patch(
    patch: &Path,
    installed: &Path,
    dry_run: bool,
) -> Result<ExecutedApplyReceipt> {
    let mut command = ProcessCommand::new(patch);
    command
        .arg("apply")
        .arg("--target")
        .arg(installed)
        .arg("--json");
    if dry_run {
        command.arg("--dry-run");
    }
    let output = command
        .output()
        .with_context(|| format!("execute generated patch {}", patch.display()))?;
    ensure!(
        output.status.success(),
        "generated patch verification failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    serde_json::from_slice(&output.stdout).context("parse generated patch receipt")
}

fn verify_generated_patch(patch: &Path, source: &Path, target: &Path) -> Result<()> {
    let temp = tempfile::tempdir().context("create patch verification directory")?;
    let installed = temp.path().join("aios-database.exe");
    fs::copy(source, &installed).context("copy source into patch verification directory")?;

    let dry_run = execute_generated_patch(patch, &installed, true)?;
    ensure!(
        dry_run.status == "verified",
        "dry-run returned {:?}",
        dry_run.status
    );
    ensure!(
        sha256_file(&installed)? == sha256_file(source)?,
        "dry-run modified the source executable"
    );

    let applied = execute_generated_patch(patch, &installed, false)?;
    ensure!(
        applied.status == "applied",
        "first apply returned {:?}",
        applied.status
    );
    ensure!(
        sha256_file(&installed)? == sha256_file(target)?,
        "applied executable does not match the target"
    );
    let backup = applied
        .backup
        .as_deref()
        .map(Path::new)
        .context("first apply did not report a backup")?;
    ensure!(
        sha256_file(backup)? == sha256_file(source)?,
        "backup does not match the source"
    );

    let second = execute_generated_patch(patch, &installed, false)?;
    ensure!(
        second.status == "already-current",
        "second apply returned {:?}",
        second.status
    );
    Ok(())
}

#[cfg(not(windows))]
fn prompt_path(label: &str, default: Option<&Path>) -> Result<PathBuf> {
    match default {
        Some(path) => print!("{label} [{}]: ", path.display()),
        None => print!("{label}: "),
    }
    stdout().flush().context("flush prompt")?;
    let mut line = String::new();
    stdin().read_line(&mut line).context("read console input")?;
    let value = line.trim().trim_matches('"');
    if value.is_empty() {
        return default.map(Path::to_path_buf).context("a path is required");
    }
    Ok(PathBuf::from(value))
}

#[cfg(windows)]
fn pick_windows_executable(title: &str) -> Result<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    use windows_sys::Win32::UI::Controls::Dialogs::{
        CommDlgExtendedError, GetOpenFileNameW, OFN_DONTADDTORECENT, OFN_EXPLORER,
        OFN_FILEMUSTEXIST, OFN_HIDEREADONLY, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };

    let mut file_buffer = [0u16; 32_768];
    let filter = "AIOS Database executable (*.exe)\0*.exe\0All files (*.*)\0*.*\0\0"
        .encode_utf16()
        .collect::<Vec<_>>();
    let title = title
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let default_extension = "exe\0".encode_utf16().collect::<Vec<_>>();
    let mut dialog: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    dialog.hwndOwner = unsafe { GetConsoleWindow() };
    dialog.lpstrFilter = filter.as_ptr();
    dialog.nFilterIndex = 1;
    dialog.lpstrFile = file_buffer.as_mut_ptr();
    dialog.nMaxFile = file_buffer.len() as u32;
    dialog.lpstrTitle = title.as_ptr();
    dialog.lpstrDefExt = default_extension.as_ptr();
    dialog.Flags = OFN_EXPLORER
        | OFN_FILEMUSTEXIST
        | OFN_PATHMUSTEXIST
        | OFN_NOCHANGEDIR
        | OFN_HIDEREADONLY
        | OFN_DONTADDTORECENT;

    if unsafe { GetOpenFileNameW(&mut dialog) } == 0 {
        let error = unsafe { CommDlgExtendedError() };
        if error == 0 {
            anyhow::bail!("file selection was cancelled");
        }
        anyhow::bail!("Windows file dialog failed with code 0x{error:08x}");
    }
    let length = file_buffer
        .iter()
        .position(|word| *word == 0)
        .unwrap_or(file_buffer.len());
    ensure!(length > 0, "Windows file dialog returned an empty path");
    Ok(PathBuf::from(OsString::from_wide(&file_buffer[..length])))
}

#[cfg(windows)]
fn run_interactive_maker_inner() -> Result<()> {
    println!("① 请选择现场正在使用的旧 aios-database.exe");
    let source = pick_windows_executable("选择现场旧 aios-database.exe")?;
    println!("   旧版：{}", source.display());
    println!("② 请选择新版 aios-database.exe");
    let target = pick_windows_executable("选择新版 aios-database.exe")?;
    println!("   新版：{}", target.display());
    let output_dir = source
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    make_patch_from_inputs(&source, &target, Some(&output_dir), 1, false).map(|_| ())
}

#[cfg(not(windows))]
fn run_interactive_maker_inner() -> Result<()> {
    println!("推荐流程：先输入现场正在使用的旧 EXE，再输入新版 EXE。");
    let source = prompt_path("① 现场旧 aios-database.exe", None)?;
    let target = prompt_path("② 新版 aios-database.exe", None)?;
    let default_output = source
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let output_dir = prompt_path("补丁输出目录", Some(&default_output))?;
    make_patch_from_inputs(&source, &target, Some(&output_dir), 1, false).map(|_| ())
}

fn run_interactive_maker() -> Result<()> {
    println!("AIOS Patch Maker");
    let result = run_interactive_maker_inner();
    if let Err(error) = &result {
        eprintln!("生成失败：{error:#}");
    } else {
        println!("补丁已生成并完成 dry-run、真实替换、备份和二次幂等验证。");
    }
    print!("按回车键关闭...");
    stdout().flush().ok();
    let mut pause = String::new();
    stdin().read_line(&mut pause).ok();
    result
}

fn create_patch(args: &CreateArgs) -> Result<()> {
    let stub = std::env::current_exe().context("locate patch generator executable")?;
    create_patch_from_stub(args, &stub)
}

fn create_patch_from_stub(args: &CreateArgs, stub: &Path) -> Result<()> {
    ensure!(
        args.source.is_file(),
        "source does not exist: {}",
        args.source.display()
    );
    ensure!(
        args.target.is_file(),
        "target does not exist: {}",
        args.target.display()
    );
    ensure!(
        args.source.canonicalize()? != args.target.canonicalize()?,
        "source and target are the same file"
    );
    validate_target_filename(&args.target_name)?;
    if args.output.exists() {
        ensure!(
            args.force,
            "output already exists (pass --force to replace it): {}",
            args.output.display()
        );
    }
    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create output directory {}", parent.display()))?;
    }

    let source_file = File::open(&args.source)
        .with_context(|| format!("open source {}", args.source.display()))?;
    let target_file = File::open(&args.target)
        .with_context(|| format!("open target {}", args.target.display()))?;
    let source = unsafe { Mmap::map(&source_file) }.context("map source executable")?;
    let target = unsafe { Mmap::map(&target_file) }.context("map target executable")?;
    ensure!(!source.is_empty(), "source executable is empty");
    ensure!(!target.is_empty(), "target executable is empty");

    let source_sha256 = sha256_bytes(&source);
    let target_sha256 = sha256_bytes(&target);
    ensure!(
        source_sha256 != target_sha256,
        "source and target have identical SHA-256; no patch is needed"
    );

    let delta_path = temporary_sibling(&args.output, "bsdiff");
    let output_temp = temporary_sibling(&args.output, "assembling");
    let outcome = (|| -> Result<()> {
        let delta_file = File::create(&delta_path)
            .with_context(|| format!("create temporary delta {}", delta_path.display()))?;
        let mut delta_writer = BufWriter::new(delta_file);
        let patch_bytes = Bsdiff::new(&source, &target)
            .compression_level(args.compression_level)
            .compare(&mut delta_writer)
            .context("generate bsdiff payload")?;
        delta_writer.flush().context("flush bsdiff payload")?;
        let delta_file = delta_writer
            .into_inner()
            .map_err(|error| error.into_error())
            .context("finish bsdiff payload")?;
        delta_file.sync_all().context("sync bsdiff payload")?;
        ensure!(
            patch_bytes == fs::metadata(&delta_path)?.len(),
            "qbsdiff byte count differs from temporary payload length"
        );
        let patch_sha256 = sha256_file(&delta_path)?;

        let manifest = PatchManifest {
            format: FORMAT.into(),
            product: PRODUCT.into(),
            generator_version: env!("CARGO_PKG_VERSION").into(),
            algorithm: "bsdiff4-bzip2".into(),
            created_unix: unix_now(),
            target_filename: args.target_name.clone(),
            source: ArtifactIdentity {
                version: args.source_version.clone(),
                build_id: args.source_build_id.clone(),
                sha256: source_sha256.clone(),
                bytes: source.len() as u64,
            },
            target: ArtifactIdentity {
                version: args.target_version.clone(),
                build_id: args.target_build_id.clone(),
                sha256: target_sha256.clone(),
                bytes: target.len() as u64,
            },
            patch_sha256,
            patch_bytes,
        };

        assemble_bundle(stub, &manifest, &delta_path, &output_temp)?;
        if args.output.exists() {
            fs::remove_file(&args.output)
                .with_context(|| format!("remove old output {}", args.output.display()))?;
        }
        fs::rename(&output_temp, &args.output)
            .with_context(|| format!("publish patch {}", args.output.display()))?;

        let bundle_sha256 = sha256_file(&args.output)?;
        let sidecar = sidecar_path(&args.output, ".sha256");
        fs::write(
            &sidecar,
            format!(
                "{}  sha256={}  bytes={}\n",
                args.output
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("patch.exe"),
                bundle_sha256,
                fs::metadata(&args.output)?.len()
            ),
        )
        .with_context(|| format!("write {}", sidecar.display()))?;
        let manifest_sidecar = sidecar_path(&args.output, ".manifest.json");
        fs::write(&manifest_sidecar, serde_json::to_vec_pretty(&manifest)?)
            .with_context(|| format!("write {}", manifest_sidecar.display()))?;

        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "patch": args.output,
                "sha256": bundle_sha256,
                "bytes": fs::metadata(&args.output)?.len(),
                "payload_bytes": patch_bytes,
                "source": manifest.source,
                "target": manifest.target,
                "manifest": manifest_sidecar,
                "checksum": sidecar,
            }))?
        );
        Ok(())
    })();
    let _ = fs::remove_file(&delta_path);
    let _ = fs::remove_file(&output_temp);
    outcome
}

fn assemble_bundle(
    stub: &Path,
    manifest: &PatchManifest,
    delta_path: &Path,
    output: &Path,
) -> Result<()> {
    let base_bytes = executable_base_bytes(stub)?;
    let manifest_bytes = serde_json::to_vec(manifest)?;
    let patch_bytes = fs::metadata(delta_path)?.len();
    ensure!(
        patch_bytes == manifest.patch_bytes,
        "manifest payload length does not match delta file"
    );

    let mut input =
        File::open(stub).with_context(|| format!("open patch stub {}", stub.display()))?;
    let mut output_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output)
        .with_context(|| format!("create bundle {}", output.display()))?;
    std::io::copy(
        &mut std::io::Read::by_ref(&mut input).take(base_bytes),
        &mut output_file,
    )
    .context("copy executable patch stub")?;
    output_file.write_all(&manifest_bytes)?;
    let mut delta =
        File::open(delta_path).with_context(|| format!("open {}", delta_path.display()))?;
    std::io::copy(&mut delta, &mut output_file).context("append bsdiff payload")?;
    output_file.write_all(&BUNDLE_MAGIC)?;
    output_file.write_all(&(manifest_bytes.len() as u64).to_le_bytes())?;
    output_file.write_all(&patch_bytes.to_le_bytes())?;
    output_file.sync_all().context("sync patch executable")?;
    Ok(())
}

fn inspect_patch(args: &InspectArgs) -> Result<()> {
    let patch = args
        .patch
        .clone()
        .map(Ok)
        .unwrap_or_else(std::env::current_exe)?;
    let layout = read_bundle_layout(&patch)?;
    if args.json {
        println!("{}", serde_json::to_string(&layout.manifest)?);
    } else {
        println!("{}", serde_json::to_string_pretty(&layout.manifest)?);
        println!(
            "bundle: base={} manifest={} patch={}",
            layout.base_bytes, layout.manifest_bytes, layout.patch_bytes
        );
    }
    Ok(())
}

fn apply_patch(args: &ApplyArgs) -> Result<()> {
    let patch_path = args
        .patch
        .clone()
        .map(Ok)
        .unwrap_or_else(std::env::current_exe)?;
    let layout = read_bundle_layout(&patch_path)?;
    let target_path = args.target.clone().unwrap_or_else(|| {
        patch_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(&layout.manifest.target_filename)
    });
    ensure!(
        patch_path.canonicalize().ok() != target_path.canonicalize().ok(),
        "patch executable and target executable resolve to the same file"
    );
    ensure!(
        target_path.is_file(),
        "target executable does not exist: {}",
        target_path.display()
    );

    let source_file = File::open(&target_path)
        .with_context(|| format!("open installed executable {}", target_path.display()))?;
    let source = unsafe { Mmap::map(&source_file) }.context("map installed executable")?;
    let actual_sha256 = sha256_bytes(&source);
    if actual_sha256 == layout.manifest.target.sha256
        && source.len() as u64 == layout.manifest.target.bytes
    {
        return print_receipt(
            args.json,
            ApplyReceipt {
                status: "already-current",
                patch: patch_path.display().to_string(),
                target: target_path.display().to_string(),
                from_version: layout.manifest.source.version.clone(),
                from_build_id: layout.manifest.source.build_id.clone(),
                to_version: layout.manifest.target.version.clone(),
                to_build_id: layout.manifest.target.build_id.clone(),
                target_sha256: actual_sha256,
                backup: None,
            },
        );
    }
    ensure!(
        source.len() as u64 == layout.manifest.source.bytes
            && actual_sha256 == layout.manifest.source.sha256,
        "installed executable is not the required source build.\n\
         expected {} / {} / {} bytes / sha256={}\n\
         actual   {} bytes / sha256={}",
        layout.manifest.source.version,
        layout.manifest.source.build_id,
        layout.manifest.source.bytes,
        layout.manifest.source.sha256,
        source.len(),
        actual_sha256
    );

    let patch_file = File::open(&patch_path)
        .with_context(|| format!("open patch executable {}", patch_path.display()))?;
    let patch_map = unsafe { Mmap::map(&patch_file) }.context("map patch executable")?;
    let patch_start = usize::try_from(layout.patch_start).context("patch offset too large")?;
    let patch_end = usize::try_from(
        layout
            .patch_start
            .checked_add(layout.patch_bytes)
            .context("patch range overflow")?,
    )
    .context("patch end offset too large")?;
    let payload = patch_map
        .get(patch_start..patch_end)
        .context("embedded patch range is outside executable")?;
    ensure!(
        sha256_bytes(payload) == layout.manifest.patch_sha256,
        "embedded patch payload SHA-256 mismatch"
    );

    if args.dry_run {
        return print_receipt(
            args.json,
            ApplyReceipt {
                status: "verified",
                patch: patch_path.display().to_string(),
                target: target_path.display().to_string(),
                from_version: layout.manifest.source.version.clone(),
                from_build_id: layout.manifest.source.build_id.clone(),
                to_version: layout.manifest.target.version.clone(),
                to_build_id: layout.manifest.target.build_id.clone(),
                target_sha256: layout.manifest.target.sha256.clone(),
                backup: None,
            },
        );
    }

    let temp_path = temporary_sibling(&target_path, "patching");
    let permissions = fs::metadata(&target_path)?.permissions();
    let outcome = (|| -> Result<ApplyReceipt> {
        let target_file = File::create(&temp_path)
            .with_context(|| format!("create patched file {}", temp_path.display()))?;
        let patcher = Bspatch::new(payload).context("parse embedded bsdiff payload")?;
        target_file.set_len(patcher.hint_target_size())?;
        let mut writer = BufWriter::with_capacity(128 * 1024, target_file);
        patcher
            .buffer_size(128 * 1024)
            .apply(&source, &mut writer)
            .context("apply binary patch")?;
        writer.flush().context("flush patched executable")?;
        let target_file = writer
            .into_inner()
            .map_err(|error| error.into_error())
            .context("finish patched executable")?;
        target_file.sync_all().context("sync patched executable")?;
        drop(target_file);
        fs::set_permissions(&temp_path, permissions)?;

        let patched_bytes = fs::metadata(&temp_path)?.len();
        let patched_sha256 = sha256_file(&temp_path)?;
        ensure!(
            patched_bytes == layout.manifest.target.bytes
                && patched_sha256 == layout.manifest.target.sha256,
            "patched output identity mismatch: expected {} bytes / {}, got {} bytes / {}",
            layout.manifest.target.bytes,
            layout.manifest.target.sha256,
            patched_bytes,
            patched_sha256
        );

        // Release read mappings before replacement on Windows. ReplaceFileW
        // swaps the files and creates the backup as one filesystem operation;
        // a running service keeps its image locked and the call then fails
        // before changing either path.
        drop(source);
        drop(source_file);
        let backup = unique_backup_path(&target_path, &layout.manifest.source.sha256);
        replace_with_backup(&target_path, &temp_path, &backup).with_context(|| {
            format!(
                "replace {} failed before any change; stop the running aios-database service and retry",
                target_path.display()
            )
        })?;

        Ok(ApplyReceipt {
            status: "applied",
            patch: patch_path.display().to_string(),
            target: target_path.display().to_string(),
            from_version: layout.manifest.source.version.clone(),
            from_build_id: layout.manifest.source.build_id.clone(),
            to_version: layout.manifest.target.version.clone(),
            to_build_id: layout.manifest.target.build_id.clone(),
            target_sha256: patched_sha256,
            backup: Some(backup.display().to_string()),
        })
    })();
    let _ = fs::remove_file(&temp_path);
    print_receipt(args.json, outcome?)
}

fn print_receipt(json: bool, receipt: ApplyReceipt) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(&receipt)?);
    } else {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
    }
    Ok(())
}

fn read_bundle_layout(path: &Path) -> Result<BundleLayout> {
    let mut file =
        File::open(path).with_context(|| format!("open patch bundle {}", path.display()))?;
    let file_bytes = file.metadata()?.len();
    ensure!(
        file_bytes >= FOOTER_BYTES,
        "{} is too small to contain a patch bundle",
        path.display()
    );
    file.seek(SeekFrom::End(-(FOOTER_BYTES as i64)))?;
    let mut footer = [0u8; FOOTER_BYTES as usize];
    file.read_exact(&mut footer)?;
    ensure!(
        footer[..16] == BUNDLE_MAGIC,
        "{} has no embedded AIOS patch bundle",
        path.display()
    );
    let manifest_bytes = u64::from_le_bytes(footer[16..24].try_into().expect("8 bytes"));
    let patch_bytes = u64::from_le_bytes(footer[24..32].try_into().expect("8 bytes"));
    let payload_bytes = manifest_bytes
        .checked_add(patch_bytes)
        .context("bundle length overflow")?;
    let base_bytes = file_bytes
        .checked_sub(FOOTER_BYTES)
        .and_then(|value| value.checked_sub(payload_bytes))
        .context("bundle lengths exceed executable size")?;
    ensure!(base_bytes >= 2, "patch executable base is truncated");
    let manifest_start = base_bytes;
    let patch_start = manifest_start
        .checked_add(manifest_bytes)
        .context("manifest range overflow")?;
    let manifest_len =
        usize::try_from(manifest_bytes).context("manifest is too large for this process")?;
    file.seek(SeekFrom::Start(manifest_start))?;
    let mut manifest_data = vec![0u8; manifest_len];
    file.read_exact(&mut manifest_data)?;
    let manifest: PatchManifest =
        serde_json::from_slice(&manifest_data).context("parse embedded patch manifest")?;
    ensure!(
        manifest.format == FORMAT,
        "unsupported patch format {}",
        manifest.format
    );
    ensure!(
        manifest.product == PRODUCT,
        "patch is for product {}",
        manifest.product
    );
    ensure!(
        manifest.patch_bytes == patch_bytes,
        "footer and manifest disagree about patch length"
    );
    validate_target_filename(&manifest.target_filename)?;
    Ok(BundleLayout {
        base_bytes,
        manifest_bytes,
        patch_start,
        patch_bytes,
        manifest,
    })
}

fn executable_base_bytes(path: &Path) -> Result<u64> {
    let bytes = fs::metadata(path)?.len();
    if bytes < FOOTER_BYTES {
        return Ok(bytes);
    }
    let mut file = File::open(path)?;
    file.seek(SeekFrom::End(-(FOOTER_BYTES as i64)))?;
    let mut magic = [0u8; 16];
    file.read_exact(&mut magic)?;
    if magic != BUNDLE_MAGIC {
        return Ok(bytes);
    }
    Ok(read_bundle_layout(path)?.base_bytes)
}

fn validate_target_filename(name: &str) -> Result<()> {
    ensure!(!name.trim().is_empty(), "target filename is empty");
    let path = Path::new(name);
    ensure!(
        path.file_name().and_then(|value| value.to_str()) == Some(name),
        "target filename must not contain a directory: {name}"
    );
    Ok(())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn temporary_sibling(path: &Path, role: &str) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("aios-database");
    parent.join(format!(".{name}.{role}-{}", std::process::id()))
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn unique_backup_path(target: &Path, source_sha256: &str) -> PathBuf {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let filename = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("aios-database.exe");
    let short_hash = &source_sha256[..source_sha256.len().min(12)];
    for suffix in 0..10_000u32 {
        let counter = if suffix == 0 {
            String::new()
        } else {
            format!("-{suffix}")
        };
        let candidate = parent.join(format!("{filename}.bak-{short_hash}{counter}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!(
        "{filename}.bak-{short_hash}-{}",
        std::process::id()
    ))
}

#[cfg(windows)]
fn replace_with_backup(target: &Path, replacement: &Path, backup: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{REPLACEFILE_WRITE_THROUGH, ReplaceFileW};

    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>()
    };
    let target = wide(target);
    let replacement = wide(replacement);
    let backup = wide(backup);
    let result = unsafe {
        ReplaceFileW(
            target.as_ptr(),
            replacement.as_ptr(),
            backup.as_ptr(),
            REPLACEFILE_WRITE_THROUGH,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_with_backup(target: &Path, replacement: &Path, backup: &Path) -> std::io::Result<()> {
    fs::copy(target, backup)?;
    if let Err(error) = fs::rename(replacement, target) {
        let _ = fs::remove_file(backup);
        return Err(error);
    }
    Ok(())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before Unix epoch")
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    fn identity(version: &str, build: &str, bytes: &[u8]) -> ArtifactIdentity {
        ArtifactIdentity {
            version: version.into(),
            build_id: build.into(),
            sha256: sha256_bytes(bytes),
            bytes: bytes.len() as u64,
        }
    }

    fn write_test_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        for (name, bytes) in entries {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn release_zip_input_extracts_only_the_root_executable() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("release.zip");
        write_test_zip(
            &archive,
            &[
                ("README.md", b"release"),
                ("nested/aios-database.exe", b"wrong"),
                ("aios-database.exe", b"MZ correct"),
            ],
        );

        let resolved = resolve_executable_input(&archive, "old").unwrap();
        assert_eq!(resolved.kind, "release-zip");
        assert_eq!(resolved.original, archive);
        assert_eq!(fs::read(&resolved.executable).unwrap(), b"MZ correct");
        let extracted = resolved.executable.clone();
        drop(resolved);
        assert!(!extracted.exists(), "temporary extraction must be removed");
    }

    #[test]
    fn release_zip_input_rejects_ambiguous_nested_executables() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("ambiguous.zip");
        write_test_zip(
            &archive,
            &[
                ("old/aios-database.exe", b"old"),
                ("new/aios-database.exe", b"new"),
            ],
        );

        let error = resolve_executable_input(&archive, "old")
            .err()
            .expect("ambiguous ZIP must fail")
            .to_string();
        assert!(error.contains("unambiguous"), "{error}");
        assert!(error.contains("2 nested candidates"), "{error}");
    }

    #[test]
    fn self_contained_bundle_applies_and_keeps_a_backup() {
        let temp = tempfile::tempdir().unwrap();
        let source_data = b"MZ old executable bytes repeated old old old";
        let target_data = b"MZ new executable bytes repeated new new new plus feature";
        let source = temp.path().join("old.exe");
        let target = temp.path().join("new.exe");
        let installed = temp.path().join("aios-database.exe");
        let stub = temp.path().join("stub.exe");
        let patch = temp.path().join("update.exe");
        fs::write(&source, source_data).unwrap();
        fs::write(&target, target_data).unwrap();
        fs::write(&installed, source_data).unwrap();
        fs::write(&stub, b"MZ fake patch executable").unwrap();

        create_patch_from_stub(
            &CreateArgs {
                source,
                target,
                output: patch.clone(),
                source_version: "0.1.21".into(),
                source_build_id: "old-build".into(),
                target_version: "0.1.22".into(),
                target_build_id: "new-build".into(),
                target_name: "aios-database.exe".into(),
                compression_level: 1,
                force: false,
            },
            &stub,
        )
        .unwrap();

        let layout = read_bundle_layout(&patch).unwrap();
        assert_eq!(
            layout.manifest.source,
            identity("0.1.21", "old-build", source_data)
        );
        assert_eq!(
            layout.manifest.target,
            identity("0.1.22", "new-build", target_data)
        );
        assert_eq!(layout.base_bytes, b"MZ fake patch executable".len() as u64);

        apply_patch(&ApplyArgs {
            target: Some(installed.clone()),
            patch: Some(patch.clone()),
            dry_run: true,
            json: true,
        })
        .unwrap();
        assert_eq!(
            fs::read(&installed).unwrap(),
            source_data,
            "dry-run must not write the installed executable"
        );

        apply_patch(&ApplyArgs {
            target: Some(installed.clone()),
            patch: Some(patch.clone()),
            dry_run: false,
            json: true,
        })
        .unwrap();
        assert_eq!(fs::read(&installed).unwrap(), target_data);
        let backups = fs::read_dir(temp.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("aios-database.exe.bak-"))
            })
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read(&backups[0]).unwrap(), source_data);

        apply_patch(&ApplyArgs {
            target: Some(installed.clone()),
            patch: Some(patch),
            dry_run: false,
            json: true,
        })
        .unwrap();
        assert_eq!(fs::read(&installed).unwrap(), target_data);
        assert_eq!(
            fs::read_dir(temp.path())
                .unwrap()
                .flatten()
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("aios-database.exe.bak-")
                })
                .count(),
            1,
            "an already-current second apply must not create another backup"
        );
    }

    #[test]
    fn wrong_source_hash_is_rejected_before_writing() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("old.exe");
        let target = temp.path().join("new.exe");
        let installed = temp.path().join("aios-database.exe");
        let stub = temp.path().join("stub.exe");
        let patch = temp.path().join("update.exe");
        fs::write(&source, b"old").unwrap();
        fs::write(&target, b"new").unwrap();
        fs::write(&installed, b"other").unwrap();
        fs::write(&stub, b"MZ stub").unwrap();
        create_patch_from_stub(
            &CreateArgs {
                source,
                target,
                output: patch.clone(),
                source_version: "1".into(),
                source_build_id: "old".into(),
                target_version: "2".into(),
                target_build_id: "new".into(),
                target_name: "aios-database.exe".into(),
                compression_level: 1,
                force: false,
            },
            &stub,
        )
        .unwrap();
        let error = apply_patch(&ApplyArgs {
            target: Some(installed.clone()),
            patch: Some(patch),
            dry_run: false,
            json: false,
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("not the required source build"));
        assert_eq!(fs::read(installed).unwrap(), b"other");
    }

    #[test]
    fn a_corrupt_embedded_payload_is_rejected_before_writing() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("old.exe");
        let target = temp.path().join("new.exe");
        let installed = temp.path().join("aios-database.exe");
        let stub = temp.path().join("stub.exe");
        let patch = temp.path().join("update.exe");
        fs::write(&source, b"MZ old payload source").unwrap();
        fs::write(&target, b"MZ new payload target with changes").unwrap();
        fs::copy(&source, &installed).unwrap();
        fs::write(&stub, b"MZ stub").unwrap();
        create_patch_from_stub(
            &CreateArgs {
                source,
                target,
                output: patch.clone(),
                source_version: "1".into(),
                source_build_id: "old".into(),
                target_version: "2".into(),
                target_build_id: "new".into(),
                target_name: "aios-database.exe".into(),
                compression_level: 1,
                force: false,
            },
            &stub,
        )
        .unwrap();

        let layout = read_bundle_layout(&patch).unwrap();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&patch)
            .unwrap();
        file.seek(SeekFrom::Start(layout.patch_start)).unwrap();
        let mut byte = [0u8; 1];
        file.read_exact(&mut byte).unwrap();
        byte[0] ^= 0xff;
        file.seek(SeekFrom::Start(layout.patch_start)).unwrap();
        file.write_all(&byte).unwrap();
        file.sync_all().unwrap();
        drop(file);

        let error = apply_patch(&ApplyArgs {
            target: Some(installed.clone()),
            patch: Some(patch),
            dry_run: false,
            json: false,
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("payload SHA-256 mismatch"), "{error}");
        assert_eq!(fs::read(installed).unwrap(), b"MZ old payload source");
    }
}
