# 新 Issue 草稿（e3d-model 或 gen-model，待用户发布）

## 标题

PJOINT / SCOJOINT 节点环件几何未生成：7999 共 1,427 件 only_baseline（附零散 2 件）

## 正文

`#85` 收尾抽样 `verification/ams-rvm-20261030` 发现：基准 RVM 里 PJOINT/SCOJOINT 记录**带真实几何**（管架/框架节点上的八角环/法兰件），而生成侧整个库不产出这两类记录——属 only_baseline 口径，不在 off 统计内，但几何确实缺失。

### 证据

- `rvm_facets_dump` 抽样 `ams7999_24383_80211.rvm`：
  - `PJOINT 1 of PNODE 1 of FRMWORK /1RX02-LSUP-1023-FRMW1`：FacetGroup，bbox ~133×157×200 mm，八角环带内孔。
  - 80211 全量：PJOINT 1,874 组中 **894 带几何**；SCOJOINT 922 组中 **533 带几何**——与 compare.json 的 only_baseline 数（894/533）完全吻合，即 only_baseline 恰好全是带几何的那批。
- `compare.json` by_noun：`PJOI both=0`、`SCOJ both=0`——生成侧零产出。
- 零散件：73927 的 `CTORUS 9 of SUBEQUIPMENT 21`（同组 13 件漏 1）、`PYRAMID 1 of SUBEQUIPMENT 21`——单件缺失，原因待查。
- BEND 6 件 only_baseline 是 ~10mm 微 torus，暂归微缺失，可并入本 issue 或单列。

### 初步判断

- 关节类（PJOI/SCOJ）目前只走「切割负体」逻辑，没有「关节自身出几何」的路径；E3D 对这两类节点写了实体环件（类似 SJOI 出几何的对应面）。
- 待查：环件参数来源（节点类型？PNO/PNOD 的属性？catalogue？），以及其它库（非 7999）是否同型缺失——7356 的 SJOINT 141 件已由 `78aeef6` 补上，PJOINT/SCOJOINT 可能同型。

### 规模

- 7999：1,427 + 2 = 1,429 件。
- 其它库未普查；建议先跑一遍全语料 PJOINT/SCOJOINT 带几何普查再定修复范围。

### 验收建议

- 7999 的 894+533 件关节环与基准逐件对上（或逐件归因）。
- 零散 2 件查明后并入或单独立项。
