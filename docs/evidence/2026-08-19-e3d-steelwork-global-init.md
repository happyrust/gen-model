# E3D Steelwork 全局初始化修复证据

## 现场症状

- E3D Design Command Window 在 CE 变化时重复打印 `(2,751) Variable
  !!STEELWORKGSETTINGS does not exist`。
- 首轮只补全该全局后，状态栏继续出现
  `!!STEELWORKCONTROLPROFILESTORAGEGRID does not exist`，证明自动命令注册早于支持命令装载。
- 现场同时存在 2026-08-15 与 2026-08-19 启动的两个 `des.exe`；验收前已收口为单实例。

## 修复

1. `Repair-E3dSteelworkGlobals.ps1` 幂等修补 shadow
   `PMLLIB/common/objects/pmlcommandmanager.pmlobj`：
   - 注册 refresh callback 前创建 `STEELWORKOSETTINGS` 全局实例；
   - 顺序固定为加载自动命令对象、加载/注册支持命令、注册自动命令。
2. `Initialize-E3dSteelworkGlobals.ps1` 在 Design 就绪后通过现有 CLR bootstrap 执行
   `ams_steelwork_globals.pmlmac`；trace 未出现 PASS 时启动命令返回失败。
3. `run_ams_gui.bat` 在每次启动时先验证 shadow 补丁，并在图形 finisher 前执行可验证初始化。

## 字面输出

```text
[PML] Steelwork globals and registration order verified: E:\reverse\e3d\shadow_e3d31_aps_all\PMLLIB\common\objects\pmlcommandmanager.pmlobj
[+] CLR exec pid=8920 ... result={'ok': True, 'step': 'ExecuteInDefaultAppDomain', 'exec_hr': 0, 'ret': 0}
[OK] STEELWORK_GLOBALS PID=8920 TRACE=E:\reverse\e3d\ams_steelwork_globals_trace.txt
begin
PASS STEELWORKGSETTINGS ready
```

命令退出码：`0`。进程验收：仅一个 `des.exe`，PID `8920`，`Responding=True`。
随后在 Model Explorer 点击 `Model World` 触发 CE refresh，状态栏未再出现变量缺失。

更新后的 `run_ams_gui.bat` 冷启动再次验收：

```text
des_count=1 steelwork_pass=True model_complete=True
E:\reverse\e3d\ams_steelwork_globals_trace.txt  2026/8/19 16:19:24
begin
PASS STEELWORKGSETTINGS ready
```

冷启动进程 PID `40252`；模型树展开完成，最终界面状态栏无 PML 变量缺失。
