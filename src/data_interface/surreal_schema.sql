-- SurrealDB Schema for PDMS Data Interface
-- This file defines the table structures needed for gen_model

-- PDMS Elements table (核心元素表)
DEFINE TABLE pdms_elements SCHEMAFULL;
DEFINE FIELD refno ON TABLE pdms_elements TYPE number;
DEFINE FIELD type_name ON TABLE pdms_elements TYPE string;
DEFINE FIELD name ON TABLE pdms_elements TYPE string;
DEFINE FIELD owner ON TABLE pdms_elements TYPE number;
DEFINE FIELD dbno ON TABLE pdms_elements TYPE number;
DEFINE FIELD version ON TABLE pdms_elements TYPE number DEFAULT 0;
DEFINE FIELD attributes ON TABLE pdms_elements TYPE object DEFAULT {};
DEFINE FIELD created_at ON TABLE pdms_elements TYPE datetime DEFAULT time::now();
DEFINE FIELD updated_at ON TABLE pdms_elements TYPE datetime DEFAULT time::now();
DEFINE INDEX idx_refno ON TABLE pdms_elements COLUMNS refno UNIQUE;
DEFINE INDEX idx_owner ON TABLE pdms_elements COLUMNS owner;
DEFINE INDEX idx_type ON TABLE pdms_elements COLUMNS type_name;
DEFINE INDEX idx_dbno ON TABLE pdms_elements COLUMNS dbno;

-- Element hierarchy edges (元素层级关系)
DEFINE TABLE owns SCHEMAFULL;
DEFINE FIELD in ON TABLE owns TYPE record<pdms_elements>;
DEFINE FIELD out ON TABLE owns TYPE record<pdms_elements>;
DEFINE FIELD order_index ON TABLE owns TYPE number DEFAULT 0;
DEFINE INDEX idx_owns ON TABLE owns COLUMNS in, out UNIQUE;

-- Catalog references (元件库引用)
DEFINE TABLE catalog_refs SCHEMAFULL;
DEFINE FIELD refno ON TABLE catalog_refs TYPE number;
DEFINE FIELD cate_name ON TABLE catalog_refs TYPE string;
DEFINE FIELD cate_hash ON TABLE catalog_refs TYPE string;
DEFINE FIELD spre_ref ON TABLE catalog_refs TYPE number;
DEFINE FIELD attributes ON TABLE catalog_refs TYPE object DEFAULT {};
DEFINE INDEX idx_catalog_refno ON TABLE catalog_refs COLUMNS refno;
DEFINE INDEX idx_catalog_name ON TABLE catalog_refs COLUMNS cate_name;
DEFINE INDEX idx_catalog_hash ON TABLE catalog_refs COLUMNS cate_hash;

-- Geometry instances (几何实例)
DEFINE TABLE shape_instances SCHEMAFULL;
DEFINE FIELD refno ON TABLE shape_instances TYPE number;
DEFINE FIELD shape_type ON TABLE shape_instances TYPE string;
DEFINE FIELD transform ON TABLE shape_instances TYPE object;
DEFINE FIELD mesh_data ON TABLE shape_instances TYPE object;
DEFINE FIELD material ON TABLE shape_instances TYPE object;
DEFINE FIELD is_negative ON TABLE shape_instances TYPE bool DEFAULT false;
DEFINE FIELD parent_refno ON TABLE shape_instances TYPE number;
DEFINE FIELD created_at ON TABLE shape_instances TYPE datetime DEFAULT time::now();
DEFINE INDEX idx_shape_refno ON TABLE shape_instances COLUMNS refno;
DEFINE INDEX idx_shape_parent ON TABLE shape_instances COLUMNS parent_refno;

-- Named attributes (命名属性映射)
DEFINE TABLE named_attributes SCHEMAFULL;
DEFINE FIELD refno ON TABLE named_attributes TYPE number;
DEFINE FIELD attr_name ON TABLE named_attributes TYPE string;
DEFINE FIELD attr_value ON TABLE named_attributes TYPE any;
DEFINE FIELD attr_type ON TABLE named_attributes TYPE string;
DEFINE INDEX idx_named_attr ON TABLE named_attributes COLUMNS refno, attr_name UNIQUE;

-- Implicit attributes (隐含属性)
DEFINE TABLE implicit_attributes SCHEMAFULL;
DEFINE FIELD refno ON TABLE implicit_attributes TYPE number;
DEFINE FIELD attr_name ON TABLE implicit_attributes TYPE string;
DEFINE FIELD attr_value ON TABLE implicit_attributes TYPE any;
DEFINE FIELD source_type ON TABLE implicit_attributes TYPE string; -- 'parent', 'catalog', 'computed'
DEFINE INDEX idx_implicit_attr ON TABLE implicit_attributes COLUMNS refno, attr_name;

