# STWALL 直接扫掠坐标与无 OCC RVM 证据（2026-08-24）

## 输入

- RVM：`test_data/rvm/1RS-WF03-W-C-RR001.rvm`
- SHA-256：`70aad6f8644a68c1951fbab7ee9611ebb42035decc30d6832bb5d1e077b98fc0`
- 基线主仓 commit：`4dcd52a9215e6fa109f668573412a0ca57eda8b0`
- aios-core 修复：`6f95b65adcd485e00d3bdaae1f4391f8d288e320`
- parse/pdms 对齐：`20fbac4be632cd7469019101562701b8cc72841d` /
  `53d9fb3b0dfb0d7cf086568f9bd6ebadff970c75`

## 行为结论

Core3D 3.1 `setSpineSegmentTransforms`（`0x10737340`）把规范 +Z 映射到曲线起始切向；
但直接 `POSS/POSE` 元素的同一路径方向已经被 `get_world_transform` 写入元素变换。
因此非 SPINE 的单位 SweepSolid 实例不得再次应用路径切向或 BANG，只消费长度、PLAX
和镜像。显式 SPINE 段继续保留 libgm 的分段局部变换。

现场反例 `pe:17496_105816` 的路径是 +X，元素四元数 `[0.5,0.5,0.5,0.5]` 已把规范
+Z 映射为 +X。旧实例又执行一次 +Z→+X，组合后错误落到 +Y；修复后只由元素变换定向。

## 验证记录

依赖图命令（退出码 0）：

```text
cargo +nightly-2026-08-02 tree -i aios_core --depth 2
aios_core ... rev=6f95b65...  # 单份
```

无 OCC 构建（退出码 0）：

```text
cargo +nightly-2026-08-02 check --locked --no-default-features \
  --features ws,gen_model,manifold,project_hd,http_api
Finished `dev` profile
```

定向生成（退出码 0）：

```text
AIOS_PROBE_ROOT=17496/105799 AIOS_PROBE_DBNUM=1112 AIOS_PROBE_FORCE=1
cargo ... --test gen_one_root_probe generating_one_root_fills_geometry_aabb_and_tree ...
[probe] 生成结果: root=17496/105689 status=Generated 可渲染=198 已写入=153
[probe] 生成后: inst_relate=5613 其中有几何=5613 有包围盒=5609，空间树=8188
test ... ok
```

RVM 门（退出码 0）：

```text
cargo +nightly-2026-08-02 test --locked --lib mesh_stwall_surface_distance \
  --no-default-features --features ws,gen_model,manifold,project_hd,rvm_verify \
  -- --ignored --nocapture
STWALL 1: gen->rvm p95=0.00 max=0.03 | rvm->gen p95=0.00 max=0.03
STWALL 2: gen->rvm p95=0.00 max=0.06 | rvm->gen p95=0.00 max=0.06
STWALL 3: gen->rvm p95=0.00 max=0.00 | rvm->gen p95=0.00 max=0.00
STWALL 4: gen->rvm p95=0.00 max=0.00 | rvm->gen p95=0.00 max=0.00
test ... ok
```

STWALL 4 字面 AABB：

```text
rvm=[-1300.0,-17201.37,-20.0]..[1300.0005,-17001.367,230.0]
gen=[-1300.0,-17201.37,-20.0]..[1300.0,-17001.37,230.0]
```
