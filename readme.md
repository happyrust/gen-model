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
