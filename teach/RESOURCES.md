# RESOURCES

> 原则：不轻信模型的记忆。以下按可信度排序。第一手证据是**实时反编译**，其次是 **AVEVA 官方文档**。

## A. 一手证据（可复现）
- **IDA Pro MCP（headless idalib）**：`http://127.0.0.1:13338/mcp`，三个会话（均已分析完、Hex-Rays 可用）：
  - `core31-retrace` = `D:\AVEVA\Everything3D3.1\core.dll`
  - `core3d-retrace` = `D:\AVEVA\Everything3D3.1\Core3D.dll` ← 批量模型生成主战场（课 03）
  - `afimodel-retrace` = `D:\AVEVA\Everything3D3.1\AfiModeling.dll`
- **调用脚本**：`teach/../.ida_scratch/ida_mcp_client.py`（`list` / `call <tool>` / `raw`，参数走 stdin）。
  例：`echo '{}' | python ida_mcp_client.py --db=core31-retrace call server_health`
- **批量还原 FORTRAN 例程名**：这两个 DLL 里绝大多数函数无符号，但每个例程入口有 `MTRENT("模块/例程", …)` traceback 串。
  `_dump_routines.py`(core) / `_dump_routines2.py <db> <out.json>`(通用) 抓串并按模块归类；
  `_resolve3d.py` 把「模块/例程」反查成函数地址；`_who3d.py <addr…>` 打印 traceback + 调用面；
  `_dec.py` / `_dec3d.py <addr…>` 批量反编译落盘；`_opcodes.py` 导出 core.dll 的 141 个 GINO opcode 表。

## B. AVEVA 官方文档（印证目录几何概念）
- 3D Geomsets (GMSET)：https://docs.aveva.com/bundle/e3d-design-ue/page/912898.html
- Constructing 3D Geomsets（含 SCYL/参数化例子）：https://docs.aveva.com/bundle/e3d-design/page/912871.html
- Creating Catalogs, Sections and Catalog Components（SCOM/SPCO/PTSET/GMSET/PARAM）：https://docs.aveva.com/bundle/e3d-design-ue/page/912847.html
- Standard Hook-up (SHU) 元素属性（含 Spref / Desparam）：https://docs.aveva.com/bundle/e3d-design/page/890578.html

## C. 本仓资源
- `vendor/aios-parse-pdms/`：PDMS 数据库解析（udtype、属性表）。
- `src/fast_model/`：几何生成（cata_model / prim_model / gen_model / occ_generate）。
- `元数据接口.txt`、`all_attr_info.json`：属性/类型元数据。

## D. 社区（获取"智慧"）
- AVEVA 官方社区论坛（Connect / AVEVA Communities），可核对 GMSET/SPREF/DESPARAM 语义与边缘情况。
