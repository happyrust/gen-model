#!/usr/bin/env python3
"""测试 meshes_path 参数的 API"""

import requests
import json

API_URL = "http://localhost:8080/api/model/generate-by-refno"

def test_api(test_name, payload):
    """测试 API 调用"""
    print(f"\n{'='*60}")
    print(f"测试: {test_name}")
    print(f"{'='*60}")
    print(f"请求数据: {json.dumps(payload, indent=2, ensure_ascii=False)}")
    
    try:
        response = requests.post(API_URL, json=payload, timeout=10)
        print(f"\n状态码: {response.status_code}")
        print(f"响应数据:")
        print(json.dumps(response.json(), indent=2, ensure_ascii=False))
    except Exception as e:
        print(f"❌ 错误: {e}")

# 测试 1: 不指定 meshes_path
test_api(
    "测试 1 - 不指定 meshes_path (使用默认配置)",
    {
        "db_num": 1112,
        "refnos": ["17496/201375"],
        "gen_mesh": True,
        "gen_model": False
    }
)

# 测试 2: 指定自定义 meshes_path (绝对路径)
test_api(
    "测试 2 - 指定自定义 meshes_path (绝对路径)",
    {
        "db_num": 1112,
        "refnos": ["17496/201375"],
        "gen_mesh": True,
        "gen_model": False,
        "meshes_path": "/Volumes/DPC/work/plant-code/gen-model-fork/output/custom_meshes"
    }
)

# 测试 3: 使用相对路径
test_api(
    "测试 3 - 使用相对路径的 meshes_path",
    {
        "db_num": 1112,
        "refnos": ["17496/201375"],
        "gen_mesh": True,
        "gen_model": False,
        "meshes_path": "output/test_meshes"
    }
)

print(f"\n{'='*60}")
print("✅ 所有测试完成！")
print(f"{'='*60}\n")

