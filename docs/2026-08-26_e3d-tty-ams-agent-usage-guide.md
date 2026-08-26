# E3D TTY 与导出使用教程（AMS 示例，供 Agent 执行）

本文说明当前仓库里 E3D TTY、RVM/ATT 导出和 noun 属性字典导出的实际用途、调用链和
验收方法。目标不是“启动一个 E3D 进程”，而是执行可审计的 PML/CAF 操作，并把验证
推进到 dabacon 语义窗口、业务恢复、服务水位，或导出文件的内容与可回滚发布。

## 1. 一句话口径

对 AMS 8000 做增量测试时，优先运行：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\e3d\Test-TtyNetWindow.ps1 `
  -L3Exe D:\Rust\target\debug\l3_suite.exe
```

这个脚本会依次完成：保存基线副本 → TTY 执行 apply 宏 → 解析
`parse.net_window` → TTY 执行 restore 宏 → 验证目标业务属性恢复 → 验证 apply 与
restore 的合并窗口业务净零。restore 位于 `finally`，apply 后面的断言失败也会尝试恢复。

**不要把 `des.exe` 进程存在、宏日志存在或文件 SHA 改变当成通过。** E3D 每次
`SAVEWORK` 都会追加会话，恢复后的 dabacon 通常不会逐字节等于基线；回滚判据是目标
元素的业务属性回到基线，且合并语义窗口不再包含目标业务变化。

## 2. 当前调用链

```text
Test-TtyNetWindow.ps1
  └─ python/.venv/Scripts/python.exe（在本轮证据目录生成并执行临时 runner）
      ├─ aios_db.parse.header / element / net_window（只读 dabacon）
      └─ l3_suite.exe --check-driver
          └─ scripts/e3d/run_ams_c_entrymacro.bat
              └─ launch_detached.ps1
                  └─ des.exe -tty AMS SYSTEM/XXXXXX /ALL
                      └─ AVEVA_DESIGN_ENTRYMACRO=$M "<macro>"
```

这条入口宏通道是当前权威路径：`Startup.dll` 在 E3D 事件循环可用后执行宏。不要改走
stdin、控制台按键注入或 GUI 坐标点击；现有实现就是为了避开这些不稳定通道。

通用 Python 封装位于：

- `scripts/python/gen_model_testing/e3d_tty.py`：`E3dTtyRunner`，负责复制并规范化宏、
  重定向 `ALPHA LOG`、移除宏中的 `QUIT`/`FINISH`，再调用 Rust driver。
- `scripts/python/gen_model_testing/rust_tools.py`：`RustTools.run_l3_driver()`，负责设置
  `L3_E3D_DRIVER` 与 `L3_E3D_INSTALL_DIR` 并执行 `l3_suite --check-driver`。
- `scripts/python/run_db8000_increment.py`：需要组合自定义宏、净窗口探针和服务 API 时的
  编排入口。

对于已经固化的 AMS FTUB 往返用例，直接用 `Test-TtyNetWindow.ps1`，不要重新拼装
`E3dTtyRunner`。

## 3. AMS 示例在改什么

默认目标如下：

| 项目 | 值 |
|---|---|
| E3D 项目 / MDB | `AMS /ALL` |
| 项目目录 | `D:\AVEVA\Projects\E3D3.1\AvevaMarineSample` |
| dabacon | `...\ams000\ams8000_0001` |
| dbnum | `8000` |
| 元素 | FTUB `24384/23262`（脚本参数写作 `24384_23262`） |
| apply | `POS = E 10887, N 12332, U 3400 mm` |
| restore | `POS = E 10887, N 12332, U 2900 mm` |

宏为：

- `scripts/e3d/db8000_bran_ftub_move_apply.mac`
- `scripts/e3d/db8000_bran_ftub_move_restore.mac`

每条宏必须恰好一次 `SAVEWORK`，不得自己写 `QUIT` 或 `FINISH`；会话生命周期归 driver
管理。restore 必须写完整终态，不要只写“反向增量”，否则它依赖运行前的隐含状态。

