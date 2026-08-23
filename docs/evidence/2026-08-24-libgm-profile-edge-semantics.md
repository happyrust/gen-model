# libgm 轮廓硬边语义逆向证据（2026-08-24）

## 输入与完整性

| 二进制 | SHA-256 | IDA 数据库 |
|---|---|---|
| E3D 3.1 `libgm.dll` | `d07129147d22acaed69424ef93763a849b7e7502a40db73fc71426ec4fd980fd` | `idalib-18608` |
| E3D 2.10 `libgm.dll` | `54a18311cf65b976feeadd18f56f6fba479443bf224cb93072c900c9cd3157eb` | `idalib-26688` |
| E3D 3.1 `libgeom.dll` | `d48e3be5f587173f9af1d7578418ed3495c277f94a12fe071807a16f0f64f8f9` | `idalib-21956` |
| E3D 2.10 `libgeom.dll` | `347f83b1fb109b217b6fd461f06566c840f378e7f0142871ca58ea5c82f678de` | `idalib-19544` |

哈希命令均为 `certutil -hashfile <binary> SHA256`，退出码均为 0。

## 结论

### `GM_Profile::getPolygonForFacet`

- 3.1：`0x1008F8B0`；2.10：`0x10059BA0`。
- 签名为 `GM_Profile::getPolygonForFacet(D2_Polygon&, FL_vector<int>&)`。
- 每个非退化 span 调用按步数展开的折线函数，并把该 span 新增的点数写进并行整数数组。
- 当前 span 不能光顺接到下一 span 时，对应点数取负；闭环不光顺时首尾两项都取负。
- 因而整数绝对值是点段长度，符号是硬边标志，不是临时控制量。

### `D2_Span::leadsSmoothlyTo`

- E3D 3.1 `libgeom.dll`：`0x10029B50`。
- 分别取得当前 span 末切向与下一 span 首切向，返回
  `abs(1.0 - dot(t_last, t_first)) <= 0.000001`。
- 直线切向是规范化端点差；圆弧切向由圆心、半径与 bulge 方向得到。

### 消费者

- `GM_Collar::calcFacetsWithoutSurfaces`：3.1 `0x10048500`。
- `GM_Revolution::calcFacetsWithoutSurfaces`：3.1 `0x10097920`。
- 两者用绝对值推进 span 内边段，并用负号选择硬边类型；负号会进入 facet edge 分类，不能在
  折线展开后丢弃。

### `GM_Facets::addEdge`

- 3.1 `0x1005CCE0`：四参数重载；`0x1005CDA0`：带双 facet 索引的重载。
- `GM_Edge` 的类型字段存于对象第 6 个 `DWORD`；`isCurve` 只认值 4，`isVisible` 排除
  1、5、6。调用者传入的硬边类型必须原样保留到 edge 对象。

## 实现约束

轮廓离散结果必须同时携带点列和每条出边的光顺标志；反转绕向时两者同步重排。最终
`PlantMesh` 不新增字段，硬边以同位置拆顶点和不同法线表达。Manifold ingest 只焊接几何
拓扑，属性顶点继续保留光顺分裂。
