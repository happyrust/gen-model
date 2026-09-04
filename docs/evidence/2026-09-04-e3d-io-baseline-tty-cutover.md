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

## 未关闭边界

- 本次 `--uda` 为关闭状态；TTY 中 30 个 UDA 行被明确记为 deferred，不计入 85 行普通属性对照。
- 仍需按 B8.5 扩大到 DESI/SYST/DICT/CATA/GLB-GLOB 五类库样本，并单列 UDA 对照。
- `DTWL 24575/13` 是内部 SYST 元素，不能从 E3D `/ALL` 导航，因此其结论只属于 e3d-io 物理读取与 SYST 持久化闭合，不冒充 TTY 外部等价。
