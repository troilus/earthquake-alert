mod coordinator;
mod reducer;

pub(crate) use coordinator::{EventCoordinator, EventPolicy};

use crate::models::IncidentId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MatchJob {
    pub(crate) id: u64,
    pub(crate) incident_id: IncidentId,
    pub(crate) event_revision: u64,
    pub(crate) created_at_ms: i64,
}
