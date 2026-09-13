//! ADR-053 direct 模式的取数底座：不经 SurrealDB，直接从 db 文件取元素属性。
//!
//! 三件事各有一处权威，别处不得再有第二份：
//!
//! * **读哪个时点**——调用方显式钉了就是它，没钉就是开库那一刻文件自报的最新会话
//!   （ADR-054：未指定时点一律取文件最新；`applied_sesno` 是摄入水位，不是时点来源）。
//!   索引是 copy-on-write，会话页各自携带当时的索引根，所以「按会话号打开」不是事后
//!   过滤，而是选哪棵树下降；`ReadOnlyEngine::open_at` 就是这一步。要与 DB 模式对拍
//!   读同一逻辑时点，用 [`pins_from_watermark`] 显式钉到 `applied_sesno`。
//! * **refno 属于哪个库**——从 refno 自身**位解码**（[`super::refno_route`]，与 core.dll
//!   `DB_Element::dbno` 同式），再拿 dbnum 问 [`CataDbLocator`] 要文件。`Ref0`（word0）不是
//!   `dbnum`——它还带着 extent 位——但解码就够了，不需要也不再有 ref0 反查表；解不出、
//!   或解出的库谁都不认识就报错，不凑一个看着像真的。跨库引用换库与跨库 owner 上溯
//!   （DESI 元素的 owner 在 SITE 库）全靠这一步。开库后核一次文件自报的库号（不变量 3）。
//! * **属性长什么形状**——[`super::direct_attmap`]，它查 DB 读侧的同一张 schema。
//!
//! 定位器登记不出的 dbnum、反查不到的 ref0、打不开的文件，一律 `Err` 并说出是哪个。
//! 生成期一个静默的空 attmap，下游会当成「这个元素没有属性」照常出图。
//!
//! **并发**：`DashMap<dbnum, Arc<Mutex<DbSession>>>`（ADR-053 R4 起步形态）。
//! 存 `Arc` 而不是直接存 `Mutex` 是必需的：拿 `Arc` 克隆出来、放掉 DashMap 的分片锁，
//! 再去锁会话。否则一个线程「持分片读锁等会话锁」、另一个「持会话锁等分片写锁」，
//! 就是一对死锁。**本模块任何路径都不得在持有会话锁时写 `engines`。**

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use aios_core::{NamedAttrMap, RefU64};
use dashmap::DashMap;
use e3d_io::record::element::ParsedElement;
use e3d_io::record::explicit::ExplicitAttribute;
use e3d_io::record::template::TemplateProvider;
use e3d_io::schema::MANIFEST_FILE;
use e3d_io::{E3dVersion, ReadOnlyEngine, SchemaPack};

use super::cata_closure::CataDbLocator;
use super::direct_attmap::{self, DirectAttrs};
use super::direct_index::{DbIndexes, IndexFingerprint};
use super::refno_route::{refno_dbnum, to_refno};

/// 显式覆盖模板与字典文件所在目录。
///
/// 不设置时不再默认为某台机器的 E3D 3.1 shadow：[`DirectSchema::open_from_env`]
/// 优先认 DbOption 手填的 `e3d_version`，其次认本变量；两者都没设置时使用
/// [`DEFAULT_E3D_VERSION`]。目录名与数据库二进制都不参与判定（后者没有可靠的
/// 版本字段）。设置本变量表示调用方明确接管选择，保留给临时探针和夹具。
pub const TEMPLATE_DIR_ENV: &str = "AIOS_E3D_TEMPLATE_DIR";
/// e3d-io schema packs 的父目录；未设置时先找 exe 同级 `schema/`（部署包），
/// 开发构建再回落到 e3d-io 源码携带的目录。
pub const SCHEMA_ROOT_ENV: &str = "AIOS_E3D_SCHEMA_ROOT";
/// `DbOption.toml` 未配置 `e3d_version` 时使用的 schema pack。
///
/// 这是产品默认值，不是数据库二进制识别结果；版本证据仍会写入启动日志供排障。
pub const DEFAULT_E3D_VERSION: &str = "e3d210";

fn schema_root_next_to_executable() -> Option<PathBuf> {
    let root = std::env::current_exe().ok()?.parent()?.join("schema");
    root.is_dir().then_some(root)
}

fn report_schema_info(message: &str) {
    log::info!("{message}");
    println!("{message}");
}

fn report_schema_error(message: &str) {
    log::error!("{message}");
    eprintln!("{message}");
}

/// 一个库的读取时点与身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbPin {
    pub dbnum: i32,
    /// `DESI` / `CATA` / `DICT` …… 决定用哪个模板文件。
    pub db_type: String,
    pub file: PathBuf,
    /// 钉住的会话号。
    ///
    /// `None` **不是「每次读最新」**，是「开库那一刻解一次文件自报的最新，然后整个
    /// 运行内冻住」——这是 DB 模式对目录库的语义（`OnDemandDbSession` →
    /// `DabaconSnapshot` 存下 `target_sesno` 再 `open_at` 它）。差别是致命的：一个生成
    /// 单元里两次「读最新」若跨了一次目录存盘，会把两个目录版本拼进同一个元件——
    /// 第一轮拿旧 SCOM、第四轮拿新 SPRE，各自自洽，合起来是个不存在的元件，而且
    /// 不报错，只会长出一个形状不对的模型。
    ///
    /// 冻结点解出来之后可以从 [`DirectStore::pinned_sesno`] 读回。这就是**未指定时点**
    /// 的默认（ADR-054）；`Some(sesno)` 只在调用方显式钉时点时出现——历史投影、
    /// 或对拍探针经 [`pins_from_watermark`] 钉到 `applied_sesno`。
    pub sesno: Option<u32>,
}

/// 文件身份：路径还在、文件已被换掉，是目录库 reinit / 外部拷贝的常态。
///
/// DB 侧靠 `SnapshotToken` 的 `StableFileId` + `verify_path_identity()` 拦这件事，direct
/// 不能没有：文件被换掉之后，冻住的那个会话号会指向**另一个文件里同号的一个不相干
/// 的会话**，读出来的东西完全正常，只是不是你要的那个工程。
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    len: u64,
    modified: Option<std::time::SystemTime>,
}

