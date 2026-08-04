# AMS 8000 按需生成模型验证与三处修复（2026-07-27）

环境：AvevaMarineSample / dbnum 8000（`applied_sesno = file_latest_sesno = 34`，已初始化），
gen-model 二进制 `aios-database`（`http_api_addr = 0.0.0.0:8021`），SurrealDB fork 落盘实例
`ws://127.0.0.1:8009`（ns `1516` / db `AvevaMarineSample`）。测试全程只读 E3D 源文件，
不产生新会话；模型侧的写入都来自 `POST /api/v1/model/ensure` 自身的生成。

起因是验证 plant-ui 与 gen-model 的按需生成。管道与结构那条路一开始就是通的，
设备（EQUI）那条路稳定失败，顺着失败一路挖到元件库闭包，中间修了三处。

## 一、初测结果（修复前）

| 用例 | refno | 结果 |
|---|---|---|
| BRAN 冷生成 | 24384/23001 | 200 `Generated`，24.8s，1 个实例 |
| 同一 BRAN 重复调用 | 24384/23001 | 200 `AlreadyAvailable`，24ms |
| SUPPO 冷生成 | 24384/25725 | 200 `Generated`，**103.9s**，7 个实例 |
| 风管 BRAN（无 FTUB） | 24384/24935 | 200 `Generated`，**98.8s**，12 个实例 |
| 并发同根两发 | 24384/23003 | 一条 `Generated` 一条 `AlreadyAvailable`，均 17s，`GENERATION_LOCKS` 生效 |
| EQUI（只挂 ELCONN） | 24384/24882、24884、24826 | **500 internal**，三个各复现 |
| ZONE / SITE | 24384/22400、22399 | 500 internal「无法解析生成根」 |
| 不存在的 refno | 99999/1 | 500 internal，与上一条无法区分 |
| 格式非法 | `abc` | 400 `bad_request` |

SUPPO 与风管 BRAN 的冷生成耗时贴着服务端 120s 超时线，plant-ui 客户端超时也是 120s，
两边同时到点；这条限制在 `docs/specs/web-service-api.md` §4.5 已记录，客户端不能设得更短。

## 二、失败链（EQUI → 元件库）

500 的报文是「已生成生成根 24384/24882 (EQUI)，但请求构件 …… 仍没有可渲染模型」。
生成其实成功了，`inst_relate:24384_24883` 确实写进了库，卡在判据上：

1. `renderable_instance_count` 走 `query_insts`，SQL 带 `where aabb.d != none`；
2. 那条 `inst_relate` 没有 `aabb`；
3. 它的 `inst_info` 有 5 条 `geo_relate`，**全部指向同一条 `inst_geo:⟨15875063915832344321⟩`**，
   而该记录 `meshed` 为空、无 `aabb`，`assets/meshes/` 下也没有对应 `.mesh`；
4. 全库 534 条 `inst_relate` 只有 360 条带 aabb，缺 aabb 的按 noun 分得很干净：
   **BEND / ELBO / ELCONN / ANCI / FIXING**——全是走元件库出几何的那批；
   BOX / CYLI / FTUB / GENSEC 都正常。指向那条缺失几何的 `geo_relate` 有 254 条。

那条被共用的几何存的是一个空轮廓的挤出体：

```json
{"PrimExtrusion": {"cur_type": "Fill", "height": 100.0, "verts": [[]]}}
```

一个 loop、零个顶点。存进去的是归一化后的 unit shape（`gen_unit_shape` 把 height 写死 100.0），
所以**所有空轮廓挤出体算出来是同一个 hash**，五类构件全塌缩到这一条记录上。

再往上游：库里 **SVER（轮廓顶点）为 0、SLOO（轮廓环）为 0**，而 SEXT 有 219 条——全是空壳。
`query_cata.rs:209` 读挤出体 / 回转体的轮廓走的是三层
`SEXT|NSEX|SREV|NSRE → SLOO → 顶点`，顶点子元素一个都没有，`verts` 与 `frads` 双双为空，
`resolve_helper.rs:286` 据此建出 `CateExtrusionParam { verts: [] }`。

而 `CataClosureConfig::precise()` 的容器子树白名单当时是
`GMSE / GMSS / NGMS / PTSE / PSTR / SPRO / DTSE`——停在几何集这一层，
SEXT 自己不在名单里，它底下的 SLOO / 顶点子树因此从来没被展开。
闭包还自认为把该拉的都拉了（日志一路 `missing=0`），连告警都不会响一声。

## 三、三处修复

### 1. `ensure` 的判据（`src/data_interface/on_demand_model.rs`）

判据原先只数一个数，「生成还没跑过」与「跑过了只是画不出来」在它眼里都是 0。
拆成两个计数：`written`（子树里写出的 `inst_relate` 条数，不看 aabb）与
`renderable`（`query_insts` 认的）。新增终态 `NoRenderableGeometry` 走 **200 而不是 5xx**——
这是数据的终局不是一次失败，底下几何不修好，重发只会把同样的生成再跑一遍。
`written > 0` 的生成根直接回状态，不再重生成。响应多一个 `generated_instance_count`。

请求新增可选 `force`，只给「人明确要求重生成」用（S4-C 的重试按钮）；显示补齐不传。
没有这个口子，一旦落进 `NoRenderableGeometry` 就再也重试不了。

### 2. 几何失败不再静默且白算（`src/fast_model/occ_generate.rs`）

