# Issue #85 终态评论草稿（待用户发布）

> 粘贴到 https://github.com/happyrust/gen-model/issues/85 作为收尾评论。

---

## 终态：off 与单侧件全部归零 ✅

九库终态回归（2026-11-05，`verification/ams-rvm-20261030/` 逐库重跑，生成侧为含
PJOI/SCOJ/CTORUS/PYRA/BEND/riser/HANG 修复链的 e3d-model 工作区版）：

| dbno | roots | both | exact | chord | film | off | only_baseline | only_generated | kind_differ |
|---|---|---|---|---|---|---|---|---|---|
| 7326 | 2 | 28,656 | 28,656 | 0 | 0 | 0 | 0 | 0 | 0 |
| 7327 | 2 | 34,152 | 34,152 | 0 | 0 | 0 | 0 | 0 | 0 |
| 7331 | 2 | 844 | 844 | 0 | 0 | 0 | 0 | 0 | 0 |
| 7332 | 2 | 844 | 844 | 0 | 0 | 0 | 0 | 0 | 0 |
| 7333 | 2 | 159,876 | 159,876 | 0 | 0 | 0 | 0 | 0 | 0 |
| 7352 | 2 | 48 | 48 | 0 | 0 | 0 | 0 | 0 | 0 |
| 7354 | 2 | 760 | 760 | 0 | 0 | 0 | 0 | 0 | 0 |
| 7356 | 1 | 1,064 | 1,064 | 0 | 0 | 0 | 0 | 0 | 0 |
| 7999 | 4 | 15,149 | 15,149 | 0 | 0 | 0 | 0 | 0 | 0 |
| **合计** | **19** | **241,393** | **241,393** | **0** | **0** | **0** | **0** | **0** | **0** |

`cargo test --lib`：428 passed / 0 failed / 5 ignored。

## 逐族归因 → commit

| 族 | 件数 | commit | 机制 |
|---|---|---|---|
| 7333 PCLA | 7,576 | `861c811` | PCLA 移出 CLFL 四分之一转白名单（SCYL 照 PAXI 直出） |
| 7356 SJOI 缺件 | 141 | `78aeef6` | CREF 缺席关节照常出几何、正体标架不乘切割符号 |
| 7356 SJOI 24 | 24 | `ebcacfe` | 双 SEXT 拓扑下 BANG 旋进目录件标架（四条件门控） |
| 7356 SJOI 5 | 5 | `d106fe9` | 左端关节不投到宿主梁线（POSL 负侧） |
| 7356 FITT 18 | 18 | `4357f86` | DRNS/DRNE 端面安装不再走 p-line 投影 |
| 7327+7999 HELE | 41 | `82b96d8` | CLFL 白名单清空——QSCYLI/CRCATI 反编译证实无四分之一转 |
| 7356 SJOI 12 | 12 | `916dbc6`+`a6af9c8`+`14b9e64`+`0017b01` | col2 改 p-line 方向；共线/单X DELP 按 GJDORG 换轴并乘 sign(BANG) |
| 7356 CT05 chord 2 | 2 | `dc416e6` | 倾斜宿主轴（0.074°）上按 CUTB 补足沿轴走量 |
| 7999 ATTA 1 | 1 | `9b9fb3e` | 参与布尔的 torus 按 E3D 粗环密度出面片（修复前后该件 Y 向 AABB 46.38→56.45mm，差 10.07mm 与 issue 记录一致） |
| 7999 BRAN 1 | 1 | `e81381c` | 共享 TEE 的贯通管身归到它指向的 BRAN（SPAMAP 哨兵判别） |
| 7999 PJOI/SCOJ | 894+533 | 工作区 | PJOI/SCOJ 走目录件路径（GTYP=JOIN + SPRE→GMRE 门），帧分别为 beam 端投影 / SNOD 站位系+`POSL=NB` 局部滚转 |
| 73927 CTORUS/PYRA | 1+1 | 工作区 | 零尺寸目录图元与设计侧退化记录走 bodyless 通道；`Snout` 出身识别器保留声明种类（kind_differ 6 件同步清零） |
| 80211 BEND 占位 | 6 | 工作区 | 缺 SPRE 的 BEND 补写零尺寸 bodyless 记录（BEND 1353/1353 exact） |
| 80211 TUBI 1 | 1 | 工作区 | `/C-CO-1RB-7AA0A` 竖井头管：`PSPE=/ACP1000-Riser`，103/103 Riser 支管基准零隐式管 → `riser spec` 门抑制 |
| 7333 HANG 轴线柱 | 529 | 工作区 | spec 驱动吊架（`HSPE` 非空）本体记录 = 链序第一个「正长+共轴」槽位的零径 `Cylinder[0, L]`（`tumat` 帧、bodyless+`local_transform`）；`1WCC` 手搭吊架（HSPE=nulref）E3D 一律不写 → `HSPE` 门 |

## 遗留账（均已归因或修复，非 off）

- **kind_differ 6 → 0**（73927：CONE 5 + VALV 1）：等径锥按声明 `Snout` 写出 + 全零 `Snout[0,0,0]` 退化记录补写 → 已修，不再是 `e3d-model#7` 的开放项。
- **TUBI 118 / BRANCH 40 键名口径差异**：E3D 把隐式管写在容器组内、我们写在 TUBI 键下 → `container_tubes` 桶逐根对拍全 exact，不再计单侧件。
- **unkeyable_baseline 529 → 0**（7333 HANG 轴线柱）与 **only_generated 3 → 0**（66456 `1WCC` 吊架 `HSPE` 门）：见上表。
- **E12 caveat 已收口**：`dc416e6` 的 `cutb*lateral` 曾标「窄门控经验拟合」；`GJTORI`/`GNAORI`/`GSPLNU`/`GSCDIR` 全链反编译后 `v44` 闭式解出（IJOINT 非端部 POSL = `normalize((v30×CUTP)×v30)`，语料内 `CUTP⊥v30` 退化为 `R·CUTP̂`）——现行实现已是忠实局部形式，代码不改；`e3d-model#15` 可关。

## 验收口径核对

issue 要求「off 归零或逐件归因」：**241,393/241,393 exact**，off/chord/film/单侧件/unkeyable/kind_differ 全 0。建议关闭本 issue。

（附：`verification/ams-rvm-20261030/` 内有完整 compare.json 与逐件 pairs 可查。）
