use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};
use tokio::sync::Mutex;
use tracing::{error, info};
use uuid::Uuid;

use crate::calendar::{CalendarEvent, CalendarPort};
use crate::database::{connect_database, database_health};
use crate::domain::{
    BatchSyncResult, BootstrapView, CanonicalEvent, Clock, CreateAction, CreateEventCommand,
    CreateEventResult, EventFilter, EventInput, EventPage, EventPreview, EventRecord,
    EventSuggestion, EventView, OperationErrorView, SyncEventResult, SyncOutcome,
    SyncOutcomeStatus, SyncStatus, TrackerError, canonicalize_new_event, format_date,
    format_timestamp,
};

const EVENT_COLUMNS: &str = "id, calendar_name, event_date, start_time, end_time, title, \
    first_item, last_item, sync_marker, sync_status, sync_attempts, \
    last_sync_error_code, last_sync_error";

pub struct TrackerService {
    pool: SqlitePool,
    calendar: Arc<dyn CalendarPort>,
    clock: Arc<dyn Clock>,
    create_lock: Mutex<()>,
    sync_lock: Mutex<()>,
}

impl std::fmt::Debug for TrackerService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("TrackerService").finish()
    }
}

impl TrackerService {
    pub async fn open(
        database_path: &Path,
        calendar: Arc<dyn CalendarPort>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, TrackerError> {
        let pool = connect_database(database_path).await?;
        Ok(Self::new(pool, calendar, clock))
    }

    pub fn new(pool: SqlitePool, calendar: Arc<dyn CalendarPort>, clock: Arc<dyn Clock>) -> Self {
        Self {
            pool,
            calendar,
            clock,
            create_lock: Mutex::new(()),
            sync_lock: Mutex::new(()),
        }
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub fn calendar_supported(&self) -> bool {
        self.calendar.is_supported()
    }

    pub async fn health(&self) -> Result<(), TrackerError> {
        database_health(&self.pool).await
    }

    pub async fn snapshot(&self) -> Result<BootstrapView, TrackerError> {
        let now = self.clock.local_now()?;
        let pending_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM events WHERE sync_status = 'pending'",
        )
        .fetch_one(&self.pool)
        .await?;
        let rows = sqlx::query(
            "SELECT calendar_name, title, MAX(last_item) AS last_item, MAX(created_at) AS latest \
             FROM events GROUP BY calendar_name, title ORDER BY latest DESC LIMIT 20",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut calendar_names = Vec::new();
        let mut titles = Vec::new();
        let mut seen_calendars = HashSet::new();
        let mut seen_titles = HashSet::new();
        let mut recent_suggestions = Vec::new();
        for row in rows {
            let calendar_name: String = row.try_get("calendar_name")?;
            let title: String = row.try_get("title")?;
            let last_item: i64 = row.try_get("last_item")?;
            if seen_calendars.insert(calendar_name.clone()) {
                calendar_names.push(calendar_name.clone());
            }
            if seen_titles.insert(title.clone()) {
                titles.push(title.clone());
            }
            let next_first_item = last_item
                .checked_add(1)
                .ok_or_else(|| TrackerError::Internal("历史编号超出整数范围".into()))?;
            recent_suggestions.push(EventSuggestion {
                calendar_name,
                title,
                next_first_item,
            });
        }
        Ok(BootstrapView {
            server_date: format_date(now.date()),
            calendar_names,
            titles,
            recent_suggestions,
            pending_count,
        })
    }

    pub async fn preview_event(&self, input: &EventInput) -> Result<EventPreview, TrackerError> {
        let event = canonicalize_new_event(input)?;
        let duplicate_status = self.find_duplicate_status(&event).await?;
        Ok(EventPreview {
            calendar_name: event.calendar_name,
            event_date: event.event_date,
            start_time: event.start_time,
            end_time: event.end_time,
            title: event.title,
            first_item: event.first_item,
            last_item: event.last_item,
            quantity: event.quantity,
            duplicate_status,
        })
    }

    pub async fn create_event(
        &self,
        command: CreateEventCommand,
    ) -> Result<CreateEventResult, TrackerError> {
        let event = canonicalize_new_event(&command.event)?;
        let id = Uuid::new_v4().to_string();
        let timestamp = format_timestamp(self.clock.utc_now());
        {
            let _guard = self.create_lock.lock().await;
            if !command.allow_duplicate && self.find_duplicate_status(&event).await?.is_some() {
                return Err(TrackerError::Duplicate);
            }
            sqlx::query(
                "INSERT INTO events (id, calendar_name, event_date, start_time, end_time, title, \
                 first_item, last_item, sync_marker, sync_status, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?)",
            )
            .bind(&id)
            .bind(&event.calendar_name)
            .bind(&event.event_date)
            .bind(&event.start_time)
            .bind(&event.end_time)
            .bind(&event.title)
            .bind(event.first_item)
            .bind(event.last_item)
            .bind(&event.marker)
            .bind(&timestamp)
            .bind(&timestamp)
            .execute(&self.pool)
            .await?;
        }

        info!(event_id = %id, marker = %event.marker, "事件已保存");
        match command.action {
            CreateAction::Save => Ok(CreateEventResult {
                event: self.get_event(&id).await?.to_view()?,
                save_status: "created".into(),
                sync: SyncOutcome::not_requested(),
            }),
            CreateAction::SaveAndSync => match self.sync_event(&id).await {
                Ok(synced) => Ok(CreateEventResult {
                    event: synced.event,
                    save_status: "created".into(),
                    sync: synced.sync,
                }),
                Err(sync_error) => {
                    error!(event_id = %id, error = %sync_error, "事件已保存，但同步状态未知");
                    Ok(CreateEventResult {
                        event: pending_view(&id, &event),
                        save_status: "created".into(),
                        sync: SyncOutcome {
                            status: SyncOutcomeStatus::Failed,
                            action: None,
                            error: Some(OperationErrorView {
                                code: "sync_state_unknown".into(),
                                message: "事件已保存，但后端无法确认本次 Calendar 同步状态；请按事件 ID 重试".into(),
                            }),
                        },
                    })
                }
            },
        }
    }

    pub async fn list_events(&self, mut filter: EventFilter) -> Result<EventPage, TrackerError> {
        if let Some(date) = &filter.event_date {
            filter.event_date = Some(format_date(crate::domain::parse_date(date, "event_date")?));
        }
        if let Some(calendar_name) = &filter.calendar_name {
            let calendar_name = calendar_name.trim();
            if calendar_name.is_empty() {
                return Err(TrackerError::field("calendar_name", "不能为空"));
            }
            filter.calendar_name = Some(calendar_name.to_owned());
        }
        let limit = if filter.limit == 0 {
            50
        } else {
            filter.limit.min(100)
        };

        let mut count = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM events");
        push_filters(&mut count, &filter);
        let total_count: i64 = count.build_query_scalar().fetch_one(&self.pool).await?;

        let mut query = QueryBuilder::<Sqlite>::new(format!("SELECT {EVENT_COLUMNS} FROM events"));
        push_filters(&mut query, &filter);
        query
            .push(" ORDER BY event_date DESC, start_time DESC, created_at DESC, id DESC LIMIT ")
            .push_bind(i64::from(limit))
            .push(" OFFSET ")
            .push_bind(i64::from(filter.offset));
        let rows = query.build().fetch_all(&self.pool).await?;
        let items = rows
            .iter()
            .map(record_from_row)
            .map(|record| record.and_then(|record| record.to_view()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(EventPage {
            items,
            total_count,
            limit,
            offset: filter.offset,
        })
    }

    pub async fn sync_event(&self, event_id: &str) -> Result<SyncEventResult, TrackerError> {
        let _guard = self.sync_lock.lock().await;
        self.sync_event_locked(event_id).await
    }

    pub async fn sync_pending(&self) -> Result<BatchSyncResult, TrackerError> {
        let _guard = self.sync_lock.lock().await;
        let ids = sqlx::query_scalar::<_, String>(
            "SELECT id FROM events WHERE sync_status = 'pending' \
             ORDER BY event_date ASC, start_time ASC, created_at ASC, id ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        let attempted = ids.len();
        let mut succeeded = 0;
        let mut failed = 0;
        let mut results = Vec::with_capacity(attempted);
        for id in ids {
            let result = self.sync_event_locked(&id).await?;
            match result.sync.status {
                SyncOutcomeStatus::Succeeded | SyncOutcomeStatus::AlreadySynced => succeeded += 1,
                SyncOutcomeStatus::Failed => failed += 1,
                SyncOutcomeStatus::NotRequested => {}
            }
            results.push(result);
        }
        Ok(BatchSyncResult {
            attempted,
            succeeded,
            failed,
            results,
        })
    }

    async fn sync_event_locked(&self, event_id: &str) -> Result<SyncEventResult, TrackerError> {
        let event = self.get_event(event_id).await?;
        if event.sync_status == SyncStatus::Synced {
            return Ok(SyncEventResult {
                event: event.to_view()?,
                sync: SyncOutcome::already_synced(),
            });
        }
        let request = CalendarEvent {
            calendar_name: event.calendar_name.clone(),
            title: event.title.clone(),
            event_date: event.event_date.clone(),
            start_time: event.start_time.clone(),
            end_time: event.end_time.clone(),
            description: event.description()?,
            marker: event.sync_marker.clone(),
        };
        let sync_started = Instant::now();
        info!(event_id = %event_id, marker = %event.sync_marker, "开始同步 Calendar");
        let attempted_at = format_timestamp(self.clock.utc_now());
        let sync = match self.calendar.create_or_find(request).await {
            Ok(action) => {
                let completed_at = format_timestamp(self.clock.utc_now());
                sqlx::query(
                    "UPDATE events SET sync_status = 'synced', sync_attempts = sync_attempts + 1, \
                     last_sync_error_code = NULL, last_sync_error = NULL, last_sync_action = ?, \
                     last_attempted_at = ?, synced_at = ?, updated_at = ? WHERE id = ?",
                )
                .bind(action.as_str())
                .bind(&attempted_at)
                .bind(&completed_at)
                .bind(&completed_at)
                .bind(event_id)
                .execute(&self.pool)
                .await?;
                info!(
                    event_id = %event_id,
                    action = action.as_str(),
                    elapsed_ms = sync_started.elapsed().as_millis(),
                    "Calendar 同步完成"
                );
                SyncOutcome {
                    status: SyncOutcomeStatus::Succeeded,
                    action: Some(action),
                    error: None,
                }
            }
            Err(calendar_error) => {
                let completed_at = format_timestamp(self.clock.utc_now());
                sqlx::query(
                    "UPDATE events SET sync_attempts = sync_attempts + 1, \
                     last_sync_error_code = ?, last_sync_error = ?, last_sync_action = NULL, \
                     last_attempted_at = ?, updated_at = ? WHERE id = ?",
                )
                .bind(calendar_error.code.as_str())
                .bind(&calendar_error.message)
                .bind(&attempted_at)
                .bind(&completed_at)
                .bind(event_id)
                .execute(&self.pool)
                .await?;
                error!(
                    event_id = %event_id,
                    code = calendar_error.code.as_str(),
                    elapsed_ms = sync_started.elapsed().as_millis(),
                    "Calendar 同步失败"
                );
                SyncOutcome {
                    status: SyncOutcomeStatus::Failed,
                    action: None,
                    error: Some(calendar_error.to_view()),
                }
            }
        };
        Ok(SyncEventResult {
            event: self.get_event(event_id).await?.to_view()?,
            sync,
        })
    }

    async fn find_duplicate_status(
        &self,
        event: &CanonicalEvent,
    ) -> Result<Option<SyncStatus>, TrackerError> {
        let status = sqlx::query_scalar::<_, String>(
            "SELECT sync_status FROM events WHERE calendar_name = ? AND event_date = ? \
             AND start_time = ? AND end_time = ? AND title = ? AND first_item = ? \
             AND last_item = ? ORDER BY created_at DESC LIMIT 1",
        )
        .bind(&event.calendar_name)
        .bind(&event.event_date)
        .bind(&event.start_time)
        .bind(&event.end_time)
        .bind(&event.title)
        .bind(event.first_item)
        .bind(event.last_item)
        .fetch_optional(&self.pool)
        .await?;
        status.as_deref().map(SyncStatus::parse).transpose()
    }

    async fn get_event(&self, event_id: &str) -> Result<EventRecord, TrackerError> {
        let query = format!("SELECT {EVENT_COLUMNS} FROM events WHERE id = ?");
        let row = sqlx::query(&query)
            .bind(event_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(TrackerError::NotFound)?;
        record_from_row(&row)
    }
}

fn push_filters<'args>(query: &mut QueryBuilder<'args, Sqlite>, filter: &'args EventFilter) {
    let mut has_filter = false;
    if let Some(status) = filter.status {
        push_filter_prefix(query, &mut has_filter);
        query.push("sync_status = ").push_bind(status.as_str());
    }
    if let Some(event_date) = &filter.event_date {
        push_filter_prefix(query, &mut has_filter);
        query.push("event_date = ").push_bind(event_date);
    }
    if let Some(calendar_name) = &filter.calendar_name {
        push_filter_prefix(query, &mut has_filter);
        query
            .push("calendar_name = ")
            .push_bind(calendar_name.trim());
    }
}

fn push_filter_prefix(query: &mut QueryBuilder<'_, Sqlite>, has_filter: &mut bool) {
    query.push(if *has_filter { " AND " } else { " WHERE " });
    *has_filter = true;
}

fn record_from_row(row: &SqliteRow) -> Result<EventRecord, TrackerError> {
    Ok(EventRecord {
        id: row.try_get("id")?,
        calendar_name: row.try_get("calendar_name")?,
        event_date: row.try_get("event_date")?,
        start_time: row.try_get("start_time")?,
        end_time: row.try_get("end_time")?,
        title: row.try_get("title")?,
        first_item: row.try_get("first_item")?,
        last_item: row.try_get("last_item")?,
        sync_marker: row.try_get("sync_marker")?,
        sync_status: SyncStatus::parse(row.try_get::<String, _>("sync_status")?.as_str())?,
        sync_attempts: row.try_get("sync_attempts")?,
        last_sync_error_code: row.try_get("last_sync_error_code")?,
        last_sync_error: row.try_get("last_sync_error")?,
    })
}

fn pending_view(id: &str, event: &CanonicalEvent) -> EventView {
    EventView {
        id: id.to_owned(),
        calendar_name: event.calendar_name.clone(),
        event_date: event.event_date.clone(),
        start_time: event.start_time.clone(),
        end_time: event.end_time.clone(),
        title: event.title.clone(),
        first_item: event.first_item,
        last_item: event.last_item,
        quantity: event.quantity,
        sync_status: SyncStatus::Pending,
        sync_attempts: 0,
        last_sync_error: None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use sqlx::Row;
    use tempfile::TempDir;
    use time::{Date, Duration, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

    use super::*;
    use crate::calendar::{CalendarError, CalendarErrorCode};
    use crate::domain::CalendarAction;

    struct FixedClock {
        local: PrimitiveDateTime,
        utc: OffsetDateTime,
    }

    impl Clock for FixedClock {
        fn local_now(&self) -> Result<PrimitiveDateTime, TrackerError> {
            Ok(self.local)
        }

        fn utc_now(&self) -> OffsetDateTime {
            self.utc
        }
    }

    struct SequencedClock {
        local: PrimitiveDateTime,
        utc_values: StdMutex<VecDeque<OffsetDateTime>>,
    }

    impl Clock for SequencedClock {
        fn local_now(&self) -> Result<PrimitiveDateTime, TrackerError> {
            Ok(self.local)
        }

        fn utc_now(&self) -> OffsetDateTime {
            self.utc_values
                .lock()
                .unwrap()
                .pop_front()
                .expect("test clock ran out of UTC timestamps")
        }
    }

    struct FakeCalendar {
        responses: StdMutex<VecDeque<Result<CalendarAction, CalendarError>>>,
        calls: AtomicUsize,
    }

    impl FakeCalendar {
        fn new(responses: Vec<Result<CalendarAction, CalendarError>>) -> Self {
            Self {
                responses: StdMutex::new(responses.into()),
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl CalendarPort for FakeCalendar {
        async fn create_or_find(
            &self,
            _event: CalendarEvent,
        ) -> Result<CalendarAction, CalendarError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.responses.lock().unwrap().pop_front().unwrap()
        }

        fn is_supported(&self) -> bool {
            true
        }
    }

    struct ClosingCalendar {
        pool: SqlitePool,
    }

    #[async_trait]
    impl CalendarPort for ClosingCalendar {
        async fn create_or_find(
            &self,
            _event: CalendarEvent,
        ) -> Result<CalendarAction, CalendarError> {
            self.pool.close().await;
            Ok(CalendarAction::Created)
        }

        fn is_supported(&self) -> bool {
            true
        }
    }

    fn fixed_clock() -> Arc<dyn Clock> {
        let date = Date::from_calendar_date(2026, Month::August, 30).unwrap();
        let local = PrimitiveDateTime::new(date, Time::from_hms(21, 45, 0).unwrap());
        let utc = local
            .assume_offset(UtcOffset::from_hms(8, 0, 0).unwrap())
            .to_offset(UtcOffset::UTC);
        Arc::new(FixedClock { local, utc })
    }

    async fn service(calendar: Arc<dyn CalendarPort>) -> (TempDir, TrackerService) {
        let directory = tempfile::tempdir().unwrap();
        let service = TrackerService::open(
            &directory.path().join("tracker.sqlite3"),
            calendar,
            fixed_clock(),
        )
        .await
        .unwrap();
        (directory, service)
    }

    fn input() -> EventInput {
        EventInput {
            calendar_name: "学习".into(),
            event_date: "2026-08-30".into(),
            start_time: "21:00".into(),
            end_time: "21:30".into(),
            title: "资料分析".into(),
            first_item: 205,
            last_item: 218,
        }
    }

    #[tokio::test]
    async fn preview_is_pure_and_reports_duplicates() {
        let calendar = Arc::new(FakeCalendar::new(vec![]));
        let (_directory, service) = service(calendar.clone()).await;

        let preview = service.preview_event(&input()).await.unwrap();
        assert_eq!(preview.end_time, "21:30:00");
        assert_eq!(preview.quantity, 14);
        assert_eq!(preview.duplicate_status, None);
        assert_eq!(calendar.calls.load(Ordering::SeqCst), 0);

        service
            .create_event(CreateEventCommand {
                event: input(),
                action: CreateAction::Save,
                allow_duplicate: false,
            })
            .await
            .unwrap();
        assert_eq!(
            service
                .preview_event(&input())
                .await
                .unwrap()
                .duplicate_status,
            Some(SyncStatus::Pending)
        );
    }

    #[tokio::test]
    async fn duplicate_requires_explicit_override() {
        let (_directory, service) = service(Arc::new(FakeCalendar::new(vec![]))).await;
        let command = || CreateEventCommand {
            event: input(),
            action: CreateAction::Save,
            allow_duplicate: false,
        };
        service.create_event(command()).await.unwrap();
        assert!(matches!(
            service.create_event(command()).await,
            Err(TrackerError::Duplicate)
        ));
        let mut allowed = command();
        allowed.allow_duplicate = true;
        service.create_event(allowed).await.unwrap();
        assert_eq!(
            service
                .list_events(EventFilter::default())
                .await
                .unwrap()
                .total_count,
            2
        );
    }

    #[tokio::test]
    async fn list_filters_normalize_date_and_calendar_name() {
        let (_directory, service) = service(Arc::new(FakeCalendar::new(vec![]))).await;
        service
            .create_event(CreateEventCommand {
                event: input(),
                action: CreateAction::Save,
                allow_duplicate: false,
            })
            .await
            .unwrap();

        let page = service
            .list_events(EventFilter {
                status: Some(SyncStatus::Pending),
                event_date: Some("2026/8/30".into()),
                calendar_name: Some(" 学习 ".into()),
                limit: 10,
                offset: 0,
            })
            .await
            .unwrap();
        assert_eq!(page.total_count, 1);
        assert_eq!(page.items[0].title, "资料分析");
        assert!(matches!(
            service
                .list_events(EventFilter {
                    calendar_name: Some("  ".into()),
                    ..EventFilter::default()
                })
                .await,
            Err(TrackerError::Validation { .. })
        ));
    }

    #[tokio::test]
    async fn calendar_failure_keeps_saved_event_pending_and_retry_recovers() {
        let calendar = Arc::new(FakeCalendar::new(vec![
            Err(CalendarError::new(
                CalendarErrorCode::PermissionDenied,
                "denied",
            )),
            Ok(CalendarAction::Existing),
        ]));
        let (_directory, service) = service(calendar).await;

        let created = service
            .create_event(CreateEventCommand {
                event: input(),
                action: CreateAction::SaveAndSync,
                allow_duplicate: false,
            })
            .await
            .unwrap();
        assert_eq!(created.event.sync_status, SyncStatus::Pending);
        assert_eq!(created.event.sync_attempts, 1);
        assert_eq!(created.sync.status, SyncOutcomeStatus::Failed);
        assert_eq!(
            created.sync.error.unwrap().code,
            "calendar_permission_denied"
        );

        let retried = service.sync_event(&created.event.id).await.unwrap();
        assert_eq!(retried.event.sync_status, SyncStatus::Synced);
        assert_eq!(retried.event.sync_attempts, 2);
        assert_eq!(retried.sync.action, Some(CalendarAction::Existing));
    }

    #[tokio::test]
    async fn successful_sync_records_completion_time_separately_from_attempt_time() {
        let date = Date::from_calendar_date(2026, Month::August, 30).unwrap();
        let local = PrimitiveDateTime::new(date, Time::from_hms(21, 45, 0).unwrap());
        let started = local
            .assume_offset(UtcOffset::from_hms(8, 0, 0).unwrap())
            .to_offset(UtcOffset::UTC);
        let attempted = started + Duration::seconds(1);
        let completed = started + Duration::seconds(18);
        let clock: Arc<dyn Clock> = Arc::new(SequencedClock {
            local,
            utc_values: StdMutex::new(VecDeque::from([started, attempted, completed])),
        });
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("tracker.sqlite3");
        let service = TrackerService::open(
            &database_path,
            Arc::new(FakeCalendar::new(vec![Ok(CalendarAction::Created)])),
            clock,
        )
        .await
        .unwrap();

        let result = service
            .create_event(CreateEventCommand {
                event: input(),
                action: CreateAction::SaveAndSync,
                allow_duplicate: false,
            })
            .await
            .unwrap();
        assert_eq!(result.event.sync_status, SyncStatus::Synced);

        let row =
            sqlx::query("SELECT last_attempted_at, synced_at, updated_at FROM events WHERE id = ?")
                .bind(result.event.id)
                .fetch_one(service.pool())
                .await
                .unwrap();
        assert_eq!(
            row.try_get::<String, _>("last_attempted_at").unwrap(),
            format_timestamp(attempted)
        );
        assert_eq!(
            row.try_get::<String, _>("synced_at").unwrap(),
            format_timestamp(completed)
        );
        assert_eq!(
            row.try_get::<String, _>("updated_at").unwrap(),
            format_timestamp(completed)
        );
    }

    #[tokio::test]
    async fn concurrent_sync_calls_only_calendar_once() {
        let calendar = Arc::new(FakeCalendar::new(vec![Ok(CalendarAction::Created)]));
        let (_directory, service) = service(calendar.clone()).await;
        let created = service
            .create_event(CreateEventCommand {
                event: input(),
                action: CreateAction::Save,
                allow_duplicate: false,
            })
            .await
            .unwrap();
        let service = Arc::new(service);
        let (first, second) = tokio::join!(
            service.sync_event(&created.event.id),
            service.sync_event(&created.event.id)
        );

        let statuses = [first.unwrap().sync.status, second.unwrap().sync.status];
        assert!(statuses.contains(&SyncOutcomeStatus::Succeeded));
        assert!(statuses.contains(&SyncOutcomeStatus::AlreadySynced));
        assert_eq!(calendar.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn post_commit_sync_infrastructure_failure_still_reports_the_saved_event() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("tracker.sqlite3");
        let pool = connect_database(&database_path).await.unwrap();
        let service = TrackerService::new(
            pool.clone(),
            Arc::new(ClosingCalendar { pool }),
            fixed_clock(),
        );

        let result = service
            .create_event(CreateEventCommand {
                event: input(),
                action: CreateAction::SaveAndSync,
                allow_duplicate: false,
            })
            .await
            .unwrap();
        assert_eq!(result.event.sync_status, SyncStatus::Pending);
        assert_eq!(result.sync.status, SyncOutcomeStatus::Failed);
        assert_eq!(result.sync.error.unwrap().code, "sync_state_unknown");

        let reopened = connect_database(&database_path).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT sync_status FROM events WHERE id = ?")
                .bind(result.event.id)
                .fetch_one(&reopened)
                .await
                .unwrap(),
            "pending"
        );
    }
}
