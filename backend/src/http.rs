use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::domain::{CreateEventCommand, EventFilter, EventInput, SyncStatus, TrackerError};
use crate::service::TrackerService;

pub fn router(service: Arc<TrackerService>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/bootstrap", get(bootstrap))
        .route("/api/v1/events", get(list_events).post(create_event))
        .route("/api/v1/events/preview", post(preview_event))
        .route("/api/v1/events/{id}/sync", post(sync_event))
        .route("/api/v1/sync", post(sync_pending))
        .layer(middleware::from_fn(log_request))
        .with_state(service)
}

async fn log_request(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let started = Instant::now();
    info!(%method, %path, "HTTP 请求开始");
    let response = next.run(request).await;
    info!(
        %method,
        %path,
        status = response.status().as_u16(),
        elapsed_ms = started.elapsed().as_millis(),
        "HTTP 请求完成"
    );
    response
}

#[derive(Debug, Serialize)]
struct HealthView {
    status: &'static str,
    database: &'static str,
    calendar_supported: bool,
}

#[derive(Debug, Deserialize)]
struct EventQuery {
    status: Option<SyncStatus>,
    event_date: Option<String>,
    calendar_name: Option<String>,
    #[serde(default = "default_limit")]
    limit: u32,
    #[serde(default)]
    offset: u32,
}

fn default_limit() -> u32 {
    50
}

async fn health(State(service): State<Arc<TrackerService>>) -> Result<Json<HealthView>, ApiError> {
    service.health().await?;
    Ok(Json(HealthView {
        status: "ok",
        database: "ok",
        calendar_supported: service.calendar_supported(),
    }))
}

async fn bootstrap(
    State(service): State<Arc<TrackerService>>,
) -> Result<Json<crate::domain::BootstrapView>, ApiError> {
    Ok(Json(service.snapshot().await?))
}

async fn preview_event(
    State(service): State<Arc<TrackerService>>,
    payload: Result<Json<EventInput>, JsonRejection>,
) -> Result<Json<crate::domain::EventPreview>, ApiError> {
    let Json(input) = payload.map_err(ApiError::invalid_json)?;
    Ok(Json(service.preview_event(&input).await?))
}

async fn create_event(
    State(service): State<Arc<TrackerService>>,
    payload: Result<Json<CreateEventCommand>, JsonRejection>,
) -> Result<(StatusCode, Json<crate::domain::CreateEventResult>), ApiError> {
    let Json(command) = payload.map_err(ApiError::invalid_json)?;
    let result = service.create_event(command).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

async fn list_events(
    State(service): State<Arc<TrackerService>>,
    query: Result<Query<EventQuery>, QueryRejection>,
) -> Result<Json<crate::domain::EventPage>, ApiError> {
    let Query(query) = query.map_err(ApiError::invalid_query)?;
    let filter = EventFilter {
        status: query.status,
        event_date: query.event_date,
        calendar_name: query.calendar_name,
        limit: query.limit,
        offset: query.offset,
    };
    Ok(Json(service.list_events(filter).await?))
}

async fn sync_event(
    State(service): State<Arc<TrackerService>>,
    Path(id): Path<String>,
) -> Result<Json<crate::domain::SyncEventResult>, ApiError> {
    Ok(Json(service.sync_event(&id).await?))
}

async fn sync_pending(
    State(service): State<Arc<TrackerService>>,
) -> Result<Json<crate::domain::BatchSyncResult>, ApiError> {
    Ok(Json(service.sync_pending().await?))
}

#[derive(Debug, Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: String,
    message: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    fields: BTreeMap<String, String>,
}

struct ApiError {
    status: StatusCode,
    body: ErrorBody,
}

impl ApiError {
    fn invalid_json(rejection: JsonRejection) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            body: ErrorBody {
                code: "invalid_json".into(),
                message: rejection.body_text(),
                fields: BTreeMap::new(),
            },
        }
    }

    fn invalid_query(rejection: QueryRejection) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            body: ErrorBody {
                code: "invalid_query".into(),
                message: rejection.body_text(),
                fields: BTreeMap::new(),
            },
        }
    }
}

