# 截图管道增量验收

目标：AMS /ALL、DB8000、BRAN **24384/26229**，名称
`/Copy-of-1RCV0101-60.3-VMB-R90-R332]R333`。这不是上轮的 24384/23257。

`cases.json` 的 18 项覆盖新增、删除、属性修改、移动管件、整管移动、旋转、
成员重排、跨根改挂、重复事件、连续保存和恢复。状态均明确为 prepared：本轮只执行只读基线，
没有把这些破坏性操作施加到源支管。新增/删除/改挂在一次性项目副本中执行；新 refno 必须由 TTY 记录。

## 已实现测试

```powershell
python -m unittest discover -s scripts/pipe26229 -p test_verify.py
cargo test --locked --test pipe26229_increment --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture
```

- Rust：7 个公开规划器行为用例，验证新增根、删除清理、成员影响归根、改挂两端、纯数据前移、
  不完整属主链降级和未生成根保持 lazy。输入从收集器输出的 touched roots 开始，不代表真实 POS 解码覆盖。
- Python：校验器单元测试及本次 TTY/API 录制数据回放；识别漏增、漏删、顺序、属主、重复、
  非有限包围盒、旧发布会话、隐式直管缺失、RVM 额外几何，以及两个 ATTA 的已知基线差异。
- `prepare_moves.py --scratch-root <新支管实际refno> --out <新目录>` 生成 M01–M04 的 apply/restore 宏。
  它只生成文件、不执行；拒绝原始根。每对宏须先验证副本中 ELBO 3 / VALV 1 的身份。
  删除/规格/旋转/改挂等案例的宏应由副本的真实 refno、有效规格和精确基线生成，当前未伪造可执行输入。

## 只读采集与三方对拍

```powershell
python scripts/pipe26229/capture.py --service http://127.0.0.1:18122 `
  --out output/pipe26229-fresh-run `
  --catalogue D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams7000_0001 `
  --catalogue D:/AVEVA/Projects/E3D3.1/ZDJ/ZDJ000/zdj7600_0001 `
  --project-dir D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000 `
  --project-dir D:/AVEVA/Projects/E3D3.1/AvevaCatalogue/acp000
```

采集器要求输出目录全新；记录工具命令/退出码、输入哈希、会话、TTY 成员、RVM、
同会话全量模型、API 树/属性/模型。生成失败、输入文件变化、几何缺件或非有限数一律失败。
默认世界包围盒容差 0.01mm，不自动放宽曲面离散化差异。RVM 几何组包括具名 ATTA/VALV 与 TUBE，
不是只筛名字含 `of BRANCH` 的对象。运行记录中的 TUBI 缺少路由序号，当前比较根内包围盒多重集合，
不宣称端点身份或网格拓扑等价。完整属性的 `complete=false` 另记为未通过，不能由 POS 比对替代。

## 每一个真实增量案例的验收步骤

1. 在副本捕获 S：源文件/目录库 hash、精确 POS/ORI/NAME/SPRE/OWNER/成员槽位、模型记录和 RVM。
2. 执行单个操作，恰好一次 SAVEWORK，TTY 记录 T。动态 refno 立刻存入该案例 manifest。
3. 只观察 watcher 和 GET/records，等待有界超时；**不先 force ensure**。
   核对 published_target==T、记录变化、树更新；数据水位成功不替代模型成功。
4. 同 T 导出 TTY 数据和 RVM，按 T 全量生成；检查全量报告零失败及输入未变化。
5. 增量记录与 full@T 比较身份集合、显式构件变换、AABB、mesh引用、隐式管段数量/区间。
   删除项在树与模型中均消失；未触及项不得改变。旋转另核对变换/连接轴，不能只比 AABB。
6. RVM 覆盖全部几何组；曲面小差异单列，大差异/缺件硬失败，不按整体包围盒掩盖局部错误。
7. 最后执行 restore；删除用项目 checkpoint 恢复原 refno，而非新建同名对象冒充恢复。
   restore 后重新验证字段、成员序、模型和隐式管段；会话历史允许增加。

## 本轮结果边界

证据目录 `output/pipe26229-baseline-20260905`：TTY/API 12 个有序成员匹配；ELBO 3 位置
误差 0.1mm（TTY 默认整毫米，位置容差 0.5mm）。正确目录文件组合生成 16 个几何、零失败。
RVM 16 组全部找到对应，但 0.01mm 世界 AABB 门未通过：多数差异约 0.07–0.34mm，
两个 ATTA 约 119.85 / 219.85mm，尚未做局部坐标/显示表示原因鉴别。
严格采集另捕获会话 285 未变但设计库、AMS 目录库字节哈希变化，失败退出；不将此批诊断升级为稳定输入验收。
所有增删改 live 会话案例仍待副本录制。运行单元测试通过仅说明断言能识别这些错误。