impl FileIdentity {
    fn of(path: &Path) -> std::io::Result<Self> {
        let meta = std::fs::metadata(path)?;
        Ok(Self {
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectStoreError {
    /// `word1` 为负或解出 dbnum 0：这串数字不指向任何库（`refno_route::refno_dbnum` 给 `None`）。
    InvalidRefNo {
        refno: String,
    },

    /// 编号合法、dbnum 解出来了，但既没被钉住、定位器也没有这个库的文件：确定性的
    /// 「不在这个 MDB」。`web_service` 把它与 `NoSuchElement` 同码 404。
    DbNotInMdb {
        dbnum: i32,
        refno: String,
    },

    /// 文件自报的库号与要开的不一致（不变量 3「文件自证」）：路径还在、文件被换成
    /// 别的库，或定位器把 dbnum 绑到了错的文件。读出来的东西完全正常，只是不是你要的库。
    DbNumberMismatch {
        dbnum: i32,
        actual: i32,
        file: PathBuf,
    },

    Session {
        dbnum: i32,
        detail: String,
    },

    NoSuchElement {
        refno: String,
        dbnum: i32,
        sesno: Option<u32>,
    },

    /// 冻结时点之后文件被换掉了。宁可报错阻断——同号会话在另一个文件里是另一件事。
    FileReplaced {
        dbnum: i32,
        file: PathBuf,
    },

    /// 定位器登记不出这个库的文件。
    NoFileForDbnum {
        dbnum: i32,
    },

    Extract {
        refno: String,
        detail: String,
    },

    Convert {
        refno: String,
        source: direct_attmap::DirectAttrError,
    },

    Poisoned {
        slot: &'static str,
    },
}

impl std::fmt::Display for DirectStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRefNo { refno } => {
                write!(
                    f,
                    "{refno} 不是合法 RefNo（word1 为负或解出 dbnum 0），无库可读"
                )
            }
            Self::DbNotInMdb { dbnum, refno } => {
                write!(
                    f,
                    "{refno} 解出 dbnum {dbnum}，但它不在当前 MDB 也不在依赖目录里"
                )
            }
            Self::DbNumberMismatch {
                dbnum,
                actual,
                file,
            } => write!(
                f,
                "要开 dbnum {dbnum}，文件 {} 自报 dbnum {actual}：不是同一个库",
                file.display()
            ),
            Self::Session { dbnum, detail } => write!(f, "dbnum {dbnum}：{detail}"),
            Self::NoSuchElement {
                refno,
                dbnum,
                sesno,
            } => write!(f, "dbnum {dbnum} 会话 {sesno:?} 的索引里没有 {refno}"),
            Self::FileReplaced { dbnum, file } => write!(
                f,
                "dbnum {dbnum} 的文件在冻结时点之后被换过了（{}）：同号会话在新文件里是另一件事",
                file.display()
            ),
            Self::NoFileForDbnum { dbnum } => {
                write!(f, "定位器登记不出 dbnum {dbnum} 的文件")
            }
            Self::Extract { refno, detail } => write!(f, "{refno}：{detail}"),
            Self::Convert { refno, source } => write!(f, "{refno}：{source}"),
            Self::Poisoned { slot } => {
                write!(f, "{slot} 锁已被另一个线程的 panic 毒化")
            }
        }
    }
}

impl std::error::Error for DirectStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Convert { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// attlib 字典单例：全进程一份，所有库共用。
///
/// 模板 provider 不在这里——`extract_element_with_descriptors` 要 `&mut` 它，共享不了，
/// 所以它按库开在 [`DbSession`] 里。这里只放真正不可变、真正共用的那一份。
pub struct DirectSchema {
    attlib: e3d_attlib::AttlibData,
    template_dir: PathBuf,
    version: Option<&'static E3dVersion>,
}

impl std::fmt::Debug for DirectSchema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectSchema")
            .field("template_dir", &self.template_dir)
            .field("version", &self.version.map(|version| version.id))
            .finish_non_exhaustive()
    }
}

impl DirectSchema {
    /// TOML 手动版本优先，其次 [`TEMPLATE_DIR_ENV`]；均未设置时读取项目数据库的
    /// 版本证据供诊断，然后打开 [`DEFAULT_E3D_VERSION`]。
    pub fn open_from_env() -> anyhow::Result<Self> {
        let started = Instant::now();
        if let Some(version) =
            crate::options::configured_e3d_version().map_err(anyhow::Error::msg)?
        {
            let schema_root = std::env::var_os(SCHEMA_ROOT_ENV)
                .map(PathBuf::from)
                .or_else(schema_root_next_to_executable)
                .unwrap_or_else(e3d_io::bundled_schema_root);
            let context = format!(
                "mode=manual source=DbOption e3d_version={version} auto_detect=false schema_root={}",
                schema_root.display()
            );
            report_schema_info(&format!("[E3D schema] 开始加载: {context}"));
            return Self::report_load_result(
                Self::open_explicit_version(&schema_root, &version),
                &context,
                started,
            );
        }
        if let Some(dir) = std::env::var_os(TEMPLATE_DIR_ENV) {
            let template_dir = PathBuf::from(dir);
            let context = format!(
                "mode=explicit source={TEMPLATE_DIR_ENV} template_dir={}",
                template_dir.display()
            );
            report_schema_info(&format!("[E3D schema] 开始加载: {context}"));
            return Self::report_load_result(Self::open(template_dir), &context, started);
        }

        // 默认模式：没有任何已证实的库字段能**可靠双向**分开 2.1 与 3.1。页身份、
        // WORL.UPGDNO、COMM.VERS 在两版样本里会相同（见 e3d-io
        // docs/evidence/2026-09-11-e3d-database-version.md）；CRINFO 又是创建工具，
        // TMPLVN 是模板兼容版本，session DAB tag 是最后写入引擎。detect_* 一律失败。
        // 这里把探针证据打出来供排障，但选版明确走产品默认值，不把默认伪装成自动识别。
        let project_root = PathBuf::from(&aios_core::get_db_option().project_path);
        let (schema_root, schema_root_source) =
            if let Some(root) = std::env::var_os(SCHEMA_ROOT_ENV) {
                (PathBuf::from(root), SCHEMA_ROOT_ENV)
            } else if let Some(root) = schema_root_next_to_executable() {
                (root, "executable-adjacent")
            } else {
                (e3d_io::bundled_schema_root(), "development-bundled")
            };
        let context = format!(
            "mode=default source=DbOption-unconfigured e3d_version={DEFAULT_E3D_VERSION} \
             auto_detect=false project={} schema_root={} schema_root_source={schema_root_source}",
            project_root.display(),
            schema_root.display()
        );
        report_schema_info(&format!(
            "[E3D schema] 开始加载: {context}；DbOption 未配置 e3d_version，使用默认 \
             {DEFAULT_E3D_VERSION}；自动识别不可用，仍读取项目数据库二进制版本证据供排障"
        ));
        let evidence = match e3d_io::project_version_evidence(&project_root) {
            Ok(evidence) => {
                let header = match e3d_io::database_header_diagnostics(&evidence.database) {
                    Ok(header) => format!(
                        "header_crinfo={:?} header_tmplno={} header_tmplvn={} \
                         header_format={:?} header_session_page={} header_session_number={} \
                         session_dab_tag={:?}",
                        header.creation_info,
                        header.template_type_name.as_deref().unwrap_or("unknown"),
                        header.template_version,
                        header.header_format,
                        header.session_page.page,
                        header.session_number,
                        header.dabacon_version_tag
                    ),
                    Err(error) => format!("header_diagnostics_error={error}"),
                };
                format!(
                    "database={} field={} raw_value={:?} raw_text={:?} session_page={:?} {header}",
                    evidence.database.display(),
                    evidence.source.field_name(),
                    evidence.raw_value,
                    evidence.raw_text,
                    evidence.session_page
                )
            }
            Err(error) => format!("读取失败: {error}"),
        };
        report_schema_info(&format!(
            "[E3D schema] 项目数据库二进制版本证据（仅诊断，不用于选版本）: project={} {evidence}",
            project_root.display()
        ));
        Self::report_load_result(
            Self::open_explicit_version(&schema_root, DEFAULT_E3D_VERSION),
            &context,
            started,
        )
    }

    fn open_explicit_version(schema_root: &Path, version: &str) -> anyhow::Result<Self> {
        let version = e3d_io::schema::version_by_id(version.trim())
            .ok_or_else(|| anyhow::anyhow!("e3d_version={version:?} 无效：支持 e3d210 / e3d31"))?;
        let manifest = schema_root.join(version.id).join(MANIFEST_FILE);
        report_schema_info(&format!(
            "[E3D schema] 开始读取 schema pack: manifest={}",
            manifest.display()
        ));
        let pack = SchemaPack::open_version(schema_root, version.id).map_err(|error| {
            anyhow::anyhow!("读取 E3D schema pack {} 失败: {error}", manifest.display())
        })?;
        report_schema_info(&format!(
            "[E3D schema] schema pack 清单读取成功: pack={} schema={} e3d=\"{}\" \
             template_entries={}",
            pack.root().display(),
            version.id,
            version.display,
            pack.template_types().count()
        ));
        report_schema_info(&format!(
            "[E3D schema] 开始校验 schema 文件并解析 attlib: pack={} \
             required=attlib.dat,DESI,CATA,DICT",
            pack.root().display()
        ));
        Self::open_pack(pack)
    }

    pub fn open(template_dir: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let template_dir = template_dir.into();
        let attlib_path = template_dir.join("attlib.dat");
        anyhow::ensure!(
            attlib_path.is_file(),
            "attlib.dat not found under {}: set {TEMPLATE_DIR_ENV} to the directory holding \
             attlib.dat and the *vir.dat template files",
            template_dir.display()
        );
        let attlib = e3d_attlib::AttlibData::parse_file(&attlib_path)
            .map_err(|e| anyhow::anyhow!("parse {}: {e}", attlib_path.display()))?;
        Ok(Self {
            attlib,
            template_dir,
            version: None,
        })
    }

