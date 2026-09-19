use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Eq, PartialEq)]
pub struct BootstrapView {
    pub server_date: String,
    pub calendar_names: Vec<String>,
    pub titles: Vec<String>,
    pub recent_suggestions: Vec<EventSuggestion>,
    pub pending_count: i64,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq)]
pub struct EventSuggestion {
    pub calendar_name: String,
    pub title: String,
    pub next_first_item: i64,
}

#[derive(Debug, Clone, Serialize, Eq, PartialEq)]
pub struct EventInput {
    pub calendar_name: String,
    pub event_date: String,
    pub start_time: String,
    pub end_time: String,
    pub title: String,
    pub first_item: i64,
    pub last_item: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CreateAction {
    Save,
    SaveAndSync,
}

#[derive(Debug, Clone, Serialize, Eq, PartialEq)]
pub struct CreateEventCommand {
    #[serde(flatten)]
    pub event: EventInput,
    pub action: CreateAction,
    pub allow_duplicate: bool,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SyncStatus {
    Pending,
    Synced,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq)]
pub struct OperationError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq)]
pub struct EventPreview {
    pub calendar_name: String,
    pub event_date: String,
    pub start_time: String,
    pub end_time: String,
    pub title: String,
    pub first_item: i64,
    pub last_item: i64,
    pub quantity: i64,
    pub duplicate_status: Option<SyncStatus>,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq)]
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
    pub last_sync_error: Option<OperationError>,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CalendarAction {
    Created,
    Existing,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SyncOutcomeStatus {
    NotRequested,
    Succeeded,
    Failed,
    AlreadySynced,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq)]
pub struct SyncOutcome {
    pub status: SyncOutcomeStatus,
    pub action: Option<CalendarAction>,
    pub error: Option<OperationError>,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq)]
pub struct CreateEventResult {
    pub event: EventView,
    pub save_status: String,
    pub sync: SyncOutcome,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq)]
pub struct ErrorEnvelope {
    pub error: ApiErrorBody,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq)]
pub struct ApiErrorBody {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
}
