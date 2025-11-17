// mod test_cata_expression;
// mod test_cata_hangers;
// mod test_dir;
// pub mod test_helper;
// pub mod common;
// mod test_api;
// mod test_spatial;
mod test_gen_model;
mod test_performance;
// mod test_spatial_index_1112; // 暂时注释，需要修复
mod test_room_integration; // 房间集成测试
mod test_room_v2_verification;
#[cfg(feature = "grpc")]
mod test_sctn_contact;
#[cfg(feature = "grpc")]
mod test_sctn_with_spatial_index;
mod test_sqlite_spatial; // 房间计算 V2 改进验证
