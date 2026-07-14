use crate::models::{
    DisasterEvent, IncidentApplyOutcome, IncidentCapacity, IncidentId, IncidentRecord,
};

#[derive(Debug, Clone)]
pub(crate) struct IncidentTransition {
    pub(crate) incident: IncidentRecord,
    pub(crate) outcome: IncidentApplyOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IncidentError {
    Capacity(IncidentCapacity),
}

impl std::fmt::Display for IncidentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "incident capacity exceeded: {:?}",
            match self {
                Self::Capacity(value) => value,
            }
        )
    }
}

impl std::error::Error for IncidentError {}

pub(crate) fn reduce_incident_at(
    current: Option<&IncidentRecord>,
    event: &DisasterEvent,
    now_ms: i64,
) -> Result<IncidentTransition, IncidentError> {
    let mut incident = current.cloned().unwrap_or_else(|| {
        IncidentRecord::new(IncidentId::derive(&event.event_key()), event, now_ms)
    });
    let outcome = if current.is_some() {
        incident.apply_outcome(event, now_ms)
    } else {
        IncidentApplyOutcome::Applied
    };
    if let IncidentApplyOutcome::CapacityExceeded(capacity) = outcome {
        return Err(IncidentError::Capacity(capacity));
    }
    if outcome.should_project() {
        incident
            .remember_source_event_keys([event.event_key().as_str()])
            .map_err(IncidentError::Capacity)?;
    }
    Ok(IncidentTransition { incident, outcome })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DisasterCategory, ProviderChannel};

    fn event(report_num: u32, level: u8) -> DisasterEvent {
        DisasterEvent {
            category: DisasterCategory::EarthquakeWarning,
            channel: ProviderChannel::Wolfx,
            source: "wolfx.cenc_eew".to_string(),
            event_id: "event".to_string(),
            revision: report_num.to_string(),
            report_num,
            title: String::new(),
            description: String::new(),
            latitude: Some(35.0),
            longitude: Some(105.0),
            magnitude: Some(5.0),
            depth_km: Some(10.0),
            affected_regions: Vec::new(),
            radius_km: None,
            level,
            occurred_at: "2026-07-13T00:00:00Z".to_string(),
            final_report: false,
            cancel: false,
            training: false,
        }
    }

    #[test]
    fn stale_high_level_report_does_not_bypass_source_order() -> anyhow::Result<()> {
        let current = reduce_incident_at(None, &event(3, 2), 1)?.incident;
        let stale = reduce_incident_at(Some(&current), &event(2, 5), 2)?;
        anyhow::ensure!(stale.outcome == IncidentApplyOutcome::Rejected);
        anyhow::ensure!(stale.incident.stream_watermarks[0].report_num == 3);
        anyhow::ensure!(stale.incident.stream_watermarks[0].level == 2);
        Ok(())
    }

    #[test]
    fn delayed_cancel_is_monotonic_without_lowering_watermark() -> anyhow::Result<()> {
        let current = reduce_incident_at(None, &event(3, 2), 1)?.incident;
        let mut cancel = event(2, 1);
        cancel.cancel = true;
        let transition = reduce_incident_at(Some(&current), &cancel, 2)?;
        anyhow::ensure!(transition.outcome == IncidentApplyOutcome::Applied);
        anyhow::ensure!(transition.incident.stream_watermarks[0].cancel);
        anyhow::ensure!(transition.incident.stream_watermarks[0].report_num == 3);
        anyhow::ensure!(transition.incident.stream_watermarks[0].level == 2);
        Ok(())
    }
}
