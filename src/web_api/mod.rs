pub mod spatial_query_api;

pub use spatial_query_api::{SpatialQueryApiState, create_spatial_query_routes};

#[cfg(test)]
mod tests;
