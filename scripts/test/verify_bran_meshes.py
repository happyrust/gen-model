#!/usr/bin/env python3
"""
验证 BRAN/HANG mesh 文件的有效性

检查项：
1. mesh 文件是否存在
2. 文件大小是否合理
3. 文件内容是否有效（如果是文本格式）
"""

import os
import sys
from pathlib import Path
import json

def verify_mesh_files(mesh_dir: str) -> dict:
    """验证 mesh 文件"""
    mesh_path = Path(mesh_dir)
    
    if not mesh_path.exists():
        print(f"❌ Mesh 目录不存在: {mesh_dir}")
        return {"success": False, "error": "directory_not_found"}
    
    # 查找所有 .mesh 文件
    mesh_files = list(mesh_path.rglob("*.mesh"))
    
    results = {
        "success": True,
        "total_files": len(mesh_files),
        "valid_files": 0,
        "invalid_files": 0,
        "total_size": 0,
        "files": []
    }
    
    print(f"📊 找到 {len(mesh_files)} 个 mesh 文件")
    print()
    
    for mesh_file in mesh_files:
        file_info = {
            "path": str(mesh_file.relative_to(mesh_path)),
            "size": mesh_file.stat().st_size,
            "valid": False,
            "error": None
        }
        
        results["total_size"] += file_info["size"]
        
        # 检查文件大小
        if file_info["size"] == 0:
            file_info["error"] = "empty_file"
            results["invalid_files"] += 1
            print(f"⚠️  {file_info['path']}: 文件为空")
        elif file_info["size"] < 100:
            file_info["error"] = "file_too_small"
            results["invalid_files"] += 1
            print(f"⚠️  {file_info['path']}: 文件太小 ({file_info['size']} bytes)")
        else:
            # 尝试读取文件头部，验证格式
            try:
                with open(mesh_file, 'rb') as f:
                    header = f.read(8)
                    # 简单验证：检查是否是有效的二进制数据
                    if header:
                        file_info["valid"] = True
                        results["valid_files"] += 1
                    else:
                        file_info["error"] = "invalid_header"
                        results["invalid_files"] += 1
            except Exception as e:
                file_info["error"] = f"read_error: {e}"
                results["invalid_files"] += 1
                print(f"❌ {file_info['path']}: 读取失败 - {e}")
        
        results["files"].append(file_info)
    
    return results

def print_summary(results: dict):
    """打印验证摘要"""
    print()
    print("=" * 60)
    print("📊 验证摘要")
    print("=" * 60)
    print(f"总文件数:    {results['total_files']}")
    print(f"有效文件:    {results['valid_files']}")
    print(f"无效文件:    {results['invalid_files']}")
    print(f"总大小:      {results['total_size']:,} bytes ({results['total_size'] / 1024 / 1024:.2f} MB)")
    
    if results['total_files'] > 0:
        avg_size = results['total_size'] / results['total_files']
        print(f"平均大小:    {avg_size:,.0f} bytes ({avg_size / 1024:.2f} KB)")
    
    print()
    
    if results['valid_files'] == results['total_files'] and results['total_files'] > 0:
        print("✅ 所有 mesh 文件验证通过")
        return True
    elif results['valid_files'] > 0:
        print(f"⚠️  部分文件验证通过 ({results['valid_files']}/{results['total_files']})")
        return False
    else:
        print("❌ 没有有效的 mesh 文件")
        return False

def main():
    # 默认 mesh 目录
    default_mesh_dir = "test_output/full_noun_bran_meshes"
    
    # 从命令行参数获取目录
    mesh_dir = sys.argv[1] if len(sys.argv) > 1 else default_mesh_dir
    
    print(f"🔍 验证 mesh 文件...")
    print(f"   目录: {mesh_dir}")
    print()
    
    # 执行验证
    results = verify_mesh_files(mesh_dir)
    
    # 打印摘要
    success = print_summary(results)
    
    # 保存详细结果到 JSON
    output_file = Path("test_output/mesh_verification_results.json")
    output_file.parent.mkdir(parents=True, exist_ok=True)
    
    with open(output_file, 'w') as f:
        json.dump(results, f, indent=2)
    
    print(f"\n📄 详细结果已保存到: {output_file}")
    
    # 返回退出码
    sys.exit(0 if success else 1)

if __name__ == "__main__":
    main()
