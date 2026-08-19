# dbnum=8000 EQUI BOX 增删增量与模型核对

日期：2026-08-19
项目：`AvevaMarineSample`
文件：`D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001`

## 新增

E3D 宏 `scripts/e3d/db8000_equi_add_box_apply.mac` 创建并保存：

```text
BOX /CODEX_DB8000_EQ_ADD_BOX
Ref =32576/1
Owner /1-LNR-Q005-PJ
会话=240
保存时间=2026-08-19T18:15:46+08:00
```

正确 Release 程序重放后的任务结果：

```text
dbnum=8000
会话区间=240..=240
新增=1 修改=1 删除=0
applied_sesno=240
staging_windows=0
```

模型接口核对：

```json
{
  "requested_refno": "32576/1",
  "generation_root": "24384/24776",
  "generation_root_noun": "EQUI",
  "status": "AlreadyAvailable",
  "model_available": true,
  "model_instance_count": 1,
  "generated_instance_count": 1
}
```

## 删除与错误二进制诊断

E3D 宏 `scripts/e3d/db8000_equi_add_box_restore.mac` 删除同一 BOX 并保存为会话 241。
首次运行误从 `D:\work\plant-code\old\target\release` 部署了旧二进制，错误结果为
`新增=0 修改=1 删除=0`。仓库实际 `target-dir` 是 `D:\Rust\target`。

真实文件离线收集结果：

```text
membership_deleted=1
241 24384_24776 Modified children_changed=([32576_1, 24384_24777], [24384_24777])
241 32576_1 Deleted
```

## 已提交窗口纠正

服务停止后执行：

```powershell
D:\Rust\target\release\db_window_repair.exe --dbnum 8000 --from 241 --to 241 `
  --expect-watermark 241 `
  --file D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001
```

字面结果：

```text
新增=0 修改=1 删除=1 成员补删=1 不可达=0
watermark 241->241
staging_windows=0
cleaned_refnos=["32576/1"]
verification=watermark unchanged; deleted pe/noun/UDA/owner/model rows absent
exit=0
```

随后部署 `D:\Rust\target\release\aios-database.exe`：

```text
SHA256=51F068F7F120213E6F7752F137C2513B705B9484E802C3E8C73D804671E40432
status=ok
model_ready=true
worker_alive=true
file_latest_sesno=241
applied_sesno=241
staging_windows=0
side_effect_pending=0
spatial_pending=0
POST /api/v1/model/ensure refno=32576/1 -> HTTP 404 构件不存在
```

候选存在性修复提交：`old-pdms-io 146a072`。回归测试为 16 passed、1 ignored，
`cargo fmt --check` 与 Release `db_window_repair` 构建均通过。
