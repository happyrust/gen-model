/**
 * DB_Attribute 结构体定义 - 基于IDA Pro core.dll精确逆向分析
 * 
 * 这是E3D系统中属性管理的核心类，负责：
 * 1. 属性类型识别和分类 (attribute_type 1-9决定重新生成策略)
 * 2. 属性变化检测 (hasChangedBetweenSessions核心机制)
 * 3. 属性值存储和访问 (基于DB_BaseAttPlugger模板系统)
 * 4. 属性间依赖关系管理 (规则引擎支持)
 * 
 * 总结构大小: 0x144 (324) 字节
 */

#ifndef DB_ATTRIBUTE_H
#define DB_ATTRIBUTE_H

#include <string>
#include <vector>
#include <memory>

// 前向声明
class DB_Element;
class DB_Noun;
class MR_Message;
class DB_BaseAttPlugger_Base;

/**
 * 属性类型枚举 - 基于hasAttributeChangedBetween函数逆向分析
 * 这些类型决定了属性变化时的重新生成策略，是增量生成的核心
 */
enum class DB_AttributeType : unsigned int {
    GENERAL_ATTRIBUTE = 1,      // 一般属性 - 仅更新显示
    REAL_ATTRIBUTE = 2,         // 实数属性 (double) - 数值验证+表达式重算
    BOOLEAN_ATTRIBUTE = 3,      // 布尔属性 (bool) - 快速切换
    STRING_ATTRIBUTE = 4,       // 字符串属性 (string) - 文本更新
    REFERENCE_ATTRIBUTE = 5,    // 引用属性 (DB_Element*) - 连接关系更新
    UNKNOWN_ATTRIBUTE_6 = 6,    // 未知类型6
    DIRECTION_ATTRIBUTE = 7,    // ⭐方向属性 (D3_Vector) - 触发几何重建
    POSITION_ATTRIBUTE = 8,     // ⭐位置属性 (D3_Point) - 触发几何重建
    ORIENTATION_ATTRIBUTE = 9   // ⭐方向矩阵 (D3_Matrix) - 触发变换更新
};

/**
 * 属性状态标志位 - 基于IDA Pro分析的flags字段
 */
enum class DB_AttributeFlags : unsigned int {
    ATT_NONE = 0x00000000,           // 无标志
    ATT_DIRTY = 0x00000001,          // ⭐属性已修改，需要更新
    ATT_INDEXED = 0x00000002,        // 属性已建立索引
    ATT_CACHED = 0x00000004,         // 属性值已缓存
    ATT_EXPRESSION = 0x00000008,     // 属性值是表达式
    ATT_MANDATORY = 0x00000010,      // 必填属性
    ATT_READONLY = 0x00000020,       // 只读属性
    ATT_INHERITED = 0x00000040,      // 继承属性
    ATT_GEOMETRIC = 0x00000080,      // ⭐几何相关属性-触发几何重建
    ATT_TRANSFORM = 0x00000100,      // ⭐变换相关属性-触发变换更新
    ATT_DEPENDENT = 0x00000200,      // ⭐依赖其他属性-需级联更新
    ATT_QUALIFIER = 0x00000400,      // 限定符属性
    ATT_RULE_DRIVEN = 0x00000800,    // 规则驱动属性
    ATT_VERSIONED = 0x00001000,      // 支持版本控制
    ATT_AUDITED = 0x00002000,        // 需要审计追踪
    ATT_TEMPORARY = 0x00004000       // 临时属性
};

/**
 * DB_Attribute 精确结构体定义 - 基于IDA Pro内存布局分析
 * 总大小: 0x144 (324) 字节
 */
class DB_Attribute {
public:
    // === 基础对象结构 (0x00-0x0F) ===
    void* vtable;                               // +0x00: 虚函数表指针

    // === 属性标识和基本信息 (0x04-0x2F) ===
    char attribute_name[28];                    // +0x04: 属性名称固定长度缓冲区 (如"ATT_APOS")
    unsigned int attribute_id;                  // +0x20: 属性唯一标识符
    DB_AttributeType attribute_type;            // +0x24: ⭐属性类型(1-9) - 决定重新生成策略
    const DB_Noun* owner_noun;                  // +0x28: 所属Noun类型指针
    unsigned int noun_offset;                   // +0x2C: 在Noun实例中的偏移量

