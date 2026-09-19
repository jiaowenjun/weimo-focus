use std::fmt;

use inquire::{Autocomplete, Confirm, CustomUserError, InquireError, Text};
use thiserror::Error;
use time::{Duration, OffsetDateTime, PrimitiveDateTime, Time};

use crate::api::{ApiError, TrackerApi};
use crate::types::{CreateAction, CreateEventCommand, EventInput, SyncOutcomeStatus, SyncStatus};

mod numeric_prompt;

const SUGGESTION_LIMIT: usize = 10;
const TARGET_CALENDAR: &str = "学习";

#[derive(Debug, Error)]
pub enum WorkflowError {
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error("用户取消录入")]
    Cancelled,
    #[error("用户中断录入")]
    Interrupted,
    #[error("record-event 需要在交互式终端中运行")]
    NotInteractive,
    #[error("终端输入失败: {0}")]
    Prompt(String),
}

#[derive(Debug)]
pub enum PromptSignal {
    Cancelled,
    Interrupted,
    NotInteractive,
    Failed(String),
}

pub trait Prompter {
    fn text(
        &mut self,
        message: &str,
        default: Option<&str>,
        suggestions: &[String],
    ) -> Result<String, PromptSignal>;
    fn integer(
        &mut self,
        message: &str,
        default: Option<i64>,
        minimum: Option<i64>,
    ) -> Result<i64, PromptSignal>;
    fn stepped_integer(
        &mut self,
        message: &str,
        default: i64,
        minimum: i64,
    ) -> Result<i64, PromptSignal>;
    fn confirm(&mut self, message: &str, default: bool) -> Result<bool, PromptSignal>;
    fn display(&mut self, message: &str);
}

pub struct InquirePrompter;

impl Prompter for InquirePrompter {
    fn text(
        &mut self,
        message: &str,
        default: Option<&str>,
        suggestions: &[String],
    ) -> Result<String, PromptSignal> {
        let mut prompt = Text::new(message).with_page_size(SUGGESTION_LIMIT);
        if let Some(default) = default {
            prompt = prompt.with_default(default);
        }
        if !suggestions.is_empty() {
            prompt = prompt.with_autocomplete(HistoryCompleter::new(suggestions.to_vec()));
        }
        prompt.prompt().map_err(map_inquire_error)
    }

    fn integer(
        &mut self,
        message: &str,
        default: Option<i64>,
        minimum: Option<i64>,
    ) -> Result<i64, PromptSignal> {
        loop {
            let default_text = default.map(|value| value.to_string());
            let value = self.text(message, default_text.as_deref(), &[])?;
            match value.trim().parse::<i64>() {
                Ok(value) if minimum.is_none_or(|minimum| value >= minimum) => return Ok(value),
                Ok(_) => println!("输入不能小于 {}。", minimum.unwrap()),
                Err(_) => println!("请输入整数。"),
            }
        }
    }

    fn stepped_integer(
        &mut self,
        message: &str,
        default: i64,
        minimum: i64,
    ) -> Result<i64, PromptSignal> {
        numeric_prompt::prompt(message, default, minimum).map_err(|error| match error {
            numeric_prompt::PromptError::Cancelled => PromptSignal::Cancelled,
            numeric_prompt::PromptError::Interrupted => PromptSignal::Interrupted,
            numeric_prompt::PromptError::Io(error) => {
                PromptSignal::Failed(format!("终端数值输入失败: {error}"))
            }
        })
    }

    fn confirm(&mut self, message: &str, default: bool) -> Result<bool, PromptSignal> {
        Confirm::new(message)
            .with_default(default)
            .prompt()
            .map_err(map_inquire_error)
    }

    fn display(&mut self, message: &str) {
        println!("{message}");
    }
}

