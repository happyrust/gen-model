#!/usr/bin/env python3
"""
handlers.rs 自动重构脚本
自动将 7479 行的 handlers.rs 拆分为多个符合规范的小文件
"""

import re
import os
from pathlib import Path
from typing import Dict, List, Tuple, Set
from dataclasses import dataclass

@dataclass
class Function:
    """函数信息"""
    name: str
    start_line: int
    end_line: int
    is_public: bool
    content: str
    module: str  # 归属的模块

# 函数分类映射（基于函数名前缀/关键词）
FUNCTION_MODULES = {
    # 端口管理
    'port': ['check_port', 'kill_port', 'is_addr_listening', 'is_port_in_use',
             'test_tcp_connection', 'command_exists'],

    # 配置管理
    'config': ['get_config', 'update_config', 'get_config_templates',
               'get_available_databases'],

    # 项目管理
    'project': ['api_get_projects', 'api_create_project', 'api_get_project',
                'api_update_project', 'api_delete_project', 'api_healthcheck_project',
                'api_projects_demo', 'projects_health_scheduler', 'ensure_projects_schema'],

    # 任务管理
    'task': ['get_tasks', 'get_task', 'create_task', 'start_task', 'stop_task',
             'restart_task', 'delete_task', 'execute_real_task', 'execute_parse_pdms_task',
             'get_next_task_number', 'get_task_templates', 'create_batch_tasks',
             'get_task_error_details', 'get_task_logs'],

    # 部署站点
    'deployment_site': ['api_get_deployment_sites', 'api_create_deployment_site',
                        'api_get_deployment_site', 'api_update_deployment_site',
                        'api_delete_deployment_site', 'api_import_deployment_site',
                        'api_browse_deployment_site', 'api_create_deployment_site_task',
                        'api_healthcheck_deployment_site', 'api_export_deployment_site',
                        'ensure_deployment_sites_schema'],

    # SurrealDB 服务
    'surreal_server': ['start_surreal', 'stop_surreal', 'restart_surreal',
                       'get_surreal_status', 'test_surreal_connection',
                       'get_system_status', 'run_remote_ssh', 'test_database_functionality'],

    # 数据库状态
    'database_status': ['get_db_status', 'execute_incremental_update', 'check_file_versions',
                        'check_model_status', 'check_mesh_status', 'set_update_finalize',
                        'convert_to_db_status', 'get_file_version_info', 'check_single_file_version'],

    # 数据库连接
    'database_connection': ['check_database_connection', 'get_startup_scripts',
                            'start_database_instance', 'check_surrealdb_connection',
                            'start_surreal_with_script', 'create_default_startup_script',
                            'handle_database_connection_error', 'run_database_diagnostics'],

    # 空间查询
    'spatial_query': ['api_space_', 'api_sqlite_spatial', 'spatial_query_page',
                      'api_sqlite_tray_supports'],

    # 导出管理
    'export': ['create_export_task', 'execute_export_task', 'get_export_status',
               'download_export', 'list_export_tasks', 'cleanup_export_tasks'],

    # 模型生成
    'model_generation': ['api_generate_by_refno', 'execute_refno_model_generation',
                         'update_room_relations', 'batch_update_room_relations'],

    # SCTN 测试
    'sctn_test': ['sctn_test', 'api_sctn_test', 'run_sctn_test', 'finish_fail'],

    # 页面渲染
    'pages': ['_page', 'index_page', 'dashboard_page', 'config_page', 'tasks_page',
              'wizard_page', 'serve_incremental_update_page', 'serve_database_status_page'],
}

def classify_function(func_name: str) -> str:
    """根据函数名判断其归属模块"""
    for module, patterns in FUNCTION_MODULES.items():
        for pattern in patterns:
            if pattern in func_name:
                return module

    # 默认归类到 misc
    print(f"⚠️  无法分类函数: {func_name}, 归入 'misc' 模块")
    return 'misc'

