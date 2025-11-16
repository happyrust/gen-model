#!/usr/bin/env python3
"""
专用 BRAN/PANE 性能测试脚本
用于深度分析 Full Noun 模式下的 BRAN 和 PANE 生成性能
"""

import requests
import json
import sys
import time
from typing import List, Dict, Optional
from datetime import datetime
import statistics

API_URL = "http://localhost:8080/api/model/generate-by-refno"

# 扩展的 BRAN 测试用例（确保这些 refno 在数据库中存在）
BRAN_EXTENDED_CASES = [
    {"db_num": 1112, "refno": "15201", "desc": "BRAN管道分支-小型"},
    {"db_num": 1112, "refno": "15202", "desc": "BRAN管道分支-中型"},
    {"db_num": 1112, "refno": "15203", "desc": "BRAN支吊架-复杂"},
    {"db_num": 1112, "refno": "15204", "desc": "BRAN管道分支-大型"},
    {"db_num": 1112, "refno": "15205", "desc": "BRAN支吊架-简单"},
]

# 扩展的 PANE 测试用例
PANE_EXTENDED_CASES = [
    {"db_num": 1112, "refno": "299", "desc": "PANE墙板-小型"},
    {"db_num": 1112, "refno": "300", "desc": "PANE楼板-标准"},
    {"db_num": 1112, "refno": "301", "desc": "PANE墙板-复杂"},
    {"db_num": 1112, "refno": "302", "desc": "FLOOR楼板-大型"},
    {"db_num": 1112, "refno": "303", "desc": "GWALL通用墙-标准"},
]


def test_single_refno(db_num: int, refno: str, repeat: int = 3) -> Dict:
    """测试单个 refno，支持多次重复以获取稳定的性能数据"""
    results = []
    
    for i in range(repeat):
        payload = {
            "db_num": db_num,
            "refnos": [refno],
            "gen_mesh": True,
            "gen_model": True,
            "apply_boolean_operation": True
        }
        
        try:
            start_time = time.time()
            response = requests.post(API_URL, json=payload, timeout=60)
            response_time = (time.time() - start_time) * 1000
            
            if response.status_code == 200:
                data = response.json()
                results.append({
                    "success": data.get("success", False),
                    "response_time_ms": response_time,
                    "task_id": data.get("task_id"),
                })
            else:
                results.append({
                    "success": False,
                    "response_time_ms": response_time,
                    "error": f"HTTP {response.status_code}"
                })
        except Exception as e:
            results.append({
                "success": False,
                "response_time_ms": 0,
                "error": str(e)
            })
        
        # 避免请求过快
        if i < repeat - 1:
            time.sleep(2)
    
    # 汇总统计
    successful = [r for r in results if r["success"]]
    if successful:
        times = [r["response_time_ms"] for r in successful]
        return {
            "db_num": db_num,
            "refno": refno,
            "success_rate": len(successful) / len(results),
            "avg_response_ms": statistics.mean(times),
            "min_response_ms": min(times),
            "max_response_ms": max(times),
            "stddev_response_ms": statistics.stdev(times) if len(times) > 1 else 0,
            "repeat_count": repeat,
            "raw_results": results
        }
    else:
        return {
            "db_num": db_num,
            "refno": refno,
            "success_rate": 0,
            "error": "All attempts failed",
            "raw_results": results
        }


