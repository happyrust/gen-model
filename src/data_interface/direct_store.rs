//! ADR-053 direct 模式的取数底座：不经 SurrealDB，直接从 db 文件取元素属性。
//!
//! 三件事各有一处权威，别处不得再有第二份：
//!
//! * **读哪个时点**——按 dbnum 钉在该库的 `applied_sesno` 上（ADR-053 Q3）。索引是
//!   copy-on-write，会话页各自携带当时的索引根，所以「按会话号打开」不是事后过滤，
//!   而是选哪棵树下降；`ReadOnlyEngine::open_at` 就是这一步。钉住它，direct 与 DB 两
//!   条路读的是同一个逻辑时点，对拍才有意义。**水位是承诺**：`applied_sesno` 说数据
//!   落库了，direct 读的就是那一刻的文件态。
//! * **ref0 属于哪个库**——走 [`CataDbLocator`] 反查。`Ref0` 不是 `dbnum`，
//!   `RefU64::get_0()` 顶不了它；反查不到就报错，不凑一个看着像真的。跨库 owner 上溯
//!   （DESI 元素的 owner 在 SITE 库）全靠这一步。
//! * **属性长什么形状**——[`super::direct_attmap`]，它查 DB 读侧的同一张 schema。
//!
//! 没钉水位的 dbnum、反查不到的 ref0、打不开的文件，一律 `Err` 并说出是哪个。
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
use e3d_io::ReadOnlyEngine;
use e3d_io::record::element::ParsedElement;
use e3d_io::record::template::TemplateProvider;
use e3d_io::refno::RefNo;

use super::cata_closure::CataDbLocator;
use super::direct_attmap::{self, DirectAttrs};
use super::direct_index::{DbIndexes, IndexFingerprint};

