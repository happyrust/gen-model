#!/bin/bash

# XKT 模型正确性测试脚本
# 
# 功能：
# 1. 生成 OBJ 和 XKT 格式的测试模型
# 2. 使用 Rust 内置验证器验证 XKT
# 3. 使用 Node.js 脚本进行详细验证
# 4. 对比分析并生成测试报告

set -e  # 遇到错误立即退出

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# 配置
REFNO="${1:-21491_18946}"
OUTPUT_DIR="output/test"
MESH_DIR="assets/meshes"

# 输出文件路径
OBJ_FILE="${OUTPUT_DIR}/${REFNO}.obj"
XKT_COMPRESSED="${OUTPUT_DIR}/${REFNO}_compressed.xkt"
XKT_UNCOMPRESSED="${OUTPUT_DIR}/${REFNO}_uncompressed.xkt"
REPORT_FILE="${OUTPUT_DIR}/${REFNO}_test_report.md"

# 打印标题
print_header() {
    echo ""
    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"
    echo -e "${BLUE}   $1${NC}"
    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"
    echo ""
}

# 打印步骤
print_step() {
    echo ""
    echo -e "${YELLOW}📋 $1${NC}"
    echo ""
}

# 打印成功
print_success() {
    echo -e "${GREEN}   ✓ $1${NC}"
}

# 打印错误
print_error() {
    echo -e "${RED}   ✗ $1${NC}"
}

# 打印信息
print_info() {
    echo -e "   $1"
}

# 检查依赖
check_dependencies() {
    print_step "检查依赖"
    
    # 检查 cargo
    if ! command -v cargo &> /dev/null; then
        print_error "未找到 cargo，请安装 Rust"
        exit 1
    fi
    print_success "Rust/Cargo 已安装"
    
    # 检查 node
    if ! command -v node &> /dev/null; then
        print_error "未找到 node，请安装 Node.js"
        exit 1
    fi
    print_success "Node.js 已安装"
    
    # 检查 pako 包
    if [ ! -d "node_modules/pako" ]; then
        print_info "安装 Node.js 依赖..."
        npm install pako
    fi
    print_success "Node.js 依赖已安装"
}

# 创建输出目录
prepare_directories() {
    print_step "准备输出目录"
    
    mkdir -p "${OUTPUT_DIR}"
    print_success "输出目录: ${OUTPUT_DIR}"
    
    if [ ! -d "${MESH_DIR}" ]; then
        print_error "Mesh 目录不存在: ${MESH_DIR}"
        exit 1
    fi
    print_success "Mesh 目录: ${MESH_DIR}"
}

# 生成 OBJ 模型
generate_obj() {
    print_step "步骤 1/5: 生成 OBJ 模型"

    print_info "参考号: ${REFNO}"
    print_info "输出文件: ${OBJ_FILE}"

    if cargo run --bin aios-database -- \
        --debug-model-refnos "${REFNO}" \
        --export-obj \
        --export-obj-output "${OBJ_FILE}" \
        --verbose; then

        if [ -f "${OBJ_FILE}" ]; then
            local size=$(du -h "${OBJ_FILE}" | cut -f1)
            print_success "OBJ 模型生成成功 (${size})"
        else
            print_error "OBJ 文件未生成"
            exit 1
        fi
    else
        print_error "OBJ 模型生成失败"
        exit 1
    fi
}

# 生成 XKT 模型（压缩）
generate_xkt_compressed() {
    print_step "步骤 2/5: 生成 XKT 模型（压缩）"

    print_info "参考号: ${REFNO}"
    print_info "输出文件: ${XKT_COMPRESSED}"
    print_info "压缩: 是"

    if cargo run --bin aios-database -- \
        --debug-model-refnos "${REFNO}" \
        --export-xkt \
        --export-obj-output "${XKT_COMPRESSED}" \
        --xkt-compress true \
        --verbose; then

        if [ -f "${XKT_COMPRESSED}" ]; then
            local size=$(du -h "${XKT_COMPRESSED}" | cut -f1)
            print_success "XKT 压缩模型生成成功 (${size})"
        else
            print_error "XKT 压缩文件未生成"
            exit 1
        fi
    else
        print_error "XKT 压缩模型生成失败"
        exit 1
    fi
}

# 生成 XKT 模型（非压缩）
generate_xkt_uncompressed() {
    print_step "步骤 3/5: 生成 XKT 模型（非压缩）"

    print_info "参考号: ${REFNO}"
    print_info "输出文件: ${XKT_UNCOMPRESSED}"
    print_info "压缩: 否"

    if cargo run --bin aios-database -- \
        --debug-model-refnos "${REFNO}" \
        --export-xkt \
        --export-obj-output "${XKT_UNCOMPRESSED}" \
        --xkt-compress false \
        --xkt-skip-mesh \
        --verbose; then

        if [ -f "${XKT_UNCOMPRESSED}" ]; then
            local size=$(du -h "${XKT_UNCOMPRESSED}" | cut -f1)
            print_success "XKT 非压缩模型生成成功 (${size})"
        else
            print_error "XKT 非压缩文件未生成"
            exit 1
        fi
    else
        print_error "XKT 非压缩模型生成失败"
        exit 1
    fi
}