    fn open_pack(pack: SchemaPack) -> anyhow::Result<Self> {
        let version = pack.version();
        let attlib_path = pack
            .attlib()
            .map_err(|error| anyhow::anyhow!("{} 的 attlib 解析路径失败: {error}", version.id))?;
        let template_dir = attlib_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("{} 没有父目录", attlib_path.display()))?
            .to_path_buf();
        // SchemaPack may search several roots. All release-bound files used
        // by the legacy directory-shaped API must still resolve together;
        // accepting a split would reintroduce the cross-version mix the pack
        // exists to prevent.
        for db_type in ["DESI", "CATA", "DICT"] {
            let template = pack.template(db_type).map_err(|error| {
                anyhow::anyhow!("{} 的 {db_type} 模板解析路径失败: {error}", version.id)
            })?;
            anyhow::ensure!(
                template.parent() == Some(template_dir.as_path()),
                "{} 的 attlib 与 {db_type} 模板不在同一 schema 目录：{} vs {}",
                version.id,
                attlib_path.display(),
                template.display()
            );
        }
        let attlib = e3d_attlib::AttlibData::parse_file(&attlib_path)
            .map_err(|error| anyhow::anyhow!("parse {}: {error}", attlib_path.display()))?;
        Ok(Self {
            attlib,
            template_dir,
            version: Some(version),
        })
    }

    fn report_load_result(
        result: anyhow::Result<Self>,
        context: &str,
        started: Instant,
    ) -> anyhow::Result<Self> {
        match result {
            Ok(schema) => {
                report_schema_info(
                    &schema.load_success_message(context, started.elapsed().as_millis()),
                );
                Ok(schema)
            }
            Err(error) => {
                report_schema_error(&Self::load_failure_message(
                    context,
                    started.elapsed().as_millis(),
                    &error,
                ));
                Err(error)
            }
        }
    }

    fn load_success_message(&self, context: &str, elapsed_ms: u128) -> String {
        let (schema_id, e3d_display, template_validation) = self
            .version
            .map(|version| (version.id, version.display, "DESI,CATA,DICT"))
            .unwrap_or((
                "explicit",
                "未知（AIOS_E3D_TEMPLATE_DIR 显式覆盖未做版本识别）",
                "deferred（按库打开时检查）",
            ));
        format!(
            "[E3D schema] 加载成功: {context} schema={schema_id} e3d=\"{e3d_display}\" \
             template_dir={} attlib={} validated_templates={template_validation} \
             elapsed_ms={elapsed_ms}",
            self.template_dir.display(),
            self.template_dir.join("attlib.dat").display()
        )
    }

    fn load_failure_message(context: &str, elapsed_ms: u128, error: &anyhow::Error) -> String {
        format!("[E3D schema] 加载失败: {context} elapsed_ms={elapsed_ms}；原因: {error:#}")
    }

    pub fn template_dir(&self) -> &Path {
        &self.template_dir
    }

    /// The release named by the schema pack this schema was opened from
    /// (DbOption `e3d_version`). Explicit [`Self::open`] overrides return
    /// `None` because a directory of `.dat` files does not identify its own
    /// release.
    pub fn version(&self) -> Option<&'static E3dVersion> {
        self.version
    }

    /// The decoded system attribute dictionary.  File-side UDA catalog loading
    /// uses the very same dictionary as element extraction, so a request cannot
    /// accidentally combine definitions decoded with a different schema.
    pub fn attlib(&self) -> &e3d_attlib::AttlibData {
        &self.attlib
    }

    /// 一个库类型的模板文件。
    ///
    /// E3D 的命名法是「库类型前三字母小写 + `vir.dat`」：`DESI` → `desvir.dat`、
    /// `CATA` → `catvir.dat`、`DICT` → `dicvir.dat`。派生而不是写死一张表——写死的表
    /// 会在遇到第一个没列进去的库类型时安静地退成 DESI，而那读出来的是别人的 schema。
    fn template_file(&self, db_type: &str) -> anyhow::Result<PathBuf> {
        let stem: String = db_type.chars().take(3).collect::<String>().to_lowercase();
        anyhow::ensure!(
            stem.len() == 3,
            "db_type {db_type:?} is too short to name a template file"
        );
        let path = self.template_dir.join(format!("{stem}vir.dat"));
        anyhow::ensure!(
            path.is_file(),
            "db_type {db_type} needs {}, which is not there",
            path.display()
        );
        Ok(path)
    }
}

/// 一个库的打开态：引擎（已钉会话）+ 该库类型的模板 provider。
struct DbSession {
    engine: ReadOnlyEngine,
    provider: TemplateProvider,
    pin: DbPin,
    /// 实际钉住的会话号。`pin.sesno` 是 `None` 时，这里是开库那一刻解出来的冻结点。
    pinned_sesno: u32,
    identity: FileIdentity,
}

/// 按 dbnum 池化的 direct 取数入口。
pub struct DirectStore {
    schema: Arc<DirectSchema>,
    locator: Arc<dyn CataDbLocator + Send + Sync>,
    pins: DashMap<i32, DbPin>,
    /// 见模块头「并发」一段：存 `Arc`，锁会话前必须先放掉分片锁。
    sessions: DashMap<i32, Arc<Mutex<DbSession>>>,
    attrs: DashMap<RefU64, Arc<DirectAttrs>>,
    /// 每库一份的派生索引（D3），键是 dbnum，随 pin 变更一起失效。
    indexes: DashMap<i32, Arc<DbIndexes>>,
}

/// One element read from one frozen database session.  Keeping the extraction
/// and explicit tail together is important for UDA resolution: `OLDKEY` can
/// only be decided against the exact record that produced the normal attrs.
#[derive(Debug, Clone)]
pub struct DirectElementSnapshot {
    pub dbnum: i32,
    pub db_type: String,
    pub file: PathBuf,
    pub sesno: u32,
    pub extraction: e3d_io::record::descriptor::ElementExtraction,
    pub explicit_attributes: Vec<ExplicitAttribute>,
}

impl DirectStore {
    pub fn new(schema: Arc<DirectSchema>, locator: Arc<dyn CataDbLocator + Send + Sync>) -> Self {
        Self {
            schema,
            locator,
            pins: DashMap::new(),
            sessions: DashMap::new(),
            attrs: DashMap::new(),
            indexes: DashMap::new(),
        }
    }

    /// 登记一个库的文件与时点。重复登记以最后一次为准，并丢掉该库已开的会话
    /// ——换了时点还接着用旧引擎，读到的就是上一个时点的树。
    pub fn pin(&self, pin: DbPin) {
        let dbnum = pin.dbnum;
        let changed = self.pins.get(&dbnum).map(|entry| entry.clone()) != Some(pin.clone());
        self.pins.insert(dbnum, pin);
        if changed {
            self.sessions.remove(&dbnum);
            self.indexes.remove(&dbnum);
            self.attrs
                .retain(|refno, _| self.dbnum_hint(*refno) != Some(dbnum));
        }
    }

    /// 登记一个只有定位器知道的库。
    ///
    /// 目录库（CATA）走 `cata_closure` 按需解析、**不入 `dbnum_watermark`**，所以没有
    /// `applied_sesno` 可钉。它的时点语义是「开库那一刻解一次最新然后冻住」，与 DB 侧
    /// `OnDemandDbSession` 一致——所以 `sesno` 留 `None`，冻结点由
    /// [`DirectStore::pinned_sesno`] 读回。
    ///
    /// 跨库引用（SPRE / LSTU / PSPE / CATR，ams8000 上 82% 的命名引用属性指向别的库）
    /// 要能读通，就得靠这条把被指向的库登记进来。
    pub fn pin_from_locator(&self, dbnum: i32) -> Result<DbPin, DirectStoreError> {
        if let Some(existing) = self.pinned(dbnum) {
            return Ok(existing);
        }
        let (_project, file) = self
            .locator
            .file_of(dbnum as u32)
            .ok_or(DirectStoreError::NoFileForDbnum { dbnum })?;
        let pin = DbPin {
            dbnum,
            db_type: self
                .locator
                .db_type_of(dbnum as u32)
                .unwrap_or_else(|| "CATA".to_string()),
            file,
            sesno: None,
        };
        self.pin(pin.clone());
        Ok(pin)
    }

