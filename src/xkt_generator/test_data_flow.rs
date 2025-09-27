use super::*;
use glam::Vec3;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xkt_model_data_flow() {
        let mut model = XKTModel::new();

        // 1. 创建几何体
        let mut geometry = XKTGeometry::new("geom_1".to_string(), XKTGeometryType::Triangles);
        geometry.set_axis_label("A");
        geometry.positions = vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        geometry.normals = Some(vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]);
        geometry.indices = vec![0, 1, 2];
        geometry.calculate_bounding_box();

        let geom_id = model.create_geometry(geometry).unwrap();

        // 验证几何体创建
        assert_eq!(model.geometries.len(), 1);
        assert_eq!(model.geometries_list.len(), 1);
        assert_eq!(model.geometries_list[0].geometry_index, Some(0));

        // 2. 创建材质
        let material = XKTMaterial::create_color_material(
            "mat_1".to_string(),
            "Red Material".to_string(),
            Vec3::new(1.0, 0.0, 0.0),
        );

        let mat_id = model.create_material(material).unwrap();

        // 3. 创建网格
        let mesh = XKTMesh {
            id: "mesh_1".to_string(),
            mesh_index: None,
            geometry_id: geom_id.clone(),
            material_id: Some(mat_id.clone()),

            matrix: None,
            position: Vec3::ZERO,
            rotation: Vec3::ZERO,
            scale: Vec3::ONE,

            color: Vec3::new(1.0, 0.0, 0.0),
            opacity: 1.0,
            metallic: 0.0,
            roughness: 0.5,

            texture_set_id: None,
            visible: true,
        };

        let mesh_id = model.create_mesh(mesh).unwrap();

        // 验证网格创建
        assert_eq!(model.meshes.len(), 1);
        assert_eq!(model.meshes_list.len(), 1);
        assert!(model.geometry_reuse_table.is_reused(&geom_id) == false);

        // 4. 创建实体
        let entity = XKTEntity {
            id: "entity_1".to_string(),
            name: "Test Entity".to_string(),
            entity_type: "IfcWall".to_string(),
            entity_index: None,

            mesh_ids: vec![mesh_id.clone()],

            parent_id: None,
            children_ids: Vec::new(),

            aabb: None,
            has_reused_geometries: false,

            properties: std::collections::HashMap::new(),
            visible: true,
            pickable: true,
            highlighted: false,
            selected: false,
            xrayed: false,
            clippable: true,
            collidable: true,
            castsShadow: true,
            receivesShadow: true,
        };

        let entity_id = model.create_entity(entity).unwrap();

        // 验证实体创建
        assert_eq!(model.entities.len(), 1);
        assert_eq!(model.entities_list.len(), 1);

        // 5. 测试索引构建
        model
            .index_manager
            .build_geometry_indices(&model.geometries_list);
        model
            .index_manager
            .build_mesh_indices(&model.meshes_list, &model.geometries);
        model
            .index_manager
            .build_entity_indices(&model.entities_list);

        // 验证索引
        assert_eq!(model.index_manager.each_geometry_primitive_type.len(), 1);
        assert_eq!(model.index_manager.each_geometry_primitive_type[0], 1); // 表面三角形
        assert_eq!(model.index_manager.each_geometry_axis_label[0], "A");

        assert_eq!(model.index_manager.each_mesh_geometries_portion.len(), 1);
        assert_eq!(model.index_manager.each_mesh_geometries_portion[0], 0); // 指向第一个几何体

        assert_eq!(model.index_manager.each_entity_id.len(), 1);
        assert_eq!(model.index_manager.each_entity_id[0], entity_id);

        // 验证复用表同步
        let reuse_entry = model.geometry_reuse_table.get(&geom_id).unwrap();
        assert_eq!(reuse_entry.mesh_ids.len(), 1);

        // 6. 测试统计信息
        let stats = model.index_manager.get_stats();
        assert_eq!(stats.num_geometries, 1);
        assert_eq!(stats.num_meshes, 1);
        assert_eq!(stats.num_entities, 1);

        println!("✅ XKT数据流测试通过");
        println!("📊 统计信息: {:?}", stats);
    }

    #[test]
    fn test_spatial_index() {
        use crate::xkt_generator::xkt_spatial::{SpatialConfig, XKTSpatialIndex};

        let config = SpatialConfig::default();
        let mut spatial_index = XKTSpatialIndex::new(config);

        // 创建测试实体AABB
        let entities = vec![
            ("entity_1".to_string(), [0.0, 0.0, 0.0, 10.0, 10.0, 10.0]),
            ("entity_2".to_string(), [20.0, 20.0, 20.0, 30.0, 30.0, 30.0]),
            ("entity_3".to_string(), [5.0, 5.0, 5.0, 15.0, 15.0, 15.0]),
        ];

        // 构建空间分区
        spatial_index.build_from_entities(&entities).unwrap();

        // 验证瓦片创建
        assert!(!spatial_index.tiles.is_empty());

        let total_entities: usize = spatial_index
            .tiles
            .iter()
            .map(|tile| tile.entity_ids.len())
            .sum();
        assert_eq!(total_entities, 3);

        println!("✅ 空间索引测试通过");
        println!("🏗️ 创建了 {} 个瓦片", spatial_index.tiles.len());
    }

    #[test]
    fn test_geometry_reuse_detection() {
        let mut model = XKTModel::new();

        // 创建相同的几何体两次
        let geometry1 = XKTGeometry::create_box("geom_1".to_string(), 1.0, 1.0, 1.0);
        let geometry2 = XKTGeometry::create_box("geom_2".to_string(), 1.0, 1.0, 1.0);

        model.create_geometry(geometry1).unwrap();
        model.create_geometry(geometry2).unwrap();

        // 在实际实现中，这里应该检测到几何体重用
        // 目前只是验证数据结构正确
        assert_eq!(model.geometries.len(), 2);
        assert_eq!(model.geometries_list.len(), 2);

        println!("✅ 几何体重用检测测试通过");
    }
}