## 4. 运行前检查

在仓库根目录用单层 PowerShell 执行：

```powershell
$required = @(
  'python\.venv\Scripts\python.exe',
  'D:\Rust\target\debug\l3_suite.exe',
  'E:\reverse\e3d\shadow_e3d31_gen_model_test\des.exe',
  'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\evarsAvevaMarineSample.bat',
  'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001',
  'scripts\e3d\db8000_bran_ftub_move_apply.mac',
  'scripts\e3d\db8000_bran_ftub_move_restore.mac'
)
$missing = $required | Where-Object { -not (Test-Path -LiteralPath $_) }
if ($missing) { throw "缺少 E3D TTY 前置项：$($missing -join ', ')" }
& python\.venv\Scripts\python.exe -c "import aios_db; print('aios_db=ok')"
& D:\Rust\target\debug\l3_suite.exe --help
```

当前 `Test-TtyNetWindow.ps1` 的 l3 查找顺序是兄弟目录
`..\target\debug\l3_suite.exe`，然后回落 `D:\Rust\target\debug\l3_suite.exe`。Agent
应显式传 `-L3Exe`，让证据记录包含本轮实际二进制，不要靠猜测。

若项目、安装或登录不是默认值，使用 launcher 支持的环境变量：

```powershell
$env:L3_E3D_INSTALL_DIR = 'E:\reverse\e3d\shadow_e3d31_gen_model_test'
$env:L3_E3D_PROJECTS_DIR = 'D:\AVEVA\Projects\E3D3.1'
$env:L3_E3D_PROJECT = 'AMS'
$env:L3_E3D_MDB = '/ALL'
$env:L3_E3D_LOGIN = 'SYSTEM/XXXXXX'
$env:L3_E3D_TIMEOUT_SECONDS = '1200'
```

登录值可由现场环境覆盖；Agent 的报告中不要展开凭据。

## 5. 标准执行：TTY 写入、语义验证、恢复

给每轮单独的证据目录，且使用单层 `-File` 调用：

```powershell
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$out = "D:\work\plant-code\old\gen-model\output\e3d-tty-net-window\$stamp"

powershell -NoProfile -ExecutionPolicy Bypass -File `
  scripts\e3d\Test-TtyNetWindow.ps1 `
  -DbFile 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001' `
  -ProjectDir 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample' `
  -AiosProject 'AvevaMarineSample' `
  -Refno '24384_23262' `
  -ApplyMacro 'scripts/e3d/db8000_bran_ftub_move_apply.mac' `
  -RestoreMacro 'scripts/e3d/db8000_bran_ftub_move_restore.mac' `
  -ExpectedApplyPos '10887,12332,3400' `
  -ExpectedRestorePos '10887,12332,2900' `
  -Output $out `
  -L3Exe 'D:\Rust\target\debug\l3_suite.exe'

if ($LASTEXITCODE -ne 0) { throw "E3D TTY 用例失败，检查 $out" }
Get-Content "$out\summary.json"
```

不要把这个命令包进另一层可展开的 PowerShell 字符串；`$LASTEXITCODE` 等变量会被外层
提前展开，既破坏命令也模糊真实退出码。

通过时至少满足：

1. `sessions` 连续增加两次：baseline → apply → restore。
2. apply 和 restore 的目标操作都是 `modified`。
3. apply 后 POS 等于 `ExpectedApplyPos`。
4. restore 后 `attrs` 与 `explicit_attrs` 都等于 baseline。
5. 合并窗口中目标 FTUB 不再出现，且没有 CACHID 以外的显式业务变化。
6. `rollback_verified=true`，进程退出码为 `0`。

合并窗口仍可能保留 BRAN 的 `CACHID` 保存元数据，这是 E3D 会话写入事实，不等于
FTUB 业务回滚失败。`unchanged_rewrites` 也应保留在报告里，不能把位置换页但内容相同的
重写误报成业务变化。

## 6. 证据目录怎么读