impl From<TrackerError> for ApiError {
    fn from(error: TrackerError) -> Self {
        let (status, code) = match &error {
            TrackerError::Validation { .. } => {
                (StatusCode::UNPROCESSABLE_ENTITY, "validation_error")
            }
            TrackerError::Duplicate => (StatusCode::CONFLICT, "duplicate_event"),
            TrackerError::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            TrackerError::DatabaseBusy => (StatusCode::SERVICE_UNAVAILABLE, "database_busy"),
            TrackerError::Database(_) | TrackerError::LocalTime | TrackerError::Internal(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
            }
        };
        if status.is_server_error() {
            error!(error = %error, "HTTP 请求失败");
        }
        let fields = error.fields().cloned().unwrap_or_default();
        let message = if status.is_server_error() {
            "后端无法完成请求".to_owned()
        } else {
            error.to_string()
        };
        Self {
            status,
            body: ErrorBody {
                code: code.into(),
                message,
                fields,
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(ErrorEnvelope { error: self.body })).into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tempfile::TempDir;
    use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};
    use tower::ServiceExt;

    use super::*;
    use crate::calendar::{CalendarError, CalendarErrorCode, CalendarEvent, CalendarPort};
    use crate::database::connect_database;
    use crate::domain::{CalendarAction, Clock, TrackerError};

    struct FixedClock;

    impl Clock for FixedClock {
        fn local_now(&self) -> Result<PrimitiveDateTime, TrackerError> {
            Ok(PrimitiveDateTime::new(
                Date::from_calendar_date(2026, Month::August, 30).unwrap(),
                Time::from_hms(21, 45, 0).unwrap(),
            ))
        }

        fn utc_now(&self) -> OffsetDateTime {
            self.local_now()
                .unwrap()
                .assume_offset(UtcOffset::from_hms(8, 0, 0).unwrap())
                .to_offset(UtcOffset::UTC)
        }
    }

    struct FakeCalendar {
        responses: Mutex<VecDeque<Result<CalendarAction, CalendarError>>>,
    }

    #[async_trait]
    impl CalendarPort for FakeCalendar {
        async fn create_or_find(
            &self,
            _event: CalendarEvent,
        ) -> Result<CalendarAction, CalendarError> {
            self.responses.lock().unwrap().pop_front().unwrap()
        }

        fn is_supported(&self) -> bool {
            true
        }
    }

    async fn app(responses: Vec<Result<CalendarAction, CalendarError>>) -> (TempDir, Router) {
        let directory = tempfile::tempdir().unwrap();
        let pool = connect_database(&directory.path().join("tracker.sqlite3"))
            .await
            .unwrap();
        let service = TrackerService::new(
            pool,
            Arc::new(FakeCalendar {
                responses: Mutex::new(responses.into()),
            }),
            Arc::new(FixedClock),
        );
        (directory, router(Arc::new(service)))
    }

    async fn json(response: Response) -> serde_json::Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn event_body(action: &str) -> String {
        serde_json::json!({
            "calendar_name": "学习",
            "event_date": "2026-08-30",
            "start_time": "21:00",
            "end_time": "21:30",
            "title": "资料分析",
            "first_item": 205,
            "last_item": 218,
            "action": action,
            "allow_duplicate": false
        })
        .to_string()
    }

    #[tokio::test]
    async fn preview_has_no_side_effect_or_internal_marker() {
        let (_directory, app) = app(vec![]).await;
        let body = serde_json::json!({
            "calendar_name": "学习",
            "event_date": "2030-01-02",
            "start_time": "08:07:12",
            "end_time": "11:43:09",
            "title": "资料分析",
            "first_item": 205,
            "last_item": 218
        });
        let response = app
            .oneshot(
                Request::post("/api/v1/events/preview")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = json(response).await;
        assert_eq!(body["event_date"], "2030-01-02");
        assert_eq!(body["start_time"], "08:07:12");
        assert_eq!(body["end_time"], "11:43:09");
        assert!(body.get("sync_marker").is_none());
        assert!(body.get("stages").is_none());
    }

    #[tokio::test]
    async fn calendar_failure_is_a_created_pending_result() {
        let (_directory, app) = app(vec![Err(CalendarError::new(
            CalendarErrorCode::Missing,
            "missing",
        ))])
        .await;
        let response = app
            .oneshot(
                Request::post("/api/v1/events")
                    .header("content-type", "application/json")
                    .body(Body::from(event_body("save_and_sync")))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        let body = json(response).await;
        assert_eq!(body["event"]["sync_status"], "pending");
        assert_eq!(body["sync"]["status"], "failed");
        assert_eq!(body["sync"]["error"]["code"], "calendar_missing");
        assert!(body["event"].get("sync_marker").is_none());
    }

    #[tokio::test]
    async fn duplicate_and_invalid_json_use_stable_error_envelopes() {
        let (_directory, app) = app(vec![]).await;
        let create = || {
            Request::post("/api/v1/events")
                .header("content-type", "application/json")
                .body(Body::from(event_body("save")))
                .unwrap()
        };
        assert_eq!(
            app.clone().oneshot(create()).await.unwrap().status(),
            StatusCode::CREATED
        );
        let duplicate = app.clone().oneshot(create()).await.unwrap();
        assert_eq!(duplicate.status(), StatusCode::CONFLICT);
        assert_eq!(json(duplicate).await["error"]["code"], "duplicate_event");

        let invalid = app
            .oneshot(
                Request::post("/api/v1/events")
                    .header("content-type", "application/json")
                    .body(Body::from("{"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        assert_eq!(json(invalid).await["error"]["code"], "invalid_json");
    }
}
