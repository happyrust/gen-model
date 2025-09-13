#!/bin/bash

# 空间索引构建脚本
# 用法: ./build_spatial_index.sh [options]

set -e  # 遇到错误时退出

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# 默认配置
DEFAULT_DB_NOS="1,2,3"
DEFAULT_OUTPUT="./spatial_index.bin"
DEFAULT_BATCH_SIZE="10000"
DEFAULT_TOLERANCE="0.001"
DEFAULT_MIN_BBOX_SIZE="0.0001"

echo -e "${BLUE}🏗️  空间索引构建工具${NC}"
echo "=========================================="

# 显示帮助信息
show_help() {
    echo "用法: $0 [选项]"
    echo ""
    echo "选项:"
    echo "  -d, --db-nos DB_NUMBERS     数据库编号列表 (逗号分隔, 默认: $DEFAULT_DB_NOS)"
    echo "  -o, --output OUTPUT_FILE    输出文件路径 (默认: $DEFAULT_OUTPUT)"
    echo "  -b, --batch-size SIZE       批量处理大小 (默认: $DEFAULT_BATCH_SIZE)"
    echo "  -t, --tolerance TOLERANCE   包围盒容差 (默认: $DEFAULT_TOLERANCE)"
    echo "  -f, --filter TYPES          过滤构件类型 (逗号分隔, 可选)"
    echo "  -m, --min-bbox MIN_SIZE     最小包围盒尺寸 (默认: $DEFAULT_MIN_BBOX_SIZE)"
    echo "  -v, --validate             构建后验证索引"
    echo "  -s, --stats                构建后显示统计信息"
    echo "  -c, --clean                构建前清理旧索引文件"
    echo "  -h, --help                 显示此帮助信息"
    echo ""
    echo "示例:"
    echo "  $0                                          # 使用默认配置"
    echo "  $0 -d 1,2 -o my_index.bin                 # 指定数据库和输出文件"
    echo "  $0 -d 1 -f PIPE,EQUI -v                   # 只索引管道和设备，并验证"
    echo "  $0 -d 1,2,3 -b 5000 -t 0.01 -s           # 自定义参数并显示统计"
}

# 解析命令行参数
DB_NOS="$DEFAULT_DB_NOS"
OUTPUT="$DEFAULT_OUTPUT"
BATCH_SIZE="$DEFAULT_BATCH_SIZE"
TOLERANCE="$DEFAULT_TOLERANCE"
MIN_BBOX_SIZE="$DEFAULT_MIN_BBOX_SIZE"
FILTER_TYPES=""
VALIDATE=false
SHOW_STATS=false
CLEAN=false

while [[ $# -gt 0 ]]; do
    case $1 in
        -d|--db-nos)
            DB_NOS="$2"
            shift 2
            ;;
        -o|--output)
            OUTPUT="$2"
            shift 2
            ;;
        -b|--batch-size)
            BATCH_SIZE="$2"
            shift 2
            ;;
        -t|--tolerance)
            TOLERANCE="$2"
            shift 2
            ;;
        -f|--filter)
            FILTER_TYPES="$2"
            shift 2
            ;;
        -m|--min-bbox)
            MIN_BBOX_SIZE="$2"
            shift 2
            ;;
        -v|--validate)
            VALIDATE=true
            shift
            ;;
        -s|--stats)
            SHOW_STATS=true
            shift
            ;;
        -c|--clean)
            CLEAN=true
            shift
            ;;
        -h|--help)
            show_help
            exit 0
            ;;
        *)
            echo -e "${RED}❌ 未知选项: $1${NC}"
            show_help
            exit 1
            ;;
    esac
done

# 验证环境
echo -e "${BLUE}🔍 验证构建环境...${NC}"

if [ ! -f "Cargo.toml" ]; then
    echo -e "${RED}❌ 错误: 请在项目根目录运行此脚本${NC}"
    exit 1
fi

if ! command -v cargo &> /dev/null; then
    echo -e "${RED}❌ 错误: 找不到 cargo 命令${NC}"
    exit 1
fi

echo -e "${GREEN}✅ 环境检查通过${NC}"

# 清理旧文件
if [ "$CLEAN" = true ] && [ -f "$OUTPUT" ]; then
    echo -e "${YELLOW}🧹 清理旧索引文件: $OUTPUT${NC}"
    rm -f "$OUTPUT"
fi

# 编译项目
echo -e "${BLUE}🔨 编译项目...${NC}"
if cargo build --example spatial_index_builder --features grpc; then
    echo -e "${GREEN}✅ 编译成功${NC}"
else
    echo -e "${RED}❌ 编译失败${NC}"
    exit 1
fi

# 构建命令参数
BUILD_ARGS="--db-nos $DB_NOS --output $OUTPUT --batch-size $BATCH_SIZE --tolerance $TOLERANCE --min-bbox-size $MIN_BBOX_SIZE"

if [ -n "$FILTER_TYPES" ]; then
    BUILD_ARGS="$BUILD_ARGS --filter-types $FILTER_TYPES"
fi

echo -e "${BLUE}🏗️  构建空间索引...${NC}"
echo "   数据库编号: $DB_NOS"
echo "   输出文件: $OUTPUT"
echo "   批量大小: $BATCH_SIZE"
echo "   容差: $TOLERANCE"
echo "   最小包围盒: $MIN_BBOX_SIZE"
if [ -n "$FILTER_TYPES" ]; then
    echo "   过滤类型: $FILTER_TYPES"
fi

# 记录开始时间
START_TIME=$(date +%s)

# 执行构建
if cargo run --example spatial_index_builder --features grpc -- build $BUILD_ARGS; then
    END_TIME=$(date +%s)
    DURATION=$((END_TIME - START_TIME))
    
    echo -e "${GREEN}✅ 索引构建完成! 耗时: ${DURATION}秒${NC}"
    
    # 显示文件信息
    if [ -f "$OUTPUT" ]; then
        FILE_SIZE=$(ls -lh "$OUTPUT" | awk '{print $5}')
        echo -e "${GREEN}📁 索引文件: $OUTPUT (大小: $FILE_SIZE)${NC}"
    fi
    
    # 验证索引
    if [ "$VALIDATE" = true ]; then
        echo -e "${BLUE}🔍 验证索引文件...${NC}"
        if cargo run --example spatial_index_builder --features grpc -- validate --file "$OUTPUT"; then
            echo -e "${GREEN}✅ 索引验证通过${NC}"
        else
            echo -e "${YELLOW}⚠️  索引验证失败${NC}"
        fi
    fi
    
    # 显示统计信息
    if [ "$SHOW_STATS" = true ]; then
        echo -e "${BLUE}📊 显示索引统计...${NC}"
        cargo run --example spatial_index_builder --features grpc -- stats --file "$OUTPUT"
    fi
    
else
    echo -e "${RED}❌ 索引构建失败${NC}"
    exit 1
fi

echo ""
echo -e "${BLUE}💡 使用提示:${NC}"
echo "   1. 启动空间查询服务器时使用预构建索引:"
echo "      cargo run --example spatial_query_server --features grpc -- --index-file $OUTPUT"
echo ""
echo "   2. 验证索引文件:"
echo "      cargo run --example spatial_index_builder --features grpc -- validate --file $OUTPUT"
echo ""
echo "   3. 查看索引统计:"
echo "      cargo run --example spatial_index_builder --features grpc -- stats --file $OUTPUT"
echo ""
echo -e "${GREEN}🎉 空间索引构建流程完成!${NC}"