    pub fn pinned(&self, dbnum: i32) -> Option<DbPin> {
        self.pins.get(&dbnum).map(|entry| entry.clone())
    }

    /// 实际钉住的会话号，开过库之后才有。
    ///
    /// 对拍探针要把它跟每一条比对记在一起：DB 里的 CATA 行是「当初那次按需解析跑的
    /// 时候」写的，可能旧于文件现在的版本。那种不一致是**两个模式的时点差**，不是
    /// direct 读错了——拿不到这个数，时点差只能被报成 bug。
    pub fn pinned_sesno(&self, dbnum: i32) -> Option<u32> {
        let session = self.sessions.get(&dbnum)?.clone();
        let sesno = session.lock().ok()?.pinned_sesno;
        Some(sesno)
    }

    /// 文件还是不是开库时那一个。
    ///
    /// 生成单元边界上调一次。DB 侧 `verify_path_identity()` 拦的就是这件事，direct 侧
    /// 没有它，目录库被换掉时冻住的会话号会安静地指向另一个文件里的另一个会话。
    pub fn verify_file_identity(&self, dbnum: i32) -> Result<(), DirectStoreError> {
        let Some(session) = self.sessions.get(&dbnum).map(|entry| entry.clone()) else {
            return Ok(());
        };
        let session = session
            .lock()
            .map_err(|_| DirectStoreError::Poisoned { slot: "session" })?;
        let now =
            FileIdentity::of(&session.pin.file).map_err(|error| DirectStoreError::Session {
                dbnum,
                detail: format!("stat {}: {error}", session.pin.file.display()),
            })?;
        if now == session.identity {
            return Ok(());
        }
        Err(DirectStoreError::FileReplaced {
            dbnum,
            file: session.pin.file.clone(),
        })
    }

    /// 全部已开库的身份复核。
    pub fn verify_all_file_identities(&self) -> Result<(), DirectStoreError> {
        let open: Vec<i32> = self.sessions.iter().map(|entry| *entry.key()).collect();
        for dbnum in open {
            self.verify_file_identity(dbnum)?;
        }
        Ok(())
    }

    pub fn pin_count(&self) -> usize {
        self.pins.len()
    }

    pub fn cached_elements(&self) -> usize {
        self.attrs.len()
    }

    /// refno → dbnum：**位解码**（`refno_route::refno_dbnum`，与 core.dll `DB_Element::dbno` 同式），
    /// 再问一句「这个库我认识吗」——钉过、或定位器有它的文件，就是认识。
    ///
    /// 既不是 `RefU64::get_0()`（那是 word0，带着 extent 位），也不再走 ref0 反查表：
    /// 同一个库每个 extent 一个 word0，表是为一个不存在的歧义建的（切片计划 §0.2）。
    /// 解不出 → [`DirectStoreError::InvalidRefNo`]；解出了但谁都不认识 →
    /// [`DirectStoreError::DbNotInMdb`]，不凑一个看着像真的。
    pub fn dbnum_of(&self, refno: RefU64) -> Result<i32, DirectStoreError> {
        let dbnum = refno_dbnum(refno).ok_or_else(|| DirectStoreError::InvalidRefNo {
            refno: refno.to_string(),
        })?;
        if self.knows_dbnum(dbnum) {
            return Ok(dbnum as i32);
        }
        Err(DirectStoreError::DbNotInMdb {
            dbnum: dbnum as i32,
            refno: refno.to_string(),
        })
    }

    /// 钉过、或定位器登记得出文件——两者任一即「认识这个库」。只问目录，不开文件。
    fn knows_dbnum(&self, dbnum: u32) -> bool {
        self.pinned(dbnum as i32).is_some() || self.locator.file_of(dbnum).is_some()
    }

    /// 取一个元素的属性，库由 ref0 反查决定。**跨库引用走的就是这条。**
    ///
    /// 反查出来的库还没登记时（目录库不入水位表），自动从定位器补一个 pin。这不是
    /// 「找不到就凑一个」——定位器登记不出那个库照样 `Err`，只是不必让每个调用方
    /// 先手动把被指向的库挂上。
    pub fn attrs(&self, refno: RefU64) -> Result<Arc<DirectAttrs>, DirectStoreError> {
        let dbnum = self.dbnum_of(refno)?;
        if self.pinned(dbnum).is_none() {
            self.pin_from_locator(dbnum)?;
        }
        self.attrs_in(dbnum, refno)
    }

    /// 取一个元素的属性，库由调用方指定。
    pub fn attrs_in(
        &self,
        dbnum: i32,
        refno: RefU64,
    ) -> Result<Arc<DirectAttrs>, DirectStoreError> {
        if let Some(hit) = self.attrs.get(&refno) {
            return Ok(hit.clone());
        }
        let fresh = Arc::new(self.read_attrs(dbnum, refno)?);
        // 两个线程同时读同一个元素时后到的那份被丢掉，值是一样的。
        Ok(self.attrs.entry(refno).or_insert(fresh).clone())
    }

    /// 与 DB 模式同形的出口：只要属性表。
    pub fn named_attmap(&self, refno: RefU64) -> Result<NamedAttrMap, DirectStoreError> {
        Ok(self.attrs(refno)?.map.clone())
    }

    /// Read the complete descriptor extraction and explicit tail under one
    /// per-db session lock.  Callers that render public attributes use this
    /// instead of the compatibility `NamedAttrMap` projection.
    pub fn element_snapshot(
        &self,
        refno: RefU64,
    ) -> Result<DirectElementSnapshot, DirectStoreError> {
        let dbnum = self.dbnum_of(refno)?;
        if self.pinned(dbnum).is_none() {
            self.pin_from_locator(dbnum)?;
        }
        let session = self.session_of(dbnum)?;
        let mut session = session
            .lock()
            .map_err(|_| DirectStoreError::Poisoned { slot: "session" })?;
        let extraction = self.extract(&mut session, dbnum, refno)?;
        let parsed = self.parse(&mut session, dbnum, refno)?;
        Ok(DirectElementSnapshot {
            dbnum,
            db_type: session.pin.db_type.clone(),
            file: session.pin.file.clone(),
            sesno: session.pinned_sesno,
            extraction,
            explicit_attributes: parsed.explicit_attributes,
        })
    }

    /// 一个元素的直接成员，**按记录里存的原序**。
    ///
    /// 顺序是语义的一部分，不是展示细节：BRAN 的成员序就是管路走向。所以这里交的是
    /// `ParsedElement.members` 原样——**不排序、不去重、不从索引反向重建**
    /// （`docs/specs/direct-mode-query-surface.md` §6.5.1）。实测 ams8000_0001 的 2332 个
    /// 带成员表的元素里，有 6 个的成员顺序不等于 refno 序；按 refno 排一遍会让对拍
    /// 照样绿，而模型已经错了。
    ///
    /// 空引用槽位在这里滤掉：那是没写过的槽，不是指向 0 号元素。
    pub fn members(&self, refno: RefU64) -> Result<Vec<RefU64>, DirectStoreError> {
        let dbnum = self.dbnum_of(refno)?;
        if self.pinned(dbnum).is_none() {
            self.pin_from_locator(dbnum)?;
        }
        self.members_in(dbnum, refno)
    }

    /// 直接成员，库由调用方指定。
    pub fn members_in(&self, dbnum: i32, refno: RefU64) -> Result<Vec<RefU64>, DirectStoreError> {
        let session = self.session_of(dbnum)?;
        let mut session = session
            .lock()
            .map_err(|_| DirectStoreError::Poisoned { slot: "session" })?;
        let parsed = self.parse(&mut session, dbnum, refno)?;

        Ok(parsed
            .members
            .iter()
            .filter(|member| member.is_valid())
            .map(|member| RefU64::from_two_nums(member.word0, member.word1))
            .collect())
    }

