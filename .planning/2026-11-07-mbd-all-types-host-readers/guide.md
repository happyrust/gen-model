# MBD 全标注类型：宿主读取器指南（2026-11-07）

plant3d-web#103 / 计划 `plant3d-web/docs/plans/2026-11-07-mbd-all-annotation-types-port-dev-plan.md` 的 M3 / M4 / M5 / M6 / M7 宿主侧。
求解器视图由 plant-mbd 各模块定义（`support::view` / `civil::view` / `equipment::view`，ADR-0015），本文件只讲**从 E3D 库怎么读**。
基于本 worktree `972b06cfa` 与 AvevaMarineSample（AMS）实测；路径缩写 V = `old/vendor/`。

## 1. 样本对象（AMS）

| 类型 | refno（db） | SITE › ZONE | 内容 |
|---|---|---|---|
| 支撑钢架 | 24383/69033（7999） | /1WCC-PIPEBJ › /1WCC-SUPP-RX | STRU /1WCC-R412.006：FRMW 下 SCTN×4（带 LNKS / LNKI）、PANE×2（一块带 NCYL×2 与 PFIT×2） |
| 吊架（与上条成对） | 24383/69027 | 同上 | REST（PURP=HANG）→ HANG 69028（HPOS / TPOS）→ PCLA 69029、HELE×2、HROD；PCLA 目录在 ZDJ 7600 |
| 吊架 | 24383/69071 | 同上 | HANG-01 下 HELE×2 + HROD，HANG-03 下 PCLA |
| 支撑 + 管上 ATTA | 24383/90901 | /1RX-ELECBJ › /1RX02-LSUP（769 个 STRU） | FRMW 下 SCTN、PNOD→PJOI；BRAN 24383/99996 上 ATTA 99998 `…E0991/DATUM`（STEX=1RX02-LSUP-E0991）与 99999 `/SREF`，**只靠名字关联** |
| 支撑 | 24384/48（8000） | /1RX03-PIPEBJ › /1RX03-MSUP | SCTN×3 无 SPRE（只画中心线）、SNOD / SCOJ / SUBJ、PJOI×2；对应 ATTA 24384/2794、2795 |
| 风管支撑 | 24384/24946 | /1CSV-HVACHB › 24384/24945 | SCTN×2 各带 FITT 端板；SUBS→EXTR→LOOP |
| 曲墙 | 24381/44303（7997） | /1RB-CIVI › /1RB-WF05 | WALL：DESP=[1500,6000]，JUSL=OBOW；SPINE = POINSP / CURVE(THRU, R=26500) / POINSP；同 CWALL 下 GWALL 44283（NXTR + PLOO） |
| 直墙 + 门 | 24381/37115 | /1RX-ARCHHB › /1RX-FN | STWALL 有 POSS / POSE，DESP=[50,1200]；门 FITT 37118 `/R162-M01`（DESP 50,900,2000…，POSL=IBOW） |
| 楼板 + 负体 | 24381/38614、38695 | /1RX-ARCHHB › /1RX-FN | 38614：PLOO + NXTR×11 + NCYL×7；38695：NBOX×3、NCYL×75、NXTR×50 |
| 1112 土建 | 17496/106424、80024、118364 | /1RS-CIVI › /1RS-WF05 | STWALL + FITT×4（穿墙件）；FLOOR：NBOX×4 + PFIT×3；GWALL：NBOX + NXTR×3 |
| 房间 | 24381/34849 | /1RX-ARCHHB › /1RX-RM | FRMW /1RX-RM03-R301 → SBFR -VOLU → PANE×2 |
| 设备 | 24383/76416（7999） | /1RCV-PIPEBJ › … | BOX×18、CYLI×8、DISH×2、NOZZ×11（都有 CATR） |
| 设备 | 24383/76360；24381/156667 | 同上；/1PTU-INST23 | NOZZ×20，CATR=0/0（无目录件，只能用 POS / ORI） |
| 设备（ELCONN） | 24384/24873、24822（8000） | /1RX03-EQUI › CASE-YK / CASE-DQ | 只有 ELCONN，没有图元 |

