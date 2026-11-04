# 7999 only_baseline / only_generated 抽样核对（2026-11-03）

> 方法：`rvm_facets_dump` 直读基准 RVM，按组名抽取，核记录是否带几何图元。
> 结论分两类：**口径差异**（几何已由别路对拍覆盖/记录键名不同）与**真缺失**（基准有几何、生成侧完全没有）。

## 抽样表

| 类型 | 数量 | 样本 | 基准记录带几何？ | 结论 |
|---|---|---|---|---|
| PJOINT | 894 | `PJOINT 1 of PNODE 1 of FRMWORK ...`（80211） | **是**：八角环/法兰 FacetGroup，~133×157×200 mm，带内孔 | **已修复（2026-11-04）**：PJOI 走目录件路径（GTYP=JOIN + SPRE→GMRE 门），帧=beam端投影 `end+((NPOS+DELP−end)·û)·û`，894/894 exact |
| SCOJOINT | 533 | 80211 全量 922 组中 533 带几何 | **是**：同类管架节点环 | **已修复（2026-11-04）**：SCOJ 走同一目录件路径，帧=SNOD 站位系（c0=|z|最大列、c2=s1），`POSL=NB` 加局部 −90° 滚转，533/533 exact |
| BRANCH | 38+1+1 | `BRANCH 2 of PIPE 10 of ZONE 2`（80211） | 有（E3D 把隐式管写在容器组内） | **口径差异**——管身几何已由 `container_tubes` 逐根对拍，全 exact；缺的只是 BRANCH 键名记录本身 |
| BEND | 3+3 | `BEND 3 of BRANCH /C-OR-1RX210-R`（80211） | 有，但是 **CircularTorus 半径 5mm、高 10mm 的微型弯头** | **已修复（2026-11-04）**：缺 SPRE 的 BEND 走 bodyless 零尺寸记录（米制声明盒修正后），80211 BEND 1353/1353 exact |
| CTORUS | 1 | `CTORUS 9 of SUBEQUIPMENT 21`（73927，同组 13 件里漏 1 件） | 是（`CircularTorus[0,0,1.5707964]` 零尺寸记录） | **已修复（2026-11-04）**：设计侧 `CTOR`（`RINS/ROUT=0/0`）+ 目录侧 `SCTO`（PAAX/PBAX 原点重合）都走 bodyless 记录，73927 CTOR 14/14 exact |
| PYRAMID | 1 | `PYRAMID 1 of SUBEQUIPMENT 21`（73927，`24383/78389` `HEIG=0`） | 是（`Pyramid[0,0,0,0,0,0,-0]` 零尺寸记录） | **已修复（2026-11-04）**：设计侧 `PYRA`（`HEIG=0`）+ 目录侧 `flat_lpyr` 全零都走 bodyless 记录，73927 PYRA 9/9 exact |
| TUBI（only_generated） | 118+1 | `/GENERATED/TUBI n of BRAN ...` | — | 118 件**口径差异**——键名不同，几何已由 container_tubes 全覆盖；**另 1 件（`/C-CO-1RB-7AA0A` 头管 3500mm）= 真多发** |
| TUBI（竖井门） | 1 | `/GENERATED/TUBI 1 of BRAN /C-CO-1RB-7AA0A`（80211） | 基准侧无 | **已修复（2026-11-04）**：该 BRAN `PSPE=/ACP1000-Riser`（电缆竖井 spec）。全库 103 条 Riser-spec 支管在基准里隐式管数=0（102 条槽位本就退化，仅这条槽位判据全过）；E3D 对竖井支管一律不画管身。已加 `riser spec` 门（`not_drawn_by_reason` 记账） |
| HANG 轴线柱 | 529 | `HANG /SUPPORT/HANGERS/PIHA/**/H1|H2|H3`（7333） | 是：`Cylinder[0, L]` 零径轴线柱 | **已修复（2026-11-04）**：柱 = 链序第一个「正长+共轴」槽位（VH298 两段 2.5mm 槽位只画第一段；VH111=HNUT3.leave→VSPR.arrive 205mm）。`route::hang_axis_stub` 走 walk_route + slot_alignment 判据 + tumat 帧，进 bodyless 通道（`local_transform` 记录标架=元素、图元=槽位标架）。7333 `both=529 exact=529`，unkeyable 清零 |
| HANG（only_generated） | 3 | `/GENERATED/1WCC-R22*/HANG-*`（66456） | 基准侧无 | **已修复（2026-11-04）**：`1WCC` 手搭吊架族（成员 PCLA/HELE/SCLA/HROD，`HSPE`=nulref）链上虽有正长共轴槽位（如 `HANG-12` slot2=360mm），E3D 对该族 241 根**一条记录不写**——判别门 = `HSPE` 非空（spec 驱动吊架才发本体记录）；7333 的 529 条记录所属吊架 `HSPE` 全非空。门后 66456 清零、7333 仍全 exact |

## 汇总

- **真缺失合计 ≈ 1,958 件 → 0**：PJOINT 894 + SCOJOINT 533 + CTORUS/PYRAMID 零散 2 + **HANG 轴线柱 529**（7333，2026-11-04 修复）全部已修（见上表）。
- **真多发 4 件 → 0**：`/C-CO-1RB-7AA0A` 竖井头管（`riser spec` 门）+ `1WCC` 手搭吊架 3 根（`HSPE` 门，2026-11-04）。
- **口径差异合计 ≈ 158 件**：BRANCH 40（键名）+ TUBI 118（键名）。
- **BEND 占位 6 件 → 0**：缺 SPRE 的零尺寸记录已补写。
- 以上全部属于 only_baseline/only_generated 口径，**不在 #85 的 "off" 统计内**（off 只统计双方都有的记录对）。

## 处置（2026-11-04 更新）

- PJOINT/SCOJOINT/CTORUS/PYRAMID/HANG 轴线柱已修并全 exact；BRANCH/TUBI 键名差异、BEND 微缺失 → 写进 #85 终态报告的「已解释遗留账」。
- 2026-11-04 终态：九库 19 roots **241,393 对全 exact**（`summary.json`，`generated_at = 2026-11-04T22:17:21+08:00`），`off 与单侧件 0 条`（新二进制逐库重跑；`_regress_all.ps1` 已补 `-BinDir` 透传，避免再静默落到共享 target 池的陈旧 debug 二进制上）。
