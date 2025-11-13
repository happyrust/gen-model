#!/bin/bash
# 应用 LOD 配置修复脚本

set -e

echo "🔧 LOD 配置修复脚本"
echo "===================="

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

CONFIG_FILE="DbOption.toml"
BACKUP_FILE="DbOption.toml.backup.$(date +%Y%m%d_%H%M%S)"

# 检查配置文件是否存在
if [ ! -f "$CONFIG_FILE" ]; then
    echo -e "${RED}❌ 错误：找不到 $CONFIG_FILE${NC}"
    exit 1
fi

echo -e "${YELLOW}步骤 1: 备份当前配置${NC}"
echo "-------------------"
cp "$CONFIG_FILE" "$BACKUP_FILE"
echo -e "${GREEN}✓${NC} 已备份到: $BACKUP_FILE"

echo -e "\n${YELLOW}步骤 2: 应用 LOD 修复配置${NC}"
echo "-------------------"

# 使用 sed 或 perl 修改配置文件
# 注意：这里使用简单的替换，实际项目中可能需要更复杂的逻辑

echo "修改 L1 配置..."

# L1.csg_settings 修改
if [[ "$OSTYPE" == "darwin"* ]]; then
    # macOS 使用 BSD sed
    sed -i '' '/\[mesh_precision.lod_profiles.L1.csg_settings\]/,/\[mesh_precision.lod_profiles.L2/ {
        s/^radial_segments = 12$/radial_segments = 10/
        s/^min_radial_segments = 12$/min_radial_segments = 6/
        s/^max_radial_segments = 18$/max_radial_segments = 14/
        s/^max_height_segments = 2$/max_height_segments = 1/
        s/^target_segment_length = 250.0$/target_segment_length = 350.0/
        s/^non_scalable_factor = 0.9$/non_scalable_factor = 1.0/
        s/^error_tolerance = 0.01$/error_tolerance = 0.015/
    }' "$CONFIG_FILE"
else
    # Linux 使用 GNU sed
    sed -i '/\[mesh_precision.lod_profiles.L1.csg_settings\]/,/\[mesh_precision.lod_profiles.L2/ {
        s/^radial_segments = 12$/radial_segments = 10/
        s/^min_radial_segments = 12$/min_radial_segments = 6/
        s/^max_radial_segments = 18$/max_radial_segments = 14/
        s/^max_height_segments = 2$/max_height_segments = 1/
        s/^target_segment_length = 250.0$/target_segment_length = 350.0/
        s/^non_scalable_factor = 0.9$/non_scalable_factor = 1.0/
        s/^error_tolerance = 0.01$/error_tolerance = 0.015/
    }' "$CONFIG_FILE"
fi

echo -e "${GREEN}✓${NC} L1 配置已修改"

echo "修改 L2 配置..."

# L2.csg_settings 修改
if [[ "$OSTYPE" == "darwin"* ]]; then
    sed -i '' '/\[mesh_precision.lod_profiles.L2.csg_settings\]/,/\[mesh_precision.lod_profiles.L3/ {
        s/^min_radial_segments = 12$/min_radial_segments = 16/
        s/^max_radial_segments = 30$/max_radial_segments = 32/
        s/^target_segment_length = 140.0$/target_segment_length = 100.0/
        s/^non_scalable_factor = 0.8$/non_scalable_factor = 0.75/
    }' "$CONFIG_FILE"
else
    sed -i '/\[mesh_precision.lod_profiles.L2.csg_settings\]/,/\[mesh_precision.lod_profiles.L3/ {
        s/^min_radial_segments = 12$/min_radial_segments = 16/
        s/^max_radial_segments = 30$/max_radial_segments = 32/
        s/^target_segment_length = 140.0$/target_segment_length = 100.0/
        s/^non_scalable_factor = 0.8$/non_scalable_factor = 0.75/
    }' "$CONFIG_FILE"
fi

echo -e "${GREEN}✓${NC} L2 配置已修改"

echo -e "\n${YELLOW}步骤 3: 验证修改${NC}"
echo "-------------------"

echo -e "\n${BLUE}L1 关键参数:${NC}"
grep -A 10 '\[mesh_precision.lod_profiles.L1.csg_settings\]' "$CONFIG_FILE" | grep -E "(min_radial_segments|max_radial_segments|target_segment_length)" || echo "  (无法提取，请手动检查)"

echo -e "\n${BLUE}L2 关键参数:${NC}"
grep -A 10 '\[mesh_precision.lod_profiles.L2.csg_settings\]' "$CONFIG_FILE" | grep -E "(min_radial_segments|max_radial_segments|target_segment_length)" || echo "  (无法提取，请手动检查)"

echo -e "\n${YELLOW}步骤 4: 下一步操作${NC}"
echo "-------------------"
echo -e "${GREEN}配置已修改完成！${NC}"
echo ""
echo "接下来需要重新生成 mesh 文件："
echo ""
echo -e "${BLUE}# 1. 删除旧的 L1 和 L2 mesh 文件${NC}"
echo "rm -rf assets/meshes/lod_L1/*.mesh"
echo "rm -rf assets/meshes/lod_L2/*.mesh"
echo ""
echo -e "${BLUE}# 2. 重新生成 mesh（使用你的实际命令）${NC}"
echo "cargo run --release -- gen-mesh <refnos> --verbose"
echo ""
echo -e "${BLUE}# 3. 验证文件大小差异${NC}"
echo "ls -lh assets/meshes/lod_L1/*.mesh | head -5"
echo "ls -lh assets/meshes/lod_L2/*.mesh | head -5"
echo ""
echo -e "${BLUE}# 4. 重新导出 GLB${NC}"
echo "rm -rf output/instanced-bundle/all_relates_all/"
echo "cargo run --release -- export-instanced-bundle <refnos> --verbose"
echo ""
echo -e "${YELLOW}如果需要恢复原始配置：${NC}"
echo "cp $BACKUP_FILE $CONFIG_FILE"
echo ""
echo -e "${GREEN}🎉 修复脚本完成！${NC}"