/// 模板与字典文件所在目录。
///
/// 这些是从 E3D 安装里抽出来的 schema 资产（`attlib.dat` 与各 `*vir.dat`），不随工程
/// 走、也不在本仓里，所以只能由环境指定。`DbOption` 的配置键属于 ADR-053 的 P2/P4
/// （`model_gen_mode` 一起进），本阶段先用环境变量 + 一个本机默认值。
pub const TEMPLATE_DIR_ENV: &str = "AIOS_E3D_TEMPLATE_DIR";
const TEMPLATE_DIR_DEFAULT: &str = r"E:\reverse\e3d\shadow_e3d31_aps_all";

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
    /// 冻结点解出来之后可以从 [`DirectStore::pinned_sesno`] 读回。水位库（DESI）一律
    /// 带 `Some(applied_sesno)`，不走这条。
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
    /// 没登记文件与时点。**不回落到「读文件最新会话」**：那读的是另一个时点，
    /// 与 DB 模式分叉，而分叉出来的属性看上去完全正常。
    NotPinned {
        dbnum: i32,
    },

    UnresolvedRef0 {
        ref0: u32,
        refno: String,
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
            Self::NotPinned { dbnum } => write!(
                f,
                "dbnum {dbnum} 没有登记文件与 applied_sesno，direct 读不了它"
            ),
            Self::UnresolvedRef0 { ref0, refno } => {
                write!(f, "ref0 {ref0} 反查不到 dbnum，{refno} 未解析")
            }
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
    /// 从 [`TEMPLATE_DIR_ENV`] 指定的目录加载，未设时用本机默认值。
    pub fn open_from_env() -> anyhow::Result<Self> {
        let dir = std::env::var_os(TEMPLATE_DIR_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(TEMPLATE_DIR_DEFAULT));
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

    /// `ref0` → dbnum，走定位器反查。**不是** `RefU64::get_0()`。
    pub fn dbnum_of(&self, refno: RefU64) -> Result<i32, DirectStoreError> {
        let ref0 = refno.get_0();
        match self.locator.resolve_ref0(ref0) {
            Ok(Some(dbnum)) => Ok(dbnum as i32),
            Ok(None) => Err(DirectStoreError::UnresolvedRef0 {
                ref0,
                refno: refno.to_string(),
            }),
            // 归属歧义（同一个 ref0 落在多个库）在定位器那里就是错误，这里照样上浮：
            // 挑一个「看着像的」库读出来的属性，比读不出来更难查。
            Err(error) => Err(DirectStoreError::Session {
                dbnum: 0,
                detail: format!("resolving ref0 {ref0} for {refno}: {error:#}"),
            }),
        }
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

        let fingerprint = IndexFingerprint::of(&pin.file, dbnum, *pinned_sesno).map_err(
            |error| DirectStoreError::Session {
                dbnum,
                detail: format!("stat {}: {error}", pin.file.display()),
            },
        )?;
        let built = DbIndexes::load_or_build(engine, provider, &self.schema.attlib, fingerprint)
            .map_err(|error| DirectStoreError::Session {
                dbnum,
                detail: format!("building derived indexes: {error:#}"),
            })?;
        drop(session);

        let fresh = Arc::new(built);
        Ok(self.indexes.entry(dbnum).or_insert(fresh).clone())
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
    /// 打开动作在 DashMap 之外完成，插入时只碰 `sessions`——见模块头「并发」一段。
    /// 两个线程同时开同一个库时会各开一次，后到的那份直接丢掉：多读几页，换不会
    /// 在文件 I/O 期间攥着分片锁。
    fn session_of(&self, dbnum: i32) -> Result<Arc<Mutex<DbSession>>, DirectStoreError> {
        if let Some(hit) = self.sessions.get(&dbnum) {
            return Ok(hit.clone());
        }
        let pin = self
            .pins
            .get(&dbnum)
            .map(|entry| entry.clone())
            .ok_or(DirectStoreError::NotPinned { dbnum })?;

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
        Ok(self
            .locator
            .resolve_ref0(refno.get_0())?
            .map(|dbnum| dbnum as i32))
    }

    /// 缓存失效用的粗筛：这个 refno 的 ref0 反查得到的库。反查不出来就别动它。
    fn dbnum_hint(&self, refno: RefU64) -> Option<i32> {
        self.locator
            .resolve_ref0(refno.get_0())
            .ok()
            .flatten()
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

/// 从 `dbnum_watermark` 读各库的文件与 `applied_sesno`。
///
/// 这是 direct 模式**唯一**还要连 SurrealDB 的地方，而且读的是元数据（文件在哪、
/// 时点是几），不是元素数据——ADR-053 Q1 的范围就画在这里。
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
        });
        let locator = FakeLocator {
            ref0_to_dbnum: BTreeMap::from([(24_384, 8000)]),
            lookups: AtomicUsize::new(0),
        };
        DirectStore::new(schema, Arc::new(locator))
    }

    /// **改成「反查不到就拿 get_0() 当 dbnum」会让这条红。**
    /// `Ref0` 不是 `dbnum`，两者恰好相等只是巧合。
    #[test]
    fn an_unresolvable_ref0_is_an_error_not_a_guess() {
        let store = store_without_files();
        let unknown = RefU64::from_two_nums(99_999, 7);
        let error = store.attrs(unknown).unwrap_err();
        assert!(
            matches!(error, DirectStoreError::UnresolvedRef0 { ref0: 99_999, .. }),
            "{error}"
        );
        assert!(!error.to_string().contains("99999_7 read from dbnum"));
    }

    /// **改成「没钉水位就读文件最新会话」会让这条红。** 那读到的是另一个时点，
    /// 与 DB 模式分叉，而分叉出来的属性看上去完全正常。
    ///
    /// 两条入口都要堵：点名库的 `attrs_in` 直接说没登记；靠 ref0 反查的 `attrs` 会先去
    /// 定位器补 pin（目录库不入水位表，只能这么来），定位器也拿不出文件时同样报错。
    #[test]
    fn a_dbnum_with_no_pin_is_an_error_not_the_newest_session() {
        let store = store_without_files();
        let refno = RefU64::from_two_nums(24_384, 18_447);

        let error = store.attrs_in(8000, refno).unwrap_err();
        assert!(
            matches!(error, DirectStoreError::NotPinned { dbnum: 8000 }),
            "{error}"
        );

        let error = store.attrs(refno).unwrap_err();
        assert!(
            matches!(error, DirectStoreError::NoFileForDbnum { dbnum: 8000 }),
            "{error}"
        );
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
        };
        // 目录里没有文件，所以只看它找的是哪一个。
        let error = schema.template_file("DESI").unwrap_err().to_string();
        assert!(error.contains("desvir.dat"), "{error}");
        let error = schema.template_file("CATA").unwrap_err().to_string();
        assert!(error.contains("catvir.dat"), "{error}");
        assert!(schema.template_file("DI").is_err());
    }
}
