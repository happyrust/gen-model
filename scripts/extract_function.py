#!/usr/bin/env python3
"""
函数提取辅助脚本 - handlers.rs 重构工具

用法:
    python3 scripts/extract_function.py <start_line> <end_line> [output_file]

示例:
    # 提取 6106-6146 行的函数
    python3 scripts/extract_function.py 6106 6146 temp_check_database_connection.rs
"""

import sys
import os
from pathlib import Path

SOURCE_FILE = "src/web_server/handlers.rs"


def extract_lines(file_path, start_line, end_line):
    """从文件中提取指定行范围"""
    try:
        with open(file_path, 'r', encoding='utf-8') as f:
            lines = f.readlines()

        if start_line < 1 or end_line > len(lines):
            print(f"错误: 行号范围无效 (文件共 {len(lines)} 行)")
            return None

        return ''.join(lines[start_line - 1:end_line])
    except FileNotFoundError:
        print(f"错误: 文件未找到 {file_path}")
        return None
    except Exception as e:
        print(f"错误: {e}")
        return None


def find_function_boundaries(file_path, start_line):
    """
    自动查找函数的结束位置（基于括号匹配）
    返回: (start_line, end_line) 或 None
    """
    try:
        with open(file_path, 'r', encoding='utf-8') as f:
            lines = f.readlines()

        brace_count = 0
        in_function = False
        function_start = None

        for i, line in enumerate(lines[start_line - 1:], start=start_line):
            # 检测函数开始
            if 'pub async fn' in line or 'pub fn' in line or 'async fn' in line or 'fn ' in line:
                if function_start is None:
                    function_start = i

            # 统计括号
            if '{' in line:
                in_function = True
                brace_count += line.count('{')
            if '}' in line:
                brace_count -= line.count('}')

            # 找到函数结束
            if in_function and brace_count == 0:
                return (function_start, i)

        return None
    except Exception as e:
        print(f"错误: {e}")
        return None


def generate_module_template(module_name, functions_code):
    """生成模块模板"""
    template = f"""// {module_name}.rs
//
// 负责处理 {module_name} 相关的 HTTP 请求

use axum::{{
    extract::{{Path, Query, State}},
    http::StatusCode,
    response::{{Html, Json}},
}};
use serde::{{Deserialize, Serialize}};
use serde_json::json;
use std::time::{{SystemTime, Duration}};

use crate::web_server::{{AppState, models::*}};

// ================= 数据结构定义 =================

// TODO: 在此处添加数据结构

// ================= API 处理器 =================

{functions_code}

// ================= 内部辅助函数 =================

// TODO: 在此处添加辅助函数
"""
    return template


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(1)

    try:
        start_line = int(sys.argv[1])
        end_line = int(sys.argv[2])
        output_file = sys.argv[3] if len(sys.argv) > 3 else None
    except ValueError:
        print("错误: 行号必须是整数")
        sys.exit(1)

    # 检查源文件是否存在
    if not os.path.exists(SOURCE_FILE):
        print(f"错误: 源文件不存在 {SOURCE_FILE}")
        print(f"当前目录: {os.getcwd()}")
        sys.exit(1)

    # 提取代码
    print(f"从 {SOURCE_FILE} 提取 {start_line}-{end_line} 行...")
    code = extract_lines(SOURCE_FILE, start_line, end_line)

    if code is None:
        sys.exit(1)

    # 输出结果
    if output_file:
        output_path = Path(output_file)
        output_path.parent.mkdir(parents=True, exist_ok=True)

        with open(output_file, 'w', encoding='utf-8') as f:
            f.write(code)

        print(f"✅ 成功提取到 {output_file}")
        print(f"   行数: {len(code.splitlines())}")

        # 显示前几行预览
        print("\n预览（前10行）:")
        print("-" * 60)
        for i, line in enumerate(code.splitlines()[:10], 1):
            print(f"{i:3d} | {line}")
        print("-" * 60)
    else:
        # 直接输出到控制台
        print("\n提取结果:")
        print("=" * 60)
        print(code)
        print("=" * 60)
        print(f"总行数: {len(code.splitlines())}")


if __name__ == "__main__":
    main()