def extract_functions(file_path: str) -> List[Function]:
    """提取文件中的所有函数"""
    with open(file_path, 'r', encoding='utf-8') as f:
        lines = f.readlines()

    functions = []
    current_func = None
    brace_count = 0

    for i, line in enumerate(lines, 1):
        # 匹配函数定义
        func_match = re.match(r'^(pub\s+)?async\s+fn\s+(\w+)', line)
        if func_match:
            if current_func:
                # 保存上一个函数
                current_func.end_line = i - 1
                current_func.content = ''.join(lines[current_func.start_line-1:current_func.end_line])
                functions.append(current_func)

            is_public = func_match.group(1) is not None
            func_name = func_match.group(2)
            current_func = Function(
                name=func_name,
                start_line=i,
                end_line=0,
                is_public=is_public,
                content='',
                module=classify_function(func_name)
            )
            brace_count = 0

        # 统计大括号
        if current_func:
            brace_count += line.count('{') - line.count('}')

            # 函数结束
            if brace_count == 0 and '{' in ''.join(lines[current_func.start_line-1:i]):
                current_func.end_line = i
                current_func.content = ''.join(lines[current_func.start_line-1:current_func.end_line])
                functions.append(current_func)
                current_func = None

    return functions

def extract_imports(file_path: str) -> str:
    """提取文件开头的 import 语句"""
    with open(file_path, 'r', encoding='utf-8') as f:
        lines = f.readlines()

    imports = []
    for line in lines:
        if line.strip().startswith('use ') or line.strip().startswith('use('):
            imports.append(line)
        elif line.strip().startswith('pub async fn') or line.strip().startswith('async fn'):
            break

    return ''.join(imports)

def group_functions_by_module(functions: List[Function]) -> Dict[str, List[Function]]:
    """按模块分组函数"""
    grouped = {}
    for func in functions:
        if func.module not in grouped:
            grouped[func.module] = []
        grouped[func.module].append(func)
    return grouped

def generate_module_file(module_name: str, functions: List[Function], base_imports: str) -> str:
    """生成模块文件内容"""
    content = f"""// {module_name.replace('_', ' ').title()} 模块
// 自动从 handlers.rs 提取

{base_imports}

"""

    for func in functions:
        content += func.content + '\n\n'

    return content

def main():
    """主函数"""
    print("🚀 开始自动重构 handlers.rs...")

    # 路径配置
    project_root = Path(__file__).parent.parent
    handlers_file = project_root / 'src/web_server/handlers.rs'
    handlers_dir = project_root / 'src/web_server/handlers'

    print(f"📁 项目根目录: {project_root}")
    print(f"📄 源文件: {handlers_file}")
    print(f"📂 目标目录: {handlers_dir}")

    # 1. 提取函数
    print("\n📖 分析函数...")
    functions = extract_functions(str(handlers_file))
    print(f"✅ 发现 {len(functions)} 个函数")

    # 2. 提取 imports
    print("\n📦 提取 import 语句...")
    base_imports = extract_imports(str(handlers_file))

    # 3. 按模块分组
    print("\n🗂️  按模块分组...")
    grouped = group_functions_by_module(functions)
    for module, funcs in grouped.items():
        print(f"   {module}: {len(funcs)} 个函数")

    # 4. 生成文件
    print("\n✍️  生成模块文件...")
    handlers_dir.mkdir(exist_ok=True)

    for module, funcs in grouped.items():
        module_file = handlers_dir / f"{module}.rs"
        content = generate_module_file(module, funcs, base_imports)

        with open(module_file, 'w', encoding='utf-8') as f:
            f.write(content)

        line_count = len(content.split('\n'))
        status = '✅' if line_count <= 250 else '⚠️ '
        print(f"   {status} {module}.rs ({line_count} 行)")

    print("\n✅ 重构完成！")
    print("\n⚠️  请注意：")
    print("   1. 检查生成的文件是否有编译错误")
    print("   2. 某些模块可能超过 250 行，需要手动拆分")
    print("   3. 更新 mod.rs 的模块导出")
    print("   4. 运行 cargo check 验证")

if __name__ == '__main__':
    main()
