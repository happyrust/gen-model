/*
 * DB_Attribute IDA Pro 结构体定义脚本
 * 基于core.dll逆向分析结果
 * 使用方法：在IDA Pro中执行此脚本来定义DB_Attribute结构体
 */

#include <idc.idc>

static main() {
    auto id, mid;
    
    // 创建DB_AttributeType枚举
    id = add_enum(-1, "DB_AttributeType", 0);
    add_enum_member(id, "GENERAL_ATTRIBUTE", 1);
    add_enum_member(id, "REAL_ATTRIBUTE", 2);
    add_enum_member(id, "BOOLEAN_ATTRIBUTE", 3);
    add_enum_member(id, "STRING_ATTRIBUTE", 4);
    add_enum_member(id, "REFERENCE_ATTRIBUTE", 5);
    add_enum_member(id, "UNKNOWN_ATTRIBUTE_6", 6);
    add_enum_member(id, "DIRECTION_ATTRIBUTE", 7);     // 方向属性-几何重建
    add_enum_member(id, "POSITION_ATTRIBUTE", 8);      // 位置属性-几何重建
    add_enum_member(id, "ORIENTATION_ATTRIBUTE", 9);   // 方向矩阵-变换更新
    
    // 创建DB_AttributeFlags枚举
    id = add_enum(-1, "DB_AttributeFlags", 0);
    add_enum_member(id, "ATT_NONE", 0x00000000);
    add_enum_member(id, "ATT_DIRTY", 0x00000001);
    add_enum_member(id, "ATT_INDEXED", 0x00000002);
    add_enum_member(id, "ATT_CACHED", 0x00000004);
    add_enum_member(id, "ATT_EXPRESSION", 0x00000008);
    add_enum_member(id, "ATT_MANDATORY", 0x00000010);
    add_enum_member(id, "ATT_READONLY", 0x00000020);
    add_enum_member(id, "ATT_INHERITED", 0x00000040);
    add_enum_member(id, "ATT_GEOMETRIC", 0x00000080);
    add_enum_member(id, "ATT_TRANSFORM", 0x00000100);
    add_enum_member(id, "ATT_DEPENDENT", 0x00000200);
    add_enum_member(id, "ATT_QUALIFIER", 0x00000400);

    // 创建DB_Attribute主结构体
    id = add_struc(-1, "DB_Attribute", 0);
    
    // +0x00: 虚函数表指针
    add_struc_member(id, "vtable", 0x00, FF_DWRD | FF_0OFF, -1, 4);
    set_member_cmt(id, 0x00, "虚函数表指针 - C++对象虚函数表", 0);
    
    // +0x04: 属性名称 (std::string对象)
    add_struc_member(id, "attribute_name", 0x04, FF_STRU, get_struc_id("std_string"), 0x1C);
    set_member_cmt(id, 0x04, "属性名称 - 如ATT_APOS, ATT_ADIR等", 0);
    
    // +0x20: 属性唯一ID
    add_struc_member(id, "attribute_id", 0x20, FF_DWRD, -1, 4);
    set_member_cmt(id, 0x20, "属性唯一标识符 - 用于快速查找和哈希", 0);
    
    // +0x24: 属性类型 (枚举值1-9)
    add_struc_member(id, "attribute_type", 0x24, FF_DWRD, get_enum("DB_AttributeType"), 4);
    set_member_cmt(id, 0x24, "属性类型 - 决定重新生成策略(1=普通,7=方向,8=位置,9=矩阵)", 0);
    
    // +0x28: 所属Noun类型指针
    add_struc_member(id, "owner_noun", 0x28, FF_DWRD | FF_0OFF, -1, 4);
    set_member_cmt(id, 0x28, "所属Noun类型指针 - 如NOUN_ADIR等", 0);
    
    // +0x2C: 在Noun中的偏移量
    add_struc_member(id, "noun_offset", 0x2C, FF_DWRD, -1, 4);
    set_member_cmt(id, 0x2C, "属性在Noun结构中的字节偏移", 0);
    
    // +0x30: 属性标志位
    add_struc_member(id, "flags", 0x30, FF_DWRD, get_enum("DB_AttributeFlags"), 4);
    set_member_cmt(id, 0x30, "属性状态标志位 - 脏标记,索引,缓存等", 0);
    
    // +0x34: 引用计数
    add_struc_member(id, "reference_count", 0x34, FF_DWRD, -1, 4);
    set_member_cmt(id, 0x34, "引用计数 - 内存管理", 0);
    
    // +0x38: 脏标志
    add_struc_member(id, "is_dirty", 0x38, FF_BYTE, -1, 1);
    set_member_cmt(id, 0x38, "脏标志 - 标记属性值是否需要更新", 0);
    
    // +0x39: 修改标志  
    add_struc_member(id, "is_modified", 0x39, FF_BYTE, -1, 1);
    set_member_cmt(id, 0x39, "修改标志 - 标记在当前会话中是否被修改", 0);
    
    // +0x3A: 对齐填充
    add_struc_member(id, "padding1", 0x3A, FF_BYTE, -1, 2);
    set_member_cmt(id, 0x3A, "对齐填充字节", 0);
    
    // +0x3C: 数据大小
    add_struc_member(id, "data_size", 0x3C, FF_DWRD, -1, 4);
    set_member_cmt(id, 0x3C, "属性数据的字节大小", 0);
    
    // +0x40: 数据对齐要求
    add_struc_member(id, "data_alignment", 0x40, FF_DWRD, -1, 4);
    set_member_cmt(id, 0x40, "数据对齐要求 - 内存对齐", 0);
    
    // +0x44: 类型信息指针 (RTTI)
    add_struc_member(id, "type_info", 0x44, FF_DWRD | FF_0OFF, -1, 4);
    set_member_cmt(id, 0x44, "运行时类型信息指针 - C++ RTTI", 0);
    
    // +0x48: 默认值指针
    add_struc_member(id, "default_value", 0x48, FF_DWRD | FF_0OFF, -1, 4);
    set_member_cmt(id, 0x48, "默认值数据指针", 0);
    
    // +0x4C: 最小值指针
    add_struc_member(id, "min_value", 0x4C, FF_DWRD | FF_0OFF, -1, 4);
    set_member_cmt(id, 0x4C, "最小值指针 - 数值类型约束", 0);
    
    // +0x50: 最大值指针
    add_struc_member(id, "max_value", 0x50, FF_DWRD | FF_0OFF, -1, 4);
    set_member_cmt(id, 0x50, "最大值指针 - 数值类型约束", 0);
    
    // +0x54: 验证函数指针
    add_struc_member(id, "validator_function", 0x54, FF_DWRD | FF_0OFF, -1, 4);
    set_member_cmt(id, 0x54, "属性值验证函数指针", 0);
    
    // +0x58: 有效值列表 (std::vector)
    add_struc_member(id, "valid_values", 0x58, FF_STRU, get_struc_id("std_vector"), 0x0C);
    set_member_cmt(id, 0x58, "有效值列表 - 枚举类型约束", 0);
    
    // +0x64: 验证规则字符串
    add_struc_member(id, "validation_rule", 0x64, FF_STRU, get_struc_id("std_string"), 0x1C);
    set_member_cmt(id, 0x64, "验证规则表达式字符串", 0);
    
    // +0x80: 依赖属性列表
    add_struc_member(id, "depends_on", 0x80, FF_STRU, get_struc_id("std_vector"), 0x0C);
    set_member_cmt(id, 0x80, "依赖的属性列表 - 属性间依赖关系", 0);
    
    // +0x8C: 被依赖属性列表
    add_struc_member(id, "dependent_attrs", 0x8C, FF_STRU, get_struc_id("std_vector"), 0x0C);
    set_member_cmt(id, 0x8C, "依赖此属性的属性列表", 0);
    
    // +0x98: 哈希值
    add_struc_member(id, "hash_code", 0x98, FF_DWRD, -1, 4);
    set_member_cmt(id, 0x98, "属性名称哈希值 - 快速查找优化", 0);
    
    // +0x9C: 索引数据指针
    add_struc_member(id, "index_data", 0x9C, FF_DWRD | FF_0OFF, -1, 4);
    set_member_cmt(id, 0x9C, "索引数据结构指针", 0);
    
    // +0xA0: 表达式文本
    add_struc_member(id, "expression_text", 0xA0, FF_STRU, get_struc_id("std_string"), 0x1C);
    set_member_cmt(id, 0xA0, "表达式文本 - 计算属性支持", 0);
    
    // +0xBC: 编译后表达式指针
    add_struc_member(id, "compiled_expression", 0xBC, FF_DWRD | FF_0OFF, -1, 4);
    set_member_cmt(id, 0xBC, "编译后的表达式对象指针", 0);
    
    // +0xC0: 显示名称 (本地化)
    add_struc_member(id, "display_name", 0xC0, FF_STRU, get_struc_id("std_string"), 0x1C);
    set_member_cmt(id, 0xC0, "本地化显示名称", 0);
    
    // +0xDC: 属性描述
    add_struc_member(id, "description", 0xDC, FF_STRU, get_struc_id("std_string"), 0x1C);
    set_member_cmt(id, 0xDC, "属性详细描述文本", 0);
    
    // +0xF8: 单位字符串
    add_struc_member(id, "unit", 0xF8, FF_STRU, get_struc_id("std_string"), 0x1C);
    set_member_cmt(id, 0xF8, "属性单位 - 数值类型的单位", 0);
    
    // +0x114: 版本号
    add_struc_member(id, "version", 0x114, FF_DWRD, -1, 4);
    set_member_cmt(id, 0x114, "属性定义版本号", 0);
    
    // +0x118: 最后修改会话ID
    add_struc_member(id, "last_modified_session", 0x118, FF_DWRD, -1, 4);
    set_member_cmt(id, 0x118, "最后修改此属性的会话标识", 0);
    
    // +0x11C: 缓存值指针
    add_struc_member(id, "cached_value", 0x11C, FF_DWRD | FF_0OFF, -1, 4);
    set_member_cmt(id, 0x11C, "缓存的属性值指针 - 性能优化", 0);
    
    // +0x120: 缓存有效标志
    add_struc_member(id, "cache_valid", 0x120, FF_BYTE, -1, 1);
    set_member_cmt(id, 0x120, "缓存有效性标志", 0);
    
    // +0x121: 对齐填充
    add_struc_member(id, "padding2", 0x121, FF_BYTE, -1, 3);
    set_member_cmt(id, 0x121, "对齐填充字节", 0);
    
    // +0x124: 访问计数
    add_struc_member(id, "access_count", 0x124, FF_DWRD, -1, 4);
    set_member_cmt(id, 0x124, "访问次数统计 - 性能分析", 0);
    
    // +0x128: 调试信息字符串
    add_struc_member(id, "debug_info", 0x128, FF_STRU, get_struc_id("std_string"), 0x1C);
    set_member_cmt(id, 0x128, "调试信息和诊断数据", 0);
    
    Message("DB_Attribute结构体定义完成！总大小: 0x%X字节\n", get_struc_size(id));
    
    return id;
} 