    /// 一个元素的直接子元素属性表，成员原序。
    ///
    /// 与 DB 模式 `get_children_named_attmaps` 同形。逐个子元素读，读不出来的那个
    /// 直接上浮——生成链拿到一份少了一个孩子的列表，比拿到错误更难查。
    pub fn children_named_attmaps(
        &self,
        refno: RefU64,
    ) -> Result<Vec<NamedAttrMap>, DirectStoreError> {
        self.members(refno)?
            .into_iter()
            .map(|member| self.named_attmap(member))
            .collect()
    }

    /// 转换之前的原始描述符抽取。
    ///
    /// 给探针用：定形规则（哪个存储形状对应哪个 `NamedAttrValue`）必须从真库里的实际
    /// 字节定出来，而不是从类型名上猜。看不到原始值就只能猜。
    pub fn extraction(
        &self,
        dbnum: i32,
        refno: RefU64,
    ) -> Result<e3d_io::record::descriptor::ElementExtraction, DirectStoreError> {
        let session = self.session_of(dbnum)?;
        let mut session = session
            .lock()
            .map_err(|_| DirectStoreError::Poisoned { slot: "session" })?;
        self.extract(&mut session, dbnum, refno)
    }

    /// 一个库的三个派生索引（D3：type / name / backref），按需构建并缓存。
    ///
    /// 首次调用触发一次全树扫描（磁盘缓存指纹命中时只是一次反序列化）；之后
    /// 是 `Arc` 克隆。索引钉在会话时点上，随 [`DirectStore::pin`] 换时点一起
    /// 失效。构建在会话锁内进行——生成期每库一次，扫描本身亚秒级，描述符
    /// 抽取秒级，可以接受；等不起的调用方先在别的线程预热。
    pub fn indexes(&self, dbnum: i32) -> Result<Arc<DbIndexes>, DirectStoreError> {
        if let Some(hit) = self.indexes.get(&dbnum) {
            return Ok(hit.clone());
        }
        let session = self.session_of(dbnum)?;
        let mut session = session
            .lock()
            .map_err(|_| DirectStoreError::Poisoned { slot: "session" })?;
        let DbSession {
            engine,
            provider,
            pin,
            pinned_sesno,
            ..
        } = &mut *session;

        let fingerprint =
            IndexFingerprint::of(&pin.file, dbnum, *pinned_sesno).map_err(|error| {
                DirectStoreError::Session {
                    dbnum,
                    detail: format!("stat {}: {error}", pin.file.display()),
                }
            })?;
        let built = DbIndexes::load_or_build(engine, provider, &self.schema.attlib, fingerprint)
            .map_err(|error| DirectStoreError::Session {
                dbnum,
                detail: format!("building derived indexes: {error:#}"),
            })?;
        drop(session);

        let fresh = Arc::new(built);
        Ok(self.indexes.entry(dbnum).or_insert(fresh).clone())
    }

    /// 谁经 `attribute` 指向 `target`——**读库文件自带的反向引用表**，不是自建索引。
    ///
    /// dabacon 的会话页除了索引根，还挂着一份表目录，其中大部分表以引用属性命名
    /// （`SPRE` / `CATR` / `PRTREF` / `GOBREF`…），每条记录就是一对
    /// `(目标 refno, 引用者 refno)`，按目标排序。core.dll 的
    /// `DB_RefTableIterator::startSearch(dbno, attId, targetRef, handle)` 查的就是它，
    /// `BREF`/`SPBREF` 那一族伪属性也从它取值。取证与语料门见
    /// `docs/evidence/2026-09-03-dabacon-named-tables-and-reference-tables.md`。
    ///
    /// **一张表只答它自己那个库。** 表存在**引用方**的库里，按目标做键：ams8000 的
    /// 设计件把自己的 `SPRE` 记进 ams8000 自己的 `SPRE` 表、键是目录件的 refno。所以
    /// 「谁引用这个目录件」的完整答案是范围内各库的并集（core 侧同理，那是
    /// `DB_MDB::findElements` 逐库迭代的原因），单库入口不代表全局答案。
    ///
    /// 空结果 = 这个库里没有这样的引用者；读不动会 `Err`，不会退化成空。
    pub fn referrers_of(
        &self,
        dbnum: i32,
        attribute: &str,
        target: RefU64,
    ) -> Result<Vec<RefU64>, DirectStoreError> {
        let session = self.session_of(dbnum)?;
        let mut session = session
            .lock()
            .map_err(|_| DirectStoreError::Poisoned { slot: "session" })?;
        let found = session
            .engine
            .referrers_of(e3d_attlib::db1_hash(attribute), to_refno(target))
            .map_err(|error| DirectStoreError::Session {
                dbnum,
                detail: format!("反向引用表 {attribute}：{error}"),
            })?;
        Ok(found
            .into_iter()
            .map(|refno| RefU64((u64::from(refno.word0) << 32) | u64::from(refno.word1)))
            .collect())
    }

    /// 这个库在当前时点声明了哪几张具名表。
    ///
    /// 名字是 base-27 反哈希出来的，解不出的照原样报哈希——目录里的名字不全是引用
    /// 属性（`NAME` / `TYPEX` / `SPAMAP` 也在里面），哪几张算引用表由字典裁决，
    /// 不由本方法猜。
    pub fn table_names(&self, dbnum: i32) -> Result<Vec<String>, DirectStoreError> {
        let session = self.session_of(dbnum)?;
        let session = session
            .lock()
            .map_err(|_| DirectStoreError::Poisoned { slot: "session" })?;
        Ok(session
            .engine
            .tables()
            .iter()
            .map(|table| table.name().unwrap_or_else(|| table.name_hash.to_string()))
            .collect())
    }

    fn read_attrs(&self, dbnum: i32, refno: RefU64) -> Result<DirectAttrs, DirectStoreError> {
        let session = self.session_of(dbnum)?;
        let mut session = session
            .lock()
            .map_err(|_| DirectStoreError::Poisoned { slot: "session" })?;
        let extraction = self.extract(&mut session, dbnum, refno)?;

        direct_attmap::to_named_attmap(&extraction).map_err(|source| DirectStoreError::Convert {
            refno: refno.to_string(),
            source,
        })
    }

    /// 整条记录。成员表只在这一层出现——描述符抽取（[`DirectStore::extract`]）不带成员。
    fn parse(
        &self,
        session: &mut DbSession,
        dbnum: i32,
        refno: RefU64,
    ) -> Result<ParsedElement, DirectStoreError> {
        let target = to_refno(refno);
        let sesno = session.pin.sesno;

        let view = session
            .engine
            .find_element(target)
            .map_err(|error| DirectStoreError::Extract {
                refno: refno.to_string(),
                detail: error.to_string(),
            })?
            .ok_or(DirectStoreError::NoSuchElement {
                refno: refno.to_string(),
                dbnum,
                sesno,
            })?;

        ParsedElement::parse(&view.raw_bytes).map_err(|detail| DirectStoreError::Extract {
            refno: refno.to_string(),
            detail,
        })
    }

    fn extract(
        &self,
        session: &mut DbSession,
        dbnum: i32,
        refno: RefU64,
    ) -> Result<e3d_io::record::descriptor::ElementExtraction, DirectStoreError> {
        let target = to_refno(refno);
        let DbSession {
            engine,
            provider,
            pin,
            ..
        } = session;

        engine
            .extract_element_with_descriptors(target, &self.schema.attlib, provider)
            .map_err(|error| {
                // e3d-io 把「索引里没有这个 refno」也走 ResolveError，但那是「不存在」，
                // 与「读坏了」是两件事，调用方要能分开处理。
                let text = error.to_string();
                if text.contains("not found") || text.contains("no element") {
                    DirectStoreError::NoSuchElement {
                        refno: refno.to_string(),
                        dbnum,
                        sesno: pin.sesno,
                    }
                } else {
                    DirectStoreError::Extract {
                        refno: refno.to_string(),
                        detail: text,
                    }
                }
            })
    }