| 文件 | 用途 |
|---|---|
| baseline DB copy | `baseline-db-file` 加 `.copy`；运行前 dabacon 副本，仅作基线与应急取证 |
| apply driver record | `apply-driver` 加 JSON 扩展名；完整命令、stdout、stderr、exit status |
| restore driver record | `restore-driver` 加 JSON 扩展名；完整命令、stdout、stderr、exit status |
| driver evidence dirs | `apply-driver` 与 `restore-driver` 两个目录；l3 driver 的分腿证据 |
| semantic window diff | `semantic-window-diff` 加 JSON 扩展名；两腿与合并窗口的语义 patch/diff |
| main summary | `summary` 加 JSON 扩展名；基线、两腿解析结果、恢复裁决和所有路径的主记录 |

Agent 报告必须给出证据目录、三段会话号、目标属性前/apply/restore 值、两腿 exit
status、合并窗口 counts、`unchanged_rewrites` 与 `rollback_verified`。不要只摘一条
`DONE` 日志。

## 7. 继续到服务端增量收口

TTY 测试通过只证明“文件写入 + 语义解析 + 业务恢复”。若任务要求验证正常服务链，
还要提交 dbnum 8000 并等待水位。先确认服务的项目、MDB、namespace、watch scope 和
文件路径确实指向同一份 AMS；不要仅凭端口可连就提交。

```powershell
@'
import json, os, sys
from pathlib import Path

repo = Path(r"D:\work\plant-code\old\gen-model")
evidence = Path(os.environ["E3D_TTY_EVIDENCE"])
sys.path.insert(0, str(repo / "scripts" / "python"))
from gen_model_testing import GenModelClient, ProjectIdentity

client = GenModelClient(
    "http://127.0.0.1:8023",
    ProjectIdentity("AvevaMarineSample", "/ALL", "1516"),
)

print(json.dumps({"health_before": client.health(), "preview": client.preview()},
                 ensure_ascii=False, indent=2))
result = client.execute([8000])
print(json.dumps({"execute": result}, ensure_ascii=False, indent=2))

# 必须取本轮 TTY 主 JSON 记录里的真实 restore 会话号，不能猜。
tty = json.loads((evidence / ("summary" + ".json")).read_text(encoding="utf-8-sig"))
expected = int(tty["restore"]["header"]["latest_sesno"])
watermark = client.wait_for_watermark(8000, expected, timeout=600)
print(json.dumps({
    "watermark": watermark,
    "queue": client.queue(),
    "tasks": client.tasks(limit=160),
    "pending_units": client.pending_units(),
    "health_after": client.health(),
}, ensure_ascii=False, indent=2))
'@ | Set-Content -LiteralPath "$out\submit-service.py" -Encoding utf8

$env:E3D_TTY_EVIDENCE = $out
python\.venv\Scripts\python.exe "$out\submit-service.py" |
  Tee-Object -FilePath "$out\service-verification.json"
```

服务端退出门：

- 8000 的 `applied_sesno >= restore.header.latest_sesno`，并与文件最新会话对齐；
- 8000 对应 task 为成功终态；
- staging 为空、side-effect pending 为 0、batch failures 为空；
- worker 存活，最终 health/initialization 达到本轮要求；
- 最后重新打开 dabacon，目标业务属性仍为 restore 值。

`watch_dbnums=[8000]` 限定的是目标数据消费范围；共享 SYST/DICT meta 任务仍可能同批
出现。报告任务数时把 8000 数据任务和共享 meta 任务分开，不要把它们误判为范围泄漏。
若 health 另报 `spatial_tree.drift`，单列为后续空间树一致性门；它不自动推翻已验证的
8000 水位，也不能被“model_ready”掩盖。

## 8. 自定义宏的最小规则

需要换目标时，复制 apply/restore 宏并同时修改脚本参数。每个宏至少具备：

```text
ALPHA LOG "<path>" OVER
$P <CASE>-ALIVE
=<真实 refno>
Q CE
Q REF
Q <待改属性>
<写入完整终态>
Q <待改属性>
SAVEWORK '<可辨识说明>'
$P <CASE>-DONE
ALPHA LOG END
```