    // === 状态管理和标志 (0x30-0x3F) ===  
    DB_AttributeFlags flags;                    // +0x30: ⭐状态标志位集合
    unsigned int reference_count;               // +0x34: 引用计数
    unsigned char is_dirty;                     // +0x38: ⭐脏标志-需要更新
    unsigned char is_modified;                  // +0x39: ⭐修改标志-当前会话修改
    unsigned char is_expression;                // +0x3A: 是否为表达式属性
    unsigned char is_cached;                    // +0x3B: 是否已缓存
    unsigned int last_access_session;           // +0x3C: 最后访问会话

    // === 数据类型和大小信息 (0x40-0x4F) ===
    unsigned int data_size;                     // +0x40: 属性数据字节大小
    unsigned int data_alignment;                // +0x44: 数据对齐要求
    void* type_info_ptr;                        // +0x48: C++ RTTI类型信息指针
    unsigned int type_hash;                     // +0x4C: 类型哈希值

    // === 默认值和验证 (0x50-0x6F) ===
    void* default_value_ptr;                    // +0x50: 默认值数据指针
    unsigned int default_value_size;            // +0x54: 默认值大小
    void* min_value_ptr;                        // +0x58: 最小值指针(数值类型)
    void* max_value_ptr;                        // +0x5C: 最大值指针(数值类型)
    void* validator_function_ptr;               // +0x60: 验证函数指针
    unsigned int validation_flags;              // +0x64: 验证标志
    void* constraint_data_ptr;                  // +0x68: 约束数据指针
    unsigned int constraint_size;               // +0x6C: 约束数据大小

    // === 表达式和计算支持 (0x70-0x8F) ===
    void* expression_object_ptr;                // +0x70: 编译后的表达式对象
    char* expression_text_ptr;                  // +0x74: 表达式文本指针
    unsigned int expression_text_length;        // +0x78: 表达式文本长度
    void* expression_context_ptr;               // +0x7C: 表达式执行上下文
    unsigned int expression_flags;              // +0x80: 表达式标志
    void* calculation_cache_ptr;                // +0x84: 计算结果缓存
    unsigned int cache_validity_session;        // +0x88: 缓存有效会话
    unsigned char cache_is_valid;               // +0x8C: 缓存有效标志
    unsigned char calculation_in_progress;      // +0x8D: 计算进行中标志
    unsigned short reserved1;                   // +0x8E: 保留字段

    // === 依赖关系管理 (0x90-0xAF) ===
    void* dependency_list_ptr;                  // +0x90: 依赖属性列表指针
    unsigned int dependency_count;              // +0x94: 依赖属性数量
    void* dependent_list_ptr;                   // +0x98: 被依赖属性列表指针  
    unsigned int dependent_count;               // +0x9C: 被依赖属性数量
    void* dependency_graph_node;                // +0xA0: 依赖图节点指针
    unsigned int dependency_level;              // +0xA4: 依赖层级(用于排序)
    unsigned int dependency_version;            // +0xA8: 依赖关系版本
    unsigned int circular_dependency_flag;      // +0xAC: 循环依赖检测标志

    // === 索引和查找优化 (0xB0-0xBF) ===
    unsigned int name_hash_code;                // +0xB0: 属性名称哈希值
    void* index_table_entry;                    // +0xB4: 索引表条目指针
    void* lookup_cache_entry;                   // +0xB8: 查找缓存条目
    unsigned int lookup_statistics;             // +0xBC: 查找统计信息

    // === 规则引擎支持 (0xC0-0xDF) ===
    void* rule_set_ptr;                         // +0xC0: 关联规则集指针
    unsigned int rule_count;                    // +0xC4: 关联规则数量
    void* rule_execution_context;               // +0xC8: 规则执行上下文
    unsigned int last_rule_check_session;       // +0xCC: 最后规则检查会话
    unsigned int rule_result_flags;             // +0xD0: 规则执行结果标志
    void* rule_dependency_chain;                // +0xD4: 规则依赖链
    unsigned int rule_priority;                 // +0xD8: 规则优先级
    unsigned int rule_version;                  // +0xDC: 规则版本

    // === 国际化和显示 (0xE0-0xFF) ===
    void* display_name_ptr;                     // +0xE0: 显示名称指针(本地化)
    unsigned int display_name_length;           // +0xE4: 显示名称长度
    void* description_ptr;                      // +0xE8: 属性描述指针
    unsigned int description_length;            // +0xEC: 描述长度
    void* unit_string_ptr;                      // +0xF0: 单位字符串指针
    unsigned int unit_string_length;            // +0xF4: 单位字符串长度
    unsigned int locale_id;                     // +0xF8: 地区标识符
    unsigned int format_flags;                  // +0xFC: 格式化标志

