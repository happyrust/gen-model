// mod test_cata_expression;
// mod test_cata_hangers;
// mod test_dir;
// pub mod test_helper;
// pub mod common;
// mod test_api;
// mod test_spatial;
#[cfg(test)]
mod fork_surreal_compat;
mod test_gen_model;
pub mod test_performance;
// mod test_incr_update;

// mod test_data_state;

// mod test_query;

// 重新导出性能测试函数，方便外部调用
pub use test_performance::{
    GenGeosDataMetrics, GenGeosDataPerformanceStats, ParallelCataPerformanceStats,
    PerformanceStats, StageAnalysis, analyze_performance_bottlenecks,
    batch_test_gen_geos_data_performance, generate_detailed_stage_analysis,
    generate_optimization_recommendations, init_performance_tracing, save_gen_geos_data_report,
    test_gen_geos_data_from_database, test_gen_geos_data_performance,
    test_model_generation_performance, test_parallel_cata_geos_performance,
};