    /// 拿到（必要时打开）一个库的会话。
    ///
    /// 没登记过的库从定位器补 pin（时点 = 开库那一刻的最新，ADR-054）；定位器也不认识
    /// 的库照样 `NoFileForDbnum`。点名库的 `*_in` 入口与靠 ref0 反查的入口因此同一口径。
    ///
    /// 打开动作在 DashMap 之外完成，插入时只碰 `sessions`——见模块头「并发」一段。
    /// 两个线程同时开同一个库时会各开一次，后到的那份直接丢掉：多读几页，换不会
    /// 在文件 I/O 期间攥着分片锁。
    fn session_of(&self, dbnum: i32) -> Result<Arc<Mutex<DbSession>>, DirectStoreError> {
        if let Some(hit) = self.sessions.get(&dbnum) {
            return Ok(hit.clone());
        }
        let pin = match self.pinned(dbnum) {
            Some(pin) => pin,
            None => self.pin_from_locator(dbnum)?,
        };

        let opened = Arc::new(Mutex::new(self.open_session(&pin)?));
        Ok(self.sessions.entry(dbnum).or_insert(opened).clone())
    }

    fn open_session(&self, pin: &DbPin) -> Result<DbSession, DirectStoreError> {
        let fail = |detail: String| DirectStoreError::Session {
            dbnum: pin.dbnum,
            detail,
        };

        let identity = FileIdentity::of(&pin.file)
            .map_err(|error| fail(format!("stat {}: {error}", pin.file.display())))?;

        let engine = match pin.sesno {
            Some(sesno) => ReadOnlyEngine::open_at(&pin.file, sesno),
            None => ReadOnlyEngine::open(&pin.file),
        }
        .map_err(|error| {
            fail(format!(
                "open {} at {:?}: {error}",
                pin.file.display(),
                pin.sesno
            ))
        })?;

        // 文件自证（不变量 3）：描述符页自报的库号必须就是要开的那个。定位器把 dbnum 绑到
        // 错的文件、或路径上的文件被换成别的库，都在这里响亮地停下——与 e3d-io
        // `DbSet::db_slot` 的 `DbNumberMismatch` 同一条门。
        let actual = engine.descriptor().db_mark as i32;
        if actual != pin.dbnum {
            return Err(DirectStoreError::DbNumberMismatch {
                dbnum: pin.dbnum,
                actual,
                file: pin.file.clone(),
            });
        }

        // 冻结点。`pin.sesno` 给了就是它；没给就是**此刻**文件自报的最新，解一次之后
        // 整个 store 生命周期内不再重解——见 `DbPin::sesno` 的注释。
        let pinned_sesno = engine.session().sesno;

        let template_file = self
            .schema
            .template_file(&pin.db_type)
            .map_err(|error| fail(format!("{error:#}")))?;
        let provider =
            TemplateProvider::open(&template_file, Some(e3d_attlib::db1_hash(&pin.db_type)))
                .map_err(|error| fail(format!("open {}: {error}", template_file.display())))?;

        Ok(DbSession {
            engine,
            provider,
            pin: pin.clone(),
            pinned_sesno,
            identity,
        })
    }

    /// 「问路」版的 [`DirectStore::dbnum_of`]：解不出、或解出的库谁都不认识，给 `None` 而不是错。
    ///
    /// 扫一个元素身上的引用属性时，指向本工程之外的引用是正常的，不该把整轮扫描打断。
    /// `Result` 外壳是历史签名（反查表时代定位器可能报歧义），现在没有失败路径，留着不改调用方。
    pub fn locator_dbnum(&self, refno: RefU64) -> anyhow::Result<Option<i32>> {
        Ok(self.dbnum_hint(refno))
    }

    /// 缓存失效用的粗筛：这个 refno 解出来、且认识的库。解不出或不认识就别动它。
    fn dbnum_hint(&self, refno: RefU64) -> Option<i32> {
        refno_dbnum(refno)
            .filter(|dbnum| self.knows_dbnum(*dbnum))
            .map(|dbnum| dbnum as i32)
    }
}

impl std::fmt::Debug for DirectStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pins: BTreeMap<i32, String> = self
            .pins
            .iter()
            .map(|entry| {
                (
                    *entry.key(),
                    format!("{}@{:?}", entry.value().db_type, entry.value().sesno),
                )
            })
            .collect();
        f.debug_struct("DirectStore")
            .field("pins", &pins)
            .field("open_sessions", &self.sessions.len())
            .field("cached_elements", &self.attrs.len())
            .finish()
    }
}

