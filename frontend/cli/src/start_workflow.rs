use std::io::{IsTerminal, Write};
use std::thread;
use std::time::{Duration as StdDuration, Instant};

use crossterm::{cursor::MoveToColumn, execute, terminal::Clear, terminal::ClearType};
use time::{Duration, OffsetDateTime};

use crate::api::TrackerApi;
use crate::types::{CreateAction, CreateEventCommand, EventInput};
use crate::workflow::{
    Prompter, TARGET_CALENDAR, WorkflowError, display_create_result, prompt_nonempty,
    prompt_result, sync_status_text,
};

const COUNTDOWN_DURATION: StdDuration = StdDuration::from_secs(30 * 60);

pub trait LocalClock {
    fn now_local(&mut self) -> Result<OffsetDateTime, String>;
}

pub struct SystemClock;

impl LocalClock for SystemClock {
    fn now_local(&mut self) -> Result<OffsetDateTime, String> {
        OffsetDateTime::now_local().map_err(|error| error.to_string())
    }
}

pub trait Countdown {
    fn wait(&mut self, duration: StdDuration) -> Result<(), String>;
}

pub struct TerminalCountdown;

impl Countdown for TerminalCountdown {
    fn wait(&mut self, duration: StdDuration) -> Result<(), String> {
        let deadline = Instant::now()
            .checked_add(duration)
            .ok_or_else(|| "无法计算倒计时结束时间".to_owned())?;
        let mut stdout = std::io::stdout();
        let is_terminal = stdout.is_terminal();
        let mut last_displayed = None;

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let seconds = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
            if is_terminal && last_displayed != Some(seconds) {
                execute!(stdout, MoveToColumn(0), Clear(ClearType::CurrentLine))
                    .map_err(|error| error.to_string())?;
                write!(stdout, "倒计时 {:02}:{:02}", seconds / 60, seconds % 60)
                    .and_then(|_| stdout.flush())
                    .map_err(|error| error.to_string())?;
                last_displayed = Some(seconds);
            }
            if remaining.is_zero() {
                break;
            }
            thread::sleep(remaining.min(StdDuration::from_secs(1)));
        }