pub fn run_entry(
    api: &impl TrackerApi,
    prompter: &mut impl Prompter,
) -> Result<i32, WorkflowError> {
    let bootstrap = api.bootstrap()?;
    let (event_date, start_time, end_time) = default_event_times()?;
    let calendar_name = TARGET_CALENDAR.to_owned();
    let title = prompt_nonempty(
        prompter,
        "事件",
        bootstrap.titles.first().map(String::as_str),
        &bootstrap.titles,
    )?;
    let first_default = bootstrap
        .recent_suggestions
        .iter()
        .find(|suggestion| suggestion.calendar_name == TARGET_CALENDAR && suggestion.title == title)
        .map(|suggestion| suggestion.next_first_item);
    let first_item = prompt_result(prompter.integer("起始编号", first_default, None))?;
    let last_default = first_item.saturating_add(1);
    let last_item = prompt_result(prompter.stepped_integer("结束编号", last_default, first_item))?;
    let event = EventInput {
        calendar_name,
        event_date,
        start_time,
        end_time,
        title,
        first_item,
        last_item,
    };
    let preview = api.preview(&event)?;
    prompter.display(&format!(
        "\n将保存：\n  日历: {}\n  时间: {} {} - {}\n  事件: {}\n  范围: {} - {}（数量 {}）",
        preview.calendar_name,
        preview.event_date,
        preview.start_time,
        preview.end_time,
        preview.title,
        preview.first_item,
        preview.last_item,
        preview.quantity,
    ));
    let allow_duplicate = if let Some(status) = preview.duplicate_status {
        prompter.display(&format!(
            "相同记录已经存在（{}）。",
            sync_status_text(status)
        ));
        if !prompt_result(prompter.confirm("仍然重复写入?", false))? {
            prompter.display("已跳过重复记录，数据库未修改。");
            return Ok(0);
        }
        true
    } else {
        false
    };
    if !prompt_result(prompter.confirm("确认保存?", true))? {
        prompter.display("已取消，数据库未修改。");
        return Ok(0);
    }
    let result = api.create(&CreateEventCommand {
        event,
        action: CreateAction::SaveAndSync,
        allow_duplicate,
    })?;
    match result.sync.status {
        SyncOutcomeStatus::NotRequested => {
            prompter.display(&format!("已保存为待同步事件：{}", result.event.id));
            Ok(0)
        }
        SyncOutcomeStatus::Succeeded => {
            prompter.display(&format!("已保存并同步到 Calendar：{}", result.event.id));
            Ok(0)
        }
        SyncOutcomeStatus::AlreadySynced => {
            prompter.display(&format!("事件已经同步：{}", result.event.id));
            Ok(0)
        }
        SyncOutcomeStatus::Failed => {
            let detail = result
                .sync
                .error
                .map(|error| format!("{}: {}", error.code, error.message))
                .unwrap_or_else(|| "未知 Calendar 错误".into());
            prompter.display(&format!(
                "事件已保存但尚未同步：{}\n{}\n事件保留为待同步状态。",
                result.event.id, detail
            ));
            Ok(1)
        }
    }
}

fn default_event_times() -> Result<(String, String, String), WorkflowError> {
    let now = OffsetDateTime::now_local()
        .map_err(|error| WorkflowError::Prompt(format!("无法读取本地时间: {error}")))?;
    let mut start = now
        .checked_sub(Duration::minutes(30))
        .ok_or_else(|| WorkflowError::Prompt("无法计算默认开始时间".into()))?;
    if start.date() != now.date() {
        start = PrimitiveDateTime::new(now.date(), Time::MIDNIGHT).assume_offset(now.offset());
    }
    let start_minute = (start.minute() / 10) * 10;
    start = start.replace_time(
        Time::from_hms(start.hour(), start_minute, 0)
            .map_err(|error| WorkflowError::Prompt(format!("无法计算默认开始时间: {error}")))?,
    );
    let end = start
        .checked_add(Duration::minutes(30))
        .ok_or_else(|| WorkflowError::Prompt("无法计算默认结束时间".into()))?;
    Ok((
        start.date().to_string(),
        format!("{:02}:{:02}", start.hour(), start.minute()),
        format!("{:02}:{:02}", end.hour(), end.minute()),
    ))
}