缺口与替代：
- 在用库里**没有 SUPPO**（只在 MDS 库 7326 / 7327 / 7333 的 APPDAR 与 `/MDS/HANGERS` 模板里），也没有 `:SUPP` UDET——支架主体用 ZONE 级 STRU + REST/HANG 成对（PML `getsuppname` 的「按 /STRU、/REST 后缀推断」分支）。
- **门没有 PURP=DOOR**（7997 的 69 个 FITT PURP 全 unset）：识别规则做成配置——SPRE→SPEC 为 `/DOOR-SPEC`，或名字匹配 `-M\d+`。由宿主判定后作为门交给求解器。
- 没有同一 STRU 里同时含 SCTN + PANE + FITT 的样本（PANE 上用的是 PFIT）；PJOI 只在 FRMW / PNOD 下；没有同时带 NOZZ 与 ELCONN 的 EQUI。
- AMS 字典**没有 `:ZD_*`、`:SUPP-ADLONGTH`**，只有 `:MDS*`、`:ItemCode`：依赖这些 UDA 的规则在 AMS 上只能验证「缺省时的行为」。

查询入口：后端只读 GET `tree/{roots,children,ancestors}`、`search?query=`、`element/attributes-at?dbnum&refno&sesno`（含 UDA）、`spatial/rooms`；探针 `noun_census_probe`、`noun_attr_dump_probe`（需 `AIOS_E3D_TEMPLATE_DIR=E:\reverse\e3d\shadow_e3d31_aps_all`；census 只认 `amsNNNN_0001`，目录里有备份文件与不在 MDB 的库）。

## 2. E3D 读 API 对照

- **元素 / 名词 / 名字 / 属主链**：`DbSet::element(&Arc, RefNo)`（V`e3d-io/src/db_element.rs`）→ `exists / element_type / stored_name / display_name / owner`；属主链照抄 `src/mbd/branch_query.rs::ancestor_name_contains`，错误归一照抄同文件 `soft()`；库根 `set.metadata(db)?.world`；refno 解析 `e3d_model_service::parse_refno`；`find_named` 全树扫，反复按名查要自建名表；UDET 显示名 `e3d_model::udet::UdetCatalog`。
- **属性**：`get_double / get_string / get_ref / get_element / get_double_array("DESP") / get_position / get_orientation` 读出的是**属主系**原值；世界矩阵 `e3d_model::transform::world_matrix`（已处理 SCTN / STWALL 上 FITT、SNOD / SJOI / PJOI、桥架件的现算标架）；带公式的属性 `get_number(name, &env)`；UDA 要求 DbSet 先 `with_uda_catalog`，否则 `UnknownAttribute`（照抄 `branch_query.rs::optional_text`）；HANG 的 HPOS / TPOS 与 BRAN head 一样乘 `world_matrix(&hang)`（`branch_query.rs` 约 231–258 行）。
- **成员**：`members()`（文件原序）、`members_of_type("PANE")`、`member_refnos()`。
- **目录图元参数**：`model_catalogue_member(&m)` 只给网格与 `part_record_repeats[i].frame`（解析参数只有 SSLC、LPYR）。要原值：`DbElementParamEnv::new(Some(m), Some(comp)).without_insulation()`，对 GMRE / NGMR 成员 `get_number("PXLE"|"PDIA"|"PHEI"|"PDIS"|"PTDI"…, &env)?.value()`；`comp` 取 `catalogue_point_set(&m)?.component`；轴 `e3d_io::record::axis_spec::direction`（`Pn` 形的轴走 PTSE）；参照私有实现 `catalogue.rs::evaluate_sbox / evaluate_catalogue_cylinder`。设计侧负体直读：NBOX `XLEN/YLEN/ZLEN`，NCYL `DIAM/HEIG`，NPYR `XBOT…HEIG`，NXTR = 环 + HEIG。
- **型材 / STWALL**：`section_catalogue → evaluate_section_profile → section_placement` 或 `spine_path`，端切 `section_cut_planes`；世界 = `world_matrix(el) * placement.local`；完整范例 `src/web_service/plines.rs::element_dto / section_frame`。STWALL 走同一条链（POSS / POSE、DESP、JUSL）。
- **PANE / FLOOR / GWALL**：`profile::loop_profile`（顶点 x、y、FRAD，高度，`sjus_drop`）；FRAD 展开 `discretise::flatten_ring`；世界放置 `elmodl::world_with_sjus`（SJUS 只对 PANE、FLOOR、GWALL 生效）。
- **曲墙 WALL**：`elmodl::wall_local_solid` 私有且只支持 POINSP-CURVE-POINSP；厚 / 高取 SPRE 截面，目录链走不通退回 DESP[厚, 高]；JUSL：OBOW / NA 在 x=0，IBOW 在 x=厚，LMIRR 镜像；SPINE 点取原始 POS，弓高 = r·(1−cos(扫角/2))。
- **P 点**：`CataloguePoints.leave(&m, Some(n))` 直接给世界坐标；`catalogue_point_set(&m)` 给局部坐标需乘世界矩阵（范例 `branch_query.rs::catalogue_ppoints`、`src/web_service/ptset.rs:169`）。NOZZ / ELCONN 从 CATR 进目录；CATR=0/0 只能用 POS / ORI。HANG 不能喂 `branch_query`（路由容器但不派生隐式管身，会被拒为 `NotRouteContainer`）。
- **EQUI 包围盒**：见 §3 SPAMAP（EQUI 本身无条目，取后代的并集），或 e3d-io `spatial::aabb::BuiltInAabb`（只支持 BOX / CYLI）。

