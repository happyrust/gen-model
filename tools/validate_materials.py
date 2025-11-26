#!/usr/bin/env python3
"""
材质验证工具 - 验证 export-all-relates 导出的材质数据

功能:
1. 验证 instances.json 中的颜色数据
2. 检查 color_index 有效性
3. 统计 noun 类型和实例数
4. 对比 ColorSchemes.toml 配置
5. 生成材质覆盖率报告

使用方法:
    python tools/validate_materials.py ../instanced-mesh/public/bundles/all_relates_all/instances.json
"""

import json
import sys
from pathlib import Path
from typing import Dict, List, Tuple


# ColorSchemes.toml 中的标准 PDMS 配色 (RGBA 0-255)
COLOR_SCHEMES_STANDARD = {
    "UNKOWN": [192, 192, 192, 255],
    "CE": [0, 100, 200, 180],
    "EQUI": [255, 190, 0, 255],
    "PIPE": [255, 255, 0, 255],
    "HANG": [255, 126, 0, 255],
    "STRU": [0, 150, 255, 255],
    "SCTN": [188, 141, 125, 255],
    "GENSEC": [188, 141, 125, 255],
    "WALL": [150, 150, 150, 255],
    "STWALL": [150, 150, 150, 255],
    "CWALL": [120, 120, 120, 255],
    "GWALL": [173, 216, 230, 128],
    "FLOOR": [210, 180, 140, 255],
    "CFLOOR": [160, 130, 100, 255],
    "PANE": [220, 220, 220, 255],
    "ROOM": [144, 238, 144, 100],
    "AREADEF": [221, 160, 221, 80],
    "HVAC": [175, 238, 238, 255],
    "EXTR": [147, 112, 219, 255],
    "REVO": [138, 43, 226, 255],
    "HANDRA": [255, 215, 0, 255],
    "CWBRAN": [255, 140, 0, 255],
    "CTWALL": [176, 196, 222, 150],
    "DEMOPA": [255, 69, 0, 255],
    "INSURQ": [255, 182, 193, 255],
    "STRLNG": [0, 255, 255, 255],
}


def normalize_color(rgba: List[int]) -> List[float]:
    """将 RGBA [0-255] 转换为归一化 [0.0-1.0]"""
    return [c / 255.0 for c in rgba]


def denormalize_color(rgba_norm: List[float]) -> List[int]:
    """将归一化颜色转换回 [0-255]"""
    return [int(round(c * 255)) for c in rgba_norm]


def color_distance(c1: List[float], c2: List[float]) -> float:
    """计算两个颜色的欧氏距离"""
    return sum((a - b) ** 2 for a, b in zip(c1, c2)) ** 0.5


