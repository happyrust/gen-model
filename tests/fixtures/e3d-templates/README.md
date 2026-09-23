# E3D schema 资产随行副本（attlib + DESI 模板）

离线夹具档（`db8000_two_delete_fixture` / `db8000_session_pairs` /
`baseline_stream_8000` / `read_through_rows_parity`）经 e3d-io 读 dabacon 文件时
要两件 schema 资产：

| 文件 | 作用 | 来源 |
|---|---|---|
| `attlib.dat` | 属性字典（`DbSet::with_attlib_file`） | E3D 3.1 安装目录 |
| `desvir.dat` | DESI 库模板（`template_file_for(dir, "DESI")` → 前三字母小写 + `vir.dat`） | 同上 |

`src/data_interface/direct_store.rs` 的 `AIOS_E3D_TEMPLATE_DIR` 指到本目录即可；
CI（`.github/workflows/windows-tests.yml`）全局这么指，从此不依赖 runner 上有
E3D 装机。本地开发机若有完整 shadow 目录，仍可指向它——本目录只是最小子集：
仓内夹具全是 db8000（DESI）库，所以只需要 `desvir.dat` 一个模板；将来收录别的
db_type 夹具（如 CATA）时，把对应的 `<前三字母>vir.dat` 一并放进来并补记
`SHA256SUMS`。

抽取自 2026-09-09 的 E3D 3.1 shadow 副本（`shadow_e3d31_aps_all`），与录制
issue-019 夹具时使用的同一套装机。散列见 `SHA256SUMS`；文件是 AVEVA 发行物的
原样字节，不做任何改写。
