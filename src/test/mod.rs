// mod test_cata_expression;
// mod test_cata_hangers;
// mod test_dir;
// pub mod test_helper;
// pub mod common;
// mod test_api;
// mod test_spatial;
mod test_gen_model;
pub mod test_performance;
// mod test_incr_update;

// mod test_data_state;

// mod test_query;

// 重新导出性能测试函数，方便外部调用
pub use test_performance::{
    test_model_generation_performance,
    analyze_performance_bottlenecks,
    init_performance_tracing,
    generate_detailed_stage_analysis,
    generate_optimization_recommendations,
    test_gen_geos_data_performance,
    batch_test_gen_geos_data_performance,
    test_gen_geos_data_from_database,
    save_gen_geos_data_report,
    test_parallel_cata_geos_performance,
    PerformanceStats,
    StageAnalysis,
    GenGeosDataPerformanceStats,
    GenGeosDataMetrics,
    ParallelCataPerformanceStats,
};
