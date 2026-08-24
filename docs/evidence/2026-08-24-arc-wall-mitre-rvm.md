# 弧墙工作切面与 RVM 收口证据（2026-08-24）

## 输入与权威行为

- RVM：`test_data/rvm/1RS-WF03-W-C-RR001.rvm`
- SHA-256：`70aad6f8644a68c1951fbab7ee9611ebb42035decc30d6832bb5d1e077b98fc0`
- Core3D 3.1 `DB_Gensec::do_solid_segments`：`0x10732FF0`
- Core3D 3.1 `setMitrePlanes`：`0x107368A0`
- aios-core：`1de7a94e8f60c0f106f2f59f0805d25e262abaeb`
- parse/pdms：`05dde0d740b7cc48cfeaf101f069e4ee9ebfb10c` /
  `a8f16214576ca9f16892b1576b7917ada7388ca8`

现场源属性（8009，只读查询，退出码 0）：

```text
SPINE:17496_105941 DRNS=[1,0,0] DRNE=[0,0,0] YDIR=[0,0,1]
POINSP:17496_105942 POS=[-5058.219,-16648.557,0]
CURVE:17496_105943 CURTYP=THRU POS=[-3909.413,-16955.131,0] RADI=17400
POINSP:17496_105944 POS=[-2742.352,-17182.535,0]
```

SPINE 三点只定义 7.83° 的中心线；起点 `DRNS=+X` 与起始切向不垂直，因此 libgm
语义不是径向端盖，而是延伸整个回转截面后按 `x = -5058.219` 工作平面裁切。内半径
与该平面的交点把可见墙体起角扩到 −108.31°，这正是 RVM 的 9.24° 范围。

## 实现与纯函数门

- Arc3D 的 DRNS/DRNE 与 center/start/axis 一起保留源坐标系；直线 SPINE 继续使用
  既有段局部坐标。
- 回转路径逐次扩角，直到所有离散截面点越过工作平面，并保留 1mm 余量；随后通过
  `trim_by_plane` 裁回。90° 内仍覆盖不了切面时响亮失败。
- `field_arc_wall_start_mitre_matches_rvm_bounds` 使用现场字面参数，验证世界 AABB。
- `sweep_mesh::tests`：37/37 通过，退出码 0。

## 现场重铺与 RVM 门

强制重铺（退出码 0）：

```text
cargo +nightly-2026-08-02 test --locked --lib \
  live_8009_regenerate_cwall_rr001_wall_meshes \
  --no-default-features --features ws,gen_model,manifold,project_hd,http_api \
  -- --ignored --nocapture
test ... ok
```

定向生成后的新关系参数：

```text
inst_geo=1241783102909912318 drns=[1,0,0] drne=null angle=0.13669491
```

无 OCC RVM 门（退出码 0）：

```text
cargo +nightly-2026-08-02 test --locked --lib mesh_wall_surface_distance \
  --no-default-features --features ws,gen_model,manifold,project_hd,rvm_verify \
  -- --ignored --nocapture
WALL 4: gen->rvm mean=1.40 p95=4.05 max=26.55
        rvm->gen mean=9.37 p95=4.28 max=648.40
        rvm angle=[-108.31,-99.07] gen angle=[-108.31,-99.07]
test ... ok
```

WALL 1–4 的 gen→RVM p95 分别为 `7.86 / 7.84 / 8.63 / 4.05mm`，全部低于
12mm 硬门；WALL 4 不再是只打印的例外。