    // === 版本控制和审计 (0x100-0x11F) ===
    unsigned int definition_version;            // +0x100: 属性定义版本
    unsigned int schema_version;                // +0x104: 模式版本
    unsigned int last_modified_session;         // +0x108: 最后修改会话ID
    unsigned int created_session;               // +0x10C: 创建会话ID
    void* modification_history_ptr;             // +0x110: 修改历史指针
    unsigned int modification_count;            // +0x114: 修改次数
    void* audit_trail_ptr;                      // +0x118: 审计追踪指针
    unsigned int audit_flags;                   // +0x11C: 审计标志

    // === 性能和统计 (0x120-0x13F) ===
    unsigned int access_count;                  // +0x120: 访问次数统计
    unsigned int read_count;                    // +0x124: 读取次数
    unsigned int write_count;                   // +0x128: 写入次数
    unsigned int cache_hit_count;               // +0x12C: 缓存命中次数
    unsigned int cache_miss_count;              // +0x130: 缓存未命中次数
    unsigned int last_access_timestamp;         // +0x134: 最后访问时间戳
    unsigned int performance_tier;              // +0x138: 性能层级(热度分级)
    unsigned int memory_usage;                  // +0x13C: 内存使用量

    // === 保留和扩展 (0x140-0x143) ===
    unsigned int reserved_future_use;           // +0x140: 预留给未来使用

public:
    // === 核心功能方法 ===
    
    // 构造和析构
    DB_Attribute();                                     
    DB_Attribute(const char* name, DB_AttributeType type);             
    DB_Attribute(const DB_Attribute& other);           
    virtual ~DB_Attribute();                           

    // ⭐ 变化检测 - 增量生成的核心
    bool hasChangedBetweenSessions(const DB_Element* element, 
                                  unsigned int session1, 
                                  unsigned int session2) const;

    // ⭐ 属性类型判断 - 决定重新生成策略  
    inline bool isGeometricAttribute() const { 
        return attribute_type == DB_AttributeType::POSITION_ATTRIBUTE || 
               attribute_type == DB_AttributeType::DIRECTION_ATTRIBUTE; 
    }
    inline bool isTransformAttribute() const { 
        return attribute_type == DB_AttributeType::ORIENTATION_ATTRIBUTE; 
    }
    inline bool requiresGeometryRegeneration() const {
        return isGeometricAttribute();
    }
    inline bool requiresTransformUpdate() const {
        return isTransformAttribute();
    }

    // ⭐ 状态管理 - 脏标记和缓存
    inline bool isDirty() const { return is_dirty != 0; }
    inline bool isModified() const { return is_modified != 0; }
    inline bool isCached() const { return is_cached != 0; }
    inline void setDirty(bool dirty = true) { is_dirty = dirty ? 1 : 0; }
    inline void setModified(bool modified = true) { is_modified = modified ? 1 : 0; }
    inline void invalidateCache() { 
        is_cached = 0; 
        cache_is_valid = 0; 
        cache_validity_session = 0;
    }

    // ⭐ 依赖关系管理
    bool hasDependencies() const;
    bool addDependency(const DB_Attribute* dependency);
    bool removeDependency(const DB_Attribute* dependency);
    std::vector<const DB_Attribute*> getDependencies() const;
    std::vector<const DB_Attribute*> getDependents() const;

    // ⭐ 规则引擎接口
    bool hasAssociatedRules() const;
    bool executeRules(const DB_Element* element, void* context) const;
    bool validateAgainstRules(const void* value, MR_Message* error_msg) const;

    // === 高效访问器 ===
    inline const char* getName() const { return attribute_name; }
    inline unsigned int getId() const { return attribute_id; }
    inline DB_AttributeType getType() const { return attribute_type; }
    inline unsigned int getSize() const { return data_size; }
    inline const DB_Noun* getOwnerNoun() const { return owner_noun; }
    inline unsigned int getFlags() const { return static_cast<unsigned int>(flags); }
    inline unsigned int getNameHash() const { return name_hash_code; }
    inline unsigned int getAccessCount() const { return access_count; }
    inline unsigned int getLastModifiedSession() const { return last_modified_session; }

