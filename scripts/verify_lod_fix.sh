#!/bin/bash
# LOD 修复验证脚本

set -e

echo "🔍 LOD 修复验证脚本"
echo "===================="

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# 1. 检查 mesh 源文件
echo -e "\n${YELLOW}步骤 1: 检查 mesh 源文件${NC}"
echo "-------------------"

MESH_DIR="assets/meshes"

if [ ! -d "$MESH_DIR/lod_L1" ] || [ ! -d "$MESH_DIR/lod_L3" ]; then
    echo -e "${RED}❌ LOD 目录不存在！请先生成 mesh 文件。${NC}"
    exit 1
fi

echo "检查 LOD 目录..."
for lod in L1 L2 L3; do
    dir="$MESH_DIR/lod_$lod"
    if [ -d "$dir" ]; then
        count=$(ls "$dir"/*.mesh 2>/dev/null | wc -l)
        echo -e "${GREEN}✓${NC} lod_$lod: $count 个 mesh 文件"

        # 显示第一个文件的大小
        first_file=$(ls "$dir"/*.mesh 2>/dev/null | head -1)
        if [ -n "$first_file" ]; then
            size=$(ls -lh "$first_file" | awk '{print $5}')
            echo "  示例: $(basename $first_file) - $size"
        fi
    else
        echo -e "${RED}✗${NC} lod_$lod: 目录不存在"
    fi
done

# 2. 对比文件大小
echo -e "\n${YELLOW}步骤 2: 对比不同 LOD 级别的文件大小${NC}"
echo "-------------------"

# 查找一个共同的 geo_hash
geo_hash=$(ls "$MESH_DIR/lod_L1"/*.mesh 2>/dev/null | head -1 | xargs basename | sed 's/_L1.mesh//')

if [ -z "$geo_hash" ]; then
    echo -e "${RED}❌ 未找到可对比的 mesh 文件${NC}"
    exit 1
fi

echo "使用 geo_hash: $geo_hash 进行对比"
echo ""

for lod in L1 L2 L3; do
    file="$MESH_DIR/lod_$lod/${geo_hash}_$lod.mesh"
    if [ -f "$file" ]; then
        size=$(stat -f%z "$file")
        size_human=$(ls -lh "$file" | awk '{print $5}')
        echo "  $lod: $size_human ($size bytes)"
    else
        echo -e "  $lod: ${RED}文件不存在${NC}"
    fi
done

# 3. 检查现有的 GLB 输出文件
echo -e "\n${YELLOW}步骤 3: 检查现有的 GLB 输出文件${NC}"
echo "-------------------"

OUTPUT_DIR="output/instanced-bundle/all_relates_all"

if [ -d "$OUTPUT_DIR" ]; then
    echo "输出目录: $OUTPUT_DIR"

    if [ -f "$OUTPUT_DIR/geometry_manifest.json" ]; then
        echo -e "\n${GREEN}✓${NC} 找到 geometry_manifest.json"

        # 提取第一个几何体的 LOD 信息
        echo -e "\n第一个几何体的 LOD 信息:"
        cat "$OUTPUT_DIR/geometry_manifest.json" | python3 -c "
import sys, json
data = json.load(sys.stdin)
if data.get('geometries'):
    geo = data['geometries'][0]
    print(f\"  geo_hash: {geo.get('geo_hash', 'N/A')}\")
    if geo.get('lods'):
        for lod in geo['lods']:
            level = lod.get('level', 'N/A')
            tri_count = lod.get('triangle_count', 'N/A')
            print(f\"    L{level}: {tri_count} 三角形\")
" 2>/dev/null || echo "  (需要 Python 3 来解析 JSON)"
    else
        echo -e "${YELLOW}⚠${NC}  geometry_manifest.json 不存在"
    fi

    # 检查 GLB 文件大小
    echo -e "\nGLB 文件大小:"
    for glb in "$OUTPUT_DIR"/*.glb; do
        if [ -f "$glb" ]; then
            size=$(ls -lh "$glb" | awk '{print $5}')
            name=$(basename "$glb")
            echo "  $name: $size"
        fi
    done
else
    echo -e "${YELLOW}⚠${NC}  输出目录不存在: $OUTPUT_DIR"
    echo "  请先运行导出命令生成输出文件"
fi

# 4. 提供重新生成建议
echo -e "\n${YELLOW}步骤 4: 重新生成验证${NC}"
echo "-------------------"
echo "要验证修复效果，请执行以下命令："
echo ""
echo -e "${GREEN}# 1. 删除旧的输出文件${NC}"
echo "rm -rf output/instanced-bundle/all_relates_all/"
echo ""
echo -e "${GREEN}# 2. 重新生成 instanced bundle（使用你的实际命令）${NC}"
echo "cargo run --release -- <your-export-command> --verbose"
echo ""
echo -e "${GREEN}# 3. 再次运行此脚本进行验证${NC}"
echo "./scripts/verify_lod_fix.sh"

# 5. 总结
echo -e "\n${YELLOW}修复摘要${NC}"
echo "-------------------"
echo "✓ 修复了 export_instanced_bundle.rs 中的 LOD 加载逻辑"
echo "✓ 现在会从对应的 lod_L1、lod_L2、lod_L3 目录加载不同精度的 mesh"
echo "✓ 每个 LOD 级别的 GLB 文件将包含不同数量的顶点和三角形"
echo ""
echo "预期效果："
echo "  • L1 (低精度): 最少的顶点/三角形，文件最小"
echo "  • L2 (中精度): 中等数量的顶点/三角形"
echo "  • L3 (高精度): 最多的顶点/三角形，文件最大"
echo ""
echo -e "${GREEN}🎉 验证完成！${NC}"
