// Module declarations
mod analyzer;
mod scorer;
mod ranker;

// Re-exports
pub use analyzer::{ContentProfile, analyze_content};
pub use scorer::calculate_pool_score;
pub use ranker::route_model;

// For backward compatibility
#[deprecated(since = "0.2", note = "Use ContentProfile instead")]
pub type ContentFeatures = ContentProfile;