def validate_instances_json(instances_path: Path) -> Dict:
    """验证 instances.json 文件"""

    print(f"\n{'='*70}")
    print(f"材质验证工具 - Export-All-Relates")
    print(f"{'='*70}\n")

    if not instances_path.exists():
        print(f"❌ 错误: 文件不存在 {instances_path}")
        return {}

    # 加载 JSON
    with open(instances_path) as f:
        data = json.load(f)

    colors = data.get("colors", [])
    components = data.get("components", [])
    version = data.get("version", "未知")
    generated_at = data.get("generated_at", "未知")

    # ========== 基础信息 ==========
    print(f"📄 文件信息:")
    print(f"   版本: {version}")
    print(f"   生成时间: {generated_at}")
    print(f"   调色板大小: {len(colors)} 个颜色")
    print(f"   组件类型数: {len(components)} 种")

    # ========== 统计 noun 类型 ==========
    noun_stats: Dict[str, int] = {}
    color_index_usage: Dict[int, int] = {}

    for comp in components:
        noun = comp.get("noun", "UNKNOWN")
        instances = comp.get("instances", [])
        noun_stats[noun] = noun_stats.get(noun, 0) + len(instances)

        for inst in instances:
            color_idx = inst.get("color_index", -1)
            color_index_usage[color_idx] = color_index_usage.get(color_idx, 0) + 1

    total_instances = sum(noun_stats.values())
    print(f"   总实例数: {total_instances:,}")

    # ========== 检查 color_index 有效性 ==========
    print(f"\n✅ Color Index 验证:")
    max_color_index = len(colors) - 1
    invalid_indices = []

    for idx in color_index_usage.keys():
        if idx < 0 or idx > max_color_index:
            invalid_indices.append(idx)

    if invalid_indices:
        print(f"   ❌ 发现 {len(invalid_indices)} 个无效索引: {invalid_indices}")
    else:
        print(f"   ✅ 所有 color_index 都在有效范围内 [0-{max_color_index}]")

    # ========== 实例数排名 ==========
    print(f"\n📊 实例数排名 (Top 10):")
    sorted_nouns = sorted(noun_stats.items(), key=lambda x: -x[1])
    for i, (noun, count) in enumerate(sorted_nouns[:10], 1):
        percentage = count / total_instances * 100
        print(f"   {i:2d}. {noun:10} → {count:6,} ({percentage:5.1f}%)")

    # ========== 材质覆盖率分析 ==========
    print(f"\n🎨 材质覆盖率分析:")

    covered_nouns = []
    uncovered_nouns = []

    for noun in noun_stats.keys():
        if noun in COLOR_SCHEMES_STANDARD:
            covered_nouns.append(noun)
        else:
            uncovered_nouns.append(noun)

    total_nouns = len(noun_stats)
    coverage_rate = len(covered_nouns) / total_nouns * 100 if total_nouns > 0 else 0

    print(f"   总类型数: {total_nouns}")
    print(f"   已覆盖: {len(covered_nouns)} ({coverage_rate:.1f}%)")
    print(f"   未覆盖: {len(uncovered_nouns)} ({100-coverage_rate:.1f}%)")

    if covered_nouns:
        print(f"\n   ✅ ColorSchemes.toml 中有定义的类型:")
        for noun in sorted(covered_nouns):
            rgb = COLOR_SCHEMES_STANDARD[noun][:3]
            count = noun_stats[noun]
            print(f"      {noun:10} → RGB{rgb} ({count:,} instances)")

    if uncovered_nouns:
        print(f"\n   ⚠️  未在 ColorSchemes.toml 中定义的类型 (使用 UNKOWN 默认色):")
        for noun in sorted(uncovered_nouns):
            count = noun_stats[noun]
            print(f"      {noun:10} ({count:,} instances)")

    # ========== 颜色验证 ==========
    print(f"\n🔍 颜色数据验证:")

    # 检查是否有重复颜色
    unique_colors = []
    duplicate_count = 0
    for color in colors:
        color_tuple = tuple(color)
        if color_tuple in [tuple(c) for c in unique_colors]:
            duplicate_count += 1
        else:
            unique_colors.append(color)

    print(f"   调色板颜色数: {len(colors)}")
    print(f"   唯一颜色数: {len(unique_colors)}")
    if duplicate_count > 0:
        print(f"   ⚠️  重复颜色: {duplicate_count} 个 (可优化)")
    else:
        print(f"   ✅ 无重复颜色")

    # 检查 UNKOWN 默认色使用情况
    unkown_color_norm = normalize_color(COLOR_SCHEMES_STANDARD["UNKOWN"])
    unkown_usage_count = 0

    for color in colors:
        if color_distance(color, unkown_color_norm) < 0.001:
            unkown_usage_count += 1

    if unkown_usage_count > 0:
        print(f"   ℹ️  UNKOWN 默认色使用次数: {unkown_usage_count} (几何原语)")

    # ========== 颜色匹配验证 ==========
    print(f"\n🎯 ColorSchemes 匹配验证 (前5个颜色):")

    for i, color_norm in enumerate(colors[:5]):
        color_int = denormalize_color(color_norm)

        # 查找最接近的 ColorSchemes 定义
        min_distance = float('inf')
        closest_noun = "未知"
        for noun, scheme_color in COLOR_SCHEMES_STANDARD.items():
            scheme_norm = normalize_color(scheme_color)
            distance = color_distance(color_norm, scheme_norm)
            if distance < min_distance:
                min_distance = distance
                closest_noun = noun

        match_status = "✅" if min_distance < 0.001 else "⚠️"
        print(f"   [{i}] RGB{color_int[:3]} → {closest_noun:10} (距离: {min_distance:.6f}) {match_status}")

    # ========== 总结 ==========
    print(f"\n{'='*70}")
    print(f"验证完成!")
    print(f"{'='*70}\n")

    return {
        "total_colors": len(colors),
        "total_nouns": total_nouns,
        "total_instances": total_instances,
        "coverage_rate": coverage_rate,
        "has_invalid_indices": bool(invalid_indices),
        "covered_nouns": covered_nouns,
        "uncovered_nouns": uncovered_nouns,
    }


def main():
    if len(sys.argv) < 2:
        print("用法: python validate_materials.py <instances.json路径>")
        print("\n例如:")
        print("  python tools/validate_materials.py ../instanced-mesh/public/bundles/all_relates_all/instances.json")
        sys.exit(1)

    instances_path = Path(sys.argv[1])
    result = validate_instances_json(instances_path)

    # 根据验证结果返回退出码
    if result.get("has_invalid_indices", False):
        sys.exit(1)  # 有错误
    else:
        sys.exit(0)  # 成功


if __name__ == "__main__":
    main()