`gen_occ_shape()` 失败的分支原先只在 `feature = "log_error"`（未开启）下打一行，
并且不把 id 放进 `shapes_map`——而下面那句「有问题的模型就不需要每次都重复生成了」
的 `set bad = true` 只遍历 `shapes_map`，永远轮不到它。`gen_inst_meshes` 的取数
（:417）恰恰按 `!out.bad` 过滤，于是每一轮生成都把同一份废参数重算一遍，全程无声。

改动：单独收一份 `unbuildable` 跟着同批 SQL 标记；三角化出来没有包围盒那一支不再
`continue` 跳过标记；两个失败分支的告警改成无条件 `eprintln!`，带上几何 hash、原因、
波及构件数。顺手把 `query_refnos_by_geo_hash(...).unwrap()` 换成 `unwrap_or_default()`
——它在 `tokio::spawn` 里，查失败就是整个任务 panic。

### 3. 元件库闭包名单（`src/data_interface/cata_closure.rs`）

`CataClosureConfig::precise()` 的白名单补上 `SEXT / NSEX / SREV / NSRE`（几何体 → 轮廓环）
与 `SLOO`（轮廓环 → 顶点）。按文件里本来就备好的机制把
`CATA_CLOSURE_SCHEMA_VERSION` 从 2 推到 3，否则旧依赖缓存会接着用。

## 四、修复后实测

| 指标 | 修复前 | 修复后 |
|---|---:|---:|
| 库中 SVER（轮廓顶点） | 0 | 5181 |
| 库中 SLOO（轮廓环） | 0 | 198 |
| 闭包单次 parsed | 0 / 94 / 209 | 315 / 960 |
| BRAN 24384/22404 可渲染实例 | 17（写出 35） | 78 |
| EQUI 24384/24882 | 500 internal | `AlreadyAvailable`，6 个实例，521ms |

对 8000 库还挂着旧空几何的 104 个生成根做了一次全量强制重生成（37 分钟）。
先用不带 `force` 的快调用把 123 个待补齐节点归并到真正的生成根（ENDATU / PLDATU / SUPC
都归到所属 SUPPO），去重后 104 个，再逐个 `force`：

| 缺包围盒的实例 | 补齐前 | 补齐后 |
|---|---:|---:|
| BEND | 161 | **0** |
| ELBO | 55 | **0** |
| ELCONN | 34 | **0** |
| FIXING | 14 | **0** |
| FTUB | 4 | **0** |
| ANCI | 6 | 7 |
| 合计 | 274 | **7** |

全库 1535 条 `inst_relate` 里 1528 条可渲染（99.5%）。

## 五、剩下那 7 个 ANCI

它们与上面不是一回事。新的 42 条退化几何都有真实顶点，只是退化得彻底：

```
verts: [[-0.007, 51.5, 0], [-0.007, 50, 0], [-0.007, 50, 0], [-0.006, 50, 0]]
→ Extrusion gen_occ_shape error: wire 顶点数量不够，小于3
```

四个点里两个重合、剩下的几乎共线，宽度千分之七毫米——元件库里就是这么建的，
不是解析丢了东西。第二处修复在这里表现得很准：**42 条退化几何、恰好 42 条告警，
一条一声、没有重复**，且都标上了 `bad`。

## 六、plant-ui 侧：按需生成在真实产品界面上仍打不通

- 唯一接了 `ensure` 的是开发壳 `plant-ui-app`：S4-C 失败单元行上那枚「重试」
  （`Cmd::RetryModelUnit` → `model_update_api::ensure_model`），要先跑出一次带失败单元的
  手动更新才露头。本次把它改成带 `force` 发，并把回执从 `()` 换成两个计数——
  「生成完成但一条都画不出来」单独记一条告警，不再混进「已重新生成」。
- 渲染宿主 `rs-plant3-d` 的同一条命令只写一行「宿主尚未接入单元重生成」，请求根本不发。
- ADR-0009 的「显示时补齐」没有落地，`model_system.rs` 里仍是那段进程内直接调
  `gen_all_geos_data`、不走服务、无条件重生成、还 `.unwrap()` 的 `auto_gen` 调试代码。

底下几何现在真的有了，接这条路才划算。

## 改动文件

- gen-model：`src/data_interface/on_demand_model.rs`、`src/data_interface/cata_closure.rs`、
  `src/fast_model/occ_generate.rs`、`src/web_service/handlers.rs`、`docs/specs/web-service-api.md`
- plant-ui：`crates/plant-ui-app/src/{model_update_api,data,main}.rs`、`crates/plant-ui/src/lib.rs`

单元测试 12 passed（`on_demand_model` 4 + `cata_closure` 8），两侧 `cargo check` 干净。

## 未处理

- `category.rs:587` 的挤出体分支缺一道 `verts.len() <= 2` 校验，而它上面 27 行的
  Revolution 分支（:540）有。补上可以从源头拒绝退化体。注意 **gen-model 编的是
  `D:\work\plant-code\rs-core-pin`、plant-ui 用的是自己的 `vendor/rs-core`**，两份同源同行号，
  改一边不影响另一边。
- 容器（ZONE / SITE）与不存在的 refno 都返回 500 `internal`，客户端无法按 ADR-0009
  区分「这是容器，该展开一层」与「服务端出错」。
- 那 7 个 ANCI 的元件库轮廓需要回到 AVEVA 工程数据里确认。
