use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time};

pub const MARKER_PREFIX: &str = "weimo-time-tracker-rust-v1:";
pub const MARKER_VERSION: &str = "v1";

#[derive(Debug, Error)]
pub enum TrackerError {
    #[error("{message}")]
    Validation {
        message: String,
        fields: BTreeMap<String, String>,
    },
    #[error("相同记录已经存在")]
    Duplicate,
    #[error("事件不存在")]
    NotFound,
    #[error("数据库正在被另一个进程使用")]
    DatabaseBusy,
    #[error("数据库操作失败")]
    Database(#[source] sqlx::Error),
    #[error("本地时间不可用")]
    LocalTime,
    #[error("内部错误: {0}")]
    Internal(String),
}

impl TrackerError {
    pub fn field(field: &str, message: impl Into<String>) -> Self {
        let message = message.into();
        Self::Validation {
            message: message.clone(),
            fields: BTreeMap::from([(field.to_owned(), message)]),
        }
    }

    pub fn fields(&self) -> Option<&BTreeMap<String, String>> {
        match self {
            Self::Validation { fields, .. } => Some(fields),
            _ => None,
        }
    }
}

impl From<sqlx::Error> for TrackerError {
    fn from(error: sqlx::Error) -> Self {
        if let sqlx::Error::Database(database_error) = &error
            && matches!(database_error.code().as_deref(), Some("5" | "6"))
        {
            return Self::DatabaseBusy;
        }
        Self::Database(error)
    }
}

pub trait Clock: Send + Sync {
    fn local_now(&self) -> Result<PrimitiveDateTime, TrackerError>;
    fn utc_now(&self) -> OffsetDateTime;
}

#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn local_now(&self) -> Result<PrimitiveDateTime, TrackerError> {
        let now = OffsetDateTime::now_local().map_err(|_| TrackerError::LocalTime)?;
        Ok(PrimitiveDateTime::new(now.date(), now.time()))
    }

    fn utc_now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EventInput {
    pub calendar_name: String,
    pub event_date: String,
    pub start_time: String,
    pub end_time: String,
    pub title: String,
    pub first_item: i64,
    pub last_item: i64,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CreateAction {
    Save,
    SaveAndSync,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CreateEventCommand {
    #[serde(flatten)]
    pub event: EventInput,
    pub action: CreateAction,
    #[serde(default)]
    pub allow_duplicate: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SyncStatus {
    Pending,
    Synced,
}

impl SyncStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Synced => "synced",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, TrackerError> {
        match value {
            "pending" => Ok(Self::Pending),
            "synced" => Ok(Self::Synced),
            other => Err(TrackerError::Internal(format!(
                "数据库包含未知同步状态 {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CalendarAction {
    Created,
    Existing,
}

impl CalendarAction {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Existing => "existing",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
pub struct OperationErrorView {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SyncOutcomeStatus {
    NotRequested,
    Succeeded,
    Failed,
    AlreadySynced,
}

#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
pub struct SyncOutcome {
    pub status: SyncOutcomeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<CalendarAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<OperationErrorView>,
}

impl SyncOutcome {
    pub fn not_requested() -> Self {
        Self {
            status: SyncOutcomeStatus::NotRequested,
            action: None,
            error: None,
        }
    }

    pub fn already_synced() -> Self {
        Self {
            status: SyncOutcomeStatus::AlreadySynced,
            action: None,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
pub struct EventView {
    pub id: String,
    pub calendar_name: String,
    pub event_date: String,
    pub start_time: String,
    pub end_time: String,
    pub title: String,
    pub first_item: i64,
    pub last_item: i64,
    pub quantity: i64,
    pub sync_status: SyncStatus,
    pub sync_attempts: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync_error: Option<OperationErrorView>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
pub struct EventPreview {
    pub calendar_name: String,
    pub event_date: String,
    pub start_time: String,
    pub end_time: String,
    pub title: String,
    pub first_item: i64,
    pub last_item: i64,
    pub quantity: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duplicate_status: Option<SyncStatus>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
pub struct EventSuggestion {
    pub calendar_name: String,
    pub title: String,
    pub next_first_item: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
pub struct BootstrapView {
    pub server_date: String,
    pub calendar_names: Vec<String>,
    pub titles: Vec<String>,
    pub recent_suggestions: Vec<EventSuggestion>,
    pub pending_count: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
pub struct CreateEventResult {
    pub event: EventView,
    pub save_status: String,
    pub sync: SyncOutcome,
}

#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
pub struct SyncEventResult {
    pub event: EventView,
    pub sync: SyncOutcome,
}

#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
pub struct BatchSyncResult {
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub results: Vec<SyncEventResult>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
pub struct EventPage {
    pub items: Vec<EventView>,
    pub total_count: i64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    pub status: Option<SyncStatus>,
    pub event_date: Option<String>,
    pub calendar_name: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct CanonicalEvent {
    pub calendar_name: String,
    pub event_date: String,
    pub start_time: String,
    pub end_time: String,
    pub title: String,
    pub first_item: i64,
    pub last_item: i64,
    pub quantity: i64,
    pub marker: String,
}

#[derive(Debug, Clone)]
pub(crate) struct EventRecord {
    pub id: String,
    pub calendar_name: String,
    pub event_date: String,
    pub start_time: String,
    pub end_time: String,
    pub title: String,
    pub first_item: i64,
    pub last_item: i64,
    pub sync_marker: String,
    pub sync_status: SyncStatus,
    pub sync_attempts: i64,
    pub last_sync_error_code: Option<String>,
    pub last_sync_error: Option<String>,
}

impl EventRecord {
    pub(crate) fn quantity(&self) -> Result<i64, TrackerError> {
        quantity(self.first_item, self.last_item)
    }

    pub(crate) fn description(&self) -> Result<String, TrackerError> {
        Ok(event_description(
            self.first_item,
            self.last_item,
            self.quantity()?,
            &self.sync_marker,
        ))
    }

    pub(crate) fn to_view(&self) -> Result<EventView, TrackerError> {
        let last_sync_error = self
            .last_sync_error
            .as_ref()
            .map(|message| OperationErrorView {
                code: self
                    .last_sync_error_code
                    .clone()
                    .unwrap_or_else(|| "calendar_unavailable".to_owned()),
                message: message.clone(),
            });
        Ok(EventView {
            id: self.id.clone(),
            calendar_name: self.calendar_name.clone(),
            event_date: self.event_date.clone(),
            start_time: self.start_time.clone(),
            end_time: self.end_time.clone(),
            title: self.title.clone(),
            first_item: self.first_item,
            last_item: self.last_item,
            quantity: self.quantity()?,
            sync_status: self.sync_status,
            sync_attempts: self.sync_attempts,
            last_sync_error,
        })
    }
}

pub(crate) fn canonicalize_new_event(input: &EventInput) -> Result<CanonicalEvent, TrackerError> {
    let calendar_name = nonempty("calendar_name", &input.calendar_name)?;
    let title = nonempty("title", &input.title)?;
    let date = parse_date(&input.event_date, "event_date")?;
    let start = parse_time(&input.start_time, "start_time")?;
    let end = parse_time(&input.end_time, "end_time")?;
    if end <= start {
        return Err(TrackerError::field("end_time", "结束时间必须晚于开始时间"));
    }
    let event_date = format_date(date);
    let start_time = format_time(start);
    let end_time = format_time(end);
    canonical_from_parts(
        calendar_name,
        event_date,
        start_time,
        end_time,
        title,
        input.first_item,
        input.last_item,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn canonical_from_parts(
    calendar_name: String,
    event_date: String,
    start_time: String,
    end_time: String,
    title: String,
    first_item: i64,
    last_item: i64,
    marker: Option<String>,
) -> Result<CanonicalEvent, TrackerError> {
    let derived_quantity = quantity(first_item, last_item)?;
    let first_text = first_item.to_string();
    let last_text = last_item.to_string();
    let marker = marker.unwrap_or_else(|| {
        event_marker(
            &[
                &calendar_name,
                &event_date,
                &start_time,
                &end_time,
                &title,
                &first_text,
                &last_text,
            ],
            derived_quantity,
        )
    });
    Ok(CanonicalEvent {
        calendar_name,
        event_date,
        start_time,
        end_time,
        title,
        first_item,
        last_item,
        quantity: derived_quantity,
        marker,
    })
}

fn nonempty(field: &str, value: &str) -> Result<String, TrackerError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(TrackerError::field(field, "不能为空"));
    }
    Ok(value.to_owned())
}

fn quantity(first: i64, last: i64) -> Result<i64, TrackerError> {
    if last < first {
        return Err(TrackerError::field("last_item", "结束不能小于起始"));
    }
    last.checked_sub(first)
        .and_then(|difference| difference.checked_add(1))
        .ok_or_else(|| TrackerError::field("last_item", "数量超出整数范围"))
}

pub(crate) fn event_marker(fields: &[&str], quantity: i64) -> String {
    let mut hasher = Sha256::new();
    for (position, value) in fields.iter().enumerate() {
        if position > 0 {
            hasher.update([0x1f]);
        }
        hasher.update(value.trim().as_bytes());
    }
    hasher.update([0x1f]);
    hasher.update(quantity.to_string().as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    format!("{MARKER_PREFIX}{}", &digest[..24])
}

fn event_description(first: i64, last: i64, quantity: i64, marker: &str) -> String {
    format!("备考记录\n起始: {first}\n结束: {last}\n数量: {quantity}\n\n{marker}")
}

pub(crate) fn parse_date(value: &str, field: &str) -> Result<Date, TrackerError> {
    let value = value.trim();
    let separator = if value.contains('/') { '/' } else { '-' };
    let parts: Vec<_> = value.split(separator).collect();
    if parts.len() != 3 {
        return Err(TrackerError::field(field, "日期格式必须为 YYYY-MM-DD"));
    }
    let year = parts[0]
        .parse::<i32>()
        .map_err(|_| TrackerError::field(field, "年份无效"))?;
    let month = parts[1]
        .parse::<u8>()
        .ok()
        .and_then(|value| Month::try_from(value).ok())
        .ok_or_else(|| TrackerError::field(field, "月份无效"))?;
    let day = parts[2]
        .parse::<u8>()
        .map_err(|_| TrackerError::field(field, "日期无效"))?;
    Date::from_calendar_date(year, month, day).map_err(|_| TrackerError::field(field, "日期无效"))
}

pub(crate) fn parse_time(value: &str, field: &str) -> Result<Time, TrackerError> {
    let value = value.trim();
    let parts: Vec<_> = value.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return Err(TrackerError::field(
            field,
            "时间格式必须为 H:MM、HH:MM 或 HH:MM:SS",
        ));
    }
    let hour = parts[0]
        .parse::<u8>()
        .map_err(|_| TrackerError::field(field, "小时无效"))?;
    let minute = parts[1]
        .parse::<u8>()
        .map_err(|_| TrackerError::field(field, "分钟无效"))?;
    let second = parts
        .get(2)
        .unwrap_or(&"0")
        .parse::<u8>()
        .map_err(|_| TrackerError::field(field, "秒无效"))?;
    Time::from_hms(hour, minute, second).map_err(|_| TrackerError::field(field, "时间无效"))
}

pub(crate) fn format_date(value: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        value.year(),
        u8::from(value.month()),
        value.day()
    )
}

pub(crate) fn format_time(value: Time) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        value.hour(),
        value.minute(),
        value.second()
    )
}

pub(crate) fn format_timestamp(value: OffsetDateTime) -> String {
    value
        .format(&time::format_description::well_known::Rfc3339)
        .expect("OffsetDateTime always formats as RFC 3339")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_event_is_canonical_and_keeps_the_v1_marker_algorithm() {
        let event = canonicalize_new_event(&EventInput {
            calendar_name: " 学习 ".into(),
            event_date: "2026-08-30".into(),
            start_time: "21:00".into(),
            end_time: "21:30".into(),
            title: " 资料分析 ".into(),
            first_item: 205,
            last_item: 218,
        })
        .unwrap();

        assert_eq!(event.calendar_name, "学习");
        assert_eq!(event.end_time, "21:30:00");
        assert_eq!(event.quantity, 14);
        assert!(event.marker.starts_with(MARKER_PREFIX));
        assert_eq!(event.marker.len(), MARKER_PREFIX.len() + 24);
    }

    #[test]
    fn accepts_arbitrary_date_and_duration() {
        let input = EventInput {
            calendar_name: "学习".into(),
            event_date: "2030-01-02".into(),
            start_time: "08:07:12".into(),
            end_time: "11:43:09".into(),
            title: "资料分析".into(),
            first_item: 1,
            last_item: 2,
        };
        let event = canonicalize_new_event(&input).unwrap();
        assert_eq!(event.event_date, "2030-01-02");
        assert_eq!(event.start_time, "08:07:12");
        assert_eq!(event.end_time, "11:43:09");
    }

    #[test]
    fn rejects_non_positive_duration() {
        let input = EventInput {
            calendar_name: "学习".into(),
            event_date: "2026-08-30".into(),
            start_time: "23:45".into(),
            end_time: "23:45".into(),
            title: "资料分析".into(),
            first_item: 1,
            last_item: 2,
        };
        assert!(matches!(
            canonicalize_new_event(&input),
            Err(TrackerError::Validation { .. })
        ));
    }
}