/// 从 `dbnum_watermark` 读各库的文件与 `applied_sesno`，**显式**钉到已应用水位。
///
/// 这是对拍探针（`direct_attmap_probe`）与 DB 模式读同一逻辑时点的工具，不是生成的
/// 默认时点来源：当前投影按 ADR-054 取文件最新会话（`model_source`），且库文件的权威
/// 是 MDB 成员而不是这张表。这里读的只是元数据（文件在哪、时点是几），不是元素数据。
pub async fn pins_from_watermark() -> anyhow::Result<Vec<DbPin>> {
    #[derive(serde::Deserialize)]
    struct Row {
        id: surrealdb::sql::Thing,
        db_type: Option<String>,
        file_path: Option<String>,
        applied_sesno: Option<i32>,
    }

    let mut response = aios_core::SUL_DB
        .query("SELECT id, db_type, file_path, applied_sesno FROM dbnum_watermark ORDER BY id;")
        .await?;
    let rows: Vec<Row> = response.take(0)?;

    let mut out = Vec::new();
    for row in rows {
        let dbnum: i32 = match row.id.id.to_raw().parse() {
            Ok(dbnum) => dbnum,
            // 水位行的 id 就是 dbnum。解析不出来的行不是「跳过」，是这张表出了状况。
            Err(error) => anyhow::bail!("dbnum_watermark id {:?} is not a dbnum: {error}", row.id),
        };
        let Some(file_path) = row.file_path.filter(|path| !path.is_empty()) else {
            // 按需解析库（CATA）在水位表里只有占位行，没有文件路径。它不是 DESI 的
            // 时点语义，交给调用方按 `cata_closure` 那条路走，这里不硬凑一个。
            continue;
        };
        out.push(DbPin {
            dbnum,
            db_type: row.db_type.unwrap_or_default(),
            file: PathBuf::from(file_path),
            sesno: row.applied_sesno.filter(|s| *s > 0).map(|s| s as u32),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeLocator {
        ref0_to_dbnum: BTreeMap<u32, u32>,
        lookups: AtomicUsize,
    }

    impl CataDbLocator for FakeLocator {
        fn dbnum_of_ref0(&self, ref0: u32) -> Option<u32> {
            self.lookups.fetch_add(1, Ordering::Relaxed);
            self.ref0_to_dbnum.get(&ref0).copied()
        }

        fn db_type_of(&self, _dbnum: u32) -> Option<String> {
            Some("DESI".to_string())
        }

        fn file_of(&self, _dbnum: u32) -> Option<(String, PathBuf)> {
            None
        }
    }

    fn store_without_files() -> DirectStore {
        let schema = Arc::new(DirectSchema {
            attlib: e3d_attlib::AttlibData::default(),
            template_dir: PathBuf::from("nowhere"),
            version: None,
        });
        let locator = FakeLocator {
            ref0_to_dbnum: BTreeMap::from([(24_384, 8000)]),
            lookups: AtomicUsize::new(0),
        };
        DirectStore::new(schema, Arc::new(locator))
    }

    /// 同上，但把定位器的把手也交出来——要数它被问了几次 ref0。
    fn store_with_counting_locator() -> (DirectStore, Arc<FakeLocator>) {
        let schema = Arc::new(DirectSchema {
            attlib: e3d_attlib::AttlibData::default(),
            template_dir: PathBuf::from("nowhere"),
            version: None,
        });
        let locator = Arc::new(FakeLocator {
            ref0_to_dbnum: BTreeMap::from([(24_384, 8000)]),
            lookups: AtomicUsize::new(0),
        });
        (DirectStore::new(schema, locator.clone()), locator)
    }

    fn ref0_lookups(locator: &FakeLocator) -> usize {
        locator.lookups.load(Ordering::Relaxed)
    }

    /// **改成「拿 get_0() 当 dbnum」会让这条红。** `99999/7` 的 word0 是 99999，解码出的
    /// dbnum 是 1695（低 13 位 0x69F；高位段为 0）——两者不等，且谁也不认识 1695。
    /// 定位器的 ref0 反查**一次都不许被问**：那张表已经不是路由依据。
    #[test]
    fn a_valid_refno_nobody_knows_is_not_in_mdb_and_never_consults_the_ref0_table() {
        let (store, locator) = store_with_counting_locator();
        let unknown = RefU64::from_two_nums(99_999, 7);
        let error = store.attrs(unknown).unwrap_err();
        assert!(
            matches!(
                &error,
                DirectStoreError::DbNotInMdb { dbnum: 1_695, refno } if refno == &unknown.to_string()
            ),
            "{error}"
        );
        let text = error.to_string();
        assert!(
            text.contains("dbnum 1695") && text.contains(&unknown.to_string()),
            "错误要同时说出 refno 与解出的 dbnum: {text}"
        );
        assert_eq!(ref0_lookups(&locator), 0, "dbnum_of 不得再问 ref0 反查表");
    }

    /// 非法引用（word1 符号位置位 / 解出 dbnum 0）在解码那一步就停，报的是 `InvalidRefNo`。
    #[test]
    fn an_invalid_refno_is_rejected_before_any_lookup() {
        let store = store_without_files();
        for refno in [
            RefU64::from_two_nums(0, 0),
            RefU64::from_two_nums(24_384, 0x8000_0000),
            RefU64::from_two_nums(3 << 13, 5),
        ] {
            let error = store.attrs(refno).unwrap_err();
            assert!(
                matches!(&error, DirectStoreError::InvalidRefNo { refno: text } if text == &refno.to_string()),
                "{refno}: {error}"
            );
            assert_eq!(store.locator_dbnum(refno).unwrap(), None);
        }
    }

    /// ADR-054：定位器**不认识**的库才是错误。**改回「没钉 pin 就报错」会让下一条红。**
    ///
    /// 两条入口两种说法：点名库的 `attrs_in` 去定位器补 pin，拿不出文件报 `NoFileForDbnum`；
    /// 靠 refno 解码的 `attrs` 在解出 dbnum 8000 之后先问「认识吗」，不认识就是 `DbNotInMdb`
    /// ——`web_service` 把后者当 404「不在这个 MDB」。两条都不凑一个看着像真的。
    #[test]
    fn a_dbnum_the_locator_does_not_know_is_an_error_not_a_guess() {
        let store = store_without_files();
        // 24384 = 0x5F40：extent 2 的 dbnum 8000。
        let refno = RefU64::from_two_nums(24_384, 18_447);

        let error = store.attrs_in(8000, refno).unwrap_err();
        assert!(
            matches!(error, DirectStoreError::NoFileForDbnum { dbnum: 8000 }),
            "{error}"
        );

        let error = store.attrs(refno).unwrap_err();
        assert!(
            matches!(error, DirectStoreError::DbNotInMdb { dbnum: 8000, .. }),
            "{error}"
        );
        assert_eq!(store.locator_dbnum(refno).unwrap(), None, "问路也说不认识");
    }

    /// 钉过的库就是认识的库，哪怕定位器没有它的文件：解码出的 dbnum 直接命中 pin。
    /// 同一个库的另一个 extent（word0 + 0x2000）落到同一个 pin。
    #[test]
    fn a_pinned_dbnum_is_known_for_every_extent_without_asking_the_locator() {
        let (store, locator) = store_with_counting_locator();
        store.pin(DbPin {
            dbnum: 8000,
            db_type: "DESI".to_string(),
            file: PathBuf::from("no-such-file"),
            sesno: None,
        });
        for word0 in [16_192u32, 24_384, 32_576] {
            let refno = RefU64::from_two_nums(word0, 1);
            assert_eq!(store.dbnum_of(refno).unwrap(), 8000, "word0={word0}");
            assert_eq!(store.locator_dbnum(refno).unwrap(), Some(8000));
        }
        assert_eq!(ref0_lookups(&locator), 0);
    }

    /// 生产代码里不得再出现 ref0 反查：`resolve_ref0(` / `dbnum_of_ref0(` 只许活在测试里。
    /// （切片 §5.2 源码护栏的本文件那一条，W8 汇总成独立测试文件前先在这里守着。）
    #[test]
    fn production_code_never_consults_the_ref0_table() {
        let source = include_str!("direct_store.rs");
        let production = source
            .split_once("#[cfg(test)]\nmod tests {")
            .expect("test module marker")
            .0;
        for needle in ["resolve_ref0(", "dbnum_of_ref0(", ".get_0()"] {
            assert!(
                !production.contains(needle),
                "direct_store 生产代码不得包含 `{needle}`：refno → dbnum 只走 refno_route"
            );
        }
        assert!(
            production.contains("refno_dbnum("),
            "解码 helper 必须是唯一入口"
        );
    }

    /// ADR-054 Q1：定位器认识、却没人钉时点的库，pin 出来就是「开库那一刻的最新」
    /// （`sesno: None`），不是错误，也不去水位表找 `applied_sesno`。
    #[test]
    fn an_unpinned_dbnum_the_locator_knows_is_pinned_to_the_latest_session() {
        struct LocatorWithFile;
        impl CataDbLocator for LocatorWithFile {
            fn dbnum_of_ref0(&self, _ref0: u32) -> Option<u32> {
                Some(8000)
            }
            fn db_type_of(&self, _dbnum: u32) -> Option<String> {
                Some("DESI".to_string())
            }
            fn file_of(&self, dbnum: u32) -> Option<(String, PathBuf)> {
                (dbnum == 8000).then(|| ("proj".to_string(), PathBuf::from("ams8000")))
            }
        }
        let schema = Arc::new(DirectSchema {
            attlib: e3d_attlib::AttlibData::default(),
            template_dir: PathBuf::from("nowhere"),
            version: None,
        });
        let store = DirectStore::new(schema, Arc::new(LocatorWithFile));
        assert!(store.pinned(8000).is_none());

        // 文件并不存在，所以走到开库会报 Session；但 pin 必须先落下来，且没有时点。
        let error = store
            .attrs_in(8000, RefU64::from_two_nums(24_384, 1))
            .unwrap_err();
        assert!(
            matches!(error, DirectStoreError::Session { dbnum: 8000, .. }),
            "{error}"
        );
        let pin = store.pinned(8000).expect("定位器认识的库要被自动登记");
        assert_eq!(pin.sesno, None, "未指定时点 = 开库时解一次最新");
        assert_eq!(pin.db_type, "DESI");
        assert_eq!(pin.file, PathBuf::from("ams8000"));
    }

    #[test]
    fn a_pin_replaces_the_one_before_it_and_drops_the_open_session() {
        let store = store_without_files();
        store.pin(DbPin {
            dbnum: 8000,
            db_type: "DESI".to_string(),
            file: PathBuf::from("a"),
            sesno: Some(263),
        });
        assert_eq!(store.pinned(8000).unwrap().sesno, Some(263));

        store.pin(DbPin {
            dbnum: 8000,
            db_type: "DESI".to_string(),
            file: PathBuf::from("a"),
            sesno: Some(264),
        });
        assert_eq!(store.pinned(8000).unwrap().sesno, Some(264));
        assert_eq!(store.pin_count(), 1);
    }

    /// **并发形态的下限：多线程同时取数不 panic、不死锁。**
    ///
    /// 没有 fixture 也要跑得起来，所以打的是错误路径——它走的仍是那两张 DashMap 和
    /// 同一条 `session_of`。真正读文件的并发在 `direct_attmap_probe` 里跑。
    /// 死锁在这里表现为测试卡住而不是失败，所以线程数取得比分片数大。
    #[test]
    fn many_threads_may_ask_at_once() {
        let store = Arc::new(store_without_files());
        store.pin(DbPin {
            dbnum: 8000,
            db_type: "DESI".to_string(),
            file: PathBuf::from("no-such-file"),
            sesno: Some(264),
        });

        let mut handles = Vec::new();
        for thread in 0..32u32 {
            let store = store.clone();
            handles.push(std::thread::spawn(move || {
                for step in 0..64u32 {
                    let refno = RefU64::from_two_nums(24_384, thread * 1000 + step);
                    // 文件不存在，所以每次都是 Session 错误——要的是「不 panic、
                    // 不死锁、每次都给同一类回答」。
                    let error = store.attrs(refno).unwrap_err();
                    assert!(
                        matches!(error, DirectStoreError::Session { dbnum: 8000, .. }),
                        "{error}"
                    );
                }
            }));
        }
        for handle in handles {
            handle.join().expect("no thread panicked");
        }
        assert_eq!(
            store.cached_elements(),
            0,
            "no failure was cached as a value"
        );
    }

    /// 水位行里 CATA 那种「有行无文件」的占位行不该变成一个指向空路径的 pin。
    #[test]
    fn a_template_file_is_named_after_the_db_type() {
        let schema = DirectSchema {
            attlib: e3d_attlib::AttlibData::default(),
            template_dir: PathBuf::from("dir"),
            version: None,
        };
        // 目录里没有文件，所以只看它找的是哪一个。
        let error = schema.template_file("DESI").unwrap_err().to_string();
        assert!(error.contains("desvir.dat"), "{error}");
        let error = schema.template_file("CATA").unwrap_err().to_string();
        assert!(error.contains("catvir.dat"), "{error}");
        assert!(schema.template_file("DI").is_err());
    }

    #[test]
    fn schema_loading_diagnostics_name_success_and_full_failure_context() {
        let source = include_str!("direct_store.rs");
        for required in [
            "[E3D schema] 开始加载",
            "[E3D schema] 项目数据库二进制版本证据（仅诊断，不用于选版本）",
            "[E3D schema] 开始读取 schema pack",
            "[E3D schema] schema pack 清单读取成功",
            "[E3D schema] 开始校验 schema 文件并解析 attlib",
            "[E3D schema] 加载成功",
            "[E3D schema] 加载失败",
        ] {
            assert!(
                source.contains(required),
                "schema 加载过程缺日志 {required}"
            );
        }
        // 自动识别已废止（e3d-io 2c6f11d）：这里只准读证据，不准再调 detect_*。
        assert!(
            !source.contains(concat!("e3d_io::", "detect_project_version("))
                && !source.contains(concat!("SchemaPack::", "detect_for_project(")),
            "schema 加载不得再调 detect_*：它一律失败，报错还是一句英文 API 说明"
        );
        assert!(
            source.contains("e3d_io::database_header_diagnostics(&evidence.database)"),
            "默认 schema 启动日志必须补充 CRINFO/TMPLVN/session DAB tag"
        );
        for required in [
            "header_crinfo=",
            "header_tmplvn=",
            "session_dab_tag=",
            "header_diagnostics_error=",
        ] {
            assert!(source.contains(required), "schema 启动头诊断缺 {required}");
        }
        assert_eq!(DEFAULT_E3D_VERSION, "e3d210");
        for required in [
            "mode=default source=DbOption-unconfigured",
            "e3d_version={DEFAULT_E3D_VERSION}",
            "DbOption 未配置 e3d_version，使用默认",
            "Self::open_explicit_version(&schema_root, DEFAULT_E3D_VERSION)",
        ] {
            assert!(
                source.contains(required),
                "未配置 e3d_version 的默认路径缺 {required}"
            );
        }

        let schema = DirectSchema {
            attlib: e3d_attlib::AttlibData::default(),
            template_dir: PathBuf::from(r"C:\runtime\schema\e3d210\binaries"),
            version: Some(&e3d_io::E3D210),
        };
        let success = schema.load_success_message(
            r"mode=auto project=D:\AVEVA\Projects\E3D2.1 schema_root=C:\runtime\schema",
            37,
        );
        for required in [
            "[E3D schema] 加载成功",
            "schema=e3d210",
            "e3d=\"AVEVA Everything3D 2.1 (2.10)\"",
            r"attlib=C:\runtime\schema\e3d210\binaries\attlib.dat",
            "validated_templates=DESI,CATA,DICT",
            "elapsed_ms=37",
        ] {
            assert!(
                success.contains(required),
                "成功日志缺字段 {required}: {success}"
            );
        }

        let explicit = DirectSchema {
            version: None,
            ..schema
        }
        .load_success_message("mode=explicit", 1);
        assert!(explicit.contains("schema=explicit"), "{explicit}");
        assert!(
            explicit.contains("validated_templates=deferred（按库打开时检查）"),
            "{explicit}"
        );

        let error = anyhow::anyhow!("attlib.dat 不存在").context("读取 pack 失败");
        let failure = DirectSchema::load_failure_message(
            r"mode=auto project=D:\broken schema_root=C:\missing",
            19,
            &error,
        );
        for required in [
            "[E3D schema] 加载失败",
            r"project=D:\broken",
            r"schema_root=C:\missing",
            "elapsed_ms=19",
            "原因: 读取 pack 失败: attlib.dat 不存在",
        ] {
            assert!(
                failure.contains(required),
                "失败日志缺字段 {required}: {failure}"
            );
        }
    }

    #[test]
    fn explicit_e3d210_opens_the_bundled_2_10_schema_pack() {
        let schema = DirectSchema::open_explicit_version(&e3d_io::bundled_schema_root(), "e3d210")
            .expect("the bundled E3D 2.10 schema pack opens");
        assert_eq!(schema.version().map(|version| version.id), Some("e3d210"));
        assert!(
            schema
                .template_dir()
                .ends_with(Path::new("schema/e3d210/binaries")),
            "{}",
            schema.template_dir().display()
        );
        assert!(schema.template_file("DESI").unwrap().is_file());
        assert!(schema.template_file("CATA").unwrap().is_file());
        assert!(schema.template_file("DICT").unwrap().is_file());
    }

    #[test]
    fn explicit_e3d31_opens_the_bundled_3_1_schema_pack() {
        let schema = DirectSchema::open_explicit_version(&e3d_io::bundled_schema_root(), " E3D31 ")
            .expect("the bundled E3D 3.1 schema pack opens (id is trimmed, case-insensitive)");
        assert_eq!(schema.version().map(|version| version.id), Some("e3d31"));
        assert!(
            schema
                .template_dir()
                .ends_with(Path::new("schema/e3d31/binaries")),
            "{}",
            schema.template_dir().display()
        );
        let invalid = DirectSchema::open_explicit_version(&e3d_io::bundled_schema_root(), "e3d40")
            .unwrap_err()
            .to_string();
        assert!(invalid.contains("支持 e3d210 / e3d31"), "{invalid}");
    }

    #[test]
    fn installed_e3d_2_1_bran_parses_through_the_direct_store() {
        let database = Path::new(r"D:\AVEVA\Projects\E3D2.1\AvevaMarineSample\ams000\ams8000_0001");
        if !database.is_file() {
            eprintln!("skipped: installed E3D 2.1 sample is absent");
            return;
        }

        let schema = Arc::new(
            DirectSchema::open_explicit_version(&e3d_io::bundled_schema_root(), "e3d210")
                .expect("the bundled E3D 2.10 schema pack opens"),
        );
        assert_eq!(schema.version().map(|version| version.id), Some("e3d210"));

        let locator = FakeLocator {
            ref0_to_dbnum: BTreeMap::from([(24_384, 8000)]),
            lookups: AtomicUsize::new(0),
        };
        let store = DirectStore::new(schema, Arc::new(locator));
        store.pin(DbPin {
            dbnum: 8000,
            db_type: "DESI".to_string(),
            file: database.to_path_buf(),
            sesno: None,
        });
        let attrs = store
            .attrs_in(8000, RefU64::from_two_nums(24_384, 23_257))
            .expect("BRAN 24384/23257 parses with the selected 2.10 descriptors");
        assert!(attrs.map.map.contains_key("TYPE"));
        assert!(attrs.map.map.contains_key("OWNER"));
        assert!(attrs.map.map.contains_key("REFNO"));
        assert!(store.pinned_sesno(8000).is_some());
    }
}

/// 真库门（手动）：跨库引用读通与文件自证，见该文件头。
#[cfg(test)]
#[path = "direct_store_refno_live_tests.rs"]
mod refno_live_tests;
