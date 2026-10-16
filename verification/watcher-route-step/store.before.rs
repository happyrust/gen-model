//! ADR-053 direct 模式的取数底座：不经 SurrealDB，直接从 db 文件取元素属性。
//!
//! 三件事各有一处权威，别处不得再有第二份：
//!
//! * **读哪个时点**——调用方显式钉了就是它，没钉就是开库那一刻文件自报的最新会话
//!   （ADR-054：未指定时点一律取文件最新；`applied_sesno` 是摄入水位，不是时点来源）。
//!   索引是 copy-on-write，会话页各自携带当时的索引根，所以「按会话号打开」不是事后
//!   过滤，而是选哪棵树下降；`ReadOnlyEngine::open_at` 就是这一步。要与 DB 模式对拍
//!   读同一逻辑时点，用 [`pins_from_watermark`] 显式钉到 `applied_sesno`。
//! * **ref0 属于哪个库**——走 [`CataDbLocator`] 反查。`Ref0` 不是 `dbnum`，
//!   `RefU64::get_0()` 顶不了它；反查不到就报错，不凑一个看着像真的。跨库 owner 上溯
//!   （DESI 元素的 owner 在 SITE 库）全靠这一步。
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

use aios_core::{NamedAttrMap, RefU64};
use dashmap::DashMap;
use e3d_io::record::element::ParsedElement;
use e3d_io::record::explicit::ExplicitAttribute;
use e3d_io::record::template::TemplateProvider;
use e3d_io::refno::RefNo;
use e3d_io::ReadOnlyEngine;

use super::cata_closure::CataDbLocator;
use super::direct_attmap::{self, DirectAttrs};
use super::direct_index::{DbIndexes, IndexFingerprint};

/// 旧版显式模板目录环境变量。
///
/// 共享加载器不再读取它：版本必须由 `DbOption.toml.e3d_version` 明确选择。
/// 保留常量只是给仍显式调用 [`DirectSchema::open`] 的工具提供一致的错误提示。
pub const TEMPLATE_DIR_ENV: &str = "AIOS_E3D_TEMPLATE_DIR";

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
struct FileIdentity(super::frozen_source::StableFileId);

impl FileIdentity {
    fn of(path: &Path) -> std::io::Result<Self> {
        // Query an open handle: Windows metadata(path) need not contain file IDs.
        let file = std::fs::File::open(path)?;
        super::frozen_source::StableFileId::from_metadata(&file.metadata()?)
            .map(Self)
            .map_err(std::io::Error::other)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectStoreError {
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
}

impl std::fmt::Debug for DirectSchema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectSchema")
            .field("template_dir", &self.template_dir)
            .finish_non_exhaustive()
    }
}

