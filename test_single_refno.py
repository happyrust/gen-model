#!/usr/bin/env python3
"""测试单个 refno 的模型生成 - 增强版（支持 BRAN/PANE 性能测试）"""

import requests
import json
import sys
import time
import argparse
from typing import List, Dict, Optional
from datetime import datetime

API_URL = "http://localhost:8080/api/model/generate-by-refno"

# BRAN 测试用例（管道分支和支吊架）
BRAN_TEST_CASES = [
    {"db_num": 1112, "refno": "15201", "desc": "BRAN管道分支-1"},
    {"db_num": 1112, "refno": "15202", "desc": "BRAN管道分支-2"},
    {"db_num": 1112, "refno": "15203", "desc": "BRAN支吊架"},
]

# PANE 测试用例（墙板和楼板）
PANE_TEST_CASES = [
    {"db_num": 1112, "refno": "299", "desc": "PANE墙板-1"},
    {"db_num": 1112, "refno": "300", "desc": "PANE楼板-1"},
    {"db_num": 1112, "refno": "301", "desc": "PANE墙板-2"},
]

def test_refno_generation(db_num: int, refno: str, meshes_path: Optional[str] = None, 
                          verbose: bool = True) -> Dict:
    """测试指定 refno 的模型生成
    
    Args:
        db_num: 数据库编号
        refno: 参考号
        meshes_path: 可选的网格输出路径
        verbose: 是否打印详细信息
        
    Returns:
        包含性能指标的结果字典
    """
    
    payload = {
        "db_num": db_num,
        "refnos": [refno],
        "gen_mesh": True,
        "gen_model": True,
        "apply_boolean_operation": True
    }
    
    if meshes_path:
        payload["meshes_path"] = meshes_path
    
    if verbose:
        print(f"\n{'='*70}")
        print(f"🚀 测试 Refno 模型生成")
        print(f"{'='*70}")
        print(f"📊 数据库编号: {db_num}")
        print(f"🔖 Refno: {refno}")
        print(f"🗂️  Mesh 输出路径: {meshes_path if meshes_path else '使用默认配置'}")
        print(f"{'='*70}\n")
    
    result = {
        "db_num": db_num,
        "refno": refno,
        "success": False,
        "response_time_ms": 0,
        "task_id": None,
        "error": None
    }
    
    try:
        start_time = time.time()
        response = requests.post(API_URL, json=payload, timeout=30)
        response_time = (time.time() - start_time) * 1000  # Convert to ms
        
        result["response_time_ms"] = response_time
        
        if verbose:
            print(f"📥 响应状态码: {response.status_code}")
            print(f"⏱️  响应时间: {response_time:.2f} ms")
        
        if response.status_code == 200:
            data = response.json()
            result["success"] = data.get("success", False)
            result["task_id"] = data.get("task_id")
            result["status"] = data.get("status")
            result["message"] = data.get("message")
            
            if verbose:
                print(f"✅ 请求成功！")
                print(f"📋 任务 ID: {result['task_id']}")
                print(f"💬 消息: {result['message']}")
        else:
            result["error"] = f"HTTP {response.status_code}: {response.text}"
            if verbose:
                print(f"❌ 请求失败: {result['error']}")
            
    except Exception as e:
        result["error"] = str(e)
        if verbose:
            print(f"❌ 错误: {e}")
    
    return result