-- World transforms cache (世界坐标变换缓存)
DEFINE TABLE world_transforms SCHEMAFULL;
DEFINE FIELD refno ON TABLE world_transforms TYPE number;
DEFINE FIELD position ON TABLE world_transforms TYPE array DEFAULT [0, 0, 0];
DEFINE FIELD rotation ON TABLE world_transforms TYPE array DEFAULT [0, 0, 0, 1];
DEFINE FIELD scale ON TABLE world_transforms TYPE array DEFAULT [1, 1, 1];
DEFINE FIELD matrix ON TABLE world_transforms TYPE array;
DEFINE FIELD updated_at ON TABLE world_transforms TYPE datetime DEFAULT time::now();
DEFINE INDEX idx_transform_refno ON TABLE world_transforms COLUMNS refno UNIQUE;

-- MDB worlds (MDB世界节点)
DEFINE TABLE mdb_worlds SCHEMAFULL;
DEFINE FIELD project ON TABLE mdb_worlds TYPE string;
DEFINE FIELD mdb_name ON TABLE mdb_worlds TYPE string;
DEFINE FIELD module ON TABLE mdb_worlds TYPE string;
DEFINE FIELD world_refno ON TABLE mdb_worlds TYPE number;
DEFINE FIELD dbno ON TABLE mdb_worlds TYPE number;
DEFINE INDEX idx_mdb_world ON TABLE mdb_worlds COLUMNS project, mdb_name, module UNIQUE;

-- Increment records (增量记录)
DEFINE TABLE increment_records SCHEMAFULL;
DEFINE FIELD id ON TABLE increment_records TYPE string;
DEFINE FIELD operation ON TABLE increment_records TYPE string; -- 'create', 'update', 'delete'
DEFINE FIELD refno ON TABLE increment_records TYPE number;
DEFINE FIELD old_data ON TABLE increment_records TYPE object;
DEFINE FIELD new_data ON TABLE increment_records TYPE object;
DEFINE FIELD timestamp ON TABLE increment_records TYPE datetime DEFAULT time::now();
DEFINE INDEX idx_incr_timestamp ON TABLE increment_records COLUMNS timestamp;
DEFINE INDEX idx_incr_refno ON TABLE increment_records COLUMNS refno;

-- Instance relationship table (实例关系表)
-- 用于存储组件间的层级关系，支持 BRAN/HANG/EQUI 分组查询
DEFINE TABLE inst_relate SCHEMAFULL;
DEFINE FIELD in ON TABLE inst_relate TYPE record<any>;
DEFINE FIELD out ON TABLE inst_relate TYPE record<any>;
DEFINE FIELD owner_type ON TABLE inst_relate TYPE string;
DEFINE FIELD owner_refno ON TABLE inst_relate TYPE number;
DEFINE FIELD created_at ON TABLE inst_relate TYPE datetime DEFAULT time::now();
DEFINE FIELD updated_at ON TABLE inst_relate TYPE datetime DEFAULT time::now();

-- 优化 BRAN/HANG/EQUI 分组查询的索引
DEFINE INDEX idx_owner_type ON TABLE inst_relate COLUMNS owner_type;
DEFINE INDEX idx_owner_refno ON TABLE inst_relate COLUMNS owner_refno;
DEFINE INDEX idx_owner_type_refno ON TABLE inst_relate COLUMNS owner_type, owner_refno;

-- Functions for common queries

-- Get element with attributes
DEFINE FUNCTION fn::get_element_full($refno: number) {
    LET $element = (SELECT * FROM pdms_elements WHERE refno = $refno)[0];
    LET $attrs = (SELECT attr_name, attr_value FROM named_attributes WHERE refno = $refno);
    RETURN {
        element: $element,
        attributes: $attrs.reduce(|$obj, $item| $obj.merge({[$item.attr_name]: $item.attr_value}), {})
    };
};

-- Get children of an element
DEFINE FUNCTION fn::get_children($owner: number) {
    RETURN SELECT value out.* FROM owns WHERE in.refno = $owner ORDER BY order_index;
};

-- Get ancestors path
DEFINE FUNCTION fn::get_ancestors($refno: number) {
    LET $path = [];
    LET $current = $refno;
    WHILE $current {
        LET $elem = (SELECT * FROM pdms_elements WHERE refno = $current)[0];
        IF $elem {
            LET $path = array::append($path, $elem);
            LET $current = $elem.owner;
        } ELSE {
            LET $current = null;
        };
    };
    RETURN $path;
};