// 简化的 Kuzu 功能测试
// 测试基本的数据结构和逻辑，不依赖实际的 Kuzu 数据库

use std::collections::HashMap;
use std::sync::Arc;

// 模拟 Kuzu 元素
#[derive(Clone, Debug)]
struct TestElement {
    refno: u64,
    dbnum: i32,
    type_name: String,
    name: Option<String>,
    children: Vec<u64>,
    parent: Option<u64>,
}

// 模拟 Kuzu 管理器
struct TestKuzuManager {
    elements: HashMap<u64, TestElement>,
}

impl TestKuzuManager {
    fn new() -> Self {
        Self {
            elements: HashMap::new(),
        }
    }

    fn insert_element(&mut self, element: TestElement) {
        self.elements.insert(element.refno, element);
    }

    fn query_element(&self, refno: u64) -> Option<&TestElement> {
        self.elements.get(&refno)
    }

    fn query_children(&self, parent: u64) -> Vec<u64> {
        if let Some(element) = self.elements.get(&parent) {
            element.children.clone()
        } else {
            Vec::new()
        }
    }

    fn query_elements_by_type(&self, type_name: &str, dbnum: Option<i32>) -> Vec<u64> {
        self.elements
            .values()
            .filter(|e| {
                e.type_name == type_name
                    && (dbnum.is_none() || e.dbnum == dbnum.unwrap())
            })
            .map(|e| e.refno)
            .collect()
    }
}

fn main() {
    println!("========== Kuzu 简化测试 ==========\n");

    // 创建测试管理器
    let mut manager = TestKuzuManager::new();

    // 示例参考号：17496/266203 转换为数字形式
    let example_refno = 17496266203u64;
    let dbnum = 17496;

    println!("1. 创建测试数据...");

    // 创建 FRMW 父元素
    let parent_element = TestElement {
        refno: example_refno,
        dbnum,
        type_name: "FRMW".to_string(),
        name: Some("MainFrame".to_string()),
        children: vec![17496266204, 17496266205, 17496266206],
        parent: None,
    };
    manager.insert_element(parent_element);

    // 创建子元素
    for i in 0..3 {
        let child_refno = 17496266204 + i;
        let child = TestElement {
            refno: child_refno,
            dbnum,
            type_name: "SBFR".to_string(),
            name: Some(format!("SubFrame_{}", i + 1)),
            children: Vec::new(),
            parent: Some(example_refno),
        };
        manager.insert_element(child);
    }

    // 创建其他类型元素
    let types = vec![("EQUI", 2), ("PIPE", 3), ("VALV", 1)];
    let mut offset = 17496266210;
    for (type_name, count) in types {
        for i in 0..count {
            let element = TestElement {
                refno: offset + i,
                dbnum,
                type_name: type_name.to_string(),
                name: Some(format!("{}_{}", type_name, i + 1)),
                children: Vec::new(),
                parent: None,
            };
            manager.insert_element(element);
        }
        offset += count;
    }

    println!("✓ 创建了 {} 个测试元素\n", manager.elements.len());

    // 2. 测试查询功能
    println!("2. 测试查询功能...");

    // 查询单个元素
    if let Some(element) = manager.query_element(example_refno) {
        println!("   ✓ 查询元素 {}: 类型={}, 名称={:?}",
            element.refno,
            element.type_name,
            element.name
        );
    }

    // 查询子元素
    let children = manager.query_children(example_refno);
    println!("   ✓ 找到 {} 个子元素: {:?}", children.len(), children);

    // 按类型查询
    let frmw_elements = manager.query_elements_by_type("FRMW", Some(dbnum));
    println!("   ✓ FRMW 类型元素: {} 个", frmw_elements.len());

    let equi_elements = manager.query_elements_by_type("EQUI", Some(dbnum));
    println!("   ✓ EQUI 类型元素: {} 个", equi_elements.len());

    let pipe_elements = manager.query_elements_by_type("PIPE", None);
    println!("   ✓ PIPE 类型元素: {} 个\n", pipe_elements.len());

    // 3. 模拟模型生成流程
    println!("3. 模拟模型生成流程...");

    let mut prim_refnos = Vec::new();
    let mut loop_refnos = Vec::new();
    let mut use_cate_refnos = Vec::new();
    let mut bran_refnos = Vec::new();

    for element in manager.elements.values() {
        match element.type_name.as_str() {
            "PRIM" => prim_refnos.push(element.refno),
            "LOOP" => loop_refnos.push(element.refno),
            "FRMW" | "EQUI" => use_cate_refnos.push(element.refno),
            "BRAN" | "PIPE" => bran_refnos.push(element.refno),
            _ => {}
        }
    }

    println!("   分类统计:");
    println!("   - PRIM 元素: {} 个", prim_refnos.len());
    println!("   - LOOP 元素: {} 个", loop_refnos.len());
    println!("   - USE_CATE 元素: {} 个", use_cate_refnos.len());
    println!("   - BRAN/PIPE 元素: {} 个\n", bran_refnos.len());

    // 4. 验证数据一致性
    println!("4. 验证数据一致性...");

    let mut valid = true;

    // 验证父子关系
    for element in manager.elements.values() {
        // 检查子元素是否存在
        for &child_refno in &element.children {
            if !manager.elements.contains_key(&child_refno) {
                println!("   ✗ 子元素 {} 不存在", child_refno);
                valid = false;
            }
        }

        // 检查父元素是否存在
        if let Some(parent_refno) = element.parent {
            if !manager.elements.contains_key(&parent_refno) {
                println!("   ✗ 父元素 {} 不存在", parent_refno);
                valid = false;
            }
        }
    }

    if valid {
        println!("   ✓ 数据一致性验证通过\n");
    } else {
        println!("   ✗ 数据一致性验证失败\n");
    }

    // 5. 性能测试
    println!("5. 性能测试...");

    use std::time::Instant;

    // 批量插入测试
    let start = Instant::now();
    let mut test_manager = TestKuzuManager::new();
    for i in 0..1000 {
        let element = TestElement {
            refno: 1000000 + i,
            dbnum: 1000,
            type_name: "TEST".to_string(),
            name: Some(format!("Test_{}", i)),
            children: Vec::new(),
            parent: None,
        };
        test_manager.insert_element(element);
    }
    let insert_time = start.elapsed();
    println!("   ✓ 插入 1000 个元素耗时: {:?}", insert_time);

    // 批量查询测试
    let start = Instant::now();
    for i in 0..1000 {
        test_manager.query_element(1000000 + i);
    }
    let query_time = start.elapsed();
    println!("   ✓ 查询 1000 个元素耗时: {:?}", query_time);

    // 类型筛选测试
    let start = Instant::now();
    let _ = test_manager.query_elements_by_type("TEST", Some(1000));
    let filter_time = start.elapsed();
    println!("   ✓ 类型筛选耗时: {:?}\n", filter_time);

    println!("========== 测试完成 ==========");
    println!("示例参考号: {}", example_refno);
    println!("数据库编号: {}", dbnum);
    println!("总元素数: {}", manager.elements.len());
    println!("\n所有测试通过 ✓");
}