        if is_terminal {
            writeln!(stdout).map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

pub fn run_start_event(
    api: &impl TrackerApi,
    prompter: &mut impl Prompter,
    clock: &mut impl LocalClock,
    countdown: &mut impl Countdown,
) -> Result<i32, WorkflowError> {
    let bootstrap = api.bootstrap()?;
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

    let estimated = event_window(clock.now_local().map_err(clock_error)?)?;
    prompter.display(&format!(
        "\n即将开始：\n  日历: {calendar_name}\n  预计时间: {} {} - {}\n  事件: {title}\n  起始编号: {first_item}",
        estimated.event_date, estimated.start_time, estimated.end_time,
    ));
    if !prompt_result(prompter.confirm("确认开始 30 分钟倒计时?", true))? {
        prompter.display("已取消，倒计时未开始，数据库未修改。");
        return Ok(0);
    }

    let actual = event_window(clock.now_local().map_err(clock_error)?)?;
    prompter.display(&format!(
        "倒计时已开始，事件时间为 {} {} - {}。",
        actual.event_date, actual.start_time, actual.end_time,
    ));
    countdown
        .wait(COUNTDOWN_DURATION)
        .map_err(|error| WorkflowError::Prompt(format!("倒计时失败: {error}")))?;
    prompter.display(&format!(
        "\n倒计时结束：\n  日历: {calendar_name}\n  时间: {} {} - {}\n  事件: {title}\n  起始编号: {first_item}",
        actual.event_date, actual.start_time, actual.end_time,
    ));

    let last_default = first_item.saturating_add(1);
    let last_item = prompt_result(prompter.stepped_integer("结束编号", last_default, first_item))?;
    let event = EventInput {
        calendar_name,
        event_date: actual.event_date,
        start_time: actual.start_time,
        end_time: actual.end_time,
        title,
        first_item,
        last_item,
    };
    let preview = api.preview(&event)?;
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

    let result = api.create(&CreateEventCommand {
        event,
        action: CreateAction::SaveAndSync,
        allow_duplicate,
    })?;
    display_create_result(prompter, result)
}

struct EventWindow {
    event_date: String,
    start_time: String,
    end_time: String,
}

fn event_window(start: OffsetDateTime) -> Result<EventWindow, WorkflowError> {
    let end = start
        .checked_add(Duration::minutes(30))
        .ok_or_else(|| WorkflowError::Prompt("无法计算预计结束时间".into()))?;
    if start.date() != end.date() {
        return Err(WorkflowError::Prompt(
            "当前 30 分钟事件会跨越午夜，start-event 暂不支持跨日事件".into(),
        ));
    }
    Ok(EventWindow {
        event_date: start.date().to_string(),
        start_time: format!(
            "{:02}:{:02}:{:02}",
            start.hour(),
            start.minute(),
            start.second()
        ),
        end_time: format!("{:02}:{:02}:{:02}", end.hour(), end.minute(), end.second()),
    })
}

fn clock_error(error: String) -> WorkflowError {
    WorkflowError::Prompt(format!("无法读取本地时间: {error}"))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::sync::Mutex;

    use time::{Date, Month, PrimitiveDateTime, Time, UtcOffset};

    use super::*;
    use crate::api::ApiError;
    use crate::types::{
        BootstrapView, CreateEventResult, EventPreview, EventSuggestion, EventView, SyncOutcome,
        SyncOutcomeStatus, SyncStatus,
    };
    use crate::workflow::PromptSignal;

    struct FakeApi {
        command: Mutex<Option<CreateEventCommand>>,
        preview_calls: Mutex<usize>,
        duplicate_status: Option<SyncStatus>,
    }

    impl FakeApi {
        fn new(duplicate_status: Option<SyncStatus>) -> Self {
            Self {
                command: Mutex::new(None),
                preview_calls: Mutex::new(0),
                duplicate_status,
            }
        }
    }

    impl TrackerApi for FakeApi {
        fn bootstrap(&self) -> Result<BootstrapView, ApiError> {
            Ok(BootstrapView {
                server_date: "2026-09-19".into(),
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
            *self.preview_calls.lock().unwrap() += 1;
            Ok(EventPreview {
                calendar_name: input.calendar_name.clone(),
                event_date: input.event_date.clone(),
                start_time: input.start_time.clone(),
                end_time: input.end_time.clone(),
                title: input.title.clone(),
                first_item: input.first_item,
                last_item: input.last_item,
                quantity: input.last_item - input.first_item + 1,
                duplicate_status: self.duplicate_status.clone(),
            })
        }

        fn create(&self, command: &CreateEventCommand) -> Result<CreateEventResult, ApiError> {
            *self.command.lock().unwrap() = Some(command.clone());
            Ok(CreateEventResult {
                event: EventView {
                    id: "event-id".into(),
                    calendar_name: command.event.calendar_name.clone(),
                    event_date: command.event.event_date.clone(),
                    start_time: command.event.start_time.clone(),
                    end_time: command.event.end_time.clone(),
                    title: command.event.title.clone(),
                    first_item: command.event.first_item,
                    last_item: command.event.last_item,
                    quantity: command.event.last_item - command.event.first_item + 1,
                    sync_status: SyncStatus::Synced,
                    sync_attempts: 1,
                    last_sync_error: None,
                },
                save_status: "created".into(),
                sync: SyncOutcome {
                    status: SyncOutcomeStatus::Succeeded,
                    action: None,
                    error: None,
                },
            })
        }
    }

    struct FakePrompter {
        texts: VecDeque<String>,
        integers: VecDeque<i64>,
        stepped_integers: VecDeque<i64>,
        confirms: VecDeque<bool>,
        messages: Vec<String>,
        events: Rc<RefCell<Vec<String>>>,
    }

    impl Prompter for FakePrompter {
        fn text(
            &mut self,
            message: &str,
            _default: Option<&str>,
            _suggestions: &[String],
        ) -> Result<String, PromptSignal> {
            self.events.borrow_mut().push(format!("text:{message}"));
            Ok(self.texts.pop_front().unwrap())
        }

        fn integer(
            &mut self,
            message: &str,
            _default: Option<i64>,
            _minimum: Option<i64>,
        ) -> Result<i64, PromptSignal> {
            self.events.borrow_mut().push(format!("integer:{message}"));
            Ok(self.integers.pop_front().unwrap())
        }

        fn stepped_integer(
            &mut self,
            message: &str,
            _default: i64,
            _minimum: i64,
        ) -> Result<i64, PromptSignal> {
            self.events
                .borrow_mut()
                .push(format!("stepped_integer:{message}"));
            Ok(self.stepped_integers.pop_front().unwrap())
        }

        fn confirm(&mut self, message: &str, _default: bool) -> Result<bool, PromptSignal> {
            self.events.borrow_mut().push(format!("confirm:{message}"));
            Ok(self.confirms.pop_front().unwrap())
        }

        fn display(&mut self, message: &str) {
            self.messages.push(message.into());
        }
    }

    struct FakeClock {
        times: VecDeque<OffsetDateTime>,
    }

    impl LocalClock for FakeClock {
        fn now_local(&mut self) -> Result<OffsetDateTime, String> {
            self.times.pop_front().ok_or_else(|| "没有测试时间".into())
        }
    }

    struct FakeCountdown {
        durations: Vec<StdDuration>,
        events: Rc<RefCell<Vec<String>>>,
    }

    impl Countdown for FakeCountdown {
        fn wait(&mut self, duration: StdDuration) -> Result<(), String> {
            self.durations.push(duration);
            self.events.borrow_mut().push("countdown".into());
            Ok(())
        }
    }

    fn local_time(hour: u8, minute: u8, second: u8) -> OffsetDateTime {
        PrimitiveDateTime::new(
            Date::from_calendar_date(2026, Month::September, 19).unwrap(),
            Time::from_hms(hour, minute, second).unwrap(),
        )
        .assume_offset(UtcOffset::from_hms(8, 0, 0).unwrap())
    }

    fn fixtures(
        confirms: impl IntoIterator<Item = bool>,
    ) -> (
        FakePrompter,
        FakeClock,
        FakeCountdown,
        Rc<RefCell<Vec<String>>>,
    ) {
        let events = Rc::new(RefCell::new(Vec::new()));
        (
            FakePrompter {
                texts: VecDeque::from(["资料分析".into()]),
                integers: VecDeque::from([205]),
                stepped_integers: VecDeque::from([218]),
                confirms: confirms.into_iter().collect(),
                messages: Vec::new(),
                events: Rc::clone(&events),
            },
            FakeClock {
                times: VecDeque::from([local_time(10, 5, 10), local_time(10, 5, 15)]),
            },
            FakeCountdown {
                durations: Vec::new(),
                events: Rc::clone(&events),
            },
            events,
        )
    }

    #[test]
    fn waits_thirty_minutes_before_asking_for_last_item_and_saves() {
        let api = FakeApi::new(None);
        let (mut prompter, mut clock, mut countdown, events) = fixtures([true]);

        assert_eq!(
            run_start_event(&api, &mut prompter, &mut clock, &mut countdown).unwrap(),
            0
        );

        assert_eq!(countdown.durations, vec![StdDuration::from_secs(1800)]);
        let events = events.borrow();
        let countdown_index = events
            .iter()
            .position(|event| event == "countdown")
            .unwrap();
        let last_item_index = events
            .iter()
            .position(|event| event == "stepped_integer:结束编号")
            .unwrap();
        assert!(countdown_index < last_item_index);

        let command = api.command.lock().unwrap().clone().unwrap();
        assert_eq!(command.event.calendar_name, "学习");
        assert_eq!(command.event.event_date, "2026-09-19");
        assert_eq!(command.event.start_time, "10:05:15");
        assert_eq!(command.event.end_time, "10:35:15");
        assert_eq!(command.event.first_item, 205);
        assert_eq!(command.event.last_item, 218);
        assert_eq!(command.action, CreateAction::SaveAndSync);
        assert!(!command.allow_duplicate);
        assert!(
            prompter
                .messages
                .iter()
                .any(|message| message.contains("预计时间: 2026-09-19 10:05:10 - 10:35:10"))
        );
        assert!(
            prompter
                .messages
                .iter()
                .any(|message| message.contains("倒计时结束") && message.contains("起始编号: 205"))
        );
    }

    #[test]
    fn declining_start_does_not_count_down_or_write() {
        let api = FakeApi::new(None);
        let (mut prompter, mut clock, mut countdown, _) = fixtures([false]);

        assert_eq!(
            run_start_event(&api, &mut prompter, &mut clock, &mut countdown).unwrap(),
            0
        );

        assert!(countdown.durations.is_empty());
        assert_eq!(*api.preview_calls.lock().unwrap(), 0);
        assert!(api.command.lock().unwrap().is_none());
        assert!(
            prompter
                .messages
                .iter()
                .any(|message| message.contains("倒计时未开始"))
        );
    }

    #[test]
    fn duplicate_still_requires_explicit_confirmation_after_countdown() {
        let api = FakeApi::new(Some(SyncStatus::Pending));
        let (mut prompter, mut clock, mut countdown, _) = fixtures([true, false]);

        assert_eq!(
            run_start_event(&api, &mut prompter, &mut clock, &mut countdown).unwrap(),
            0
        );

        assert_eq!(countdown.durations.len(), 1);
        assert!(api.command.lock().unwrap().is_none());
        assert!(
            prompter
                .messages
                .iter()
                .any(|message| message.contains("已跳过重复记录"))
        );
    }

    #[test]
    fn rejects_countdowns_that_would_cross_midnight() {
        let api = FakeApi::new(None);
        let (mut prompter, _, mut countdown, _) = fixtures([true]);
        let mut clock = FakeClock {
            times: VecDeque::from([local_time(23, 45, 0)]),
        };

        let error = run_start_event(&api, &mut prompter, &mut clock, &mut countdown).unwrap_err();

        assert!(error.to_string().contains("暂不支持跨日事件"));
        assert!(countdown.durations.is_empty());
        assert!(api.command.lock().unwrap().is_none());
    }
}