约束：

- 目标 refno、基线值和预期值必须先由只读探针确认；不要用近似标识填充。
- apply 与 restore 各一个 `SAVEWORK`，因此预期各增加一个会话。
- restore 写完整业务终态，并在 `finally` 执行。
- 宏中不写 `QUIT`/`FINISH`。
- 用 `aios_db.parse.net_window(..., detail=True)` 校验语义，不用文本日志代替。
- 若新用例会进入自动 live 测试，按仓库规则同步更新
  `docs/2026-08-12_live-test-ledger.md` 与 `docs/evidence/`。

## 9. 常见失败与下一动作

| 现象 | 判断与动作 |
|---|---|
| `required path is missing` | 逐个核对 Python、l3、项目、dabacon 和两条宏；显式传 `-L3Exe` |
| driver 等不到 `ALIVE` | 核对项目 evars、安装目录、登录/MDB；看分腿 driver 日志，不要盲目重跑 |
| dabacon 被占用 / instance lock | 查持锁 E3D/服务进程；不要终止不属于本轮的共享会话，改用隔离项目副本 |
| apply 通过但后续断言失败 | 先确认 `finally` 的 restore 结果；恢复未证实时停止服务提交 |
| 文件 SHA 与基线不同 | 正常检查会话增长；以属性恢复和合并净窗口为准 |
| 合并窗口只剩 `CACHID` | 记录为保存元数据；目标业务变化已经抵消 |
| execute 后 preview 不再含 8000 | 任务已消费时是正常现象；改查 `/api/v1/dbnums`、task 和已落盘快照，不要重复 execute |
| 水位不前进 | 查 dbnum blocker、task、staging、side effects、batch failures 和 worker；写失败时水位本就不得推进 |

## 10. 已验证样例

2026-08-25 的 AMS 8000 实跑结果为：会话 `256 → 257 → 258`，FTUB POS.U
`2900 → 3400 → 2900`；apply/restore 各 `modified=2`，合并窗口业务净零，只剩 BRAN
`CACHID`；服务任务成功后 `applied_sesno=258`，与文件最新会话对齐。证据说明见
`docs/evidence/2026-08-25-e3d-tty-increment-update.md`。

这些数字是历史样例，不是下一轮的预期输入。Agent 每次都必须从本轮主 JSON 记录
读取 baseline 与 restore 会话号，再决定服务提交的 `expected`。

## 11. RVM 几何导出

### 11.1 两条路径怎么选

| 需求 | 入口 | 结果 |
|---|---|---|
| 只要 RVM 几何真值 | `l3_suite --check-driver` 执行 `rvm_export_*.mac` | 只读 E3D 数据库，产出 `.rvm` |
| RVM 与元素属性一起归档 | `GenModelRvmExport` CAF addin 的 pair 模式 | 同一目标同时产出 `.rvm` 与 `.att`，成对发布 |
| 将 RVM/ATT 变成对拍输入 | `rvm_verify import` | 产出带 scope、dbnum 和 root refno 的快照 JSON |

RVM 宏本身没有 `SAVEWORK`，不会增加 dabacon 会话，也不会推进服务水位。它仍会覆盖
目标导出文件，因此运行前要记录旧文件 SHA-256；CAF pair 模式会先写临时文件，RVM 与
ATT 都成功后才替换最终文件，第二个文件发布失败时会恢复第一个文件。

### 11.2 AMS 8000：TTY 导出单份 RVM

AMS 已有两个 dbnum 8000 宏：

- `scripts/e3d/rvm_export_c_iy_1r330_b.mac`：BRAN `/C-IY-1R330-B`；这是早期宏，未写与
  driver 同名的 `ALPHA LOG`，适合手工 E3D 命令窗口，不作为新 Agent 的首选 driver 用例。
- `scripts/e3d/rvm_export_c_or_1r345_c.mac`：BRAN `/C-OR-1R345-C`；包含 ALIVE/DONE 和
  同名日志，是 AMS TTY 示例的首选。

标准命令：

