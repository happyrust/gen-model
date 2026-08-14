# ADR-026：扫掠体步骤对齐 Core3D，并以单位网格身份做持久化复用

状态：Accepted（2026-08-14）

关联：ADR-002（增量影响以 core.dll 为准；几何生成权威在 core3d.dll）；ADR-024（SavePlan 同 ID 异内容阻断）；术语见 `CONTEXT.md`「扫掠体 / 单位几何 / 实例变换 / 单位网格身份 / 规范挤出 / 斜切平面」。

扫掠体（PrimLoft / `SweepSolid`）的公开生成步骤按 `DB_Gensec` 的 Rust 蛇形命名实现（`set_mitre_planes`、`set_implied_bangs`、`set_spine_segment_transforms`、`do_solid_segments`）。斜切判定与内核相同：端面法向相对**该段切向**垂直或平行则无斜切，且只产出工作斜切平面、不改元素上的 `DRNS`/`DRNE`。相对内核唯一多出来的差别是持久化的单位网格身份：可复用直线、无斜切时身份只键目录截面；`inst_geo` 仍存带规范挤出常数的 `PrimLoft` 信封（长度固定 10）；长度 / BANG / PLAX / 镜像进实例变换。真斜切与圆弧仍用完整参数。SavePlan 继续整份严格比较，禁止 first/last-wins。全图元 `BrepShapeTrait` 改名、多段 SPINE、规范长度迁移不在本期。

验证：CI 用从 `setMitrePlanes` 反出的表驱动纯函数（切向垂直/平行、`1e-6`、非 Z 切向）做门。与 Core3D 的 A/B 走 ADR-019 TTY：同一 GENSEC 夹具对照斜切调试日志（`"DRNS is actually perp; no mitre"` / `"parallel; suppress the mitre"`）以及世界外观（RVM/AABB）；安排在 L1–L5 单测变绿之后留证据，不挡本期改名与谓词。不在本进程 FFI `core3d.dll`。

## 否决方案

- 把 `BANG` 烤进截面再 hash（抄 `D2_Profile::rotateBy`）：缩小复用。
- 单位几何存纯截面变体、或按真实长度挤出后落库：分别是存储迁移和身份分裂，不是本期闭环。
- 在 `aios-database` 对 loft JSON 忽略字段或 first/last-wins：把身份规则做成保存特例。
- 用世界 `±Z` 当斜切谓词：非 Z 向 SPINE 上垂直端面会被误判为斜切，无法复用。
- 对 `core3d.dll` 做 FFI A/B：DLL 是 32 位、`DB_Gensec`/`gm_Create*` 未导出，且需要完整 `des.exe` 初始化；同进程无法加载，比的也不是稳定 C ABI。