fn prompt_nonempty(
    prompter: &mut impl Prompter,
    message: &str,
    default: Option<&str>,
    suggestions: &[String],
) -> Result<String, WorkflowError> {
    loop {
        let value = prompt_result(prompter.text(message, default, suggestions))?;
        let value = value.trim();
        if !value.is_empty() {
            return Ok(value.to_owned());
        }
        prompter.display("输入不能为空。");
    }
}

fn prompt_result<T>(result: Result<T, PromptSignal>) -> Result<T, WorkflowError> {
    match result {
        Ok(value) => Ok(value),
        Err(PromptSignal::Cancelled) => Err(WorkflowError::Cancelled),
        Err(PromptSignal::Interrupted) => Err(WorkflowError::Interrupted),
        Err(PromptSignal::NotInteractive) => Err(WorkflowError::NotInteractive),
        Err(PromptSignal::Failed(message)) => Err(WorkflowError::Prompt(message)),
    }
}

fn sync_status_text(status: SyncStatus) -> &'static str {
    match status {
        SyncStatus::Pending => "待同步",
        SyncStatus::Synced => "已同步",
    }
}

fn map_inquire_error(error: InquireError) -> PromptSignal {
    match error {
        InquireError::OperationCanceled => PromptSignal::Cancelled,
        InquireError::OperationInterrupted => PromptSignal::Interrupted,
        InquireError::NotTTY => PromptSignal::NotInteractive,
        other => PromptSignal::Failed(other.to_string()),
    }
}

#[derive(Clone)]
struct HistoryCompleter {
    candidates: Vec<String>,
}

impl HistoryCompleter {
    fn new(candidates: Vec<String>) -> Self {
        Self { candidates }
    }
}

impl fmt::Debug for HistoryCompleter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HistoryCompleter")
            .field("candidates", &self.candidates.len())
            .finish()
    }
}

impl Autocomplete for HistoryCompleter {
    fn get_suggestions(&mut self, input: &str) -> Result<Vec<String>, CustomUserError> {
        let normalized = input.to_lowercase();
        let mut prefix = Vec::new();
        let mut contains = Vec::new();
        for candidate in &self.candidates {
            let candidate_normalized = candidate.to_lowercase();
            if candidate_normalized.starts_with(&normalized) {
                prefix.push(candidate.clone());
            } else if candidate_normalized.contains(&normalized) {
                contains.push(candidate.clone());
            }
        }
        Ok(prefix
            .into_iter()
            .chain(contains)
            .take(SUGGESTION_LIMIT)
            .collect())
    }

