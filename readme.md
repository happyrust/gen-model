## 本地 SurrealDB：**别用 `PATH` 上的 `surreal`**

启动一律走 `./scripts/Start-Surreal8009.ps1`（`-Memory` 起一次性空库，跑 live 测试用）。

本仓库的客户端 SDK 是 fork 的 SurrealDB **2.1.4**
（`Cargo.lock`: `git+https://gitee.com/happydpc/surrealdb.git#45013fc9…`），
服务端必须是同一条线的 2.1.x。对的二进制就在仓库里：`bin/surreal.exe`。
而 `PATH` 上通常是 `cargo install` 装的 3.x，用它会撞上两个坑：

- 打开 `.surreal/ams-8009`（存储版本 2）报
  `The data stored on disk is out-of-date with this version (Expected: 3, Actual: 2)`。
  它让你按升级指南迁移——**别照做**，那是不可逆的，你需要的只是换个二进制；
- 即便改用 `memory` 后端绕开数据，也会报
  `WebSocket protocol error: SubProtocol error: Server sent no subprotocol`。

脚本会在启动前核对版本并直接拒绝 3.x，所以照着敲就不会掉进去。

---

## 架构一句话

数据面：稳态增量窗口直写 RocksDB 后端的 SurrealDB，尾事务推进 `applied_sesno`（ADR-056，kv-mem 暂存层已退役）。
模型面：`vendor/e3d-model` 经 `e3d-io` 直读 E3D 库文件生成几何（ADR-053 / ADR-057），gen-model 只持有
`gen_root` 凭证、队列、房间归属、空间树、MQTT 与 HTTP 面。术语以 `CONTEXT.md` 为准，决策见 `docs/adr/`。

---

## 分组规则组（GPWL / GPSET / SCOSEL）

GPSET 规则组的成员不是 GPITEM 槽，而是一段 `SCOSEL` 规则：后端求值后按解析视图返回成员行。三键形状区分成员类型——GPITEM 成员 `via: <refno>` + `secondary: true` + `rule: false`；规则组解析出的成员 `via: null` + `secondary: true` + `rule: true`；主链子节点三个键都不出现。

开关 `group_scosel`（配置项，默认 `false`），环境变量 `AIOS_GROUP_SCOSEL` 可覆盖；关着时所有响应与改动前逐字相同，`rule` 键完全不出现。

求值域是 MDB 声明范围（`element_databases()` 里 `path.is_some()` 的库），不是全库全扫。APS 实测：MDB 范围内 28 个 GPSET（27 条规则组 + `/ACCEPT-GROUP`），27 条中 24 条有命中、合计 57 个成员，3 条出 `scosel_no_match`，`/ACCEPT-GROUP` 出 `group_empty`。

验证入口：

- 单元/契约：`cargo test --lib`（判据是 **0 failed**，不是某个 passed 数）
- 真库只读：`cargo test --lib live_rule_groups -- --ignored --nocapture --test-threads=1`
- E3D 只读对照：`target\debug\l3_suite.exe --check-driver <宏>`，四个必知细节见下条专题文档

### 回执里的 `unresolved[]` 原因码明细

`POST /api/v1/model/generate` 与 `POST /api/v1/model/records` 的分组回执现在也带 `unresolved[]`，每项 `{ via, target, reason }`，形状与 `/api/v1/tree/children?refno=<GPSET>&resolved=1` 的同名键**完全一致**（三个端点共用 `handlers.rs` 里同一个 `unresolved_slots_json` helper）：`via` / `target` 走 `RefnoEnum` 的下划线形（如 `"2_15"`），`reason` 走 `UnresolvedReason::as_str()`。表为空时该键**不出现**（不是 `[]`），沿用 `resolved_children_body` 的 `!is_empty()` 守卫；出现时恒有 `unresolved_count == unresolved.len()`。

改动前，「分组真的空」与「规则一个也没命中」的 generate 回执**逐字段完全相同**——`status` / `generation_roots` / `generation_root_count` / `scope_refnos` / `unresolved_count` / `dbnum` / `routes` 全一样，用户无法区分两者。`unresolved[]` 就是为消掉这个歧义而加的。

7 个原因码分两族：

| 族 | 码 | 含义 |
|---|---|---|
| GPREF | `gpref_missing` | GPITEM 没写 GPREF |
| GPREF | `db_not_in_mdb` | 目标库不在本 MDB |
| GPREF | `target_missing` | 库在、元素不在 |
| SCOSEL | `scosel_eval_failed` | `SCOSEL` 字节解不出来，整条规则没参与求值 |
| SCOSEL | `scosel_unsupported` | 规则解出来了，但求值器有形状认不出（已算出的合法成员仍返回） |
| SCOSEL | `scosel_no_match` | 规则完整解出、无认不出的形状，但一个成员都没命中 |
| SCOSEL | `group_empty` | 分组既没成员也没规则 |

真库实测（开关开，2026-11-09，generate 与 records 两端点一致）：

| 分组 | refno | `unresolved[]` | 其余字段 |
|---|---|---|---|
| `/ACCEPT-GROUP` | `2013286764_3` | `[{via:"2013286764_3", target:null, reason:"group_empty"}]` | `unresolved_count=1`、`generation_root_count=0`、records `total=0` |
| `/AREA01_HVAC` | `2013286748_7` | `[{via:"2013286748_7", target:null, reason:"scosel_no_match"}]` | `generation_root_count=0`、`total=0` |
| `/AREA01_PIPE` | `2013286748_2` | **键不出现** | `unresolved_count=0`、`status=Generated`、`generation_root_count=98`、`total=727` |

开关关时，3 个分组 × 2 个端点共 6 份回执全部没有 `unresolved` 键，`unresolved_count=0`。

口径：规则族坏槽（`scosel_no_match` / `scosel_unsupported` / `scosel_eval_failed`）与 `group_empty` 的 `via` 填的是**分组自己的 refno**——`UnresolvedSlot.via` 是必填 `RefU64`，而这类槽并没有经由的 GPITEM。所以客户端**只应读 `reason` 做文案分流，不要把 `via` 当「成员经由的 GPITEM」去做成员级渲染**，否则会渲染出「成员经由分组自己」的误导文案。

完整记录：`docs/plans/2026-11-09-g3-group-rule-members-implementation.md`（8 个 commit 逐条说明、7 条关键设计决定、三键矩阵、实测数据、已知限制）；设计阶段见 `docs/plans/2026-11-08-gpwl-gpset-group-nodes.md` 与 `docs/plans/2026-11-08-gpwl-gpset-group-nodes-dev-plan.md`。

---

## all_attr_info.json 字段手动修改说明

1. PTCDI -> 更改为 PTCD
2. PAXI -> 类型要修改为 STRING
3. PAAX -> 类型要修改为 STRING
4. PBAX PCAX -> 类型要修改为 STRING
5. LEVE -> 类型要修改为 INTVEC
6. PTS -> 类型要修改为 INTVEC
7. CURD -> 类型要修改为 RefU64Vec
8. SPCO.DETR -> offset 改为 17
9. ANCI.PTNB -> 类型要修改为 INTEGER

---

## 编译

### Centos 7 的 cross build

```powershell
cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.17
```
