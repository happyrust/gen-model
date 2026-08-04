# 案例 16 · WALL 的精确 CATA 闭包漏了 GMSS，几何数为 0

<sub>族 E 解析与按需 · High · 已修 · 证据层 B（单测）+ C（实库按需生成）</sub>

## 一句话

按需解析的白名单少了一层 `GMSS`，于是 `SPRF → GSTR → GMSS` 这条目录几何链断在最后一跳——
数据库里只有 `pe_owner` 边、没有几何实体，生成出来是 **0 个几何体**。

## 现象

请求 `WALL 24381/180032` 的按需模型生成：

- 不报错，流程走完；
- `resolve_desi_comp` 返回**空几何**；
- 库里查得到该 WALL 的 `pe_owner` 边，查不到它的目录几何实体。

## 证据

出处 [`../../docs/2026-07-25_test-structure-floor-wall-incremental-report.md`](../../docs/2026-07-25_test-structure-floor-wall-incremental-report.md)。

WALL 的目录几何链是 `SPRF → GSTR → GMSS`。而精确 CATA 闭包的原白名单只有
`GMSE / NGMS / PTSE / PSTR / SPRO / DTSE`——**没有 `GMSS`**。

修复后的实库结果：

| 类型 | 请求参考号 | 生成根 / 父级 | 结果 |
|---|---:|---:|---|
| FLOOR | `24381/180272` | `CFLOOR 24381/180271` | 通过；AABB `[0,0,0]`～`[24450,24450,100]`，平移 `[0,0,45130]` |
| WALL | `24381/180032` | `CWALL 24381/180031` | **修复前 0 几何**；修复后通过，实际更新 22 个模型节点 |
| STWALL | `24381/180037` | `CWALL 24381/180031` | 实例、AABB、变换均存在 |
| GWALL | `24381/180703` | `24381/180702` | 通过；AABB `[2148.79,23858.53,41550]`～`[6476.95,25436.68,45130]` |

WALL `24381/180032` 的最终数据库证据：
`inst_relate:24381_180032.aabb = aabb:⟨582248735169211718⟩`，
AABB `[-13020.83,-24188.41,-6000]`～`[-7467.17,-20341.62,-5900]`，
world translation `[101.91,226.26,-6000]`。

## 根因

「精确闭包」是一份**手写的类型白名单**：从种子出发沿引用关系传递可达、收口到元件库类型
（术语见 [`CONTEXT.md`](../../CONTEXT.md) 的**引用闭包**）。白名单少一个类型，就意味着某一类
目录子树整棵不会被解析——而缺失的表现不是报错，是**安静地生成 0 个几何**。

第二层根因更值得记：**闭包结果有缓存**。白名单改了之后，旧缓存仍然宣称「这个种子的依赖已经解析完了」，
于是修复不生效。

## 修法

1. 精确闭包加入 `GMSS`；
2. 闭包依赖缓存增加 **`CATA_CLOSURE_SCHEMA_VERSION = 2`**——规则变化后旧缓存自动失效；
3. `SPINE` 缺通用 `pe` 行时从 `SPINE` noun 表读取（与案例 17 的隐含子元素同一类问题）。

## 验证

```powershell
cargo test data_interface::cata_closure::tests::precise_config_limits_container_subtree_and_adds_desi --lib
# 1 passed

AIOS_CATA_CLOSURE_MODE=on AIOS_ON_DEMAND_TEST_REFNO=24381/180032 `
  cargo test live_generates_a_missing_model --lib -- --ignored --nocapture
# WALL: passed

AIOS_CATA_CLOSURE_MODE=on AIOS_ON_DEMAND_TEST_REFNO=24381/180703 `
  cargo test live_generates_a_missing_model --lib -- --ignored --nocapture
# GWALL: passed
```

同一轮还验证了结构专业的增量路由（分类器 / 生成根 / 真实模型生成三层）：

| 修改 | 期望动作 | 结果 |
|---|---|---|
| PAVE/PLOO 下 `FRAD` | 重生成 FLOOR 几何 | 通过 |
| WALL/FLOOR 的 `HEIG`、`DESP` | 重生成几何 | 通过 |
| `POS` 移动 | 只更新 world transform | 通过 |
| WALL/STWALL 的 `SPRE` | 目录依赖反向级联并重生成 | 通过 |
| `NAME` | 更新数据和模型树，不重建几何 | 通过 |
| PAVE/VERT/PLOO 子构件 | 归一到 FLOOR/CFLOOR 最小交付单元 | 通过 |
| WALL 子构件 | 归一到 CWALL 最小交付单元 | 通过 |

**未完成的可视化验收**（如实记录）：E3D 与 `rs-plant-3d` 的窗口自动化在当前桌面会话连续发生
monitor capture interop 错误（`IGraphicsCaptureItemInterop.CreateForMonitor failed (0x80070057)`），
无法采集前后截图。本报告确认数据库增量判定、生成根、目录按需解析、模型实例与 AABB 正确，
**不把数据库 AABB 证据冒充三维前后截图**。

## 规律

**白名单式的可达性规则，缺一项的表现是「安静的空结果」而不是报错。** 这类规则必须配一条
「结果为空即失败」的断言——按需生成拿到 0 个几何时，正确反应是报错而不是写一条空记录。

**规则变了，缓存必须跟着失效。** 任何缓存「计算结果」的地方都要带上**规则版本号**，
否则一次白名单修复会被旧缓存完整地吃掉，而且看起来像是「修复无效」。
`CATA_CLOSURE_SCHEMA_VERSION` 这种做法应该成为闭包 / 分类 / 名单类缓存的默认配置。

## 关联

- [`../../docs/2026-07-25_test-structure-floor-wall-incremental-report.md`](../../docs/2026-07-25_test-structure-floor-wall-incremental-report.md)
- [`ADR-004 按需目录解析移植`](../../docs/adr/ADR-004-on-demand-cata-parsing-port.md) · [`CONTEXT.md`](../../CONTEXT.md)（引用闭包 / 部分解析 / 惰性兜底 / 闭包漏边）
- 案例 [17 结构专业三连](case-17-structural-on-demand-trio.md)（同一批测试里的其余修复）