    fn get_completion(
        &mut self,
        input: &str,
        highlighted_suggestion: Option<String>,
    ) -> Result<Option<String>, CustomUserError> {
        if highlighted_suggestion.is_some() {
            return Ok(highlighted_suggestion);
        }
        let matches = self.get_suggestions(input)?;
        Ok((matches.len() == 1).then(|| matches[0].clone()))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use crate::types::{
        BootstrapView, CreateEventResult, EventPreview, EventSuggestion, EventView, OperationError,
        SyncOutcome,
    };

    struct FakeApi {
        command: Mutex<Option<CreateEventCommand>>,
    }

    impl TrackerApi for FakeApi {
        fn bootstrap(&self) -> Result<crate::types::BootstrapView, ApiError> {
            Ok(BootstrapView {
                server_date: "2026-08-30".into(),
                calendar_names: vec!["学习".into()],
                titles: vec!["资料分析".into()],
                recent_suggestions: vec![EventSuggestion {
                    calendar_name: "学习".into(),
                    title: "资料分析".into(),
                    next_first_item: 205,
                }],
                pending_count: 0,
            })
        }

        fn preview(&self, input: &EventInput) -> Result<EventPreview, ApiError> {
            Ok(EventPreview {
                calendar_name: input.calendar_name.clone(),
                event_date: input.event_date.clone(),
                start_time: "21:00:00".into(),
                end_time: "21:30:00".into(),
                title: input.title.clone(),
                first_item: input.first_item,
                last_item: input.last_item,
                quantity: input.last_item - input.first_item + 1,
                duplicate_status: Some(SyncStatus::Pending),
            })
        }

        fn create(&self, command: &CreateEventCommand) -> Result<CreateEventResult, ApiError> {
            *self.command.lock().unwrap() = Some(command.clone());
            Ok(CreateEventResult {
                event: EventView {
                    id: "event-id".into(),
                    calendar_name: command.event.calendar_name.clone(),
                    event_date: command.event.event_date.clone(),
                    start_time: "21:00:00".into(),
                    end_time: "21:30:00".into(),
                    title: command.event.title.clone(),
                    first_item: command.event.first_item,
                    last_item: command.event.last_item,
                    quantity: 14,
                    sync_status: SyncStatus::Pending,
                    sync_attempts: 1,
                    last_sync_error: Some(OperationError {
                        code: "calendar_missing".into(),
                        message: "missing".into(),
                    }),
                },
                save_status: "created".into(),
                sync: SyncOutcome {
                    status: SyncOutcomeStatus::Failed,
                    action: None,
                    error: Some(OperationError {
                        code: "calendar_missing".into(),
                        message: "missing".into(),
                    }),
                },
            })
        }
    }

    struct FakePrompter {
        texts: VecDeque<String>,
        text_calls: Vec<String>,
        integers: VecDeque<i64>,
        stepped_integers: VecDeque<i64>,
        stepped_integer_calls: Vec<(String, i64, i64)>,
        confirms: VecDeque<bool>,
        messages: Vec<String>,
    }

    impl Prompter for FakePrompter {
        fn text(
            &mut self,
            message: &str,
            _default: Option<&str>,
            _suggestions: &[String],
        ) -> Result<String, PromptSignal> {
            self.text_calls.push(message.into());
            Ok(self.texts.pop_front().unwrap())
        }

        fn integer(
            &mut self,
            _message: &str,
            _default: Option<i64>,
            _minimum: Option<i64>,
        ) -> Result<i64, PromptSignal> {
            Ok(self.integers.pop_front().unwrap())
        }

        fn stepped_integer(
            &mut self,
            message: &str,
            default: i64,
            minimum: i64,
        ) -> Result<i64, PromptSignal> {
            self.stepped_integer_calls
                .push((message.to_owned(), default, minimum));
            Ok(self.stepped_integers.pop_front().unwrap())
        }

        fn confirm(&mut self, _message: &str, _default: bool) -> Result<bool, PromptSignal> {
            Ok(self.confirms.pop_front().unwrap())
        }

        fn display(&mut self, message: &str) {
            self.messages.push(message.into());
        }
    }

    #[test]
    fn saved_but_unsynced_is_nonzero_and_duplicate_override_is_explicit() {
        let api = FakeApi {
            command: Mutex::new(None),
        };
        let mut prompter = FakePrompter {
            texts: VecDeque::from(["资料分析".into()]),
            text_calls: Vec::new(),
            integers: VecDeque::from([205]),
            stepped_integers: VecDeque::from([218]),
            stepped_integer_calls: Vec::new(),
            confirms: VecDeque::from([true, true]),
            messages: Vec::new(),
        };

        assert_eq!(run_entry(&api, &mut prompter).unwrap(), 1);
        let command = api.command.lock().unwrap().clone().unwrap();
        assert!(command.allow_duplicate);
        assert_eq!(command.action, CreateAction::SaveAndSync);
        let minute = command
            .event
            .start_time
            .split(':')
            .nth(1)
            .unwrap()
            .parse::<u8>()
            .unwrap();
        assert_eq!(minute % 10, 0);
        assert!(!command.event.end_time.is_empty());
        assert_eq!(prompter.text_calls, vec!["事件"]);
        assert_eq!(
            prompter.stepped_integer_calls,
            vec![("结束编号".into(), 206, 205)]
        );
        assert!(
            prompter
                .messages
                .iter()
                .any(|message| message.contains("已保存但尚未同步"))
        );
    }
}