## 3. 不用生成 AABB 树的空间候选：SPAMAP

e3d-model 没有解析式的「元素世界包围盒」helper。E3D 自己维护的 SPAMAP 表（`WITHIN VOLUME` 用的就是它）可直接读：`e3d_io::session::DbView::open(file)` → `e3d_io::index::read_spamap` → 遍历 `entries()` 或按元素 `for_refno`；`aabb_words` 是有符号 0.1 mm，用 `SpamapCodec::new(RuleSet::confirmed()).decode_words` 解码。实测（`V/e3d-io/target/release/e3d-descriptor.exe spamap --db <文件> --refno a/b`）：FLOOR 24381/38614 → [-8233.7, -25504.3, -20]–[9842.8, -15762.3, 80]；STWALL 37115 与其 POSS / POSE / DESP 吻合；7997 全表 5.6 万条，可一次读入。

覆盖：WALL、STWALL、GWALL、FLOOR、PANE、SCTN、FITT、PCLA、ATTA 有条目；STRU、FRMW、EQUI、HANG 等容器与 NBOX / NCYL / NXTR 负体没有。命中后用 `element_type` 过滤、沿属主链上溯。服务端 `/api/v1/spatial/nearby` 读的是生成的 AABB 树，mem 实例里条目数为 0，不能依赖（计划决策 D7）。

## 4. 房间

本 worktree 默认 features：`ws, gen_model, manifold, project_hd, http_api`（无 project_hh）→ `RoomHierarchy::Hd`：房间 = NAME 含 `room_key_word`（DbOption `["-RM"]`）的 FRMW；面板 = 子 / 孙 PANE；房号 = NAME 最后一个 `-` 后的段，须匹配 `^[A-Z]\d{3}$`；在线 215 间（7997 有 214 间），`GET /api/v1/spatial/rooms` 可列名单。

楼板标房号（代码放 `src/mbd/`，同 crate 才能调）：`room_topology::room_groups_from_set(&set, &[site], Hd)` → `room_panel_groups(&g, &kw)` → `room_model::room_panel_map_from_groups(.., configured_match_room_fn())` 得 `RoomPanels{room, room_num, panels}` → 面板 `loop_profile` + `world_with_sjus` 做棱柱（R301 两块面板 z 范围 −20..2705 与 2705..3500）→ 按楼板标高选出覆盖它的棱柱交给求解器。服务进程里也可用 `room_resident::current_for_calculation()`；roots 不要给整库 WORL（7997 约 17 万个元素）。