impl DirectSchema {
    /// 只接受 DbOption.toml 的显式版本。
    ///
    /// 已调查的 `UPGDNO`、`Saving Meta Data.` 与 `COMM.VERS` 都不是持久产品版本
    /// 字段；缺配置就失败，不能从路径或会话现场猜一套偏移继续解析。
    pub fn open_from_env() -> anyhow::Result<Self> {
        let version = crate::options::configured_e3d_version()
            .map_err(anyhow::Error::msg)?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "DbOption.toml 必须显式配置 e3d_version = \"e3d210\" 或 \"e3d31\"；\
                     自动识别已禁用，因为尚未证实持久的数据库产品版本字段"
                )
            })?;
        Self::open_version(&version)
    }

    /// 直接打开该版本的整套模板，不读取工程或数据库进行版本识别。
    fn open_version(version: &str) -> anyhow::Result<Self> {
        let version = e3d_io::schema::version_by_id(version.trim())
            .ok_or_else(|| anyhow::anyhow!("e3d_version={version:?} 无效：支持 e3d210 / e3d31"))?;
        let schema = Self::open_pack(version)?;
        log::info!(
            "[E3D schema] source=DbOption evidence_field=manual raw_value=- raw_text=- \
             e3d_version={} auto_detect=false template_dir={}",
            version.id,
            schema.template_dir.display()
        );
        Ok(schema)
    }

    fn open_pack(version: &'static e3d_io::E3dVersion) -> anyhow::Result<Self> {
        let pack = e3d_io::SchemaPack::open_version(e3d_io::bundled_schema_root(), version.id)?;
        let attlib = pack.attlib()?;
        let dir = attlib
            .parent()
            .ok_or_else(|| anyhow::anyhow!("attlib 缺少父目录"))?;
        Self::open(dir)
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
        })
    }

    pub fn template_dir(&self) -> &Path {
        &self.template_dir
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

#[derive(Default)]
struct DbSlot {
    // None: legacy locator; Some(None): removed; Some(Some(pin)): current MDB route.
    route: Option<Option<DbPin>>,
    generation: u64,
    pin: Option<DbPin>,
    session: Option<Arc<Mutex<DbSession>>>,
    attrs: BTreeMap<RefU64, Arc<DirectAttrs>>,
    indexes: Option<Arc<DbIndexes>>,
}

/// 按 dbnum 池化的 direct 取数入口。
pub struct DirectStore {
    schema: Arc<DirectSchema>,
    locator: Arc<dyn CataDbLocator + Send + Sync>,
    slots: DashMap<i32, Arc<Mutex<DbSlot>>>,
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
            slots: DashMap::new(),
        }
    }

    /// 登记一个库的文件与时点。重复登记以最后一次为准，并丢掉该库已开的会话
    /// ——换了时点还接着用旧引擎，读到的就是上一个时点的树。
    fn get_slot(&self, dbnum: i32) -> Arc<Mutex<DbSlot>> {
        self.slots.entry(dbnum).or_default().clone()
    }

    fn slot_snapshot(&self) -> Vec<(i32, Arc<Mutex<DbSlot>>)> {
        self.slots.iter().map(|e| (*e.key(), e.value().clone())).collect()
    }

    pub fn pin(&self, pin: DbPin) {
        let dbnum = pin.dbnum;
        let slot = self.get_slot(dbnum);
        let mut state = slot.lock().expect("database slot poisoned");
        if state.pin.as_ref() != Some(&pin) {
            state.generation += 1;
            state.pin = Some(pin);
            state.session = None;
            state.indexes = None;
            state.attrs.clear();
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
    /// Atomically replace the target database route and discard its cached state.
    /// A removed route is a tombstone, never a fallback to the old locator.
    pub(crate) fn refresh_route(&self, dbnum: i32, route: Option<DbPin>) {
        let slot = self.get_slot(dbnum);
        let mut state = slot.lock().expect("database slot poisoned");
        Self::replace_route(&mut state, route);
    }

    pub(crate) fn route_generation(&self, dbnum: i32) -> u64 {
        let slot = self.get_slot(dbnum);
        let generation = slot.lock().expect("database slot poisoned").generation;
        generation
    }

    /// Watch observations carry the generation they observed. A delayed event
    /// must not erase a newer route or remove a database that has been restored.
    pub(crate) fn refresh_route_if_generation(
        &self, dbnum: i32, expected: u64, route: Option<DbPin>,
    ) -> bool {
        let slot = self.get_slot(dbnum);
        let mut state = slot.lock().expect("database slot poisoned");
        if state.generation != expected { return false; }
        Self::replace_route(&mut state, route);
        true
    }

    fn replace_route(state: &mut DbSlot, route: Option<DbPin>) {
        state.generation = state.generation.checked_add(1).expect("database generation exhausted");
        state.route = Some(route);
        state.pin = None;
        state.session = None;
        state.attrs.clear();
        state.indexes = None;
    }

    fn route_pin(&self, dbnum: i32, state: &DbSlot) -> Result<DbPin, DirectStoreError> {
        match &state.route {
            Some(route) => route.clone().ok_or(DirectStoreError::NoFileForDbnum { dbnum }),
            None => self.resolve_pin(dbnum),
        }
    }

    fn resolve_pin(&self, dbnum: i32) -> Result<DbPin, DirectStoreError> {
        let (_, file) = self.locator.file_of(dbnum as u32)
            .ok_or(DirectStoreError::NoFileForDbnum { dbnum })?;
        Ok(DbPin { dbnum, db_type: self.locator.db_type_of(dbnum as u32)
            .unwrap_or_else(|| "CATA".into()), file, sesno: None })
    }

    pub fn pin_from_locator(&self, dbnum: i32) -> Result<DbPin, DirectStoreError> {
        let slot = self.get_slot(dbnum);
        let mut state = slot.lock().map_err(|_| DirectStoreError::Poisoned { slot: "database" })?;
        if let Some(pin) = &state.pin { return Ok(pin.clone()); }
        let pin = self.route_pin(dbnum, &state)?;
        state.pin = Some(pin.clone());
        Ok(pin)
    }

    pub fn pinned(&self, dbnum: i32) -> Option<DbPin> {
        let slot = self.slots.get(&dbnum)?.clone();
        let pin = slot.lock().ok()?.pin.clone();
        pin
    }

    fn opened_session(&self, dbnum: i32) -> Option<Arc<Mutex<DbSession>>> {
        let slot = self.slots.get(&dbnum)?.clone();
        let session = slot.lock().ok()?.session.clone();
        session
    }

    /// 实际钉住的会话号，开过库之后才有。
    ///
    /// 对拍探针要把它跟每一条比对记在一起：DB 里的 CATA 行是「当初那次按需解析跑的
    /// 时候」写的，可能旧于文件现在的版本。那种不一致是**两个模式的时点差**，不是
    /// direct 读错了——拿不到这个数，时点差只能被报成 bug。
    pub fn pinned_sesno(&self, dbnum: i32) -> Option<u32> {
        let session = self.opened_session(dbnum)?;
        let sesno = session.lock().ok()?.pinned_sesno;
        Some(sesno)
    }

    /// 文件还是不是开库时那一个。
    ///
    /// 生成单元边界上调一次。DB 侧 `verify_path_identity()` 拦的就是这件事，direct 侧
    /// 没有它，目录库被换掉时冻住的会话号会安静地指向另一个文件里的另一个会话。
    pub fn verify_file_identity(&self, dbnum: i32) -> Result<(), DirectStoreError> {
        let Some(session) = self.opened_session(dbnum) else {
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
        let open = self.opened_dbnums();
        for dbnum in open {
            self.verify_file_identity(dbnum)?;
        }
        Ok(())
    }

    pub fn pin_count(&self) -> usize {
        self.slot_snapshot().iter().filter(|(_, slot)| slot.lock().unwrap().pin.is_some()).count()
    }

    /// Drop only one database's frozen state after its file generation changes.
    pub fn invalidate_dbnum(&self, dbnum: i32) {
        let slot = self.get_slot(dbnum);
        let mut state = slot.lock().expect("database slot poisoned");
        state.generation += 1;
        state.pin = None;
        state.session = None;
        state.indexes = None;
        state.attrs.clear();
    }

    /// Successful open sessions, rather than pins that may never have been read.
    pub fn opened_dbnums(&self) -> Vec<i32> {
        let mut dbnums: Vec<_> = self.slot_snapshot().into_iter().filter_map(|(n, slot)| slot.lock().unwrap().session.is_some().then_some(n)).collect();
        dbnums.sort_unstable();
        dbnums
    }

    pub fn cached_elements(&self) -> usize {
        self.slot_snapshot().iter().map(|(_, slot)| slot.lock().unwrap().attrs.len()).sum()
    }

    /// `ref0` → dbnum，走定位器反查。**不是** `RefU64::get_0()`。
    pub fn dbnum_of(&self, refno: RefU64) -> Result<i32, DirectStoreError> {
        let dbnum = crate::data_interface::model_source::dbnum_from_word0(refno.get_0());
        i32::try_from(dbnum).map_err(|_| DirectStoreError::Session { dbnum: 0, detail: format!("dbnum overflow for {refno}") })
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
        let slot = self.get_slot(dbnum);
        let mut state = slot.lock().map_err(|_| DirectStoreError::Poisoned { slot: "database" })?;
        if let Some(hit) = state.attrs.get(&refno) { return Ok(hit.clone()); }
        let session = self.session_in_slot(dbnum, &mut state)?;
        let mut session = session.lock().map_err(|_| DirectStoreError::Poisoned { slot: "session" })?;
        let fresh = Arc::new(self.read_attrs(&mut session, dbnum, refno)?);
        state.attrs.insert(refno, fresh.clone());
        Ok(fresh)
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
        let slot = self.get_slot(dbnum);
        let mut state = slot.lock().map_err(|_| DirectStoreError::Poisoned { slot: "database" })?;
        if let Some(hit) = &state.indexes { return Ok(hit.clone()); }
        let session = self.session_in_slot(dbnum, &mut state)?;
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
        state.indexes = Some(fresh.clone());
        Ok(fresh)
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
            .referrers_of(
                e3d_attlib::db1_hash(attribute),
                RefNo::new(target.get_0(), target.get_1()),
            )
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

    fn read_attrs(&self, session: &mut DbSession, dbnum: i32, refno: RefU64) -> Result<DirectAttrs, DirectStoreError> {
        let extraction = self.extract(session, dbnum, refno)?;

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
        let target = RefNo::new(refno.get_0(), refno.get_1());
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
        let target = RefNo::new(refno.get_0(), refno.get_1());
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
    /// Clone the stable slot before locking; no DashMap shard is held during I/O.
    /// Pin changes and session publication share this lock, so one generation
    /// can open only once and an old opener cannot publish after invalidation.
    fn session_of(&self, dbnum: i32) -> Result<Arc<Mutex<DbSession>>, DirectStoreError> {
        let slot = self.get_slot(dbnum);
        let mut state = slot.lock().map_err(|_| DirectStoreError::Poisoned { slot: "database" })?;
        self.session_in_slot(dbnum, &mut state)
    }

    // Caller owns the per-db slot lock; never acquire it recursively.
    fn session_in_slot(&self, dbnum: i32, state: &mut DbSlot) -> Result<Arc<Mutex<DbSession>>, DirectStoreError> {
        if let Some(session) = &state.session { return Ok(session.clone()); }
        let pin = match &state.pin {
            Some(pin) => pin.clone(),
            None => {
                let pin = self.route_pin(dbnum, &state)?;
                state.pin = Some(pin.clone());
                pin
            }
        };
        let session = Arc::new(Mutex::new(self.open_session(&pin)?));
        state.session = Some(session.clone());
        Ok(session)
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

    /// ref0 反查，反查不到给 `None` 而不是错。
    ///
    /// 与 [`DirectStore::dbnum_of`] 的区别是「问路」和「要读」：扫一个元素身上的引用
    /// 属性时，指向本工程之外的引用是正常的，不该把整轮扫描打断。
    pub fn locator_dbnum(&self, refno: RefU64) -> anyhow::Result<Option<i32>> {
        Ok(Some(self.dbnum_of(refno).map_err(|e| anyhow::anyhow!(e))?))
    }

    /// 缓存失效用的粗筛：这个 refno 的 ref0 反查得到的库。反查不出来就别动它。
    fn dbnum_hint(&self, refno: RefU64) -> Option<i32> {
        self.dbnum_of(refno).ok()
    }
}

impl std::fmt::Debug for DirectStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pins: BTreeMap<i32, String> = self.slot_snapshot().into_iter()
            .filter_map(|(n, slot)| slot.lock().unwrap().pin.as_ref()
                .map(|p| (n, format!("{}@{:?}", p.db_type, p.sesno))))
            .collect();
        f.debug_struct("DirectStore")
            .field("pins", &pins)
            .field("open_sessions", &self.opened_dbnums().len())
            .field("cached_elements", &self.cached_elements())
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

    #[test]
    fn explicit_e3d_version_overrides_legacy_directory_in_shared_loader() {
        const CHILD: &str = "AIOS_TEST_EXPLICIT_E3D_VERSION";
        if std::env::var_os(CHILD).is_some() {
            let schema = DirectSchema::open_from_env().unwrap();
            assert!(schema.template_dir().ends_with("e3d210/binaries"));
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("DbOption.toml");
        std::fs::write(&config, "e3d_version = \"e3d210\"\n").unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "data_interface::direct_store::tests::explicit_e3d_version_overrides_legacy_directory_in_shared_loader",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("DB_OPTION_FILE", config)
            .env("AIOS_E3D_TEMPLATE_DIR", dir.path().join("missing-templates"))
            .env_remove("E3D210_SCHEMA_DIR")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }

    #[test]
    fn explicit_e3d_version_opens_templates_without_a_database() {
        for version in ["e3d210", "e3d31"] {
            let schema = DirectSchema::open_version(version).unwrap();
            let template = schema.template_file("DESI").unwrap();
            let mut provider = TemplateProvider::open(template, None).unwrap();
            assert!(provider
                .template_for(e3d_attlib::db1_hash("BRAN"))
                .unwrap()
                .is_some());
        }
    }

    #[test]
    fn explicit_e3d_version_typo_does_not_fall_back() {
        for version in ["e3d99", "", "auto"] {
            let error = DirectSchema::open_version(version).unwrap_err().to_string();
            assert!(error.contains("e3d_version"), "{error}");
        }
    }

    #[test]
    fn missing_e3d_version_never_probes_or_falls_back() {
        const CHILD: &str = "AIOS_TEST_MISSING_E3D_VERSION";
        if std::env::var_os(CHILD).is_some() {
            let error = DirectSchema::open_from_env().unwrap_err().to_string();
            assert!(error.contains("必须显式配置 e3d_version"), "{error}");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("DbOption.toml");
        std::fs::write(
            &config,
            "project_path = \"D:/AVEVA/Projects/E3D3.1\"\n\
             included_projects = [\"AvevaMarineSample\"]\n",
        )
        .unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "data_interface::direct_store::tests::missing_e3d_version_never_probes_or_falls_back",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("DB_OPTION_FILE", config)
            .env(
                TEMPLATE_DIR_ENV,
                e3d_io::bundled_schema_root().join("e3d31/binaries"),
            )
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }

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
        });
        let locator = FakeLocator {
            ref0_to_dbnum: BTreeMap::from([(24_384, 8000)]),
            lookups: AtomicUsize::new(0),
        };
        DirectStore::new(schema, Arc::new(locator))
    }

    /// RefNo 解出的 dbnum 没有对应文件时，路由必须明确失败。
    #[test]
    fn an_unresolvable_ref0_is_an_error_not_a_guess() {
        let store = store_without_files();
        let unknown = RefU64::from_two_nums(99_999, 7);
        let error = store.attrs(unknown).unwrap_err();
        assert!(matches!(error, DirectStoreError::NoFileForDbnum { dbnum: 1695 }), "{error}");
        assert!(!error.to_string().contains("99999_7 read from dbnum"));
    }

    /// ADR-054：定位器**不认识**的库才是错误。**改回「没钉 pin 就报错」会让下一条红。**
    ///
    /// 两条入口同一口径：点名库的 `attrs_in` 与靠 ref0 反查的 `attrs` 都先去定位器补 pin，
    /// 定位器也拿不出文件时才报 `NoFileForDbnum`——不凑一个看着像真的。
    #[test]
    fn a_dbnum_the_locator_does_not_know_is_an_error_not_a_guess() {
        let store = store_without_files();
        let refno = RefU64::from_two_nums(24_384, 18_447);

        let error = store.attrs_in(8000, refno).unwrap_err();
        assert!(
            matches!(error, DirectStoreError::NoFileForDbnum { dbnum: 8000 }),
            "{error}"
        );

        let error = store.attrs(refno).unwrap_err();
        assert!(
            matches!(error, DirectStoreError::NoFileForDbnum { dbnum: 8000 }),
            "{error}"
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
        assert_eq!(store.pin_count(), 1);
        assert!(store.opened_dbnums().is_empty(), "a failed open is not an open session");
        assert_eq!(store.cached_elements(), 0);
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

    #[test]
    fn refresh_waits_for_slot_publication_and_preserves_other_database_cache() {
        use std::sync::{Barrier, mpsc};
        let store = Arc::new(store_without_files());
        let target = RefU64::from_two_nums(24384, 46);
        let other = RefU64::from_two_nums(24383, 46);
        let retained = Arc::new(DirectAttrs::default());
        store.get_slot(7999).lock().unwrap().attrs.insert(other, retained.clone());
        let acquired = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let reader = {
            let store = store.clone(); let acquired = acquired.clone(); let release = release.clone();
            std::thread::spawn(move || {
                let slot = store.get_slot(8000);
                let mut state = slot.lock().unwrap();
                acquired.wait();
                release.wait();
                state.attrs.insert(target, Arc::new(DirectAttrs::default()));
            })
        };
        acquired.wait();
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let refresher = {
            let store = store.clone();
            std::thread::spawn(move || {
                started_tx.send(()).unwrap();
                store.invalidate_dbnum(8000);
                done_tx.send(()).unwrap();
            })
        };
        started_rx.recv().unwrap();
        assert!(matches!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release.wait();
        reader.join().unwrap();
        refresher.join().unwrap();
        done_rx.recv().unwrap();
        assert!(store.attrs_in(8000, target).is_err(), "old published value must be gone");
        assert!(Arc::ptr_eq(&retained, &store.attrs_in(7999, other).unwrap()));
        assert_eq!(store.cached_elements(), 1);
    }

    #[test]
    fn route_replacement_removal_and_restore_never_fall_back_to_old_locator() {
        let store = store_without_files();
        let slot = store.get_slot(8000);
        let route = |path: &str| DbPin { dbnum: 8000, db_type: "DESI".into(), file: path.into(), sesno: None };
        store.refresh_route(8000, Some(route("new-project/new.db")));
        assert_eq!(store.pin_from_locator(8000).unwrap().file, PathBuf::from("new-project/new.db"));
        store.invalidate_dbnum(8000);
        assert_eq!(store.pin_from_locator(8000).unwrap().file, PathBuf::from("new-project/new.db"));
        store.refresh_route(8000, None);
        assert!(matches!(store.pin_from_locator(8000), Err(DirectStoreError::NoFileForDbnum { dbnum: 8000 })));
        store.refresh_route(8000, Some(route("restored.db")));
        assert_eq!(store.pin_from_locator(8000).unwrap().file, PathBuf::from("restored.db"));
        assert!(Arc::ptr_eq(&slot, &store.get_slot(8000)));
        assert!(store.opened_dbnums().is_empty());
    }

    #[test]
    fn stale_refresh_event_cannot_replace_new_route_or_clear_other_slot() {
        let store = store_without_files();
        let route = |path: &str| DbPin { dbnum: 8000, db_type: "DESI".into(), file: path.into(), sesno: None };
        let first = store.route_generation(8000);
        assert!(store.refresh_route_if_generation(8000, first, Some(route("new.db"))));
        let current = store.route_generation(8000);
        assert_eq!(current, first + 1);
        assert!(!store.refresh_route_if_generation(8000, first, Some(route("old.db"))));
        assert!(!store.refresh_route_if_generation(8000, first, None));
        assert_eq!(store.route_generation(8000), current);
        assert_eq!(store.pin_from_locator(8000).unwrap().file, PathBuf::from("new.db"));
        assert_eq!(store.route_generation(7999), 0);
        assert!(store.refresh_route_if_generation(8000, current, None));
        assert!(store.pin_from_locator(8000).is_err());
    }

    #[test]
    fn dbslot_remains_stable_across_pin_and_invalidation() {
        let store = store_without_files();
        let first = store.get_slot(8000);
        store.pin(DbPin { dbnum: 8000, db_type: "DESI".into(), file: "first".into(), sesno: Some(1) });
        let generation = first.lock().unwrap().generation;
        store.invalidate_dbnum(8000);
        assert!(Arc::ptr_eq(&first, &store.get_slot(8000)));
        assert!(first.lock().unwrap().generation > generation);
        assert!(store.pinned(8000).is_none());
    }

    #[test]
    fn targeted_invalidation_keeps_unrelated_database_pins() {
        let store = store_without_files();
        for dbnum in [7999, 8000] {
            store.pin(DbPin { dbnum, db_type: "DESI".into(), file: PathBuf::from(format!("{dbnum}")), sesno: Some(1) });
        }
        store.invalidate_dbnum(8000);
        assert!(store.pinned(8000).is_none());
        assert_eq!(store.pinned(7999).unwrap().sesno, Some(1));
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
        };
        // 目录里没有文件，所以只看它找的是哪一个。
        let error = schema.template_file("DESI").unwrap_err().to_string();
        assert!(error.contains("desvir.dat"), "{error}");
        let error = schema.template_file("CATA").unwrap_err().to_string();
        assert!(error.contains("catvir.dat"), "{error}");
        assert!(schema.template_file("DI").is_err());
    }
}

#[cfg(test)]
mod stable_identity_regression {
    use super::FileIdentity;
    use std::fs::{self, File, FileTimes, OpenOptions};
    use std::io::Write;

    #[test]
    fn file_identity_accepts_append_but_detects_same_size_same_time_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.db");
        fs::write(&path, b"first").unwrap();
        let original = FileIdentity::of(&path).unwrap();
        OpenOptions::new().append(true).open(&path).unwrap().write_all(b" append").unwrap();
        assert_eq!(original, FileIdentity::of(&path).unwrap(), "SAVEWORK append preserves physical identity");
        let metadata = fs::metadata(&path).unwrap();
        let replacement = dir.path().join("replacement.db");
        fs::write(&replacement, vec![b'x'; metadata.len() as usize]).unwrap();
        File::options().write(true).open(&replacement).unwrap()
            .set_times(FileTimes::new().set_modified(metadata.modified().unwrap())).unwrap();
        fs::rename(&path, dir.path().join("original.db")).unwrap();
        fs::rename(&replacement, &path).unwrap();
        assert_ne!(original, FileIdentity::of(&path).unwrap(), "same length and mtime are not file identity");
    }
}