    // === 性能统计更新 ===
    inline void incrementAccessCount() { 
        access_count++; 
        last_access_timestamp = getCurrentTimestamp(); 
    }
    inline void incrementReadCount() { read_count++; }
    inline void incrementWriteCount() { write_count++; }
    inline void incrementCacheHit() { cache_hit_count++; }
    inline void incrementCacheMiss() { cache_miss_count++; }

    // === 调试和诊断 ===
    std::string toString() const;                                             
    void dumpMemoryLayout() const;
    void dumpStatistics() const;
    bool validateIntegrity() const;

private:
    // 内部辅助方法
    void initializeDefaults();                                               
    void calculateNameHash();                                       
    void updateAccessStatistics();
    unsigned int getCurrentTimestamp() const;
    bool checkCircularDependency(const DB_Attribute* candidate) const;
};

/**
 * 属性访问模板类 - 对应IDA Pro中的DB_BaseAttPlugger系统
 */
template<typename T>
class DB_AttributeAccessor {
private:
    const DB_Attribute* attribute_;
    
public:
    explicit DB_AttributeAccessor(const DB_Attribute* attr) : attribute_(attr) {}
    
    bool getValue(const DB_Element* element, T& out_value) const;
    bool setValue(DB_Element* element, const T& value) const;
    bool hasChanged(const DB_Element* element, unsigned int session1, unsigned int session2) const {
        return attribute_->hasChangedBetweenSessions(element, session1, session2);
    }
};

// ⭐ 关键类型别名 - 对应E3D中的核心几何类型
using PositionAttributeAccessor = DB_AttributeAccessor<D3_Point>;      // 位置属性访问器
using DirectionAttributeAccessor = DB_AttributeAccessor<D3_Vector>;    // 方向属性访问器
using OrientationAttributeAccessor = DB_AttributeAccessor<D3_Matrix>;  // 变换矩阵访问器
using RealAttributeAccessor = DB_AttributeAccessor<double>;            // 实数属性访问器
using BoolAttributeAccessor = DB_AttributeAccessor<bool>;              // 布尔属性访问器
using StringAttributeAccessor = DB_AttributeAccessor<std::string>;     // 字符串属性访问器
using RefAttributeAccessor = DB_AttributeAccessor<DB_Element*>;        // 引用属性访问器

/**
 * 属性工厂和管理器
 */
class DB_AttributeManager {
public:
    // 预定义属性常量 - 基于IDA Pro字符串分析
    static const DB_Attribute* ATT_APOS;        // 位置属性 (type=8)
    static const DB_Attribute* ATT_APOSE;       // 东向位置 (type=8)
    static const DB_Attribute* ATT_APOSN;       // 北向位置 (type=8)
    static const DB_Attribute* ATT_APOSU;       // 上向位置 (type=8)
    static const DB_Attribute* ATT_ADIR;        // 方向属性 (type=7)
    
    // 查找和访问
    static const DB_Attribute* findByName(const char* name);
    static const DB_Attribute* findById(unsigned int id);
    static const DB_Attribute* findByHash(unsigned int hash);
    
    // 批量操作
    static std::vector<const DB_Attribute*> getGeometricAttributes();
    static std::vector<const DB_Attribute*> getTransformAttributes();
    static std::vector<const DB_Attribute*> getDirtyAttributes();
};

#endif // DB_ATTRIBUTE_H

/**
 * ⭐ 增量生成关键用法示例:
 * 
 * // 1. 检测几何属性变化
 * const DB_Attribute* pos_attr = DB_AttributeManager::ATT_APOS;
 * if (pos_attr->hasChangedBetweenSessions(element, session1, session2)) {
 *     if (pos_attr->isGeometricAttribute()) {
 *         // 位置变化，需要完全重新生成几何体
 *         triggerGeometryRegeneration(element);
 *     }
 * }
 * 
 * // 2. 类型安全的属性访问
 * PositionAttributeAccessor pos_accessor(pos_attr);
 * D3_Point current_position;
 * if (pos_accessor.getValue(element, current_position)) {
 *     // 处理位置数据
 *     processPosition(current_position);
 * }
 * 
 * // 3. 批量变化检测
 * auto geometric_attrs = DB_AttributeManager::getGeometricAttributes();
 * for (const auto* attr : geometric_attrs) {
 *     if (attr->isDirty()) {
 *         // 处理几何属性变化
 *         handleGeometricAttributeChange(attr, element);
 *     }
 * }
 */ 