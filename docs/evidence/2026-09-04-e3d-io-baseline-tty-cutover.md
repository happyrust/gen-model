# e3d-io 基线与 E3D TTY 对照证据（2026-09-04）

## 判据

- 候选事实只来自 e3d-io 对 Dabacon 二进制、`attlib.dat` 和对应 `*vir.dat` 的读取。
- 外部等价只由 E3D TTY 的 `ATTLIST` 导出裁决。
- 旧 parser 输出、旧数据库快照和旧 parser 形状不作为参照数据。

## 7997 基线与持久化闭合

命令：

```powershell
scripts/Invoke-SiteDbnumParse.ps1 -Dbnum 7997 -Port 8797 -ReleaseDir D:\Rust\target\release -Storage memory
```

结果：

```text
BASELINE|AvevaMarineSample|7997|ok|170048
pe_count=170048
dbnum_info_count=170048
applied_sesno=106
file_latest_sesno=106
model_update_pending=18675
AMS DBNUM integrity passed for: 7997
```

模型工作项按最多 500 条的事务批先行幂等 UPSERT；全部成功后，尾事务才推进水位。这样避免了 18675 条工作项和水位共处一个超大事务导致的 closed-channel 失败。

## SYST 零属性 noun

命令：

```powershell
D:\Rust\target\release\e3d-descriptor.exe describe --desvir E:\reverse\e3d\shadow_e3d31_aps_all\sysvir.dat --attlib E:\reverse\e3d\shadow_e3d31_aps_all\attlib.dat --noun 0x000BFA34 --template-type SYST
```

物理描述结果：

```text
noun=DTWL
noun_hash=0x000BFA34
authoritative=true
attribute_count=0
```

`DTWL` 是合法的权威零属性 noun。允许空属性提取后，`amssys` 的 SYST 基线完成：`1205` 个索引元素写入，SYS/DICT meta 阶段成功。

## BRAN 对 E3D TTY

目标：`ams7999_0001` 中的 `BRAN 24383/73930`。TTY 输入为 e3d-io 仓库中的真实导出夹具 `tests/fixtures/bran_24383_73930_attlist_tty.log`。

先用 e3d-io 的数据库页头探针定位引用拥有者：

```powershell
cargo test --locked --test scratch_locate_ref0 -- --ignored --nocapture
```

关键结果：

```text
ref0 13245: 1 file(s)
    AvevaMarineSample\ams000\ams5053_0001
```

对照命令：

```powershell
D:\Rust\target\release\e3d-descriptor.exe extract-complete `
  --db D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7999_0001 `
  --desvir E:\reverse\e3d\shadow_e3d31_aps_all\desvir.dat `
  --attlib E:\reverse\e3d\shadow_e3d31_aps_all\attlib.dat `
  --refno 24383/73930 `
  --tty-log D:\work\plant-code\old\vendor\e3d-io\tests\fixtures\bran_24383_73930_attlist_tty.log `
  --template-type DESI `
  --catalog-layout D:\work\plant-code\old\vendor\e3d-io\schema\e3d31\catalog\noun_layout.json `
  --catalog-fields D:\work\plant-code\old\vendor\e3d-io\schema\e3d31\catalog\noun_attr_fields.json `
  --reference-db-list D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams5053_0001
