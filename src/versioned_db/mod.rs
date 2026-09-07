pub mod attmap;
/// 基线写者的渲染半边：e3d-io 退化窗口的块 → `INSERT IGNORE` 批（行形状与增量路同一份）。
pub mod baseline;
/// 基线写者的编排半边：一个库文件 → 流式收集 → 有界队列 → 单写者 → 统计收尾（不动水位）。
pub mod baseline_ingest;
pub mod database;