## 5. 构建与测试

现有：`cargo test --test mbd_branch_query`（6 个，ams7999 / 7997）、`--test mbd_http_endpoint`（axum + reqwest 回环）。开库：`DirectoryDbResolver::scan(&[ams000], tpl)` 惰性解析目录库，主库 `DbFilePin{file, template_file_for(tpl,"DESI"), db_type:Some("DESI"), sesno:None}` 钉入再 `.with_element_cache()`；缺样本或模板时 `eprintln!("SKIP")` 后 return（不用 `#[ignore]`）。目录库跨项目：WCC 支撑的 PCLA / SCTN 目录在 ZDJ 7600，墙的 SPCO 在 acp7001，MDS 在 acp7320；dbnum 7000 在 ams000 与 acp000 都有，须用 **AMS 优先的链式 resolver**（服务端按 MDB 解析的 `build_set` 是 `pub(crate)`，测试里用不了）。

新测试 `tests/mbd_object_query.rs` 最小模板：

```rust
use std::{path::{Path, PathBuf}, sync::Arc};
use e3d_io::{db_element::{DbFilePin, DbFileResolver, DbSet, template_file_for}, refno::RefNo};
use e3d_model::db_discovery::DirectoryDbResolver;

const DIRS: [&str; 3] = [r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000",
    r"D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000", r"D:\AVEVA\Projects\E3D3.1\ZDJ\ZDJ000"];
struct Chain(Vec<DirectoryDbResolver>); // AMS 优先：7000 两个目录都有
impl DbFileResolver for Chain {
    fn resolve(&self, d: u32) -> Option<DbFilePin> { self.0.iter().find_map(|r| r.resolve(d)) }
}
fn open(dbs: &[&str]) -> Option<Arc<DbSet>> {
    let tpl = std::env::var_os("AIOS_E3D_TEMPLATE_DIR").map(PathBuf::from)
        .unwrap_or_else(|| r"E:\reverse\e3d\shadow_e3d31_aps_all".into());
    if !tpl.join("attlib.dat").is_file() || !Path::new(DIRS[0]).is_dir() { eprintln!("SKIP"); return None; }
    let chain = Chain(DIRS.iter().filter(|d| Path::new(d).is_dir())
        .filter_map(|d| DirectoryDbResolver::scan(&[PathBuf::from(d)], &tpl).ok()).collect());
    let set = Arc::new(DbSet::with_attlib_file_and_resolver(tpl.join("attlib.dat"), Box::new(chain)).ok()?.with_element_cache());
    for f in dbs {
        set.add_db(DbFilePin { file: Path::new(DIRS[0]).join(f), template: template_file_for(&tpl, "DESI").ok()?,
            db_type: Some("DESI".into()), sesno: None }).ok()?;
    }
    Some(set)
}
```

## 风险

1. 目录库跨项目（见 §5）。
2. 可见性：`wall_local_solid`、`evaluate_*`、`member_env`、`catalogue_component`、`resolve_catalogue_axis`、`room_topology` 是私有或 `pub(crate)`；要么给 e3d-model 加公开 API，要么照抄并接受与网格结果慢慢偏离的风险（SCYL 负高度翻轴、CLFL、`Pn` 轴、PARA / DESP 取值规则）。
3. UDA：测试里的 DbSet 不挂 UDA 字典时所有 `:X` 悄悄变 None；AMS 本身没有 `:ZD_*`。
4. 样本覆盖不全：没有在用 SUPPO、没有 PURP=DOOR、很多 NOZZ 没目录件、MSUP 的 SCTN 只画中心线、SPAMAP 没有容器与负体条目、NOZZ 只有点盒。
5. 坐标系与时效：POS / ORI / HPOS / POSS 是属主系，必须乘 `world_matrix`；SCTN / STWALL 自身没有 POS / ORI；SPAMAP 是存盘快照、精度 0.1 mm；DbSet 打开时钉住会话，长时间读取要比对 `is_current`。