def run_benchmark(test_cases: List[Dict], test_type: str) -> List[Dict]:
    """运行性能基准测试
    
    Args:
        test_cases: 测试用例列表
        test_type: 测试类型（BRAN 或 PANE）
        
    Returns:
        性能测试结果列表
    """
    print(f"\n{'='*70}")
    print(f"🎯 开始 {test_type} 性能基准测试")
    print(f"{'='*70}")
    print(f"测试用例数量: {len(test_cases)}")
    print(f"开始时间: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
    print(f"{'='*70}\n")
    
    results = []
    
    for i, case in enumerate(test_cases, 1):
        print(f"\n[{i}/{len(test_cases)}] 测试: {case['desc']}")
        print(f"  DB: {case['db_num']}, Refno: {case['refno']}")
        
        result = test_refno_generation(
            db_num=case["db_num"],
            refno=case["refno"],
            verbose=False
        )
        
        result["desc"] = case["desc"]
        result["test_type"] = test_type
        results.append(result)
        
        # 打印简要结果
        if result["success"]:
            print(f"  ✅ 成功 - 响应时间: {result['response_time_ms']:.2f} ms")
        else:
            print(f"  ❌ 失败 - {result['error']}")
        
        # 避免请求过快
        time.sleep(1)
    
    return results


def generate_report(results: List[Dict], output_file: str = "performance_report.json"):
    """生成性能测试报告
    
    Args:
        results: 测试结果列表
        output_file: 输出文件名
    """
    print(f"\n{'='*70}")
    print(f"📊 性能测试报告")
    print(f"{'='*70}\n")
    
    # 按测试类型分组统计
    by_type = {}
    for result in results:
        test_type = result.get("test_type", "Unknown")
        if test_type not in by_type:
            by_type[test_type] = []
        by_type[test_type].append(result)
    
    # 打印统计信息
    for test_type, type_results in by_type.items():
        success_count = sum(1 for r in type_results if r["success"])
        total_count = len(type_results)
        
        response_times = [r["response_time_ms"] for r in type_results if r["success"]]
        avg_time = sum(response_times) / len(response_times) if response_times else 0
        min_time = min(response_times) if response_times else 0
        max_time = max(response_times) if response_times else 0
        
        print(f"📈 {test_type} 类型:")
        print(f"  成功率: {success_count}/{total_count} ({success_count/total_count*100:.1f}%)")
        print(f"  平均响应时间: {avg_time:.2f} ms")
        print(f"  最快响应: {min_time:.2f} ms")
        print(f"  最慢响应: {max_time:.2f} ms")
        print()
    
    # 保存详细结果
    report = {
        "timestamp": datetime.now().isoformat(),
        "total_tests": len(results),
        "summary": {
            test_type: {
                "count": len(type_results),
                "success": sum(1 for r in type_results if r["success"]),
                "avg_response_ms": sum(r["response_time_ms"] for r in type_results if r["success"]) / 
                                   len([r for r in type_results if r["success"]]) 
                                   if any(r["success"] for r in type_results) else 0
            }
            for test_type, type_results in by_type.items()
        },
        "details": results
    }
    
    with open(output_file, 'w', encoding='utf-8') as f:
        json.dump(report, f, indent=2, ensure_ascii=False)
    
    print(f"💾 详细报告已保存至: {output_file}")
    print(f"{'='*70}\n")


def main():
    parser = argparse.ArgumentParser(description='Full Noun BRAN/PANE 模型生成测试')
    parser.add_argument('refno', nargs='?', help='单个 refno 测试')
    parser.add_argument('db_num', nargs='?', type=int, default=1112, help='数据库编号')
    parser.add_argument('meshes_path', nargs='?', help='可选的 mesh 输出路径')
    parser.add_argument('--benchmark', action='store_true', help='运行性能基准测试')
    parser.add_argument('--type', choices=['bran', 'pane', 'all'], default='all', 
                       help='基准测试类型')
    parser.add_argument('--output', default='performance_report.json', 
                       help='性能报告输出文件')
    
    args = parser.parse_args()
    
    if args.benchmark:
        # 性能基准测试模式
        results = []
        
        if args.type in ['bran', 'all']:
            results.extend(run_benchmark(BRAN_TEST_CASES, "BRAN"))
        
        if args.type in ['pane', 'all']:
            results.extend(run_benchmark(PANE_TEST_CASES, "PANE"))
        
        generate_report(results, args.output)
        
    else:
        # 单个 refno 测试模式
        refno = args.refno or "299"
        db_num = args.db_num
        meshes_path = args.meshes_path
        
        print(f"\n💡 提示: refno 格式应该是 'db_num/refno'，例如 '1112/299'")
        print(f"💡 如果要测试 15201，应该使用 '1112/15201' 或直接使用数字 '15201'")
        
        result = test_refno_generation(db_num, refno, meshes_path, verbose=True)
        
        if result["success"]:
            print("\n✅ 测试完成！")
            print(f"💡 提示: 请查看服务器日志以监控任务执行进度")
            print(f"💡 Chrome Tracing: 检查 chrome_trace_cata_model.json 文件")
            sys.exit(0)
        else:
            print("\n❌ 测试失败！")
            sys.exit(1)


if __name__ == "__main__":
    main()

