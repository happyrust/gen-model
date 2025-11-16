# Refno 模型生成 API - meshes_path 参数说明

## 概述

`/api/generate-by-refno` 接口现在支持通过 `meshes_path` 参数指定 mesh 文件的输出目录。

## API 端点

```
POST /api/generate-by-refno
```

## 请求参数

| 参数名 | 类型 | 必填 | 说明 |
|--------|------|------|------|
| `db_num` | `u32` | 是 | 数据库编号 |
| `refnos` | `Vec<String>` | 是 | Refno 列表（支持 "123" 或 "1/456" 格式） |
| `gen_mesh` | `bool` | 否 | 是否生成网格（默认从配置读取） |
| `gen_model` | `bool` | 否 | 是否生成模型（默认从配置读取） |
| `apply_boolean_operation` | `bool` | 否 | 是否应用布尔运算（默认从配置读取） |
| `meshes_path` | `String` | 否 | **新增**：Mesh 文件输出目录（默认从配置读取） |

## 请求示例

### 示例 1：使用默认 meshes_path

```json
{
    "db_num": 1500,
    "refnos": ["21491_18946", "24381_46952"],
    "gen_mesh": true,
    "gen_model": true
}
```

此时 mesh 文件将输出到配置文件（`DbOption.toml`）中指定的 `meshes_path`，如果配置文件中未指定，则使用默认路径。

### 示例 2：指定自定义 meshes_path

```json
{
    "db_num": 1500,
    "refnos": ["21491_18946", "24381_46952"],
    "gen_mesh": true,
    "gen_model": true,
    "meshes_path": "/custom/output/meshes"
}
```

此时 mesh 文件将输出到 `/custom/output/meshes` 目录。

### 示例 3：相对路径

```json
{
    "db_num": 1500,
    "refnos": ["21491_18946"],
    "gen_mesh": true,
    "meshes_path": "output/project_meshes"
}
```

相对路径将基于项目根目录解析。

## 响应格式

```json
{
    "success": true,
    "task_id": "refno_gen_1500_20250114_123456",
    "status": "Pending",
    "message": "任务已创建并开始执行，将处理 2 个 refno",
    "refno_count": 2
}
```

## 使用 curl 测试

```bash
curl -X POST http://localhost:8080/api/generate-by-refno \
  -H "Content-Type: application/json" \
  -d '{
    "db_num": 1500,
    "refnos": ["21491_18946"],
    "gen_mesh": true,
    "gen_model": true,
    "meshes_path": "/Volumes/DPC/work/output/meshes"
  }'
```

## 配置优先级

1. **API 请求参数** - 最高优先级，如果在请求中指定了 `meshes_path`，将使用该值
2. **DbOption.toml 配置文件** - 如果请求中未指定，则使用配置文件中的 `meshes_path`
3. **默认值** - 如果以上都未指定，使用系统默认路径

## 注意事项

1. 确保指定的目录存在且有写入权限
2. 路径可以是绝对路径或相对路径
3. 建议使用绝对路径以避免路径解析问题
4. 如果目录不存在，系统会尝试自动创建（取决于底层实现）

## 修改内容总结

本次改进涉及以下文件：

- `src/web_server/models.rs` - 添加 `meshes_path` 字段到 `DatabaseConfig` 和 `RefnoModelGenerationRequest`
- `src/web_server/handlers.rs` - 在 API 处理和任务执行中传递 `meshes_path` 参数
- `src/web_server/database_status_handlers.rs` - 更新配置初始化
- `src/web_server/wizard_handlers.rs` - 更新配置初始化

## 向后兼容性

此改进完全向后兼容，现有的 API 调用无需修改即可继续工作。`meshes_path` 是可选参数，不传递时使用默认行为。

