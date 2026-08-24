# OCC 退役双副本 RVM 执行记录（2026-08-24）

## 输入与隔离

- 源码基线：`efb425a1affbd30c6127c11a268596692cec64c2`，本记录包含其后的候选差异。
- 较大库来源：`.surreal/ams-7997-e3d-test-20260805` 的独立工作副本
  `.scratch/occ-census-7997-20260824`；源目录未打开。
- 服务端：仓库脚本 `scripts/Start-Surreal8009.ps1`，锁定二进制
  `2.1.4+20250317.45013fc9`，副本监听 `127.0.0.1:8039`。
- 副本 mesh 目录：`.scratch/meshes-8039`，不与 8009 共用。
- RVM：`test_data/rvm/1RS-WF03-W-C-RR001.rvm`，SHA-256
  `70aad6f8644a68c1951fbab7ee9611ebb42035decc30d6832bb5d1e077b98fc0`。

RVM live 测试新增两个显式覆盖项，默认行为仍指向 8009：

```text
AIOS_RVM_DB_ENDPOINT=ws://127.0.0.1:8039
AIOS_RVM_MESH_DIR=.scratch/meshes-8039
DB_OPTION_FILE=.scratch/DbOption-8039
```

## 生成根重建

执行：

```text
cargo +nightly-2026-08-02 test --locked --lib \
  --no-default-features --features ws,gen_model,manifold,project_hd,rvm_verify \
  live_regenerate_cwall_rr001_from_generation_root -- --ignored --nocapture
```

字面结果：`1 passed; 0 failed`，退出码 0；生成根 `17496/105799` 产生 20 个 GWALL
生产关系，`inst_geo` 从 8,094 增至 8,115。三件大体量布尔网格与 8009 确定性一致：

| mesh | SHA-256 |
|---|---|
| `17496_105828_716.mesh` | `0788db883cb6691c9ed86107c740de930aee56add3533e97b20dbb0be5429dc6` |
| `17496_105880_716.mesh` | `d2c87e3a8019f0baedebdd2f8f72d809786b053865d7c855db5607cef0a2a9fc` |
| `17496_116569_716.mesh` | `e5cdc05072499c5c0d840f2ffbe7eaf43dfeb65e2ed4f6890a8a006fae170283` |

## 8039 RVM 结果

```text
mesh_gwall_union_surface_distance
rvm=20/20 gen=20/20 missing=[]
gen->rvm mean=2.41mm p95=4.14mm max=509.32mm
rvm->gen mean=8.30mm p95=7.84mm max=647.13mm
1 passed; 0 failed; exit 0

mesh_gwall_extra_against_cwall_union
17496/105828: NXTR=4, p95=0.1mm
17496/105880: NXTR=5, p95=9.3mm
17496/116569: NXTR=8, p95=167.5mm
1 passed; 0 failed; exit 0
```

同一候选在默认 8009 重新执行 `mesh_wall_surface_distance` 与
`mesh_stwall_surface_distance`，两项均退出 0：四堵弧墙 gen→RVM p95 为
`7.86/7.84/8.63/4.05mm`，四堵直墙双向 p95 均为 0。

## 未闭合门

7997 副本的生产生成根只生成 20 个 GWALL；该副本没有 8 条历史测试专用
WALL/STWALL `inst_relate`。因此两项测试响亮失败于“目标库没有生成几何”，没有把
缺行当作距离通过。T046–T048 目前状态是：8009 的 WALL/STWALL 门通过、两个副本的
GWALL 门通过，7997 的 WALL/STWALL 测试资产仍需按源属性建立后再复验。

SPHE 现场样本仍被当前 E3D 交互会话占用挡住；本次没有终止或复用该会话，T049 与最终
发布门继续保持未完成。
