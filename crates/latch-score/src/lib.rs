pub use latch_core::{
    config::PoolFeedback,
    score::{
        EndpointScore, ObservationError, RequestObservation, ScoreBreakdown, ScoreConfig,
        ScoreTier,
    },
};

mod engine;
mod scoring;
mod state;
mod types;

pub use engine::ScoringEngine;
pub use state::{EndpointState, ScoreSnapshot};
pub use types::{Clock, PoolRanking, SystemClock};

#[cfg(test)]
mod tests;
