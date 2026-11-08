# G3 规则组 E3D 对照（只读）

产出 `tests/fixtures/group-contract/e3d-rule-groups.json`：E3D 自己那套求值口径对 APS 27 条
规则组的**真值**，供 `web_service::handlers::tests::live_rule_groups_*` 逐条比对。

E3D 侧不能从 Rust 测试里驱动（`des.exe` 会话要串行、单次 6-13 秒、`l3_suite` 是独立二进制），
所以走「先跑宏、再把日志固化成夹具」这条路。

## 一、跑两个只读宏

```powershell
cd D:\work\plant-code\old\gen-model
$env:L3_E3D_INSTALL_DIR = "E:\reverse\e3d\shadow_e3d31_gen_model_test"

# 1) 规则原文（Q ATT）+ 求值成员（Q ALL <noun> WITH (...)）
.\target\debug\l3_suite.exe --check-driver "D:/work/plant-code/old/gen-model/scripts/e3d/g3-rule-groups/live27.mac" `
    --project-dir "D:\AVEVA\Projects\E3D3.1\AvevaPlantSample" `
    --e3d-project APS --e3d-mdb /ALL `
    --output "D:/work/plant-code/old/gen-model/output/live-rule-groups/run1"

# 2) 每个成员的 refno（Q REF）
.\target\debug\l3_suite.exe --check-driver "D:/work/plant-code/old/gen-model/scripts/e3d/g3-rule-groups/refs27.mac" `
    --project-dir "D:\AVEVA\Projects\E3D3.1\AvevaPlantSample" `
    --e3d-project APS --e3d-mdb /ALL `
    --output "D:/work/plant-code/old/gen-model/output/live-rule-groups/run2"
```

四个必知细节（`library/e3d-reference.md` §2，漏一条就跑不起来）：

1. `L3_E3D_INSTALL_DIR` **必须显式设**（默认路径本机不可用，报 `0xC0020001`）
2. 宏里的 `ALPHA LOG` 路径必须等于 `<宏文件名>.log`（同目录）
3. `--project-dir` 必须是 `D:\AVEVA\Projects\E3D3.1\AvevaPlantSample`
   （`D:\e3dio-e2e\AvevaPlantSample` 里没有 `/AREA01_PIPE`，`Q ATT` 会解析成 WORLD）
4. 宏里不许有 `QUIT` / `SAVEWORK`；无 `SAVEWORK` 走纯只读路径

## 二、固化夹具

```powershell
cd D:\work\plant-code\old\gen-model
python scripts/e3d/g3-rule-groups/build_fixture.py   # 日志 → e3d_scosel / e3d_members
python scripts/e3d/g3-rule-groups/merge_refs.py      # 加 e3d_member_refnos
```

## 三、跑对照测试

```powershell
cd D:\work\plant-code\old\gen-model
cargo test --lib live_rule_groups -- --ignored --nocapture --test-threads=1
```

`--test-threads=1` 是必须的：开关用进程级静态量，与要求「29 个 GPSET 全部 resolved=0」的
既有真库测试并发会互相干扰。

## 只读约束

宏里**没有** `SAVEWORK` / `NEW` / `DELETE` / `MOVE` / `COPY`，也**没有** PML 集合对象
（`COLL` + `Size()`，实测会让 `des.exe` 崩 `0xC0000005`）。真库
`D:\AVEVA\Projects\E3D3.1\` 全程只读。
