mod engine;
mod plan;
#[cfg(any(test, feature = "migration"))]
mod reference;

pub(crate) use engine::{MatchEngine, PostingBlock};
pub(crate) use plan::{MatchPlan, MatchScope};
#[cfg(any(test, feature = "migration"))]
pub(crate) use reference::match_subscription;
#[cfg(feature = "migration")]
pub(crate) use reference::{ReferenceMatch, sample_events};