```powershell
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$evidence = "D:\work\plant-code\old\gen-model\output\rvm-export\$stamp"
$rvm = 'D:\work\plant-code\old\gen-model\test_data\rvm\C-OR-1R345-C.rvm'

New-Item -ItemType Directory -Force -Path $evidence | Out-Null
if (Test-Path -LiteralPath $rvm) {
  Copy-Item -LiteralPath $rvm -Destination "$evidence\before.rvm" -Force
  Get-FileHash -LiteralPath $rvm -Algorithm SHA256 |
    ConvertTo-Json | Set-Content "$evidence\before-rvm-hash.json" -Encoding utf8
}

$env:L3_E3D_INSTALL_DIR = 'E:\reverse\e3d\shadow_e3d31_gen_model_test'
& 'D:\Rust\target\debug\l3_suite.exe' `
  --check-driver 'scripts/e3d/rvm_export_c_or_1r345_c.mac' `
  --project-dir 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample' `
  --e3d-project AMS --e3d-mdb /ALL --output $evidence
if ($LASTEXITCODE -ne 0) { throw "RVM TTY 导出失败：$evidence" }

$file = Get-Item -LiteralPath $rvm
if ($file.Length -le 0) { throw 'RVM 输出为空' }
Get-FileHash -LiteralPath $rvm -Algorithm SHA256 |
  ConvertTo-Json | Set-Content "$evidence\after-rvm-hash.json" -Encoding utf8
Get-Content "$evidence\check-driver.log"
```

AMS 宏使用窄口径：`repre insu off`、`repre obst off`、`repre tube on`，隐含管段分容器，
并通过 `/expdri.so` 导出。不要随意改成 wide 后继续复用旧快照；保温/障碍体在 RVM 中
可能仍表现为普通 primitive，会污染成员 AABB。口径改变后必须重新 import，并在快照中
声明真实 scope。

通过判据：

1. driver 的 `L3-ALIVE` 与 `L3-DONE` 都出现，且命令退出码为 0；
2. 宏自己的日志包含 `CODEX-DB8000-RVM-EXPORT-DONE`；
3. RVM 文件存在、非空、mtime 属于本轮，SHA-256 已记录；
4. RVM 头能识别为 AVEVA E3D Design Review 文件；
5. 后续 `rvm_verify import` 成功，不把“文件存在”当成内容验收。

宏不会改 dabacon，所以这里的 rollback 是恢复旧导出物：

```powershell
if (Test-Path "$evidence\before.rvm") {
  Copy-Item "$evidence\before.rvm" $rvm -Force
} else {
  Remove-Item -LiteralPath $rvm -Force
}
```

### 11.3 RVM 与 ATT 属性成对导出

这里的 ATT 是“目标元素及后代的实例属性”，用于把 RVM group 稳定映射到真实 refno；
它不是下一节的全局 noun 属性字典。ATT 导出依赖 E3D appware 的 `!!cdxAttDump` 表单，
因此走已注册的 `GenModelRvmExport` CAF addin，而不是几何-only 的纯 TTY 宏。

当前已安装入口：

- DLL：`E:\reverse\e3d\shadow_e3d31_aps_all\GenModelRvmExport.dll`
- 注册：影子安装的 addin 注册清单包含 `GenModelRvmExport`
- 启动器：`E:\reverse\e3d\launch_e3d_sample_repaired.ps1`

AMS `/C-IY-1R330-B` 配方：

```powershell
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$evidence = "D:\work\plant-code\old\gen-model\output\rvm-att-export\$stamp"
$rvm = 'D:\work\plant-code\old\gen-model\test_data\rvm\C-IY-1R330-B.rvm'
$att = 'D:\work\plant-code\old\gen-model\test_data\rvm\C-IY-1R330-B.att'
$log = "$evidence\rvm-att-export.log"
New-Item -ItemType Directory -Force -Path $evidence | Out-Null

foreach ($item in @($rvm, $att)) {
  if (Test-Path -LiteralPath $item) {
    $name = [IO.Path]::GetFileName($item)
    Copy-Item -LiteralPath $item -Destination "$evidence\before-$name" -Force
  }
}