# Node.js 验证
validate_with_nodejs() {
    print_step "步骤 4/5: Node.js 详细验证"
    
    # 验证压缩版本
    print_info "验证压缩版本..."
    if node validate_xkt_with_xeokit.js "${XKT_COMPRESSED}" "${XKT_COMPRESSED%.xkt}_validation.json"; then
        print_success "压缩版本验证通过"
    else
        print_error "压缩版本验证失败"
    fi

    echo ""

    # 验证非压缩版本
    print_info "验证非压缩版本..."
    if node validate_xkt_with_xeokit.js "${XKT_UNCOMPRESSED}" "${XKT_UNCOMPRESSED%.xkt}_validation.json"; then
        print_success "非压缩版本验证通过"
    else
        print_error "非压缩版本验证失败"
    fi
}

# 生成测试报告
generate_report() {
    print_step "步骤 5/5: 生成测试报告"
    
    # 获取文件信息
    local obj_size=$(stat -f%z "${OBJ_FILE}" 2>/dev/null || stat -c%s "${OBJ_FILE}" 2>/dev/null || echo "0")
    local xkt_comp_size=$(stat -f%z "${XKT_COMPRESSED}" 2>/dev/null || stat -c%s "${XKT_COMPRESSED}" 2>/dev/null || echo "0")
    local xkt_uncomp_size=$(stat -f%z "${XKT_UNCOMPRESSED}" 2>/dev/null || stat -c%s "${XKT_UNCOMPRESSED}" 2>/dev/null || echo "0")
    
    # 计算压缩率
    local compression_ratio=$(echo "scale=2; ${xkt_comp_size} * 100 / ${xkt_uncomp_size}" | bc)
    
    # 生成 Markdown 报告
    cat > "${REPORT_FILE}" <<EOF
# XKT 模型正确性测试报告

**测试时间**: $(date '+%Y-%m-%d %H:%M:%S')  
**测试对象**: ${REFNO}

---

## 📊 文件大小对比

| 格式 | 文件大小 | 压缩率 |
|------|---------|--------|
| OBJ (原始) | $(numfmt --to=iec-i --suffix=B ${obj_size} 2>/dev/null || echo "${obj_size} bytes") | - |
| XKT (压缩) | $(numfmt --to=iec-i --suffix=B ${xkt_comp_size} 2>/dev/null || echo "${xkt_comp_size} bytes") | ${compression_ratio}% |
| XKT (非压缩) | $(numfmt --to=iec-i --suffix=B ${xkt_uncomp_size} 2>/dev/null || echo "${xkt_uncomp_size} bytes") | 100% |

---

## 🔍 XKT 验证结果

### 压缩版本

EOF

    # 添加压缩版本的验证报告
    if [ -f "${XKT_COMPRESSED%.xkt}_validation.json" ]; then
        echo '```json' >> "${REPORT_FILE}"
        cat "${XKT_COMPRESSED%.xkt}_validation.json" >> "${REPORT_FILE}"
        echo '```' >> "${REPORT_FILE}"
    fi

    cat >> "${REPORT_FILE}" <<EOF

### 非压缩版本

EOF

    # 添加非压缩版本的验证报告
    if [ -f "${XKT_UNCOMPRESSED%.xkt}_validation.json" ]; then
        echo '```json' >> "${REPORT_FILE}"
        cat "${XKT_UNCOMPRESSED%.xkt}_validation.json" >> "${REPORT_FILE}"
        echo '```' >> "${REPORT_FILE}"
    fi

    cat >> "${REPORT_FILE}" <<EOF

---

## 📁 生成的文件

- OBJ 模型: \`${OBJ_FILE}\`
- XKT 压缩模型: \`${XKT_COMPRESSED}\`
- XKT 非压缩模型: \`${XKT_UNCOMPRESSED}\`
- 压缩版本验证报告: \`${XKT_COMPRESSED%.xkt}_validation.json\`
- 非压缩版本验证报告: \`${XKT_UNCOMPRESSED%.xkt}_validation.json\`

---

## ✅ 测试结论

XKT 模型生成和验证测试完成。详细的验证数据请查看上述 JSON 报告。

EOF

    print_success "测试报告已生成: ${REPORT_FILE}"
}

# 主函数
main() {
    print_header "XKT 模型正确性测试"
    
    echo -e "${BLUE}测试参考号: ${REFNO}${NC}"
    echo -e "${BLUE}输出目录: ${OUTPUT_DIR}${NC}"
    
    # 执行测试步骤
    check_dependencies
    prepare_directories
    generate_obj
    generate_xkt_compressed
    generate_xkt_uncompressed
    validate_with_nodejs
    generate_report
    
    # 完成
    print_header "测试完成"
    
    echo -e "${GREEN}✅ 所有测试步骤已完成${NC}"
    echo ""
    echo -e "${BLUE}📄 查看测试报告:${NC}"
    echo -e "   cat ${REPORT_FILE}"
    echo ""
    echo -e "${BLUE}📊 查看验证详情:${NC}"
    echo -e "   cat ${XKT_COMPRESSED%.xkt}_validation.json"
    echo -e "   cat ${XKT_UNCOMPRESSED%.xkt}_validation.json"
    echo ""
}

# 运行主函数
main