```

结果：

```text
tty_row_count=85
tty_unique_count=85
comparison_counts.equivalent=85
omitted_row_count=0
mismatch=0
source_counts={binary_explicit:7,binary_implicit:40,catalog_default:29,runtime_derived:6,structural:3}
```

三个跨库引用也与 TTY 相同：

| 属性 | 二进制 RefNo | e3d-io 解析名 | TTY |
|---|---|---|---|
| Hstube | `13245/846612` | `/VMB1/CPP00102:P,50` | `/VMB1/CPP00102:P,50` |
| Pspec | `13245/846025` | `/VMB1` | `/VMB1` |
| Ispec | `13245/918008` | `/I100-HL` | `/I100-HL` |

开启 UDA 后，最小 DICT 集合为：

```text
ams5100_0001
acp7006_0001
scb6002_0001
```

`ams5100` 提供项目 UDA；`acp7006` 提供 `PFILoose/PFILExcess/PFIAExcess/PFConsChk`；`scb6002` 提供 TTY 中两个同名但不同定义的 `SCHrefHole`。完整对照结果：

```text
tty_row_count=115
tty_unique_count=112
uda_enabled=true
deferred_uda_row_count=0
comparison_counts.equivalent=115
omitted_row_count=0
mismatch=0
```

该结果已经固化为 e3d-io 的真实 CLI 回归测试 `tests/e3d-descriptor.rs::bran_binary_and_dictionary_attributes_match_the_e3d_tty_capture`。字典集合属于 TTY 运行时 MDB 身份的一部分，后续样本必须显式记录，不能按目录中的“最新”或同名库猜测。

## BRAN 成员序对 E3D TTY

同一目标 `BRAN 24383/73930` 由 `DirectTreeService` 从 e3d-io 直读，并在同一只读 E3D TTY 会话执行 `Q MEMBERS`。证据目录为 `docs/evidence/2026-09-04-bran-24383-73930-members/`：

```text
direct_nodes=18
parents_compared=18
mismatch_count=0
direct_exit_status=0
tty_exit_status=0
```

根 BRAN 的 17 个直接成员逐槽相同；带 RefNo 的成员按 RefNo 比较，命名成员按 noun 与名称比较。TTY 原文已固化为 `tests/fixtures/bran_24383_73930_members_tty.log`，回归测试 `tests/direct_index_contract.rs::bran_members_match_the_e3d_tty_capture_in_order` 已通过。

## 最大 CATA 库对 E3D TTY

`/ALL` 声明的最大库为 `CATA 7351`（`ams7351_0001`，1,238,308,864 bytes）。e3d-io 的 `(noun, layout)` 采样器从中选出 10 个桶；首个样本为 `CATE 23735/3`。`e3d_tty_attlist_capture` 在只读 E3D TTY 会话导出完整 `ATTLIST`，e3d-io 以该库、`catvir.dat`、`attlib.dat` 及 MDB 顺序的六个 DICT 库对账：

```text
tty_row_count=46
tty_unique_count=37
comparison_counts.equivalent=46
omitted_row_count=0
deferred_uda_row_count=0
mismatch=0
exit_status=0
```

原始 TTY 与 JSON 报告在 `docs/evidence/2026-09-04-cata7351-cate-23735-3/`；TTY fixture 和命令行回归已固化为 e3d-io 的 `tests/fixtures/cate_23735_3_attlist_tty.log` 与 `tests/e3d-descriptor.rs::cata_binary_and_dictionary_attributes_match_the_e3d_tty_capture`。

## 最大 DICT 库对 E3D TTY

最大可读 DICT 库 `ams5100_0001`（169,984 bytes）的采样覆盖 9 个 noun 桶；首个样本为 `UDA 13292/58`。首轮 45 行裁决暴露两项差异：`TUNIt` 错取静态 `unset`，`Udname` 缺失。TTY 证明前者应从物理 `UUNI=NONE` 回退，后者应从物理 `NAME=/FQ56/UDASUP/VISIBLE` 的末段派生。e3d-io 补齐两个 runtime evaluator 后结果为：

```text
tty_row_count=45
tty_unique_count=45
comparison_counts.equivalent=45
omitted_row_count=0
deferred_uda_row_count=0
mismatch=0
exit_status=0
```

原始 TTY 与修复后报告在 `docs/evidence/2026-09-04-dict5100-uda-13292-58/`；e3d-io 回归 `tests/e3d-descriptor.rs::dictionary_runtime_attributes_match_the_e3d_tty_capture` 同时钉住 `uda_default_unit_falls_back_to_dimension` 与 `uda_name_from_element_name` 两个 evaluator。

## SYST 世界对 E3D TTY

`amssys` 的物理采样覆盖 42 个 noun 桶。`MDB 24575/1308` 与已知的 `DTWL 24575/13` 都不能从 `/ALL` 按 RefNo 导航；系统世界 `WORL 24575/3` 可导航，并包含一个项目 UDA。以 `amssys + sysvir.dat + attlib.dat + DICT` 对账结果为：

```text
tty_row_count=5
tty_unique_count=5
comparison_counts.equivalent=5
omitted_row_count=0
deferred_uda_row_count=0
mismatch=0
exit_status=0
```

原始 TTY 与 JSON 报告在 `docs/evidence/2026-09-04-syst-worl-24575-3/`，回归固化为 e3d-io 的 `tests/e3d-descriptor.rs::system_world_attributes_match_the_e3d_tty_capture`。不可导航的内部元素没有混入外部裁决。

## 未关闭边界

- 本样本的普通属性、引用名、UDA 和 e3d-io 直读成员序已闭合：115 个 TTY 属性行全部等价，18 个父节点成员序差异为 0。
- CATA 已有一组 46/46 的 TTY 外部裁决。
- DICT 已有一组 45/45、SYST 世界已有一组 5/5 的 TTY 外部裁决。e3d-io 对 1,067 个项目目录文件的头扫描未发现物理 `GLB/GLOB`，实际全局类型为 `COMM/MISC`；两类已按 `comvir/misvir + attlib` 解码并修复纳入 Meta，但 Design `/ALL` 不能导航其 refno，TTY 属性裁决仍保持未关闭。另需验证持久化后的 `pe_owner` 边。
- `DTWL 24575/13` 是内部 SYST 元素，不能从 E3D `/ALL` 导航，因此其结论只属于 e3d-io 物理读取与 SYST 持久化闭合，不冒充 TTY 外部等价。
