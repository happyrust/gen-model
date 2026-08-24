# GWALL NonZero 轮廓与 Manifold 布尔闭环（2026-08-24）

## 输入

- 数据库：AMS 1112（本机 8009，`DB_OPTION_FILE=DbOption`）。
- RVM：`test_data/rvm/1RS-WF03-W-C-RR001.rvm`
  - SHA-256 `70aad6f8644a68c1951fbab7ee9611ebb42035decc30d6832bb5d1e077b98fc0`
- 现场目标：RVM 三件 `17496/105828`、`17496/105880`、`17496/116569`；另含
  生成日志里已进入 `bad_bool` 的 `17496/105691`、`17496/116713`。
- 基线源码：`935074ba`，本记录对应其后的未发布候选差异。

## 根因与修复

1. `17496/105880` 的三个大 FRADIUS 令离散轮廓自交。原始侧壁与 earcut 端盖使用
   不同边界，落盘网格开口并在生产 ingest 报 `NotManifold`。
2. NonZero 分区会产出多个互相重叠的正轮廓。直接拼接分量会留下实体内部表面；现按
   原始 libgm span 恢复每条边的光顺组，再用属性传播 Manifold union 消除内部面。
3. Manifold 适配层曾忽略旧 `more_precision` 两档坐标栅格。普通索引网格恢复“正体向零
   截到 0.1mm、负体四舍五入到 0.01mm”；已完全展开属性顶点的 NonZero/Manifold
   网格保持 f64 精确坐标，且同一布尔组正负两侧必须使用同一档。
4. 定向 `replace_exist=true` 原先仍被 `!bad_bool` 挡住。现可复活坏行，成功时原子清除
   `bad_bool`。`CataCrossNeg` 同时承认直接 `neg_relate` 与 `ngmr_relate` 白名单来源。

## 验证记录

### 纯函数与构建

```text
cargo +nightly-2026-08-02 test --locked --lib fast_model::manifold_tessellate::tests \
  --no-default-features --features ws,gen_model,manifold,project_hd -- --test-threads=1
35 passed; 0 failed; exit 0

cargo +nightly-2026-08-02 test --locked --lib fast_model::manifold_csg::tests \
  --no-default-features --features ws,gen_model,manifold,project_hd -- --test-threads=1
8 passed; 0 failed; exit 0

cargo +nightly-2026-08-02 test --locked --lib fast_model::manifold_bool \
  --no-default-features --features ws,gen_model,manifold,project_hd -- --test-threads=1
8 passed; 0 failed; 1 ignored; exit 0

cargo +nightly-2026-08-02 check --locked --no-default-features \
  --features ws,gen_model,manifold,project_hd,http_api
exit 0
```

### 强制重生成与 Required 布尔

```text
DB_OPTION_FILE=DbOption cargo +nightly-2026-08-02 test --locked --lib \
  live_8009_regenerate_extreme_fillet_gwall_and_boolean \
  --no-default-features --features ws,gen_model,manifold,project_hd,http_api \
  -- --ignored --nocapture
1 passed; 0 failed; exit 0
```

生成资产 SHA-256：

- `17496_105828_716.mesh`: `0788db883cb6691c9ed86107c740de930aee56add3533e97b20dbb0be5429dc6`
- `17496_105880_716.mesh`: `d2c87e3a8019f0baedebdd2f8f72d809786b053865d7c855db5607cef0a2a9fc`
- `17496_116569_716.mesh`: `e5cdc05072499c5c0d840f2ffbe7eaf43dfeb65e2ed4f6890a8a006fae170283`
- `17496_105691_716.mesh`: `aa14d00a9bebf140f4162640a582ccd938b3028bbd66a3756c3fd9a88ab5ef55`
- `17496_116713_716.mesh`: `96803533a1bad584a15151a51df5b21aba986a962ec7036de99cbd2bf901b3cf`

重算后五行均为 `booled=true, bad_bool=false` 且 `booled_id` 指向上述可读文件。

### RVM 门

```text
mesh_gwall_extra_against_cwall_union
17496/105828: NXTR=4,  gen->GWALL p95=0.1mm
17496/105880: NXTR=5,  gen->GWALL p95=9.3mm
17496/116569: NXTR=8,  gen->GWALL p95=167.5mm
1 passed; 0 failed; exit 0

mesh_gwall_union_surface_distance
rvm=20/20 gen=20/20 missing=[]
gen->rvm mean=3.86mm p95=8.06mm max=527.87mm
盒状（≤16 三角）硬门全部通过；exit 0
```

`105828` 的新旧布尔网格额外做了双向 16,000 点表面核对：候选→已知通过资产
`p95=0.0388mm/max=0.1040mm`，反向 `p95=0.0383mm/max=0.1013mm`。