$env:GENMODEL_RVM_ELEMENT = '/C-IY-1R330-B'
$env:GENMODEL_RVM_OUT = $rvm
$env:GENMODEL_ATT_OUT = $att
$env:GENMODEL_RVM_LOG = $log
$env:GENMODEL_RVM_INSU = 'off'
$env:GENMODEL_RVM_OBST = 'off'
$env:GENMODEL_RVM_LEVEL = '6'
$env:GENMODEL_RVM_DELAY_MS = '30000'
$env:GENMODEL_RVM_QUIT = '1'

pwsh -NoProfile -ExecutionPolicy Bypass `
  -File 'E:\reverse\e3d\launch_e3d_sample_repaired.ps1' `
  -UseShadowInstall -EarlyInit3DState `
  -ProjectCode ams -ProjectDirectory AvevaMarineSample `
  -ProjectEnvPrefix AMS -Mdb /ALL

if (-not (Test-Path $log)) { throw 'RVM/ATT addin 没有生成日志' }
$text = Get-Content $log -Raw
if ($text -notmatch 'PAIR OK' -or $text -match 'THREW|PAIR FAILED') {
  throw "RVM/ATT pair 未通过：$log"
}
foreach ($item in @($rvm, $att)) {
  $file = Get-Item -LiteralPath $item
  if ($file.Length -le 0) { throw "导出文件为空：$item" }
}
Get-FileHash $rvm,$att -Algorithm SHA256 |
  ConvertTo-Json | Set-Content "$evidence\pair-hashes.json" -Encoding utf8
```

`GENMODEL_ATT_OUT` 为空时 addin 只导 RVM；非空时调用 `RunPair`。ATT 路径不能与 RVM
路径相同。无人值守时必须使用不存在的临时文件再发布，现有实现已处理旧文件确认框以及
两文件替换失败的回滚。

pair 通过后导入快照：

```powershell
cargo run --features rvm_verify --bin rvm_verify -- import `
  --rvm test_data/rvm/C-IY-1R330-B.rvm `
  --att test_data/rvm/C-IY-1R330-B.att `
  --dbnum 8000 --root-refno 24384/22404 --scope narrow `
  --out test_data/rvm/C-IY-1R330-B.rvm.json
```

导入通过后检查快照里的 `export_scope=narrow`、dbnum、root name/refno、成员数以及
`unresolved`。ATT 的核心价值就是身份解析；存在 ATT 却仍大量 unresolved 时，要先查
ATT 覆盖范围与目标 CE，而不是放宽几何容差。

pair rollback：分别恢复证据目录中的 before 文件；某个 before 不存在则删除对应的新
文件。回滚后重新计算两份 SHA-256，并与运行前记录比较。

## 12. 全局 noun/属性字典导出

全局字典导出与 RVM 的 ATT 不同：它枚举活 E3D 中的所有 `DbElementType`、每个 noun 的
有序 system attributes，并为所有不同属性导出 `DbAttributeField`。当前实现还可扫描
dabacon 的 per-noun 属性描述符 slot。

| 产物 | 内容 |
|---|---|
| noun layout JSON | noun、base/hard type、有序属性、类型、数组、长度、hidden/noClaim/default 探针 |
| noun attribute fields JSON | 不同属性的全部可读 `DbAttributeField`，包括 DCHC 等字段 |
| noun descriptor slots JSON | 从活会话扫描的 per-noun dabacon 描述符；受 element budget 控制 |

源码和入口：

- `scripts/e3d/NounLayoutExport.cs`
- `scripts/e3d/export_noun_layout.mac`
- `scripts/e3d/slots.mac`
- `scripts/e3d/run_export_ams_c.bat`（CAF addin 历史入口）

当前本机 `GenModelNounLayout.dll` 与 `GenModelSlots2.dll` 在两个 E3D 影子安装中都存在；
但当前影子安装的 addin 清单只注册了 `GenModelRvmExport`。因此新 Agent 不应仅运行
`run_export_ams_c.bat` 后凭进程退出码宣称字典刷新成功；应使用显式 import 宏，或先明确
恢复 noun addin 注册，并以三份输出和日志为准。

使用现有 `E3dTtyRunner` 显式执行两个宏：

```powershell
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$env:E3D_PROPERTY_EVIDENCE =
  "D:\work\plant-code\old\gen-model\output\noun-export\$stamp"
