# NOTES（教学偏好）

- 语言：中文讲解。
- 风格：重证据——每个结论尽量给出 `core.dll` 具体地址 + 反编译片段；能举例就举例。
- 背景：用户在做 `gen-model`（Rust，从 PDMS/E3D 生成 3D 模型），学习目的是复刻/增量更新几何。
- 已确认掌握：GraphicsUpdate→FZXUPD→FUPALL→GLUPDA 刷新链；sgl5NET/libgeom 依赖。
- 交付形态：teach 工作区（MISSION + lessons/*.html + reference/*.html + learning-records/*.md + cases/*.md + assets）。
- 案例卡（`cases/`）固定七段：一句话 / 现象 / 证据 / 根因 / 修法 / 验证 / 规律；证据分 A 内核反编译、B 离线单测、C 端到端实库三层，三层不互相顶替。
- 未验证、被阻塞、只做了一半的事要在卡片和参考文档里如实写出来，不并进"已完成"。
