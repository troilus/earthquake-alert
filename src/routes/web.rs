use crate::delivery::NotificationVerifyError;
use crate::models::IncidentId;
use crate::routes::AppState;
use crate::routes::detail_page::{
    detail_error, detail_not_found, detail_response, detail_unavailable, render_incident_page,
};
use axum::{
    extract::{Path, State},
    response::{Html, IntoResponse, Response},
};

const INDEX_HTML: &str = include_str!(concat!(env!("OUT_DIR"), "/index.min.html"));

enum DetailLoadError {
    InvalidLink(anyhow::Error),
    Storage(anyhow::Error),
}

pub(crate) async fn index_handler() -> impl IntoResponse {
    Html(INDEX_HTML)
}

pub(crate) async fn incident_detail_handler(
    State(state): State<AppState>,
    Path((incident_id, token)): Path<(String, String)>,
) -> Response {
    let Some(incident_id) = IncidentId::parse(&incident_id) else {
        return detail_not_found();
    };
    let Ok(permit) = state.detail_concurrency.clone().try_acquire_owned() else {
        tracing::warn!(
            event = "incident.detail_overloaded",
            incident_id = %incident_id.as_str(),
            "incident.detail_overloaded"
        );
        return detail_unavailable();
    };
    let Ok(database_permit) = state.storage_concurrency.clone().try_acquire_owned() else {
        tracing::warn!(
            event = "incident.detail_storage_overloaded",
            incident_id = %incident_id.as_str(),
            "incident.detail_storage_overloaded"
        );
        return detail_unavailable();
    };
    let links = state.notification_links.clone();
    let verify_incident_id = incident_id.clone();
    let storage = state.storage.clone();
    let loaded = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _database_permit = database_permit;
        let snapshot = links
            .verify(&verify_incident_id, &token)
            .map_err(|error| match error {
                NotificationVerifyError::Invalid(error) => DetailLoadError::InvalidLink(error),
                NotificationVerifyError::Storage(error) => DetailLoadError::Storage(error),
            })?;
        let incident = storage
            .incident(&verify_incident_id)
            .map_err(DetailLoadError::Storage)?;
        Ok::<_, DetailLoadError>((snapshot, incident))
    })
    .await;
    let (snapshot, incident) = match loaded {
        Ok(Ok(loaded)) => loaded,
        Ok(Err(DetailLoadError::InvalidLink(error))) => {
            tracing::warn!(
                event = "incident.invalid_notification_link",
                incident_id = %incident_id.as_str(),
                error = %error,
                "incident.invalid_notification_link"
            );
            return detail_not_found();
        }
        Ok(Err(DetailLoadError::Storage(error))) => {
            tracing::error!(
                event = "incident.read_failed",
                incident_id = %incident_id.as_str(),
                error = ?error,
                "incident.read_failed"
            );
            return detail_error();
        }
        Err(error) => {
            tracing::error!(
                event = "incident.notification_verify_task_failed",
                incident_id = %incident_id.as_str(),
                error = ?error,
                "incident.notification_verify_task_failed"
            );
            return detail_error();
        }
    };
    detail_response(render_incident_page(&snapshot, incident.as_deref()))
}