New-Item -ItemType Directory -Force -Path $env:E3D_PROPERTY_EVIDENCE | Out-Null

$propertyOutputs = @(
  'D:\work\plant-code\old\gen-model\output\noun_layout.json',
  'D:\work\plant-code\old\gen-model\output\noun_attr_fields.json',
  'D:\work\plant-code\old\gen-model\output\noun_descriptor_slots.json'
)
foreach ($item in $propertyOutputs) {
  if (Test-Path -LiteralPath $item) {
    $name = [IO.Path]::GetFileName($item)
    Copy-Item -LiteralPath $item `
      -Destination "$env:E3D_PROPERTY_EVIDENCE\before-$name" -Force
  }
}

@'
import os, sys
from pathlib import Path

repo = Path(r"D:\work\plant-code\old\gen-model")
sys.path.insert(0, str(repo / "scripts" / "python"))
from gen_model_testing import E3dTtyRunner, RustTools

evidence = Path(os.environ["E3D_PROPERTY_EVIDENCE"])
tools = RustTools(repo, bin_dir=Path(r"D:\Rust\target\debug"))
runner = E3dTtyRunner(
    tools,
    Path(r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample"),
    Path(r"E:\reverse\e3d\shadow_e3d31_gen_model_test"),
    evidence,
    e3d_project="AMS",
    e3d_mdb="/ALL",
)

layout = runner.run(repo / "scripts/e3d/export_noun_layout.mac",
                    label="noun-layout", timeout=1200)
slots = runner.run(repo / "scripts/e3d/slots.mac",
                   label="noun-slots", timeout=1200)
print("layout_exit=", layout.returncode)
print("slots_exit=", slots.returncode)
'@ | Set-Content "$env:E3D_PROPERTY_EVIDENCE\run-export.py" -Encoding utf8

python\.venv\Scripts\python.exe "$env:E3D_PROPERTY_EVIDENCE\run-export.py" |
  Tee-Object "$env:E3D_PROPERTY_EVIDENCE\run-export.log"
```

注意：仓内 `slots.mac` 当前预算是 3000，适合通道检查；完整扫描使用
`GENMODEL_NOUN_SLOTS_BUDGET=300000` 的 addin 配方，或复制宏后把 `ExportSlotsN` 的
budget 明确调大并把生成宏保存到本轮证据目录。不要把 3000 元素的结果写成“全库 slot
已覆盖”。

字典导出退出门：

1. layout 与 slots 两腿都 exit 0，driver 记录 ALIVE/DONE；
2. 日志含 `OK nouns=... attrs=... distinct=... fields=...`，没有 `FAIL`/`THREW`；
3. 目标 JSON 能被严格解析，顶层结构与记录数量非零；
4. layout 中每个 noun 的属性顺序保留，不能转成无序集合；
5. slot 报告记录实际 element budget、扫描数和覆盖边界；
6. 新旧 JSON 做规范化 diff，并记录三份 SHA-256；生产接入前跑 noun layout 探针和相关
   模型影响/DCHC 回归，不能因导出成功直接替换内嵌权威快照。

回滚时逐份恢复证据目录中的 `before-<name>`；运行前不存在的输出应删除。恢复后重新解析
JSON 并核对 SHA-256。noun/属性字典导出只写输出文件，不写 dabacon，因此不涉及会话号
回退。

2026-07-26 的已验证字典样例为 `1935 nouns / 22095 attribute declarations / 4271
distinct attributes / 203454 fields`。当前工作区仍有对应 layout 与 attribute-field 输出，
但 descriptor-slots 输出当前缺失；这是当前状态，不是三件套全部已刷新。
