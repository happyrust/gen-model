# OCC 退役：无 OCC 发布产物验证

日期：2026-08-24  
源码 commit：`dee934f525adab0ec2ba84e6a9810fa07e7274ca`  
工具链：`nightly-2026-08-02`  
local deps：`OFF`

## 结论

过渡发布构建已经在不启用任何 OCC feature 的口径下成功生成。Cargo 完整依赖图、PE
导入表和发布二进制字符串均未发现 OpenCascade/OCCT；Python 离线档和三条源码删除护栏
通过。这证明最终源码可以产出无 OCC 的 Windows release 资产，但不替代 T049 现场曲面
RVM 与 caliber 原子重建门，因此当前不宣称最终发布完成。

## 1. Release 构建

```powershell
cargo +nightly-2026-08-02 build --release --locked --bin aios-database `
  --no-default-features --features ws,gen_model,manifold,project_hd,http_api
```

```text
Finished `release` profile [optimized] target(s) in 7m 51s
exit 0
D:\Rust\target\release\aios-database.exe
size = 95131648 bytes
sha256 = e95579694ff50bcda48eeddb718fea58159e9773a36bb26a2527115acbceb742
```

## 2. Cargo 依赖与 feature 删除门

```powershell
cargo +nightly-2026-08-02 metadata --locked --format-version 1
```

对全部 package 名称和 feature 键筛查，字面结果：

```text
packages= 826
bad_packages= []
occ_features= []
exit 0
```

发布依赖来自固定 git revision，且只有一份 `aios_core`：

```text
aios_core 0.2.0 @ 1de7a94e8f60c0f106f2f59f0805d25e262abaeb
parse_pdms_db 3.0.0 @ 05dde0d740b7cc48cfeaf101f069e4ee9ebfb10c
pdms_io 0.1.0 @ a8f16214576ca9f16892b1576b7917ada7388ca8
local deps patch: OFF (all three crates come from their git sources)
```

筛查拒绝 `opencascade`、`opencascade-sys`、`occt-rs` 包名及名为 `occ` 的 feature。

## 3. Windows PE 导入与二进制内容

使用 Python `pefile 2024.8.26` 读取发布 PE 的 import directory，并扫描
`opencascade`、`opencascade-sys`、`occt-rs`、`TKernel.dll`、`TKBRep.dll`：

```text
imports=['MSVCP140.dll', 'VCRUNTIME140.dll', 'VCRUNTIME140_1.dll',
'advapi32.dll', 'api-ms-win-core-synch-l1-2-0.dll',
'api-ms-win-crt-environment-l1-1-0.dll', 'api-ms-win-crt-heap-l1-1-0.dll',
'api-ms-win-crt-locale-l1-1-0.dll', 'api-ms-win-crt-math-l1-1-0.dll',
'api-ms-win-crt-runtime-l1-1-0.dll', 'api-ms-win-crt-stdio-l1-1-0.dll',
'api-ms-win-crt-string-l1-1-0.dll', 'api-ms-win-crt-time-l1-1-0.dll',
'bcrypt.dll', 'bcryptprimitives.dll', 'kernel32.dll', 'ntdll.dll',
'oleaut32.dll', 'pdh.dll', 'powrprof.dll', 'psapi.dll', 'shell32.dll',
'ws2_32.dll']
occ_imports=[]
occ_strings=[]
exit 0
```

## 4. 删除护栏与 Python

```powershell
cargo +nightly-2026-08-02 test --locked --lib --no-default-features `
  --features ws,gen_model,manifold,project_hd `
  fast_model::occ_retirement_guard -- --nocapture
```

```text
3 passed; 0 failed; 0 ignored; 1188 filtered out
exit 0
```

```powershell
Set-Location python
.venv\Scripts\python.exe -m pytest -m offline -q
```

```text
85 passed, 23 deselected in 5.83s
exit 0
```

## 5. 尚未解除的发布硬门

- T049 仍缺 SPHE 的 E3D/RVM 现场基准；YOFF Snout、SSCL、多面体已有源样本和生产网格证据。
- 8009/7997 的 caliber 预检仍分别报告 29/392 个缺 caliber 的复用身份，尚未进入维护窗口。
- 因而本记录只关闭“源码能生成无 OCC release 且产物不携带 OCCT”的门，不关闭最终部署门。