def run_performance_suite(
    test_cases: List[Dict], 
    test_type: str,
    repeat_per_case: int = 3
) -> Dict:
    """运行完整的性能测试套件"""
    print(f"\n{'='*80}")
    print(f"🎯 {test_type} 性能测试套件")
    print(f"{'='*80}")
    print(f"测试用例数: {len(test_cases)}")
    print(f"每个用例重复次数: {repeat_per_case}")
    print(f"开始时间: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
    print(f"{'='*80}\n")
    
    suite_start = time.time()
    results = []
    
    for i, case in enumerate(test_cases, 1):
        print(f"\n[{i}/{len(test_cases)}] 测试: {case['desc']}")
        print(f"  Refno: {case['refno']}, 重复: {repeat_per_case} 次")
        
        result = test_single_refno(
            db_num=case["db_num"],
            refno=case["refno"],
            repeat=repeat_per_case
        )
        
        result["desc"] = case["desc"]
        result["test_type"] = test_type
        results.append(result)
        
        if result["success_rate"] > 0:
            print(f"  ✅ 成功率: {result['success_rate']*100:.1f}%")
            print(f"  📊 响应时间: {result['avg_response_ms']:.2f} ms (±{result.get('stddev_response_ms', 0):.2f})")
            print(f"  📈 范围: {result['min_response_ms']:.2f} - {result['max_response_ms']:.2f} ms")
        else:
            print(f"  ❌ 失败: {result.get('error', 'Unknown error')}")
    
    suite_duration = time.time() - suite_start
    
    return {
        "test_type": test_type,
        "suite_duration_s": suite_duration,
        "total_cases": len(test_cases),
        "repeat_per_case": repeat_per_case,
        "results": results
    }


def analyze_and_report(bran_suite: Dict, pane_suite: Dict, output_file: str):
    """分析结果并生成详细报告"""
    print(f"\n{'='*80}")
    print(f"📊 综合性能分析报告")
    print(f"{'='*80}\n")
    
    # BRAN 分析
    bran_results = bran_suite["results"]
    bran_successful = [r for r in bran_results if r["success_rate"] > 0]
    
    if bran_successful:
        bran_avg_times = [r["avg_response_ms"] for r in bran_successful]
        print(f"📌 BRAN 性能统计:")
        print(f"  成功率: {len(bran_successful)}/{len(bran_results)} ({len(bran_successful)/len(bran_results)*100:.1f}%)")
        print(f"  平均响应时间: {statistics.mean(bran_avg_times):.2f} ms")
        print(f"  响应时间中位数: {statistics.median(bran_avg_times):.2f} ms")
        print(f"  最快响应: {min(bran_avg_times):.2f} ms")
        print(f"  最慢响应: {max(bran_avg_times):.2f} ms")
        print(f"  标准差: {statistics.stdev(bran_avg_times) if len(bran_avg_times) > 1 else 0:.2f} ms")
        print()
    
    # PANE 分析
    pane_results = pane_suite["results"]
    pane_successful = [r for r in pane_results if r["success_rate"] > 0]
    
    if pane_successful:
        pane_avg_times = [r["avg_response_ms"] for r in pane_successful]
        print(f"📌 PANE 性能统计:")
        print(f"  成功率: {len(pane_successful)}/{len(pane_results)} ({len(pane_successful)/len(pane_results)*100:.1f}%)")
        print(f"  平均响应时间: {statistics.mean(pane_avg_times):.2f} ms")
        print(f"  响应时间中位数: {statistics.median(pane_avg_times):.2f} ms")
        print(f"  最快响应: {min(pane_avg_times):.2f} ms")
        print(f"  最慢响应: {max(pane_avg_times):.2f} ms")
        print(f"  标准差: {statistics.stdev(pane_avg_times) if len(pane_avg_times) > 1 else 0:.2f} ms")
        print()
    
    # 对比分析
    if bran_successful and pane_successful:
        bran_mean = statistics.mean(bran_avg_times)
        pane_mean = statistics.mean(pane_avg_times)
        print(f"🔄 BRAN vs PANE 对比:")
        print(f"  BRAN 平均: {bran_mean:.2f} ms")
        print(f"  PANE 平均: {pane_mean:.2f} ms")
        print(f"  差异: {abs(bran_mean - pane_mean):.2f} ms ({abs(bran_mean - pane_mean) / min(bran_mean, pane_mean) * 100:.1f}%)")
        if bran_mean > pane_mean:
            print(f"  ⚠️  BRAN 比 PANE 慢 {(bran_mean / pane_mean - 1) * 100:.1f}%")
        else:
            print(f"  ℹ️  PANE 比 BRAN 慢 {(pane_mean / bran_mean - 1) * 100:.1f}%")
        print()
    
    # 保存详细报告
    report = {
        "timestamp": datetime.now().isoformat(),
        "bran_suite": bran_suite,
        "pane_suite": pane_suite,
        "summary": {
            "bran": {
                "total_cases": len(bran_results),
                "successful_cases": len(bran_successful),
                "avg_response_ms": statistics.mean(bran_avg_times) if bran_successful else None
            },
            "pane": {
                "total_cases": len(pane_results),
                "successful_cases": len(pane_successful),
                "avg_response_ms": statistics.mean(pane_avg_times) if pane_successful else None
            }
        }
    }
    
    with open(output_file, 'w', encoding='utf-8') as f:
        json.dump(report, f, indent=2, ensure_ascii=False)
    
    print(f"💾 详细报告已保存至: {output_file}")
    print(f"{'='*80}\n")


def main():
    import argparse
    
    parser = argparse.ArgumentParser(
        description='BRAN/PANE 深度性能分析',
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
示例:
  # 运行完整测试套件（每个用例重复3次）
  python3 test_bran_pane_performance.py --full
  
  # 快速测试（每个用例1次）
  python3 test_bran_pane_performance.py --quick
  
  # 深度测试（每个用例5次）
  python3 test_bran_pane_performance.py --full --repeat 5
        """
    )
    
    parser.add_argument('--quick', action='store_true', help='快速测试（每个用例1次）')
    parser.add_argument('--full', action='store_true', help='完整测试套件')
    parser.add_argument('--repeat', type=int, default=3, help='每个用例的重复次数')
    parser.add_argument('--output', default='bran_pane_performance_report.json', help='报告输出文件')
    parser.add_argument('--type', choices=['bran', 'pane', 'both'], default='both', help='测试类型')
    
    args = parser.parse_args()
    
    if args.quick:
        repeat = 1
    else:
        repeat = args.repeat
    
    # 运行测试
    bran_suite = None
    pane_suite = None
    
    if args.type in ['bran', 'both']:
        bran_suite = run_performance_suite(BRAN_EXTENDED_CASES, "BRAN", repeat)
    
    if args.type in ['pane', 'both']:
        pane_suite = run_performance_suite(PANE_EXTENDED_CASES, "PANE", repeat)
    
    # 生成报告
    if bran_suite and pane_suite:
        analyze_and_report(bran_suite, pane_suite, args.output)
    elif bran_suite:
        print(f"\n💾 BRAN 报告已保存至: {args.output}")
        with open(args.output, 'w', encoding='utf-8') as f:
            json.dump(bran_suite, f, indent=2, ensure_ascii=False)
    elif pane_suite:
        print(f"\n💾 PANE 报告已保存至: {args.output}")
        with open(args.output, 'w', encoding='utf-8') as f:
            json.dump(pane_suite, f, indent=2, ensure_ascii=False)
    
    print("\n✅ 性能测试完成！")
    print("💡 提示:")
    print("  - 查看 chrome_trace_cata_model.json 获取详细性能追踪")
    print("  - 在 chrome://tracing 中打开追踪文件进行可视化分析")
    print("  - 检查服务器日志获取详细的性能指标")


if __name__ == "__main__":
    main()
